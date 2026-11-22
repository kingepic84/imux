//! `#(command)` in the status line formats (`status-left`, `status-right`, `window-status-*`).
//!
//! The bar never waits for a command. `status_with` asks the `SegmentView` for the output of the
//! last completed run and gets nothing (or the older output) when there is none; asking also puts
//! the command on the list of wanted ones. Every housekeeping turn `segments_tick` reads that list,
//! starts a helper thread for each command whose output is older than `status-interval` (at most
//! `MAX_RUNNING` at once, at most `status-commands-max` different ones), and `finish_segments`
//! stores what the threads send back through the channel of the worktree jobs' waker and asks for
//! a redraw. Only a bar that is drawn for a client asks (measuring or clicking does not), and a
//! command is refreshed only while it was asked for during the last two intervals, so nothing runs
//! for a bar nobody sees. A command nobody asks for any more is forgotten after a while.
//!
//! The command text is a format whose variables are checked like shell text (`format.rs`): a
//! window name, title or directory is refused unless written `#{q:name}`, and the bar then shows
//! what was refused instead of running anything. The command runs as `run-shell` does (`sh -c` in
//! a process group of its own, `IMUX_*` variables, the directory of the pane) except that its
//! standard error is thrown away and it is stopped after `status-command-timeout`. Only the first
//! line of its output is shown, without control characters, at most `MAX_TEXT` bytes, and every `#`
//! in it is a stand-in (`format::LITERAL_HASH`): output can never be a variable, a style or
//! another command.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use super::Server;
use super::shellcmd::{Outcome, run_with};
use crate::config::format;
use crate::log;

/// Commands running at once; the others wait for the next turn.
const MAX_RUNNING: usize = 4;
/// Longest output kept, in bytes.
const MAX_TEXT: usize = 200;
/// A command nobody asks for stays at least this long, so a bar that redraws rarely keeps it.
const KEEP_AT_LEAST: Duration = Duration::from_secs(60);

/// The options whose values may hold `#(command)`.
const FORMAT_OPTIONS: [&str; 5] = ["status-left", "status-right", "window-status-format", "window-status-current-format", "window-status-separator"];

/// A command and the session whose pane it is about.
type Want = (usize, String);

/// What `status_with` sees: the outputs found so far, the list to put wanted commands on, and the
/// variables of each session's current pane (only when a status option has a command in it).
#[derive(Clone, Default)]
pub(super) struct SegmentView {
    texts: HashMap<String, String>,
    pub(super) wanted: Rc<RefCell<Vec<Want>>>,
    vars: HashMap<usize, HashMap<&'static str, String>>,
    /// The variables of the active pane of each window, as (session id, window index), only when a
    /// status option names a pane variable.
    window_vars: HashMap<(usize, usize), HashMap<&'static str, String>>,
    /// The user options (`@name`) and the options the status options name, with whether the value
    /// may go into shell text unquoted (a user option no pane gave).
    user: HashMap<String, (String, bool)>,
    /// Ids of the sessions with a client that pressed the prefix key and has not yet pressed the
    /// next one (`#{client_prefix}`; the bar is one per session, so it shows for all its clients).
    prefix: HashSet<usize>,
}

impl SegmentView {
    /// Whether a client of the session waits for the key after the prefix.
    pub fn prefix(&self, session: usize) -> bool {
        self.prefix.contains(&session)
    }

    /// The output of the last run of `command` for a session, empty when there is none yet. The
    /// command is wanted from now on.
    pub fn text(&self, session: usize, command: &str) -> String {
        if command.trim().is_empty() {
            return String::new();
        }
        let mut wanted = self.wanted.borrow_mut();
        if !wanted.iter().any(|(s, c)| *s == session && c == command) {
            wanted.push((session, command.to_string()));
        }
        self.texts.get(command).cloned().unwrap_or_default()
    }

