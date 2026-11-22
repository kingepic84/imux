mod checkconfig;
mod doctor;
pub mod eventwait;
mod probe;
pub mod tmux;

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use mio::unix::SourceFd;
use mio::{Events, Interest, Poll, Token};
use nix::sys::signal::Signal;

use crate::log;
use crate::paths;
use crate::proto::{self, Caps, ClientMsg, FrameBuf, ImageMode, PersistOp, ServerMsg, Size, Want, WorktreeArgs};
use crate::tty::pty::{set_nonblocking, window_size};
use crate::tty::raw::RawMode;
use crate::tty::signal;
use crate::tty::{ctx, read_some, write_all};

const TTY_IN: Token = Token(0);
const SOCK: Token = Token(1);
const SIGNALS: Token = Token(2);

/// Undo whatever modes the pane's program left enabled on the outer terminal.
const TERMINAL_RESET: &[u8] =
    b"\x1b[?2026l\x1b[?7h\x1b[?1l\x1b>\x1b[0 q\x1b[?1049l\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1004l\x1b[0m\r\n";

/// The client owns the whole outer screen: alternate screen, autowrap off so painting the last
/// column never scrolls.
const TERMINAL_SETUP: &[u8] = b"\x1b[?1049h\x1b[?7l\x1b[2J";

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    NewOrAttach,
    Attach,
}

enum Outcome {
    Exited(i32),
    Detached,
    ServerGone,
    ServerError(String),
}

/// Open the controlling tty by its real device path: kqueue rejects the /dev/tty alias, and a
/// fresh open gives its own file description, so O_NONBLOCK does not leak to the parent shell.
fn open_tty(write: bool) -> io::Result<File> {
    let path = nix::unistd::ttyname(io::stdin()).map_err(|e| ctx(e, "ttyname(stdin)"))?;
    OpenOptions::new()
        .read(!write)
        .write(write)
        .open(&path)
        .map_err(|e| ctx(e, &format!("open {}", path.display())))
}

fn start_server() -> io::Result<()> {
    paths::ensure_socket_dir()?;
    let exe = std::env::current_exe().map_err(|e| ctx(e, "current_exe"))?;
    let mut cmd = Command::new(exe);
    cmd.arg("server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and runs in the forked child before exec.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid().map(|_| ()).map_err(io::Error::from)
        });
    }
    cmd.spawn().map_err(|e| ctx(e, "start server"))?;
    Ok(())
}

/// Connect, and make sure the other end is ours: the server checks who connects, so the client
/// checks who it talks to (a directory under `/tmp` that somebody else made could hold a socket
/// that answers the protocol and reads every key; security review F-E).
fn dial(sock: &std::path::Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(sock)?;
    match crate::tty::proc::peer_uid(stream.as_fd()) {
        Some(uid) if uid == nix::unistd::getuid().as_raw() => Ok(stream),
        _ => Err(io::Error::new(io::ErrorKind::PermissionDenied, "the socket belongs to another user")),
    }
}

/// Connect and tell the server which protocol this imux speaks, before anything else.
fn connect(mode: Mode) -> io::Result<UnixStream> {
    let stream = connect_raw(mode)?;
    send(&stream, &ClientMsg::Version(proto::version()))?;
    Ok(stream)
}

/// What the client says when the server hangs up on it before it has heard a single answer: an
/// older server does that to the `Version` it does not know.
const OLDER_SERVER: &str = "the server closed the connection without an answer; if it is an older imux, run `imux kill-server` (the sessions end) and start imux again";

/// `connect` without the version: `kill-server` has to work against a server of any version.
fn connect_raw(mode: Mode) -> io::Result<UnixStream> {
    let sock = paths::socket_path();
    paths::check_socket_dir()?;
    match dial(&sock) {
        Ok(s) => return Ok(s),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            if mode == Mode::Attach {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no server running"));
            }
        }
        Err(e) => return Err(ctx(e, &format!("connect {}", sock.display()))),
    }
    log::info("starting server");
    start_server()?;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(30));
        match dial(&sock) {
            Ok(s) => return Ok(s),
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {}
            Err(e) => return Err(ctx(e, &format!("connect {}", sock.display()))),
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, format!("server did not come up (see {} in the socket dir)", paths::log_file_name())))
}

