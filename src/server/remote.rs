//! `imux <command> ...`: commands sent to the running server by name, from a script or a shell,
//! with targets (`-t`) and output (`-F`, `-p`).
//!
//! A command that names a place (`list-windows -t work`, `send-keys -t %3`) is carried out here
//! on the server's state, with no client needed. Any other command of the command prompt is run
//! for a client that looks at the session. Who is asking matters: a program that runs inside a
//! pane (the caller's process descends from the pane's shell) may look but not touch, unless the
//! option `command-from-panes` says more (`spawned`: only the panes it spawned; `on`: as the
//! client; see `spawned.rs`), because a program in a pane (an agent that was talked into
//! something, a package script) must not be able to type into the other panes.

use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;

use super::spawned::{self, Power};
use super::Server;
use crate::config::format;
use crate::copymode::Op;
use crate::errcode::tag;
use crate::input::keys::{self, Key};
use crate::input::{bindings::Command, commands};
use crate::log;
use crate::proto::{RunArgs, ServerMsg};
use crate::session::layout::{Direction, PaneId};
use crate::session::{Pane, Session, Window};
use crate::target::{self, Kind, Part};
use crate::tty::proc;

/// A pane and where it is: session and window positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Place {
    pub si: usize,
    pub wi: usize,
    pub pane: PaneId,
}

/// The value of a format variable for a pane: its variables (`vars`), then a user option, an option
/// name, and an environment name. A free function over the fields it reads, so a caller can keep a
/// closure over it while it changes other parts of the server. Menu and popup titles and positions
/// use it as `-F` formats do.
pub(super) fn lookup_one(
    options: &crate::config::options::Options,
    pane_env: &[(String, Option<String>)],
    vars: &HashMap<&'static str, String>,
    name: &str,
) -> Option<String> {
    vars.get(name)
        .cloned()
        .or_else(|| options.user(name).or_else(|| options.value_text(name)).map(str::to_string))
        .or_else(|| super::environ::environment_in(pane_env, name))
}

/// The flags and words of one command line.
#[derive(Default, Debug)]
pub(super) struct Flags {
    values: HashMap<char, String>,
    /// Every value a valued flag was given, in order (`-e A=1 -e B=2`).
    every: Vec<(char, String)>,
    on: HashSet<char>,
    pub words: Vec<String>,
}

impl Flags {
    /// All the values of a flag given more than once.
    pub fn values_of(&self, flag: char) -> Vec<&str> {
        self.every.iter().filter(|(c, _)| *c == flag).map(|(_, v)| v.as_str()).collect()
    }

    pub fn has(&self, flag: char) -> bool {
        self.on.contains(&flag)
    }

    pub fn value(&self, flag: char) -> Option<&str> {
        self.values.get(&flag).map(String::as_str)
    }
}

/// Read flags in front of the words: `valued` lists the flags that take the next word (or the
/// rest of their own word, `-tfoo`), `plain` the ones that stand alone and may be grouped (`-dh`).
/// `--` ends the flags.
pub(super) fn flags(name: &str, args: &[String], valued: &str, plain: &str) -> Result<Flags, String> {
    let mut out = Flags::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--" {
            out.words.extend(it.by_ref().cloned());
            break;
        }
        let Some(letters) = arg.strip_prefix('-').filter(|l| !l.is_empty()) else {
            // The first word ends the flags, so `new-window sh -c 'cmd'` keeps its own `-c`.
            out.words.push(arg.clone());
            out.words.extend(it.by_ref().cloned());
            break;
        };
        for (at, c) in letters.char_indices() {
            if valued.contains(c) {
                let attached = &letters[at + c.len_utf8()..];
                let value = if attached.is_empty() {
                    it.next().cloned().ok_or_else(|| format!("{name}: -{c} needs a value"))?
                } else {
                    attached.to_string()
                };
                out.every.push((c, value.clone()));
                out.values.insert(c, value);
                break;
            } else if plain.contains(c) {
                out.on.insert(c);
            } else {
                return Err(format!("{name}: unknown flag -{c}"));
            }
        }
    }
    Ok(out)
}

/// The first and last line (positions in scrollback followed by the screen) of a capture, where
/// `start` and `end` are as `capture-pane -S/-E` takes them: 0 is the first line on the screen,
/// negative numbers go back into the history, `-` means the very start or the very end.
fn capture_range(history: usize, rows: usize, start: Option<&str>, end: Option<&str>) -> Result<(usize, usize), String> {
    let total = history + rows;
    let at = |text: Option<&str>, default: isize, edge: usize| -> Result<usize, String> {
        match text {
            None => Ok((history as isize + default).clamp(0, total as isize - 1) as usize),
            Some("-") => Ok(edge),
            Some(n) => {
                let n: isize = n.parse().map_err(|_| format!("capture-pane: {n:?} is not a line number"))?;
                Ok((history as isize + n).clamp(0, total as isize - 1) as usize)
            }
        }
    };
    let first = at(start, 0, 0)?;
    let last = at(end, rows as isize - 1, total - 1)?;
    Ok((first.min(last), first.max(last)))
}

/// An option value as `set` would take it back: quoted when it is empty or holds a blank or a quote.
fn option_text(value: &str) -> String {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '#' | '$' | '\\' | ';')) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// What `capture-pane` leaves in or out of the text.
#[derive(Clone, Copy, Default)]
struct CaptureStyle {
    /// `-J`: a row that wraps onto the next one is not ended.
    join: bool,
    /// `-N`: the blanks at the end of a row stay.
    keep_blanks: bool,
    /// `-e`: colors and attributes as escape sequences.
    escapes: bool,
}

/// A line of text from a row, without trailing blanks unless asked to keep them.
fn row_text(row: &crate::term::cell::Row, how: CaptureStyle) -> String {
    use crate::term::cell::{Cell, Style};
    let mut cells: &[Cell] = &row.cells;
    if !how.keep_blanks {
        let blank = |c: &Cell| (c.ch == ' ' || c.ch == '\0') && c.mark == 0 && (c.style == Style::default() || !how.escapes) && c.tile == 0;
        while cells.last().is_some_and(blank) {
            cells = &cells[..cells.len() - 1];
        }
    }
    if !how.escapes {
        return Cell::text_for_copy(cells);
    }
    let mut out = String::new();
    let mut at = Style::default();
    for c in cells.iter().filter(|c| c.width != 0) {
        if c.style != at {
            crate::render::sgr(&mut out, &c.style, true);
            at = c.style;
        }
        if c.ch == '\0' {
            out.push(' ');
        } else {
            c.push_to(&mut out);
        }
    }
    if at != Style::default() {
        out.push_str("\x1b[0m");
    }
    out
}

/// The plain text a pane shows (`visible`) or, with `history` more, the last `history` lines
/// of what scrolled off above it too; wrapped rows are joined. What `pane wait-output` searches.
pub(super) fn searchable_text(pane: &Pane, visible_only: bool, history: usize) -> String {
    let (past, rows) = capture_size(pane);
    let first = if visible_only { past } else { past.saturating_sub(history) };
    capture_text(pane, first, past + rows - 1, CaptureStyle { join: true, ..CaptureStyle::default() })
}

/// The text of lines `first..=last` of a pane: the history, then the screen.
fn capture_text(pane: &Pane, first: usize, last: usize, how: CaptureStyle) -> String {
    let g = &pane.term.grid;
    let history = if g.is_alt() { 0 } else { g.scrollback.len() };
    let mut out = String::new();
    for line in first..=last {
        let row = if line < history { g.scrollback.get(line) } else { g.screen.get(line - history) };
        let Some(row) = row else { continue };
        out.push_str(&row_text(row, how));
        if !(how.join && row.wrapped) {
            out.push('\n');
        }
    }
    out
}

/// How many lines the history and the screen of a pane hold in all.
fn capture_size(pane: &Pane) -> (usize, usize) {
    let g = &pane.term.grid;
    (if g.is_alt() { 0 } else { g.scrollback.len() }, g.rows)
}

