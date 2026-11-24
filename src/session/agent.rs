//! What a pane's program says it is doing (OSC 7501), kept per pane. The rules come from the
//! protocol: a `working` or `blocked` record ends with the program or at the next shell prompt,
//! `done` and `error` stay until someone has looked at them, there is no heartbeat.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::term::status::{Kind, Report, State};

/// `source` of records that came from the program's own OSC 7501 reports.
pub const OSC_SOURCE: &str = "osc";

/// Limits on a resume command a program reports, to be typed into a shell later.
const MAX_RESUME_ARGS: usize = 64;
const MAX_RESUME_BYTES: usize = 8192;

/// A `working` this young is shown as what came before it: Claude Code sends `working` for a
/// moment between a denied permission or an Esc and its final `idle`.
pub const FLICKER: Duration = Duration::from_millis(150);

#[derive(Clone, Debug)]
pub struct AgentRecord {
    pub state: State,
    /// The state before this one, for hiding a flicker.
    pub before: Option<State>,
    pub app: Option<String>,
    pub kind: Option<Kind>,
    pub progress: Option<u8>,
    pub msg: Option<String>,
    pub title: Option<String>,
    /// When `state` last changed.
    pub since: Instant,
    /// Clients that have looked at the pane since `state` last changed.
    pub seen_by: HashSet<usize>,
    /// Who made the last change: `OSC_SOURCE`, or the `--source` of a socket report.
    pub source: String,
    /// Highest `--seq` accepted from each socket source; older ones are stale.
    pub seqs: HashMap<String, u64>,
    /// The command that brings the program back after a restart, as reported (and validated).
    pub resume: Option<Vec<String>>,
    /// The name given with `imux agent name`: unique among the panes that have a record, and gone
    /// with the record (or when another program takes the pane over).
    pub name: Option<String>,
    /// The name of the program's own session (Claude Code's `/rename`), as its hook gave it with
    /// `imux agent session`: free text, cleaned, data. It follows the same program, like `name`.
    pub session: Option<String>,
    /// The session name before the last change. Setting the name that is already there changes
    /// nothing, so this is never the same name as `session`.
    pub previous_session: Option<String>,
}

/// The most characters of a session name.
pub const MAX_SESSION: usize = 128;

/// Give the record the session name `text` (empty takes it off). The name already there again
/// changes nothing and returns false, so a hook that says the name on every prompt does not push the
/// same name into `previous_session`. A different name moves the one before it there.
pub fn set_session(record: &mut AgentRecord, text: &str) -> bool {
    let cleaned: String = crate::sanitize::clean(text).chars().take(MAX_SESSION).collect();
    let new = Some(cleaned.trim().to_string()).filter(|s| !s.is_empty());
    if record.session == new {
        return false;
    }
    if record.session.is_some() {
        record.previous_session = record.session.take();
    }
    record.session = new;
    true
}

/// The most characters of an agent's name.
pub const MAX_NAME: usize = 32;

/// Whether `name` may name an agent: a lowercase letter, then lowercase letters, digits, `_` and
/// `-`, at most `MAX_NAME` in all. No `:` or `.` or `%`, so it can never be read as a target.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= MAX_NAME
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

impl AgentRecord {
    /// The state to show at `now`.
    pub fn shown(&self, now: Instant) -> State {
        match (self.state, self.before) {
            (State::Working, Some(before)) if now.duration_since(self.since) < FLICKER => before,
            (state, _) => state,
        }
    }

    /// `done` and `error` count until `client` has looked at the pane.
    pub fn unseen_by(&self, client: usize) -> bool {
        matches!(self.state, State::Done | State::Error) && !self.seen_by.contains(&client)
    }
}

/// How much a state asks for attention in a rollup: higher wins.
pub fn rank(state: State) -> u8 {
    match state {
        State::Blocked => 6,
        State::Error => 5,
        State::Working => 4,
        State::Done => 3,
        State::Idle => 1,
        State::Clear => 0,
    }
}

/// What `client` should see for a record: a finished (`done`, `error`) pane that the client has
/// already looked at is just idle. With no client (sizing the status bar), nothing is hidden.
pub fn shown_to(record: &AgentRecord, client: Option<usize>, now: Instant) -> State {
    let state = record.shown(now);
    match client {
        Some(c) if matches!(state, State::Done | State::Error) && !record.unseen_by(c) => State::Idle,
        _ => state,
    }
}