pub fn kill_server() -> io::Result<()> {
    let mut stream = connect_raw(Mode::Attach)?;
    let mut wire = Vec::new();
    proto::frame(&ClientMsg::Kill, &mut wire);
    write_all(stream.as_fd(), &wire)?;
    // Stay until the server has dealt with it: it refuses a plugin that has no right to do this, and
    // it can only tell who asked while the asking process is still there. The server closes the
    // connection when it goes (or answers an error), so this ends at once.
    stream.set_read_timeout(Some(Duration::from_secs(3))).map_err(|e| ctx(e, "set timeout"))?;
    let mut frames = FrameBuf::default();
    let mut buf = vec![0u8; 4096];
    loop {
        match io::Read::read(&mut stream, &mut buf) {
            Ok(0) | Err(_) => return Ok(()),
            Ok(n) => {
                frames.push(&buf[..n]);
                if let Some(ServerMsg::Error(msg)) = frames.next::<ServerMsg>()? {
                    return Err(io::Error::other(msg));
                }
            }
        }
    }
}

/// Leave the terminal usable after a panic: the same reset a normal exit sends. The terminal
/// modes themselves are restored when the unwinding drops `RawMode`.
pub fn emergency_reset() {
    if let Ok(tty) = open_tty(true)
        && let Err(e) = write_all(tty.as_fd(), TERMINAL_RESET)
    {
        log::warn(&format!("reset terminal after panic: {e}"));
    }
}

/// `imux check-config [--summary] [FILE ...]`: what imux does not take in these configuration
/// files (or the one it would read); changes nothing, starts nothing.
pub fn check_config(args: &[String]) -> io::Result<i32> {
    checkconfig::run(args)
}

/// `imux doctor`: report what the terminal, the configuration and the server say; changes nothing.
pub fn doctor() -> io::Result<i32> {
    doctor::run()
}

/// `imux ls`: print the sessions and return, without touching the terminal modes.
pub fn list() -> io::Result<i32> {
    request(&ClientMsg::List)
}

/// `imux save|restore|snapshots|delete-snapshot`: ask the server and print its answer.
pub fn persist(op: PersistOp) -> io::Result<i32> {
    request(&ClientMsg::Persist(op))
}

/// `imux snapshots --history`: the rolling copies of the autosave, newest first, read from the state
/// directory (the server need not run). `imux restore history/NAME` brings one back.
pub fn history_list() -> io::Result<i32> {
    let store = crate::persist::Store::from_env()?;
    let copies = store.list_history()?;
    let mut out = io::stdout().lock();
    if copies.is_empty() {
        writeln!(out, "no rolling copies")?;
        return Ok(0);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    for (name, snap) in copies {
        let age = now.saturating_sub(name.parse().unwrap_or(now));
        let ago = match age {
            0..=119 => format!("{age}s"),
            120..=7199 => format!("{}m", age / 60),
            7200..=172_799 => format!("{}h", age / 3600),
            _ => format!("{}d", age / 86_400),
        };
        let windows: usize = snap.sessions.iter().map(|s| s.windows.len()).sum();
        let names: Vec<String> = snap.sessions.iter().map(|s| crate::sanitize::clean(&s.name)).collect();
        writeln!(out, "history/{name}  {ago} ago  {} session(s), {windows} window(s): {}", snap.sessions.len(), names.join(" "))?;
    }
    Ok(0)
}

/// `imux worktree ...`: the server opens the window in the session used last.
pub fn worktree(mut args: WorktreeArgs) -> io::Result<i32> {
    args.from = std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned());
    request(&ClientMsg::Worktree(args))
}

/// `imux worktree-remove ...`: the server removes the worktree and closes its window.
pub fn worktree_remove(mut args: proto::WorktreeRemoveArgs) -> io::Result<i32> {
    args.from = std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned());
    request(&ClientMsg::WorktreeRemove(args))
}

/// What `imux attach` and `imux new` were given, after their flags are read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: Option<String>,
    /// `-d` of attach, `-D` of new: detach the other clients of the session first.
    pub detach_others: bool,
    /// `-A` of new: attach when the named session exists instead of failing.
    pub attach_existing: bool,
    /// `-r` of attach: a client that only watches.
    pub read_only: bool,
}

