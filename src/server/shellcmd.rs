//! `run-shell` and `if-shell`. The command runs under `sh -c` in a process group of its own with
//! a time limit (`run-shell-timeout`). Without `-b` the commands after it wait for it:
//!
//! - while a configuration file is read (also `source-file` at any time) the command runs right
//!   there and the reading waits, so a later line sees what an `if-shell` decided. The loop is
//!   stopped meanwhile, so the command must not ask the server for anything;
//! - from a key or the prompt the command runs on a helper thread and the rest of the line (or
//!   of the chain bound to the key) is held in `Shells.held` until the result comes back through
//!   the channel and the waker of the worktree jobs, so the loop stays free;
//! - with `-b` it runs on a helper thread and nothing waits.
//!
//! The trust line: the configuration, the key bindings and the prompt may run commands; nothing a
//! pane program prints can. The socket does not reach these commands at all (`remote.rs`). The text
//! is a format, and a variable that a program in a pane could have set (window name, title,
//! directory...) is refused in it unless it is written `#{q:name}`; the same values are handed to
//! the command as `IMUX_*` environment variables, which cannot be mistaken for code.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use super::Server;
use super::remote::Place;
use crate::config::stmt::{self, Stmt};
use crate::config::format;
use crate::input::bindings::{Command as ImuxCommand, ShellIf, ShellRun};
use crate::log;

/// Commands running at once; more are refused.
const MAX_RUNNING: usize = 8;
/// What is kept of a command's output.
const MAX_OUTPUT: usize = 64 * 1024;
/// Longest message made from a command's output.
const MESSAGE_LEN: usize = 200;

/// Process groups of the commands that run now, so the server can stop them on its way out.
static GROUPS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Stop every command that is still running.
pub(super) fn kill_all() {
    for group in GROUPS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if let Err(e) = killpg(Pid::from_raw(*group), Signal::SIGTERM) {
            log::debug(&format!("run-shell: SIGTERM to group {group}: {e}"));
        }
    }
}

/// What to do when the command is done.
enum Then {
    /// `run-shell`: show the first line of its output.
    Report,
    /// `if-shell`: run `then` when it exited with 0, else `otherwise`.
    Branch { then: String, otherwise: Option<String> },
}

pub(super) struct Outcome {
    pub pid: u32,
    /// `None` when a signal ended it.
    pub status: Option<i32>,
    pub timed_out: bool,
    /// The first line it printed, cleaned for display.
    pub line: String,
}

struct Finished {
    client: Option<usize>,
    label: String,
    limit: Duration,
    what: Then,
    result: Result<Outcome, String>,
    /// What came after the command on its line or key: it goes on now.
    rest: VecDeque<Stmt>,
}

pub(super) struct Shells {
    tx: Sender<Finished>,
    rx: Receiver<Finished>,
    running: usize,
    /// Set by `apply_queue` for the command that is about to start; that command takes it.
    held: Option<VecDeque<Stmt>>,
}

impl Default for Shells {
    fn default() -> Self {
        let (tx, rx) = channel();
        Shells { tx, rx, running: 0, held: None }
    }
}

/// A statement that makes the ones after it wait: a shell command without `-b`.
fn holds(s: &Stmt) -> bool {
    match s {
        Stmt::Run(ImuxCommand::RunShell(r)) => !r.background && !r.imux,
        Stmt::Run(ImuxCommand::IfShell(i)) => !i.background && !i.format,
        _ => false,
    }
}

