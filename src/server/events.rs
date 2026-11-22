//! The event stream: `imux events` keeps a connection open and gets one line of JSON for each
//! event that the hooks see (`hooks.rs`: the changes found by comparing snapshots, the `after-`
//! events of commands, `command-error`, `agent-state-changed`, `worktree-created`...). Anything
//! that listens uses this instead of polling `agent list`.
//!
//! A line is `{"event":"window-renamed","seq":12,"time":1731160000000,"session":"$1",...}`: the
//! variables of the event without their `hook_` prefix, as strings. `seq` counts up over the life
//! of the server and `time` is milliseconds since 1970. The first line of a subscription is
//! `{"event":"subscribed","api":1,"seq":N}`.
//!
//! Events only say that something changed; they are not a log. A subscriber that does not read
//! gets at most `BACKLOG` bytes queued (about 500 events); then it is sent
//! `{"event":"events_lost"}` and the connection is closed, and it has to read the state again
//! (`agent list`, `list-windows`...). A subscriber can name the types it wants; with none named it
//! gets all but the `after-<command>` ones, which have to be asked for by name. Values are cut and
//! have no control characters (the same as the hook variables), but they can still be chosen by a
//! program in a pane: a reader must treat them as text.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use super::Server;
use super::hooks::{self, Vars};
use crate::log;
use crate::proto::{self, API_VERSION, ServerMsg};

/// Bytes a subscriber may have queued before it is told it lost events.
pub(super) const BACKLOG: usize = 64 * 1024;
/// Event types one subscription may name, and the longest name.
const MAX_TYPES: usize = 32;
const MAX_TYPE: usize = 64;

struct Sub {
    /// `None`: everything but `after-` events.
    types: Option<HashSet<String>>,
}

impl Sub {
    fn wants(&self, name: &str) -> bool {
        match &self.types {
            Some(types) => types.contains(name),
            None => !name.starts_with("after-"),
        }
    }
}

/// Events kept for `--since`.
const RING: usize = 512;
/// The pseudo type that asks for a replay: `@since=SEQ` among the names of a subscription.
pub const SINCE: &str = "@since=";

#[derive(Default)]
pub(super) struct Stream {
    subs: HashMap<usize, Sub>,
    seq: u64,
    /// Once something asked for a sequence number (a subscription, `api-seq`) every event is
    /// counted and kept in `ring`, with or without a subscriber, so a caller can note `seq`,
    /// act, and ask for what came after. Until the first ask nothing is counted or found.
    armed: bool,
    /// (seq, name, line) of the last `RING` events, consecutive in `seq`.
    ring: std::collections::VecDeque<(u64, String, String)>,
}

impl Stream {
    pub(super) fn is_empty(&self) -> bool {
        !self.armed && self.subs.is_empty()
    }

    /// Whether anybody wants the events called `name`: a subscriber, or the ring (every event
    /// but the `after-` ones, which only a subscriber's name brings in).
    pub(super) fn wants(&self, name: &str) -> bool {
        (self.armed && !name.starts_with("after-")) || self.subs.values().any(|s| s.wants(name))
    }

    /// The sequence number of the last event, and from now on every event is kept.
    pub(super) fn arm(&mut self) -> u64 {
        self.armed = true;
        self.seq
    }

    /// What `--since seq` can say: the kept events after `seq` that `wants` takes, or `None` when
    /// some of them are gone (or `seq` is from another server's life).
    fn replay(&self, since: u64, wants: impl Fn(&str) -> bool) -> Option<Vec<&str>> {
        let oldest = self.ring.front().map_or(self.seq + 1, |e| e.0);
        if since > self.seq || since + 1 < oldest {
            return None;
        }
        Some(self.ring.iter().filter(|e| e.0 > since && wants(&e.1)).map(|e| e.2.as_str()).collect())
    }
}