/// Read the words after `imux attach` (`-t name`, `-d`, `-r`, `-x`) or `imux new` (`-s name`, `-A`, `-D`).
pub fn entry_args(creating: bool, words: &[String]) -> Result<Entry, String> {
    let cmd = if creating { crate::cli::new_command() } else { crate::cli::attach_command() };
    Ok(crate::cli::entry_from(&crate::cli::parse_leaf(cmd, words)?, creating))
}

/// `imux attach [-t name] [-d] [-r] [-x]`.
pub fn attach(entry: Entry) -> io::Result<i32> {
    if entry.detach_others {
        detach_clients_of(entry.name.as_deref().unwrap_or(""));
    }
    run_as(Mode::Attach, entry.name.map(Want::Attach), entry.read_only)
}

/// `imux new [-s name] [-A] [-D]`: with `-A` and a session of that name, attach to it.
pub fn create(entry: Entry) -> io::Result<i32> {
    if entry.attach_existing
        && let Some(name) = &entry.name
        && session_exists(name)
    {
        return attach(Entry { name: entry.name, detach_others: entry.detach_others, ..Entry::default() });
    }
    run(Mode::NewOrAttach, Some(Want::Create(entry.name)))
}

/// Whether the running server (if any) has a session with exactly this name.
fn session_exists(name: &str) -> bool {
    let words = vec!["has-session".to_string(), "-t".to_string(), format!("={name}")];
    request_with(Mode::Attach, &ClientMsg::Run(proto::RunArgs { words, cwd: None })).is_ok()
}

/// Detach every client of the session (the most recent one when `session` is empty): `attach -d`.
/// A server that is not there, or a session that is not, is left for the attach to report.
fn detach_clients_of(session: &str) {
    let words = vec!["detach-client".to_string(), "-s".to_string(), session.to_string()];
    if let Err(e) = request_with(Mode::Attach, &ClientMsg::Run(proto::RunArgs { words, cwd: None })) {
        log::info(&format!("detach other clients: {e}"));
    }
}

/// Whether a server is listening on the socket.
pub fn server_is_up() -> bool {
    std::os::unix::net::UnixStream::connect(crate::paths::socket_path()).is_ok()
}

/// `imux <command> ...`: the server runs the command (see the COMMANDS in the manual) and the
/// answer is printed.
pub fn command(words: Vec<String>) -> io::Result<i32> {
    let cwd = std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned());
    // Creating a session starts the server when none runs, as a script expects.
    let mode = if matches!(words.first().map(String::as_str), Some("new-session" | "new")) { Mode::NewOrAttach } else { Mode::Attach };
    request_with(mode, &ClientMsg::Run(proto::RunArgs { words, cwd }))
}

/// `imux agent list|get|wait` and `imux pane report-agent|release-agent`: one request, the answer
/// on stdout.
pub fn ask(msg: ClientMsg) -> io::Result<i32> {
    request(&msg)
}

