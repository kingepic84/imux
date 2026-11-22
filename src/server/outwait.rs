//! `imux pane wait-output` (P10.5): hold a connection open until a pane shows a line with some
//! text, or one the regular expression matches.
//!
//! The request is the socket command `api-pane-wait PANE KIND PATTERN SOURCE LINES TIMEOUT_MS TEXT`
//! (the connection stays open, `Server::deferred`). What the pane shows now is looked at first, so
//! a line that is already there matches. After that a waiter is looked at again only when its pane
//! produced output (`output_touched`, set by the pane read path), so no thread or timer polls.
//! The wait ends with the match, with the pane gone or its program ended, at the timeout, or when
//! the connection closes (`drop_waiters_of`). Waiters are bounded: `WAITERS_MAX` in all and
//! `WAITERS_PER_CALLER` for one pane (or for the client and everything outside a pane). The text
//! of a pane is data: it is matched, never run, and the answer is the line as the pane shows it.

use std::time::{Duration, Instant};

use super::Server;
use super::remote::searchable_text;
use crate::errcode::tag;
use crate::log;
use crate::session::Pane;
use crate::session::layout::PaneId;

/// Waiters at once, for the whole server.
pub(super) const WAITERS_MAX: usize = 64;
/// Waiters at once for one caller: a pane, or the client and everything outside a pane.
pub(super) const WAITERS_PER_CALLER: usize = 8;
/// The longest pattern, for text and for a regular expression.
pub const PATTERN_MAX: usize = 512;
/// The most history lines `--lines` may name.
pub const LINES_MAX: usize = 10_000;

/// What a line has to hold.
pub(super) enum Matcher {
    Literal(String),
    Regex(regex_lite::Regex),
}

impl Matcher {
    fn line_matches(&self, line: &str) -> bool {
        match self {
            Matcher::Literal(text) => line.contains(text.as_str()),
            Matcher::Regex(re) => re.is_match(line),
        }
    }

    /// The first line of `text` that matches.
    fn first_in<'a>(&self, text: &'a str) -> Option<&'a str> {
        text.lines().find(|line| self.line_matches(line))
    }
}

/// A client holding its connection open until a pane shows a line.
pub struct OutputWaiter {
    client: usize,
    caller: Option<PaneId>,
    pane: PaneId,
    matcher: Matcher,
    visible_only: bool,
    history: usize,
    deadline: Option<Instant>,
    with_text: bool,
}

fn answer(line: &str, text: &str, with_text: bool) -> String {
    if with_text { format!("{line}\n{text}") } else { format!("{line}\n") }
}

impl OutputWaiter {
    /// The answer if the pane shows a matching line now.
    fn look(&self, pane: &Pane) -> Option<String> {
        let text = searchable_text(pane, self.visible_only, self.history);
        self.matcher.first_in(&text).map(|line| answer(line, &text, self.with_text))
    }
}

impl Server {
    fn pane_ref(&self, id: PaneId) -> Option<&Pane> {
        let place = self.place_of_pane(id)?;
        self.sessions.get(place.si)?.windows.get(place.wi)?.panes.get(&id)
    }