/// The `@since=SEQ` among `types`, and the types without it.
pub(super) fn split_since(types: &[String]) -> Result<(Option<u64>, Vec<String>), String> {
    let mut since = None;
    let mut rest = Vec::new();
    for t in types {
        match t.strip_prefix(SINCE) {
            Some(n) if since.is_none() => since = Some(n.parse::<u64>().map_err(|_| format!("{SINCE}{n}: not a sequence number"))?),
            Some(_) => return Err("at most one @since".to_string()),
            None => rest.push(t.clone()),
        }
    }
    Ok((since, rest))
}

/// `text` as a JSON string, quotes included.
pub(super) fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if crate::sanitize::is_unsafe(c) || matches!(c, '\u{2028}' | '\u{2029}') => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The line of an event: its name, `seq` and `time`, then each variable as a string.
pub(super) fn event_line(name: &str, seq: u64, time_ms: u128, vars: &[(&str, String)]) -> String {
    let mut out = format!("{{\"event\":{},\"seq\":{seq},\"time\":{time_ms}", json_string(name));
    for (key, value) in vars {
        if *key == "hook" {
            continue;
        }
        let key = key.strip_prefix("hook_").unwrap_or(key);
        out.push_str(&format!(",{}:{}", json_string(key), json_string(value)));
    }
    out.push('}');
    out
}