/// `imux events`: print each event the server sends, one line of JSON each, until the server goes
/// away (0), the reader of our output does (0) or the server says events were lost (1).
///
/// With `plan.since` the kept events after that sequence number come first. With `plan.wait` only
/// the first event that has the name and the fields is printed (exit 0); none within the time
/// is an error, `timeout`.
pub fn events(plan: crate::cli::EventsPlan) -> io::Result<i32> {
    let mut types = plan.types;
    if let Some(wait) = &plan.wait {
        types = vec![wait.event.clone()];
    }
    if let Some(n) = plan.since {
        types.push(format!("{}{n}", crate::server::SINCE));
    }
    let deadline = plan.wait.as_ref().and_then(|w| w.timeout_ms).map(|ms| std::time::Instant::now() + Duration::from_millis(ms));
    let mut stream = connect(Mode::Attach)?;
    send(&stream, &ClientMsg::Subscribe(types))?;
    let mut frames = FrameBuf::default();
    let mut buf = vec![0u8; 8192];
    let stdout = io::stdout();
    loop {
        if let Some(d) = deadline {
            let left = d.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(io::Error::other(crate::errcode::tag("timeout", "no such event in time")));
            }
            stream.set_read_timeout(Some(left)).map_err(|e| ctx(e, "set the read timeout"))?;
        }
        let n = match io::Read::read(&mut stream, &mut buf) {
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(ctx(e, "read events")),
        };
        if n == 0 {
            return Ok(0);
        }
        frames.push(&buf[..n]);
        while let Some(msg) = frames.next::<ServerMsg>()? {
            match msg {
                ServerMsg::Event(line) => {
                    let lost = line.starts_with("{\"event\":\"events_lost\"");
                    if let Some(wait) = &plan.wait
                        && !lost
                    {
                        if eventwait::matches(&line, &wait.event, &wait.matches) {
                            return print_line(&stdout, &line);
                        }
                        continue;
                    }
                    let mut out = stdout.lock();
                    match writeln!(out, "{line}").and_then(|()| out.flush()) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(0),
                        Err(e) => return Err(ctx(e, "write event")),
                    }
                    if line.starts_with("{\"event\":\"events_lost\"") {
                        return Ok(1);
                    }
                }
                ServerMsg::Error(msg) => return Err(io::Error::other(msg)),
                _ => {}
            }
        }
    }
}

/// Print one event line; a reader that has gone away is not an error.
fn print_line(stdout: &io::Stdout, line: &str) -> io::Result<i32> {
    let mut out = stdout.lock();
    match writeln!(out, "{line}").and_then(|()| out.flush()) {
        Ok(()) => Ok(0),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        Err(e) => Err(ctx(e, "write event")),
    }
}

fn request(msg: &ClientMsg) -> io::Result<i32> {
    request_with(Mode::Attach, msg)
}

fn request_with(mode: Mode, msg: &ClientMsg) -> io::Result<i32> {
    let text = request_text(mode, msg)?;
    io::stdout().write_all(text.as_bytes()).map_err(|e| ctx(e, "write session list"))?;
    Ok(0)
}

/// One request to the running server and the text it answers with, for the commands that print
/// JSON themselves; an error answer is an error whose text may start with an error code.
pub fn ask_text(msg: ClientMsg) -> io::Result<String> {
    request_text(Mode::Attach, &msg)
}

/// One request and the text the server answers with; an error answer is an error.
fn request_text(mode: Mode, msg: &ClientMsg) -> io::Result<String> {
    let mut stream = connect(mode)?;
    match send(&stream, msg) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Err(io::Error::other(OLDER_SERVER)),
        other => other?,
    }
    let mut frames = FrameBuf::default();
    let mut buf = vec![0u8; 4096];
    let mut heard = false;
    loop {
        let n = io::Read::read(&mut stream, &mut buf).map_err(|e| ctx(e, "read session list"))?;
        if n == 0 {
            return Err(io::Error::other(if heard { "server closed the connection" } else { OLDER_SERVER }));
        }
        heard = true;
        frames.push(&buf[..n]);
        match frames.next::<ServerMsg>()? {
            Some(ServerMsg::Text(text)) => return Ok(text),
            Some(ServerMsg::Error(msg)) => return Err(io::Error::other(msg)),
            _ => {}
        }
    }
}

