//! `imux agent start NAME [--kind K] [--pane PANE] [--timeout MS] [-- args...]` (P10.7): type the
//! command of an agent kind into a shell pane and hold the connection until the program says it is
//! ready.
//!
//! The command is never chosen by the caller. Without a kind it is `worktree-command`; with one it
//! is the program the user listed for it in `agent-kind` (`name=program`, whitespace between
//! entries), and that program must also be in `restore-commands`, as for a resume command. What the
//! caller adds are arguments, each quoted for the pane's shell. The pane has to be at its shell's
//! prompt with nothing running and no agent record; the caller's own pane is never the target.
//!
//! Ready is the first report of state `idle` (or `done`) on the pane, from OSC 7501 or
//! `imux pane report-agent`: imux does not read the screen to guess. A program that reports
//! `blocked` at startup ends the wait at once with `not_ready` and keeps its name; one that never
//! reports ends it at the timeout. The name waits in `pending_names` and goes to the first record
//! the pane gets (see `names.rs`), so a started agent is also named through a restore.

use std::time::{Duration, Instant};

use super::Server;
use crate::errcode::{self, tag};
use crate::log;
use crate::session::agent;
use crate::session::layout::PaneId;
use crate::target::Kind;
use crate::term::status::State;

/// How long to wait for the first report when `--timeout` is not given, and the bounds of it.
pub const TIMEOUT_DEFAULT_MS: u64 = 30_000;
pub const TIMEOUT_MIN_MS: u64 = 3_000;
pub const TIMEOUT_MAX_MS: u64 = 300_000;
/// Arguments after `--`: how many, and how many bytes in all.
const ARGS_MAX: usize = 64;
const ARGS_BYTES: usize = 8192;

/// The program the `agent-kind` table (`name=program ...`) gives `kind`.
pub fn kind_program(table: &str, kind: &str) -> Option<String> {
    table.split_whitespace().filter_map(|entry| entry.split_once('=')).find(|(name, _)| *name == kind).map(|(_, program)| program.to_string())
}

/// A pane an agent is being started in.
pub struct StartWaiter {
    client: usize,
    pane: PaneId,
    name: String,
    deadline: Instant,
    timeout_ms: u64,
}

impl Server {
    /// `api-agent-start NAME KIND PANE TIMEOUT_MS [ARG...]` (KIND and PANE may be empty).
    pub(super) fn cmd_agent_start(&mut self, client: usize, rest: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let [name, kind, spec, timeout, args @ ..] = rest else {
            return Err(tag("invalid_argument", "usage: api-agent-start name kind pane timeout_ms [arg ...]"));
        };
        let bad = |what: &str| tag("invalid_argument", what);
        if !agent::valid_name(name) {
            return Err(bad(&format!("a name is a lowercase letter, then lowercase letters, digits, _ and -, at most {} characters", agent::MAX_NAME)));
        }
        if self.pane_named(name).is_some() || self.pending_names.values().any(|n| n == name) {
            return Err(bad(&format!("the name {name} is already taken")));
        }
        let timeout_ms = match timeout.parse::<u64>().map_err(|_| bad("timeout is a number of milliseconds"))? {
            0 => TIMEOUT_DEFAULT_MS,
            ms if (TIMEOUT_MIN_MS..=TIMEOUT_MAX_MS).contains(&ms) => ms,
            _ => return Err(bad(&format!("timeout is from {TIMEOUT_MIN_MS} to {TIMEOUT_MAX_MS} milliseconds"))),
        };
        if !self.options.flag("agent-control") {
            return Err(tag("permission_denied", "agent start is off: set agent-control on to let scripts start and type into agents"));
        }
        if args.len() > ARGS_MAX || args.iter().map(String::len).sum::<usize>() > ARGS_BYTES || args.iter().any(|a| a.chars().any(char::is_control)) {
            return Err(bad(&format!("at most {ARGS_MAX} arguments and {ARGS_BYTES} bytes, with no control characters")));
        }
        let shell = self.shell_kind();
        let quoted: Vec<String> = match args.iter().map(|a| shell.quote(a)).collect::<Option<Vec<String>>>() {
            Some(q) => q,
            None => return Err(bad("the pane shell is not one imux can quote for (see shell-syntax), so no arguments can be passed")),
        };
        let program = if kind.is_empty() {
            let command = self.options.text("worktree-command").trim().to_string();
            if command.is_empty() {
                return Err(bad("no kind given and worktree-command is empty: set it, or name a kind from agent-kind"));
            }
            command
        } else {
            if !agent::valid_name(kind) {
                return Err(bad("a kind is named like an agent: lowercase letters, digits, _ and -"));
            }
            let program = kind_program(self.options.text("agent-kind"), kind).ok_or_else(|| bad(&format!("no kind {kind} in agent-kind")))?;
            let allowed: Vec<String> = self.options.text("restore-commands").split_whitespace().map(str::to_string).collect();
            agent::validate_resume(std::slice::from_ref(&program), &allowed).map_err(|e| tag("permission_denied", &format!("kind {kind}: {e}")))?;
            shell.quote(&program).ok_or_else(|| bad("the pane shell is not one imux can quote for (see shell-syntax)"))?
        };

        let place = if spec.is_empty() {
            if caller.is_some() {
                return Err(bad("from a pane, say which pane (--pane): spawn one first with split-window"));
            }
            self.default_place(None).map_err(|e| tag("not_found", &e))?
        } else {
            self.resolve(spec, Kind::Pane, caller).map_err(|e| if errcode::split(&e).0.is_some() { e } else { tag("not_found", &e) })?
        };
        if caller == Some(place.pane) {
            return Err(bad("an agent is not started in the pane that asks: start it in another pane"));
        }
        let id = place.pane;
        let shell_path = self.shell();
        let pane = self.sessions[place.si].windows[place.wi].panes.get(&id).ok_or_else(|| tag("not_found", &format!("no pane {id}")))?;
        if pane.dead.is_some() {
            return Err(tag("not_ready", &format!("pane {id} is dead")));
        }
        if pane.agent.is_some() {
            return Err(tag("not_ready", &format!("pane {id} already has an agent")));
        }
        if !super::capture::shell_is_foreground(pane, &shell_path) {
            return Err(tag("not_ready", &format!("pane {id} is not at its shell prompt: something is running in it")));
        }
        let mut line = program;
        for q in &quoted {
            line.push(' ');
            line.push_str(q);
        }
        pane.pty.write(format!("{line}\r").as_bytes()).map_err(|e| format!("agent start: {e}"))?;
        log::info(&format!("agent start: {name} ({}) typed into pane {id}", if kind.is_empty() { "worktree-command" } else { kind }));
        self.pending_names.insert(id, name.clone());
        self.started_names.insert(id);
        self.start_waiters.push(StartWaiter { client, pane: id, name: name.clone(), deadline: Instant::now() + Duration::from_millis(timeout_ms), timeout_ms });
        self.deferred = true;
        Ok(String::new())
    }

