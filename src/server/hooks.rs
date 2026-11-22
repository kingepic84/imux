//! Event hooks: `set-hook name command` runs an imux command line when something happens.
//!
//! Two kinds of event. `after-<command>` is emitted by the places that run commands (the dispatcher
//! for keys and the prompt, the socket for `imux <command>`, the statements `set`, `bind` and
//! `unbind`). Everything else is found by comparing a snapshot of the sessions, windows, panes and
//! clients with the one of the loop turn before (`diff`, a pure function), so no code path that
//! changes a window name or closes a pane has to know about hooks, and a hook never runs inside
//! the code that caused it: events are queued and run at the end of the loop turn.
//!
//! Limits: hooks started by hooks nest at most `MAX_DEPTH` deep, one event name fires at most
//! `MAX_PER_SECOND` times a second (the rest is dropped, with one log line), at most `MAX_RUNS`
//! hook commands run per loop turn.
//!
//! The variables of an event (`#{hook_window_name}` and so on) can be set by a program in a pane
//! (names, titles, messages), so they are kept out of shell text like every such variable: see
//! `shellcmd::pane_safe`. Control characters are removed from all of them.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use super::Server;
use super::conf::state_name;
use crate::config::stmt;
use crate::log;
use crate::session::Alerts;
use crate::session::layout::PaneId;
use crate::term::status::State;

const MAX_DEPTH: usize = 3;
const MAX_PER_SECOND: u32 = 20;
const MAX_RUNS: usize = 50;
const MAX_HOOKS: usize = 64;
const MAX_SLOTS: u32 = 16;
const MAX_COMMAND: usize = 4096;
const MAX_VALUE: usize = 256;

/// Events imux fires.
pub(super) const ACTIVE: &[&str] = &[
    "session-created",
    "session-closed",
    "session-renamed",
    "session-window-changed",
    "client-attached",
    "client-detached",
    "client-resized",
    "client-session-changed",
    "window-renamed",
    "window-layout-changed",
    "window-pane-changed",
    "window-linked",
    "window-unlinked",
    "pane-exited",
    "pane-title-changed",
    "pane-mode-changed",
    "command-error",
    "agent-state-changed",
    "worktree-created",
    "worktree-removed",
    "pane-died",
    "pane-focus-in",
    "pane-focus-out",
    "client-focus-in",
    "client-focus-out",
    "alert-activity",
    "alert-bell",
    "alert-silence",
];

/// Names that are accepted so that a configuration loads, but nothing fires them yet.
const INACTIVE: &[&str] = &[
    "pane-set-clipboard",
    "client-light-theme",
    "client-dark-theme",
];

/// The names of `hook_argument_N`.
const ARGUMENTS: [&str; 10] = [
    "hook_argument_0",
    "hook_argument_1",
    "hook_argument_2",
    "hook_argument_3",
    "hook_argument_4",
    "hook_argument_5",
    "hook_argument_6",
    "hook_argument_7",
    "hook_argument_8",
    "hook_argument_9",
];

pub(super) type Vars = Vec<(&'static str, String)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Known {
    Active,
    Inactive,
    Unknown,
}

fn known(name: &str) -> Known {
    if ACTIVE.contains(&name) || name.starts_with("after-") {
        Known::Active
    } else if INACTIVE.contains(&name) {
        Known::Inactive
    } else {
        Known::Unknown
    }
}

/// `name` or `name[N]`: a hook name made of lowercase letters, digits and `-`, with a slot.
fn parse_name(text: &str) -> Result<(String, Option<u32>), String> {
    let (base, slot) = match text.split_once('[') {
        None => (text, None),
        Some((base, rest)) => {
            let n = rest
                .strip_suffix(']')
                .filter(|n| !n.is_empty() && n.len() <= 3 && n.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n < MAX_SLOTS)
                .ok_or_else(|| format!("bad hook slot in {text:?} (0 to {})", MAX_SLOTS - 1))?;
            (base, Some(n))
        }
    };
    let ok = base.len() <= 48
        && base.starts_with(|c: char| c.is_ascii_lowercase())
        && base.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok { Ok((base.to_string(), slot)) } else { Err(format!("bad hook name: {text:?}")) }
}