/// `#{C:text}`: the number of the first row of the pane's screen (counted from 1) that holds `text`
/// (a regular expression with `regex`), or 0. A pattern that does not compile finds nothing.
fn search_screen(pane: &Pane, text: &str, regex: bool, ignore_case: bool) -> usize {
    let re = if regex {
        match regex_lite::RegexBuilder::new(text).case_insensitive(ignore_case).build() {
            Ok(re) => Some(re),
            Err(_) => return 0,
        }
    } else {
        None
    };
    let needle = if ignore_case { text.to_lowercase() } else { text.to_string() };
    let g = &pane.term.grid;
    for row in 0..g.rows {
        let Some(r) = g.screen.get(row) else { continue };
        let line = row_text(r, CaptureStyle::default());
        let hit = match &re {
            Some(re) => re.is_match(&line),
            None if ignore_case => line.to_lowercase().contains(&needle),
            None => line.contains(&needle),
        };
        if hit {
            return row + 1;
        }
    }
    0
}

/// Commands carried out here, by every name they go by.
const REMOTE: &[&str] = &[
    "list-sessions", "list-windows", "lsw", "list-panes", "lsp", "list-clients", "lsc", "has-session", "has", "display-message", "display",
    "capture-pane", "capturep", "send-keys", "send", "kill-session", "kill-window", "killw", "kill-pane", "killp", "select-window",
    "selectw", "select-pane", "selectp", "rename-session", "rename", "rename-window", "renamew", "new-session", "new-window", "neww",
    "split-window", "splitw", "set-environment", "setenv", "show-environment", "showenv", "set-option", "set", "set-window-option", "setw", "bind-key", "bind", "unbind-key", "unbind", "source-file",
    "source", "respawn-pane", "respawnp", "respawn-window", "respawnw", "join-pane", "joinp", "move-pane", "movep", "wait-for", "wait", "pipe-pane", "pipep", "list-keys", "lsk", "show-options", "show", "show-window-options", "showw",
    "set-hook", "show-hooks", "plugin-reload", "select-layout", "selectl", "resize-pane", "resizep", "move-window", "movew", "swap-window", "swapw",
    "detach-client", "detach", "swap-pane", "swapp", "unlink-window", "unlinkw", "copy-mode",
    "api-agent-list", "api-agent-get", "api-pane-run", "api-pane-wait",
    "api-agent-name", "api-agent-resolve", "api-agent-start", "api-seq",
];

/// The flags of `new-window`, `split-window` and `select-window` (those that take a value, then
/// those that stand alone): the one table the handlers and the key parser read, so a key can be
/// bound to exactly the forms the socket takes.
pub(super) const NEW_WINDOW_FLAGS: (&str, &str) = ("tnFce", "dPa");
pub(super) const SPLIT_WINDOW_FLAGS: (&str, &str) = ("tFclpe", "hvdPbf");
pub(super) const SELECT_WINDOW_FLAGS: (&str, &str) = ("t", "lnpT");
pub(super) const SELECT_PANE_FLAGS: (&str, &str) = ("tT", "LRUDlmMZde");

/// Whether a key may run `name` through the socket's handler: one of the commands above that
/// answers at once (`wait-for` waits for another command, and the bind and source commands have
/// their own forms).
pub fn runs_from_a_key(name: &str) -> bool {
    REMOTE.contains(&name) && !matches!(name, "wait-for" | "wait" | "plugin-reload" | "api-pane-wait" | "api-agent-start")
}

/// Whether `args` are flags the socket's handler of `name` takes, for the commands whose typed
/// form takes fewer.
pub fn bound_flags_fit(name: &str, args: &[String]) -> bool {
    let (valued, plain) = match name {
        "new-window" | "neww" => NEW_WINDOW_FLAGS,
        "split-window" | "splitw" => SPLIT_WINDOW_FLAGS,
        "select-window" | "selectw" => SELECT_WINDOW_FLAGS,
        "kill-window" | "killw" | "kill-pane" | "killp" => ("t", "a"),
        "unlink-window" | "unlinkw" => ("t", "ak"),
        "copy-mode" if !args.is_empty() => ("t", "eHuq"),
        "detach-client" | "detach" if args.iter().any(|a| a.starts_with("-t") || a.starts_with("-s")) => ("ts", "a"),
        "swap-pane" | "swapp" if args.iter().any(|a| a.starts_with("-t") || a.starts_with("-s")) => ("st", "dZ"),
        "select-pane" | "selectp" if args.iter().any(|a| a == "-t") => SELECT_PANE_FLAGS,
        "select-layout" | "selectl" if args.iter().any(|a| a == "-t" || crate::session::layout::looks_like_layout(a)) => ("t", "En"),
        "resize-pane" | "resizep" if args.iter().any(|a| a == "-M") => ("txy", "LRUDZM"),
        "resize-pane" | "resizep" if args.iter().any(|a| a.starts_with("-t") || a.starts_with("-x") || a.starts_with("-y")) => ("txy", "LRUDZ"),
        "move-window" | "movew" if args.iter().any(|a| a.contains(':') || matches!(a.as_str(), "-a" | "-k" | "-r")) => ("st", "dakr"),
        "swap-window" | "swapw" if args.iter().any(|a| a.contains(':')) => ("st", "d"),
        _ => return false,
    };
    let takes_words = matches!(name, "new-window" | "neww" | "split-window" | "splitw" | "select-layout" | "selectl" | "resize-pane" | "resizep");
    flags(name, args, valued, plain).is_ok_and(|f| takes_words || f.words.is_empty())
}

/// The table above, for the test that every command has a decision for plugins.
#[cfg(test)]
pub fn names() -> &'static [&'static str] {
    REMOTE
}

/// Whether `name` is one of the socket's own commands, by an exact name.
pub fn knows_exact(name: &str) -> bool {
    REMOTE.contains(&name)
}

/// Whether `imux <name> ...` is a command the server can run: one of the above, or one of the
/// commands of the command prompt.
pub fn knows(name: &str) -> bool {
    REMOTE.contains(&name) || !commands::parse_words_exact(&[name.to_string()]).is_err_and(|e| e.starts_with("unknown command"))
}

