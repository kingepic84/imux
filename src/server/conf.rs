//! Server side of the configuration: reading the file, applying `set`, `bind`, `unbind` and the
//! commands that come with them, and turning options into the values the server uses.

use std::time::{Duration, Instant};

use super::Server;
use super::segments::SegmentView;
use super::shellcmd::pane_safe;
use crate::config::options::{Set, StyleSpec};
use crate::config::stmt::{self, Stmt};
use crate::config::{self, format};
use crate::input::bindings::{Command, SourceSpec};
use crate::input::keys::Key;
use crate::log;
use crate::render::status::{self, Parts, Run, Status};
use crate::session::layout::PaneId;
use crate::session::{Session, Window, agent};
use crate::term::cell::{BOLD, Style};
use crate::term::status::State;

/// `source-file` inside `source-file` stops here.
const MAX_SOURCE_DEPTH: usize = 8;

/// Largest configuration file `source-file` reads.
const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;

/// Status line options in use.
pub(super) struct StatusCfg {
    pub bar: Style,
    left: String,
    right: String,
    left_len: usize,
    right_len: usize,
    tab: String,
    current: String,
    sep: String,
    base: usize,
    /// Label and text styles of messages, and of the command prompt.
    pub message: (Style, Style),
    pub command: (Style, Style),
    /// Glyph and `#[...]` style of working, blocked, done and error panes.
    glyphs: [String; 4],
    agent_styles: [String; 4],
    /// The marked pane (`select-pane -m`), if any; its window shows the flag `M`.
    pub marked: Option<PaneId>,
    /// The global `synchronize-panes`, for the windows that have no value of their own.
    sync_default: bool,
    /// `#[...]` style of a tab with an activity flag and with a bell flag.
    alert_styles: [String; 2],
    /// `#[...]` styles of the left and right segment (`status-left-style`, `status-right-style`).
    side_styles: [String; 2],
    /// `#[...]` styles of a tab (`window-status-style`), of the current window's and of the last one.
    tab_styles: [String; 3],
    justify: status::Justify,
    /// Rows the bar has at least: one past the last `status-format[N]` set.
    pub rows: usize,
    /// `status-format[0..5]`: rows drawn from their own text instead of the standard layout.
    formats: [Option<String>; crate::config::options::STATUS_FORMATS],
}

/// The agent states a status bar shows: one per window of the session, as one client sees them,
/// and the most urgent state anywhere with the number of panes in it.
#[derive(Clone, Debug, Default)]
pub(super) struct AgentStates {
    pub windows: Vec<Option<State>>,
    pub global: Option<(State, usize)>,
}

pub(super) fn state_name(state: Option<State>) -> &'static str {
    match state {
        Some(State::Idle) => "idle",
        Some(State::Working) => "working",
        Some(State::Done) => "done",
        Some(State::Blocked) => "blocked",
        Some(State::Error) => "error",
        Some(State::Clear) | None => "",
    }
}

impl StatusCfg {
    fn slot(state: State) -> Option<usize> {
        match state {
            State::Working => Some(0),
            State::Blocked => Some(1),
            State::Done => Some(2),
            State::Error => Some(3),
            State::Idle | State::Clear => None,
        }
    }

    /// Glyph text for a state, wrapped in its style; nothing for idle.
    fn glyph(&self, state: Option<State>) -> String {
        let Some(i) = state.and_then(Self::slot) else { return String::new() };
        if self.glyphs[i].is_empty() {
            return String::new();
        }
        format!("#[{}]{}#[default]", self.agent_styles[i], self.glyphs[i])
    }
}

/// Columns of every agent glyph among `runs`, which start at column `start`.
fn glyph_spans(runs: &[Run], start: usize, glyphs: &[String; 4]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut x = start;
    for (text, _) in runs {
        for g in glyphs.iter().filter(|g| !g.is_empty()) {
            for (at, _) in text.match_indices(g.as_str()) {
                let from = x + status::width(&text[..at]);
                spans.push((from, from + status::width(g)));
            }
        }
        x += status::width(text);
    }
    spans.sort_unstable();
    spans.dedup();
    spans
}

/// Label and text styles of the bottom row: the built-in look unless the option sets something.
fn message_style(spec: StyleSpec, bar: Style) -> (Style, Style) {
    if spec == StyleSpec::default() {
        (Style { fg: status::DARK, bg: status::BLUE, attrs: BOLD }, bar)
    } else {
        let style = spec.apply(bar);
        (style, style)
    }
}

pub(super) fn host() -> String {
    crate::paths::hostname()
}

/// The status bar of a session with no `#(command)` output to show.
#[cfg(test)]
pub(super) fn status_for(session: &Session, cols: usize, min_rows: usize, cfg: &StatusCfg, agents: &AgentStates) -> Status {
    status_with(session, cols, min_rows, cfg, agents, &SegmentView::default())
}

/// What a bar shows for a `#(command)` whose text has variables that a pane could have set.
fn refused_text(names: &[String]) -> String {
    format!("[#{{q:{}}}?]", names.first().map_or("name", String::as_str))
}