/// The most urgent state among `states`, and how many are in it.
pub fn worst(states: impl IntoIterator<Item = State>) -> Option<(State, usize)> {
    let mut best: Option<(State, usize)> = None;
    for s in states {
        best = match best {
            Some((b, n)) if rank(s) == rank(b) => Some((b, n + 1)),
            Some((b, n)) if rank(s) < rank(b) => Some((b, n)),
            _ => Some((s, 1)),
        };
    }
    best
}

/// Apply a report to a pane's slot. Only the root record is kept; reports for other ids are
/// ignored. Returns whether anything changed.
pub fn apply(slot: &mut Option<AgentRecord>, report: Report, now: Instant) -> bool {
    put(slot, report, OSC_SOURCE, now, None)
}

fn put(slot: &mut Option<AgentRecord>, report: Report, source: &str, now: Instant, resume: Option<Vec<String>>) -> bool {
    if report.id.is_some() {
        return false;
    }
    if report.state == State::Clear {
        return slot.take().is_some();
    }
    let kept_name = slot.as_ref().filter(|old| old.app == report.app).and_then(|old| old.name.clone());
    let (kept_session, kept_previous) =
        slot.as_ref().filter(|old| old.app == report.app).map_or((None, None), |old| (old.session.clone(), old.previous_session.clone()));
    let (before, since, seen_by, seqs, kept) = match slot.take() {
        Some(old) if old.state == report.state => (old.before, old.since, old.seen_by, old.seqs, old.resume),
        Some(old) => (Some(old.state), now, HashSet::new(), old.seqs, old.resume),
        None => (None, now, HashSet::new(), HashMap::new(), None),
    };
    *slot = Some(AgentRecord {
        state: report.state,
        before,
        app: report.app,
        kind: report.kind,
        progress: report.progress,
        msg: report.msg,
        title: report.title,
        since,
        seen_by,
        source: source.to_string(),
        seqs,
        resume: resume.or(kept),
        name: kept_name,
        session: kept_session,
        previous_session: kept_previous,
    });
    true
}

/// What a report over the socket did.
#[derive(Debug, PartialEq, Eq)]
pub enum Reported {
    Applied,
    /// Its `seq` was not higher than the last one from that source; ignored.
    Stale,
}

/// Apply a report from a socket `source` with sequence number `seq`, optionally with the command
/// that resumes the program. Sequence numbers only have to rise per source.
pub fn apply_reported(
    slot: &mut Option<AgentRecord>,
    report: Report,
    source: &str,
    seq: u64,
    resume: Option<Vec<String>>,
    now: Instant,
) -> Reported {
    if slot.as_ref().and_then(|r| r.seqs.get(source)).is_some_and(|&last| seq <= last) {
        return Reported::Stale;
    }
    let mut seqs = slot.as_ref().map(|r| r.seqs.clone()).unwrap_or_default();
    seqs.insert(source.to_string(), seq);
    put(slot, report, source, now, resume);
    if let Some(r) = slot.as_mut() {
        r.seqs = seqs;
    }
    Reported::Applied
}

/// A program says it is gone: clear the record if `source` is the one that wrote it.
pub fn release(slot: &mut Option<AgentRecord>, source: &str) -> bool {
    if slot.as_ref().is_some_and(|r| r.source == source) {
        *slot = None;
        return true;
    }
    false
}

/// Forget the resume command (the program it belonged to is gone). Returns whether there was one.
pub fn drop_resume(slot: &mut Option<AgentRecord>) -> bool {
    slot.as_mut().is_some_and(|r| r.resume.take().is_some())
}

/// Whether `argv` may be reported as a resume command: a bare command name from `allowed`, at most
/// 64 arguments and 8 KiB, and nothing in an argument that could break out of quoting.
pub fn validate_resume(argv: &[String], allowed: &[String]) -> Result<(), String> {
    let Some(first) = argv.first() else { return Err("empty command".to_string()) };
    let bare = !first.is_empty()
        && !first.starts_with('-')
        && first.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b));
    if !bare {
        return Err(format!("{first:?} is not a plain command name"));
    }
    if !allowed.iter().any(|a| a == first) {
        return Err(format!("{first} is not in restore-commands"));
    }
    if argv.len() > MAX_RESUME_ARGS {
        return Err(format!("more than {MAX_RESUME_ARGS} arguments"));
    }
    if argv.iter().map(String::len).sum::<usize>() > MAX_RESUME_BYTES {
        return Err(format!("more than {MAX_RESUME_BYTES} bytes"));
    }
    if let Some(bad) = argv.iter().find(|a| a.contains('\'') || a.chars().any(char::is_control)) {
        return Err(format!("{bad:?} has an apostrophe or a control character"));
    }
    Ok(())
}