/// Whether a word could be the name of a `command-alias`: letters, digits and `-`, starting with a
/// letter. The server decides whether it is one.
pub fn alias_shaped(word: &str) -> bool {
    word.len() <= 32 && word.starts_with(|c: char| c.is_ascii_alphabetic()) && word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Commands that only look. Anything else is refused to a caller inside a pane unless
/// `command-from-panes` is on.
pub(super) fn is_read_only(command: &str) -> bool {
    matches!(
        command,
        "list-sessions"
            | "ls"
            | "list-windows"
            | "lsw"
            | "list-panes"
            | "lsp"
            | "list-clients"
            | "lsc"
            | "has-session"
            | "has"
            | "list-keys"
            | "lsk"
            | "show-hooks"
            | "api-agent-list"
            | "api-agent-get"
            | "api-agent-resolve"
            | "api-seq"
    )
}

impl Server {
    /// Give every window a number for `@3` targets; new ones get the next.
    pub(super) fn number_windows(&mut self) {
        for w in self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).filter(|w| w.id == 0) {
            w.id = self.next_window;
            self.next_window += 1;
        }
    }

    /// The pane a process runs in, found by walking up from the peer of the connection.
    pub(super) fn pane_of_peer(&self, client: usize) -> Option<PaneId> {
        let peer = self.clients.get(&client).and_then(|c| c.peer)?;
        self.sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| {
                // The program of a popup the user opened is the user, not a pane.
                w.panes.iter().filter(move |(id, _)| w.popup.as_ref().is_none_or(|p| p.pane != **id || p.origin.is_some()))
            })
            .find(|(_, p)| proc::descends_from(peer, p.pty.child.as_raw()))
            .map(|(id, _)| *id)
    }

    pub(super) fn place_of_pane(&self, id: PaneId) -> Option<Place> {
        self.sessions.iter().enumerate().find_map(|(si, s)| {
            s.windows.iter().position(|w| w.panes.contains_key(&id)).map(|wi| Place { si, wi, pane: id })
        })
    }

    /// Where a command without a target works: the pane of the caller, else the active pane of
    /// the current window of the session used last.
    pub(super) fn default_place(&self, caller: Option<PaneId>) -> Result<Place, String> {
        if let Some(place) = caller.and_then(|p| self.place_of_pane(p)) {
            return Ok(place);
        }
        let si = self.session_index(self.recent).or(if self.sessions.is_empty() { None } else { Some(0) }).ok_or("no sessions")?;
        let win = self.sessions[si].window();
        Ok(Place { si, wi: self.sessions[si].current, pane: win.active })
    }

    fn session_by_spec(&self, spec: &str) -> Result<usize, String> {
        if let Some(si) = self.sessions.iter().position(|s| s.name == spec) {
            return Ok(si);
        }
        if let Part::Exact(name) = target::reference(spec) {
            return self.sessions.iter().position(|s| s.name == name).ok_or_else(|| format!("can't find session: {spec}"));
        }
        if let Part::Id('$', n) = target::reference(spec) {
            return self.sessions.iter().position(|s| s.id == n).ok_or_else(|| format!("can't find session: {spec}"));
        }
        target::pick_name(self.sessions.iter().map(|s| s.name.as_str()), spec, "session")
    }

    fn window_in(&self, si: usize, spec: &str) -> Result<usize, String> {
        let session = &self.sessions[si];
        let count = session.windows.len();
        let base = self.base_index();
        match target::reference(spec) {
            Part::Number(n) => n.checked_sub(base).filter(|i| *i < count).ok_or_else(|| format!("can't find window: {spec}")),
            Part::Id('@', n) => session.windows.iter().position(|w| w.id == n).ok_or_else(|| format!("can't find window: {spec}")),
            Part::Id(..) => Err(format!("can't find window: {spec}")),
            Part::Name(name) => target::pick_name(session.windows.iter().map(|w| w.name.as_str()), &name, "window"),
            Part::Exact(name) => session.windows.iter().position(|w| w.name == name).ok_or_else(|| format!("can't find window: {spec}")),
            other => target::step(&other, session.current, session.last_index(), count).ok_or_else(|| format!("can't find window: {spec}")),
        }
    }

    /// The part of the screen the panes of session `si` fill, for targets that need geometry.
    fn window_area(&self, si: usize) -> Result<crate::session::layout::Rect, String> {
        let size = self.size_of(si).ok_or("no screen size yet")?;
        Ok(self.pane_area(size, self.sessions[si].status_rows))
    }

    fn pane_in(&self, si: usize, wi: usize, spec: &str) -> Result<PaneId, String> {
        let win = &self.sessions[si].windows[wi];
        let order = win.layout.panes();
        let current = order.iter().position(|&p| p == win.active).unwrap_or(0);
        let last = win.last_pane().and_then(|l| order.iter().position(|&p| p == l));
        let base = self.pane_base_index();
        match target::reference(spec) {
            Part::Number(n) => n.checked_sub(base).and_then(|i| order.get(i)).copied().ok_or_else(|| format!("can't find pane: {spec}")),
            Part::Id('%', n) => u32::try_from(n)
                .ok()
                .filter(|id| win.panes.contains_key(id))
                .ok_or_else(|| format!("can't find pane: {spec}")),
            Part::Id(..) | Part::Name(_) | Part::Exact(_) => Err(format!("can't find pane: {spec}")),
            Part::Beside(dir) => {
                let area = self.window_area(si)?;
                let dir = match dir {
                    target::Dir::Up => Direction::Up,
                    target::Dir::Down => Direction::Down,
                    target::Dir::Left => Direction::Left,
                    target::Dir::Right => Direction::Right,
                };
                win.layout.neighbor(win.active, dir, area).ok_or_else(|| format!("can't find pane: {spec}"))
            }
            Part::Edge(v, h) => {
                let area = self.window_area(si)?;
                win.layout.at_edge(v, h, area).ok_or_else(|| format!("can't find pane: {spec}"))
            }
            other => target::step(&other, current, last, order.len()).and_then(|i| order.get(i)).copied().ok_or_else(|| format!("can't find pane: {spec}")),
        }
    }

    /// Turn a `-t` argument into a place. An empty one is the default place.
    pub(super) fn resolve(&self, spec: &str, kind: Kind, caller: Option<PaneId>) -> Result<Place, String> {
        // `agent:NAME` is the pane of the agent of that name, for a target of any kind (the window
        // or session of that pane when the command wants one). It never falls back to a session
        // or window that happens to be called `agent`.
        if let Some(name) = spec.strip_prefix("agent:") {
            return self.place_of_agent(name);
        }
        let t = target::parse(spec, kind)?;
        let def = self.default_place(caller)?;
        if let Some(Part::Id('%', id)) = t.pane.as_deref().filter(|_| t.session.is_none() && t.window.is_none()).map(target::reference) {
            let id = u32::try_from(id).map_err(|_| format!("can't find pane: {spec}"))?;
            return self.place_of_pane(id).ok_or_else(|| format!("can't find pane: {spec}"));
        }
        let si = match &t.session {
            Some(s) => self.session_by_spec(s)?,
            None => def.si,
        };
        let wi = match (&t.window, &t.session) {
            (Some(w), _) => match self.window_in(si, w) {
                Ok(wi) => wi,
                Err(e) if kind == Kind::Window && t.session.is_none() => match self.session_by_spec(w) {
                    Ok(other) => return Ok(self.current_place(other)),
                    Err(_) => return Err(e),
                },
                Err(e) => return Err(e),
            },
            (None, Some(_)) => self.sessions[si].current,
            (None, None) => def.wi,
        };
        let pane = match &t.pane {
            Some(p) => self.pane_in(si, wi, p)?,
            None if si == def.si && wi == def.wi => def.pane,
            None => self.sessions[si].windows[wi].active,
        };
        Ok(Place { si, wi, pane })
    }

    pub(super) fn current_place(&self, si: usize) -> Place {
        let s = &self.sessions[si];
        Place { si, wi: s.current, pane: s.window().active }
    }

    /// The variables of formats (`#{window_name}` and so on) for a pane and its window and session.
    pub(super) fn place_vars(&self, p: Place) -> HashMap<&'static str, String> {
        let s: &Session = &self.sessions[p.si];
        let w: &Window = &s.windows[p.wi];
        let order = w.layout.panes();
        let pane_index = order.iter().position(|&id| id == p.pane).map_or(0, |i| i + self.pane_base_index());
        let pane = w.panes.get(&p.pane);
        let fg = pane.and_then(|pane| proc::foreground_pid(pane.pty.master.as_fd()));
        let rect_size = pane.map(|pane| (pane.term.grid.cols, pane.term.grid.rows)).unwrap_or((0, 0));
        let flag = |on: bool| if on { "1" } else { "0" }.to_string();
        let host = super::conf::host();
        let mut v: HashMap<&'static str, String> = HashMap::new();
        let session_name = s.name.clone();
        let window_index = (p.wi + self.base_index()).to_string();
        v.insert("session_name", session_name.clone());
        v.insert("S", session_name);
        v.insert("session_id", format!("${}", s.id));
        v.insert("session_windows", s.windows.len().to_string());
        v.insert("session_attached", self.clients.values().filter(|c| c.session == s.id && c.size.is_some()).count().to_string());
        v.insert("window_index", window_index.clone());
        v.insert("I", window_index);
        v.insert("window_name", w.name.clone());
        v.insert("W", w.name.clone());
        v.insert("window_id", format!("@{}", w.id));
        v.insert("window_active", flag(p.wi == s.current));
        v.insert("window_panes", w.tiled().to_string());
        v.insert("window_zoomed_flag", flag(w.layout.zoomed().is_some()));
        let synchronized = flag(self.window_flag(p.si, p.wi, "synchronize-panes"));
        v.insert("window_synchronized", synchronized.clone());
        v.insert("pane_synchronized", synchronized);
        v.insert("window_activity_flag", flag(w.alerts.activity.is_some()));
        v.insert("window_bell_flag", flag(w.alerts.bell.is_some()));
        v.insert("window_silence_flag", flag(w.alerts.silence.is_some()));
        v.insert(
            "window_flags",
            format!(
                "{}{}{}{}{}",
                if w.alerts.activity.is_some() { "#" } else { "" },
                if w.alerts.bell.is_some() { "!" } else { "" },
                if w.alerts.silence.is_some() { "~" } else { "" },
                if p.wi == s.current { "*" } else { "" },
                if w.layout.zoomed().is_some() { "Z" } else { "" }
            ),
        );
        v.insert("pane_id", format!("%{}", p.pane));
        v.insert("pane_index", pane_index.to_string());
        v.insert("P", pane_index.to_string());
        v.insert("pane_active", flag(p.pane == w.active));
        v.insert("pane_width", rect_size.0.to_string());
        v.insert("pane_height", rect_size.1.to_string());
        if let Some(size) = self.size_of(p.si) {
            let area = self.pane_area(size, s.status_rows);
            v.insert("window_width", area.w.to_string());
            v.insert("window_height", area.h.to_string());
            let layout = w.layout.to_tmux(area);
            v.insert("window_layout", layout.clone());
            v.insert("window_visible_layout", layout);
            if let Some((_, r)) = w.layout.rects(area).into_iter().find(|(id, _)| *id == p.pane) {
                let (left, top) = (r.x.saturating_sub(area.x), r.y.saturating_sub(area.y));
                v.insert("pane_left", left.to_string());
                v.insert("pane_top", top.to_string());
                v.insert("pane_right", (left + r.w).saturating_sub(1).to_string());
                v.insert("pane_bottom", (top + r.h).saturating_sub(1).to_string());
                v.insert("pane_at_left", flag(left == 0));
                v.insert("pane_at_top", flag(top == 0));
                v.insert("pane_at_right", flag(left + r.w >= area.w));
                v.insert("pane_at_bottom", flag(top + r.h >= area.h));
            }
        }
        if let Some(pane) = pane {
            v.insert("cursor_x", pane.term.grid.cx.to_string());
            v.insert("cursor_y", pane.term.grid.cy.to_string());
        }
        v.insert("pane_in_mode", flag(pane.is_some_and(|pane| pane.copy.is_some())));
        v.insert("pane_input_off", flag(pane.is_some_and(|pane| pane.input_off)));
        v.insert("pane_restore_missing", flag(self.kept_dirs.contains_key(&p.pane)));
        v.insert("agent_name", pane.and_then(|pane| pane.agent.as_ref()).and_then(|a| a.name.clone()).unwrap_or_default());
        v.insert("pane_spawned_by",self.spawned_by.get(&p.pane).map_or(String::new(), |by| format!("%{by}")));
        v.insert("pane_dead", flag(pane.is_some_and(|pane| pane.dead.is_some())));
        v.insert("pane_dead_status", pane.and_then(|pane| pane.dead).map_or(String::new(), |c| c.to_string()));
        v.insert("pane_pid", pane.map_or(String::new(), |pane| pane.pty.child.to_string()));
        v.insert("pane_start_command", pane.and_then(|pane| pane.start.as_deref()).map_or(String::new(), crate::sanitize::clean));
        // Values a program in the pane or a repository chose: no control, C1 or bidirectional
        // characters, wherever the value ends up (status cells, scripts, hooks; review P-08, F-F).
        let clean = crate::sanitize::clean;
        // A pane nobody titled has the host name as its title, as in tmux (scripts that read
        // tab-separated fields lose their place at an empty one).
        let title = pane.map_or(String::new(), |pane| if pane.term.grid.title.is_empty() { crate::paths::hostname() } else { clean(&pane.term.grid.title) });
        v.insert("T", title.clone());
        v.insert("pane_title", title);
        v.insert("pane_current_path", fg.and_then(proc::cwd).map_or(String::new(), |d| clean(&d.to_string_lossy())));
        v.insert("pane_current_command", clean(&fg.and_then(proc::command_name).unwrap_or_default()));
        let host_short = host.split('.').next().unwrap_or_default().to_string();
        v.insert("host", host.clone());
        v.insert("H", host);
        v.insert("host_short", host_short.clone());
        v.insert("h", host_short);
        v.insert("pid", std::process::id().to_string());
        v.insert("start_time", self.started.to_string());
        v.insert("socket_path", crate::paths::socket_path().to_string_lossy().into_owned());
        v.insert("version", format::COMPAT_VERSION.to_string());
        v.insert("window_last_flag", flag(s.last_index() == Some(p.wi)));
        v.insert("client_prefix", flag(self.sessions_in_prefix().contains(&s.id)));
        v.insert("session_many_attached", flag(self.clients.values().filter(|c| c.session == s.id && c.size.is_some()).count() > 1));
        v.insert("pane_tty", pane.map_or(String::new(), |pane| pane.pty.tty_name()));
        v.insert("pane_mode", pane.filter(|pane| pane.copy.is_some()).map_or(String::new(), |_| "copy-mode".to_string()));
        v.insert("D", format!("%{}", p.pane));
        let flags_text = v.get("window_flags").cloned().unwrap_or_default();
        v.insert("F", flags_text);
        let meta = |key: &str| w.meta.iter().find(|(k, _)| k == key).map(|(_, val)| clean(val)).unwrap_or_default();
        v.insert("worktree_name", meta("worktree.name"));
        v.insert("worktree_branch", meta("worktree.branch"));
        v.insert("worktree_path", meta("worktree.path"));
        v.insert("worktree_repo", meta("worktree.repo").rsplit('/').next().unwrap_or_default().to_string());
        // The variables of the hook that is running, if one is.
        v.extend(self.hooks.vars.iter().map(|(k, val)| (*k, val.clone())));
        if let Some(ctx) = &self.mouse_ctx {
            v.extend(ctx.vars.iter().map(|(k, val)| (*k, val.clone())));
        }
        v
    }

    /// `text` with the variables of the pane at `p` filled in.
    /// A variable that is not one of the pane's is read as the name of an option (`#{status}`,
    /// `#{status-position}`), as a configuration written for other multiplexers expects.
    ///
    /// The loops `#{P:...}` (the panes of the window), `#{W:...}` (the windows of the session) and
    /// `#{S:...}` (the sessions) and the search `#{C:text}` (the first row of the pane's screen that
    /// holds it) work here; the variables of a loop's turn are those of that pane, or of the active
    /// pane of that window or session.
    pub(super) fn expand_at(&self, p: Place, text: &str) -> String {
        let vars = self.place_vars(p);
        let lookup = |name: &str| lookup_one(&self.options, &self.pane_env, &vars, name);
        let s = &self.sessions[p.si];
        let panes: Vec<PaneId> = if text.contains("{P:") { s.windows[p.wi].layout.panes() } else { Vec::new() };
        let pane_vars: Vec<_> = panes.iter().map(|&pane| self.place_vars(Place { pane, ..p })).collect();
        let window_vars: Vec<_> = if text.contains("{W:") {
            s.windows.iter().enumerate().map(|(wi, w)| self.place_vars(Place { si: p.si, wi, pane: w.active })).collect()
        } else {
            Vec::new()
        };
        let session_vars: Vec<_> = if text.contains("{S:") {
            self.sessions.iter().enumerate().map(|(si, s)| self.place_vars(Place { si, wi: s.current, pane: s.window().active })).collect()
        } else {
            Vec::new()
        };
        let in_pane = |i: usize, name: &str| pane_vars.get(i).and_then(|v| v.get(name).cloned());
        let in_window = |i: usize, name: &str| window_vars.get(i).and_then(|v| v.get(name).cloned());
        let in_session = |i: usize, name: &str| session_vars.get(i).and_then(|v| v.get(name).cloned());
        let current = panes.iter().position(|&id| id == s.windows[p.wi].active).unwrap_or(0);
        let loops = format::Loops {
            panes: (!pane_vars.is_empty()).then_some(format::Windows { count: pane_vars.len(), current, lookup: &in_pane }),
            windows: (!window_vars.is_empty()).then_some(format::Windows { count: window_vars.len(), current: s.current, lookup: &in_window }),
            sessions: (!session_vars.is_empty()).then_some(format::Windows { count: session_vars.len(), current: p.si, lookup: &in_session }),
        };
        let screen = s.windows[p.wi].panes.get(&p.pane);
        let search = |text: &str, regex: bool, ignore_case: bool| screen.map_or(0, |pane| search_screen(pane, text, regex, ignore_case));
        format::expand_with(text, &lookup, loops, Some(&search))
    }

    /// Whether shell text may hold the variable unquoted: one of the pane-safe ones, or a user
    /// option no program in a pane gave a value to.
    pub(super) fn shell_safe(&self, name: &str) -> bool {
        super::shellcmd::pane_safe(name) || self.options.user_trusted(name)
    }

    /// `Command::Server`: a key (or the prompt) runs the words as `imux <command>` would, on the
    /// session of the client that pressed it. The values of `-c` and `-n` are formats, as in a
    /// binding. What the command prints is shown on the client; a mistake is a message.
    pub(super) fn run_words(&mut self, client: usize, words: &[String]) {
        let Some(si) = self.client_session(client) else { return };
        let place = self.current_place(si);
        let mut words = words.to_vec();
        for i in 1..words.len().saturating_sub(1) {
            if matches!(words[i].as_str(), "-c" | "-n") && words[i + 1].contains('#') {
                words[i + 1] = self.expand_at(place, &words[i + 1]);
            }
        }
        let name = words[0].clone();
        let saved = std::mem::replace(&mut self.recent, self.sessions[si].id);
        self.take_after_sent();
        let result = self.run_remote(client, &RunArgs { words: words.clone(), cwd: None });
        if self.sessions.iter().any(|s| s.id == saved) {
            self.recent = saved;
        }
        self.deferred = false;
        match result {
            Ok(text) => {
                if !self.take_after_sent() {
                    self.emit_after(Some(client), &name, &words[1..]);
                }
                let lines: Vec<String> = text.lines().map(str::to_string).collect();
                match lines.len() {
                    0 => {}
                    1 => self.show_message(client, lines[0].clone()),
                    _ => self.show_text(client, &name, lines),
                }
            }
            Err(e) => {
                self.emit_command_error(Some(client), &name, &e);
                self.show_message(client, e);
            }
        }
        self.dirty = true;
    }

    /// Handle `ClientMsg::Run`: run the command and answer the connection.
    pub(super) fn run_request(&mut self, client: usize, mut args: RunArgs) {
        self.deferred = false;
        if args.words.first().is_some_and(|w| !knows(w)) {
            match crate::config::stmt::expand_alias(&args.words, &self.aliases) {
                Ok(Some(words)) => args.words = words,
                Ok(None) => {}
                Err(e) => return self.finish_request(client, Err(e)),
            }
        }
        match args.words.first().map(|w| commands::resolve_prefix(w)) {
            Some(Ok(Some(full))) => args.words[0] = full.to_string(),
            Some(Err(e)) => return self.finish_request(client, Err(e)),
            _ => {}
        }
        self.take_after_sent();
        let result = self.run_remote(client, &args);
        self.from_pane = false;
        // A command that went through the dispatcher has told its hooks already.
        if result.is_ok()
            && !self.take_after_sent()
            && let Some((name, rest)) = args.words.split_first()
        {
            self.emit_after(None, name, rest);
        }
        if let Err(e) = &result {
            self.emit_command_error(None, args.words.first().map_or("", String::as_str), e);
        }
        if !std::mem::take(&mut self.deferred) {
            self.finish_request(client, result);
        }
    }

    fn run_remote(&mut self, client: usize, args: &RunArgs) -> Result<String, String> {
        self.number_windows();
        let Some((name, rest)) = args.words.split_first() else { return Err("no command given".to_string()) };
        self.plugin_allows_command(client, name).inspect_err(|e| log::warn(e))?;
        let caller = self.pane_of_peer(client);
        let power = self.power_of(caller);
        let popup = matches!(name.as_str(), "display-popup" | "popup");
        let forced: RunArgs;
        let args = match power {
            Power::Full => args,
            // `wait-for` only touches the channels, and it is how a command sent to a pane reports back.
            Power::Look => {
                if popup && !self.options.flag("popup-from-panes") {
                    log::warn(&format!("refused {name} from pane {}: popup-from-panes is off", caller.unwrap_or_default()));
                    return Err(format!("{name}: not allowed from inside a pane (set popup-from-panes or command-from-panes on to allow it)"));
                }
                if !popup && !is_read_only(name) && !matches!(name.as_str(), "display-message" | "display" | "capture-pane" | "capturep" | "wait-for" | "wait" | "api-pane-wait") {
                    log::warn(&format!("refused {name} from pane {}: command-from-panes is off", caller.unwrap_or_default()));
                    return Err(format!("{name}: not allowed from inside a pane (set command-from-panes on to allow it)"));
                }
                args
            }
            Power::Subtree(me) => match self.gate_subtree(me, name, rest) {
                Ok(words) => {
                    forced = RunArgs { words, cwd: args.cwd.clone() };
                    &forced
                }
                Err(e) => {
                    log::warn(&format!("refused {name} from pane {me}: {e}"));
                    return Err(e);
                }
            },
        };
        self.from_pane = caller.is_some();
        let result = self.dispatch_remote(client, args, caller, power == Power::Look);
        if let Some(me) = caller
            && power != Power::Look
            && args.words.first().is_some_and(|n| spawned::is_write(n))
        {
            self.log_pane_write(me, power, &args.words, &result);
        }
        result
    }

    /// The commands of the socket, by name. `own_pane_only`: the caller may read only its own pane.
    fn dispatch_remote(&mut self, client: usize, args: &RunArgs, caller: Option<PaneId>, own_pane_only: bool) -> Result<String, String> {
        let Some((name, rest)) = args.words.split_first() else { return Err("no command given".to_string()) };
        match name.as_str() {
            "list-sessions" | "ls" => self.list_sessions(rest),
            "list-windows" | "lsw" => self.list_windows(rest, caller),
            "list-panes" | "lsp" => self.list_panes(rest, caller),
            "list-clients" | "lsc" => self.list_clients(rest),
            "has-session" | "has" => {
                let f = flags(name, rest, "t", "")?;
                self.session_by_spec(f.value('t').unwrap_or("")).map(|_| String::new())
            }
            "display-message" | "display" => self.display_message(client, rest, caller),
            "capture-pane" | "capturep" => self.capture_pane(rest, caller, own_pane_only),
            "send-keys" | "send" => self.send_keys(rest, caller),
            "plugin-reload" => self.plugin_reload(),
            "api-pane-run" => self.pane_run(rest),
            "api-agent-name" => self.cmd_agent_name(rest),
            "api-agent-resolve" => self.cmd_agent_resolve(rest),
            "api-pane-wait" => self.cmd_pane_wait(client, rest, caller, own_pane_only),
            "api-agent-start" => self.cmd_agent_start(client, rest, caller),
            "api-seq" if rest.is_empty() => Ok(format!("{}\n", self.event_seq())),
            "api-agent-list" if rest.is_empty() => Ok(self.agent_list_json()),
            "api-agent-get" => {
                let [pane] = rest else { return Err(tag("invalid_argument", "usage: api-agent-get pane")) };
                let id = pane.trim_start_matches('%').parse::<u32>().map_err(|_| tag("invalid_argument", &format!("{pane:?} is not a pane number")))?;
                self.agent_json(id).map(|json| json + "\n")
            }
            "detach-client" | "detach" if rest.iter().any(|a| a.starts_with("-t") || a.starts_with("-s")) => self.cmd_detach_client(client, rest, caller),
            "kill-session" => {
                let f = flags(name, rest, "t", "a")?;
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Session, caller)?;
                if f.has('a') {
                    let keep = self.sessions[place.si].id;
                    let ids: Vec<usize> = self.sessions.iter().map(|s| s.id).filter(|id| *id != keep).collect();
                    for id in ids {
                        if let Some(si) = self.sessions.iter().position(|s| s.id == id) {
                            self.kill_session(si);
                        }
                    }
                } else {
                    self.kill_session(place.si);
                }
                Ok(String::new())
            }
            "kill-window" | "killw" | "unlink-window" | "unlinkw" => {
                // Windows are never linked into two sessions, so `unlink-window` is `kill-window`
                // and its `-k` (kill it even if it is the last link) has nothing to change.
                let f = flags(name, rest, "t", if name.starts_with("unlink") { "ak" } else { "a" })?;
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Window, caller)?;
                let session = &self.sessions[place.si];
                // With -a every window but the target goes.
                let ids: Vec<PaneId> = session
                    .windows
                    .iter()
                    .enumerate()
                    .filter(|(wi, _)| (*wi == place.wi) != f.has('a'))
                    .flat_map(|(_, w)| w.panes.keys().copied())
                    .collect();
                for id in ids {
                    self.close_pane(id, None);
                }
                Ok(String::new())
            }
            "kill-pane" | "killp" => {
                let f = flags(name, rest, "t", "a")?;
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
                // With -a every pane of the window but the target goes.
                let ids: Vec<PaneId> = if f.has('a') {
                    self.sessions[place.si].windows[place.wi].panes.keys().copied().filter(|id| *id != place.pane).collect()
                } else {
                    vec![place.pane]
                };
                for id in ids {
                    self.close_pane(id, None);
                }
                Ok(String::new())
            }
            "show-options" | "show" | "show-window-options" | "showw" => {
                let f = flags(name, rest, "t", "gqvAws")?;
                let window = f.value('t').map(|t| self.resolve(t, Kind::Window, caller)).transpose()?;
                match f.words.as_slice() {
                    [] => Ok(self.options.listing().into_iter().chain(self.options.user_listing()).map(|(n, v)| format!("{n} {}\n", option_text(v))).collect()),
                    [one] => match self
                        .window_value(window, one)
                        .or_else(|| self.options.user(one).map(str::to_string))
                        .or_else(|| self.options.value_text(one).map(str::to_string))
                    {
                        Some(v) if f.has('v') => Ok(format!("{v}\n")),
                        Some(v) => Ok(format!("{one} {}\n", option_text(&v))),
                        None if f.has('q') => Ok(String::new()),
                        None => Err(format!("unknown option: {one}")),
                    },
                    _ => Err("usage: show-options [-gqv] [-t window] [option]".to_string()),
                }
            }
            "list-keys" | "lsk" => {
                let f = flags(name, rest, "T", "")?;
                if !f.words.is_empty() {
                    return Err("usage: list-keys [-T table]".to_string());
                }
                let table = f.value('T');
                if let Some(t) = table.filter(|t| !matches!(*t, "prefix" | "root" | "copy-mode" | "copy-mode-vi")) {
                    return Err(format!("unknown key table: {t}"));
                }
                let lines = self.bindings.list_lines().into_iter().filter(|l| table.is_none_or(|t| l.contains(&format!("-T {t} "))));
                Ok(lines.map(|l| l + "\n").collect())
            }
            "respawn-pane" | "respawnp" => self.cmd_respawn(rest, caller, false),
            "respawn-window" | "respawnw" => self.cmd_respawn(rest, caller, true),
            "join-pane" | "joinp" | "move-pane" | "movep" => self.cmd_join_pane(rest, caller),
            "wait-for" | "wait" => self.cmd_wait_for(client, rest),
            "pipe-pane" | "pipep" => self.cmd_pipe_pane(rest, caller),
            "show-hooks" => {
                let f = flags(name, rest, "", "g")?;
                if !f.words.is_empty() {
                    return Err("usage: show-hooks [-g]".to_string());
                }
                Ok(self.hook_lines().into_iter().map(|l| l + "\n").collect())
            }
            "set-option" | "set" | "set-window-option" | "setw" | "bind-key" | "bind" | "unbind-key" | "unbind" | "source-file" | "source" | "set-hook" => {
                let stmt = crate::config::stmt::parse_words(&args.words)?;
                if let crate::config::stmt::Stmt::Run(Command::SetWindowOption { name, op, target }) = stmt {
                    let place = self.resolve(target.as_deref().unwrap_or(""), Kind::Window, caller)?;
                    self.set_window_option(place.si, place.wi, &name, op)?;
                    self.emit_after(None, "set-option", std::slice::from_ref(&name));
                    return Ok(String::new());
                }
                self.apply(None, stmt).map(|()| String::new())
            }
            "set-environment" | "setenv" => self.cmd_set_environment(rest),
            "show-environment" | "showenv" => self.cmd_show_environment(rest),
            "new-session" | "new" => self.cmd_new_session(rest, args.cwd.as_deref()),
            "new-window" | "neww" => self.cmd_new_window(rest, caller),
            "split-window" | "splitw" => self.cmd_split_window(rest, caller),
            "select-window" | "selectw" => self.select_window(rest, caller),
            "select-pane" | "selectp" => self.select_pane(rest, caller),
            "swap-pane" | "swapp" if rest.iter().any(|a| a.starts_with("-s") || a.starts_with("-t")) => self.cmd_swap_pane(rest, caller),
            "copy-mode" if !rest.is_empty() => self.cmd_copy_mode(rest, caller),
            "select-layout" | "selectl" => self.cmd_select_layout(rest, caller),
            "resize-pane" | "resizep" => self.cmd_resize_pane(rest, caller),
            "move-window" | "movew" | "swap-window" | "swapw" => self.cmd_move_window(name, rest, caller),
            "rename-session" | "rename" => {
                let f = flags(name, rest, "t", "")?;
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Session, caller)?;
                let [new] = f.words.as_slice() else { return Err(format!("usage: {name} [-t session] new-name")) };
                let new = new.trim();
                if new.is_empty() || new.chars().any(char::is_control) {
                    return Err(format!("{name}: bad name {new:?}"));
                }
                if self.sessions.iter().enumerate().any(|(i, s)| i != place.si && s.name == new) {
                    return Err(format!("duplicate session: {new}"));
                }
                self.sessions[place.si].name = new.to_string();
                self.dirty = true;
                Ok(String::new())
            }
            "rename-window" | "renamew" => {
                let f = flags(name, rest, "t", "")?;
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Window, caller)?;
                let [new] = f.words.as_slice() else { return Err(format!("usage: {name} [-t window] new-name")) };
                let new = new.trim();
                if new.is_empty() || new.chars().any(char::is_control) {
                    return Err(format!("{name}: bad name {new:?}"));
                }
                let win = &mut self.sessions[place.si].windows[place.wi];
                win.name = new.to_string();
                win.auto_rename = false;
                self.dirty = true;
                Ok(String::new())
            }
            _ => self.run_for_client(client, name, args, caller),
        }
    }

    /// The value a window has of its own for `name` (`show-options -t window`): its window option
    /// override, or `automatic-rename` as the window has it. `None` follows the global option.
    fn window_value(&self, window: Option<Place>, name: &str) -> Option<String> {
        let w = &self.sessions[window?.si].windows[window?.wi];
        let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
        match name {
            "automatic-rename" => Some(on_off(w.auto_rename)),
            _ => w.opts.get(name).map(on_off),
        }
    }

    /// Any other command of the command prompt, run for a client that looks at the session.
    fn run_for_client(&mut self, requester: usize, name: &str, args: &RunArgs, caller: Option<PaneId>) -> Result<String, String> {
        let command: Command = commands::parse_words(&args.words).map_err(|e| {
            if e.starts_with("unknown command") { format!("unknown command: {name}") } else { e }
        })?;
        if matches!(command, Command::RunShell(_) | Command::IfShell(_)) {
            log::warn(&format!("refused {name} from the socket"));
            return Err(format!("{name}: only a configuration file, a key binding or the command prompt can run shell commands"));
        }
        let place = self.default_place(caller)?;
        let sid = self.sessions[place.si].id;
        let target = self
            .clients
            .iter()
            .filter(|(id, c)| **id != requester && c.session == sid && c.size.is_some() && !c.closing)
            .map(|(id, _)| *id)
            .min()
            .ok_or_else(|| format!("{name}: no client is attached to the session"))?;
        if let Command::DisplayPopup(spec) = &command {
            // The command waits until the popup is gone, then answers with how its program ended.
            self.deferred = self.open_popup(target, spec, Some(requester));
            return Ok(String::new());
        }
        self.run_command(target, &command);
        Ok(String::new())
    }

    /// `detach-client [-a] [-s session] [-t client]` from the socket: `-t client-N` (or a bare
    /// number) detaches that client, with `-a` every other attached client; `-s` detaches every
    /// client of the session. The command's own connection is never one of them.
    fn cmd_detach_client(&mut self, requester: usize, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("detach-client", args, "ts", "a")?;
        if !f.words.is_empty() {
            return Err("usage: detach-client [-a] [-s session] [-t client]".to_string());
        }
        let attached = |s: &Self, id: usize| s.clients.get(&id).is_some_and(|c| c.size.is_some() && !c.closing && c.session != usize::MAX && id != requester);
        let mut ids: Vec<usize> = self.clients.keys().copied().filter(|id| attached(self, *id)).collect();
        if let Some(spec) = f.value('s') {
            let sid = self.sessions[self.resolve(spec, Kind::Session, caller)?.si].id;
            ids.retain(|id| self.clients[id].session == sid);
        }
        if let Some(spec) = f.value('t') {
            let wanted = spec.strip_prefix("client-").unwrap_or(spec).parse::<usize>().map_err(|_| format!("can't find client: {spec}"))?;
            if !ids.contains(&wanted) {
                return Err(format!("can't find client: {spec}"));
            }
            ids.retain(|id| (*id == wanted) != f.has('a'));
        }
        for id in ids {
            log::info(&format!("client {id} detached by a command"));
            self.queue(id, &ServerMsg::Detached);
            self.close_after_flush(id);
        }
        Ok(String::new())
    }

    /// Whether a line passes the `-f` filter of a `list-*` command: no filter keeps everything; a
    /// filter is a format and keeps the line when it expands to something other than nothing or `0`.
    fn passes(&self, filter: Option<&str>, place: Place) -> bool {
        filter.is_none_or(|f| !matches!(self.expand_at(place, f).trim(), "" | "0"))
    }

    fn list_sessions(&self, args: &[String]) -> Result<String, String> {
        let f = flags("list-sessions", args, "Ff", "")?;
        let mut out = String::new();
        for (si, s) in self.sessions.iter().enumerate() {
            if !self.passes(f.value('f'), self.current_place(si)) {
                continue;
            }
            match f.value('F') {
                Some(fmt) => out.push_str(&self.expand_at(self.current_place(si), fmt)),
                None => out.push_str(&self.session_summary(s)),
            }
            out.push('\n');
        }
        Ok(out)
    }

    fn list_windows(&self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("list-windows", args, "Fft", "a")?;
        let sessions: Vec<usize> = if f.has('a') {
            (0..self.sessions.len()).collect()
        } else {
            vec![self.resolve(f.value('t').unwrap_or(""), Kind::Session, caller)?.si]
        };
        let mut out = String::new();
        for si in sessions {
            for (wi, w) in self.sessions[si].windows.iter().enumerate() {
                let place = Place { si, wi, pane: w.active };
                if !self.passes(f.value('f'), place) {
                    continue;
                }
                let line = match f.value('F') {
                    Some(fmt) => self.expand_at(place, fmt),
                    None => {
                        let n = w.tiled();
                        self.expand_at(place, &format!("#{{window_index}}: #{{window_name}}#{{window_flags}} ({n} pane{})", if n == 1 { "" } else { "s" }))
                    }
                };
                out.push_str(&line);
                out.push('\n');
            }
        }
        Ok(out)
    }

    fn list_panes(&self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("list-panes", args, "Fft", "as")?;
        let places: Vec<(usize, usize)> = if f.has('a') {
            self.sessions.iter().enumerate().flat_map(|(si, s)| (0..s.windows.len()).map(move |wi| (si, wi))).collect()
        } else if f.has('s') {
            let si = self.resolve(f.value('t').unwrap_or(""), Kind::Session, caller)?.si;
            (0..self.sessions[si].windows.len()).map(|wi| (si, wi)).collect()
        } else {
            let p = self.resolve(f.value('t').unwrap_or(""), Kind::Window, caller)?;
            vec![(p.si, p.wi)]
        };
        let mut out = String::new();
        for (si, wi) in places {
            let win = &self.sessions[si].windows[wi];
            for pane in win.layout.panes() {
                let place = Place { si, wi, pane };
                if !self.passes(f.value('f'), place) {
                    continue;
                }
                match f.value('F') {
                    Some(fmt) => out.push_str(&self.expand_at(place, fmt)),
                    None => {
                        let v = self.place_vars(place);
                        let active = if win.active == pane { " (active)" } else { "" };
                        out.push_str(&format!("{}: [{}x{}] {}{active}", v["pane_index"], v["pane_width"], v["pane_height"], v["pane_id"]));
                    }
                }
                out.push('\n');
            }
        }
        Ok(out)
    }

    fn list_clients(&self, args: &[String]) -> Result<String, String> {
        let f = flags("list-clients", args, "Fft", "")?;
        let mut ids: Vec<usize> = self.clients.iter().filter(|(_, c)| c.size.is_some() && !c.closing).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let mut out = String::new();
        for id in ids {
            let c = &self.clients[&id];
            let Some(si) = self.session_index(c.session) else { continue };
            let size = c.size.map_or((0, 0), |s| (s.cols, s.rows));
            let mut vars = self.place_vars(self.current_place(si));
            vars.insert("client_name", format!("client-{id}"));
            vars.insert("client_session", self.sessions[si].name.clone());
            vars.insert("client_width", size.0.to_string());
            vars.insert("client_height", size.1.to_string());
            vars.insert("client_readonly", u8::from(c.read_only).to_string());
            let fmt = f.value('F').unwrap_or("#{client_name}: #{client_session} [#{client_width}x#{client_height}]#{?client_readonly, (read-only),}");
            let look = |name: &str| vars.get(name).cloned().or_else(|| self.options.user(name).map(str::to_string));
            if f.value('f').is_some_and(|flt| matches!(format::expand(flt, &look).trim(), "" | "0")) {
                continue;
            }
            out.push_str(&format::expand(fmt, &look));
            out.push('\n');
        }
        Ok(out)
    }

    fn display_message(&mut self, client: usize, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("display-message", args, "tcF", "pa")?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        let text = f.words.first().map(String::as_str).or(f.value('F')).unwrap_or("#{session_name}:#{window_index}.#{pane_index}");
        let shown = self.expand_at(place, text);
        if f.has('p') {
            return Ok(shown + "\n");
        }
        let sid = self.sessions[place.si].id;
        let target = self.clients.iter().filter(|(id, c)| **id != client && c.session == sid && c.size.is_some()).map(|(id, _)| *id).min();
        match target {
            Some(id) => {
                self.show_message(id, shown);
                Ok(String::new())
            }
            None => {
                // No client to show it on is not an error (tmux says nothing either): scripts
                // that tell the user something do not fail when nobody is looking.
                log::debug("display-message: no client is attached to the session, nothing shown");
                Ok(String::new())
            }
        }
    }

    fn capture_pane(&mut self, args: &[String], caller: Option<PaneId>, own_pane_only: bool) -> Result<String, String> {
        let f = flags("capture-pane", args, "tSEb", "pJeNCq")?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        if own_pane_only && caller != Some(place.pane) {
            return Err("capture-pane: a program in a pane may only read its own pane (set command-from-panes on to allow more)".to_string());
        }
        let pane = self.sessions[place.si].windows[place.wi].panes.get(&place.pane).ok_or("can't find pane")?;
        let (history, rows) = capture_size(pane);
        let (first, last) = capture_range(history, rows, f.value('S'), f.value('E'))?;
        let how = CaptureStyle { join: f.has('J'), keep_blanks: f.has('N'), escapes: f.has('e') };
        let text = capture_text(pane, first, last, how);
        if f.has('p') {
            return Ok(text);
        }
        let limit = self.buffer_limit();
        match f.value('b') {
            Some(name) => self.buffers.set(name, &text, false)?,
            None => {
                self.buffers.push(text, limit);
            }
        }
        Ok(String::new())
    }

    fn send_keys(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("send-keys", args, "tN", "lXRMFH")?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        let words: Vec<String> = if f.has('F') && !f.has('X') { f.words.iter().map(|w| self.expand_at(place, w).chars().filter(|c| !c.is_control()).collect()).collect() } else { f.words.clone() };
        let hex: Option<Vec<u8>> = if f.has('H') && !f.has('X') {
            let bytes = words.iter().map(|w| u8::from_str_radix(w.trim_start_matches("0x"), 16).ok().filter(|_| w.len() <= 4));
            Some(bytes.collect::<Option<Vec<u8>>>().ok_or("send-keys: -H takes bytes in hexadecimal (1b, 5b, 41)")?)
        } else {
            None
        };
        let repeat: usize = match f.value('N') {
            Some(n) => n.parse().ok().filter(|n| (1..=1000).contains(n)).ok_or("send-keys: -N takes a number from 1 to 1000")?,
            None => 1,
        };
        if f.has('X') {
            let [action] = f.words.as_slice() else { return Err("usage: send-keys -X action".to_string()) };
            let op = Op::parse(action).ok_or_else(|| format!("send-keys: unknown copy mode action {action}"))?;
            return self.copy_action(place, op).map(|()| String::new());
        }
        let win = &self.sessions[place.si].windows[place.wi];
        let pane = win.panes.get(&place.pane).ok_or("can't find pane")?;
        let encode = |app_cursor: bool| {
            let mut bytes = Vec::new();
            if let Some(raw) = &hex {
                bytes.extend_from_slice(raw);
            }
            for word in words.iter().filter(|_| hex.is_none()) {
                let key = if f.has('l') { None } else { Key::parse(word) };
                match key.and_then(|k| keys::encode(&k, app_cursor)) {
                    Some(b) => bytes.extend_from_slice(&b),
                    None => bytes.extend_from_slice(word.as_bytes()),
                }
            }
            let once = bytes.clone();
            for _ in 1..repeat {
                bytes.extend_from_slice(&once);
            }
            bytes
        };
        let bytes = encode(pane.term.grid.modes.cursor_keys);
        if bytes.len() > 1 << 20 {
            return Err("send-keys: too much to type at once".to_string());
        }
        if pane.dead.is_some() {
            return Err("send-keys: the pane is dead (respawn-pane starts it again)".to_string());
        }
        pane.pty.write_typed(&bytes).map_err(|e| format!("send-keys: {e}"))?;
        for id in self.typing_targets(place.si, place.wi, place.pane).into_iter().skip(1) {
            let Some(other) = win.panes.get(&id) else { continue };
            if let Err(e) = other.pty.write_typed(&encode(other.term.grid.modes.cursor_keys)) {
                log::warn(&format!("send-keys to a synchronized pane: {e}"));
            }
        }
        Ok(String::new())
    }

    fn select_window(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("select-window", args, SELECT_WINDOW_FLAGS.0, SELECT_WINDOW_FLAGS.1)?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Window, caller)?;
        let s = &mut self.sessions[place.si];
        let to = if f.has('l') {
            s.last_index().ok_or("no last window")?
        } else if f.has('n') || f.has('p') {
            let n = s.windows.len();
            if f.has('n') { (s.current + 1) % n } else { (s.current + n - 1) % n }
        } else {
            place.wi
        };
        s.select(to);
        self.dirty = true;
        Ok(String::new())
    }

    fn select_pane(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("select-pane", args, SELECT_PANE_FLAGS.0, SELECT_PANE_FLAGS.1)?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        let size = self.size_of(place.si);
        let status_rows = self.sessions[place.si].status_rows;
        let area = size.map(|s| self.pane_area(s, status_rows));
        let win = &mut self.sessions[place.si].windows[place.wi];
        win.layout.unzoom();
        win.focus(place.pane);
        let dir = [('L', Direction::Left), ('R', Direction::Right), ('U', Direction::Up), ('D', Direction::Down)]
            .into_iter()
            .find(|(c, _)| f.has(*c))
            .map(|(_, d)| d);
        match (dir, area) {
            (Some(d), Some(area)) => win.focus_direction(d, area),
            (Some(_), None) => return Err("select-pane: no screen size yet".to_string()),
            (None, _) if f.has('l') => win.focus_last(),
            _ => {}
        }
        if f.has('m') {
            self.marked = Some(place.pane);
        }
        if f.has('M') {
            self.marked = None;
        }
        if let Some(title) = f.value('T')
            && let Some(p) = self.sessions[place.si].windows[place.wi].panes.get_mut(&place.pane)
        {
            // What a program says through OSC 0/2 is cut and cleaned the same way.
            p.term.grid.title = crate::sanitize::clean(title).chars().take(256).collect();
        }
        if f.has('d') || f.has('e') {
            if f.has('d') && f.has('e') {
                return Err("select-pane: use -d or -e, not both".to_string());
            }
            if let Some(p) = self.sessions[place.si].windows[place.wi].panes.get_mut(&place.pane) {
                p.input_off = f.has('d');
            }
        }
        self.relayout();
        self.dirty = true;
        Ok(String::new())
    }

    /// `send-keys -X`: a copy mode action on a pane that is in copy mode.
    fn copy_action(&mut self, place: Place, op: Op) -> Result<(), String> {
        let separators = self.options.text("word-separators").to_string();
        let win = self.sessions[place.si].windows.get_mut(place.wi).ok_or("can't find window")?;
        let pane = win.panes.get_mut(&place.pane).ok_or("can't find pane")?;
        let mut mode = pane.copy.take().ok_or("pane is not in a mode")?;
        mode.separators = separators;
        let action = mode.apply(&pane.term.grid, op);
        self.finish_copy_step(place.si, place.pane, mode, action);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn flags_take_values_group_and_stop_at_double_dash() {
        let f = flags("x", &words("-t work -dh -- -not-a-flag word"), "tF", "dh").unwrap();
        assert_eq!(f.value('t'), Some("work"));
        assert!(f.has('d') && f.has('h') && !f.has('x'));
        assert_eq!(f.words, ["-not-a-flag", "word"]);
        let f = flags("x", &words("-twork:2.1 -F #{a}"), "tF", "").unwrap();
        assert_eq!((f.value('t'), f.value('F')), (Some("work:2.1"), Some("#{a}")));
        let f = flags("x", &words("-dt work"), "t", "d").unwrap();
        assert!(f.has('d') && f.value('t') == Some("work"), "a valued flag may end a group");
    }

    #[test]
    fn the_first_word_ends_the_flags() {
        let f = flags("x", &words("-n name sh -c cmd -d"), "nc", "d").unwrap();
        assert_eq!(f.value('n'), Some("name"));
        assert_eq!(f.value('c'), None);
        assert!(!f.has('d'));
        assert_eq!(f.words, ["sh", "-c", "cmd", "-d"]);
    }

    #[test]
    fn flags_refuse_unknown_letters_and_missing_values() {
        assert_eq!(flags("cmd", &words("-z"), "t", "d").unwrap_err(), "cmd: unknown flag -z");
        assert_eq!(flags("cmd", &words("-t"), "t", "d").unwrap_err(), "cmd: -t needs a value");
        assert!(flags("cmd", &words("-"), "t", "d").unwrap().words == ["-"], "a lone dash is a word");
    }

    #[test]
    fn a_negative_number_after_a_valued_flag_is_its_value() {
        let f = flags("x", &words("-S -20 -E -"), "SE", "").unwrap();
        assert_eq!((f.value('S'), f.value('E')), (Some("-20"), Some("-")));
    }

    #[test]
    fn capture_ranges_count_from_the_first_screen_line() {
        // 5 lines of history, 3 on the screen: positions 0..=7, the screen is 5..=7.
        assert_eq!(capture_range(5, 3, None, None), Ok((5, 7)));
        assert_eq!(capture_range(5, 3, Some("-"), None), Ok((0, 7)));
        assert_eq!(capture_range(5, 3, Some("-2"), Some("0")), Ok((3, 5)));
        assert_eq!(capture_range(5, 3, Some("1"), Some("1")), Ok((6, 6)));
        assert_eq!(capture_range(5, 3, Some("-99"), Some("99")), Ok((0, 7)), "out of range is clamped");
        assert_eq!(capture_range(5, 3, Some("2"), Some("0")), Ok((5, 7)), "a reversed range is turned round");
        assert_eq!(capture_range(0, 3, Some("-"), Some("-")), Ok((0, 2)));
        assert!(capture_range(0, 3, Some("x"), None).is_err());
    }

    #[test]
    fn only_looking_commands_are_read_only() {
        for c in ["list-sessions", "lsw", "list-panes", "has-session", "list-clients"] {
            assert!(is_read_only(c), "{c}");
        }
        for c in ["send-keys", "kill-pane", "new-window", "split-window", "select-pane", "rename-window", "display-message", "capture-pane"] {
            assert!(!is_read_only(c), "{c}");
        }
    }
}
