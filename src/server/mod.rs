mod agents;
mod agentstart;
mod alerts;
mod animate;
mod arrange;
mod buffers;
mod capture;
mod channels;
mod clock;
mod conf;
mod control;
mod copy;
mod create;
mod dead;
mod environ;
mod events;
mod focus;
mod hooks;
mod mark;
mod menu;
mod mousekey;
mod names;
mod outwait;
mod modes;
mod pipes;
mod plugins;
mod popup;
mod position;
mod regions;
mod remote;
pub use events::SINCE;
pub use remote::{alias_shaped, bound_flags_fit, knows as knows_command, knows_exact, runs_from_a_key};

/// The names `imux <command>` runs on the server, for the table of what a plugin may do.
#[cfg(test)]
pub fn remote_command_names() -> &'static [&'static str] {
    remote::names()
}

/// The events imux fires, for the shell completions.
pub fn hook_names() -> &'static [&'static str] {
    hooks::ACTIVE
}

/// Whether every name is an event that imux fires (or `after-` and a command), for a manifest.
pub fn check_event_types(types: &[String]) -> Result<(), String> {
    events::check_types(types).map(|_| ())
}
mod restore;
mod saving;
mod search;
mod segments;
mod shellcmd;
mod spawned;
mod winopts;
mod worktree;

use conf::status_with;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mio::net::{UnixListener, UnixStream};
use mio::unix::SourceFd;
use mio::{Events, Interest, Poll, Registry, Token};
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use crate::config::lex::Vars;
use crate::config::options::Options;
use crate::copymode;
use crate::input::bindings::{Bindings, Command};
use crate::input::keys::{self, Parsed};
use crate::log::{self, Level};
use crate::paths;
use crate::persist::{Store, writer};
use crate::proto::{self, ClientMsg, FrameBuf, ImageMode, ServerMsg, Size, Want};
use crate::render::status;
use crate::render::{self, PaneView, Renderer, overlay};
use modes::{Mode, PromptKind};
use crate::session::layout::{PaneId, Preset, Rect, Separator, Split};
use crate::session::agent;
use crate::session::{Pane, Session, Window};
use crate::shell::{self, ShellKind};
use crate::term::Terminal;
use crate::term::grid::{Event, MouseMode};
use crate::tty::signal::{self, SignalPipe};
use crate::tty::{ctx, proc, pty, read_some};

/// Longest terminal title (`set-titles`) sent to a client, in characters.
const MAX_TERMINAL_TITLE: usize = 256;
const LISTENER: Token = Token(0);
const SIGNALS: Token = Token(2);
const CLIENT_BASE: usize = 16;
const PANE_BASE: usize = 1 << 24;
const MAX_OUTBUF: usize = 32 * 1024 * 1024;
/// Connections held at once (attached terminals, `imux <command>` calls, event streams, waits).
const MAX_CLIENTS: usize = 256;
/// Minimum time between frames while output is streaming.
const FRAME_INTERVAL: Duration = Duration::from_millis(8);
/// Bytes of pty output consumed per pane per loop turn, so a flood cannot starve clients and rendering.
const PTY_BUDGET: usize = 256 * 1024;
/// How often window names are refreshed from the foreground processes.
const NAME_INTERVAL: Duration = Duration::from_millis(500);

/// Per-client key decoding state.
#[derive(Default)]
struct KeyState {
    /// The prefix was pressed; the next key is looked up in the prefix table.
    prefix: bool,
    /// Until then, repeatable bindings need no prefix (`repeat-time`).
    repeat_until: Option<Instant>,
    /// Unfinished escape sequence from the last chunk.
    rest: Vec<u8>,
    /// When `rest` stops waiting for more bytes (`escape-time`).
    esc_at: Option<Instant>,
}

struct AskMore {
    /// The prompts still to ask: label and first text.
    steps: Vec<(String, Option<String>)>,
    answers: Vec<String>,
}

struct Client {
    stream: UnixStream,
    inbuf: FrameBuf,
    outbuf: Vec<u8>,
    size: Option<Size>,
    keys: KeyState,
    /// Text shown on the bottom row until the deadline (`display-message`, command errors).
    message: Option<(String, Instant)>,
    /// Start of a mouse report whose end has not arrived yet.
    pending: Vec<u8>,
    closing: bool,
    renderer: Renderer,
    /// The picture protocols the terminal accepts (`ClientMsg::Modes`); all until it says.
    modes: u8,
    /// Id of the session this client looks at; `usize::MAX` until it says hello.
    session: usize,
    mode: Mode,
    /// Entry of the command history shown in the `:` prompt, and the line typed before recalling.
    recall: Option<usize>,
    draft: Vec<u8>,
    /// The question of an open `command-prompt` or `confirm-before` (label and initial text
    /// already expanded).
    ask: Option<crate::input::bindings::AskSpec>,
    /// The prompts still to ask when `command-prompt -p` named several, and the answers so far.
    ask_more: Option<AskMore>,
    /// The command a pane number picked in `display-panes` runs, with `%%` the pane's id.
    panes_template: Option<String>,
    /// Session asked for on the command line, used by the next `Hello`.
    want: Option<Want>,
    /// The process at the other end of the socket, when the system could tell.
    peer: Option<i32>,
    /// Repositories listed by `repo-wizard`, and the one picked from them.
    repos: Vec<PathBuf>,
    wizard_repo: Option<PathBuf>,
    /// Title and lines of the list `list-keys` and `show-messages` show.
    text: (String, Vec<String>),
    /// The session this client looked at before the current one (`switch-client -l`).
    last_session: Option<usize>,
    /// Whether the terminal window of this client has the keyboard focus, as far as it has said
    /// (`focus-events`); true until it reports otherwise.
    focused: bool,
    /// The terminal title last sent to this client (`set-titles`); empty until one is.
    title: String,
    /// The query typed after `/` in the picker or the agent panel that is open.
    filter: Option<modes::Filter>,
    /// The client speaks another protocol version: it has been told so and its write side is shut;
    /// what it still sends is read and dropped until it hangs up (closing at once with its next
    /// frames unread would reset the connection and the explanation would never arrive).
    refused: bool,
    /// When this client last typed, moved the mouse over the window or changed its size
    /// (`window-size latest` follows the one that did so last).
    last_input: Instant,
    /// `attach -r` (`ClientMsg::ReadOnly`): what this client types and its mouse are dropped, bar
    /// the keys that detach it.
    read_only: bool,
}

impl Client {
    /// Switch to a mode; a query typed in an earlier picker or panel does not carry over.
    fn enter(&mut self, mode: Mode) {
        self.mode = mode;
        self.filter = None;
    }
}

/// A decoded SGR mouse report (`CSI < code ; x ; y M|m`), coordinates 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mouse {
    code: u16,
    x: u16,
    y: u16,
    release: bool,
}

enum Input {
    Keys(Vec<u8>),
    Mouse(Mouse),
}

const MOUSE_MOTION: u16 = 32;
const MOUSE_WHEEL: u16 = 64;
/// Longest report we wait for before treating the bytes as ordinary input.
const MAX_MOUSE_REPORT: usize = 32;

/// Split raw client input into key bytes and mouse reports. An unfinished report is kept in
/// `pending` and completed by the next chunk.
fn split_input(pending: &mut Vec<u8>, data: &[u8]) -> Vec<Input> {
    let mut buf = std::mem::take(pending);
    buf.extend_from_slice(data);
    let mut out = Vec::new();
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        if !buf[i..].starts_with(b"\x1b[<") {
            keys.push(buf[i]);
            i += 1;
            continue;
        }
        let body = &buf[i + 3..];
        let end = body.iter().position(|b| !(b.is_ascii_digit() || *b == b';'));
        match end {
            None if body.len() < MAX_MOUSE_REPORT => {
                *pending = buf[i..].to_vec();
                break;
            }
            Some(n) if matches!(body[n], b'M' | b'm') => {
                let fields: Vec<Option<u16>> =
                    std::str::from_utf8(&body[..n]).unwrap_or("").split(';').map(|f| f.parse().ok()).collect();
                if let [Some(code), Some(x), Some(y)] = fields[..] {
                    if !keys.is_empty() {
                        out.push(Input::Keys(std::mem::take(&mut keys)));
                    }
                    out.push(Input::Mouse(Mouse {
                        code,
                        x: x.saturating_sub(1),
                        y: y.saturating_sub(1),
                        release: body[n] == b'm',
                    }));
                    i += 3 + n + 1;
                    continue;
                }
            }
            _ => {}
        }
        keys.push(buf[i]);
        i += 1;
    }
    if !keys.is_empty() {
        out.push(Input::Keys(keys));
    }
    out
}

struct Server {
    registry: Registry,
    listener: UnixListener,
    clients: HashMap<usize, Client>,
    next_id: usize,
    sessions: Vec<Session>,
    next_session: usize,
    /// When the server started, in seconds since the epoch (`#{start_time}`).
    started: u64,
    /// Session a new client attaches to: the one most recently switched to.
    recent: usize,
    next_pane: PaneId,
    status_tick: Instant,
    name_tick: Instant,
    /// Smallest attached client over all sessions; the fallback for a session with no size of its own.
    size: Option<Size>,
    /// Size of each session by id: the smallest client looking at it, kept after the last one
    /// leaves; a session made with `new-session -d` has 80x24 until a client looks at it.
    session_sizes: HashMap<usize, Size>,
    dirty: bool,
    /// Where the last `display-menu` box was (`-x L`).
    last_menu: Option<(usize, usize)>,
    /// Where the last `display-popup` box was (`-x L`).
    last_popup: Option<(usize, usize)>,
    backlog: HashSet<PaneId>,
    last_render: Instant,
    scratch: Vec<u8>,
    /// Closed panes whose process had not exited yet.
    zombies: Vec<Pid>,
    /// Pane that received the last button press; drags and the release go there.
    mouse_target: Option<PaneId>,
    /// The mouse button that is down, for the keys of a drag (`MouseDrag1Pane`).
    mouse_held: Option<mousekey::Held>,
    /// The last press, to tell second and third presses.
    mouse_pressed: Option<mousekey::Pressed>,
    /// What the pointer was at, only while a command bound to a mouse key runs.
    mouse_ctx: Option<mousekey::MouseCtx>,
    /// A command from a program in a pane is running, so a user option it sets is tainted.
    from_pane: bool,
    /// Divider being dragged with the mouse.
    drag: Option<Separator>,
    /// The paste buffers: what was copied lately and what `set-buffer` made.
    buffers: crate::buffers::Buffers,
    /// The marked pane (`select-pane -m`); may name a pane that is gone, see `marked_pane`.
    marked: Option<PaneId>,
    /// The number the next window to be numbered gets (`@3` in targets).
    next_window: usize,
    /// Variables new panes get besides the server's own environment (`set-environment`, and the
    /// ones `update-environment` copies from clients); `None` is a variable they do not get.
    pane_env: Vec<(String, Option<String>)>,
    /// Panes restored into the home directory because the saved one was gone: the saved path and
    /// the home directory they started in (`#{pane_restore_missing}`; the snapshot keeps the saved
    /// path until the pane leaves that directory).
    kept_dirs: HashMap<PaneId, (String, String)>,
    /// Who made a pane: the pane whose program asked for it over the socket (`spawned.rs`). A pane
    /// the user made has no entry. Never saved; ids are never reused, so an entry cannot be taken
    /// over by another pane.
    spawned_by: HashMap<PaneId, PaneId>,
    /// Saved directories that were gone at restore and not told to a client yet.
    missing_notice: Vec<String>,
    /// Clipboard helper processes still running.
    copiers: Vec<std::process::Child>,
    /// When each pane last had a clipboard write (OSC 52) passed on, for the rate limit.
    clip_at: HashMap<PaneId, Instant>,
    /// The last messages shown on the bottom row, oldest first, for `show-messages`.
    messages: VecDeque<String>,
    autoscroll: Option<copy::AutoScroll>,
    autoscroll_tick: Instant,
    /// Pane and cell where the left button went down, for starting a drag selection.
    press: Option<(PaneId, usize, usize)>,
    last_click: Option<copy::Click>,
    bindings: Bindings,
    options: Options,
    /// `name=value` variables of the configuration file.
    vars: Vars,
    /// Nesting of `source-file`, to stop a file that includes itself.
    source_depth: usize,
    /// Problems found reading the configuration at start, for the first client to see.
    config_errors: Vec<String>,
    /// Lines run from the `:` prompt, oldest first, shared by all clients.
    history: Vec<String>,
    /// Where autosaves and named snapshots live; `None` when no state directory can be found.
    store: Option<Store>,
    /// The thread that writes the periodic autosave; the final and the requested saves are
    /// written by the loop itself after it has finished what the thread was given.
    writer: Option<writer::Writer>,
    autosave_at: Instant,
    /// Hash of what the last autosave held (without its time), to skip identical ones.
    autosaved: Option<u64>,
    /// Finished `worktree` jobs waiting for the loop.
    jobs: worktree::Jobs,
    /// Pictures are decoded here, off this thread (`image::worker`).
    decoder: crate::image::worker::Pool,
    /// `run-shell` and `if-shell` commands that run or have ended.
    shells: shellcmd::Shells,
    /// `#(command)` outputs of the status line.
    segments: segments::Segments,
    /// Connections that follow the events (`imux events`).
    stream: events::Stream,
    /// The plugins that run and the connections that belong to them.
    plugins: plugins::Runtime,
    /// `set-hook` commands and the events waiting to run them.
    hooks: hooks::Hooks,
    focus: focus::Focus,
    /// The agent dock is shown (`agent-panel dock`, toggled by the `agent-panel` command).
    dock_open: bool,
    /// `imux agent wait` connections held open.
    waiters: Vec<control::Waiter>,
    /// `imux pane wait-output` connections held open, and the panes that printed since the last
    /// loop turn (only recorded while someone waits).
    output_waiters: Vec<outwait::OutputWaiter>,
    output_touched: HashSet<PaneId>,
    /// `imux agent start` connections held open until the program reports (`agentstart.rs`).
    start_waiters: Vec<agentstart::StartWaiter>,
    /// Agent names of restored panes, until their program reports again (`names.rs`).
    pending_names: HashMap<PaneId, String>,
    /// Panes whose waiting name came from `agent start`: the name goes to every record the pane
    /// gets until the started program ends, so a program that clears or replaces its record while
    /// it starts up (behind a dialog, say) does not lose it (`names.rs`).
    started_names: HashSet<PaneId>,
    /// `wait-for` channels and the clients blocked on them.
    channels: channels::Channels,
    /// `pipe-pane` commands, by pane.
    pipes: pipes::Pipes,
    /// `command-alias` entries: alias to the command it stands for, and which `command-alias[N]`
    /// slot holds which alias.
    aliases: HashMap<String, String>,
    alias_slots: HashMap<u32, String>,
    /// Set by a command run for `imux <command>` that must keep its connection open.
    deferred: bool,
    shutdown: bool,
}