/// The shell is showing a prompt: whatever was running is over. `done` and `error` stay.
pub fn prompt_started(slot: &mut Option<AgentRecord>) -> bool {
    if slot.as_ref().is_some_and(|r| matches!(r.state, State::Working | State::Blocked | State::Idle)) {
        *slot = None;
        return true;
    }
    false
}

/// The pane's foreground process is back at the shell: drop what only lives while it runs.
pub fn program_ended(slot: &mut Option<AgentRecord>) -> bool {
    if slot.as_ref().is_some_and(|r| matches!(r.state, State::Working | State::Blocked)) {
        *slot = None;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: State) -> Report {
        Report { state, kind: None, progress: None, app: Some("claude-code".into()), title: None, msg: None, id: None }
    }

    #[test]
    fn a_name_is_a_lowercase_word_that_cannot_be_read_as_a_target() {
        for good in ["a", "reviewer", "w2", "a_b-c", &"a".repeat(MAX_NAME)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "A", "1a", "-a", "_a", "a b", "a:b", "a.b", "%1", "é", "a/b", &"a".repeat(MAX_NAME + 1)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_name_follows_the_same_program_and_goes_when_another_takes_the_pane_or_the_record_goes() {
        let mut slot = None;
        let t = Instant::now();
        apply(&mut slot, report(State::Idle), t);
        slot.as_mut().unwrap().name = Some("rev".to_string());
        apply(&mut slot, report(State::Working), t);
        assert_eq!(slot.as_ref().unwrap().name.as_deref(), Some("rev"), "a new state keeps the name");
        let other = Report { app: Some("codex".into()), ..report(State::Working) };
        apply(&mut slot, other, t);
        assert_eq!(slot.as_ref().unwrap().name, None, "another program took the pane");
        slot.as_mut().unwrap().name = Some("x".to_string());
        assert!(release(&mut slot, OSC_SOURCE));
        apply(&mut slot, report(State::Idle), t);
        assert_eq!(slot.as_ref().unwrap().name, None, "a new record starts without one");
    }

    #[test]
    fn a_report_fills_the_slot_and_a_clear_empties_it() {
        let mut slot = None;
        let t = Instant::now();
        assert!(apply(&mut slot, report(State::Idle), t));
        assert_eq!(slot.as_ref().unwrap().state, State::Idle);
        assert!(apply(&mut slot, report(State::Clear), t));
        assert!(slot.is_none());
        assert!(!apply(&mut slot, report(State::Clear), t));
    }

    #[test]
    fn only_the_root_record_is_kept() {
        let mut slot = None;
        let mut r = report(State::Working);
        r.id = Some("build/test".into());
        assert!(!apply(&mut slot, r, Instant::now()));
        assert!(slot.is_none());
    }

    #[test]
    fn the_time_of_the_state_only_moves_when_the_state_changes() {
        let mut slot = None;
        let t0 = Instant::now();
        apply(&mut slot, report(State::Working), t0);
        let t1 = t0 + Duration::from_secs(1);
        let mut again = report(State::Working);
        again.progress = Some(10);
        apply(&mut slot, again, t1);
        let r = slot.as_ref().unwrap();
        assert_eq!((r.since, r.progress), (t0, Some(10)));
        apply(&mut slot, report(State::Done), t1);
        let r = slot.as_ref().unwrap();
        assert_eq!((r.since, r.before), (t1, Some(State::Working)));
    }

    #[test]
    fn a_working_flicker_after_blocked_shows_as_blocked_then_idle() {
        let mut slot = None;
        let t0 = Instant::now();
        apply(&mut slot, report(State::Blocked), t0);
        let t1 = t0 + Duration::from_secs(2);
        apply(&mut slot, report(State::Working), t1);
        let r = slot.as_ref().unwrap();
        assert_eq!(r.shown(t1 + Duration::from_millis(40)), State::Blocked);
        assert_eq!(r.shown(t1 + Duration::from_millis(400)), State::Working);
        apply(&mut slot, report(State::Idle), t1 + Duration::from_millis(100));
        assert_eq!(slot.as_ref().unwrap().shown(t1 + Duration::from_millis(100)), State::Idle);
    }

    #[test]
    fn done_is_unseen_until_a_client_looks_and_a_new_state_resets_that() {
        let mut slot = None;
        let t = Instant::now();
        apply(&mut slot, report(State::Done), t);
        assert!(slot.as_ref().unwrap().unseen_by(1));
        slot.as_mut().unwrap().seen_by.insert(1);
        assert!(!slot.as_ref().unwrap().unseen_by(1));
        assert!(slot.as_ref().unwrap().unseen_by(2));
        apply(&mut slot, report(State::Working), t);
        apply(&mut slot, report(State::Done), t);
        assert!(slot.as_ref().unwrap().unseen_by(1));
        apply(&mut slot, report(State::Idle), t);
        assert!(!slot.as_ref().unwrap().unseen_by(2));
    }

    #[test]
    fn the_most_urgent_state_wins_a_rollup_and_is_counted() {
        use State::*;
        assert_eq!(worst([]), None);
        assert_eq!(worst([Idle, Idle]), Some((Idle, 2)));
        assert_eq!(worst([Idle, Done, Working]), Some((Working, 1)));
        assert_eq!(worst([Working, Blocked, Done, Blocked]), Some((Blocked, 2)));
        assert_eq!(worst([Working, Error, Done]), Some((Error, 1)));
        assert_eq!(worst([Done, Working, Working]), Some((Working, 2)));
    }

    #[test]
    fn a_finished_pane_a_client_has_looked_at_counts_as_idle_for_that_client() {
        let mut slot = None;
        let t = Instant::now();
        apply(&mut slot, report(State::Done), t);
        let r = slot.as_mut().unwrap();
        assert_eq!(shown_to(r, Some(1), t), State::Done);
        assert_eq!(shown_to(r, None, t), State::Done);
        r.seen_by.insert(1);
        assert_eq!(shown_to(r, Some(1), t), State::Idle);
        assert_eq!(shown_to(r, Some(2), t), State::Done);
        assert_eq!(shown_to(r, None, t), State::Done);
    }

    #[test]
    fn a_shell_prompt_ends_running_records_but_not_finished_ones() {
        for (state, gone) in [
            (State::Working, true),
            (State::Blocked, true),
            (State::Idle, true),
            (State::Done, false),
            (State::Error, false),
        ] {
            let mut slot = None;
            apply(&mut slot, report(state), Instant::now());
            assert_eq!(prompt_started(&mut slot), gone, "{state:?}");
            assert_eq!(slot.is_none(), gone, "{state:?}");
        }
    }

    #[test]
    fn the_program_ending_drops_working_and_blocked_only() {
        for (state, gone) in [(State::Working, true), (State::Blocked, true), (State::Idle, false), (State::Done, false)] {
            let mut slot = None;
            apply(&mut slot, report(state), Instant::now());
            assert_eq!(program_ended(&mut slot), gone, "{state:?}");
        }
    }

    #[test]
    fn socket_reports_need_a_higher_seq_per_source() {
        let mut slot = None;
        let t = Instant::now();
        assert_eq!(apply_reported(&mut slot, report(State::Working), "hook", 5, None, t), Reported::Applied);
        assert_eq!(apply_reported(&mut slot, report(State::Idle), "hook", 5, None, t), Reported::Stale);
        assert_eq!(apply_reported(&mut slot, report(State::Idle), "hook", 4, None, t), Reported::Stale);
        assert_eq!(slot.as_ref().unwrap().state, State::Working);
        assert_eq!(apply_reported(&mut slot, report(State::Idle), "other", 1, None, t), Reported::Applied);
        assert_eq!(slot.as_ref().unwrap().source, "other");
        assert_eq!(apply_reported(&mut slot, report(State::Done), "hook", 6, None, t), Reported::Applied);
        assert_eq!(slot.as_ref().unwrap().seqs.len(), 2);
    }

    #[test]
    fn the_resume_command_and_the_sequence_numbers_survive_an_osc_report() {
        let mut slot = None;
        let t = Instant::now();
        let cmd = vec!["claude".to_string(), "--resume".to_string(), "abc".to_string()];
        apply_reported(&mut slot, report(State::Idle), "hook", 3, Some(cmd.clone()), t);
        apply(&mut slot, report(State::Working), t);
        let r = slot.as_ref().unwrap();
        assert_eq!(r.source, OSC_SOURCE);
        assert_eq!(r.resume.as_ref(), Some(&cmd));
        assert_eq!(r.seqs.get("hook"), Some(&3));
        assert!(drop_resume(&mut slot));
        assert!(!drop_resume(&mut slot));
    }

    #[test]
    fn only_the_source_that_wrote_a_record_can_release_it() {
        let mut slot = None;
        let t = Instant::now();
        apply_reported(&mut slot, report(State::Working), "hook", 1, None, t);
        assert!(!release(&mut slot, "intruder"));
        assert!(slot.is_some());
        assert!(release(&mut slot, "hook"));
        assert!(slot.is_none());
        assert!(!release(&mut slot, "hook"));
    }

    #[test]
    fn a_resume_command_must_be_plain_listed_and_small() {
        let allowed = vec!["claude".to_string(), "vim".to_string()];
        let v = |parts: &[&str]| validate_resume(&parts.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &allowed);
        assert!(v(&["claude", "--resume", "abc-123"]).is_ok());
        assert!(v(&["vim"]).is_ok());
        for bad in [
            &[][..],
            &["rm", "-rf", "x"],
            &["/usr/bin/claude"],
            &["-claude"],
            &["claude", "it's"],
            &["claude", "a\nb"],
            &["claude", "a\u{1b}[31m"],
            &["claude;ls"],
            &[""],
        ] {
            assert!(v(bad).is_err(), "{bad:?}");
        }
        let many: Vec<String> = std::iter::once("claude".to_string()).chain((0..64).map(|i| i.to_string())).collect();
        assert!(validate_resume(&many, &allowed).is_err());
        let big = vec!["claude".to_string(), "x".repeat(9000)];
        assert!(validate_resume(&big, &allowed).is_err());
    }

    fn with_session_slot() -> Option<AgentRecord> {
        let mut slot = None;
        apply(&mut slot, Report { state: State::Idle, kind: None, progress: None, app: Some("claude-code".into()), title: None, msg: None, id: None }, Instant::now());
        slot
    }

    #[test]
    fn the_same_session_name_again_changes_nothing_and_a_new_one_keeps_the_one_before() {
        let mut slot = with_session_slot();
        let r = slot.as_mut().unwrap();
        assert!(set_session(r, "Homelab and imux"), "the first name is a change");
        assert_eq!((r.session.as_deref(), r.previous_session.as_deref()), (Some("Homelab and imux"), None));
        // a hook that says it on every prompt
        for _ in 0..3 {
            assert!(!set_session(r, "Homelab and imux"), "the same name is no change");
        }
        assert_eq!((r.session.as_deref(), r.previous_session.as_deref()), (Some("Homelab and imux"), None), "nothing was pushed");
        assert!(set_session(r, "Second"));
        assert_eq!((r.session.as_deref(), r.previous_session.as_deref()), (Some("Second"), Some("Homelab and imux")));
        assert!(!set_session(r, "Second"));
        assert_eq!(r.previous_session.as_deref(), Some("Homelab and imux"), "still the name before");
        assert!(set_session(r, "Homelab and imux"), "going back is a change");
        assert_eq!((r.session.as_deref(), r.previous_session.as_deref()), (Some("Homelab and imux"), Some("Second")));
        assert!(set_session(r, ""), "an empty name takes it off");
        assert_eq!((r.session.as_deref(), r.previous_session.as_deref()), (None, Some("Homelab and imux")));
        assert!(!set_session(r, "  "), "and nothing again is no change");
    }

    #[test]
    fn a_session_name_is_cleaned_and_cut_and_follows_the_same_program() {
        let mut slot = with_session_slot();
        let r = slot.as_mut().unwrap();
        set_session(r, "  a\u{1b}[31mb\u{202e}c\u{7}  ");
        assert_eq!(r.session.as_deref(), Some("a[31mbc"), "controls and bidi marks are dropped, the ends trimmed");
        set_session(r, &"x".repeat(500));
        assert_eq!(r.session.as_ref().map(|s| s.chars().count()), Some(MAX_SESSION));
        set_session(r, "kept");
        let (before, previous) = (r.session.clone(), r.previous_session.clone());
        // a new state from the same program keeps both
        apply(&mut slot, Report { state: State::Working, kind: None, progress: None, app: Some("claude-code".into()), title: None, msg: None, id: None }, Instant::now());
        let r = slot.as_ref().unwrap();
        assert_eq!((r.session.clone(), r.previous_session.clone()), (before, previous));
        // another program taking the pane starts without them
        apply(&mut slot, Report { state: State::Working, kind: None, progress: None, app: Some("other".into()), title: None, msg: None, id: None }, Instant::now());
        let r = slot.as_ref().unwrap();
        assert_eq!((r.session.as_deref(), r.previous_session.as_deref()), (None, None));
    }
}