/// `imux worktree list [repo]`: the worktrees of the repository (the one the current directory is
/// in, or `repo`) with their branch and the windows of a running server that show each.
pub fn worktree_list(args: &[String]) -> io::Result<i32> {
    let (repo, from) = match args {
        [] => (None, std::env::current_dir().map_err(|e| ctx(e, "current directory"))?),
        [one] => (Some(std::path::PathBuf::from(one)), std::env::current_dir().map_err(|e| ctx(e, "current directory"))?),
        _ => return Err(io::Error::other("usage: worktree list [repo]")),
    };
    let entries = crate::worktree::list(repo.as_deref(), &from).map_err(io::Error::other)?;
    let windows = if server_is_up() {
        let words = ["list-windows", "-a", "-F", "#{worktree_path}\t#{session_name}:#{window_index}"].map(String::from).to_vec();
        request_text(Mode::Attach, &ClientMsg::Run(proto::RunArgs { words, cwd: None })).unwrap_or_default()
    } else {
        String::new()
    };
    let mut out = io::stdout().lock();
    for e in entries {
        let shown: Vec<&str> = windows
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .filter(|(p, _)| !p.is_empty() && crate::worktree::same_path(std::path::Path::new(p), &e.path))
            .map(|(_, w)| w)
            .collect();
        let mut notes = Vec::new();
        if e.main {
            notes.push("main");
        }
        if e.locked {
            notes.push("locked");
        }
        let branch = e.branch.as_deref().map_or_else(|| "(detached)".to_string(), crate::sanitize::clean);
        let place = if shown.is_empty() { "-".to_string() } else { shown.join(",") };
        let line = format!("{}\t{branch}\t{place}\t{}", crate::sanitize::clean(&e.path.to_string_lossy()), notes.join(","));
        writeln!(out, "{line}").map_err(|e| ctx(e, "write worktree list"))?;
    }
    Ok(0)
}

/// Attach to the session `want` names, or the most recent one when it is `None`.
pub fn run(mode: Mode, want: Option<Want>) -> io::Result<i32> {
    run_as(mode, want, false)
}

/// `run`, as a client that only watches when `read_only` is set.
fn run_as(mode: Mode, want: Option<Want>, read_only: bool) -> io::Result<i32> {
    let tty_in = open_tty(false)?;
    let tty_out = open_tty(true)?;
    let stream = connect(mode)?;
    stream.set_nonblocking(true).map_err(|e| ctx(e, "socket nonblocking"))?;
    set_nonblocking(tty_in.as_fd())?;

    let signals = signal::install(&[Signal::SIGWINCH])?;
    let raw = RawMode::enter(tty_out.as_fd())?;
    if let Err(e) = write_all(raw.fd(), TERMINAL_SETUP) {
        log::warn(&format!("set up terminal: {e}"));
    }
    let env_caps = detect_caps(|k| std::env::var(k).ok());
    let options = crate::config::client_options();
    let env: Vec<(String, String)> =
        options.text("update-environment").split_whitespace().filter_map(|n| Some((n.to_string(), std::env::var(n).ok()?))).collect();
    let patience = Duration::from_millis(options.num("image-probe-timeout"));
    let probed = if patience.is_zero() || !probe::should_ask(options.text("image-probe"), |k| std::env::var(k).ok()) {
        log::info("terminal probe skipped (image-probe, or image-probe-timeout 0), using the environment");
        Ok((probe::Probe::default(), Vec::new()))
    } else {
        probe::run(&tty_in, &tty_out, patience)
    };
    let mut accepted = 7;
    let mut kitty = env_caps.images == ImageMode::Kitty;
    let (caps, typed) = match probed {
        Ok((found, typed)) => {
            let caps = probe::merge_preferring(env_caps, &found, options.text("image-prefer"));
            accepted = probe::accepted_modes(env_caps, &found);
            kitty = found.kitty || caps.images == ImageMode::Kitty;
            log::info(&format!(
                "terminal probe: name={:?} kitty={} sixel={} answered={}; images {:?} (environment said {:?})",
                found.name, found.kitty, found.sixel, found.done, caps.images, env_caps.images
            ));
            (caps, typed)
        }
        Err(e) => {
            log::warn(&format!("terminal probe failed, using the environment: {e}"));
            (env_caps, Vec::new())
        }
    };
    let outcome = event_loop(&tty_in, &tty_out, &raw, &stream, &signals, Start { want, caps, accepted, kitty, typed, env, read_only });

    // Pictures stored on a Kitty terminal under ids outlive the program that sent them.
    if kitty
        && let Err(e) = write_all(raw.fd(), crate::render::kitty::DELETE_ALL.as_bytes())
    {
        log::warn(&format!("delete stored pictures: {e}"));
    }
    if let Err(e) = write_all(raw.fd(), TERMINAL_RESET) {
        log::warn(&format!("reset terminal: {e}"));
    }
    drop(raw);
    let mut out = io::stdout();
    let code = match outcome? {
        Outcome::Exited(code) => code,
        Outcome::Detached => {
            let _ = writeln!(out, "[detached]");
            0
        }
        Outcome::ServerGone => {
            let _ = writeln!(out, "[server exited]");
            1
        }
        Outcome::ServerError(msg) => {
            let _ = writeln!(out, "[error: {msg}]");
            1
        }
    };
    Ok(code)
}

