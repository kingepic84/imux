//! `imux agent list|get|wait` and `imux pane report-agent|release-agent`: what scripts and
//! programs in panes can ask of the agent records over the socket. A report is only taken from a
//! process that runs inside the pane it names.

use std::time::{Duration, Instant};

use super::Server;
use super::agents::{self, Sort};
use super::spawned::Power;
use crate::buffers;
use crate::errcode::{json_string, tag};
use crate::log;
use crate::proto::{
    AgentWaitArgs, MAX_PROMPT, PromptArgs, ReportArgs, ServerMsg, UNTIL_BLOCKED, UNTIL_DONE, UNTIL_ERROR, UNTIL_IDLE, UNTIL_WORKING,
};
use crate::session::Pane;
use crate::session::agent::{self, Reported};
use crate::session::layout::PaneId;
use crate::term::status::{Report, State};
use crate::tty::proc;

const MAX_SOURCE: usize = 80;
const MAX_AGENT: usize = 32;
const MAX_MESSAGE: usize = 2048;

/// A client that is holding its connection open until a pane's agent reaches a state.
pub struct Waiter {
    client: usize,
    pane: PaneId,
    until: u8,
    deadline: Option<Instant>,
    /// The program that was there when the wait began; another one taking the pane ends the wait.
    app: Option<String>,
    /// Only a state that began after this counts (`agent prompt --wait`: the state the agent was
    /// in when the text was typed is not the answer to it).
    after: Option<Instant>,
    /// A prompt wait: the agent has been seen working or blocked since the text (or was working
    /// when it was typed). Until then no settled state answers the wait, and `stall` after the
    /// text (the option `agent-stall-time`) with none seen the wait fails as `prompt_stalled`.
    active: bool,
    stall: Duration,
}

/// A stall or a timeout of a prompt says nothing about whether the text arrived.
const NOT_PROOF: &str = "that does not show the text was not delivered: read the pane (imux agent read) before typing it again";

/// One look at the agent of a prompt wait that began when the text was typed at `sent`: the
/// answer if there is one. A state only counts if it began after the text; the first settled one
/// answers once the agent has been active (`working`, `blocked`, or a turn that was already
/// running), `blocked` and `error` at once; an older `idle` or `done` never does.
fn prompt_step(w: &mut Waiter, r: &agent::AgentRecord, sent: Instant, now: Instant) -> Option<Result<String, String>> {
    let changed = r.since > sent;
    if changed && (matches!(r.state, State::Working | State::Blocked) || matches!(r.before, Some(State::Working | State::Blocked))) {
        w.active = true;
    }
    if changed && state_bit(r.state) & w.until != 0 && (w.active || r.state == State::Error) {
        return Some(Ok(format!("{}\n", state_word(r.state))));
    }
    if w.deadline.is_some_and(|d| d <= now) {
        return Some(Err(tag("timeout", &format!("timed out waiting for the agent (the timeout covers typing and the wait); {NOT_PROOF}"))));
    }
    if !w.active && now.duration_since(sent) >= w.stall {
        return Some(Err(tag(
            "prompt_stalled",
            &format!("the agent reported neither working nor blocked within {} seconds of the text (agent-stall-time); {NOT_PROOF}", w.stall.as_secs()),
        )));
    }
    None
}

fn state_bit(state: State) -> u8 {
    match state {
        State::Idle => UNTIL_IDLE,
        State::Working => UNTIL_WORKING,
        State::Done => UNTIL_DONE,
        State::Blocked => UNTIL_BLOCKED,
        State::Error => UNTIL_ERROR,
        State::Clear => 0,
    }
}

pub fn state_word(state: State) -> &'static str {
    match state {
        State::Idle => "idle",
        State::Working => "working",
        State::Done => "done",
        State::Blocked => "blocked",
        State::Error => "error",
        State::Clear => "clear",
    }
}

fn kind_word(kind: crate::term::status::Kind) -> &'static str {
    use crate::term::status::Kind;
    match kind {
        Kind::Permission => "permission",
        Kind::Question => "question",
        Kind::Auth => "auth",
    }
}

fn parse_state(word: &str) -> Option<State> {
    [State::Idle, State::Working, State::Done, State::Blocked, State::Error].into_iter().find(|s| state_word(*s) == word)
}