/// Build the status bar of a session from the format options. `segments` holds the output of the
/// `#(command)`s found so far and the variables of the session's current pane.
pub(super) fn status_with(session: &Session, cols: usize, min_rows: usize, cfg: &StatusCfg, agents: &AgentStates, segments: &SegmentView) -> Status {
    let full = host();
    let short = full.split('.').next().unwrap_or("").to_string();
    let window_state = |i: usize| agents.windows.get(i).copied().flatten();
    let global = agents.global.map(|(s, _)| s);
    let lookup_flags = |i: usize| {
        let alerts = session.windows.get(i).map(|w| w.alerts).unwrap_or_default();
        let activity = if alerts.activity.is_some() { "#" } else { "" };
        let bell = if alerts.bell.is_some() { "!" } else { "" };
        let silence = if alerts.silence.is_some() { "~" } else { "" };
        let current = if i == session.current { "*" } else { "" };
        let zoomed = if session.windows.get(i).is_some_and(|w| w.layout.zoomed().is_some()) { "Z" } else { "" };
        let marked = if cfg.marked.is_some_and(|m| session.windows.get(i).is_some_and(|w| w.panes.contains_key(&m))) { "M" } else { "" };
        format!("{activity}{bell}{silence}{current}{zoomed}{marked}")
    };
    // The value of a variable as seen from a window (a tab, or one turn of `#{W:...}`), or from
    // the bar itself with `None`.
    let window_lookup = |window: Option<(usize, &str)>, name: &str| -> Option<String> {
        // The window a worktree variable is about: the one a tab shows, else the current one.
        let meta = |key: &str| -> String {
            let win = window.and_then(|(i, _)| session.windows.get(i)).or_else(|| session.windows.get(session.current));
            win.and_then(|w| w.meta.iter().find(|(k, _)| k == key)).map(|(_, v)| v.clone()).unwrap_or_default()
        };
        match name {
            "window_flags" => window.map(|(i, _)| lookup_flags(i)),
            "window_activity_flag" => window.map(|(i, _)| {
                if session.windows.get(i).is_some_and(|w| w.alerts.activity.is_some()) { "1" } else { "0" }.to_string()
            }),
            "window_bell_flag" => window.map(|(i, _)| {
                if session.windows.get(i).is_some_and(|w| w.alerts.bell.is_some()) { "1" } else { "0" }.to_string()
            }),
            "window_marked_flag" => window.map(|(i, _)| {
                let there = cfg.marked.is_some_and(|m| session.windows.get(i).is_some_and(|w| w.panes.contains_key(&m)));
                if there { "1" } else { "0" }.to_string()
            }),
            "window_zoomed_flag" => window.map(|(i, _)| {
                if session.windows.get(i).is_some_and(|w| w.layout.zoomed().is_some()) { "1" } else { "0" }.to_string()
            }),
            "window_silence_flag" => window.map(|(i, _)| {
                if session.windows.get(i).is_some_and(|w| w.alerts.silence.is_some()) { "1" } else { "0" }.to_string()
            }),
            "window_synchronized" | "pane_synchronized" => window.map(|(i, _)| {
                let own = session.windows.get(i).and_then(|w| w.opts.get("synchronize-panes"));
                if own.unwrap_or(cfg.sync_default) { "1" } else { "0" }.to_string()
            }),
            "window_active" => window.map(|(i, _)| if i == session.current { "1" } else { "0" }.to_string()),
            "worktree_name" => Some(meta("worktree.name")),
            "worktree_branch" => Some(meta("worktree.branch")),
            "worktree_path" => Some(meta("worktree.path")),
            "worktree_repo" => Some(meta("worktree.repo").rsplit('/').next().unwrap_or_default().to_string()),
            "S" | "session_name" => Some(session.name.clone()),
            "I" | "window_index" => window.map(|(i, _)| (i + cfg.base).to_string()),
            "W" | "window_name" => window.map(|(_, n)| n.to_string()),
            "h" | "host_short" => Some(short.clone()),
            "H" | "host" => Some(full.clone()),
            "window_agent_state" => window.map(|(i, _)| state_name(window_state(i)).to_string()),
            "window_agent_glyph" => window.map(|(i, _)| cfg.glyph(window_state(i))),
            "agent_state" => Some(state_name(global).to_string()),
            "agent_glyph" => Some(cfg.glyph(global)),
            "agent_count" => Some(agents.global.filter(|&(_, n)| n > 1).map_or(String::new(), |(_, n)| n.to_string())),
            "F" => window.map(|(i, _)| lookup_flags(i)),
            "window_last_flag" => window.map(|(i, _)| if session.last_index() == Some(i) { "1" } else { "0" }.to_string()),
            // The options a `status-format` row reads with `#{E:name}`.
            "status-left" => Some(cfg.left.clone()),
            "status-right" => Some(cfg.right.clone()),
            "window-status-format" => Some(cfg.tab.clone()),
            "window-status-current-format" => Some(cfg.current.clone()),
            "window-status-separator" => Some(cfg.sep.clone()),
            "status-left-style" => Some(cfg.side_styles[0].clone()),
            "status-right-style" => Some(cfg.side_styles[1].clone()),
            "window-status-style" => Some(cfg.tab_styles[0].clone()),
            "window-status-current-style" => Some(cfg.tab_styles[1].clone()),
            "window-status-last-style" => Some(cfg.tab_styles[2].clone()),
            "version" => Some(format::COMPAT_VERSION.to_string()),
            "client_prefix" => Some(if segments.prefix(session.id) { "1" } else { "0" }.to_string()),
            "pid" => Some(std::process::id().to_string()),
            "socket_path" => Some(crate::paths::socket_path().to_string_lossy().into_owned()),
            _ => window.and_then(|(i, _)| segments.window_var(session.id, i, name)).or_else(|| segments.var(session.id, name)),
        }
    };
    let expanded = |fmt: &str, window: Option<(usize, &str)>| -> String {
        let lookup = |name: &str| window_lookup(window, name);
        let in_window = |i: usize, name: &str| session.windows.get(i).and_then(|w| window_lookup(Some((i, w.name.as_str())), name));
        let windows = format::Windows { count: session.windows.len(), current: session.current, lookup: &in_window };
        format::expand_with_windows(
            &format::strftime(fmt),
            &lookup,
            &|n| pane_safe(n) || segments.user_safe(n),
            &|command| match command {
                Ok(command) => segments.text(session.id, &command),
                Err(names) => refused_text(&names),
            },
            Some(windows),
        )
    };
    let unhash = |mut runs: Vec<Run>| -> Vec<Run> {
        for (text, _) in &mut runs {
            *text = format::restore_hash(text);
        }
        runs
    };
    let runs = |fmt: &str, window: Option<(usize, &str)>| -> Vec<Run> { unhash(format::runs(&expanded(fmt, window), cfg.bar)) };
    let styled = |style: &str, fmt: &str| if style.is_empty() { fmt.to_string() } else { format!("#[{style}]{fmt}") };
    let left = status::clip(&runs(&styled(&cfg.side_styles[0], &cfg.left), None), cfg.left_len);
    let right = status::clip(&runs(&styled(&cfg.side_styles[1], &cfg.right), None), cfg.right_len);
    let sep = runs(&cfg.sep, None);
    let tabs: Vec<Vec<Run>> = session
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let format = if i == session.current { &cfg.current } else { &cfg.tab };
            // A bell beats activity; the style comes first so the format can still override it.
            let style = if w.alerts.bell.is_some() {
                cfg.alert_styles[1].as_str()
            } else if w.alerts.activity.is_some() {
                cfg.alert_styles[0].as_str()
            } else {
                ""
            };
            // Every tab, then the current one or the last one, then its alert: later wins.
            let own = if i == session.current { cfg.tab_styles[1].as_str() } else if session.last_index() == Some(i) { cfg.tab_styles[2].as_str() } else { "" };
            let text = styled(style, format);
            let text = styled(own, &text);
            runs(&styled(&cfg.tab_styles[0], &text), Some((i, &w.name)))
        })
        .collect();
    let parts = Parts { bar: cfg.bar, left: &left, tabs: &tabs, sep: &sep, right: &right };
    let mut bar = status::layout_justified(cols, &parts, min_rows.max(cfg.rows), cfg.justify);
    for (i, format) in cfg.formats.iter().enumerate() {
        if let Some(format) = format {
            let text = expanded(format, None);
            if text.contains("align=") {
                let [left, center, right] = format::aligned_runs(&text, cfg.bar);
                bar.replace_row_aligned(cols, i, cfg.bar, &[unhash(left), unhash(center), unhash(right)]);
            } else {
                bar.replace_row(cols, i, cfg.bar, &unhash(format::runs(&text, cfg.bar)));
            }
        }
    }
    bar.agent_spans = glyph_spans(&bar.right, bar.right_start, &cfg.glyphs);
    bar
}