/// A shell command ready to start.
pub(super) struct Job {
    pub text: String,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

/// Variables that hold nothing a program in a pane could have chosen: numbers, flags and ids.
/// Everything else (names, titles, directories, commands) is refused in shell text.
pub(super) fn pane_safe(name: &str) -> bool {
    matches!(
        name,
        "session_id"
            | "session_windows"
            | "session_attached"
            | "window_index"
            | "I"
            | "window_id"
            | "window_active"
            | "window_panes"
            | "window_zoomed_flag"
            | "window_synchronized"
            | "pane_synchronized"
            | "window_activity_flag"
            | "window_bell_flag"
            | "window_silence_flag"
            | "window_flags"
            | "pane_id"
            | "pane_index"
            | "P"
            | "pane_active"
            | "pane_width"
            | "pane_height"
            | "mouse_x"
            | "mouse_y"
            | "mouse_any_flag"
            | "pane_in_mode"
            | "pane_input_off"
            | "pane_restore_missing"
            | "pane_spawned_by"
            | "agent_name"
            | "hook_name"
            | "pane_pid"
            | "pane_dead"
            | "pane_dead_status"
            | "hook_exit_status"
            | "host"
            | "host_short"
            | "hook"
            | "hook_client"
            | "hook_session"
            | "hook_window"
            | "hook_pane"
            | "hook_state"
    )
}

/// The first line of `output` made fit to show: control characters gone, at most `MESSAGE_LEN`.
fn first_line(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().filter(|&c| !crate::sanitize::is_unsafe(c)).take(MESSAGE_LEN).collect()
}

fn set_nonblocking(fd: &impl AsRawFd) {
    // SAFETY: fcntl on a descriptor we own, only changing its O_NONBLOCK flag.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

/// Run `job` to its end or to `limit`, whichever comes first. Runs on a helper thread.
fn run(job: &Job, limit: Duration) -> Result<Outcome, String> {
    run_with(job, limit, true)
}

/// `run`; with `keep_stderr` off what the command writes to its standard error is thrown away.
pub(super) fn run_with(job: &Job, limit: Duration, keep_stderr: bool) -> Result<Outcome, String> {
    let (mut reader, writer) = std::io::pipe().map_err(|e| format!("cannot make a pipe: {e}"))?;
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(&job.text)
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(|e| format!("cannot share a pipe: {e}"))?);
    if keep_stderr {
        command.stderr(writer);
    } else {
        command.stderr(Stdio::null());
        drop(writer);
    }
    if let Some(dir) = job.cwd.as_ref().filter(|d| d.is_dir()) {
        command.current_dir(dir);
    }
    let mut child = command.spawn().map_err(|e| format!("cannot start sh: {e}"))?;
    // The pipe's write ends the command holds are the only ones left open.
    drop(command);
    set_nonblocking(&reader);
    let pid = child.id();
    let group = i32::try_from(pid).unwrap_or(i32::MAX);
    GROUPS.lock().unwrap_or_else(|e| e.into_inner()).push(group);
    let deadline = Instant::now() + limit;
    let (mut output, mut chunk) = (Vec::new(), [0u8; 8192]);
    let (mut status, mut timed_out) = (None, false);
    loop {
        let mut idle = true;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    idle = false;
                    let room = MAX_OUTPUT.saturating_sub(output.len());
                    output.extend_from_slice(&chunk[..n.min(room)]);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        match child.try_wait() {
            Ok(Some(st)) => {
                status = st.code();
                break;
            }
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                if let Err(e) = killpg(Pid::from_raw(group), Signal::SIGKILL) {
                    log::debug(&format!("run-shell: SIGKILL to group {group}: {e}"));
                }
                let _ = child.wait();
                break;
            }
            Ok(None) => {
                if idle {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(e) => {
                GROUPS.lock().unwrap_or_else(|e| e.into_inner()).retain(|g| *g != group);
                return Err(format!("cannot wait for sh: {e}"));
            }
        }
    }
    // What it printed last.
    while let Ok(n) = reader.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let room = MAX_OUTPUT.saturating_sub(output.len());
        output.extend_from_slice(&chunk[..n.min(room)]);
    }
    GROUPS.lock().unwrap_or_else(|e| e.into_inner()).retain(|g| *g != group);
    // A command that ran over its time shows nothing: the group is killed one process after
    // another, so a shell may still have run a last command (or printed half a line) in between.
    let line = if timed_out { String::new() } else { first_line(&output) };
    Ok(Outcome { pid, status, timed_out, line })
}

/// `run`, with the panic caught and one log line (command start, pid, status, duration).
fn run_logged(job: &Job, limit: Duration, label: &str) -> Result<Outcome, String> {
    let started = Instant::now();
    let result = std::panic::catch_unwind(|| run(job, limit))
        .unwrap_or_else(|p| Err(format!("run-shell failed: {}", log::panic_text(p.as_ref()))));
    if let Ok(o) = &result {
        log::info(&format!(
            "run-shell: {label:?} pid {} status {:?}{} in {} ms",
            o.pid,
            o.status,
            if o.timed_out { " (timed out)" } else { "" },
            started.elapsed().as_millis()
        ));
    }
    result
}

impl Server {
    /// Where a command typed by `client` (or run by the configuration) is about: the current pane
    /// of the client's session, else of the session used last.
    fn shell_place(&self, client: Option<usize>) -> Option<Place> {
        let si = match client {
            Some(c) => self.client_session(c),
            None => self.sessions.iter().position(|s| s.id == self.recent),
        }?;
        Some(self.current_place(si))
    }