    /// A view with the given outputs and, for session ids, variables.
    #[cfg(test)]
    pub fn for_test(texts: &[(&str, &str)], vars: &[(usize, &'static str, &str)]) -> Self {
        let mut view = SegmentView { texts: texts.iter().map(|(c, t)| (c.to_string(), t.to_string())).collect(), ..SegmentView::default() };
        for (session, name, value) in vars {
            view.vars.entry(*session).or_default().insert(name, value.to_string());
        }
        view
    }

    /// A variable of the current pane of a session.
    pub fn var(&self, session: usize, name: &str) -> Option<String> {
        if name.starts_with('@') {
            return self.user.get(name).map(|(v, _)| v.clone());
        }
        self.vars.get(&session).and_then(|v| v.get(name).cloned()).or_else(|| self.user.get(name).map(|(v, _)| v.clone()))
    }

    /// A variable of the active pane of one window of a session (for the tab of that window).
    pub fn window_var(&self, session: usize, window: usize, name: &str) -> Option<String> {
        self.window_vars.get(&(session, window))?.get(name).cloned()
    }

    /// Whether a `#(command)` in the bar may take the variable unquoted (a user option no pane set).
    pub fn user_safe(&self, name: &str) -> bool {
        self.user.get(name).is_some_and(|(_, trusted)| *trusted)
    }
}

#[derive(Debug)]
struct Entry {
    text: String,
    fetched: Option<Instant>,
    running: bool,
    wanted_at: Instant,
    session: usize,
}

struct Done {
    command: String,
    result: Result<Outcome, String>,
}

pub(super) struct Segments {
    cache: HashMap<String, Entry>,
    wanted: Rc<RefCell<Vec<Want>>>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    running: usize,
    /// Commands whose trouble was logged already, so a failing one logs once.
    warned: HashSet<String>,
}

impl Default for Segments {
    fn default() -> Self {
        let (tx, rx) = channel();
        Segments { cache: HashMap::new(), wanted: Rc::default(), tx, rx, running: 0, warned: HashSet::new() }
    }
}

/// The first line of command output as the bar shows it: no control characters, none of the
/// stand-ins for `#`, at most `MAX_TEXT` bytes.
fn clean(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars().filter(|&c| !crate::sanitize::is_unsafe(c) && c != format::LITERAL_HASH) {
        if out.len() + c.len_utf8() > MAX_TEXT {
            break;
        }
        out.push(c);
    }
    out.trim().to_string()
}

impl Segments {
    /// Take the wanted commands in: a known one is marked as still wanted, a new one gets an entry
    /// unless `max` different commands are there already (the name of such a command is returned).
    fn take_wanted(&mut self, wanted: Vec<Want>, now: Instant, max: usize) -> Vec<String> {
        let mut over = Vec::new();
        for (session, command) in wanted {
            if let Some(e) = self.cache.get_mut(&command) {
                e.wanted_at = now;
                e.session = session;
            } else if self.cache.len() >= max {
                over.push(command);
            } else {
                self.cache.insert(command, Entry { text: String::new(), fetched: None, running: false, wanted_at: now, session });
            }
        }
        over
    }

    /// Forget the commands that nobody has asked for during `keep` and that do not run now.
    fn forget(&mut self, now: Instant, keep: Duration) {
        self.cache.retain(|_, e| e.running || now.saturating_duration_since(e.wanted_at) < keep);
    }

    /// The commands to start now, never-run ones first and then the one with the oldest output.
    /// A command is only refreshed while a bar has asked for it during the last `watched`: a bar
    /// that is drawn asks at least once every `status-interval`, so after that nobody looks.
    fn due(&self, now: Instant, interval: Duration, watched: Duration, room: usize) -> Vec<String> {
        let mut due: Vec<(&String, Option<Instant>)> = self
            .cache
            .iter()
            .filter(|(_, e)| !e.running && now.saturating_duration_since(e.wanted_at) <= watched)
            .filter(|(_, e)| e.fetched.is_none_or(|t| now.saturating_duration_since(t) >= interval))
            .map(|(k, e)| (k, e.fetched))
            .collect();
        due.sort_by_key(|(_, fetched)| *fetched);
        due.into_iter().take(room).map(|(k, _)| k.clone()).collect()
    }

    /// The result of a run. Returns whether what the bar shows changed.
    fn store(&mut self, command: &str, result: &Result<Outcome, String>, now: Instant, limit: Duration) -> bool {
        let Some(entry) = self.cache.get_mut(command) else { return false };
        entry.running = false;
        entry.fetched = Some(now);
        let text = match result {
            Ok(o) if o.timed_out => {
                if self.warned.insert(command.to_string()) {
                    log::warn(&format!("status command stopped after {} s: {command:?}", limit.as_secs()));
                }
                String::new()
            }
            Ok(o) => {
                self.warned.remove(command);
                clean(&o.line)
            }
            Err(e) => {
                if self.warned.insert(command.to_string()) {
                    log::error(&format!("status command {command:?}: {e}"));
                }
                String::new()
            }
        };
        let changed = entry.text != text;
        entry.text = text;
        changed
    }
}

impl Server {
    /// What a status bar needs to resolve its commands (see the top). Only a bar that is drawn for
    /// a client (`drawn`) puts its commands on the wanted list; one that is only measured or
    /// clicked on does not, so a bar nobody sees never makes anything run.
    pub(super) fn segment_view(&self, drawn: bool) -> SegmentView {
        let formats: Vec<&str> = FORMAT_OPTIONS.iter().map(|o| self.options.text(o)).collect();
        let uses_commands = formats.iter().any(|t| t.contains("#("));
        let uses_panes = uses_commands || formats.iter().any(|t| t.contains("pane_") || t.contains("#T") || t.contains("#D"));
        let vars = if uses_panes {
            self.sessions.iter().enumerate().map(|(si, s)| (s.id, self.place_vars(self.current_place(si)))).collect()
        } else {
            HashMap::new()
        };
        let window_vars = if formats.iter().any(|t| t.contains("pane_") || t.contains("#T") || t.contains("#D")) {
            let mut all = HashMap::new();
            for (si, s) in self.sessions.iter().enumerate() {
                for (wi, w) in s.windows.iter().enumerate() {
                    all.insert((s.id, wi), self.place_vars(super::remote::Place { si, wi, pane: w.active }));
                }
            }
            all
        } else {
            HashMap::new()
        };
        let texts = self.segments.cache.iter().map(|(k, e)| (k.clone(), e.text.clone())).collect();
        let wanted = if drawn { Rc::clone(&self.segments.wanted) } else { Rc::default() };
        let mut user = HashMap::new();
        for text in &formats {
            for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '-'))).filter(|w| w.len() > 1) {
                if user.contains_key(word) {
                    continue;
                }
                if let Some(value) = self.options.user(word) {
                    user.insert(word.to_string(), (value.to_string(), self.options.user_trusted(word)));
                } else if let Some(value) = self.options.value_text(word) {
                    user.insert(word.to_string(), (value.to_string(), false));
                }
            }
        }
        let prefix = if self.status_mentions("client_prefix") { self.sessions_in_prefix() } else { HashSet::new() };
        SegmentView { texts, wanted, vars, window_vars, user, prefix }
    }

    /// Whether a status format names `word`.
    pub(super) fn status_mentions(&self, word: &str) -> bool {
        FORMAT_OPTIONS.iter().any(|o| self.options.text(o).contains(word))
    }

    /// The ids of the sessions that have an attached client between the prefix key and its key.
    pub(super) fn sessions_in_prefix(&self) -> HashSet<usize> {
        self.clients.values().filter(|c| c.size.is_some() && !c.closing && c.keys.prefix).map(|c| c.session).collect()
    }

    /// Every housekeeping turn: take in the commands the bar asked for and start the ones that are
    /// due.
    pub(super) fn segments_tick(&mut self) {
        let now = Instant::now();
        let interval = Duration::from_secs(self.options.num("status-interval"));
        let max = self.options.num("status-commands-max") as usize;
        let wanted = std::mem::take(&mut *self.segments.wanted.borrow_mut());
        for command in self.segments.take_wanted(wanted, now, max) {
            if self.segments.warned.insert(format!("max:{command}")) {
                log::warn(&format!("status-commands-max is {max}: not running {command:?}"));
            }
        }
        self.segments.forget(now, (interval * 3).max(KEEP_AT_LEAST));
        let room = MAX_RUNNING.saturating_sub(self.segments.running);
        for command in self.segments.due(now, interval, interval * 2 + Duration::from_secs(1), room) {
            self.start_segment(command, now);
        }
    }

    fn start_segment(&mut self, command: String, now: Instant) {
        let Some(session) = self.segments.cache.get(&command).map(|e| e.session) else { return };
        let place = self.session_index(session).map(|si| self.current_place(si));
        let vars = place.map_or_else(|| self.hooks.vars.clone(), |p| self.place_vars(p));
        let job = self.job_with(place, &vars, command.clone());
        let limit = Duration::from_secs(self.options.num("status-command-timeout"));
        let (tx, waker) = (self.segments.tx.clone(), self.jobs.waker());
        let key = command.clone();
        let spawned = std::thread::Builder::new().name("segment".into()).spawn(move || {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| run_with(&job, limit, false)))
                .unwrap_or_else(|p| Err(format!("panicked: {}", log::panic_text(p.as_ref()))));
            if tx.send(Done { command: key, result }).is_err() {
                log::warn("a status command finished after the server stopped listening");
            }
            if let Err(e) = waker.wake() {
                log::error(&format!("waking the server: {e}"));
            }
        });
        match spawned {
            Ok(_) => {
                self.segments.running += 1;
                if let Some(e) = self.segments.cache.get_mut(&command) {
                    e.running = true;
                }
            }
            Err(e) => {
                log::error(&format!("status command: cannot start a thread: {e}"));
                if let Some(e) = self.segments.cache.get_mut(&command) {
                    e.fetched = Some(now);
                }
            }
        }
    }

    /// Store what the helper threads have sent back and redraw if the bar changes.
    pub(super) fn finish_segments(&mut self) {
        let limit = Duration::from_secs(self.options.num("status-command-timeout"));
        while let Ok(done) = self.segments.rx.try_recv() {
            self.segments.running = self.segments.running.saturating_sub(1);
            if self.segments.store(&done.command, &done.result, Instant::now(), limit) {
                self.dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(line: &str) -> Result<Outcome, String> {
        Ok(Outcome { pid: 1, status: Some(0), timed_out: false, line: line.to_string() })
    }

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    const FIVE: Duration = Duration::from_secs(5);
    const WATCHED: Duration = Duration::from_secs(3600);

    #[test]
    fn output_is_one_clean_short_line() {
        assert_eq!(clean("main"), "main");
        assert_eq!(clean(" a\u{1b}[31mb\u{7}c "), "a[31mbc");
        assert_eq!(clean(&format!("a{}b", format::LITERAL_HASH)), "ab", "a stand-in can not come from a command");
        assert_eq!(clean(&"x".repeat(500)).len(), MAX_TEXT);
        assert_eq!(clean(&"é".repeat(500)).len(), MAX_TEXT, "cut at a character, not in the middle of one");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn asking_for_a_command_puts_it_on_the_list_once() {
        let view = SegmentView::default();
        assert_eq!(view.text(1, "echo hi"), "");
        assert_eq!(view.text(1, "echo hi"), "");
        assert_eq!(view.text(2, "echo hi"), "");
        assert_eq!(view.text(1, "  "), "");
        assert_eq!(*view.wanted.borrow(), vec![(1, "echo hi".to_string()), (2, "echo hi".to_string())]);
    }

    #[test]
    fn a_new_command_gets_an_entry_and_runs_at_once() {
        let (base, mut s) = (Instant::now(), Segments::default());
        assert!(s.take_wanted(vec![(1, "a".into()), (1, "b".into())], base, 8).is_empty());
        let mut due = s.due(base, FIVE, WATCHED, 4);
        due.sort();
        assert_eq!(due, ["a", "b"]);
    }

    #[test]
    fn output_is_fresh_for_the_interval_and_then_due_again() {
        let (base, mut s) = (Instant::now(), Segments::default());
        s.take_wanted(vec![(1, "a".into())], base, 8);
        s.cache.get_mut("a").unwrap().running = true;
        assert!(s.due(base, FIVE, WATCHED, 4).is_empty(), "not while it runs");
        assert!(s.store("a", &outcome("one"), at(base, 1), Duration::from_secs(2)));
        assert!(s.due(at(base, 3), FIVE, WATCHED, 4).is_empty(), "fresh");
        assert!(s.due(at(base, 5), FIVE, WATCHED, 4).is_empty(), "the interval counts from the end of the run");
        assert_eq!(s.due(at(base, 6), FIVE, WATCHED, 4), ["a"]);
    }

    #[test]
    fn a_command_nobody_has_asked_for_lately_is_not_refreshed() {
        let (base, mut s) = (Instant::now(), Segments::default());
        let watched = Duration::from_secs(11);
        s.take_wanted(vec![(1, "a".into())], base, 8);
        s.store("a", &outcome("one"), base, Duration::from_secs(2));
        assert_eq!(s.due(at(base, 10), FIVE, watched, 4), ["a"]);
        assert_eq!(s.due(at(base, 11), FIVE, watched, 4), ["a"]);
        assert!(s.due(at(base, 12), FIVE, watched, 4).is_empty(), "the bar stopped asking");
        s.take_wanted(vec![(1, "a".into())], at(base, 12), 8);
        assert_eq!(s.due(at(base, 12), FIVE, watched, 4), ["a"], "and when it asks again it goes on");
    }

    #[test]
    fn the_stored_text_is_kept_while_the_next_run_goes_on() {
        let (base, mut s) = (Instant::now(), Segments::default());
        s.take_wanted(vec![(1, "a".into())], base, 8);
        s.store("a", &outcome("one"), base, Duration::from_secs(2));
        s.cache.get_mut("a").unwrap().running = true;
        assert_eq!(s.cache["a"].text, "one");
        assert!(!s.store("a", &outcome("one"), at(base, 6), Duration::from_secs(2)), "the same text is no change");
        assert!(s.store("a", &outcome("two"), at(base, 12), Duration::from_secs(2)));
    }

    #[test]
    fn a_command_that_ran_over_or_failed_shows_nothing() {
        let (base, mut s) = (Instant::now(), Segments::default());
        s.take_wanted(vec![(1, "a".into())], base, 8);
        s.store("a", &outcome("old"), base, Duration::from_secs(2));
        let slow = Ok(Outcome { pid: 1, status: None, timed_out: true, line: "partial".into() });
        assert!(s.store("a", &slow, at(base, 6), Duration::from_secs(2)));
        assert_eq!(s.cache["a"].text, "", "an old value would look right and not be");
        s.store("a", &outcome("back"), at(base, 12), Duration::from_secs(2));
        assert!(s.store("a", &Err("no sh".into()), at(base, 18), Duration::from_secs(2)));
        assert_eq!(s.cache["a"].text, "");
    }

    #[test]
    fn a_result_for_a_forgotten_command_is_dropped() {
        let (base, mut s) = (Instant::now(), Segments::default());
        assert!(!s.store("gone", &outcome("x"), base, Duration::from_secs(2)));
        assert!(s.cache.is_empty());
    }

    #[test]
    fn only_so_many_different_commands_are_kept() {
        let (base, mut s) = (Instant::now(), Segments::default());
        let wanted = vec![(1, "a".into()), (1, "b".into()), (1, "c".into())];
        assert_eq!(s.take_wanted(wanted, base, 2), ["c"]);
        assert_eq!(s.cache.len(), 2);
        assert!(s.take_wanted(vec![(1, "a".into())], base, 2).is_empty(), "one that is there is not over the limit");
    }

    #[test]
    fn only_so_many_run_at_once_the_never_run_ones_and_the_oldest_first() {
        let (base, mut s) = (Instant::now(), Segments::default());
        s.take_wanted((0..5).map(|i| (1, format!("c{i}"))).collect(), base, 8);
        s.store("c0", &outcome("x"), at(base, 1), Duration::from_secs(2));
        s.store("c1", &outcome("x"), at(base, 2), Duration::from_secs(2));
        let due = s.due(at(base, 20), FIVE, WATCHED, 4);
        assert_eq!(due.len(), 4);
        let mut never: Vec<_> = due[..3].to_vec();
        never.sort();
        assert_eq!(never, ["c2", "c3", "c4"], "the ones that never ran come first");
        assert_eq!(due[3], "c0", "then the oldest output");
        assert_eq!(s.due(at(base, 20), FIVE, WATCHED, 0).len(), 0);
    }

    #[test]
    fn a_command_nobody_asks_for_is_forgotten_but_not_while_it_runs() {
        let (base, mut s) = (Instant::now(), Segments::default());
        s.take_wanted(vec![(1, "a".into()), (1, "b".into())], base, 8);
        s.cache.get_mut("b").unwrap().running = true;
        s.forget(at(base, 59), KEEP_AT_LEAST);
        assert_eq!(s.cache.len(), 2);
        s.take_wanted(vec![(1, "a".into())], at(base, 60), 8);
        s.forget(at(base, 119), KEEP_AT_LEAST);
        assert_eq!(s.cache.len(), 2, "asking again starts the time over; b still runs");
        s.forget(at(base, 121), KEEP_AT_LEAST);
        assert_eq!(s.cache.keys().collect::<Vec<_>>(), ["b"]);
    }
}