/// What is typed for a prompt: the text as a paste (inside the bracketed paste markers when the
/// program there asked for them, line breaks as Enter, no escape or other control character), then
/// Enter.
fn prompt_payload(text: &str, bracketed: bool) -> Result<Vec<u8>, String> {
    let body = buffers::paste_bytes(text, "\r", false);
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err("nothing to send".to_string());
    }
    let mut bytes = Vec::with_capacity(body.len() + 13);
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    bytes.extend_from_slice(&body);
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    bytes.push(b'\r');
    Ok(bytes)
}

fn name_ok(text: &str, max: usize, extra: &[u8]) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

impl Server {
    fn pane_by_id(&self, id: PaneId) -> Option<&Pane> {
        self.sessions.iter().flat_map(|s| s.windows.iter()).find_map(|w| w.panes.get(&id))
    }

    fn pane_by_id_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).find_map(|w| w.panes.get_mut(&id))
    }

    /// Answer a one-shot request and close the connection.
    pub(super) fn finish_request(&mut self, client: usize, result: Result<String, String>) {
        let msg = match result {
            Ok(text) => ServerMsg::Text(text),
            Err(e) => ServerMsg::Error(e),
        };
        self.queue(client, &msg);
        self.close_after_flush(client);
    }

    /// One line per pane with a status record: pane, place, state, name, message.
    pub(super) fn agent_list_text(&self) -> String {
        let rows = agents::build_rows(&self.sessions, usize::MAX, Instant::now(), Sort::Order);
        let mut out = String::new();
        for r in rows {
            let session = self.sessions.iter().find(|s| s.id == r.session).map_or("", |s| s.name.as_str());
            out.push_str(&format!("{}\t{}:{}\t{}\t{}\t{}\n", r.pane, session, r.window, state_word(r.state), r.name, r.msg));
        }
        out
    }

    pub(super) fn agent_get_text(&self, pane: u32) -> Result<String, String> {
        let p = self.pane_by_id(pane).ok_or_else(|| tag("not_found", &format!("no pane {pane}")))?;
        let r = p.agent.as_ref().ok_or_else(|| tag("agent_not_running", &self.no_record_reason(pane)))?;
        let mut out = format!("pane: {pane}\nstate: {}\nsource: {}\n", state_word(r.state), r.source);
        if let Some(app) = &r.app {
            out.push_str(&format!("agent: {app}\n"));
        }
        if let Some(name) = &r.name {
            out.push_str(&format!("name: {name}\n"));
        }
        if let Some(kind) = r.kind {
            out.push_str(&format!("kind: {}\n", kind_word(kind)));
        }
        if let Some(progress) = r.progress {
            out.push_str(&format!("progress: {progress}\n"));
        }
        if let Some(msg) = &r.msg {
            out.push_str(&format!("message: {msg}\n"));
        }
        if let Some(resume) = &r.resume {
            out.push_str(&format!("resume: {}\n", resume.join(" ")));
        }
        out.push_str(&format!("since: {}s\n", r.since.elapsed().as_secs()));
        out.push_str(&format!("seen: {}\n", if r.seen_by.is_empty() { "no" } else { "yes" }));
        let mut seqs: Vec<(&String, &u64)> = r.seqs.iter().collect();
        seqs.sort();
        for (source, seq) in seqs {
            out.push_str(&format!("seq {source}: {seq}\n"));
        }
        Ok(out)
    }

    /// Why a pane has no agent record, for the message: nothing reported, or a source released it.
    fn no_record_reason(&self, pane: u32) -> String {
        format!("pane {pane} has no agent record (no program has reported on it, or its source released it)")
    }

    /// The record of one pane as a JSON object with the field names of the event stream (ids as
    /// strings: `$1`, `@2`, `%3`), or why there is none.
    pub(super) fn agent_json(&self, pane: u32) -> Result<String, String> {
        let place = self.place_of_pane(pane).ok_or_else(|| tag("not_found", &format!("no pane {pane}")))?;
        let session = &self.sessions[place.si];
        let window = &session.windows[place.wi];
        let p = window.panes.get(&pane).ok_or_else(|| tag("not_found", &format!("no pane {pane}")))?;
        let r = p.agent.as_ref().ok_or_else(|| tag("agent_not_running", &self.no_record_reason(pane)))?;
        let text = |value: &str| json_string(&crate::sanitize::clean(value));
        let mut out = format!(
            "{{\"session\":\"${}\",\"session_name\":{},\"window\":\"@{}\",\"window_name\":{},\"pane\":\"%{pane}\",\"state\":\"{}\",\"source\":{}",
            session.id,
            text(&session.name),
            window.id,
            text(&window.name),
            state_word(r.state),
            text(&r.source)
        );
        if let Some(app) = &r.app {
            out.push_str(&format!(",\"agent\":{}", text(app)));
        }
        if let Some(name) = &r.name {
            out.push_str(&format!(",\"name\":{}", text(name)));
        }
        if let Some(kind) = r.kind {
            out.push_str(&format!(",\"kind\":\"{}\"", kind_word(kind)));
        }
        if let Some(progress) = r.progress {
            out.push_str(&format!(",\"progress\":{progress}"));
        }
        if let Some(msg) = &r.msg {
            out.push_str(&format!(",\"message\":{}", text(msg)));
        }
        out.push_str(&format!(",\"state_since\":{},\"seen\":{}", r.since.elapsed().as_secs(), !r.seen_by.is_empty()));
        let mut seqs: Vec<(&String, &u64)> = r.seqs.iter().collect();
        seqs.sort();
        let seqs: Vec<String> = seqs.into_iter().map(|(source, seq)| format!("{}:{seq}", text(source))).collect();
        out.push_str(&format!(",\"seqs\":{{{}}}", seqs.join(",")));
        if let Some(resume) = &r.resume {
            let words: Vec<String> = resume.iter().map(|w| text(w)).collect();
            out.push_str(&format!(",\"resume\":[{}]", words.join(",")));
        }
        out.push_str(&format!(",\"pid\":{}", p.pty.child.as_raw()));
        if let Some(cwd) = crate::tty::proc::cwd(p.pty.child.as_raw()) {
            out.push_str(&format!(",\"cwd\":{}", text(&cwd.to_string_lossy())));
        }
        out.push('}');
        Ok(out)
    }

    /// Every pane that has a record, in the order of `agent list`, as a JSON array.
    pub(super) fn agent_list_json(&self) -> String {
        let rows = agents::build_rows(&self.sessions, usize::MAX, Instant::now(), Sort::Order);
        let objects: Vec<String> = rows.iter().filter_map(|r| self.agent_json(r.pane).ok()).collect();
        format!("[{}]\n", objects.join(","))
    }

    /// The caller of a pane request has to be a descendant of the pane's shell.
    fn caller_runs_in(&self, client: usize, pane: PaneId) -> Result<(), String> {
        let peer = self.clients.get(&client).and_then(|c| c.peer).ok_or("cannot tell which process is asking")?;
        let child = self.pane_by_id(pane).ok_or_else(|| format!("no pane {pane}"))?.pty.child.as_raw();
        if proc::descends_from(peer, child) { Ok(()) } else { Err(format!("pane {pane} is not where this command runs")) }
    }

    fn check_source(source: &str) -> Result<(), String> {
        if !name_ok(source, MAX_SOURCE, b":._-") {
            return Err("source must be 1 to 80 characters of letters, digits and : . _ -".to_string());
        }
        if source.starts_with("imux:") || source == agent::OSC_SOURCE {
            return Err(format!("source {source} is reserved"));
        }
        Ok(())
    }

    pub(super) fn handle_report(&mut self, client: usize, args: ReportArgs) -> Result<String, String> {
        self.caller_runs_in(client, args.pane)?;
        Self::check_source(&args.source)?;
        if !name_ok(&args.agent, MAX_AGENT, b"_.+-") {
            return Err("agent must be 1 to 32 characters of letters, digits and _ . + -".to_string());
        }
        let state = parse_state(&args.state).ok_or_else(|| format!("unknown state {}", args.state))?;
        if let Some(m) = &args.message
            && (m.len() > MAX_MESSAGE || m.chars().any(crate::sanitize::is_unsafe))
        {
            return Err("message is too long or has a control or bidirectional formatting character".to_string());
        }
        let resume = (!args.resume.is_empty()).then(|| args.resume.clone());
        if let Some(argv) = &resume {
            let allowed: Vec<String> = self.options.text("restore-commands").split_whitespace().map(str::to_string).collect();
            agent::validate_resume(argv, &allowed).map_err(|e| format!("resume command refused: {e}"))?;
        }
        let report = Report { state, kind: None, progress: None, app: Some(args.agent), title: None, msg: args.message, id: None };
        let applied = {
            let pane = self.pane_by_id_mut(args.pane).ok_or_else(|| format!("no pane {}", args.pane))?;
            agent::apply_reported(&mut pane.agent, report, &args.source, args.seq, resume, Instant::now())
        };
        self.dirty = true;
        self.adopt_pending_name(args.pane);
        self.check_waiters();
        Ok(if applied == Reported::Applied { "ok\n".to_string() } else { "ok (stale, ignored)\n".to_string() })
    }

    pub(super) fn handle_release(&mut self, client: usize, pane: u32, source: &str) -> Result<String, String> {
        self.caller_runs_in(client, pane)?;
        Self::check_source(source)?;
        let cleared = self.pane_by_id_mut(pane).is_some_and(|p| agent::release(&mut p.agent, source));
        self.dirty = true;
        self.check_waiters();
        Ok(if cleared { "released\n".to_string() } else { "nothing to release\n".to_string() })
    }

    /// Start waiting, or answer at once when the agent is already in one of the states.
    pub(super) fn start_wait(&mut self, client: usize, args: AgentWaitArgs) {
        let found = self.pane_by_id(args.pane).map(|p| p.agent.as_ref().map(|r| (r.state, r.app.clone())));
        let (state, app) = match found {
            None => return self.finish_request(client, Err(tag("not_found", &format!("no pane {}", args.pane)))),
            Some(None) => return self.finish_request(client, Err(tag("agent_not_running", &format!("pane {} has no agent record", args.pane)))),
            Some(Some(found)) => found,
        };
        let mask = if args.until == 0 { UNTIL_IDLE | UNTIL_DONE | UNTIL_BLOCKED | UNTIL_ERROR } else { args.until };
        if state_bit(state) & mask != 0 {
            return self.finish_request(client, Ok(format!("{}\n", state_word(state))));
        }
        let deadline = (args.timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(u64::from(args.timeout_ms)));
        self.waiters.push(Waiter { client, pane: args.pane, until: mask, deadline, app, after: None, active: false, stall: Duration::ZERO });
    }

    /// Why `imux agent prompt` may not type into a pane, or what to type.
    fn prompt_bytes(&self, client: usize, args: &PromptArgs) -> Result<Vec<u8>, String> {
        if !self.options.flag("agent-control") {
            return Err(tag("permission_denied", "agent prompt is off: set agent-control on to let scripts type into agent panes"));
        }
        let caller = self.pane_of_peer(client);
        match self.power_of(caller) {
            Power::Full => {}
            Power::Look => {
                let from = caller.unwrap_or_default();
                log::warn(&format!("refused agent prompt from pane {from}: command-from-panes is off"));
                return Err(tag("permission_denied", "agent prompt: not allowed from inside a pane (set command-from-panes on to allow it)"));
            }
            Power::Subtree(me) if !self.in_subtree(me, args.pane, true) => {
                log::warn(&format!("refused agent prompt from pane {me} into pane {}: not one it spawned", args.pane));
                let message = format!("agent prompt: pane {} is not one this pane spawned (command-from-panes {} holds a pane to the panes it spawned)", args.pane, self.options.text("command-from-panes"));
                return Err(tag("permission_denied", &message));
            }
            Power::Subtree(_) => {}
        }
        if args.text.len() > MAX_PROMPT {
            return Err(tag("limit_reached", &format!("the prompt is longer than {MAX_PROMPT} bytes")));
        }
        let pane = self.pane_by_id(args.pane).ok_or_else(|| tag("not_found", &format!("no pane {}", args.pane)))?;
        if pane.dead.is_some() {
            return Err(tag("not_ready", &format!("pane {} is dead", args.pane)));
        }
        let record = pane.agent.as_ref().ok_or_else(|| tag("agent_not_running", &format!("pane {} has no agent record", args.pane)))?;
        if record.state == State::Blocked {
            return Err(tag("agent_blocked", &format!("pane {} is blocked on a permission prompt: answer that first, the text would be taken as the answer", args.pane)));
        }
        prompt_payload(&args.text, pane.term.grid.modes.bracketed_paste).map_err(|e| tag("invalid_argument", &e))
    }

    /// `ClientMsg::AgentPrompt`: type the text and Enter into the pane, then answer, or hold the
    /// connection until the agent has gone through a turn.
    pub(super) fn handle_prompt(&mut self, client: usize, args: PromptArgs) {
        let bytes = match self.prompt_bytes(client, &args) {
            Ok(bytes) => bytes,
            Err(e) => return self.finish_request(client, Err(e)),
        };
        let Some(pane) = self.pane_by_id(args.pane) else { return self.finish_request(client, Err(tag("not_found", &format!("no pane {}", args.pane)))) };
        let app = pane.agent.as_ref().and_then(|r| r.app.clone());
        let working = pane.agent.as_ref().is_some_and(|r| r.state == State::Working);
        if let Err(e) = pane.pty.write_typed(&bytes) {
            return self.finish_request(client, Err(format!("agent prompt: {e}")));
        }
        let sent = Instant::now();
        let stall = Duration::from_secs(self.options.num("agent-stall-time"));
        log::info(&format!("agent prompt: {} bytes typed into pane {}{}", args.text.len(), args.pane, if args.wait { ", waiting" } else { "" }));
        if !args.wait {
            return self.finish_request(client, Ok("sent\n".to_string()));
        }
        let deadline = (args.timeout_ms > 0).then(|| sent + Duration::from_millis(u64::from(args.timeout_ms)));
        let until = UNTIL_IDLE | UNTIL_DONE | UNTIL_BLOCKED | UNTIL_ERROR;
        self.waiters.push(Waiter { client, pane: args.pane, until, deadline, app, after: Some(sent), active: working, stall });
    }

    /// `api-pane-run PANE TEXT`: type the text and Enter into a pane, as `agent prompt` does, with
    /// no agent needed. Success means the bytes were written, not that the program ran them. A
    /// pane that has an agent is typed into only with `agent-control` on and when it is not
    /// blocked (the text would answer its question). Who may ask is the gate of the socket
    /// (`gate_subtree`: panes the caller spawned).
    pub(super) fn pane_run(&mut self, rest: &[String]) -> Result<String, String> {
        let [pane, text] = rest else { return Err(tag("invalid_argument", "usage: api-pane-run pane text")) };
        let id = pane.trim_start_matches('%').parse::<u32>().map_err(|_| tag("invalid_argument", &format!("{pane:?} is not a pane number")))?;
        if text.len() > MAX_PROMPT {
            return Err(tag("limit_reached", &format!("the text is longer than {MAX_PROMPT} bytes")));
        }
        let agent_control = self.options.flag("agent-control");
        let found = self.pane_by_id(id).ok_or_else(|| tag("not_found", &format!("no pane {id}")))?;
        if found.dead.is_some() {
            return Err(tag("not_ready", &format!("pane {id} is dead")));
        }
        if let Some(record) = &found.agent {
            if !agent_control {
                return Err(tag("permission_denied", &format!("pane {id} runs an agent: set agent-control on to let scripts type into agent panes")));
            }
            if record.state == State::Blocked {
                return Err(tag("agent_blocked", &format!("pane {id} is blocked on a permission prompt: answer that first, the text would be taken as the answer")));
            }
        }
        let bytes = prompt_payload(text, found.term.grid.modes.bracketed_paste).map_err(|e| tag("invalid_argument", &e))?;
        found.pty.write_typed(&bytes).map_err(|e| format!("pane run: {e}"))?;
        log::info(&format!("pane run: {} bytes typed into pane {id}", text.len()));
        Ok("sent\n".to_string())
    }

    /// Answer every waiter whose pane reached its state, lost its agent or ran out of time.
    pub(super) fn check_waiters(&mut self) {
        if self.waiters.is_empty() {
            return;
        }
        let now = Instant::now();
        let waiters = std::mem::take(&mut self.waiters);
        let mut keep = Vec::new();
        for mut w in waiters {
            let record = self.pane_by_id(w.pane).map(|p| p.agent.clone());
            let outcome = match record {
                None => Some(Err(tag("not_found", &format!("pane {} closed", w.pane)))),
                Some(None) => Some(Err(tag("agent_not_running", "the agent exited"))),
                Some(Some(r)) if r.app != w.app => Some(Err(tag("agent_not_running", "another agent took the pane"))),
                Some(Some(r)) => match w.after {
                    Some(sent) => prompt_step(&mut w, &r, sent, now),
                    None if state_bit(r.state) & w.until != 0 => Some(Ok(format!("{}\n", state_word(r.state)))),
                    None if w.deadline.is_some_and(|d| d <= now) => Some(Err(tag("timeout", "timed out waiting for the agent"))),
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
        self.waiters.extend(keep);
    }

    /// The soonest wait timeout, for the event loop's poll.
    pub(super) fn waiter_deadline(&self) -> Option<Instant> {
        self.waiters.iter().flat_map(|w| w.deadline.into_iter().chain(w.after.filter(|_| !w.active).map(|t| t + w.stall))).min()
    }

    pub(super) fn drop_waiters_of(&mut self, client: usize) {
        self.channels.drop_client(client);
        if self.waiters.iter().any(|w| w.client == client) {
            log::info(&format!("client {client} left while waiting"));
            self.waiters.retain(|w| w.client != client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_words_round_trip() {
        for s in [State::Idle, State::Working, State::Done, State::Blocked, State::Error] {
            assert_eq!(parse_state(state_word(s)), Some(s));
            assert_ne!(state_bit(s), 0);
        }
        assert_eq!(parse_state("clear"), None);
        assert_eq!(parse_state(""), None);
    }

    #[test]
    fn a_prompt_is_typed_as_a_paste_and_then_enter() {
        assert_eq!(prompt_payload("fix it", false).unwrap(), b"fix it\r");
        assert_eq!(prompt_payload("fix it", true).unwrap(), b"\x1b[200~fix it\x1b[201~\r");
        assert_eq!(prompt_payload("a\nb\r\nc", false).unwrap(), b"a\rb\rc\r");
    }

    #[test]
    fn a_prompt_can_not_end_the_paste_or_send_keys() {
        let evil = prompt_payload("x\x1b[201~rm -rf /\x03\x04\x7f\u{9b}y", true).unwrap();
        assert_eq!(evil, b"\x1b[200~x[201~rm -rf /y\x1b[201~\r");
        assert_eq!(evil.iter().filter(|b| **b == 0x1b).count(), 2, "only our own two markers");
    }

    #[test]
    fn a_prompt_with_nothing_to_type_is_refused() {
        for empty in ["", " ", "\n\n", "\t \r\n", "\x1b\x03"] {
            assert!(prompt_payload(empty, false).is_err(), "{empty:?}");
        }
    }

    fn secs(base: Instant, n: u64) -> Instant {
        base + Duration::from_secs(n)
    }

    fn put_state(slot: &mut Option<agent::AgentRecord>, state: State, at: Instant) {
        agent::apply(slot, Report { state, kind: None, progress: None, app: Some("fake".into()), title: None, msg: None, id: None }, at);
    }

    fn prompt_waiter(sent: Instant, active: bool, timeout: Option<u64>) -> Waiter {
        let until = UNTIL_IDLE | UNTIL_DONE | UNTIL_BLOCKED | UNTIL_ERROR;
        Waiter { client: 1, pane: 0, until, deadline: timeout.map(|t| secs(sent, t)), app: Some("fake".into()), after: Some(sent), active, stall: Duration::from_secs(5) }
    }

    fn step(w: &mut Waiter, slot: &Option<agent::AgentRecord>, sent: Instant, at: Instant) -> Option<Result<String, String>> {
        prompt_step(w, slot.as_ref().unwrap(), sent, at)
    }

    #[test]
    fn a_prompt_wait_needs_the_agent_to_become_active_before_a_settled_state_counts() {
        let base = Instant::now();
        let sent = secs(base, 10);
        let mut slot = None;
        put_state(&mut slot, State::Idle, base);
        let mut w = prompt_waiter(sent, false, None);
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 1)), None, "idle from before the text is nothing");
        put_state(&mut slot, State::Working, secs(sent, 1));
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 2)), None);
        assert!(w.active);
        put_state(&mut slot, State::Idle, secs(sent, 3));
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 3)), Some(Ok("idle\n".to_string())));
    }

    #[test]
    fn a_turn_that_came_and_went_between_two_looks_is_seen_through_the_state_before() {
        let base = Instant::now();
        let sent = secs(base, 10);
        let mut slot = None;
        put_state(&mut slot, State::Idle, base);
        put_state(&mut slot, State::Working, secs(sent, 1));
        put_state(&mut slot, State::Done, secs(sent, 2));
        let mut w = prompt_waiter(sent, false, None);
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 3)), Some(Ok("done\n".to_string())));
    }

    #[test]
    fn a_settled_state_without_activity_stalls_the_prompt_after_five_seconds() {
        let base = Instant::now();
        let sent = secs(base, 10);
        let mut slot = None;
        put_state(&mut slot, State::Done, base);
        put_state(&mut slot, State::Idle, secs(sent, 1));
        let mut w = prompt_waiter(sent, false, None);
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 4)), None, "an idle after the text is not an answer");
        let stalled = step(&mut w, &slot, sent, secs(sent, 5)).unwrap().unwrap_err();
        assert!(stalled.starts_with("prompt_stalled: ") && stalled.contains("not delivered"), "{stalled}");
    }

    #[test]
    fn blocked_and_error_after_the_text_answer_at_once() {
        let base = Instant::now();
        let sent = secs(base, 10);
        for (state, word) in [(State::Blocked, "blocked\n"), (State::Error, "error\n")] {
            let mut slot = None;
            put_state(&mut slot, State::Idle, base);
            put_state(&mut slot, state, secs(sent, 1));
            let mut w = prompt_waiter(sent, false, None);
            assert_eq!(step(&mut w, &slot, sent, secs(sent, 1)), Some(Ok(word.to_string())));
        }
    }

    #[test]
    fn an_agent_that_was_working_when_the_text_came_answers_at_the_end_of_its_turn() {
        let base = Instant::now();
        let sent = secs(base, 10);
        let mut slot = None;
        put_state(&mut slot, State::Working, base);
        let mut w = prompt_waiter(sent, true, None);
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 20)), None, "still working: no stall");
        put_state(&mut slot, State::Idle, secs(sent, 21));
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 21)), Some(Ok("idle\n".to_string())));
    }

    #[test]
    fn the_one_timeout_covers_the_text_and_the_wait_and_says_delivery_is_unknown() {
        let base = Instant::now();
        let sent = secs(base, 10);
        let mut slot = None;
        put_state(&mut slot, State::Idle, base);
        put_state(&mut slot, State::Working, secs(sent, 1));
        let mut w = prompt_waiter(sent, false, Some(3));
        assert_eq!(step(&mut w, &slot, sent, secs(sent, 2)), None);
        let late = step(&mut w, &slot, sent, secs(sent, 3)).unwrap().unwrap_err();
        assert!(late.starts_with("timeout: ") && late.contains("not delivered"), "{late}");
        let mut quick = prompt_waiter(sent, false, Some(2));
        let e = step(&mut quick, &slot, sent, secs(sent, 2)).unwrap().unwrap_err();
        assert!(e.starts_with("timeout: "), "a timeout under five seconds beats the stall: {e}");
    }

    #[test]
    fn sources_and_agent_names_are_checked() {
        for ok in ["hook", "my-agent:1", "a.b_c", &"x".repeat(80)] {
            assert!(Server::check_source(ok).is_ok(), "{ok}");
        }
        for bad in ["", "imux:claude", "osc", "has space", "semi;colon", "new\nline", &"x".repeat(81)] {
            assert!(Server::check_source(bad).is_err(), "{bad:?}");
        }
        assert!(name_ok("claude-code", MAX_AGENT, b"_.+-"));
        assert!(!name_ok("a/b", MAX_AGENT, b"_.+-"));
        assert!(!name_ok(&"y".repeat(33), MAX_AGENT, b"_.+-"));
    }
}