    /// Say something about a command: on the client's message row, else in the log.
    fn shell_say(&mut self, client: Option<usize>, text: String) {
        match client.filter(|c| self.clients.contains_key(c)) {
            Some(id) => self.show_message(id, text),
            None => log::info(&format!("run-shell: {text}")),
        }
    }

    /// The text as a format with the variables of the current pane filled in (no shell check).
    fn shell_format(&self, client: Option<usize>, text: &str) -> String {
        match self.shell_place(client) {
            Some(place) => self.expand_at(place, text),
            None => format::expand(text, &|_| None),
        }
    }

    /// Text that will be parsed as imux commands: variables a pane could have set are refused
    /// unless written `#{c:...}` (a double-quoted word, which the command parser reads as one word).
    fn command_text(&self, client: Option<usize>, text: &str) -> Result<String, String> {
        let place = self.shell_place(client);
        let vars = place.map_or_else(|| self.hooks.vars.clone(), |p| self.place_vars(p));
        let lookup = |name: &str| vars.get(name).cloned().or_else(|| self.options.user(name).map(str::to_string));
        format::expand_for_shell(text, &lookup, &|n| self.shell_safe(n)).map_err(|names| {
            let list = names.iter().map(|n| format!("#{{{n}}}")).collect::<Vec<_>>().join(", ");
            let first = names.first().map_or("name", String::as_str);
            format!("{list} can be set by a program in the pane: write #{{c:{first}}}")
        })
    }

    /// Text for the shell: variables a pane could have set are refused unless written `#{q:...}`.
    /// The command also gets every variable as an `IMUX_*` environment variable.
    fn shell_job(&self, client: Option<usize>, text: &str) -> Result<Job, String> {
        let place = self.shell_place(client);
        // Without a pane to speak of (the last session just closed) a hook still has its variables.
        let vars = place.map_or_else(|| self.hooks.vars.clone(), |p| self.place_vars(p));
        let lookup = |name: &str| vars.get(name).cloned().or_else(|| self.options.user(name).map(str::to_string));
        let text = format::expand_for_shell(text, &lookup, &|n| self.shell_safe(n)).map_err(|names| {
            let list = names.iter().map(|n| format!("#{{{n}}}")).collect::<Vec<_>>().join(", ");
            let first = names.first().map_or("name", String::as_str);
            format!("{list} can be set by a program in the pane: write #{{q:{first}}} or use $IMUX_{}", first.to_uppercase())
        })?;
        Ok(self.job_with(place, &vars, text))
    }

    /// A job for `text`, which is final shell text: the environment holds every variable of `vars`
    /// as `IMUX_*`, and it runs in the directory of the pane at `place`.
    pub(super) fn job_with(&self, place: Option<Place>, vars: &std::collections::HashMap<&'static str, String>, text: String) -> Job {
        let mut env: Vec<(String, String)> = vars
            .iter()
            .filter(|(k, _)| k.len() > 1)
            .map(|(k, v)| (format!("IMUX_{}", k.to_uppercase()), v.replace('\0', "")))
            .collect();
        env.push(("IMUX_ENV".to_string(), "1".to_string()));
        env.push(("IMUX_SOCKET_PATH".to_string(), crate::paths::socket_path().to_string_lossy().into_owned()));
        if let Some(p) = place {
            env.push(("IMUX_PANE_ID".to_string(), p.pane.to_string()));
        }
        // Scripts written for tmux (plugins) call `tmux`: with `tmux-compat` it is the shim, and
        // the process is the server's child, not a pane's, so its commands have the user's power.
        if self.options.flag("tmux-compat") {
            let pane = place.map_or(0, |p| p.pane);
            env.extend(self.compat_env(pane, &[]).into_iter().filter(|(k, _)| place.is_some() || k != "TMUX_PANE"));
        }
        let si = place.map(|p| p.si);
        Job { text, env, cwd: si.and_then(|si| self.active_cwd(si)) }
    }