/// The types a subscription asks for: known names, or `after-` followed by a command name.
pub(super) fn check_types(types: &[String]) -> Result<Option<HashSet<String>>, String> {
    if types.is_empty() {
        return Ok(None);
    }
    if types.len() > MAX_TYPES {
        return Err(format!("at most {MAX_TYPES} event types"));
    }
    let mut set = HashSet::new();
    for t in types {
        let known = hooks::ACTIVE.contains(&t.as_str()) || (t.starts_with("after-") && t.len() > 6);
        let shaped = t.len() <= MAX_TYPE && t.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !known || !shaped {
            return Err(format!("unknown event type {t:?} (imux events --types lists none; see the manual page for the names)"));
        }
        set.insert(t.clone());
    }
    Ok(Some(set))
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

impl Server {
    /// `ClientMsg::Subscribe`: the connection stays open and gets events from now on.
    pub(super) fn subscribe(&mut self, client: usize, types: &[String]) {
        let (since, names) = match split_since(types) {
            Ok(split) => split,
            Err(e) => return self.finish_request(client, Err(crate::errcode::tag("invalid_argument", &e))),
        };
        let types = match self.plugin_event_types(client, &names).and_then(|t| check_types(&t)) {
            Ok(types) => types,
            Err(e) => return self.finish_request(client, Err(e)),
        };
        self.stream.arm();
        let sub = Sub { types };
        let replay = since.map(|n| self.stream.replay(n, |name| sub.wants(name)).map(|lines| lines.into_iter().map(str::to_string).collect::<Vec<_>>()));
        let seq = self.stream.seq;
        self.stream.subs.insert(client, sub);
        let hello = format!("{{\"event\":\"subscribed\",\"api\":{API_VERSION},\"seq\":{seq}}}");
        self.queue(client, &ServerMsg::Event(hello));
        match replay {
            Some(Some(lines)) => {
                for line in lines {
                    self.queue(client, &ServerMsg::Event(line));
                }
            }
            Some(None) => {
                let lost = format!("{{\"event\":\"events_lost\",\"seq\":{seq}}}");
                self.queue(client, &ServerMsg::Event(lost));
            }
            None => {}
        }
    }

    /// `api-seq`: the sequence number of the last event, and every event is kept from now on.
    pub(super) fn event_seq(&mut self) -> u64 {
        self.stream.arm()
    }

    /// Count an event, keep it in the ring, and give it to every subscriber that wants it.
    pub(super) fn publish(&mut self, name: &str, vars: &Vars) {
        let ids: Vec<usize> = self.stream.subs.iter().filter(|(_, s)| s.wants(name)).map(|(id, _)| *id).collect();
        if ids.is_empty() && !self.stream.armed {
            return;
        }
        self.stream.seq += 1;
        let line = event_line(name, self.stream.seq, now_ms(), vars);
        if self.stream.armed {
            self.stream.ring.push_back((self.stream.seq, name.to_string(), line.clone()));
            if self.stream.ring.len() > RING {
                self.stream.ring.pop_front();
            }
        }
        for id in ids {
            self.send_event(id, &line);
        }
    }

    fn send_event(&mut self, id: usize, line: &str) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        if c.outbuf.len() >= BACKLOG {
            log::warn(&format!("event subscriber {id} does not read: events lost, closing"));
            let lost = format!("{{\"event\":\"events_lost\",\"seq\":{}}}", self.stream.seq);
            proto::frame(&ServerMsg::Event(lost), &mut c.outbuf);
            self.stream.subs.remove(&id);
            return self.close_after_flush(id);
        }
        self.queue(id, &ServerMsg::Event(line.to_string()));
    }

    pub(super) fn drop_subscriber(&mut self, id: usize) {
        self.stream.subs.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_escaped_for_json() {
        assert_eq!(json_string("plain"), "\"plain\"");
        assert_eq!(json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json_string("l1\nl2\r\t"), "\"l1\\nl2\\r\\t\"");
        assert_eq!(json_string("\u{1b}[31m\u{7f}\u{0}"), "\"\\u001b[31m\\u007f\\u0000\"");
        assert_eq!(json_string("é 日本 \u{1f600}"), "\"é 日本 \u{1f600}\"");
        assert_eq!(json_string(""), "\"\"");
    }

    #[test]
    fn a_line_has_the_name_the_counters_and_the_variables_without_their_prefix() {
        let vars = vec![("hook_session", "$1".to_string()), ("hook_window_name", "a \"b\"".to_string()), ("hook", "x".to_string())];
        assert_eq!(
            event_line("window-renamed", 12, 1731160000000, &vars),
            "{\"event\":\"window-renamed\",\"seq\":12,\"time\":1731160000000,\"session\":\"$1\",\"window_name\":\"a \\\"b\\\"\"}"
        );
        assert_eq!(event_line("x", 1, 2, &[]), "{\"event\":\"x\",\"seq\":1,\"time\":2}");
        let other = vec![("pane", "%3".to_string())];
        assert!(event_line("x", 1, 2, &other).ends_with(",\"pane\":\"%3\"}"), "a name without the prefix is kept");
    }

    #[test]
    fn a_value_cannot_break_out_of_its_string() {
        let vars = vec![("hook_message", "\",\"event\":\"events_lost\",\"x\":\"".to_string())];
        let line = event_line("command-error", 1, 2, &vars);
        assert_eq!(line.matches("\"event\":").count(), 1, "{line}");
        assert!(line.contains("\\\",\\\"event\\\""), "{line}");
    }

    #[test]
    fn no_types_means_everything_but_the_after_events() {
        let all = Sub { types: None };
        assert!(all.wants("window-renamed") && all.wants("agent-state-changed") && all.wants("command-error"));
        assert!(!all.wants("after-new-window"));
        let some = Sub { types: check_types(&["pane-died".into(), "after-new-window".into()]).unwrap() };
        assert!(some.wants("pane-died") && some.wants("after-new-window"));
        assert!(!some.wants("window-renamed") && !some.wants("after-kill-pane"));
    }

    #[test]
    fn types_are_checked() {
        assert_eq!(check_types(&[]), Ok(None));
        assert!(check_types(&["window-renamed".into(), "after-split-window".into()]).is_ok());
        for bad in ["", "nonsense", "Window-Renamed", "after-", "pane-set-clipboard", "window renamed", &"a".repeat(65)] {
            assert!(check_types(&[bad.to_string()]).is_err(), "{bad:?}");
        }
        let many: Vec<String> = (0..33).map(|_| "pane-died".to_string()).collect();
        assert!(check_types(&many).is_err());
    }

    #[test]
    fn every_event_imux_fires_can_be_asked_for() {
        for name in hooks::ACTIVE {
            assert!(check_types(&[name.to_string()]).is_ok(), "{name}");
        }
    }

    fn kept(stream: &mut Stream, names: &[&str]) {
        for name in names {
            stream.seq += 1;
            stream.ring.push_back((stream.seq, name.to_string(), format!("line{}", stream.seq)));
            if stream.ring.len() > RING {
                stream.ring.pop_front();
            }
        }
    }

    #[test]
    fn an_armed_stream_wants_every_event_but_the_after_ones() {
        let mut stream = Stream::default();
        assert!(stream.is_empty() && !stream.wants("pane-died"));
        assert_eq!(stream.arm(), 0);
        assert!(!stream.is_empty() && stream.wants("pane-died") && stream.wants("agent-state-changed"));
        assert!(!stream.wants("after-kill-pane"), "after- events only come in by name");
        stream.subs.insert(1, Sub { types: check_types(&["after-kill-pane".into()]).unwrap() });
        assert!(stream.wants("after-kill-pane"));
    }

    #[test]
    fn a_replay_is_what_came_after_the_sequence_and_the_subscriber_wants() {
        let mut stream = Stream::default();
        stream.arm();
        kept(&mut stream, &["pane-died", "window-renamed", "pane-died"]);
        let all = |_: &str| true;
        assert_eq!(stream.replay(0, all), Some(vec!["line1", "line2", "line3"]));
        assert_eq!(stream.replay(2, all), Some(vec!["line3"]));
        assert_eq!(stream.replay(3, all), Some(vec![]), "nothing came after the last one");
        assert_eq!(stream.replay(0, |n| n == "window-renamed"), Some(vec!["line2"]));
    }

    #[test]
    fn a_replay_says_lost_when_events_are_gone_or_the_sequence_is_from_elsewhere() {
        let mut stream = Stream::default();
        stream.arm();
        let names: Vec<&str> = (0..RING + 10).map(|_| "pane-died").collect();
        kept(&mut stream, &names);
        assert_eq!(stream.ring.len(), RING);
        assert_eq!(stream.seq as usize, RING + 10);
        assert!(stream.replay(0, |_| true).is_none(), "the first ten are gone");
        assert!(stream.replay(9, |_| true).is_none());
        assert_eq!(stream.replay(10, |_| true).map(|l| l.len()), Some(RING), "all the kept ones are there");
        assert!(stream.replay(stream.seq + 1, |_| true).is_none(), "a number from a longer life");
        let mut empty = Stream::default();
        empty.arm();
        assert_eq!(empty.replay(0, |_| true), Some(vec![]));
        assert!(empty.replay(1, |_| true).is_none());
    }

    #[test]
    fn since_is_taken_out_of_the_types() {
        let words = |list: &[&str]| list.iter().map(|w| w.to_string()).collect::<Vec<String>>();
        assert_eq!(split_since(&words(&["pane-died", "@since=12"])), Ok((Some(12), words(&["pane-died"]))));
        assert_eq!(split_since(&words(&[])), Ok((None, vec![])));
        assert!(split_since(&words(&["@since=x"])).is_err());
        assert!(split_since(&words(&["@since=1", "@since=2"])).is_err());
        assert!(check_types(&words(&["@since=1"])).is_err(), "it is not an event name");
    }

    #[test]
    fn the_stream_knows_whether_anybody_wants_an_event() {
        let mut stream = Stream::default();
        assert!(stream.is_empty() && !stream.wants("pane-died"));
        stream.subs.insert(1, Sub { types: check_types(&["pane-died".into()]).unwrap() });
        assert!(stream.wants("pane-died") && !stream.wants("window-renamed") && !stream.is_empty());
        stream.subs.insert(2, Sub { types: None });
        assert!(stream.wants("window-renamed") && !stream.wants("after-kill-pane"));
    }
}