impl Server {
    pub(super) fn opt_ms(&self, name: &str) -> Duration {
        Duration::from_millis(self.options.num(name))
    }

    /// Number shown for the first window (`base-index`) and the first pane (`pane-base-index`).
    pub(super) fn base_index(&self) -> usize {
        self.options.num("base-index") as usize
    }

    pub(super) fn pane_base_index(&self) -> usize {
        self.options.num("pane-base-index") as usize
    }

    /// `(pane-border-style, pane-active-border-style)` as cell styles.
    pub(super) fn border_styles(&self) -> (Style, Style) {
        (self.options.style("pane-border-style").style(), self.options.style("pane-active-border-style").style())
    }

    /// `pane-border-status` as the compositor reads it.
    pub(super) fn pane_status(&self) -> crate::render::PaneStatus {
        crate::render::PaneStatus::from_name(self.options.text("pane-border-status"))
    }

    /// The part of a pane's rect its program gets: with `pane-border-status` the first or last row
    /// is the label row. A rect of one row has no label.
    pub(super) fn content_rect(&self, rect: crate::session::layout::Rect) -> crate::session::layout::Rect {
        self.pane_status().content(rect)
    }

    /// How panes look: `(window-style, window-style then window-active-style)`, what a pane that is
    /// not active, and the active one, take their default colors from, and `pane-border-lines`.
    pub(super) fn pane_look(&self) -> crate::render::Look {
        let plain = self.options.style("window-style");
        let active = self.options.style("window-active-style");
        crate::render::Look {
            bases: (plain.style(), active.apply(plain.style())),
            lines: crate::render::BorderLines::from_name(self.options.text("pane-border-lines")),
            status: self.pane_status(),
        }
    }

    pub(super) fn mode_style(&self) -> Style {
        self.options.style("mode-style").style()
    }

    /// The status line options, copied out so the bar can be built while sessions are borrowed.
    pub(super) fn status_cfg(&self) -> StatusCfg {
        let o = &self.options;
        let bar = o.style("status-style").style();
        StatusCfg {
            bar,
            left: o.text("status-left").to_string(),
            right: o.text("status-right").to_string(),
            left_len: o.num("status-left-length") as usize,
            right_len: o.num("status-right-length") as usize,
            tab: o.text("window-status-format").to_string(),
            current: o.text("window-status-current-format").to_string(),
            sep: o.text("window-status-separator").to_string(),
            base: self.base_index(),
            message: message_style(o.style("message-style"), bar),
            command: message_style(o.style("message-command-style"), bar),
            glyphs: ["working", "blocked", "done", "error"].map(|s| o.text(&format!("agent-glyph-{s}")).to_string()),
            agent_styles: ["working", "blocked", "done", "error"].map(|s| o.style(&format!("agent-style-{s}")).directive()),
            marked: self.marked_pane(),
            sync_default: o.flag("synchronize-panes"),
            alert_styles: ["activity", "bell"].map(|s| o.style(&format!("window-status-{s}-style")).directive()),
            side_styles: ["left", "right"].map(|s| o.style(&format!("status-{s}-style")).directive()),
            tab_styles: ["", "current-", "last-"].map(|s| o.style(&format!("window-status-{s}style")).directive()),
            justify: status::Justify::named(o.text("status-justify")),
            // `status 2..5` stays "on", so rows come from the formats: one past the last one set.
            rows: (0..crate::config::options::STATUS_FORMATS).rev().find(|&i| o.status_format(i).is_some()).map_or(1, |i| i + 1),
            formats: std::array::from_fn(|i| o.status_format(i).map(str::to_string)),
        }
    }