/// Entry point of the daemon (`imux server`).
pub fn run() -> io::Result<()> {
    let dir = paths::ensure_socket_dir()?;
    log::redirect_to_file(&dir.join(paths::log_file_name()), Level::Info);
    crate::tty::limits::harden();
    let sock = paths::socket_path();
    let start_lock = lock_start(&dir)?;
    claim_socket(&sock)?;

    // SAFETY: umask only changes this process's file creation mask; set before bind so the socket
    // never exists with a wider mode than 0600, put back right after (no other thread runs yet).
    let old_mask = unsafe { nix::libc::umask(0o177) };
    let bound = UnixListener::bind(&sock);
    // SAFETY: as above.
    unsafe { nix::libc::umask(old_mask) };
    let mut listener = bound.map_err(|e| ctx(e, &format!("bind {}", sock.display())))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| ctx(e, "chmod socket"))?;
    drop(start_lock);
    let signals = signal::install(&[Signal::SIGCHLD, Signal::SIGTERM, Signal::SIGHUP])?;

    let mut poll = Poll::new().map_err(|e| ctx(e, "mio poll"))?;
    let registry = poll.registry().try_clone().map_err(|e| ctx(e, "clone registry"))?;
    registry
        .register(&mut listener, LISTENER, Interest::READABLE)
        .map_err(|e| ctx(e, "register listener"))?;
    registry
        .register(&mut SourceFd(&signals.read.as_raw_fd()), SIGNALS, Interest::READABLE)
        .map_err(|e| ctx(e, "register signals"))?;

    log::info(&format!("server listening on {}", sock.display()));
    let registry_for_jobs = registry.try_clone().map_err(|e| ctx(e, "clone registry"))?;
    let store = match Store::from_env() {
        Ok(store) => Some(store),
        Err(e) => {
            log::warn(&format!("sessions will not be saved: {e}"));
            None
        }
    };
    if let Some(store) = &store {
        store.set_aside_unreadable_autosave();
    }
    let writer = store.clone().and_then(|store| match writer::Writer::spawn(store) {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn(&format!("autosave thread: {e}; the loop will write the autosave itself"));
            None
        }
    });
    // One waker serves every helper thread: mio allows a single `Waker` per `Poll`.
    let jobs = worktree::Jobs::new(&registry_for_jobs).map_err(|e| ctx(e, "worktree waker"))?;
    let decoder = {
        let waker = jobs.waker();
        crate::image::worker::spawn(move || {
            let _ = waker.wake();
        })
        .map_err(|e| ctx(e, "picture decoder"))?
    };
    let mut server = Server {
        registry,
        listener,
        clients: HashMap::new(),
        next_id: 0,
        sessions: Vec::new(),
        next_session: 0,
        started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        recent: 0,
        next_pane: 0,
        status_tick: Instant::now(),
        name_tick: Instant::now(),
        size: None,
        session_sizes: HashMap::new(),
        dirty: false,
        last_menu: None,
        last_popup: None,
        backlog: HashSet::new(),
        last_render: Instant::now(),
        scratch: vec![0u8; 64 * 1024],
        zombies: Vec::new(),
        mouse_target: None,
        mouse_held: None,
        mouse_pressed: None,
        mouse_ctx: None,
        from_pane: false,
        drag: None,
        buffers: crate::buffers::Buffers::default(),
        marked: None,
        next_window: 1,
        pane_env: Vec::new(),
        kept_dirs: HashMap::new(),
        spawned_by: HashMap::new(),
        missing_notice: Vec::new(),
        copiers: Vec::new(),
        clip_at: HashMap::new(),
        messages: VecDeque::new(),
        autoscroll: None,
        autoscroll_tick: Instant::now(),
        press: None,
        last_click: None,
        bindings: Bindings::defaults(),
        options: Options::new(),
        vars: Vars::new(),
        source_depth: 0,
        config_errors: Vec::new(),
        history: Vec::new(),
        store,
        writer,
        autosave_at: Instant::now(),
        autosaved: None,
        jobs,
        decoder,
        shells: shellcmd::Shells::default(),
        segments: segments::Segments::default(),
        stream: events::Stream::default(),
        plugins: plugins::Runtime::default(),
        hooks: hooks::Hooks::default(),
        focus: focus::Focus::default(),
        dock_open: false,
        waiters: Vec::new(),
        output_waiters: Vec::new(),
        output_touched: HashSet::new(),
        start_waiters: Vec::new(),
        pending_names: HashMap::new(),
        started_names: HashSet::new(),
        channels: channels::Channels::default(),
        pipes: pipes::Pipes::default(),
        aliases: HashMap::new(),
        alias_slots: HashMap::new(),
        deferred: false,
        shutdown: false,
    };
    let keys = copymode::default_mode_keys(std::env::var("VISUAL").ok().as_deref(), std::env::var("EDITOR").ok().as_deref());
    if let Err(e) = server.options.set("mode-keys", Some(keys), false) {
        log::warn(&format!("mode-keys from the editor: {e}"));
    }
    server.sync_history();
    server.load_config();
    server.plugins_start();
    let result = server.event_loop(&mut poll, &signals);
    server.plugins_stop();
    server.stop_writer();
    server.pipes.close_all();
    shellcmd::kill_all();
    log::info(&format!("options never read: {}", server.options.unused().join(" ")));
    for pane in server.sessions.iter().flat_map(|s| s.windows.iter()).flat_map(|w| w.panes.values()).filter(|p| p.dead.is_none()) {
        hang_up(pane.pty.child);
    }
    if let Err(e) = std::fs::remove_file(&sock) {
        log::warn(&format!("remove socket {}: {e}", sock.display()));
    }
    log::info("server stopped");
    result
}

/// Run one piece of work so that a panic in it is logged and contained instead of ending the
/// server, and with it every session. `None` means it panicked.
fn guarded<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(payload) => {
            log::error(&format!("{what} panicked and was abandoned: {}", log::panic_text(payload.as_ref())));
            None
        }
    }
}

fn hang_up(child: Pid) {
    match killpg(child, Signal::SIGHUP) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(e) => log::warn(&format!("SIGHUP to {child}: {e}")),
    }
}

/// Take the lock that serializes server starts until the socket is bound. Clients that find no
/// server each start one; without the lock the later one could take a socket the earlier one has
/// just bound for a stale one, remove it and leave the earlier server unreachable with its sessions.
fn lock_start(dir: &Path) -> io::Result<nix::fcntl::Flock<std::fs::File>> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join("start.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| ctx(e, &format!("open {}", path.display())))?;
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
        .map_err(|(_, e)| ctx(io::Error::from(e), "lock server start"))
}

/// Refuse to start if a live server owns the socket; remove a stale one. Called with the start
/// lock held.
fn claim_socket(sock: &Path) -> io::Result<()> {
    match std::os::unix::net::UnixStream::connect(sock) {
        Ok(_) => Err(io::Error::new(io::ErrorKind::AddrInUse, "server already running")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            log::warn(&format!("removing stale socket {}", sock.display()));
            std::fs::remove_file(sock).map_err(|e| ctx(e, "remove stale socket"))
        }
        Err(e) => Err(ctx(e, &format!("probe {}", sock.display()))),
    }
}

fn cell_px(size: Size) -> (u16, u16) {
    (size.xpix.checked_div(size.cols).unwrap_or(0), size.ypix.checked_div(size.rows).unwrap_or(0))
}

/// The smallest of some client sizes; the pixel size scales from the cells of the narrowest one.
fn smallest(sizes: impl Iterator<Item = Size>) -> Option<Size> {
    let mut best: Option<Size> = None;
    for size in sizes {
        best = Some(match best {
            None => size,
            Some(b) => {
                let cols = b.cols.min(size.cols);
                let rows = b.rows.min(size.rows);
                let src = if size.cols < b.cols { size } else { b };
                let (cw, ch) = cell_px(src);
                Size { cols, rows, xpix: cw * cols, ypix: ch * rows }
            }
        });
    }
    best
}

/// The largest of some client sizes; the pixel size scales from the cells of the widest one.
fn largest(sizes: impl Iterator<Item = Size>) -> Option<Size> {
    let mut best: Option<Size> = None;
    for size in sizes {
        best = Some(match best {
            None => size,
            Some(b) => {
                let cols = b.cols.max(size.cols);
                let rows = b.rows.max(size.rows);
                let src = if size.cols > b.cols { size } else { b };
                let (cw, ch) = cell_px(src);
                Size { cols, rows, xpix: cw * cols, ypix: ch * rows }
            }
        });
    }
    best
}

/// Window name until automatic-rename: the shell's name, e.g. `zsh`.
fn default_window_name(shell: &str) -> String {
    shell.rsplit('/').next().unwrap_or("sh").to_string()
}

fn pane_size(rect: Rect, cell: (u16, u16)) -> Size {
    Size { cols: rect.w, rows: rect.h, xpix: cell.0 * rect.w, ypix: cell.1 * rect.h }
}