    /// `run-shell`.
    pub(super) fn run_shell(&mut self, client: Option<usize>, r: &ShellRun) {
        if r.imux {
            // The expanded text is run as commands, so a pane-chosen title like `x ; run-shell ...`
            // would become commands: the same rule as for shell text applies (security review F-D).
            let line = match self.command_text(client, &r.text) {
                Ok(line) => line,
                Err(e) => return self.shell_say(client, format!("run-shell -C: {e}")),
            };
            let queue = self.parse_branch(client, &line);
            return self.resume(client, queue);
        }
        match self.shell_job(client, &r.text) {
            Ok(job) => self.start_shell(client, job, Then::Report, r.background),
            Err(e) => self.shell_say(client, format!("run-shell: {e}")),
        }
    }

    /// `if-shell`.
    pub(super) fn if_shell(&mut self, client: Option<usize>, i: &ShellIf) {
        if i.format {
            let condition = self.shell_format(client, &i.condition);
            let chosen = if format::truthy(&condition) { Some(&i.then) } else { i.otherwise.as_ref() };
            if let Some(line) = chosen {
                let queue = self.parse_branch(client, line);
                self.resume(client, queue);
            }
            return;
        }
        match self.shell_job(client, &i.condition) {
            Ok(job) => self.start_shell(client, job, Then::Branch { then: i.then.clone(), otherwise: i.otherwise.clone() }, i.background),
            Err(e) => self.shell_say(client, format!("if-shell: {e}")),
        }
    }

    /// Run the command: here and now while a file is being read (unless `-b`), else on a helper
    /// thread whose result `finish_shells` picks up.
    fn start_shell(&mut self, client: Option<usize>, job: Job, what: Then, background: bool) {
        let limit = Duration::from_secs(self.options.num("run-shell-timeout"));
        let label: String = job.text.chars().filter(|c| !c.is_control()).take(80).collect();
        if !background && self.source_depth > 0 {
            let result = run_logged(&job, limit, &label);
            return self.conclude(client, &label, limit, what, result, VecDeque::new());
        }
        if self.shells.running >= MAX_RUNNING {
            return self.shell_say(client, format!("run-shell: {MAX_RUNNING} commands are running already"));
        }
        let rest = self.shells.held.take().unwrap_or_default();
        let (tx, waker) = (self.shells.tx.clone(), self.jobs.waker());
        let thread_label = label;
        let spawned = std::thread::Builder::new().name("shell".into()).spawn(move || {
            let result = run_logged(&job, limit, &thread_label);
            if tx.send(Finished { client, label: thread_label, limit, what, result, rest }).is_err() {
                log::warn("run-shell finished after the server stopped listening");
            }
            if let Err(e) = waker.wake() {
                log::error(&format!("waking the server: {e}"));
            }
        });
        match spawned {
            Ok(_) => self.shells.running += 1,
            Err(e) => self.shell_say(client, format!("run-shell: cannot start a thread: {e}")),
        }
    }

    /// Act on every command that has ended on a helper thread.
    pub(super) fn finish_shells(&mut self) {
        while let Ok(done) = self.shells.rx.try_recv() {
            self.shells.running = self.shells.running.saturating_sub(1);
            let Finished { client, label, limit, what, result, rest } = done;
            self.conclude(client, &label, limit, what, result, rest);
        }
    }

    /// What follows a command that has ended: its message, or the branch an `if-shell` chose, and
    /// then the statements that waited for it.
    fn conclude(&mut self, client: Option<usize>, label: &str, limit: Duration, what: Then, result: Result<Outcome, String>, rest: VecDeque<Stmt>) {
        let mut queue = VecDeque::new();
        match (result, what) {
            (Err(e), _) => self.shell_say(client, format!("run-shell: {e}")),
            (Ok(o), Then::Report) => {
                if o.timed_out {
                    self.shell_say(client, format!("run-shell: stopped after {} s: {label}", limit.as_secs()));
                } else if !o.line.is_empty() {
                    self.shell_say(client, o.line);
                } else if let Some(code) = o.status.filter(|c| *c != 0) {
                    self.shell_say(client, format!("'{label}' returned {code}"));
                }
            }
            (Ok(o), Then::Branch { then, otherwise }) => {
                if o.timed_out {
                    self.shell_say(client, format!("if-shell: the condition ran over {} s and was stopped: {label}", limit.as_secs()));
                } else if let Some(line) = if o.status == Some(0) { Some(then) } else { otherwise } {
                    queue = self.parse_branch(client, &line);
                }
            }
        }
        queue.extend(rest);
        self.resume(client, queue);
    }