/// A value for a hook variable: no control characters, at most `MAX_VALUE` of them.
pub(super) fn clean(value: &str) -> String {
    value.chars().filter(|&c| !crate::sanitize::is_unsafe(c)).take(MAX_VALUE).collect()
}

struct Event {
    name: String,
    client: Option<usize>,
    vars: Vars,
    depth: usize,
}

#[derive(Default)]
pub(super) struct Hooks {
    /// Hook name to commands by slot.
    table: BTreeMap<String, BTreeMap<u32, String>>,
    /// What the sessions looked like at the end of the last loop turn that looked.
    last: Option<Snap>,
    pending: VecDeque<Event>,
    /// Depth of the hook that is running now; events it causes are one deeper.
    depth: usize,
    /// Events emitted in the last second, by name.
    rate: HashMap<String, (Instant, u32)>,
    warned: HashSet<String>,
    /// The variables of the hook that is running now.
    pub(super) vars: HashMap<&'static str, String>,
    /// Set when an `after-` event was emitted for the command being run.
    after_sent: bool,
}

impl Hooks {
    pub(super) fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

// What the observer sees.

#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionSnap {
    name: String,
    /// Id of the current window.
    current: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowSnap {
    session: usize,
    name: String,
    active: PaneId,
    /// The panes in layout order with their sizes, and whether one is zoomed.
    layout: Vec<(PaneId, usize, usize)>,
    zoomed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneSnap {
    window: usize,
    session: usize,
    title: String,
    in_mode: bool,
    agent: Option<(State, Option<String>)>,
    /// The exit status of a program that ended in a pane `remain-on-exit` kept.
    dead: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClientSnap {
    session: usize,
    /// `None` until the client has a size, and when it is going away.
    size: Option<(u16, u16)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Snap {
    sessions: BTreeMap<usize, SessionSnap>,
    windows: BTreeMap<usize, WindowSnap>,
    panes: BTreeMap<PaneId, PaneSnap>,
    clients: BTreeMap<usize, ClientSnap>,
    /// The windows that have an activity or bell flag.
    alerts: BTreeMap<usize, Alerts>,
    /// The names of agents (`imux agent name`), by pane.
    agent_names: BTreeMap<PaneId, String>,
}

/// An event found by `diff`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Fired {
    name: &'static str,
    client: Option<usize>,
    vars: Vars,
}

fn id_vars(snap: &Snap, session: Option<usize>, window: Option<usize>, pane: Option<PaneId>) -> Vars {
    let mut v = Vars::new();
    if let Some(s) = session.and_then(|id| snap.sessions.get(&id).map(|s| (id, s))) {
        v.push(("hook_session", format!("${}", s.0)));
        v.push(("hook_session_name", clean(&s.1.name)));
    }
    if let Some(w) = window.and_then(|id| snap.windows.get(&id).map(|w| (id, w))) {
        v.push(("hook_window", format!("@{}", w.0)));
        v.push(("hook_window_name", clean(&w.1.name)));
    }
    if let Some(p) = pane {
        v.push(("hook_pane", format!("%{p}")));
    }
    v
}

/// The events that turn `old` into `new`, in a fixed order.
pub(super) fn diff(old: &Snap, new: &Snap) -> Vec<Fired> {
    let mut out = Vec::new();
    let mut fire = |name: &'static str, client: Option<usize>, vars: Vars| out.push(Fired { name, client, vars });

    for sid in new.sessions.keys().filter(|id| !old.sessions.contains_key(id)) {
        fire("session-created", None, id_vars(new, Some(*sid), None, None));
    }
    for sid in old.sessions.keys().filter(|id| !new.sessions.contains_key(id)) {
        fire("session-closed", None, id_vars(old, Some(*sid), None, None));
    }
    for (sid, s) in &new.sessions {
        let Some(before) = old.sessions.get(sid) else { continue };
        if before.name != s.name {
            fire("session-renamed", None, id_vars(new, Some(*sid), None, None));
        }
        if before.current != s.current {
            fire("session-window-changed", None, id_vars(new, Some(*sid), Some(s.current), None));
        }
    }

    for (wid, w) in new.windows.iter().filter(|(id, _)| !old.windows.contains_key(id)) {
        fire("window-linked", None, id_vars(new, Some(w.session), Some(*wid), None));
    }
    for (wid, w) in old.windows.iter().filter(|(id, _)| !new.windows.contains_key(id)) {
        fire("window-unlinked", None, id_vars(old, Some(w.session), Some(*wid), None));
    }
    for (wid, w) in &new.windows {
        let Some(before) = old.windows.get(wid) else { continue };
        let vars = || id_vars(new, Some(w.session), Some(*wid), None);
        if before.name != w.name {
            fire("window-renamed", None, vars());
        }
        if before.layout != w.layout || before.zoomed != w.zoomed {
            fire("window-layout-changed", None, vars());
        }
        if before.active != w.active {
            fire("window-pane-changed", None, id_vars(new, Some(w.session), Some(*wid), Some(w.active)));
        }
    }

    for (wid, a) in &new.alerts {
        let (Some(w), before) = (new.windows.get(wid), old.alerts.get(wid).copied().unwrap_or_default()) else { continue };
        if let (None, Some(pane)) = (before.activity, a.activity) {
            fire("alert-activity", None, id_vars(new, Some(w.session), Some(*wid), Some(pane)));
        }
        if let (None, Some(pane)) = (before.bell, a.bell) {
            fire("alert-bell", None, id_vars(new, Some(w.session), Some(*wid), Some(pane)));
        }
        if let (None, Some(pane)) = (before.silence, a.silence) {
            fire("alert-silence", None, id_vars(new, Some(w.session), Some(*wid), Some(pane)));
        }
    }

    for (pid, p) in old.panes.iter().filter(|(id, _)| !new.panes.contains_key(id)) {
        fire("pane-exited", None, id_vars(old, Some(p.session), Some(p.window), Some(*pid)));
    }
    for (pid, p) in &new.panes {
        let Some(before) = old.panes.get(pid) else { continue };
        let vars = || id_vars(new, Some(p.session), Some(p.window), Some(*pid));
        if before.title != p.title {
            fire("pane-title-changed", None, vars());
        }
        if before.in_mode != p.in_mode {
            fire("pane-mode-changed", None, vars());
        }
        if let (None, Some(code)) = (before.dead, p.dead) {
            let mut v = vars();
            v.push(("hook_exit_status", code.to_string()));
            fire("pane-died", None, v);
        }
        let name = new.agent_names.get(pid);
        if before.agent != p.agent || old.agent_names.get(pid) != name {
            let mut v = vars();
            v.push(("hook_state", state_name(p.agent.as_ref().map(|a| a.0)).to_string()));
            v.push(("hook_message", clean(p.agent.as_ref().and_then(|a| a.1.as_deref()).unwrap_or(""))));
            if let Some(name) = name {
                v.push(("hook_name", clean(name)));
            }
            fire("agent-state-changed", None, v);
        }
    }

    for (cid, c) in &new.clients {
        let was = old.clients.get(cid);
        let mut vars = id_vars(new, Some(c.session), None, None);
        vars.push(("hook_client", cid.to_string()));
        match (was, c.size) {
            (None | Some(ClientSnap { size: None, .. }), Some(_)) => fire("client-attached", Some(*cid), vars),
            (Some(before), Some(now)) if before.size.is_some() => {
                if before.session != c.session {
                    fire("client-session-changed", Some(*cid), vars);
                } else if before.size != Some(now) {
                    fire("client-resized", Some(*cid), vars);
                }
            }
            _ => {}
        }
    }
    for (cid, before) in &old.clients {
        let gone = new.clients.get(cid).is_none_or(|c| c.size.is_none());
        if gone && before.size.is_some() {
            let mut vars = id_vars(old, Some(before.session), None, None);
            vars.push(("hook_client", cid.to_string()));
            fire("client-detached", None, vars);
        }
    }
    out
}

impl Server {
    fn snapshot(&self) -> Snap {
        let mut snap = Snap::default();
        for s in &self.sessions {
            snap.sessions.insert(s.id, SessionSnap { name: s.name.clone(), current: s.window().id });
            for w in &s.windows {
                let layout = w
                    .layout
                    .panes()
                    .into_iter()
                    .filter_map(|id| w.panes.get(&id).map(|p| (id, p.term.grid.cols, p.term.grid.rows)))
                    .collect();
                snap.windows.insert(
                    w.id,
                    WindowSnap { session: s.id, name: w.name.clone(), active: w.active, layout, zoomed: w.layout.zoomed().is_some() },
                );
                if w.alerts.any() {
                    snap.alerts.insert(w.id, w.alerts);
                }
                for (id, p) in &w.panes {
                    snap.panes.insert(
                        *id,
                        PaneSnap {
                            window: w.id,
                            session: s.id,
                            title: p.term.grid.title.clone(),
                            in_mode: p.copy.is_some(),
                            agent: p.agent.as_ref().map(|a| (a.state, a.msg.clone())),
                            dead: p.dead,
                        },
                    );
                    if let Some(name) = p.agent.as_ref().and_then(|a| a.name.as_ref()) {
                        snap.agent_names.insert(*id, name.clone());
                    }
                }
            }
        }
        for (id, c) in &self.clients {
            let size = c.size.filter(|_| !c.closing).map(|s| (s.cols, s.rows));
            snap.clients.insert(*id, ClientSnap { session: c.session, size });
        }
        snap
    }

    /// Tell the event stream, and queue the event if a hook listens for it, within the limits.
    pub(super) fn emit(&mut self, name: &str, client: Option<usize>, vars: Vars) {
        if self.stream.wants(name) {
            self.publish(name, &vars);
        }
        if !self.hooks.table.contains_key(name) {
            return;
        }
        let depth = self.hooks.depth;
        if depth > MAX_DEPTH {
            log::warn(&format!("hook {name}: dropped, hooks started by hooks go {MAX_DEPTH} deep at most"));
            return;
        }
        let now = Instant::now();
        let entry = self.hooks.rate.entry(name.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(1) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > MAX_PER_SECOND {
            if entry.1 == MAX_PER_SECOND + 1 {
                log::warn(&format!("hook {name}: more than {MAX_PER_SECOND} a second, the rest is dropped"));
            }
            return;
        }
        self.hooks.pending.push_back(Event { name: name.to_string(), client, vars, depth });
    }

    /// `after-<command>` for a command that has just been run.
    pub(super) fn emit_after(&mut self, client: Option<usize>, command: &str, words: &[String]) {
        let name = format!("after-{}", canonical(command));
        self.hooks.after_sent = true;
        if !self.hooks.table.contains_key(&name) && !self.stream.wants(&name) {
            return;
        }
        let mut vars: Vars = words.iter().take(ARGUMENTS.len()).enumerate().map(|(i, w)| (ARGUMENTS[i], clean(w))).collect();
        vars.push(("hook_command", clean(command)));
        self.emit(&name, client, vars);
    }

    /// `command-error`: a command failed; `message` is what the user is told.
    pub(super) fn emit_command_error(&mut self, client: Option<usize>, command: &str, message: &str) {
        if self.hooks.depth > 0 {
            return;
        }
        let vars = vec![("hook_command", clean(command)), ("hook_message", clean(message))];
        self.emit("command-error", client, vars);
    }

    /// Whether an `after-` event was emitted since the last call; the socket uses it to emit one
    /// only for commands that did not go through the dispatcher.
    pub(super) fn take_after_sent(&mut self) -> bool {
        std::mem::take(&mut self.hooks.after_sent)
    }

    /// Look for events and run the hooks that are due. Called once at the end of a loop turn.
    pub(super) fn run_hooks(&mut self) {
        if self.hooks.is_empty() && self.stream.is_empty() {
            self.hooks.last = None;
            return;
        }
        self.number_windows();
        let now = self.snapshot();
        if let Some(old) = self.hooks.last.take() {
            for f in diff(&old, &now) {
                let client = f.client.or_else(|| self.client_for_vars(&f.vars));
                let mut vars = f.vars;
                vars.push(("hook", f.name.to_string()));
                self.emit(f.name, client, vars);
            }
        }
        self.hooks.last = Some(now);
        let mut runs = 0;
        while runs < MAX_RUNS {
            let Some(event) = self.hooks.pending.pop_front() else { break };
            runs += self.run_event(event);
        }
    }

    /// A client looking at the session of an event, for commands that need one (`display-message`).
    fn client_for_vars(&self, vars: &Vars) -> Option<usize> {
        let session = vars.iter().find(|(k, _)| *k == "hook_session")?.1.strip_prefix('$')?.parse::<usize>().ok()?;
        self.clients.iter().filter(|(_, c)| c.session == session && c.size.is_some() && !c.closing).map(|(id, _)| *id).min()
    }

    /// Run the commands of one event; returns how many there were.
    fn run_event(&mut self, event: Event) -> usize {
        let commands: Vec<String> = self.hooks.table.get(&event.name).map(|m| m.values().cloned().collect()).unwrap_or_default();
        self.hooks.depth = event.depth + 1;
        let count = commands.len().max(1);
        // Without a client of its own the event uses one that looks at the session used last.
        let client = event.client.or_else(|| {
            self.clients.iter().filter(|(_, c)| c.session == self.recent && c.size.is_some() && !c.closing).map(|(id, _)| *id).min()
        });
        for text in commands {
            self.hooks.vars = event.vars.iter().cloned().collect();
            self.hooks.vars.entry("hook").or_insert_with(|| event.name.clone());
            if let Err(e) = self.run_hook_text(client, &text) {
                log::warn(&format!("hook {}: {text:?}: {e}", event.name));
            }
        }
        self.hooks.vars.clear();
        self.hooks.depth = 0;
        count
    }

    fn run_hook_text(&mut self, client: Option<usize>, text: &str) -> Result<(), String> {
        let stmts = stmt::parse_line_aliased(text, &mut self.vars, &self.aliases)?;
        match client.filter(|c| self.clients.contains_key(c)) {
            Some(id) => self.apply_queue(id, stmts.into()),
            None => stmts.into_iter().try_for_each(|s| self.apply(None, s)),
        }
    }

    /// `set-hook`.
    pub(super) fn set_hook(&mut self, target: &str, value: Option<&str>, append: bool, unset: bool, run: bool) -> Result<(), String> {
        let (name, slot) = parse_name(target)?;
        if run {
            return self.run_hook_now(&name);
        }
        if unset {
            match slot {
                None => {
                    self.hooks.table.remove(&name);
                }
                Some(n) => {
                    if let Some(slots) = self.hooks.table.get_mut(&name) {
                        slots.remove(&n);
                        if slots.is_empty() {
                            self.hooks.table.remove(&name);
                        }
                    }
                }
            }
            return Ok(());
        }
        let Some(command) = value else { return Err(format!("set-hook: {target} needs a command")) };
        if command.len() > MAX_COMMAND || command.contains('\0') {
            return Err("set-hook: the command is too long".to_string());
        }
        match known(&name) {
            Known::Active => {}
            Known::Inactive => self.warn_once(&name, "is accepted but nothing fires it yet"),
            Known::Unknown => self.warn_once(&name, "is not an event imux knows; it is kept and never runs"),
        }
        if !self.hooks.table.contains_key(&name) && self.hooks.table.len() >= MAX_HOOKS {
            return Err(format!("set-hook: at most {MAX_HOOKS} hooks"));
        }
        // Events count from the moment the first hook exists.
        if self.hooks.last.is_none() {
            self.number_windows();
            self.hooks.last = Some(self.snapshot());
        }
        let slots = self.hooks.table.entry(name).or_default();
        match (slot, append) {
            (Some(n), _) => {
                slots.insert(n, command.to_string());
            }
            (None, true) => {
                let next = slots.keys().next_back().map_or(0, |n| n + 1);
                if next >= MAX_SLOTS {
                    return Err(format!("set-hook: at most {MAX_SLOTS} commands for one hook"));
                }
                slots.insert(next, command.to_string());
            }
            (None, false) => {
                slots.clear();
                slots.insert(0, command.to_string());
            }
        }
        Ok(())
    }

    fn warn_once(&mut self, name: &str, why: &str) {
        if self.hooks.warned.insert(name.to_string()) {
            log::warn(&format!("hook {name} {why}"));
        }
    }

    /// `set-hook -R`: run the hook now, without variables.
    fn run_hook_now(&mut self, name: &str) -> Result<(), String> {
        if !self.hooks.table.contains_key(name) {
            return Err(format!("no hook {name}"));
        }
        let vars = vec![("hook", name.to_string())];
        let depth = self.hooks.depth;
        self.run_event(Event { name: name.to_string(), client: None, vars, depth });
        self.hooks.depth = depth;
        Ok(())
    }

    /// `show-hooks`: one line per command, `name[slot] command`.
    pub(super) fn hook_lines(&self) -> Vec<String> {
        self.hooks.table.iter().flat_map(|(name, slots)| slots.iter().map(move |(n, c)| format!("{name}[{n}] {c}"))).collect()
    }
}

/// The full name of a command that has a short one (`neww`), so a hook is written once.
fn canonical(name: &str) -> &str {
    match name {
        "neww" => "new-window",
        "splitw" => "split-window",
        "selectw" => "select-window",
        "selectp" => "select-pane",
        "killw" => "kill-window",
        "killp" => "kill-pane",
        "renamew" => "rename-window",
        "rename" => "rename-session",
        "new" => "new-session",
        "set" | "setw" | "set-window-option" => "set-option",
        "bind" => "bind-key",
        "unbind" => "unbind-key",
        "source" => "source-file",
        "send" => "send-keys",
        "display" => "display-message",
        "capturep" => "capture-pane",
        "respawnp" => "respawn-pane",
        "respawnw" => "respawn-window",
        "joinp" => "join-pane",
        "movep" => "move-pane",
        "pipep" => "pipe-pane",
        "wait" => "wait-for",
        "setenv" => "set-environment",
        "if" => "if-shell",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snap {
        let mut s = Snap::default();
        s.sessions.insert(1, SessionSnap { name: "work".into(), current: 10 });
        s.windows.insert(
            10,
            WindowSnap { session: 1, name: "edit".into(), active: 100, layout: vec![(100, 80, 24)], zoomed: false },
        );
        s.panes.insert(100, PaneSnap { window: 10, session: 1, title: "t".into(), in_mode: false, agent: None, dead: None });
        s.clients.insert(5, ClientSnap { session: 1, size: Some((80, 24)) });
        s
    }

    fn names(old: &Snap, new: &Snap) -> Vec<&'static str> {
        diff(old, new).into_iter().map(|f| f.name).collect()
    }

    #[test]
    fn nothing_changed_nothing_fires() {
        assert!(diff(&snap(), &snap()).is_empty());
    }

    #[test]
    fn a_new_session_with_its_window_pane_and_client() {
        let mut new = snap();
        new.sessions.insert(2, SessionSnap { name: "play".into(), current: 20 });
        new.windows.insert(20, WindowSnap { session: 2, name: "sh".into(), active: 200, layout: vec![(200, 80, 24)], zoomed: false });
        new.panes.insert(200, PaneSnap { window: 20, session: 2, title: String::new(), in_mode: false, agent: None, dead: None });
        let fired = diff(&snap(), &new);
        assert_eq!(fired.iter().map(|f| f.name).collect::<Vec<_>>(), ["session-created", "window-linked"]);
        assert!(fired[0].vars.contains(&("hook_session_name", "play".to_string())));
        assert!(fired[1].vars.contains(&("hook_window", "@20".to_string())));
    }

    #[test]
    fn closing_a_session_fires_its_closing_and_what_was_in_it() {
        let mut new = snap();
        new.sessions.clear();
        new.windows.clear();
        new.panes.clear();
        assert_eq!(names(&snap(), &new), ["session-closed", "window-unlinked", "pane-exited"]);
    }

    #[test]
    fn renames_layout_and_focus_changes() {
        let mut new = snap();
        new.sessions.get_mut(&1).unwrap().name = "job".into();
        new.sessions.get_mut(&1).unwrap().current = 11;
        let w = new.windows.get_mut(&10).unwrap();
        w.name = "ed".into();
        w.layout = vec![(100, 40, 24), (101, 40, 24)];
        w.active = 101;
        assert_eq!(
            names(&snap(), &new),
            ["session-renamed", "session-window-changed", "window-renamed", "window-layout-changed", "window-pane-changed"]
        );
    }

    #[test]
    fn zoom_alone_is_a_layout_change() {
        let mut new = snap();
        new.windows.get_mut(&10).unwrap().zoomed = true;
        assert_eq!(names(&snap(), &new), ["window-layout-changed"]);
    }

    #[test]
    fn pane_changes_carry_their_variables() {
        let mut new = snap();
        let p = new.panes.get_mut(&100).unwrap();
        p.title = "vim".into();
        p.in_mode = true;
        p.agent = Some((State::Blocked, Some("approve Bash".into())));
        let fired = diff(&snap(), &new);
        assert_eq!(fired.iter().map(|f| f.name).collect::<Vec<_>>(), ["pane-title-changed", "pane-mode-changed", "agent-state-changed"]);
        let agent = &fired[2].vars;
        assert!(agent.contains(&("hook_state", "blocked".to_string())));
        assert!(agent.contains(&("hook_message", "approve Bash".to_string())));
        assert!(agent.contains(&("hook_pane", "%100".to_string())));
    }

    #[test]
    fn a_pane_that_is_gone_exited() {
        let mut new = snap();
        new.panes.clear();
        let fired = diff(&snap(), &new);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].name, "pane-exited");
        assert!(fired[0].vars.contains(&("hook_window_name", "edit".to_string())));
    }

    #[test]
    fn client_events() {
        let mut new = snap();
        new.clients.get_mut(&5).unwrap().size = Some((100, 30));
        assert_eq!(names(&snap(), &new), ["client-resized"]);
        let fired = diff(&snap(), &new);
        assert_eq!(fired[0].client, Some(5));
        assert!(fired[0].vars.contains(&("hook_client", "5".to_string())));

        let mut other = snap();
        other.sessions.insert(2, SessionSnap { name: "x".into(), current: 20 });
        other.clients.get_mut(&5).unwrap().session = 2;
        assert!(names(&snap(), &other).contains(&"client-session-changed"));

        let mut gone = snap();
        gone.clients.clear();
        assert_eq!(names(&snap(), &gone), ["client-detached"]);
        let mut closing = snap();
        closing.clients.get_mut(&5).unwrap().size = None;
        assert_eq!(names(&snap(), &closing), ["client-detached"]);

        let mut before = snap();
        before.clients.get_mut(&5).unwrap().size = None;
        assert_eq!(names(&before, &snap()), ["client-attached"]);
        let mut joined = snap();
        joined.clients.insert(6, ClientSnap { session: 1, size: Some((10, 10)) });
        assert_eq!(names(&snap(), &joined), ["client-attached"]);
    }

    #[test]
    fn hook_names_and_slots() {
        assert_eq!(parse_name("after-new-window"), Ok(("after-new-window".to_string(), None)));
        assert_eq!(parse_name("pane-exited[3]"), Ok(("pane-exited".to_string(), Some(3))));
        for bad in ["", "Pane", "a b", "x[]", "x[16]", "x[a]", "x[1", "-x", "x;y", &"a".repeat(49)] {
            assert!(parse_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn event_names_are_active_inactive_or_unknown() {
        assert_eq!(known("pane-exited"), Known::Active);
        assert_eq!(known("after-split-window"), Known::Active);
        assert_eq!(known("alert-bell"), Known::Active);
        assert_eq!(known("whatever"), Known::Unknown);
    }

    #[test]
    fn variable_values_lose_control_characters_and_are_cut() {
        assert_eq!(clean("a\x1b[31mb\nc"), "a[31mbc");
        assert_eq!(clean(&"x".repeat(1000)).len(), MAX_VALUE);
    }

    #[test]
    fn a_pane_that_ends_in_a_kept_pane_dies_once_with_its_status() {
        let mut dead = snap();
        dead.panes.get_mut(&100).unwrap().dead = Some(3);
        let fired = diff(&snap(), &dead);
        assert_eq!(fired.iter().map(|f| f.name).collect::<Vec<_>>(), ["pane-died"]);
        assert!(fired[0].vars.contains(&("hook_exit_status", "3".to_string())));
        assert!(fired[0].vars.contains(&("hook_pane", "%100".to_string())));
        assert!(diff(&dead, &dead).is_empty(), "it does not die again");
        let mut gone = snap();
        gone.panes.clear();
        assert_eq!(diff(&dead, &gone).iter().map(|f| f.name).collect::<Vec<_>>(), ["pane-exited"], "killing it is an exit");
    }

    #[test]
    fn the_focus_and_death_hooks_fire() {
        for name in ["pane-died", "pane-focus-in", "pane-focus-out", "client-focus-in", "client-focus-out"] {
            assert_eq!(known(name), Known::Active, "{name}");
        }
    }

    #[test]
    fn the_alert_hooks_fire() {
        assert_eq!(known("alert-activity"), Known::Active);
        assert_eq!(known("alert-bell"), Known::Active);
        assert_eq!(known("alert-silence"), Known::Active);
        assert_eq!(known("pane-set-clipboard"), Known::Inactive);
    }

    #[test]
    fn a_window_that_goes_quiet_fires_alert_silence_once() {
        let mut quiet = snap();
        quiet.alerts.insert(10, Alerts { silence: Some(100), ..Alerts::default() });
        let fired = diff(&snap(), &quiet);
        assert_eq!(fired.iter().map(|f| f.name).collect::<Vec<_>>(), ["alert-silence"]);
        assert!(fired[0].vars.contains(&("hook_pane", "%100".to_string())));
        assert!(diff(&quiet, &quiet).is_empty());
    }

    #[test]
    fn a_window_that_gets_a_flag_fires_once_with_the_pane_that_caused_it() {
        let mut rang = snap();
        rang.alerts.insert(10, Alerts { activity: None, bell: Some(100), ..Alerts::default() });
        let fired = diff(&snap(), &rang);
        assert_eq!(fired.iter().map(|f| f.name).collect::<Vec<_>>(), ["alert-bell"]);
        assert!(fired[0].vars.contains(&("hook_pane", "%100".to_string())));
        assert!(fired[0].vars.contains(&("hook_window", "@10".to_string())));
        assert!(diff(&rang, &rang).is_empty(), "a flag that stays is not a new alert");
        assert!(diff(&rang, &snap()).is_empty(), "clearing a flag fires nothing");
        let mut both = rang.clone();
        both.alerts.insert(10, Alerts { activity: Some(100), bell: Some(100), ..Alerts::default() });
        assert_eq!(diff(&rang, &both).iter().map(|f| f.name).collect::<Vec<_>>(), ["alert-activity"], "only the new one");
    }

    #[test]
    fn short_command_names_give_one_hook_name() {
        assert_eq!(canonical("neww"), "new-window");
        assert_eq!(canonical("set"), "set-option");
        assert_eq!(canonical("kill-pane"), "kill-pane");
    }
}