    /// `api-pane-wait PANE match|regex PATTERN visible|recent LINES TIMEOUT_MS TEXT(0|1)`.
    /// Who may ask is the gate of the socket; a program that may only look reads its own pane.
    pub(super) fn cmd_pane_wait(&mut self, client: usize, rest: &[String], caller: Option<PaneId>, own_pane_only: bool) -> Result<String, String> {
        let [pane, kind, pattern, source, lines, timeout_ms, with_text] = rest else {
            return Err(tag("invalid_argument", "usage: api-pane-wait pane match|regex pattern visible|recent lines timeout_ms text"));
        };
        let bad = |what: &str| tag("invalid_argument", what);
        let id = pane.trim_start_matches('%').parse::<PaneId>().map_err(|_| bad(&format!("{pane:?} is not a pane number")))?;
        if pattern.is_empty() {
            return Err(bad("the text to wait for is empty"));
        }
        let matcher = match kind.as_str() {
            "match" if pattern.len() <= PATTERN_MAX => Matcher::Literal(pattern.clone()),
            "regex" if pattern.len() <= PATTERN_MAX => Matcher::Regex(
                regex_lite::RegexBuilder::new(pattern).build().map_err(|e| tag("invalid_regex", &format!("the pattern does not compile: {e}")))?,
            ),
            "match" | "regex" => return Err(tag(if kind == "regex" { "invalid_regex" } else { "invalid_argument" }, &format!("the pattern is longer than {PATTERN_MAX} bytes"))),
            _ => return Err(bad("the kind is match or regex")),
        };
        let visible_only = match source.as_str() {
            "visible" => true,
            "recent" => false,
            _ => return Err(bad("the source is visible or recent")),
        };
        let history = lines.parse::<usize>().ok().filter(|n| (1..=LINES_MAX).contains(n)).ok_or_else(|| bad(&format!("lines is a number from 1 to {LINES_MAX}")))?;
        let timeout = timeout_ms.parse::<u64>().map_err(|_| bad("timeout is a number of milliseconds"))?;
        if own_pane_only && caller != Some(id) {
            return Err(tag("permission_denied", "pane wait-output: a program in a pane may only read its own pane (set command-from-panes on or spawned to allow more)"));
        }
        let waiter = OutputWaiter {
            client,
            caller,
            pane: id,
            matcher,
            visible_only,
            history,
            deadline: (timeout > 0).then(|| Instant::now() + Duration::from_millis(timeout)),
            with_text: with_text == "1",
        };
        let found = self.pane_ref(id).ok_or_else(|| tag("not_found", &format!("no pane {id}")))?;
        if let Some(done) = waiter.look(found) {
            return Ok(done);
        }
        if found.dead.is_some() {
            return Err(tag("not_ready", &format!("the program in pane {id} has ended")));
        }
        if self.output_waiters.len() >= WAITERS_MAX {
            return Err(tag("limit_reached", &format!("{WAITERS_MAX} waits for output are already open on this server")));
        }
        if self.output_waiters.iter().filter(|w| w.caller == caller).count() >= WAITERS_PER_CALLER {
            return Err(tag("limit_reached", &format!("{WAITERS_PER_CALLER} waits for output are already open for this caller")));
        }
        self.output_waiters.push(waiter);
        self.deferred = true;
        Ok(String::new())
    }

    /// Answer the waiters whose pane showed a match, ended, vanished or ran out of time. Called
    /// once per loop turn; only a pane that produced output since the last turn is read again.
    pub(super) fn check_output_waiters(&mut self) {
        if self.output_waiters.is_empty() {
            self.output_touched.clear();
            return;
        }
        let touched = std::mem::take(&mut self.output_touched);
        let now = Instant::now();
        let waiters = std::mem::take(&mut self.output_waiters);
        let mut keep = Vec::new();
        for w in waiters {
            let outcome = match self.pane_ref(w.pane) {
                None => Some(Err(tag("not_found", &format!("pane {} closed", w.pane)))),
                Some(pane) => match touched.contains(&w.pane).then(|| w.look(pane)).flatten() {
                    Some(done) => Some(Ok(done)),
                    None if pane.dead.is_some() => Some(Err(tag("not_ready", &format!("the program in pane {} has ended", w.pane)))),
                    None if w.deadline.is_some_and(|d| d <= now) => Some(Err(tag("timeout", "timed out waiting for the output"))),
                    None => None,
                },
            };
            match outcome {
                Some(result) => {
                    if self.clients.contains_key(&w.client) {
                        self.finish_request(w.client, result);
                    }
                }
                None => keep.push(w),
            }
        }
        self.output_waiters.extend(keep);
    }

    /// The soonest timeout of a wait for output, for the event loop's poll.
    pub(super) fn output_deadline(&self) -> Option<Instant> {
        self.output_waiters.iter().filter_map(|w| w.deadline).min()
    }

    /// A client went away: its waits for output end.
    pub(super) fn drop_output_waiters_of(&mut self, client: usize) {
        if self.output_waiters.iter().any(|w| w.client == client) {
            log::info(&format!("client {client} left while waiting for output"));
            self.output_waiters.retain(|w| w.client != client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_matches_a_part_of_a_line_and_the_first_line_wins() {
        let m = Matcher::Literal("done".to_string());
        assert_eq!(m.first_in("one\nall done here\nnot done yet\n"), Some("all done here"));
        assert_eq!(m.first_in("nothing\n"), None);
        assert_eq!(m.first_in(""), None);
    }

    #[test]
    fn a_regex_is_matched_per_line() {
        let m = Matcher::Regex(regex_lite::Regex::new(r"^ok \d+$").unwrap());
        assert_eq!(m.first_in("ok x\nok 12\n"), Some("ok 12"));
        assert_eq!(m.first_in("ok 1\nok 2"), Some("ok 1"));
        assert_eq!(m.first_in("a ok 12\n"), None, "the anchor is the line, not the text");
    }

    #[test]
    fn the_answer_is_the_line_and_with_text_the_text_after_it() {
        assert_eq!(answer("hit", "a\nhit\n", false), "hit\n");
        assert_eq!(answer("hit", "a\nhit\n", true), "hit\na\nhit\n");
    }
}