    /// Answer the starts whose pane reported, closed or ran out of time. Called once per loop turn.
    pub(super) fn check_start_waiters(&mut self) {
        if self.start_waiters.is_empty() {
            return;
        }
        let now = Instant::now();
        let waiters = std::mem::take(&mut self.start_waiters);
        let mut keep = Vec::new();
        for w in waiters {
            let record = self.sessions.iter().flat_map(|s| s.windows.iter()).find_map(|win| win.panes.get(&w.pane)).map(|p| p.agent.as_ref().map(|r| (r.state, r.msg.clone())));
            let outcome = match record {
                None => Some(Err(tag("not_found", &format!("pane {} closed", w.pane)))),
                Some(Some((state @ (State::Idle | State::Done), _))) => Some(Ok(format!("{} {}\n", w.pane, super::control::state_word(state)))),
                Some(Some((State::Blocked, msg))) => {
                    let why = msg.map(|m| format!(": {}", crate::sanitize::clean(&m))).unwrap_or_default();
                    Some(Err(tag("not_ready", &format!("the agent in pane {} is blocked at startup{why}; it keeps the name {}", w.pane, w.name))))
                }
                Some(Some((State::Error, _))) => Some(Err(tag("not_ready", &format!("the agent in pane {} reported an error at startup", w.pane)))),
                Some(_) if w.deadline <= now => {
                    if self.pending_names.get(&w.pane) == Some(&w.name) {
                        self.pending_names.remove(&w.pane);
                        self.started_names.remove(&w.pane);
                    }
                    Some(Err(tag(
                        "timeout",
                        &format!(
                            "no idle report from pane {} in {} ms: imux reads reports, never the screen, so a program that does not report its state cannot be waited for (the name {} is not kept)",
                            w.pane, w.timeout_ms, w.name
                        ),
                    )))
                }
                Some(_) => None,
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
        self.start_waiters.extend(keep);
    }

    /// The soonest timeout of a start, for the event loop's poll.
    pub(super) fn start_deadline(&self) -> Option<Instant> {
        self.start_waiters.iter().map(|w| w.deadline).min()
    }

    /// A client went away: its starts stop being waited for (the program stays, and keeps its name).
    pub(super) fn drop_start_waiters_of(&mut self, client: usize) {
        if self.start_waiters.iter().any(|w| w.client == client) {
            log::info(&format!("client {client} left while an agent was starting"));
            self.start_waiters.retain(|w| w.client != client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_gives_the_program_of_a_kind() {
        let table = "codex=codex aider=aider-chat";
        assert_eq!(kind_program(table, "codex").as_deref(), Some("codex"));
        assert_eq!(kind_program(table, "aider").as_deref(), Some("aider-chat"));
        assert_eq!(kind_program(table, "other"), None);
        assert_eq!(kind_program("", "codex"), None);
        assert_eq!(kind_program("codex", "codex"), None, "an entry with no = gives nothing");
    }

    #[test]
    fn the_first_entry_of_a_name_wins_and_a_word_without_equals_is_skipped() {
        assert_eq!(kind_program("a=x a=y", "a").as_deref(), Some("x"));
        assert_eq!(kind_program("junk a=x", "a").as_deref(), Some("x"));
        assert_eq!(kind_program("a=", "a").as_deref(), Some(""), "an empty program is refused later, by the command check");
    }
}