impl Server {
    fn event_loop(&mut self, poll: &mut Poll, signals: &SignalPipe) -> io::Result<()> {
        let mut events = Events::with_capacity(64);
        while !self.shutdown {
            let mut timeout = if !self.backlog.is_empty() {
                Some(Duration::ZERO)
            } else if self.dirty {
                Some(FRAME_INTERVAL.saturating_sub(self.last_render.elapsed()))
            } else if !self.sessions.is_empty() {
                Some(NAME_INTERVAL.saturating_sub(self.name_tick.elapsed()))
            } else {
                None
            };
            if let Some(deadline) = self.next_key_deadline().into_iter().chain(self.autoscroll_due()).chain(self.waiter_deadline()).chain(self.output_deadline()).chain(self.start_deadline()).chain(self.typeahead_deadline()).chain(self.animation_deadline()).min() {
                let wait = deadline.saturating_duration_since(Instant::now());
                timeout = Some(timeout.map_or(wait, |t| t.min(wait)));
            }
            match poll.poll(&mut events, timeout) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ctx(e, "mio poll")),
            }
            let pending: Vec<PaneId> = self.backlog.drain().collect();
            for id in pending {
                self.read_pane_guarded(id);
            }
            for event in events.iter() {
                match event.token() {
                    LISTENER => {
                        guarded("accept", || self.accept());
                    }
                    SIGNALS => self.on_signals(signals)?,
                    worktree::WAKE => {
                        guarded("decoded pictures", || self.finish_decodes());
                        guarded("worktree", || self.finish_worktrees());
                        guarded("shell commands", || self.finish_shells());
                        guarded("status commands", || self.finish_segments());
                    }
                    Token(t) if t >= PANE_BASE => self.read_pane_guarded((t - PANE_BASE) as PaneId),
                    Token(t) if t >= CLIENT_BASE => {
                        let id = t - CLIENT_BASE;
                        if event.is_readable() || event.is_read_closed() {
                            self.client_guarded(id, "client input", |s| s.read_client(id));
                        }
                        if event.is_writable() {
                            self.client_guarded(id, "client output", |s| s.flush_client(id));
                        }
                    }
                    Token(_) => {}
                }
            }
            let now = Instant::now();
            let due: Vec<usize> =
                self.clients.iter().filter(|(_, c)| c.keys.esc_at.is_some_and(|t| t <= now)).map(|(&id, _)| id).collect();
            for id in due {
                self.client_guarded(id, "key timeout", |s| s.flush_keys(id));
            }
            guarded("expire", || self.expire(now));
            guarded("waiters", || self.check_waiters());
            guarded("output waiters", || self.check_output_waiters());
            guarded("start waiters", || self.check_start_waiters());
            guarded("typeahead", || self.release_typeahead());
            guarded("animations", || self.step_animations());
            if self.autoscroll_due().is_some_and(|t| t <= now) {
                guarded("autoscroll", || self.autoscroll_step());
            }
            if self.name_tick.elapsed() >= NAME_INTERVAL {
                self.name_tick = Instant::now();
                guarded("housekeeping", || {
                    self.reap_copiers();
                    self.prune_clip_times();
                    self.update_window_names();
                    self.prune_pending_names();
                    self.expire_agents();
                });
                guarded("pipes", || self.service_pipes());
                guarded("status commands", || self.segments_tick());
                guarded("plugins", || self.plugins_tick());
                guarded("autosave", || self.autosave_tick());
            }
            guarded("silence", || self.check_silence());
            guarded("alerts", || self.settle_alerts());
            guarded("focus", || self.track_focus());
            guarded("hooks", || self.run_hooks());
            if self.status_tick.elapsed() >= Duration::from_secs(self.options.num("status-interval")) {
                self.status_tick = Instant::now();
                self.dirty = true;
            }
            if self.dirty
                && self.backlog.is_empty()
                && self.last_render.elapsed() >= FRAME_INTERVAL
                && guarded("render", || self.render()).is_none()
            {
                self.dirty = false;
                for c in self.clients.values_mut() {
                    c.renderer.invalidate();
                }
            }
        }
        Ok(())
    }

    /// Read a pane's output; if that panics the pane is closed, since its state cannot be trusted.
    fn read_pane_guarded(&mut self, id: PaneId) {
        if guarded("pane output", || self.read_pane(id)).is_none() {
            guarded("close broken pane", || self.close_pane(id, None));
        }
    }

    /// Run work for one client; if it panics the client is disconnected, the sessions live on.
    fn client_guarded(&mut self, id: usize, what: &str, f: impl FnOnce(&mut Server)) {
        if guarded(what, || f(self)).is_none() {
            guarded("drop broken client", || self.remove_client(id));
        }
    }

    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    // The directory and the socket are private to this user, so this can only
                    // fail if a mode was changed by hand; the check does not rely on them.
                    let mine = nix::unistd::getuid().as_raw();
                    match proc::peer_uid(stream.as_fd()) {
                        Some(uid) if uid == mine => {}
                        other => {
                            log::warn(&format!("connection refused: peer user {other:?}, not {mine}"));
                            continue;
                        }
                    }
                    if self.clients.len() >= MAX_CLIENTS {
                        log::warn(&format!("connection refused: {MAX_CLIENTS} clients already connected"));
                        continue;
                    }
                    let id = self.next_id;
                    self.next_id += 1;
                    let interest = Interest::READABLE | Interest::WRITABLE;
                    if let Err(e) = self.registry.register(&mut stream, Token(CLIENT_BASE + id), interest) {
                        log::error(&format!("register client {id}: {e}"));
                        continue;
                    }
                    let peer = proc::peer_pid(stream.as_fd());
                    log::info(&format!("client {id} connected"));
                    self.clients.insert(
                        id,
                        Client {
                            stream,
                            inbuf: FrameBuf::default(),
                            outbuf: Vec::new(),
                            size: None,
                            keys: KeyState::default(),
                            message: None,
                            pending: Vec::new(),
                            closing: false,
                            renderer: Renderer::new(),
                            modes: 7,
                            session: usize::MAX,
                            mode: Mode::Normal,
                            recall: None,
                            draft: Vec::new(),
                            ask: None,
                            ask_more: None,
                            panes_template: None,
                            want: None,
                            repos: Vec::new(),
                            wizard_repo: None,
                            text: (String::new(), Vec::new()),
                            last_session: None,
                            focused: true,
                            title: String::new(),
                            filter: None,
                            refused: false,
                            last_input: Instant::now(),
                            read_only: false,
                            peer,
                        },
                    );
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    log::error(&format!("accept: {e}"));
                    break;
                }
            }
        }
    }

    fn read_client(&mut self, id: usize) {
        let mut buf = std::mem::take(&mut self.scratch);
        let mut msgs = Vec::new();
        let mut dead = false;
        if let Some(c) = self.clients.get_mut(&id) {
            loop {
                match c.stream.read(&mut buf) {
                    Ok(0) => {
                        dead = true;
                        break;
                    }
                    Ok(n) => c.inbuf.push(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        log::warn(&format!("read client {id}: {e}"));
                        dead = true;
                        break;
                    }
                }
            }
            loop {
                match c.inbuf.next::<ClientMsg>() {
                    Ok(Some(m)) => msgs.push(m),
                    Ok(None) => break,
                    Err(e) => {
                        log::warn(&format!("client {id} protocol error: {e}"));
                        dead = true;
                        break;
                    }
                }
            }
        }
        self.scratch = buf;
        for msg in msgs {
            self.handle_msg(id, msg);
        }
        if dead {
            self.remove_client(id);
        }
    }

    fn handle_msg(&mut self, id: usize, msg: ClientMsg) {
        if self.clients.get(&id).is_some_and(|c| c.refused) {
            return;
        }
        if let Err(e) = self.plugin_gate(id, &msg) {
            log::warn(&format!("client {id}: {e}"));
            return self.finish_request(id, Err(e));
        }
        let Some(c) = self.clients.get_mut(&id) else { return };
        match msg {
            ClientMsg::Hello(size) => {
                c.size = Some(size);
                c.renderer.set_cell_px(size.xpix.into(), size.ypix.into(), size.cols.into(), size.rows.into());
                let want = c.want.take();
                log::info(&format!("client {id} attached {}x{}", size.cols, size.rows));
                let first = self.sessions.is_empty();
                if first {
                    self.size = Some(size);
                }
                let restored = first && self.restore_on_start();
                let found = match want {
                    Some(Want::Attach(name)) => self
                        .sessions
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.id)
                        .ok_or_else(|| format!("can't find session: {name}")),
                    Some(Want::Create(Some(name))) if restored && self.sessions.iter().any(|s| s.name == name) => {
                        Ok(self.sessions.iter().find(|s| s.name == name).map_or(self.recent, |s| s.id))
                    }
                    Some(Want::Create(Some(name))) if self.sessions.iter().any(|s| s.name == name) => {
                        Err(format!("duplicate session: {name}"))
                    }
                    Some(Want::Create(Some(name))) if name.is_empty() => Err("empty session name".to_string()),
                    Some(Want::Create(name)) => self.start_session(None, name).map_err(|e| format!("cannot start shell: {e}")),
                    None if self.sessions.is_empty() => self.start_session(None, None).map_err(|e| format!("cannot start shell: {e}")),
                    None => Ok(self.recent),
                };
                match found {
                    Ok(sid) => {
                        self.attach(id, sid);
                        if !first {
                            self.apply_size();
                        }
                        if !self.missing_notice.is_empty() {
                            let gone = std::mem::take(&mut self.missing_notice);
                            let first_path: String = crate::sanitize::clean(&gone[0]).chars().take(80).collect();
                            let text = match gone.len() {
                                1 => format!("restore: {first_path} is gone, the pane started in the home directory"),
                                n => format!("restore: {n} directories are gone (first: {first_path}), those panes started in the home directory"),
                            };
                            self.show_message(id, text);
                        }
                    }
                    Err(msg) => {
                        log::error(&format!("client {id}: {msg}"));
                        self.queue(id, &ServerMsg::Error(msg));
                        self.close_after_flush(id);
                    }
                }
                self.dirty = true;
            }
            ClientMsg::Version(theirs) => {
                let ours = proto::version();
                if theirs != ours {
                    log::warn(&format!("client {id} speaks protocol {theirs}, this server {ours}: refused"));
                    self.queue(id, &ServerMsg::Error(proto::mismatch_text(ours, theirs, std::process::id())));
                    match self.clients.get_mut(&id) {
                        Some(c) if c.outbuf.is_empty() && c.stream.shutdown(std::net::Shutdown::Write).is_ok() => c.refused = true,
                        _ => self.close_after_flush(id),
                    }
                }
            }
            ClientMsg::Want(want) => c.want = Some(want),
            ClientMsg::Caps(caps) => c.renderer.set_caps(caps),
            ClientMsg::Modes(mask) => c.modes = mask & 7,
            ClientMsg::ReadOnly => {
                c.read_only = true;
                log::info(&format!("client {id} is read-only"));
            }
            ClientMsg::List => {
                let text = self.listing();
                self.queue(id, &ServerMsg::Text(text));
                self.close_after_flush(id);
            }
            ClientMsg::Resize(size) => {
                c.size = Some(size);
                if !c.read_only {
                    c.last_input = Instant::now();
                }
                c.renderer.set_cell_px(size.xpix.into(), size.ypix.into(), size.cols.into(), size.rows.into());
                c.renderer.invalidate();
                self.apply_size();
                self.dirty = true;
            }
            ClientMsg::Input(data) => {
                if !c.read_only {
                    c.last_input = Instant::now();
                }
                self.handle_input(id, &data);
                if self.options.text("window-size") == "latest" && self.size_would_change() {
                    self.apply_size();
                }
            }
            ClientMsg::Kill => {
                log::info(&format!("client {id} requested kill"));
                self.discard_autosave();
                self.shutdown = true;
            }
            ClientMsg::Persist(op) => {
                let reply = match self.persist_op(&op) {
                    Ok(text) => ServerMsg::Text(text),
                    Err(msg) => ServerMsg::Error(msg),
                };
                self.queue(id, &reply);
                self.close_after_flush(id);
            }
            ClientMsg::Worktree(args) => self.start_worktree(worktree::Reply::Cli(id), args),
            ClientMsg::WorktreeRemove(args) => self.start_remove(worktree::Reply::Cli(id), args),
            ClientMsg::ListKeys => {
                let text = self.bindings.list_lines().join("\n") + "\n";
                self.finish_request(id, Ok(text));
            }
            ClientMsg::Run(args) => self.run_request(id, args),
            ClientMsg::Env(vars) => self.update_environment(vars),
            ClientMsg::AgentList => {
                let text = self.agent_list_text();
                self.finish_request(id, Ok(text));
            }
            ClientMsg::AgentGet(pane) => {
                let result = self.agent_get_text(pane);
                self.finish_request(id, result);
            }
            ClientMsg::AgentWait(args) => self.start_wait(id, args),
            ClientMsg::AgentPrompt(args) => self.handle_prompt(id, args),
            ClientMsg::Subscribe(types) => self.subscribe(id, &types),
            ClientMsg::PaneReport(args) => {
                let result = self.handle_report(id, args);
                self.finish_request(id, result);
            }
            ClientMsg::PaneRelease(pane, source) => {
                let result = self.handle_release(id, pane, &source);
                self.finish_request(id, result);
            }
        }
    }

    fn handle_input(&mut self, id: usize, data: &[u8]) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let read_only = c.read_only;
        for input in split_input(&mut c.pending, data) {
            match input {
                Input::Keys(keys) => self.handle_keys(id, &keys),
                Input::Mouse(_) if read_only => {}
                Input::Mouse(m) => self.handle_mouse(id, m),
            }
        }
    }

    /// Clicking a pane focuses it, clicking a window tab selects that window. Programs that asked
    /// for mouse events also get the event, translated to pane coordinates.
    fn handle_mouse(&mut self, client: usize, m: Mouse) {
        let Some(c) = self.clients.get(&client) else { return };
        let Some(size) = self.size_for(c.session) else { return };
        let Some(client_size) = c.size else { return };
        if self.client_popup(client).is_some() {
            return self.popup_mouse(client, m);
        }
        if matches!(c.mode, Mode::Picker(_)) {
            return self.picker_mouse(client, m);
        }
        if matches!(c.mode, Mode::Menu(_)) {
            return self.menu_mouse(client, m);
        }
        if matches!(c.mode, Mode::Agents(_)) {
            return self.agents_mouse(client, m);
        }
        if matches!(c.mode, Mode::Prompt(_)) {
            return;
        }
        if matches!(c.mode, Mode::Clock(_)) {
            if !m.release && m.code & (MOUSE_MOTION | MOUSE_WHEEL) == 0 {
                self.mode_keys(client, b"q");
            }
            return;
        }
        let Some(si) = self.session_index(c.session) else { return };
        let wheel = m.code & MOUSE_WHEEL != 0;
        let motion = m.code & MOUSE_MOTION != 0;
        let press = !m.release && !motion && !wheel;
        if self.drag.is_none() && self.mouse_binding(client, si, m) {
            return;
        }
        if m.release && self.drag.take().is_some() {
            return;
        }
        let status_rows = self.sessions[si].status_rows;
        let bar_rows = status_rows.min(client_size.rows);
        let bar_first = if self.status_top() { 0 } else { client_size.rows - bar_rows };
        if bar_rows > 0 && m.y >= bar_first && m.y < bar_first + bar_rows {
            if press && m.code & 3 == 0 {
                self.click_status(client, usize::from(m.y - bar_first), m.x);
            }
            return;
        }
        if let Some(dock) = self.dock_rect(size, status_rows)
            && m.x >= dock.x
            && m.x < dock.x + dock.w
            && m.y >= dock.y
            && m.y < dock.y + dock.h
        {
            if press && m.code & 3 == 0 {
                self.dock_click(client, si, m);
            }
            return;
        }
        let area = self.pane_area(size, status_rows);
        let label = self.pane_status();
        let win = self.sessions[si].window_mut();
        if let Some(sep) = self.drag {
            if motion && m.code & 3 == 0 {
                let pos = if sep.vertical { m.x } else { m.y };
                self.drag = win.layout.drag_divider(sep, pos, area);
                self.dirty = true;
                self.relayout();
            } else if m.release {
                self.drag = None;
            }
            return;
        }
        if press && m.code & 3 == 0 {
            let on_border = |s: &Separator| {
                if s.vertical {
                    m.x == s.x && m.y >= s.y && m.y < s.y + s.len
                } else {
                    m.y == s.y && m.x >= s.x && m.x < s.x + s.len
                }
            };
            if let Some(sep) = win.layout.separators(area).into_iter().find(on_border) {
                self.drag = Some(sep);
                return;
            }
        }
        let rects: Vec<(PaneId, Rect)> = win.layout.rects(area).into_iter().map(|(id, r)| (id, label.content(r))).collect();
        let target = if press || wheel {
            rects.iter().find(|(_, r)| m.x >= r.x && m.x < r.x + r.w && m.y >= r.y && m.y < r.y + r.h)
        } else {
            self.mouse_target.and_then(|t| rects.iter().find(|(id, _)| *id == t))
        };
        if m.release {
            self.mouse_target = None;
        }
        let Some(&(pid, rect)) = target else { return };
        if press {
            self.mouse_target = Some(pid);
            if win.active != pid {
                win.focus(pid);
                self.dirty = true;
            }
        }
        let imux_handles = win.panes.get(&pid).is_some_and(|p| {
            let g = &p.term.grid;
            p.copy.is_some() || (g.modes.mouse == MouseMode::None && !g.is_alt())
        });
        if imux_handles {
            return self.copy_mouse(client, si, pid, rect, m);
        }
        let Some(pane) = win.panes.get(&pid) else { return };
        let modes = &pane.term.grid.modes;
        let wanted = match modes.mouse {
            MouseMode::None => false,
            MouseMode::Click => !motion,
            MouseMode::Drag => !motion || m.code & 3 != 3,
            MouseMode::Any => true,
        };
        if !wanted {
            return;
        }
        let rx = m.x.saturating_sub(rect.x).min(rect.w - 1);
        let ry = m.y.saturating_sub(rect.y).min(rect.h - 1);
        let bytes = if modes.mouse_sgr {
            format!("\x1b[<{};{};{}{}", m.code, rx + 1, ry + 1, if m.release { 'm' } else { 'M' }).into_bytes()
        } else {
            let code = if m.release { m.code | 3 } else { m.code };
            match (u8::try_from(code + 32), u8::try_from(rx + 33), u8::try_from(ry + 33)) {
                (Ok(b), Ok(x), Ok(y)) => vec![0x1b, b'[', b'M', b, x, y],
                _ => return,
            }
        };
        if pane.dead.is_some() || pane.input_off {
            return;
        }
        if let Err(e) = pane.pty.write(&bytes) {
            log::warn(&format!("pane {pid}: write mouse to pty: {e}"));
        }
    }

    /// Decode client input into keys. A sequence cut off at the end of the chunk waits up to
    /// `escape-time` for the rest, so a lone Escape is told apart from the start of a sequence.
    fn handle_keys(&mut self, id: usize, data: &[u8]) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let mut buf = std::mem::take(&mut c.keys.rest);
        buf.extend_from_slice(data);
        let (parsed, used) = keys::parse(&buf, false);
        c.keys.rest = buf[used..].to_vec();
        let escape_time = Duration::from_millis(self.options.num("escape-time"));
        c.keys.esc_at = (!c.keys.rest.is_empty()).then(|| Instant::now() + escape_time);
        self.process_keys(id, parsed);
    }

    /// A key of a read-only client (`attach -r`): the prefix state machine runs so the keys that
    /// detach still work, anything else it would do is dropped.
    fn read_only_key(&mut self, id: usize, p: &Parsed) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let state = &mut c.keys;
        let bindings = &self.bindings;
        let command = if state.prefix {
            state.prefix = false;
            bindings.prefix_lookup(&p.key).map(|b| b.command.clone())
        } else if bindings.is_prefix(&p.key) {
            state.prefix = true;
            None
        } else {
            bindings.root_lookup(&p.key).map(|b| b.command.clone())
        };
        if let Some(Command::DetachClient) = command {
            self.run_command(id, &Command::DetachClient);
        }
    }

    /// `escape-time` ran out: whatever is left of the input is complete.
    fn flush_keys(&mut self, id: usize) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        c.keys.esc_at = None;
        let buf = std::mem::take(&mut c.keys.rest);
        let (parsed, _) = keys::parse(&buf, true);
        self.process_keys(id, parsed);
    }

    /// Run keys through the prefix state machine and the binding tables. Keys that are not
    /// bound go to the active pane unchanged; a key after the prefix that is not bound is dropped.
    fn process_keys(&mut self, id: usize, parsed: Vec<Parsed>) {
        let mut forward = Vec::new();
        let repeat_time = Duration::from_millis(self.options.num("repeat-time"));
        let in_prefix = self.clients.get(&id).is_some_and(|c| c.keys.prefix);
        for p in parsed {
            // The terminal says it gained or lost the focus: not a key for anybody.
            if p.key.code == keys::Code::Other && matches!(p.raw.as_slice(), b"\x1b[I" | b"\x1b[O") && self.options.flag("focus-events") {
                self.write_forward(id, &mut forward);
                self.client_focus(id, p.raw[2] == b'I');
                continue;
            }
            if self.clients.get(&id).is_some_and(|c| c.read_only) {
                self.read_only_key(id, &p);
                continue;
            }
            if self.client_popup(id).is_some() {
                self.write_forward(id, &mut forward);
                self.popup_key(id, &p);
                continue;
            }
            let Some(c) = self.clients.get_mut(&id) else { return };
            if !matches!(c.mode, Mode::Normal) {
                self.write_forward(id, &mut forward);
                let bytes = match p.key.code {
                    keys::Code::Escape => vec![0x1b],
                    keys::Code::Enter => vec![b'\r'],
                    keys::Code::Backspace => vec![0x7f],
                    _ => p.raw,
                };
                self.mode_keys(id, &bytes);
                continue;
            }
            let now = Instant::now();
            let state = &mut c.keys;
            let bindings = &self.bindings;
            let mut pass = false;
            let command = if state.prefix {
                state.prefix = false;
                let bound = bindings.prefix_lookup(&p.key);
                state.repeat_until = bound.filter(|b| b.repeat).map(|_| now + repeat_time);
                bound.map(|b| b.command.clone())
            } else if let Some(b) = bindings
                .prefix_lookup(&p.key)
                .filter(|b| b.repeat && state.repeat_until.is_some_and(|t| t > now))
            {
                state.repeat_until = Some(now + repeat_time);
                Some(b.command.clone())
            } else {
                state.repeat_until = None;
                if bindings.is_prefix(&p.key) {
                    state.prefix = true;
                    None
                } else if let Some(b) = bindings.root_lookup(&p.key) {
                    Some(b.command.clone())
                } else {
                    pass = true;
                    None
                }
            };
            if let Some(command) = command {
                self.write_forward(id, &mut forward);
                self.run_command(id, &command);
            } else if pass {
                if self.active_in_copy_mode(id) {
                    self.write_forward(id, &mut forward);
                    self.copy_key(id, p.key);
                } else {
                    forward.extend_from_slice(&p.raw);
                }
            }
        }
        self.write_forward(id, &mut forward);
        // A bar that shows `#{client_prefix}` changes when the prefix is pressed or used.
        if self.clients.get(&id).is_some_and(|c| c.keys.prefix) != in_prefix && self.status_mentions("client_prefix") {
            self.dirty = true;
        }
    }

    fn write_forward(&mut self, id: usize, bytes: &mut Vec<u8>) {
        if !bytes.is_empty() {
            self.write_active(id, bytes);
            bytes.clear();
        }
    }

    /// Soonest moment something per client expires: a half-received key sequence, pane numbers
    /// or a message.
    fn next_key_deadline(&self) -> Option<Instant> {
        self.clients
            .values()
            .flat_map(|c| {
                let panes = if let Mode::Panes(t) | Mode::Clock(t) = c.mode { Some(t) } else { None };
                [c.keys.esc_at, panes, c.message.as_ref().map(|m| m.1)]
            })
            .flatten()
            .min()
    }

    /// Drop pane numbers and messages whose time is up.
    fn expire(&mut self, now: Instant) {
        let mut changed = false;
        for c in self.clients.values_mut() {
            if matches!(c.mode, Mode::Panes(t) if t <= now) {
                c.mode = Mode::Normal;
                changed = true;
            }
            if matches!(c.mode, Mode::Clock(t) if t <= now) {
                c.mode = Mode::Clock(clock::next_tick());
                changed = true;
            }
            if c.message.as_ref().is_some_and(|m| m.1 <= now) {
                c.message = None;
                changed = true;
            }
        }
        self.dirty |= changed;
    }

    fn click_status(&mut self, client: usize, row: usize, x: u16) {
        let Some(c) = self.clients.get(&client) else { return };
        let (Some(client_size), Some(si)) = (c.size, self.session_index(c.session)) else { return };
        let cfg = self.status_cfg();
        let segments = self.segment_view(false);
        let agents = self.agent_states(si, Some(client));
        let session = &mut self.sessions[si];
        let bar = status_with(session, usize::from(client_size.cols), usize::from(session.status_rows), &cfg, &agents, &segments);
        if row == 0 && bar.agent_spans.iter().any(|&(start, end)| (start..end).contains(&usize::from(x))) {
            return self.open_agent_panel(client);
        }
        if row == 0 && usize::from(x) < bar.left_w {
            return self.open_picker(client, false);
        }
        let hit = bar
            .tab_spans
            .iter()
            .zip(&bar.tab_rows)
            .position(|(&(start, end), &r)| r == row && (start..end).contains(&usize::from(x)));
        if let Some(index) = hit {
            session.select(index);
            self.dirty = true;
        }
    }

    /// What the status line has at a cell, for mouse bindings: the part of it (see `MouseLoc`) and,
    /// on a window's entry, that window's position.
    fn status_spot(&mut self, client: usize, si: usize, row: usize, x: u16) -> (keys::MouseLoc, Option<usize>) {
        use keys::MouseLoc;
        let Some(client_size) = self.clients.get(&client).and_then(|c| c.size) else { return (MouseLoc::StatusDefault, None) };
        let cfg = self.status_cfg();
        let segments = self.segment_view(false);
        let agents = self.agent_states(si, Some(client));
        let session = &mut self.sessions[si];
        let cols = usize::from(client_size.cols);
        let bar = status_with(session, cols, usize::from(session.status_rows), &cfg, &agents, &segments);
        let x = usize::from(x);
        if row == 0 && x < bar.left_w {
            return (MouseLoc::StatusLeft, None);
        }
        let hit = bar.tab_spans.iter().zip(&bar.tab_rows).position(|(&(start, end), &r)| r == row && (start..end).contains(&x));
        if hit.is_some() {
            return (MouseLoc::Status, hit);
        }
        if row == 0 && !bar.right.is_empty() && x >= bar.right_start {
            return (MouseLoc::StatusRight, None);
        }
        (MouseLoc::StatusDefault, None)
    }

    fn write_active(&mut self, client: usize, bytes: &[u8]) {
        let Some(si) = self.client_session(client) else { return };
        let wi = self.sessions[si].current;
        let win = self.sessions[si].window();
        if win.panes.get(&win.active).is_none_or(|p| p.dead.is_some() || p.input_off) {
            return;
        }
        for id in self.typing_targets(si, wi, win.active) {
            if let Some(pane) = win.panes.get(&id).filter(|p| !p.input_off)
                && let Err(e) = pane.pty.write(bytes)
            {
                log::warn(&format!("write to pty: {e}"));
            }
        }
    }

    /// Run a bound command on behalf of a client, then tell the `after-<command>` hooks.
    fn run_command(&mut self, client: usize, cmd: &Command) {
        self.run_command_inner(client, cmd);
        if !matches!(cmd, Command::Chain(_) | Command::Server(_)) && !self.hooks.is_empty() {
            let text = cmd.describe();
            let mut words = text.split_whitespace().map(str::to_string);
            if let Some(name) = words.next() {
                let args: Vec<String> = words.collect();
                self.emit_after(Some(client), &name, &args);
            }
        }
    }

    fn run_command_inner(&mut self, client: usize, cmd: &Command) {
        let Some(si) = self.client_session(client) else { return };
        let Some(size) = self.size_of(si) else { return };
        let area = self.pane_area(size, self.sessions[si].status_rows);
        match cmd {
            Command::DetachClient => {
                log::info(&format!("client {client} detached"));
                self.queue(client, &ServerMsg::Detached);
                return self.close_after_flush(client);
            }
            Command::DetachOthers => {
                let others: Vec<usize> = self.clients.iter().filter(|(id, c)| **id != client && c.session != usize::MAX).map(|(id, _)| *id).collect();
                for id in others {
                    log::info(&format!("client {id} detached by client {client}"));
                    self.queue(id, &ServerMsg::Detached);
                    self.close_after_flush(id);
                }
                return;
            }
            Command::KillOtherSessions(target) => {
                let keep = match target {
                    None => Some(si),
                    Some(name) => self.sessions.iter().position(|s| &s.name == name),
                };
                match keep.map(|k| self.sessions[k].id) {
                    Some(keep) => {
                        let ids: Vec<usize> = self.sessions.iter().map(|s| s.id).filter(|id| *id != keep).collect();
                        for id in ids {
                            if let Some(i) = self.sessions.iter().position(|s| s.id == id) {
                                self.kill_session(i);
                            }
                        }
                    }
                    None => self.show_message(client, "can't find session".to_string()),
                }
                return;
            }
            Command::SwapWindowBy { by, select } => {
                let session = &mut self.sessions[si];
                let to = i64::try_from(session.current).ok().and_then(|c| usize::try_from(c + i64::from(*by)).ok());
                if to.is_some_and(|to| session.swap(session.current, to, *select)) {
                    self.dirty = true;
                } else {
                    self.show_message(client, "can't find window".to_string());
                }
                return;
            }
            Command::SwitchTo(target) => {
                match self.resolve(target, crate::target::Kind::Window, None) {
                    Ok(place) => {
                        let sid = self.sessions[place.si].id;
                        self.attach(client, sid);
                        if let Some(si) = self.sessions.iter().position(|s| s.id == sid) {
                            self.sessions[si].select(place.wi);
                        }
                        self.dirty = true;
                    }
                    Err(e) => self.show_message(client, e),
                }
                return;
            }
            Command::ClearHistory => {
                let win = self.sessions[si].window_mut();
                let active = win.active;
                if let Some(pane) = win.panes.get_mut(&active) {
                    pane.copy = None;
                    pane.term.grid.erase_display(3);
                }
                self.dirty = true;
                return;
            }
            Command::DisplayPanesFor(ms) => return self.display_panes_for(client, std::time::Duration::from_millis(*ms)),
            Command::DisplayPanesRun { ms, template } => return self.display_panes_run(client, *ms, template),
            Command::BreakPaneKeep => return self.break_pane_keep(client, si),
            Command::Server(words) => return self.run_words(client, words),
            Command::NewWindow => return self.new_window(si),
            Command::KillWindow => return self.kill_window(si),
            Command::NewSession => return self.new_session(client),
            Command::ChooseSession => return self.open_picker(client, false),
            Command::ChooseWindow => return self.open_picker(client, true),
            Command::RepoWizard => return self.open_wizard(client),
            Command::AgentPanel => return self.open_agent_panel(client),
            Command::RenameWindow => return self.open_prompt(client, PromptKind::Window),
            Command::RenameSession => return self.open_prompt(client, PromptKind::Session),
            Command::OpenPrompt => return self.open_prompt(client, PromptKind::Command),
            Command::Ask(spec) => return self.open_ask(client, spec),
            Command::DisplayPanes => return self.display_panes(client),
            Command::CopyMode => return self.enter_copy_mode(client),
            Command::PasteBuffer => return self.paste_buffer(client),
            Command::Buffer(op) => return self.buffer_op(client, op),
            Command::SetWindowName(name) => return self.set_name(client, PromptKind::Window, name.clone()),
            Command::SetSessionName(name) => return self.set_name(client, PromptKind::Session, name.clone()),
            Command::SourceFile(path) => return self.source_for(client, path),
            Command::SourceWith(spec) => return self.source_with_for(client, spec),
            Command::ShowHooks => {
                let lines = self.hook_lines();
                return self.show_text(client, "hooks", lines);
            }
            Command::RunShell(r) => return self.run_shell(Some(client), r),
            Command::IfShell(i) => return self.if_shell(Some(client), i),
            Command::DisplayMessage(text) => {
                let shown = self.expand_message(client, text);
                return self.show_message(client, shown);
            }
            Command::ListKeys => {
                let lines = self.bindings.list_lines();
                return self.show_text(client, "keys", lines);
            }
            Command::ShowMessages => {
                let lines: Vec<String> = self.messages.iter().rev().cloned().collect();
                return self.show_text(client, "messages", lines);
            }
            Command::RefreshClient => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.renderer.invalidate();
                }
                self.dirty = true;
                return;
            }
            Command::ToggleZoom => return self.toggle_zoom(client, si),
            Command::BreakPane => return self.break_pane(client, si),
            Command::SwapPane(down) => return self.swap_pane(client, si, *down),
            Command::RotateWindow(down) => return self.rotate_window(client, si, *down),
            Command::NextLayout => return self.arrange(si, arrange::Arrange::Next),
            Command::LayoutPreset(p) => return self.arrange(si, arrange::Arrange::Preset(*p)),
            Command::SpreadLayout => return self.arrange(si, arrange::Arrange::Spread),
            Command::SelectLayout(kind) => {
                let preset = if *kind == Split::SideBySide { Preset::EvenHorizontal } else { Preset::EvenVertical };
                return self.arrange(si, arrange::Arrange::Preset(preset));
            }
            Command::FindWindow(None) => return self.open_prompt(client, PromptKind::Find),
            Command::FindWindow(Some(text)) => return self.find_window(client, text),
            Command::LastSession => return self.last_session(client),
            Command::SuspendClient => return self.queue(client, &ServerMsg::Suspend),
            Command::ChooseClient => return self.choose_client(client),
            Command::ClockMode => return self.open_clock(client),
            Command::DisplayMenu(spec) => return self.open_menu(client, spec),
            Command::DisplayPopup(spec) => {
                self.open_popup(client, spec, None);
                return;
            }
            Command::MarkPane(set) => return self.mark_pane(client, si, *set),
            Command::SwapWithMarked => return self.swap_with_marked(client, si),
            Command::WindowByNumber => return self.open_prompt(client, PromptKind::Index),
            Command::MoveWindowTo => return self.open_prompt(client, PromptKind::MoveTo),
            Command::ShowOptions => return self.show_options(client),
            Command::SetWindowOption { name, op, target } => return self.set_window_option_for(client, si, name, *op, target.as_deref()),
            Command::Persist(op) => {
                let text = self.persist_op(op).unwrap_or_else(|e| e);
                return self.show_message(client, text.trim_end().replace('\n', ", "));
            }
            Command::Worktree(args) => return self.start_worktree(worktree::Reply::Client(client), args.clone()),
            Command::WorktreeRemove(args) => return self.start_remove(worktree::Reply::Client(client), args.clone()),
            Command::Chain(list) => {
                let queue = list.iter().cloned().map(crate::config::stmt::Stmt::Run).collect();
                if let Err(e) = self.apply_queue(client, queue) {
                    self.show_message(client, e);
                }
                return;
            }
            Command::SwapWindow { src, dst, select } | Command::MoveWindow { src, dst, select } => {
                let base = self.base_index();
                let session = &mut self.sessions[si];
                let from = src.map_or(Some(session.current), |n| n.checked_sub(base));
                let done = match (from, dst.checked_sub(base)) {
                    (Some(from), Some(to)) if matches!(cmd, Command::SwapWindow { .. }) => session.swap(from, to, *select),
                    (Some(from), Some(to)) => session.move_to(from, to, *select),
                    _ => false,
                };
                if done {
                    self.dirty = true;
                } else {
                    self.show_message(client, "can't find window".to_string());
                }
                return;
            }
            Command::KillSession(target) => {
                let target = match target {
                    None => Some(si),
                    Some(name) => self.sessions.iter().position(|s| &s.name == name),
                };
                match target {
                    Some(ti) => self.kill_session(ti),
                    None => self.show_message(client, "can't find session".to_string()),
                }
                return;
            }
            Command::PreviousSession => return self.step_session(client, false),
            Command::NextSession => return self.step_session(client, true),
            Command::SplitWindow(kind) => return self.split_active(si, *kind),
            Command::SendPrefix => {
                let raw = self.bindings.prefix_raw.clone();
                return self.write_active(client, &raw);
            }
            Command::SendPrefix2 => {
                let Some(raw) = self.bindings.prefix2.as_ref().map(|(_, raw)| raw.clone()) else {
                    return self.show_message(client, "prefix2 is not set".to_string());
                };
                return self.write_active(client, &raw);
            }
            Command::NextAlertWindow | Command::PreviousAlertWindow => {
                let forward = matches!(cmd, Command::NextAlertWindow);
                if !self.sessions[si].select_alert(forward) {
                    self.show_message(client, format!("No {} window with an alert", if forward { "next" } else { "previous" }));
                }
                self.dirty = true;
                return;
            }
            Command::NextWindow | Command::PreviousWindow | Command::LastWindow | Command::SelectWindow(_) => {
                let base = self.base_index();
                let session = &mut self.sessions[si];
                match cmd {
                    Command::NextWindow => session.select_next(),
                    Command::PreviousWindow => session.select_previous(),
                    Command::LastWindow => session.select_last(),
                    Command::SelectWindow(n) => {
                        if let Some(index) = n.checked_sub(base) {
                            session.select(index);
                        }
                    }
                    _ => {}
                }
                self.dirty = true;
                return;
            }
            _ => {}
        }
        let win = self.sessions[si].window_mut();
        // Moving about or resizing panes brings the others back from a zoom.
        if matches!(cmd, Command::NextPane | Command::LastPane | Command::SelectPane(_) | Command::ResizePane(..) | Command::KillPane) {
            win.layout.unzoom();
        }
        match cmd {
            Command::NextPane => win.focus_next(),
            Command::LastPane => win.focus_last(),
            Command::SelectPane(dir) => win.focus_direction(*dir, area),
            Command::ResizePane(dir, cells) => {
                let active = win.active;
                win.layout.resize(active, *dir, *cells, area);
            }
            Command::KillPane => {
                let active = win.active;
                return self.close_pane(active, None);
            }
            _ => {}
        }
        self.relayout();
    }

    fn session_index(&self, sid: usize) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == sid)
    }

    /// Position in `sessions` of the session this client looks at.
    fn client_session(&self, client: usize) -> Option<usize> {
        self.session_index(self.clients.get(&client)?.session)
    }

    /// Point a client at a session and make it the one new clients join.
    fn attach(&mut self, client: usize, sid: usize) {
        if let Some(c) = self.clients.get_mut(&client) {
            if c.session != sid && c.session != usize::MAX {
                c.last_session = Some(c.session);
            }
            c.session = sid;
            c.mode = Mode::Normal;
            c.renderer.invalidate();
        }
        self.recent = sid;
        self.apply_size();
        self.dirty = true;
    }

    /// Lowest unused number as a name, like tmux's numbered sessions.
    fn free_session_name(&self) -> String {
        (0..).map(|n| n.to_string()).find(|name| !self.sessions.iter().any(|s| &s.name == name)).unwrap_or_default()
    }

    /// Create a session with one window, named `name` or else with the lowest unused number;
    /// returns its id.
    fn start_session(&mut self, cwd: Option<PathBuf>, name: Option<String>) -> io::Result<usize> {
        let Some(size) = self.size else { return Err(io::Error::other("no client size yet")) };
        let id = self.alloc_pane_id();
        let area = self.pane_area(size, self.first_status_rows());
        let pane = self.spawn_pane(id, area, cell_px(size), cwd.as_deref())?;
        let window = Window::new(default_window_name(&self.shell()), id, pane);
        let sid = self.next_session;
        self.next_session += 1;
        let name = name.unwrap_or_else(|| self.free_session_name());
        log::info(&format!("session {sid} ({name}) created"));
        self.sessions.push(Session::new(sid, name, window));
        self.dirty = true;
        Ok(sid)
    }

    fn new_session(&mut self, client: usize) {
        let cwd = self.client_session(client).and_then(|si| self.active_cwd(si));
        match self.start_session(cwd, None) {
            Ok(sid) => self.attach(client, sid),
            Err(e) => log::error(&format!("new session: {e}")),
        }
    }

    /// `name: 2 windows (attached)`, as shown by the session list and `imux ls`.
    fn session_summary(&self, s: &Session) -> String {
        let n = s.windows.len();
        let attached = self.clients.values().any(|c| c.session == s.id);
        format!("{}: {n} window{}{}", s.name, if n == 1 { "" } else { "s" }, if attached { " (attached)" } else { "" })
    }

    /// One line per session, for `imux ls`.
    fn listing(&self) -> String {
        self.sessions.iter().map(|s| self.session_summary(s) + "\n").collect()
    }

    /// Move a client to the next or previous session in creation order.
    fn step_session(&mut self, client: usize, forward: bool) {
        let Some(si) = self.client_session(client) else { return };
        let n = self.sessions.len();
        let to = if forward { (si + 1) % n } else { (si + n - 1) % n };
        if to != si {
            self.attach(client, self.sessions[to].id);
        }
    }

    /// Clients looking at a session that no longer exists move to the one most recently used.
    fn rehome_clients(&mut self) {
        let Some(fallback) = self.sessions.iter().find(|s| s.id == self.recent).or(self.sessions.first()).map(|s| s.id)
        else {
            return;
        };
        self.recent = fallback;
        let lost: Vec<usize> = self
            .clients
            .iter()
            .filter(|(_, c)| c.session != usize::MAX && self.session_index(c.session).is_none())
            .map(|(id, _)| *id)
            .collect();
        for id in lost {
            self.attach(id, fallback);
        }
    }

    fn alloc_pane_id(&mut self) -> PaneId {
        let id = self.next_pane;
        self.next_pane += 1;
        id
    }

    fn new_window(&mut self, si: usize) {
        let Some(size) = self.size_of(si) else { return };
        let id = self.alloc_pane_id();
        let cwd = self.active_cwd(si);
        let area = self.pane_area(size, self.sessions[si].status_rows);
        match self.spawn_pane(id, area, cell_px(size), cwd.as_deref()) {
            Ok(pane) => {
                let name = default_window_name(&self.shell());
                self.sessions[si].add(Window::new(name, id, pane));
                self.relayout();
            }
            Err(e) => log::error(&format!("new window: {e}")),
        }
    }

    fn kill_window(&mut self, si: usize) {
        let ids: Vec<PaneId> = self.sessions[si].window().panes.keys().copied().collect();
        for id in ids {
            self.close_pane(id, None);
        }
    }

    /// Close every pane of a session; clients looking at it move to another. Killing the only
    /// session ends the server, like closing its last pane.
    fn kill_session(&mut self, si: usize) {
        let ids: Vec<PaneId> = self.sessions[si].windows.iter().flat_map(|w| w.panes.keys().copied()).collect();
        for id in ids {
            self.close_pane(id, None);
        }
    }

    /// Working directory of the active pane's foreground process, for new panes and windows.
    fn active_cwd(&self, si: usize) -> Option<PathBuf> {
        let win = self.sessions.get(si)?.window();
        let pane = win.panes.get(&win.active)?;
        let pid = proc::foreground_pid(pane.pty.master.as_fd()).unwrap_or(pane.pty.child.as_raw());
        proc::cwd(pid)
    }

    /// Name each window after what its active pane is running (`automatic-rename on`).
    fn update_window_names(&mut self) {
        if !self.options.flag("automatic-rename") {
            return;
        }
        let fmt = self.options.text("automatic-rename-format").to_string();
        let mut changed = false;
        for win in self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).filter(|w| w.auto_rename) {
            let Some(pane) = win.panes.get(&win.active) else { continue };
            let Some(pid) = proc::foreground_pid(pane.pty.master.as_fd()) else { continue };
            let Some(command) = proc::command_name(pid) else { continue };
            let command = crate::sanitize::clean(&command);
            let name = crate::config::format::expand(&fmt, &|v| (v == "pane_current_command").then(|| command.clone()));
            if win.name != name {
                win.name = name;
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    /// The shell started in panes: `default-shell`, else `$SHELL`, else `/bin/sh`.
    /// What a pane gets for `tmux-compat`: `$TMUX` (`socket,pid,session`, the shape tools split on),
    /// `$TMUX_PANE`, and the shim directory first on `PATH` so a plain `tmux` is imux's. A shim that
    /// cannot be made is logged and left out; the two variables alone would only make tools try a
    /// real `tmux` against the wrong server.
    fn compat_env(&self, pane: PaneId, earlier: &[(String, String)]) -> Vec<(String, String)> {
        let shim = std::env::current_exe().and_then(|exe| paths::ensure_shim(&exe));
        let dir = match shim {
            Ok(dir) => dir,
            Err(e) => {
                log::warn(&format!("tmux-compat: no shim, {e}"));
                return Vec::new();
            }
        };
        let path = earlier.iter().rev().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
        let shim_first = if path.is_empty() { dir.to_string_lossy().into_owned() } else { format!("{}:{path}", dir.display()) };
        let socket = paths::socket_path();
        vec![
            ("TMUX".to_string(), format!("{},{},0", socket.display(), std::process::id())),
            ("TMUX_PANE".to_string(), format!("%{pane}")),
            ("PATH".to_string(), shim_first),
            ("IMUX_SHIM_DIR".to_string(), dir.to_string_lossy().into_owned()),
        ]
    }

    pub(super) fn shell(&self) -> String {
        shell::path(self.options.text("default-shell"))
    }

    /// Syntax to write for what is typed into panes: `shell-syntax`, or by the name of the shell.
    pub(super) fn shell_kind(&self) -> ShellKind {
        ShellKind::named(self.options.text("shell-syntax")).unwrap_or_else(|| ShellKind::of(&self.shell()))
    }

    fn spawn_pane(&mut self, id: PaneId, rect: Rect, cell: (u16, u16), cwd: Option<&Path>) -> io::Result<Pane> {
        self.spawn_pane_with(id, rect, cell, cwd, None)
    }

    /// `spawn_pane`, running `command` through the shell (`-c`) instead of an interactive shell.
    pub(super) fn spawn_pane_with(&mut self, id: PaneId, rect: Rect, cell: (u16, u16), cwd: Option<&Path>, command: Option<&str>) -> io::Result<Pane> {
        self.spawn_pane_env(id, rect, cell, cwd, command, &[])
    }

    /// `spawn_pane_with` with more variables for the program (`display-popup -e`); they come
    /// before imux's own, which cannot be overridden.
    pub(super) fn spawn_pane_env(
        &mut self,
        id: PaneId,
        rect: Rect,
        cell: (u16, u16),
        cwd: Option<&Path>,
        command: Option<&str>,
        more: &[(String, String)],
    ) -> io::Result<Pane> {
        let shell = self.shell();
        let size = pane_size(rect, cell);
        // What `set-environment` and `update-environment` say first; imux's own variables last, so
        // they cannot be overridden.
        let mut env: Vec<(String, String)> = self.pane_env.iter().filter_map(|(k, v)| Some((k.clone(), v.clone()?))).collect();
        let mut unset: Vec<String> = self.pane_env.iter().filter(|(_, v)| v.is_none()).map(|(k, _)| k.clone()).collect();
        env.extend(more.iter().filter(|(k, _)| !k.starts_with("IMUX_")).cloned());
        let inherited: Vec<String> = std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).collect();
        let stripped = environ::stripped_names(self.options.text("strip-environment"), &inherited, &|n| env.iter().any(|(k, _)| k == n));
        unset.extend(stripped);
        let compat = if self.options.flag("tmux-compat") { self.compat_env(id, &env) } else { Vec::new() };
        if compat.is_empty() {
            // The server may have been started inside another multiplexer: its `$TMUX` says nothing
            // about this pane.
            unset.extend(["TMUX", "TMUX_PANE"].iter().filter(|k| !env.iter().any(|(n, _)| n == *k)).map(|k| (*k).to_string()));
        }
        env.extend(compat);
        env.extend([
            ("IMUX_ENV".to_string(), "1".to_string()),
            ("IMUX_PANE_ID".to_string(), id.to_string()),
            ("IMUX_SOCKET_PATH".to_string(), paths::socket_path().to_string_lossy().into_owned()),
        ]);
        let pty = pty::spawn_command(&shell, command, &size.to_winsize(), cwd, &env, &unset)?;
        self.registry
            .register(&mut SourceFd(&pty.master.as_raw_fd()), Token(PANE_BASE + id as usize), Interest::READABLE)
            .map_err(|e| ctx(e, "register pty"))?;
        log::info(&format!("pane {id}: spawned {shell} pid={} size={}x{}", pty.child, rect.w, rect.h));
        let mut term = Terminal::new(rect.w as usize, rect.h as usize, self.options.num("history-limit") as usize);
        term.grid.cell_px = cell;
        term.set_decoder(self.decoder.handle.with_tag(u64::from(id)));
        Ok(Pane { pty, term, copy: None, agent: None, typeahead: None, dead: None, start: command.map(str::to_string), input_off: false })
    }

    fn split_active(&mut self, si: usize, kind: Split) {
        let Some(size) = self.size_of(si) else { return };
        let area = self.pane_area(size, self.sessions[si].status_rows);
        let new = self.alloc_pane_id();
        let cwd = self.active_cwd(si);
        let win = self.sessions[si].window_mut();
        win.layout.unzoom();
        let target = win.active;
        if let Err(e) = win.layout.split(target, kind, new, area) {
            log::info(&format!("split refused: {e}"));
            return;
        }
        let rect = win.layout.rects(area).into_iter().find(|(id, _)| *id == new).map(|(_, r)| r);
        let spawned = match rect {
            Some(rect) => self.spawn_pane(new, self.content_rect(rect), cell_px(size), cwd.as_deref()),
            None => Err(io::Error::other("new pane has no rect")),
        };
        let win = self.sessions[si].window_mut();
        match spawned {
            Ok(pane) => {
                win.panes.insert(new, pane);
                win.focus(new);
            }
            Err(e) => {
                log::error(&format!("split: {e}"));
                win.layout.remove(new);
            }
        }
        self.relayout();
    }

    /// The size `window-size` makes of some clients: the smallest (default), the largest, the one
    /// that was last used, or `default-size` whatever the clients are (`manual`).
    fn pick_size<'a>(&self, clients: impl Iterator<Item = &'a Client>) -> Option<Size> {
        let mut sized: Vec<(&Client, Size)> = clients.filter_map(|c| Some((c, c.size?))).collect();
        match self.options.text("window-size") {
            "largest" => largest(sized.into_iter().map(|(_, s)| s)),
            "latest" => {
                sized.sort_by_key(|(c, _)| c.last_input);
                sized.last().map(|(_, s)| *s)
            }
            "manual" => (!sized.is_empty()).then(|| self.default_size()),
            _ => smallest(sized.into_iter().map(|(_, s)| s)),
        }
    }

    /// The size of all attached clients together, by `window-size`.
    fn effective_size(&self) -> Option<Size> {
        self.pick_size(self.clients.values())
    }

    /// The size of the clients looking at session `sid`, if any.
    fn attached_size(&self, sid: usize) -> Option<Size> {
        self.pick_size(self.clients.values().filter(|c| c.session == sid && !c.closing))
    }

    /// Whether `apply_size` would give a session another size than it has.
    fn size_would_change(&self) -> bool {
        self.sessions.iter().any(|s| self.attached_size(s.id).is_some_and(|z| self.session_sizes.get(&s.id) != Some(&z)))
    }

    /// The size the panes of session `sid` are laid out for.
    fn size_for(&self, sid: usize) -> Option<Size> {
        self.session_sizes.get(&sid).copied().or(self.size)
    }

    /// The same for the session at position `si`.
    fn size_of(&self, si: usize) -> Option<Size> {
        self.size_for(self.sessions.get(si)?.id)
    }

    /// Take the client sizes into account: each session follows the smallest client looking at it
    /// and keeps its last size when nobody does.
    fn apply_size(&mut self) {
        if let Some(eff) = self.effective_size() {
            self.size = Some(eff);
        }
        let ids: Vec<usize> = self.sessions.iter().map(|s| s.id).collect();
        for sid in &ids {
            if let Some(size) = self.attached_size(*sid) {
                self.session_sizes.insert(*sid, size);
            }
        }
        self.session_sizes.retain(|sid, _| ids.contains(sid));
        self.relayout();
    }

    /// Recompute how many rows each session's status bar needs at its own width.
    fn refresh_status_rows(&mut self) {
        let cfg = self.status_cfg();
        let segments = self.segment_view(false);
        let agents: Vec<_> = (0..self.sessions.len()).map(|si| self.agent_states(si, None)).collect();
        let widths: Vec<Option<u16>> = self.sessions.iter().map(|s| self.size_for(s.id).map(|z| z.cols)).collect();
        let on = self.status_on();
        for ((session, agents), width) in self.sessions.iter_mut().zip(&agents).zip(widths) {
            let Some(cols) = width else { continue };
            session.status_rows = if on { status_with(session, usize::from(cols), 1, &cfg, agents, &segments).rows as u16 } else { 0 };
        }
    }

    /// Give every pane the size its rect now has.
    fn relayout(&mut self) {
        self.refresh_status_rows();
        let dock = self.dock();
        let sizes: Vec<Option<Size>> = self.sessions.iter().map(|s| self.size_for(s.id)).collect();
        let top = self.status_top();
        let label = self.pane_status();
        for (session, size) in self.sessions.iter_mut().zip(sizes) {
            let Some(size) = size else { continue };
            let cell = cell_px(size);
            let area = regions::layout_regions_at(size, session.status_rows, top, dock).0;
            for win in &mut session.windows {
                for (id, rect) in win.layout.rects(area) {
                    let rect = label.content(rect);
                    if rect.w == 0 || rect.h == 0 {
                        continue;
                    }
                    let Some(pane) = win.panes.get_mut(&id) else { continue };
                    let g = &pane.term.grid;
                    if g.cols == rect.w as usize && g.rows == rect.h as usize && g.cell_px == cell {
                        continue;
                    }
                    if let Err(e) =
                        pty::set_window_size(pane.pty.master.as_fd(), &pane_size(rect, cell).to_winsize())
                    {
                        log::warn(&format!("pane {id}: resize pty: {e}"));
                    }
                    pane.term.grid.resize(rect.w as usize, rect.h as usize, cell);
                }
                popup::fit_popup(win, area, cell);
            }
        }
        self.dirty = true;
    }

    /// The dock to lay out, when it is open.
    fn dock(&self) -> Option<regions::Dock> {
        if !self.dock_open {
            return None;
        }
        let (pos, size) = match self.options.text("agent-dock-position") {
            "left" => (regions::DockPos::Left, self.options.num("agent-dock-width")),
            "top" => (regions::DockPos::Top, self.options.num("agent-dock-height")),
            "bottom" => (regions::DockPos::Bottom, self.options.num("agent-dock-height")),
            _ => (regions::DockPos::Right, self.options.num("agent-dock-width")),
        };
        Some(regions::Dock { pos, size: size as u16 })
    }

    /// Whether the status line is shown (`status`).
    fn status_on(&self) -> bool {
        self.options.text("status") != "off"
    }

    /// Whether the status line is at the top of the screen (`status-position`).
    fn status_top(&self) -> bool {
        self.status_on() && self.options.text("status-position") == "top"
    }

    /// Status rows to assume before a bar has been measured: one, or none with `status off`.
    fn first_status_rows(&self) -> u16 {
        u16::from(self.status_on())
    }

    /// The part of a `size` screen the panes occupy: beside the status rows and the dock.
    fn pane_area(&self, size: Size, status_rows: u16) -> Rect {
        regions::layout_regions_at(size, status_rows, self.status_top(), self.dock()).0
    }

    /// Where the dock is drawn, when it is open and fits.
    fn dock_rect(&self, size: Size, status_rows: u16) -> Option<Rect> {
        regions::layout_regions_at(size, status_rows, self.status_top(), self.dock()).1
    }

    /// The screen row of the message line and the `:` prompt: the status line's row, which is the
    /// last row of the screen unless the bar is on top; with no bar the last row of the panes.
    fn message_row(&self, rows: u16) -> Option<usize> {
        let row = if self.status_top() { 0 } else { usize::from(rows).checked_sub(1)? };
        Some(row)
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.sessions.iter_mut().find_map(|s| s.pane_mut(id))
    }

    /// A finished pane that a client has in front of it counts as seen by that client.
    fn mark_seen(&mut self) {
        let watching: Vec<(usize, usize)> =
            self.clients.iter().filter(|(_, c)| c.size.is_some() && !c.closing).map(|(id, c)| (*id, c.session)).collect();
        for (client, sid) in watching {
            let Some(session) = self.sessions.iter_mut().find(|s| s.id == sid) else { continue };
            let win = session.window_mut();
            let active = win.active;
            if let Some(record) = win.panes.get_mut(&active).and_then(|p| p.agent.as_mut()) {
                record.seen_by.insert(client);
            }
        }
    }

    /// A `working` or `blocked` record lives while the program runs: drop it once the pane's
    /// foreground process is the shell again (killed, crashed, or left without saying so).
    fn expire_agents(&mut self) {
        let mut changed = false;
        let mut ended = Vec::new();
        for (id, pane) in self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).flat_map(|w| w.panes.iter_mut()) {
            if pane.agent.is_some() && proc::foreground_pid(pane.pty.master.as_fd()) == Some(pane.pty.child.as_raw()) {
                changed |= agent::program_ended(&mut pane.agent);
                changed |= agent::drop_resume(&mut pane.agent);
                ended.push(*id);
            }
        }
        for id in ended {
            self.forget_start_name(id);
        }
        if changed {
            self.dirty = true;
        }
    }

    /// Drain a pane's pty into its emulator. Returns true at end of file.
    fn read_pane_data(&mut self, id: PaneId) -> bool {
        let mut buf = std::mem::take(&mut self.scratch);
        let mut eof = false;
        let mut total = 0;
        let mut pipe_failure: Option<String> = None;
        if let Some(pane) = self.sessions.iter_mut().find_map(|s| s.pane_mut(id)) {
            loop {
                // A picture of this pane is being decoded: what follows waits in the pty until
                // `finish_decodes` has put the pane back in the backlog.
                if pane.term.is_held() {
                    break;
                }
                if total >= PTY_BUDGET {
                    self.backlog.insert(id);
                    break;
                }
                match read_some(pane.pty.master.as_fd(), &mut buf) {
                    Ok(Some(0)) => {
                        eof = true;
                        break;
                    }
                    Ok(Some(n)) => {
                        total += n;
                        pane.term.feed(&buf[..n]);
                        if let Some(t) = &mut pane.typeahead {
                            t.saw(&buf[..n]);
                        }
                        if pipe_failure.is_none() {
                            pipe_failure = self.pipes.feed(id, &buf[..n]);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        log::error(&format!("pane {id}: read pty: {e}"));
                        eof = true;
                        break;
                    }
                }
            }
        }
        self.scratch = buf;
        if let Some(why) = pipe_failure {
            self.pipe_failed(id, &why);
        }
        if total > 0 {
            self.dirty = true;
            self.note_activity(id);
            if !self.output_waiters.is_empty() {
                self.output_touched.insert(id);
            }
        }
        self.drain_terminal(id);
        eof
    }

    /// Hand the decoder's answers to the panes that wait for them, and read on in the panes that
    /// are free again.
    fn finish_decodes(&mut self) {
        while let Ok(done) = self.decoder.done.try_recv() {
            let id = PaneId::try_from(done.tag).unwrap_or(PaneId::MAX);
            let Some(pane) = self.pane_mut(id) else { continue };
            if !pane.term.finish_decode(done.seq, done.result) {
                continue;
            }
            let free = !pane.term.is_held();
            self.dirty = true;
            self.drain_terminal(id);
            if free {
                self.backlog.insert(id);
            }
        }
    }

    fn read_pane(&mut self, id: PaneId) {
        if self.read_pane_data(id) {
            self.pane_exited(id, None);
        }
    }

    /// Send what the emulator produced besides screen contents: replies go back to the program,
    /// clipboard and bell go out to the attached terminals.
    fn drain_terminal(&mut self, id: PaneId) {
        let Some(pane) = self.pane_mut(id) else { return };
        let replies = pane.term.grid.take_replies();
        let events = pane.term.grid.take_events();
        // Answers to a program that does not read are dropped; one log line when it stops, not
        // one per turn while it stays stopped.
        if !replies.is_empty()
            && let Err(e) = pane.pty.write(&replies)
            && e.kind() != std::io::ErrorKind::WouldBlock
        {
            log::warn(&format!("pane {id}: write reply to pty: {e}"));
        }
        let mut belled = false;
        for event in events {
            let bytes = match event {
                // One bell per read turn, however many a program printed (security review P-12).
                Event::Bell if belled => continue,
                Event::Bell => {
                    belled = true;
                    match self.bell_in(id) {
                        Some(bytes) => bytes,
                        None => continue,
                    }
                }
                Event::Clipboard { target, data } => {
                    self.pane_clipboard(id, &target, &data);
                    continue;
                }
                Event::Title(_) | Event::Cwd(_) => continue,
                Event::Agent(report) => {
                    if self.pane_mut(id).is_some_and(|p| agent::apply(&mut p.agent, report, Instant::now())) {
                        self.dirty = true;
                        self.adopt_pending_name(id);
                    }
                    continue;
                }
                Event::PromptStart => {
                    if self.pane_mut(id).is_some_and(|p| agent::prompt_started(&mut p.agent)) {
                        self.dirty = true;
                    }
                    // the shell is asking for a command: the program `agent start` typed is over
                    self.forget_start_name(id);
                    continue;
                }
            };
            self.broadcast(&ServerMsg::Output(bytes));
        }
    }

    /// Remove a pane whose program exited (`reaped` is its exit code if already collected) or
    /// that the user closed. Closing the last pane ends the server.
    fn close_pane(&mut self, id: PaneId, reaped: Option<i32>) {
        if self.is_popup_pane(id) {
            return self.close_popup(id, reaped);
        }
        self.close_popup_with_window(id);
        let Some(si) = self.sessions.iter().position(|s| s.window_of(id).is_some()) else { return };
        let sole_session = self.sessions.len() == 1;
        let session = &mut self.sessions[si];
        let Some(index) = session.window_of(id) else { return };
        let last_in_window = session.windows[index].tiled() == 1;
        // The last pane of a window takes the window with it, and the last window its session;
        // only the last pane of the last session ends the server.
        let pane = if !last_in_window {
            match session.windows[index].remove(id) {
                Some(pane) => pane,
                None => return,
            }
        } else if session.windows.len() > 1 {
            match session.remove(index).and_then(|mut w| w.panes.remove(&id)) {
                Some(pane) => pane,
                None => return,
            }
        } else if !sole_session {
            let dead = self.sessions.remove(si);
            log::info(&format!("session {} ({}) ended", dead.id, dead.name));
            match dead.windows.into_iter().next().and_then(|mut w| w.panes.remove(&id)) {
                Some(pane) => pane,
                None => return,
            }
        } else {
            let code = match reaped {
                Some(code) => code,
                None => session.windows[index].panes.get(&id).map_or(0, |p| {
                    if let Some(code) = p.dead {
                        return code;
                    }
                    hang_up(p.pty.child);
                    pty::wait_blocking(p.pty.child).unwrap_or_else(|e| {
                        log::warn(&format!("wait child: {e}"));
                        0
                    })
                }),
            };
            self.finish(code);
            return;
        };
        if self.mouse_target == Some(id) {
            self.mouse_target = None;
        }
        // A dead pane (remain-on-exit) left the poll and had its child collected already.
        if pane.dead.is_none()
            && let Err(e) = self.registry.deregister(&mut SourceFd(&pane.pty.master.as_raw_fd()))
        {
            log::debug(&format!("pane {id}: deregister pty: {e}"));
        }
        self.backlog.remove(&id);
        self.pipes.close(id, true);
        let exited = pane.dead.is_some() || reaped.is_some() || matches!(pty::try_reap(pane.pty.child), Ok(Some(_)));
        if !exited {
            hang_up(pane.pty.child);
            self.zombies.push(pane.pty.child);
        }
        log::info(&format!("pane {id} closed"));
        self.rehome_clients();
        self.relayout();
    }

    fn on_signals(&mut self, signals: &SignalPipe) -> io::Result<()> {
        for sig in signals.drain()? {
            match sig {
                Signal::SIGCHLD => self.reap_children(),
                Signal::SIGTERM => {
                    log::info("SIGTERM, shutting down");
                    self.final_save();
                    self.shutdown = true;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn reap_children(&mut self) {
        self.zombies.retain(|&pid| matches!(pty::try_reap(pid), Ok(None)));
        let children: Vec<(PaneId, Pid)> = self
            .sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.iter().filter(|(_, p)| p.dead.is_none()).map(|(id, p)| (*id, p.pty.child)))
            .collect();
        for (id, pid) in children {
            match pty::try_reap(pid) {
                Ok(Some(code)) => {
                    self.read_pane_data(id);
                    self.pane_exited(id, Some(code));
                }
                Ok(None) => {}
                Err(e) => log::warn(&format!("pane {id}: reap child: {e}")),
            }
        }
    }

    /// The last pane is gone: show the final screen, tell the clients, stop.
    fn finish(&mut self, code: i32) {
        log::info(&format!("last pane exited code={code}"));
        self.render();
        self.broadcast(&ServerMsg::Exit(code));
        let ids: Vec<usize> = self.clients.keys().copied().collect();
        for id in ids {
            self.close_after_flush(id);
        }
        self.sessions.clear();
        self.discard_autosave();
        self.shutdown = true;
    }

    /// Compose and diff one frame per attached client.
    fn render(&mut self) {
        self.dirty = false;
        self.last_render = Instant::now();
        let before: Vec<u16> = self.sessions.iter().map(|s| s.status_rows).collect();
        self.refresh_status_rows();
        if self.sessions.iter().map(|s| s.status_rows).ne(before) {
            self.relayout();
            self.dirty = false;
        }
        let Some(eff) = self.size else { return };
        let labels: HashMap<usize, Vec<String>> = self
            .clients
            .iter()
            .filter_map(|(id, c)| match &c.mode {
                Mode::Picker(p) => Some((*id, self.picker_labels(p, *id))),
                _ => None,
            })
            .collect();
        let titles: HashMap<usize, String> = self
            .clients
            .iter()
            .filter(|(_, c)| matches!(&c.mode, Mode::Picker(p) if c.filter.as_ref().map_or(&p.entries[..], |f| &f.all[..]).iter().any(|e| matches!(e, modes::Entry::Text(_)))))
            .map(|(id, c)| (*id, c.text.0.clone()))
            .collect();
        let footers: HashMap<usize, String> = self
            .clients
            .iter()
            .filter_map(|(id, c)| match (&c.mode, &c.filter) {
                (Mode::Picker(p), Some(f)) => Some((*id, f.footer(p.entries.len(), f.all.len()))),
                _ => None,
            })
            .collect();
        let agent_titles: HashMap<usize, String> = self.clients.keys().map(|id| (*id, self.agent_title(*id))).collect();
        let pane_base = self.pane_base_index();
        let cfg = self.status_cfg();
        let segments = self.segment_view(true);
        let (borders, mode_style, mouse_on) = (self.border_styles(), self.mode_style(), self.options.flag("mouse"));
        let look = self.pane_look();
        let label_format = self.options.text("pane-border-format").to_string();
        let border_labels: HashMap<usize, Vec<(PaneId, Vec<status::Run>)>> = if look.status == render::PaneStatus::Off {
            HashMap::new()
        } else {
            self.sessions
                .iter()
                .enumerate()
                .map(|(si, s)| {
                    let (wi, win) = (s.current, s.window());
                    let list = win
                        .layout
                        .panes()
                        .into_iter()
                        .map(|pane| {
                            let text = self.expand_at(remote::Place { si, wi, pane }, &label_format);
                            let mut runs = crate::config::format::runs(&text, if pane == win.active { borders.1 } else { borders.0 });
                            for (t, _) in &mut runs {
                                *t = crate::config::format::restore_hash(t);
                            }
                            (pane, runs)
                        })
                        .collect();
                    (s.id, list)
                })
                .collect()
        };
        let pane_colors = (
            self.options.style("display-panes-color").fg.unwrap_or_default(),
            self.options.style("display-panes-active-color").fg.unwrap_or_default(),
        );
        let focus_on = self.options.flag("focus-events");
        let band_rows = usize::try_from(self.options.num("image-band-rows")).unwrap_or(0);
        let kitty_source = self.options.text("image-kitty-placement") == "source";
        let image_scroll = self.options.flag("image-scroll");
        let dither = self.options.text("image-dither") == "ordered";
        render::quad::set_glyphs(render::quad::Glyphs::parse(self.options.text("image-glyphs")));
        let (clock_text, clock_suffix) = self.clock_face();
        let clock_color = self.options.style("clock-mode-color").fg.unwrap_or_default();
        let image_override = match self.options.text("image-mode") {
            "blocks" => Some(ImageMode::Blocks),
            "iterm2" => Some(ImageMode::Iterm2),
            "kitty" => Some(ImageMode::Kitty),
            "sixel" => Some(ImageMode::Sixel),
            _ => None,
        };
        self.mark_seen();
        let dock = self.dock();
        let (bar_on, bar_top) = (self.status_on(), self.status_top());
        let message_rows: HashMap<usize, usize> =
            self.clients.iter().filter_map(|(id, c)| Some((*id, self.message_row(c.size?.rows)?))).collect();
        let agent_draws: HashMap<usize, agents::AgentDraw> =
            self.clients.keys().filter_map(|id| Some((*id, self.agent_view(*id)?))).collect();
        let dock_draws: HashMap<usize, agents::DockDraw> =
            self.clients.iter().filter_map(|(id, c)| Some((*id, self.dock_view(*id, self.session_index(c.session)?)?))).collect();
        let agent_states: HashMap<usize, conf::AgentStates> = self
            .clients
            .iter()
            .filter_map(|(id, c)| Some((*id, self.agent_states(self.session_index(c.session)?, Some(*id)))))
            .collect();
        let mut frames = Vec::new();
        let titles_on = self.options.flag("set-titles");
        let title_format = self.options.text("set-titles-string").to_string();
        let window_base = self.base_index();
        let host_name = conf::host();
        let session_sizes = self.session_sizes.clone();
        for (id, c) in self.clients.iter_mut() {
            let Some(size) = c.size else { continue };
            if c.closing {
                continue;
            }
            let Some(session) = self.sessions.iter().find(|s| s.id == c.session) else { continue };
            let win = session.window();
            let area = regions::layout_regions_at(session_sizes.get(&session.id).copied().unwrap_or(eff), session.status_rows, bar_top, dock).0;
            let rects = win.layout.rects(area);
            let seps = win.layout.separators(area);
            let copy_views: HashMap<PaneId, copymode::View> = win
                .panes
                .iter()
                .filter_map(|(id, p)| p.copy.as_ref().map(|m| (*id, m.view_with(&p.term.grid, mode_style))))
                .collect();
            let views: Vec<PaneView> = rects
                .iter()
                .filter_map(|(id, rect)| {
                    win.panes.get(id).map(|p| PaneView { rect: *rect, grid: &p.term.grid, active: *id == win.active, copy: copy_views.get(id) })
                })
                .collect();
            let no_agents = conf::AgentStates::default();
            let agents = agent_states.get(id).unwrap_or(&no_agents);
            let bar = status_with(session, size.cols as usize, usize::from(session.status_rows), &cfg, agents, &segments);
            let (bar_cells, bar_rows) = if bar_on { (&bar.cells[..], bar.rows) } else { (&[][..], 0) };
            render::quad::set_dither(dither && !c.renderer.caps().truecolor);
            let marker = win.popup.as_ref().and_then(|p| p.origin).map(|o| format!("[from %{o}]")).unwrap_or_default();
            let popup_view = win.popup.as_ref().and_then(|p| {
                let pane = win.panes.get(&p.pane)?;
                Some(render::PopupView {
                    rect: (usize::from(area.x) + p.x, usize::from(area.y) + p.y, p.w, p.h),
                    grid: &pane.term.grid,
                    lines: &p.lines,
                    title: &p.title,
                    marker: &marker,
                    border: p.styles.1,
                    base: p.styles.0,
                })
            });
            let mut screen = render::compose_window_full(
                &views,
                &seps,
                size.cols as usize,
                size.rows as usize,
                bar_cells,
                bar_rows,
                bar_top,
                borders,
                look,
                crate::proto::pick_mode(image_override, c.modes, c.renderer.caps().images),
                popup_view.as_ref(),
            );
            if let Some(list) = border_labels.get(&session.id) {
                for (pane, runs) in list {
                    let Some((_, r)) = rects.iter().find(|(p, _)| p == pane) else { continue };
                    if let Some(row) = look.status.label_row(*r) {
                        let border = if *pane == win.active { borders.1 } else { borders.0 };
                        overlay::draw_pane_label(&mut screen, usize::from(r.x), row, usize::from(r.w), look.lines.across(), border, runs);
                    }
                }
            }
            if let Some(d) = dock_draws.get(id) {
                render::agentview::draw_dock(&mut screen, d.rect, d.edge, agent_titles.get(id).map_or("agents", String::as_str), &d.rows, d.selected);
            }
            match &c.mode {
                Mode::Normal => {
                    if let (Some((text, _)), Some(y)) = (&c.message, message_rows.get(id).copied()) {
                        let cols = usize::from(size.cols);
                        let (row, _) = status::prompt_row_styled(cols, "imux", text, cfg.message.0, cfg.message.1);
                        screen.cells[y * cols..(y + 1) * cols].copy_from_slice(&row);
                    }
                }
                Mode::Panes(_) => {
                    let labels: Vec<_> = win
                        .layout
                        .rects(area)
                        .iter()
                        .enumerate()
                        .map(|(i, (pane, r))| {
                            (
                                i + pane_base,
                                usize::from(r.x),
                                usize::from(r.y),
                                usize::from(r.w),
                                usize::from(r.h),
                                *pane == win.active,
                            )
                        })
                        .collect();
                    overlay::draw_pane_numbers(&mut screen, &labels, pane_colors);
                }
                Mode::Clock(_) => {
                    if let Some((_, r)) = rects.iter().find(|(pane, _)| *pane == win.active) {
                        let place = (usize::from(r.x), usize::from(r.y), usize::from(r.w), usize::from(r.h));
                        overlay::draw_clock(&mut screen, place, &clock_text, &clock_suffix, clock_color);
                    }
                }
                Mode::Agents(panel) if !panel.docked => {
                    if let Some(d) = agent_draws.get(id) {
                        render::agentview::draw(&mut screen, d.shape, agent_titles.get(id).map_or("agents", String::as_str), &d.rows, d.selected, d.preview_lines, &d.preview);
                    }
                }
                Mode::Agents(_) => {}
                Mode::Menu(state) => {
                    if let Some(origin) = state.origin(usize::from(size.cols), usize::from(area.y), usize::from(area.h)) {
                        overlay::draw_menu(&mut screen, &state.look(origin), &state.menu.rows());
                    }
                }
                Mode::Picker(p) => {
                    let every: &[modes::Entry] = c.filter.as_ref().map_or(&p.entries[..], |f| &f.all[..]);
                    let windows = every.iter().any(|e| matches!(e, modes::Entry::Window(..)));
                    let repos = every.iter().any(|e| matches!(e, modes::Entry::Repo(_)));
                    let title = match (repos, windows) {
                        _ if titles.contains_key(id) => titles[id].as_str(),
                        (true, _) => "repositories",
                        (_, true) => "sessions and windows",
                        _ => "sessions",
                    };
                    let pane_rows = usize::from(size.rows).saturating_sub(bar_rows);
                    let first_row = if bar_top { bar_rows } else { 0 };
                    let footer = footers.get(id).map_or("", String::as_str);
                    overlay::draw_at(&mut screen, first_row, pane_rows, title, footer, labels.get(id).map_or(&[][..], Vec::as_slice), p.selected);
                }
                Mode::Prompt(p) => {
                    let asked = c.ask.as_ref().map(|a| {
                        let text = a.label.clone().unwrap_or_else(|| if a.confirm { format!("Confirm '{}'?", a.template) } else { String::new() });
                        if a.confirm { format!("{text} (y/n) ") } else if text.is_empty() { ": ".to_string() } else { format!("{text} ") }
                    });
                    let label = match p.kind {
                        PromptKind::Ask => asked.as_deref().unwrap_or(": "),
                        PromptKind::Window => "rename-window: ",
                        PromptKind::Session => "rename-session: ",
                        PromptKind::Command => ": ",
                        PromptKind::Worktree => "worktree name [slug]: ",
                        PromptKind::Find => "find-window: ",
                        PromptKind::Index => "index: ",
                        PromptKind::MoveTo => "move-window to: ",
                    };
                    let cols = usize::from(size.cols);
                    let (row, col) = status::prompt_row_styled(cols, label, &p.string(), cfg.command.0, cfg.command.1);
                    if let Some(y) = message_rows.get(id).copied() {
                        screen.cells[y * cols..(y + 1) * cols].copy_from_slice(&row);
                        screen.cursor = Some((col, y));
                    }
                }
            }
            // imux handles the mouse itself (focus, later copy mode) and relays to programs that ask.
            if mouse_on {
                screen.modes.mouse = if screen.modes.mouse == MouseMode::Any { MouseMode::Any } else { MouseMode::Drag };
            }
            screen.modes.mouse_sgr = true;
            // With `focus-events` imux asks the terminal for focus reports whatever the pane wants;
            // without it nobody gets them.
            screen.modes.focus_events = focus_on;
            c.renderer.set_band(band_rows);
            c.renderer.set_kitty_source(kitty_source);
            c.renderer.set_scroll(image_scroll);
            c.renderer.set_dither(dither);
            let mut bytes = c.renderer.frame(screen);
            if titles_on {
                let pane_title = win.panes.get(&win.active).map_or(String::new(), |p| crate::sanitize::clean(&p.term.grid.title));
                let lookup = |name: &str| match name {
                    "S" | "session_name" => Some(session.name.clone()),
                    "I" | "window_index" => Some((session.current + window_base).to_string()),
                    "W" | "window_name" => Some(win.name.clone()),
                    "T" | "pane_title" => Some(if pane_title.is_empty() { host_name.clone() } else { pane_title.clone() }),
                    "H" | "host" => Some(host_name.clone()),
                    "h" | "host_short" => Some(host_name.split('.').next().unwrap_or_default().to_string()),
                    _ => None,
                };
                let title: String = crate::sanitize::clean(&crate::config::format::expand(&crate::config::format::strftime(&title_format), &lookup))
                    .chars()
                    .take(MAX_TERMINAL_TITLE)
                    .collect();
                if title != c.title {
                    bytes.extend_from_slice(format!("\x1b]2;{title}\x1b\\").as_bytes());
                    c.title = title;
                }
            }
            if !bytes.is_empty() {
                frames.push((*id, bytes));
            }
        }
        for (id, bytes) in frames {
            self.queue(id, &ServerMsg::Output(bytes));
        }
    }

    fn broadcast(&mut self, msg: &ServerMsg) {
        let mut wire = Vec::new();
        proto::frame(msg, &mut wire);
        let ids: Vec<usize> =
            self.clients.iter().filter(|(_, c)| c.size.is_some() && !c.closing).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(c) = self.clients.get_mut(&id) {
                c.outbuf.extend_from_slice(&wire);
            }
            self.flush_client(id);
        }
    }

    fn queue(&mut self, id: usize, msg: &ServerMsg) {
        if let Some(c) = self.clients.get_mut(&id) {
            proto::frame(msg, &mut c.outbuf);
        }
        self.flush_client(id);
    }

    fn close_after_flush(&mut self, id: usize) {
        if let Some(c) = self.clients.get_mut(&id) {
            c.closing = true;
        }
        self.flush_client(id);
    }

    fn flush_client(&mut self, id: usize) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let mut drop_it = false;
        while !c.outbuf.is_empty() {
            match c.stream.write(&c.outbuf) {
                Ok(0) => {
                    drop_it = true;
                    break;
                }
                Ok(n) => {
                    c.outbuf.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    log::info(&format!("write client {id}: {e}"));
                    drop_it = true;
                    break;
                }
            }
        }
        if c.outbuf.len() > MAX_OUTBUF {
            log::warn(&format!("client {id} too slow, dropping"));
            drop_it = true;
        }
        if drop_it || (c.closing && c.outbuf.is_empty()) {
            self.remove_client(id);
        }
    }

    fn remove_client(&mut self, id: usize) {
        if let Some(mut c) = self.clients.remove(&id) {
            if let Err(e) = self.registry.deregister(&mut c.stream) {
                log::debug(&format!("deregister client {id}: {e}"));
            }
            log::info(&format!("client {id} disconnected"));
            self.drop_waiters_of(id);
            self.drop_output_waiters_of(id);
            self.drop_start_waiters_of(id);
            self.drop_popup_waiter(id);
            self.drop_subscriber(id);
            self.forget_plugin_peer(id);
            self.apply_size();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cols: u16, rows: u16, cw: u16, ch: u16) -> Size {
        Size { cols, rows, xpix: cols * cw, ypix: rows * ch }
    }

    #[test]
    fn the_smallest_size_takes_the_least_columns_and_rows_and_the_narrowest_cells() {
        assert_eq!(smallest(std::iter::empty()), None);
        assert_eq!(smallest([size(100, 30, 8, 16)].into_iter()), Some(size(100, 30, 8, 16)));
        let both = smallest([size(120, 30, 8, 16), size(80, 40, 10, 20)].into_iter());
        assert_eq!(both, Some(size(80, 30, 10, 20)));
        let other_way = smallest([size(80, 40, 10, 20), size(120, 30, 8, 16)].into_iter());
        assert_eq!(other_way, both);
    }

    #[test]
    fn the_largest_size_takes_the_most_columns_and_rows_and_the_widest_cells() {
        assert_eq!(largest(std::iter::empty()), None);
        assert_eq!(largest([size(100, 30, 8, 16)].into_iter()), Some(size(100, 30, 8, 16)));
        let both = largest([size(120, 30, 8, 16), size(80, 40, 10, 20)].into_iter());
        assert_eq!(both, Some(size(120, 40, 8, 16)));
        let other_way = largest([size(80, 40, 10, 20), size(120, 30, 8, 16)].into_iter());
        assert_eq!(other_way, both);
    }

    #[test]
    fn server_starts_wait_for_each_other() {
        let dir = std::env::temp_dir().join(format!("imux-startlock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = lock_start(&dir).unwrap();
        let started = Instant::now();
        let other = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let _second = lock_start(&dir).unwrap();
                started.elapsed()
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        drop(first);
        let waited = other.join().unwrap();
        assert!(waited >= Duration::from_millis(250), "the second start did not wait: {waited:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_panic_in_guarded_work_is_contained() {
        assert_eq!(guarded("fine", || 7), Some(7));
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let lost = guarded("boom", || -> u8 { panic!("deliberate") });
        let lost_string = guarded("boom", || -> u8 { panic!("{}", String::from("formatted")) });
        std::panic::set_hook(previous);
        assert_eq!(lost, None);
        assert_eq!(lost_string, None);
    }

    #[test]
    fn panic_payloads_become_text() {
        assert_eq!(log::panic_text(&"static text"), "static text");
        assert_eq!(log::panic_text(&String::from("owned")), "owned");
        assert_eq!(log::panic_text(&42u8), "unknown panic");
    }

    fn mouse(code: u16, x: u16, y: u16, release: bool) -> Input {
        Input::Mouse(Mouse { code, x, y, release })
    }

    fn same(a: &[Input], b: &[Input]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|pair| match pair {
                (Input::Keys(x), Input::Keys(y)) => x == y,
                (Input::Mouse(x), Input::Mouse(y)) => x == y,
                _ => false,
            })
    }

    #[test]
    fn plain_keys_pass_through() {
        let mut pending = Vec::new();
        let got = split_input(&mut pending, b"ls\x1b[A\x1b");
        assert!(same(&got, &[Input::Keys(b"ls\x1b[A\x1b".to_vec())]));
        assert!(pending.is_empty());
    }

    #[test]
    fn mouse_reports_are_cut_out_of_the_key_stream() {
        let mut pending = Vec::new();
        let got = split_input(&mut pending, b"a\x1b[<0;10;5Mb\x1b[<0;10;5mc");
        let want = [
            Input::Keys(b"a".to_vec()),
            mouse(0, 9, 4, false),
            Input::Keys(b"b".to_vec()),
            mouse(0, 9, 4, true),
            Input::Keys(b"c".to_vec()),
        ];
        assert!(same(&got, &want));
    }

    #[test]
    fn a_report_split_across_reads_is_reassembled() {
        let mut pending = Vec::new();
        assert!(split_input(&mut pending, b"\x1b[<0;1").is_empty());
        let got = split_input(&mut pending, b"2;7Mx");
        assert!(same(&got, &[mouse(0, 11, 6, false), Input::Keys(b"x".to_vec())]));
        assert!(pending.is_empty());
    }

    #[test]
    fn malformed_reports_fall_back_to_keys() {
        let mut pending = Vec::new();
        let got = split_input(&mut pending, b"\x1b[<1;2Mz");
        assert!(same(&got, &[Input::Keys(b"\x1b[<1;2Mz".to_vec())]));
    }
}