/// `IMUX_RECORD_OUTPUT=PATH`: every byte this client writes to its terminal for the server is also
/// appended to `PATH.<pid>` (mode 0600, links not followed). For finding out what a terminal is
/// sent when a picture flickers: `cat` the file in that terminal to see the same thing again.
fn open_recording() -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let base = std::env::var_os("IMUX_RECORD_OUTPUT")?;
    let mut path = base.clone();
    path.push(format!(".{}", std::process::id()));
    match OpenOptions::new().create_new(true).append(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&path) {
        Ok(f) => {
            log::info(&format!("recording the terminal output to {}", path.to_string_lossy()));
            Some(f)
        }
        Err(e) => {
            log::error(&format!("cannot record the terminal output to {}: {e}", path.to_string_lossy()));
            None
        }
    }
}

/// Append to the recording; one failure (disk full) ends it, the session goes on.
fn record(file: &mut Option<File>, data: &[u8]) {
    if let Some(f) = file
        && let Err(e) = f.write_all(data)
    {
        log::error(&format!("recording of the terminal output stopped: {e}"));
        *file = None;
    }
}

fn current_size(tty_out: &File) -> io::Result<Size> {
    Ok(Size::from_winsize(&window_size(tty_out.as_fd())?))
}

fn send(stream: &UnixStream, msg: &ClientMsg) -> io::Result<()> {
    let mut wire = Vec::new();
    proto::frame(msg, &mut wire);
    write_all(stream.as_fd(), &wire)
}

/// What the outer terminal can do, judged from its environment (`env` looks a variable up).
/// Images use the best protocol known to work: iTerm2 (`TERM_PROGRAM`, or `LC_TERMINAL` which
/// survives ssh), then Kitty graphics, then Sixel, else plain blocks.
fn detect_caps(env: impl Fn(&str) -> Option<String>) -> Caps {
    let get = |name: &str| env(name).unwrap_or_default();
    let is = |name: &str, want: &str| get(name) == want;
    let (term, program) = (get("TERM"), get("TERM_PROGRAM"));
    let iterm = program == "iTerm.app" || is("LC_TERMINAL", "iTerm2") || program == "WezTerm";
    let kitty = term == "xterm-kitty" || !get("KITTY_WINDOW_ID").is_empty() || program == "ghostty" || term == "xterm-ghostty";
    let sixel = term.starts_with("foot") || term.starts_with("mlterm") || term.contains("sixel") || program == "contour";
    let images = if iterm {
        ImageMode::Iterm2
    } else if kitty {
        ImageMode::Kitty
    } else if sixel {
        ImageMode::Sixel
    } else {
        ImageMode::Blocks
    };
    let colorterm = is("COLORTERM", "truecolor") || is("COLORTERM", "24bit");
    Caps { images, truecolor: images != ImageMode::Blocks || colorterm }
}

/// `suspend-client`: hand the terminal back, stop the way a job does on Ctrl-Z, and when the
/// shell continues the job take the terminal again and ask the server for a full repaint.
fn suspend(tty_out: &File, raw: &RawMode<BorrowedFd<'_>>, stream: &UnixStream) -> io::Result<()> {
    write_all(raw.fd(), TERMINAL_RESET)?;
    raw.pause()?;
    // SIGSTOP, not SIGTSTP: the kernel discards a SIGTSTP in an orphaned process group (a client
    // started by a script or under `script`), and to a shell both look like a stopped job.
    if let Err(e) = nix::sys::signal::raise(Signal::SIGSTOP) {
        log::warn(&format!("stop the client: {e}"));
    }
    raw.resume()?;
    write_all(raw.fd(), TERMINAL_SETUP)?;
    send(stream, &ClientMsg::Resize(current_size(tty_out)?))
}