    /// The statements of an imux command line; a mistake is reported and gives none.
    fn parse_branch(&mut self, client: Option<usize>, line: &str) -> VecDeque<Stmt> {
        match stmt::parse_line_aliased(line, &mut self.vars, &self.aliases) {
            Ok(stmts) => stmts.into(),
            Err(e) => {
                self.shell_say(client, format!("{line:?}: {e}"));
                VecDeque::new()
            }
        }
    }

    /// Run `queue` for the client, or for the configuration when there is none (any more).
    fn resume(&mut self, client: Option<usize>, queue: VecDeque<Stmt>) {
        if queue.is_empty() {
            return;
        }
        match client.filter(|c| self.clients.contains_key(c)) {
            Some(id) => {
                if let Err(e) = self.apply_queue(id, queue) {
                    self.show_message(id, e);
                }
            }
            None => {
                for s in queue {
                    if let Err(e) = self.apply(None, s) {
                        return log::warn(&format!("run-shell: {e}"));
                    }
                }
            }
        }
    }

    /// Apply statements for a client in order. A shell command without `-b` holds the ones after
    /// it back until it has ended (while a file is being read it just runs, see the top).
    pub(super) fn apply_queue(&mut self, client: usize, mut queue: VecDeque<Stmt>) -> Result<(), String> {
        while let Some(s) = queue.pop_front() {
            if self.source_depth == 0 && holds(&s) {
                self.shells.held = Some(std::mem::take(&mut queue));
            }
            let result = self.apply(Some(client), s);
            // Still there: the command failed before it started, and what waited is dropped.
            if self.shells.held.take().is_some() {
                log::debug("run-shell: the commands after a failed one are dropped");
            }
            result?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(text: &str) -> Job {
        Job { text: text.to_string(), env: vec![("IMUX_TEST_VALUE".to_string(), "a b".to_string())], cwd: None }
    }

    #[test]
    fn a_command_returns_its_first_line_and_status() {
        let o = run(&job("echo first; echo second; exit 3"), Duration::from_secs(5)).unwrap();
        assert_eq!((o.line.as_str(), o.status, o.timed_out), ("first", Some(3), false));
    }

    #[test]
    fn standard_error_counts_as_output() {
        let o = run(&job("echo oops >&2"), Duration::from_secs(5)).unwrap();
        assert_eq!(o.line, "oops");
    }

    #[test]
    fn the_environment_reaches_the_command() {
        let o = run(&job("printf '%s' \"$IMUX_TEST_VALUE\""), Duration::from_secs(5)).unwrap();
        assert_eq!(o.line, "a b");
    }

    #[test]
    fn a_command_that_runs_too_long_is_killed_with_its_children() {
        let started = Instant::now();
        let o = run(&job("sleep 30 & sleep 30; echo late"), Duration::from_millis(300)).unwrap();
        assert!(o.timed_out && o.line.is_empty());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(GROUPS.lock().unwrap().iter().all(|g| *g != i32::try_from(o.pid).unwrap()), "the group is forgotten");
    }

    #[test]
    fn a_lot_of_output_is_cut_and_does_not_block_the_command() {
        let o = run(&job("yes 0123456789 | head -c 3000000; echo done"), Duration::from_secs(20)).unwrap();
        assert!(!o.timed_out);
        assert_eq!(o.line, "0123456789");
    }

    #[test]
    fn the_message_line_has_no_control_characters_and_a_limit() {
        assert_eq!(first_line(b"\n  \x1b[31mred\x07 text\r\nnext"), "[31mred text");
        assert_eq!(first_line(&vec![b'x'; 5000]).len(), MESSAGE_LEN);
        assert_eq!(first_line(b""), "");
    }

    #[test]
    fn what_a_pane_can_set_is_not_safe_in_shell_text() {
        for name in ["window_name", "W", "session_name", "S", "pane_title", "pane_current_path", "pane_current_command", "hook_window_name", "nonsense"] {
            assert!(!pane_safe(name), "{name}");
        }
        for name in ["pane_id", "window_index", "pane_width", "window_active"] {
            assert!(pane_safe(name), "{name}");
        }
    }

    #[test]
    fn worktree_variables_come_from_git_refs_and_directories_so_they_are_not_safe_in_shell_text() {
        for name in ["worktree_name", "worktree_branch", "worktree_path", "worktree_repo"] {
            assert!(!pane_safe(name), "{name}");
        }
    }
}