    /// The glyph of a state as plain text (pickers have no styles); empty for idle.
    pub(super) fn plain_glyph(&self, state: Option<State>) -> String {
        let name = match state {
            Some(State::Working) => "working",
            Some(State::Blocked) => "blocked",
            Some(State::Done) => "done",
            Some(State::Error) => "error",
            _ => return String::new(),
        };
        self.options.text(&format!("agent-glyph-{name}")).to_string()
    }

    /// The most urgent state among the panes of `windows` as `client` sees it.
    pub(super) fn windows_state<'a>(&self, windows: impl IntoIterator<Item = &'a Window>, client: usize) -> Option<State> {
        let now = Instant::now();
        agent::worst(
            windows
                .into_iter()
                .flat_map(|w| w.panes.values())
                .filter_map(|p| p.agent.as_ref().map(|r| agent::shown_to(r, Some(client), now))),
        )
        .map(|(s, _)| s)
    }

    /// The agent states a status bar shows for session `si` as `client` sees them (`None`: do not
    /// hide finished panes, for sizing the bar).
    pub(super) fn agent_states(&self, si: usize, client: Option<usize>) -> AgentStates {
        let now = Instant::now();
        let pane_state = |p: &crate::session::Pane| p.agent.as_ref().map(|r| agent::shown_to(r, client, now));
        let windows = self.sessions[si]
            .windows
            .iter()
            .map(|w| agent::worst(w.panes.values().filter_map(pane_state)).map(|(s, _)| s))
            .collect();
        let global = agent::worst(
            self.sessions.iter().flat_map(|s| s.windows.iter()).flat_map(|w| w.panes.values()).filter_map(pane_state),
        )
        .filter(|&(s, _)| s != State::Idle);
        AgentStates { windows, global }
    }

    /// Whether the condition of a `%if` line holds: a format with no pane in it.
    fn config_condition(&self, text: &str) -> bool {
        config::cond::holds(&self.options, text, &|name| self.environment_value(name))
    }

    /// Read the configuration file named by `config::default_path`, logging every problem.
    pub(super) fn load_config(&mut self) {
        let Some(path) = config::default_path() else {
            log::info("no configuration file; using defaults");
            return;
        };
        let errors = self.source_file(None, &path.to_string_lossy());
        log::info(&format!("loaded {} with {} error(s)", path.display(), errors.len()));
        self.config_errors = errors;
    }

    /// Run every statement of a file; returns one line per problem, each naming file and line.
    pub(super) fn source_file(&mut self, client: Option<usize>, path: &str) -> Vec<String> {
        self.source_file_with(client, path, false, false)
    }

    /// `source-file` with its flags: `quiet` (a missing file or a pattern with no match is no
    /// error), `parse_only` (read and check, change nothing), `verbose` (log each line read). The
    /// path may end in a pattern; the files are read in name order.
    pub(super) fn source_spec(&mut self, client: Option<usize>, spec: &SourceSpec) -> Vec<String> {
        let files = crate::paths::expand_source_path(&spec.path);
        if files.is_empty() {
            return if spec.quiet { Vec::new() } else { vec![format!("{}: no file matches", spec.path)] };
        }
        let mut errors = Vec::new();
        for file in files {
            let missing = !file.exists();
            if missing && spec.quiet {
                continue;
            }
            errors.extend(self.source_file_with(client, &file.to_string_lossy(), spec.parse_only, spec.verbose));
        }
        errors
    }

    fn source_file_with(&mut self, client: Option<usize>, path: &str, dry: bool, verbose: bool) -> Vec<String> {
        let mut dry_vars = if dry { self.vars.clone() } else { config::lex::Vars::new() };
        if self.source_depth >= MAX_SOURCE_DEPTH {
            return vec![format!("{path}: source-file nested too deeply")];
        }
        let text = match crate::paths::read_regular(std::path::Path::new(path), MAX_CONFIG_BYTES)
            .and_then(|b| String::from_utf8(b).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "not valid UTF-8")))
        {
            Ok(t) => t,
            Err(e) => return vec![format!("{path}: {e}")],
        };
        self.source_depth += 1;
        let mut errors = Vec::new();
        let mut conditions = config::cond::Conditions::default();
        for (line_no, line) in config::lex::logical_lines(&text) {
            let line = match conditions.step(&line, &|c| self.config_condition(c)) {
                config::cond::Step::Run(l) => l,
                config::cond::Step::Skip => continue,
                config::cond::Step::Error(e) => {
                    errors.push(format!("{path}:{line_no}: {e}"));
                    continue;
                }
            };
            if verbose {
                log::info(&format!("source-file: {path}:{line_no}: {line}"));
            }
            let parsed = if dry { stmt::parse_line_aliased(&line, &mut dry_vars, &self.aliases) } else { stmt::parse_line_aliased(&line, &mut self.vars, &self.aliases) };
            match parsed {
                Ok(stmts) if !dry => {
                    for s in stmts {
                        if let Err(e) = self.apply(client, s) {
                            errors.push(format!("{path}:{line_no}: {e}"));
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => errors.push(format!("{path}:{line_no}: {e}")),
            }
        }
        if conditions.open() {
            errors.push(format!("{path}: a %if has no %endif"));
        }
        self.source_depth -= 1;
        for e in &errors {
            log::warn(&format!("config: {e}"));
        }
        errors
    }

    /// `set command-alias name=command`, also written `command-alias[N] name=command`: another
    /// name for a command (with arguments, if given). `set -u command-alias` forgets all of
    /// them, `set -u command-alias[N]` the one in slot N.
    fn set_alias(&mut self, name: &str, value: Option<&str>) -> Result<(), String> {
        const MAX_ALIASES: usize = 64;
        let slot = match name.strip_prefix("command-alias[") {
            None => None,
            Some(rest) => Some(
                rest.strip_suffix(']')
                    .filter(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|n| n.parse::<u32>().ok())
                    .ok_or_else(|| format!("bad option name: {name}"))?,
            ),
        };
        let Some(value) = value else {
            match slot {
                Some(n) => {
                    if let Some(alias) = self.alias_slots.remove(&n) {
                        self.aliases.remove(&alias);
                    }
                }
                None => {
                    self.aliases.clear();
                    self.alias_slots.clear();
                }
            }
            return Ok(());
        };
        let Some((alias, command)) = value.split_once('=') else {
            return Err("command-alias: expected name=command".to_string());
        };
        let valid = !alias.is_empty()
            && alias.len() <= 32
            && alias.starts_with(|c: char| c.is_ascii_alphabetic())
            && alias.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !valid {
            return Err(format!("command-alias: bad alias name {alias:?}"));
        }
        if super::remote::knows(alias) {
            return Err(format!("command-alias: {alias} is a command already"));
        }
        if command.trim().is_empty() || command.len() > 512 || command.chars().any(char::is_control) {
            return Err("command-alias: the command is empty, too long or has a control character".to_string());
        }
        let replaces = slot.and_then(|n| self.alias_slots.get(&n)).cloned();
        if let Some(old) = replaces {
            self.aliases.remove(&old);
        }
        if !self.aliases.contains_key(alias) && self.aliases.len() >= MAX_ALIASES {
            return Err(format!("command-alias: at most {MAX_ALIASES} aliases"));
        }
        self.aliases.insert(alias.to_string(), command.to_string());
        if let Some(n) = slot {
            self.alias_slots.insert(n, alias.to_string());
        }
        Ok(())
    }

    /// `set`: give an option a value (or its default with `None`) and act on the ones with side
    /// effects.
    pub(super) fn set_option(&mut self, name: &str, value: Option<&str>, append: bool) -> Result<(), String> {
        if name == "command-alias" || name.starts_with("command-alias[") {
            return self.set_alias(name, value);
        }
        if name.starts_with('@') {
            self.options.set_user(name, value, append, self.from_pane)?;
            self.dirty = true;
            return Ok(());
        }
        if name == "status-format" || name.starts_with("status-format[") {
            if append {
                return Err(format!("{name}: cannot append"));
            }
            self.options.set_status_format(name, value)?;
            self.relayout();
            self.dirty = true;
            return Ok(());
        }
        if self.options.set(name, value, append)? == Set::Ignored {
            log::info(&format!("option {name} is accepted and has no effect"));
        }
        if name == "prefix" {
            let key = Key::parse(self.options.text("prefix")).ok_or("prefix: unknown key")?;
            self.bindings.set_prefix(key)?;
        }
        if name == "prefix2" {
            let key = Key::parse(self.options.text("prefix2"));
            self.bindings.set_prefix2(key)?;
        }
        if name == "image-copy-text" {
            crate::term::cell::set_image_text(self.options.text(name));
        }
        if name == "kitty-media" {
            crate::image::media::set_level(crate::image::media::Level::of(self.options.text(name)));
        }
        if name == "emoji-width" {
            crate::term::grid::set_emoji_clusters(self.options.text(name) == "cluster");
        }
        if name == "status" || name == "status-position" || name == "pane-border-status" {
            self.relayout();
        }
        if name == "autosave-history" || name == "autosave-history-interval" {
            self.sync_history();
        }
        if name == "window-size" || name == "default-size" {
            self.apply_size();
        }
        self.dirty = true;
        Ok(())
    }

    /// Apply one statement. `client` is who typed it; the configuration file has none.
    pub(super) fn apply(&mut self, client: Option<usize>, s: Stmt) -> Result<(), String> {
        let after = match &s {
            Stmt::Set { name, .. } | Stmt::SetGuarded { name, .. } => Some(("set-option", name.clone())),
            Stmt::Bind { .. } => Some(("bind-key", String::new())),
            Stmt::Unbind { .. } => Some(("unbind-key", String::new())),
            _ => None,
        };
        let result = self.apply_stmt(client, s);
        if result.is_ok()
            && let Some((command, arg)) = after
        {
            let args: Vec<String> = Some(arg).filter(|a| !a.is_empty()).into_iter().collect();
            self.emit_after(client, command, &args);
        }
        result
    }

    fn apply_stmt(&mut self, client: Option<usize>, s: Stmt) -> Result<(), String> {
        match s {
            Stmt::SetHook { name, command, append, unset, run } => self.set_hook(&name, command.as_deref(), append, unset, run),
            Stmt::Set { name, value, append } => self.set_option(&name, value.as_deref(), append),
            Stmt::SetGuarded { name, value, append, quiet, keep } => {
                if keep && self.options.was_set(&name) {
                    return Ok(());
                }
                if quiet && !self.options.knows(&name) {
                    return Ok(());
                }
                self.set_option(&name, value.as_deref(), append)
            }
            Stmt::Bind { root, key, binding } => {
                self.bindings.bind_key(root, key, binding);
                Ok(())
            }
            Stmt::Unbind { root, key } => {
                self.bindings.unbind_key(root, &key);
                Ok(())
            }
            Stmt::BindCopy { vi, key, op } => {
                self.bindings.bind_copy(vi, key, op);
                Ok(())
            }
            Stmt::UnbindCopy { vi, key } => {
                self.bindings.unbind_copy(vi, &key);
                Ok(())
            }
            Stmt::Ignored(why) => {
                if !why.is_empty() {
                    log::info(&format!("config: {why}"));
                }
                Ok(())
            }
            Stmt::Run(command) => match client {
                Some(id) => {
                    self.run_command(id, &command);
                    Ok(())
                }
                None => match command {
                    Command::SourceFile(path) => match self.source_file(None, &path).first() {
                        Some(e) => Err(e.clone()),
                        None => Ok(()),
                    },
                    Command::SourceWith(spec) => match self.source_spec(None, &spec).first() {
                        Some(e) => Err(e.clone()),
                        None => Ok(()),
                    },
                    Command::DisplayMessage(_) => Ok(()),
                    Command::Server(words) if words.first().is_some_and(|n| matches!(n.as_str(), "set-environment" | "setenv")) => {
                        self.cmd_set_environment(&words[1..]).map(|_| ())
                    }
                    Command::SetWindowOption { name, op, .. } => self.set_global_window_option(&name, op),
                    Command::RunShell(r) => {
                        self.run_shell(None, &r);
                        Ok(())
                    }
                    Command::IfShell(i) => {
                        self.if_shell(None, &i);
                        Ok(())
                    }
                    other => Err(format!("{other:?} needs a client")),
                },
            },
        }
    }

    /// `source-file` from a key or the prompt: report the outcome on the client's message row.
    pub(super) fn source_for(&mut self, client: usize, path: &str) {
        let errors = self.source_file(Some(client), path);
        self.source_report(client, &errors);
    }

    /// `source_for` for the forms with flags or a pattern.
    pub(super) fn source_with_for(&mut self, client: usize, spec: &SourceSpec) {
        let errors = self.source_spec(Some(client), spec);
        self.source_report(client, &errors);
    }

    fn source_report(&mut self, client: usize, errors: &[String]) {
        match errors.first() {
            Some(first) => self.show_message(client, format!("{} error(s); first: {first}", errors.len())),
            None => self.dirty = true,
        }
    }

    /// `show-options`: list the options nothing has read, the candidates for removal.
    pub(super) fn show_options(&mut self, client: usize) {
        let unused = self.options.unused();
        log::info(&format!("unused options: {}", unused.join(" ")));
        let text = if unused.is_empty() { "every option has been read".to_string() } else { format!("unused: {}", unused.join(" ")) };
        self.show_message(client, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::layout::PaneId;
    use crate::session::{Pane, Window};
    use crate::term::Terminal;
    use crate::tty::pty;

    fn window(name: &str, id: PaneId) -> Window {
        let size = nix::pty::Winsize { ws_row: 2, ws_col: 2, ws_xpixel: 0, ws_ypixel: 0 };
        let pty = pty::spawn("/usr/bin/true", &size, None).unwrap();
        Window::new(name.to_string(), id, Pane { pty, term: Terminal::new(2, 2, 0), copy: None, agent: None, typeahead: None, dead: None, start: None, input_off: false })
    }

    fn cfg(right: &str, tab: &str) -> StatusCfg {
        let bar = Style::default();
        StatusCfg {
            bar,
            left: String::new(),
            right: right.to_string(),
            left_len: 40,
            right_len: 40,
            tab: tab.to_string(),
            current: tab.to_string(),
            sep: String::new(),
            base: 1,
            message: (bar, bar),
            command: (bar, bar),
            glyphs: ["W", "B", "D", "E"].map(String::from),
            agent_styles: ["fg=red", "fg=green", "fg=blue", "fg=yellow"].map(String::from),
            marked: None,
            sync_default: false,
            alert_styles: ["reverse", "fg=red"].map(String::from),
            side_styles: [String::new(), String::new()],
            tab_styles: [String::new(), String::new(), String::new()],
            justify: status::Justify::Left,
            rows: 1,
            formats: Default::default(),
        }
    }

    fn text(s: &Status) -> String {
        s.cells.iter().filter(|c| c.width != 0).map(|c| c.ch).collect::<String>().trim_end().to_string()
    }

    fn session() -> Session {
        let mut s = Session::new(0, "s".into(), window("one", 0));
        s.add(window("two", 1));
        s
    }

    #[test]
    fn window_and_global_agent_variables_fill_the_bar() {
        let s = session();
        let agents = AgentStates { windows: vec![Some(State::Blocked), None], global: Some((State::Blocked, 2)) };
        let bar = status_for(&s, 40, 1, &cfg("#{agent_glyph}#{agent_count}", "#I#{window_agent_glyph}#{window_agent_state}"), &agents);
        let t = text(&bar);
        assert!(t.starts_with("1Bblocked2"), "{t}");
        assert!(t.ends_with("B2"), "{t}");
    }

    #[test]
    fn synchronized_windows_are_flagged_by_their_own_value_or_the_global_one() {
        let mut s = session();
        let agents = AgentStates { windows: vec![None, None], global: None };
        let c = cfg("", "#I=#{window_synchronized}#{pane_synchronized} ");
        assert!(text(&status_for(&s, 40, 1, &c, &agents)).starts_with("1=00 2=00"), "off by default");
        s.windows[1].opts.set("synchronize-panes", Some(true));
        assert!(text(&status_for(&s, 40, 1, &c, &agents)).starts_with("1=00 2=11"), "one window of its own");
        let mut on = cfg("", "#I=#{window_synchronized} ");
        on.sync_default = true;
        assert!(text(&status_for(&s, 40, 1, &on, &agents)).starts_with("1=1 2=1"), "the global option reaches both");
        s.windows[0].opts.set("synchronize-panes", Some(false));
        assert!(text(&status_for(&s, 40, 1, &on, &agents)).starts_with("1=0 2=1"), "an override of off beats the global on");
    }

    #[test]
    fn idle_and_unknown_states_show_no_glyph() {
        let s = session();
        let agents = AgentStates { windows: vec![Some(State::Idle), None], global: None };
        let bar = status_for(&s, 40, 1, &cfg("[#{agent_glyph}#{agent_state}#{agent_count}]", "#I#{window_agent_glyph}"), &agents);
        let t = text(&bar);
        assert!(t.starts_with("12"), "{t}");
        assert!(t.ends_with("[]"), "{t}");
    }

    #[test]
    fn a_glyph_is_drawn_in_its_style_and_the_rest_keeps_the_bar_style() {
        let s = session();
        let agents = AgentStates { windows: vec![None, None], global: Some((State::Working, 1)) };
        let bar = status_for(&s, 20, 1, &cfg("x#{agent_glyph}y", "#I"), &agents);
        let i = bar.cells.iter().position(|c| c.ch == 'W').unwrap();
        assert_ne!(bar.cells[i].style, bar.cells[i - 1].style);
        assert_eq!(bar.cells[i + 1].style, bar.cells[i - 1].style);
    }

    #[test]
    fn the_glyph_columns_are_found_for_clicks() {
        let s = session();
        let agents = AgentStates { windows: vec![None, None], global: Some((State::Done, 1)) };
        let bar = status_for(&s, 20, 1, &cfg("ab#{agent_glyph}c", "#I"), &agents);
        assert_eq!(bar.agent_spans, vec![(18, 19)]);
        let none = status_for(&s, 20, 1, &cfg("abc", "#I"), &AgentStates::default());
        assert!(none.agent_spans.is_empty());
    }

    #[test]
    fn window_flags_mark_the_current_window_and_a_zoomed_one() {
        let mut s = session();
        let tab = "#I:#W#{window_flags}";
        let t = text(&status_for(&s, 60, 1, &cfg("", tab), &AgentStates::default()));
        assert!(t.contains("2:two*"), "{t}");
        assert!(t.contains("1:one") && !t.contains("1:one*"), "{t}");
        let pane = s.windows[1].active;
        s.windows[1].layout.toggle_zoom(pane);
        assert!(!text(&status_for(&s, 60, 1, &cfg("", tab), &AgentStates::default())).contains('Z'), "one pane cannot be zoomed");
        let extra = window("x", 99).panes.remove(&99).unwrap();
        s.windows[1].panes.insert(99, extra);
        let area = crate::session::layout::Rect { x: 0, y: 0, w: 40, h: 10 };
        s.windows[1].layout.split(pane, crate::session::layout::Split::SideBySide, 99, area).unwrap();
        s.windows[1].layout.toggle_zoom(pane);
        let t = text(&status_for(&s, 60, 1, &cfg("", tab), &AgentStates::default()));
        assert!(t.contains("2:two*Z"), "{t}");
        assert!(t.contains("1:one") && !t.contains("1:one*"), "{t}");
    }

    #[test]
    fn a_window_with_an_alert_shows_the_flag_and_the_variables() {
        let mut s = session();
        let tab = "#I:#W#{window_flags}/#{window_activity_flag}#{window_bell_flag}";
        let quiet = text(&status_for(&s, 80, 1, &cfg("", tab), &AgentStates::default()));
        assert!(quiet.contains("1:one/00") && quiet.contains("2:two*/00"), "{quiet}");
        s.windows[0].alerts.activity = Some(0);
        let t = text(&status_for(&s, 80, 1, &cfg("", tab), &AgentStates::default()));
        assert!(t.contains("1:one#/10"), "{t}");
        s.windows[0].alerts.bell = Some(0);
        let t = text(&status_for(&s, 80, 1, &cfg("", tab), &AgentStates::default()));
        assert!(t.contains("1:one#!/11"), "activity first, then bell: {t}");
        s.windows[0].alerts.activity = None;
        let t = text(&status_for(&s, 80, 1, &cfg("", tab), &AgentStates::default()));
        assert!(t.contains("1:one!/01") && t.contains("2:two*/00"), "{t}");
    }

    #[test]
    fn a_tab_with_an_alert_is_drawn_in_the_alert_style_and_a_bell_beats_activity() {
        let mut s = session();
        let agents = AgentStates::default();
        let color_of = |s: &Session, ch: char| {
            let bar = status_for(s, 40, 1, &cfg("", "<#W>"), &agents);
            let i = bar.cells.iter().position(|c| c.ch == ch).unwrap();
            bar.cells[i].style
        };
        let plain = color_of(&s, 'o');
        s.windows[0].alerts.activity = Some(0);
        let activity = color_of(&s, 'o');
        assert_ne!(activity, plain, "activity changes the tab");
        s.windows[0].alerts.bell = Some(0);
        let bell = color_of(&s, 'o');
        assert_ne!(bell, activity, "a bell wins over activity");
        assert_eq!(color_of(&s, 't'), plain, "other tabs keep their look");
    }

    #[test]
    fn the_window_with_the_marked_pane_has_the_flag_m() {
        let s = session();
        let tab = "#I:#W#{window_flags}/#{window_marked_flag}";
        let mut c = cfg("", tab);
        let plain = text(&status_for(&s, 80, 1, &c, &AgentStates::default()));
        assert!(plain.contains("1:one/0") && plain.contains("2:two*/0"), "{plain}");
        c.marked = Some(0);
        let t = text(&status_for(&s, 80, 1, &c, &AgentStates::default()));
        assert!(t.contains("1:oneM/1") && t.contains("2:two*/0"), "{t}");
        c.marked = Some(99);
        assert!(text(&status_for(&s, 80, 1, &c, &AgentStates::default())).contains("1:one/0"), "a mark on a pane that is gone shows nowhere");
    }

    #[test]
    fn conditionals_choose_by_window_state_and_agent_state() {
        let s = session();
        let tab = "#{?window_active,<#W>,#W}";
        let t = text(&status_for(&s, 60, 1, &cfg("", tab), &AgentStates::default()));
        assert!(t.contains("one") && !t.contains("<one>"), "{t}");
        assert!(t.contains("<two>"), "{t}");
        let agents = AgentStates { windows: vec![None, None], global: Some((State::Blocked, 1)) };
        let right = "#{?#{==:#{agent_state},blocked},!#{agent_glyph}!,calm}";
        let t = text(&status_for(&s, 60, 1, &cfg(right, "#I"), &agents));
        assert!(t.contains("!B!"), "{t}");
        let t = text(&status_for(&s, 60, 1, &cfg(right, "#I"), &AgentStates::default()));
        assert!(t.contains("calm"), "{t}");
    }

    fn with_worktree(mut s: Session) -> Session {
        s.windows[1].meta = [
            ("worktree.repo", "/w/proj"),
            ("worktree.name", "ABC-7"),
            ("worktree.branch", "feature/ABC-7"),
            ("worktree.path", "/w/proj-worktrees/ABC-7"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        s
    }

    #[test]
    fn worktree_variables_come_from_the_window_a_tab_shows() {
        let s = with_worktree(session());
        let tab = "#I[#{worktree_name}|#{worktree_branch}|#{worktree_repo}|#{worktree_path}]";
        let bar = status_for(&s, 120, 1, &cfg("", tab), &AgentStates::default());
        let t = text(&bar);
        assert!(t.contains("1[|||]"), "a window without a worktree shows nothing: {t}");
        assert!(t.contains("2[ABC-7|feature/ABC-7|proj|/w/proj-worktrees/ABC-7]"), "{t}");
    }

    #[test]
    fn worktree_variables_in_the_left_and_right_parts_follow_the_current_window() {
        let mut s = with_worktree(session());
        s.current = 1;
        let mut c = cfg("R:#{worktree_name}", "#I");
        c.left = "L:#{worktree_repo}".to_string();
        let t = text(&status_for(&s, 60, 1, &c, &AgentStates::default()));
        assert!(t.starts_with("L:proj"), "{t}");
        assert!(t.ends_with("R:ABC-7"), "{t}");
        s.current = 0;
        let t = text(&status_for(&s, 60, 1, &c, &AgentStates::default()));
        assert!(t.starts_with("L:"), "{t}");
        assert!(t.ends_with("R:"), "{t}");
    }

    fn bar_with(right: &str, view: &SegmentView) -> Status {
        status_with(&session(), 60, 1, &cfg(right, "#I"), &AgentStates::default(), view)
    }

    #[test]
    fn a_command_shows_the_output_of_its_last_run() {
        let view = SegmentView::for_test(&[("echo hi", "hi there")], &[]);
        let t = text(&bar_with("<#(echo hi)>", &view));
        assert!(t.ends_with("<hi there>"), "{t}");
        let t = text(&bar_with("<#(echo other)>", &view));
        assert!(t.ends_with("<>"), "nothing yet: {t}");
        assert_eq!(*view.wanted.borrow(), vec![(0, "echo hi".to_string()), (0, "echo other".to_string())]);
    }

    #[test]
    fn a_command_is_not_wanted_when_the_bar_has_none() {
        let view = SegmentView::for_test(&[], &[]);
        bar_with("plain %% #(", &view);
        assert!(view.wanted.borrow().is_empty());
    }

    #[test]
    fn what_a_command_prints_cannot_style_or_expand() {
        let view = SegmentView::for_test(&[("x", "#[fg=red]#{session_name}#(y)##")], &[]);
        let bar = bar_with("[#(x)]", &view);
        let t = text(&bar);
        assert!(t.ends_with("[#[fg=red]#{session_name}#(y)##]"), "{t}");
        let first = bar.cells.iter().position(|c| c.ch == '[').unwrap();
        let last = bar.cells.iter().rposition(|c| c.ch == ']').unwrap();
        assert!(bar.cells[first..=last].iter().all(|c| c.style == bar.cells[first].style), "one style all through");
        assert_eq!(view.wanted.borrow().len(), 1, "and nothing it printed became a command");
    }

    #[test]
    fn the_text_of_a_command_takes_the_variables_of_the_current_pane() {
        let vars = [(0, "pane_current_path", "/tmp/a b"), (0, "pane_id", "%3"), (0, "pane_width", "80")];
        let view = SegmentView::for_test(&[("git -C '/tmp/a b' branch", "main")], &vars);
        let t = text(&bar_with("#(git -C #{q:pane_current_path} branch)", &view));
        assert!(t.ends_with("main"), "{t}");
        bar_with("#(echo #{pane_id}#{pane_width})", &view);
        assert!(view.wanted.borrow().iter().any(|(_, c)| c == "echo %380"), "ids and sizes need no quoting");
    }

    #[test]
    fn a_command_with_a_variable_a_pane_could_set_is_refused_and_says_so() {
        let view = SegmentView::for_test(&[], &[(0, "pane_current_path", "/tmp/$(boom)")]);
        let t = text(&bar_with("[#(ls #{pane_current_path})]", &view));
        assert!(t.ends_with("[[#{q:pane_current_path}?]]"), "{t}");
        assert!(view.wanted.borrow().is_empty(), "so nothing is run");
        let t = text(&bar_with("[#(ls #{session_name})]", &view));
        assert!(t.ends_with("[[#{q:session_name}?]]"), "{t}");
        let t = text(&bar_with("[#(ls #{q:session_name})]", &view));
        assert!(t.ends_with("[]"), "quoted it is fine, and nothing is known yet: {t}");
        assert!(view.wanted.borrow().iter().any(|(_, c)| c == "ls 's'"));
    }

    #[test]
    fn the_variables_of_the_pane_reach_status_text_too() {
        let view = SegmentView::for_test(&[], &[(0, "pane_current_path", "/tmp/a")]);
        assert!(text(&bar_with("<#{pane_current_path}>", &view)).ends_with("</tmp/a>"));
        assert!(text(&bar_with("<#{pane_current_path}>", &SegmentView::default())).ends_with("<>"));
        let other = SegmentView::for_test(&[], &[(7, "pane_current_path", "/tmp/a")]);
        assert!(text(&bar_with("<#{pane_current_path}>", &other)).ends_with("<>"), "another session's pane");
    }
}