/// What the client sends first: the request, what the terminal can do, and keys typed during the probe.
struct Start {
    want: Option<Want>,
    caps: Caps,
    /// The picture protocols the terminal accepts (`ClientMsg::Modes`).
    accepted: u8,
    /// The terminal speaks the Kitty graphics protocol (so its answers are not for the pane, and
    /// pictures stored on it are deleted at the end).
    kitty: bool,
    typed: Vec<u8>,
    /// The variables `update-environment` names that this environment has.
    env: Vec<(String, String)>,
    /// `attach -r`: tell the server (`ClientMsg::ReadOnly`) before the hello.
    read_only: bool,
}

fn event_loop(
    tty_in: &File,
    tty_out: &File,
    raw: &RawMode<BorrowedFd<'_>>,
    stream: &UnixStream,
    signals: &signal::SignalPipe,
    start: Start,
) -> io::Result<Outcome> {
    let Start { want, caps, accepted, kitty, typed, env, read_only } = start;
    let mut poll = Poll::new().map_err(|e| ctx(e, "mio poll"))?;
    let reg = poll.registry();
    let reg_err = |e| ctx(e, "mio register");
    reg.register(&mut SourceFd(&tty_in.as_raw_fd()), TTY_IN, Interest::READABLE).map_err(reg_err)?;
    reg.register(&mut SourceFd(&stream.as_raw_fd()), SOCK, Interest::READABLE).map_err(reg_err)?;
    reg.register(&mut SourceFd(&signals.read.as_raw_fd()), SIGNALS, Interest::READABLE).map_err(reg_err)?;

    let hello = current_size(tty_out)?;
    let first = || -> io::Result<()> {
        if let Some(want) = want {
            send(stream, &ClientMsg::Want(want))?;
        }
        send(stream, &ClientMsg::Caps(caps))?;
        send(stream, &ClientMsg::Modes(accepted))?;
        send(stream, &ClientMsg::Env(env))?;
        if read_only {
            send(stream, &ClientMsg::ReadOnly)?;
        }
        send(stream, &ClientMsg::Hello(hello))?;
        if !typed.is_empty() {
            send(stream, &ClientMsg::Input(typed))?;
        }
        Ok(())
    };
    match first() {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(Outcome::ServerError(OLDER_SERVER.to_string())),
        other => other?,
    }
    let mut heard = false;

    let mut events = Events::with_capacity(16);
    let mut buf = vec![0u8; 64 * 1024];
    let mut frames = FrameBuf::default();
    let mut recording = open_recording();
    let mut replies = probe::ReplyFilter::default();
    loop {
        match poll.poll(&mut events, None) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ctx(e, "mio poll")),
        }
        for event in events.iter() {
            match event.token() {
                TTY_IN => {
                    while let Some(n) = read_some(tty_in.as_fd(), &mut buf)? {
                        if n == 0 {
                            return Err(io::Error::other("tty input closed"));
                        }
                        let typed = if kitty { replies.feed(&buf[..n]) } else { buf[..n].to_vec() };
                        if !typed.is_empty() {
                            send(stream, &ClientMsg::Input(typed))?;
                        }
                    }
                }
                SOCK => {
                    // The server closes right after a goodbye (`Detached`, `Exit`): act on what
                    // arrived before the end of the stream, not on the end alone.
                    let mut closed = false;
                    loop {
                        match read_some(stream.as_fd(), &mut buf)? {
                            Some(0) => {
                                closed = true;
                                break;
                            }
                            Some(n) => frames.push(&buf[..n]),
                            None => break,
                        }
                    }
                    while let Some(msg) = frames.next::<ServerMsg>()? {
                        heard = true;
                        match msg {
                            ServerMsg::Output(data) => {
                                record(&mut recording, &data);
                                write_all(tty_out.as_fd(), &data)?
                            }
                            ServerMsg::Exit(code) => return Ok(Outcome::Exited(code)),
                            ServerMsg::Detached => return Ok(Outcome::Detached),
                            ServerMsg::Error(msg) => return Ok(Outcome::ServerError(msg)),
                            ServerMsg::Text(_) | ServerMsg::Event(_) => {}
                            ServerMsg::Suspend => suspend(tty_out, raw, stream)?,
                        }
                    }
                    if closed {
                        return Ok(if heard { Outcome::ServerGone } else { Outcome::ServerError(OLDER_SERVER.to_string()) });
                    }
                }
                SIGNALS => {
                    for sig in signals.drain()? {
                        if sig == Signal::SIGWINCH {
                            send(stream, &ClientMsg::Resize(current_size(tty_out)?))?;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(vars: &[(&str, &str)]) -> Caps {
        detect_caps(|k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()))
    }

    #[test]
    fn iterm2_is_found_locally_and_over_ssh() {
        let iterm = Caps { images: ImageMode::Iterm2, truecolor: true };
        assert_eq!(caps(&[("TERM_PROGRAM", "iTerm.app")]), iterm);
        assert_eq!(caps(&[("LC_TERMINAL", "iTerm2")]), iterm);
    }

    #[test]
    fn terminal_app_and_unknown_terminals_get_blocks_and_256_colors() {
        assert_eq!(caps(&[("TERM_PROGRAM", "Apple_Terminal")]), Caps::BASIC);
        assert_eq!(caps(&[]), Caps::BASIC);
    }

    #[test]
    fn the_best_known_protocol_wins() {
        let images = |v: &[(&str, &str)]| caps(v).images;
        assert_eq!(images(&[("TERM", "xterm-kitty")]), ImageMode::Kitty);
        assert_eq!(images(&[("KITTY_WINDOW_ID", "3")]), ImageMode::Kitty);
        assert_eq!(images(&[("TERM_PROGRAM", "ghostty")]), ImageMode::Kitty);
        assert_eq!(images(&[("TERM", "foot")]), ImageMode::Sixel);
        assert_eq!(images(&[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]), ImageMode::Blocks);
        assert_eq!(images(&[("TERM_PROGRAM", "iTerm.app"), ("TERM", "xterm-kitty")]), ImageMode::Iterm2);
        assert_eq!(images(&[("TERM", "xterm-kitty"), ("TERM_PROGRAM", "x")]), ImageMode::Kitty);
        assert!(caps(&[("TERM", "foot")]).truecolor);
    }

    #[test]
    fn the_words_of_attach_and_new_are_read() {
        let words = |w: &[&str]| w.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let named = |n: &str, detach_others, attach_existing| Entry { name: Some(n.to_string()), detach_others, attach_existing, ..Entry::default() };
        assert_eq!(entry_args(false, &words(&["-t", "w", "-d"])), Ok(named("w", true, false)));
        assert_eq!(entry_args(false, &words(&[])), Ok(Entry::default()));
        assert_eq!(entry_args(true, &words(&["-A", "-D", "-s", "w"])), Ok(named("w", true, true)));
        assert_eq!(entry_args(true, &words(&["-s", "w"])), Ok(named("w", false, false)));
        assert!(entry_args(false, &words(&["-A"])).is_err(), "-A belongs to new");
        assert!(entry_args(true, &words(&["-d"])).is_err(), "new -d is the new-session command");
        assert!(entry_args(false, &words(&["-t"])).is_err());
        assert!(entry_args(false, &words(&["-t", "a", "-t", "b"])).is_err());
    }

    #[test]
    fn attach_reads_the_read_only_and_ignored_flags() {
        let words = |w: &[&str]| w.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let e = entry_args(false, &words(&["-r", "-t", "w", "-d"])).unwrap();
        assert!(e.read_only && e.detach_others && e.name.as_deref() == Some("w"));
        assert!(!entry_args(false, &words(&["-t", "w"])).unwrap().read_only);
        assert_eq!(entry_args(false, &words(&["-x"])), Ok(Entry::default()), "-x is accepted and has no effect");
        assert!(entry_args(true, &words(&["-r"])).is_err(), "-r belongs to attach");
        assert!(entry_args(true, &words(&["-x"])).is_err(), "-x belongs to attach");
    }

    #[test]
    fn colorterm_alone_turns_on_truecolor() {
        assert_eq!(caps(&[("COLORTERM", "truecolor")]), Caps { images: ImageMode::Blocks, truecolor: true });
        assert!(caps(&[("COLORTERM", "24bit")]).truecolor);
        assert!(!caps(&[("COLORTERM", "yes")]).truecolor);
    }
}
