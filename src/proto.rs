use std::io;

use nix::pty::Winsize;

const MAX_FRAME: usize = 16 * 1024 * 1024;

/// The version of the conversation between a client and the server. Bump it by hand whenever a tag
/// or a payload changes (`the_wire_format_is_pinned` fails until you do): a client that meets a
/// server of another version says so and does not talk to it. A client of an older imux sends no
/// `ClientMsg::Version` at all, and an older server closes the connection on it.
pub const PROTO_VERSION: u16 = 2;

/// The version this process speaks. In a debug build `IMUX_TEST_PROTO_VERSION` replaces it, so a
/// test can start a server that disagrees with the clients that follow.
pub fn version() -> u16 {
    #[cfg(debug_assertions)]
    if let Some(v) = std::env::var("IMUX_TEST_PROTO_VERSION").ok().and_then(|v| v.parse::<u16>().ok()) {
        return v;
    }
    PROTO_VERSION
}

/// What a server of version `server` (process `pid`) says to a client of version `client`.
pub fn mismatch_text(server: u16, client: u16, pid: u32) -> String {
    format!(
        "this imux speaks protocol {client} but the server speaks {server}: run `imux kill-server` to end it (the sessions end), or `kill {pid}` to save them first (restore-on-start brings them back), then start imux again"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub xpix: u16,
    pub ypix: u16,
}

impl Size {
    pub fn from_winsize(ws: &Winsize) -> Size {
        Size { cols: ws.ws_col, rows: ws.ws_row, xpix: ws.ws_xpixel, ypix: ws.ws_ypixel }
    }

    pub fn to_winsize(self) -> Winsize {
        Winsize { ws_row: self.rows, ws_col: self.cols, ws_xpixel: self.xpix, ws_ypixel: self.ypix }
    }

    fn put(self, out: &mut Vec<u8>) {
        for v in [self.cols, self.rows, self.xpix, self.ypix] {
            out.extend_from_slice(&v.to_be_bytes());
        }
    }

    fn get(payload: &[u8]) -> io::Result<Size> {
        if payload.len() != 8 {
            return Err(bad("size payload must be 8 bytes"));
        }
        let v = |i: usize| u16::from_be_bytes([payload[i], payload[i + 1]]);
        let size = Size { cols: v(0), rows: v(2), xpix: v(4), ypix: v(6) };
        // Every pane grid is sized from this: 65535 x 65535 cells would be 100 GB (security
        // review F-I). No terminal is anywhere near this.
        if size.cols > MAX_COLS || size.rows > MAX_ROWS {
            return Err(bad("terminal size too large"));
        }
        Ok(size)
    }
}

/// Largest terminal size a client may announce.
pub const MAX_COLS: u16 = 1024;
pub const MAX_ROWS: u16 = 1024;

pub fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

pub trait Message: Sized {
    fn encode(&self) -> (u8, Vec<u8>);
    fn decode(tag: u8, payload: &[u8]) -> io::Result<Self>;
}

/// Which session a client wants, sent before its `Hello`; without one it joins the most recent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Want {
    /// An existing session, by name.
    Attach(String),
    /// A new session, with the given name or else the lowest unused number.
    Create(Option<String>),
}

/// How a client's terminal shows images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMode {
    /// Drawn with quadrant block characters; works anywhere.
    Blocks,
    /// iTerm2 inline image protocol (OSC 1337).
    Iterm2,
    /// Kitty graphics protocol: placements float over the text and must be deleted explicitly.
    Kitty,
    /// Sixel: stamped into the cells like text.
    Sixel,
}

/// What the client's terminal can do, sent before its `Hello`; without it a client gets `Caps::BASIC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub images: ImageMode,
    pub truecolor: bool,
}

impl Caps {
    pub const BASIC: Caps = Caps { images: ImageMode::Blocks, truecolor: false };

    fn put(self, out: &mut Vec<u8>) {
        out.push(match self.images {
            ImageMode::Blocks => 0,
            ImageMode::Iterm2 => 1,
            ImageMode::Kitty => 2,
            ImageMode::Sixel => 3,
        });
        out.push(u8::from(self.truecolor));
    }

    fn get(payload: &[u8]) -> io::Result<Caps> {
        let images = match payload {
            [0, _] => ImageMode::Blocks,
            [1, _] => ImageMode::Iterm2,
            [2, _] => ImageMode::Kitty,
            [3, _] => ImageMode::Sixel,
            _ => return Err(bad("bad capabilities payload")),
        };
        Ok(Caps { images, truecolor: payload[1] != 0 })
    }
}

/// Bit of `ClientMsg::Modes` for a protocol (blocks need none).
pub fn mode_bit(mode: ImageMode) -> u8 {
    match mode {
        ImageMode::Iterm2 => 1,
        ImageMode::Kitty => 2,
        ImageMode::Sixel => 4,
        ImageMode::Blocks => 0,
    }
}

/// The way to show pictures to a client: the one `forced` (`image-mode`) asks for if its terminal
/// accepts it, else `detected`, else the usual order (iTerm2, Kitty, Sixel), and color blocks when
/// the terminal accepts none. A protocol the terminal does not accept is never tried.
pub fn pick_mode(forced: Option<ImageMode>, accepted: u8, detected: ImageMode) -> ImageMode {
    let ok = |m: ImageMode| m == ImageMode::Blocks || accepted & mode_bit(m) != 0;
    match forced {
        Some(f) if ok(f) => f,
        // Nothing forced: what the client found, which is Blocks when it found nothing (and an
        // unknown terminal that was not probed accepts "all" only for a forced mode).
        None => detected,
        Some(_) if ok(detected) && detected != ImageMode::Blocks => detected,
        Some(_) => [ImageMode::Iterm2, ImageMode::Kitty, ImageMode::Sixel].into_iter().find(|m| ok(*m)).unwrap_or(ImageMode::Blocks),
    }
}

/// What `save-session` and friends ask of the server; names are snapshot names, `None` is the autosave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistOp {
    Save(Option<String>),
    Restore(Option<String>),
    List,
    Delete(String),
}

/// What `worktree` is asked for. `None` fields fall back to the options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeArgs {
    pub name: String,
    pub repo: Option<String>,
    pub base: Option<String>,
    pub detach: Option<bool>,
    pub start: Option<String>,
    /// Optional description for `worktree-branch-format`'s `#{slug}`.
    pub slug: Option<String>,
    /// Directory the request comes from (the CLI's), when it is not a pane of the server.
    pub from: Option<String>,
    /// `--setup text`, or an empty string for `--no-setup`; replaces `worktree-setup`.
    pub setup: Option<String>,
}

impl WorktreeArgs {
    fn put(&self, out: &mut Vec<u8>) {
        let flags = u8::from(self.repo.is_some())
            | u8::from(self.base.is_some()) << 1
            | u8::from(self.start.is_some()) << 2
            | u8::from(self.from.is_some()) << 3
            | match self.detach {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            } << 4
            | u8::from(self.slug.is_some()) << 6
            | u8::from(self.setup.is_some()) << 7;
        out.push(flags);
        let fields = [
            Some(&self.name),
            self.repo.as_ref(),
            self.base.as_ref(),
            self.start.as_ref(),
            self.from.as_ref(),
            self.slug.as_ref(),
            self.setup.as_ref(),
        ];
        let text: Vec<&str> = fields.iter().map(|f| f.map_or("", |s| s.as_str())).collect();
        out.extend_from_slice(text.join("\0").as_bytes());
    }

    fn get(payload: &[u8]) -> io::Result<Self> {
        let [flags, rest @ ..] = payload else { return Err(bad("short worktree request")) };
        let text = std::str::from_utf8(rest).map_err(|_| bad("worktree request is not text"))?;
        let fields: Vec<&str> = text.split('\0').collect();
        let [name, repo, base, start, from, slug, setup] = fields[..] else { return Err(bad("bad worktree request")) };
        let opt = |bit: u8, s: &str| (flags >> bit & 1 == 1).then(|| s.to_string());
        let detach = match flags >> 4 & 3 {
            0 => None,
            1 => Some(false),
            2 => Some(true),
            _ => return Err(bad("bad worktree request")),
        };
        Ok(WorktreeArgs {
            name: name.to_string(),
            repo: opt(0, repo),
            base: opt(1, base),
            detach,
            start: opt(2, start),
            from: opt(3, from),
            slug: opt(6, slug),
            setup: opt(7, setup),
        })
    }
}

/// What `worktree-remove` is asked for. Without `name` it is the worktree of the current window
/// (or, for the subcommand, the one the current directory is in).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeRemoveArgs {
    pub name: Option<String>,
    pub repo: Option<String>,
    /// Directory the request comes from (the CLI's), when it is not a pane of the server.
    pub from: Option<String>,
    pub force: bool,
}

impl WorktreeRemoveArgs {
    fn put(&self, out: &mut Vec<u8>) {
        let flags = u8::from(self.name.is_some())
            | u8::from(self.repo.is_some()) << 1
            | u8::from(self.from.is_some()) << 2
            | u8::from(self.force) << 3;
        out.push(flags);
        let text: Vec<&str> =
            [&self.name, &self.repo, &self.from].iter().map(|f| f.as_ref().map_or("", |s| s.as_str())).collect();
        out.extend_from_slice(text.join("\0").as_bytes());
    }

    fn get(payload: &[u8]) -> io::Result<Self> {
        let [flags, rest @ ..] = payload else { return Err(bad("short worktree-remove request")) };
        if flags >> 4 != 0 {
            return Err(bad("bad worktree-remove request"));
        }
        let text = std::str::from_utf8(rest).map_err(|_| bad("worktree-remove request is not text"))?;
        let [name, repo, from] = text.split('\0').collect::<Vec<_>>()[..] else {
            return Err(bad("bad worktree-remove request"));
        };
        let opt = |bit: u8, s: &str| (flags >> bit & 1 == 1).then(|| s.to_string());
        Ok(WorktreeRemoveArgs { name: opt(0, name), repo: opt(1, repo), from: opt(2, from), force: flags >> 3 & 1 == 1 })
    }
}

/// Most variables, and longest name or value, one `Env` message may carry.
pub const MAX_ENV_VARS: usize = 64;
pub const MAX_ENV_TEXT: usize = 8192;

fn put_env(vars: &[(String, String)], out: &mut Vec<u8>) {
    for (k, v) in vars {
        for s in [k, v] {
            out.extend_from_slice(&(s.len() as u32).to_be_bytes());
            out.extend_from_slice(s.as_bytes());
        }
    }
}

fn get_env(mut rest: &[u8]) -> io::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    while !rest.is_empty() {
        let mut pair = Vec::with_capacity(2);
        for _ in 0..2 {
            let (len, tail) = rest.split_first_chunk::<4>().ok_or_else(|| bad("short environment message"))?;
            let len = u32::from_be_bytes(*len) as usize;
            if len > tail.len() || len > MAX_ENV_TEXT {
                return Err(bad("bad environment message"));
            }
            let (text, tail) = tail.split_at(len);
            pair.push(String::from_utf8(text.to_vec()).map_err(|_| bad("environment message is not text"))?);
            rest = tail;
        }
        if out.len() >= MAX_ENV_VARS {
            return Err(bad("too many environment variables"));
        }
        let value = pair.pop().unwrap_or_default();
        out.push((pair.pop().unwrap_or_default(), value));
    }
    Ok(out)
}

/// Most words a `Run` request holds.
pub const MAX_RUN_WORDS: usize = 256;

/// `imux <command> ...`: a command line for the server to run, as the words of the command.
/// `cwd` is the directory it was typed in, for commands that start something.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunArgs {
    pub words: Vec<String>,
    pub cwd: Option<String>,
}

impl RunArgs {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(self.cwd.is_some()));
        let cwd = self.cwd.as_deref().unwrap_or("");
        for word in std::iter::once(cwd).chain(self.words.iter().map(String::as_str)) {
            out.extend_from_slice(&(word.len() as u32).to_be_bytes());
            out.extend_from_slice(word.as_bytes());
        }
    }

    fn get(payload: &[u8]) -> io::Result<Self> {
        let Some((&flags, mut rest)) = payload.split_first() else { return Err(bad("short command request")) };
        if flags > 1 {
            return Err(bad("bad command request"));
        }
        let mut strings = Vec::new();
        while !rest.is_empty() {
            let (len, tail) = rest.split_first_chunk::<4>().ok_or_else(|| bad("short command request"))?;
            let len = u32::from_be_bytes(*len) as usize;
            if len > tail.len() || strings.len() > MAX_RUN_WORDS {
                return Err(bad("bad command request"));
            }
            let (word, tail) = tail.split_at(len);
            strings.push(String::from_utf8(word.to_vec()).map_err(|_| bad("command request is not text"))?);
            rest = tail;
        }
        if strings.is_empty() {
            return Err(bad("short command request"));
        }
        let cwd = strings.remove(0);
        Ok(RunArgs { words: strings, cwd: (flags == 1).then_some(cwd) })
    }
}

/// States an agent can be waited for, as bits of `AgentWaitArgs::until`.
pub const UNTIL_IDLE: u8 = 1;
pub const UNTIL_WORKING: u8 = 2;
pub const UNTIL_DONE: u8 = 4;
pub const UNTIL_BLOCKED: u8 = 8;
pub const UNTIL_ERROR: u8 = 16;

/// `imux agent wait`: hold the connection until the pane's agent is in one of the `until` states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentWaitArgs {
    pub pane: u32,
    pub until: u8,
    /// Milliseconds; 0 waits for ever.
    pub timeout_ms: u32,
}

/// The version of the socket API: events and the agent commands. Additive changes keep it, a
/// change that breaks a script or a plugin raises it.
pub const API_VERSION: u32 = 1;

/// Longest text `imux agent prompt` types, in bytes.
pub const MAX_PROMPT: usize = 8 * 1024;

/// `imux agent prompt`: type `text` and Enter into the pane of an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptArgs {
    pub pane: u32,
    /// Hold the connection until the agent has taken the text up and its state changes again.
    pub wait: bool,
    /// Milliseconds the wait may last; 0 waits for ever.
    pub timeout_ms: u32,
    pub text: String,
}

impl PromptArgs {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.pane.to_be_bytes());
        out.extend_from_slice(&self.timeout_ms.to_be_bytes());
        out.push(u8::from(self.wait));
        out.extend_from_slice(self.text.as_bytes());
    }

    fn get(payload: &[u8]) -> io::Result<PromptArgs> {
        let text = std::str::from_utf8(payload.get(9..).ok_or_else(|| bad("short prompt"))?).map_err(|_| bad("prompt is not text"))?;
        let wait = match payload[8] {
            0 => false,
            1 => true,
            _ => return Err(bad("bad prompt flags")),
        };
        Ok(PromptArgs { pane: be32(payload, 0)?, timeout_ms: be32(payload, 4)?, wait, text: text.to_string() })
    }
}

/// `imux pane report-agent`: what a program in a pane says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportArgs {
    pub pane: u32,
    pub source: String,
    pub agent: String,
    pub state: String,
    pub seq: u64,
    pub message: Option<String>,
    /// The command that resumes the program, empty for none.
    pub resume: Vec<String>,
}

fn be32(payload: &[u8], at: usize) -> io::Result<u32> {
    payload.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| bad("short message"))
}

impl ReportArgs {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.pane.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.push(u8::from(self.message.is_some()) | u8::from(!self.resume.is_empty()) << 1);
        let mut fields = vec![self.source.as_str(), self.agent.as_str(), self.state.as_str(), self.message.as_deref().unwrap_or("")];
        fields.extend(self.resume.iter().map(String::as_str));
        out.extend_from_slice(fields.join("\0").as_bytes());
    }

    fn get(payload: &[u8]) -> io::Result<ReportArgs> {
        let pane = be32(payload, 0)?;
        let seq = payload
            .get(4..12)
            .map(|b| u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
            .ok_or_else(|| bad("short report"))?;
        let flags = *payload.get(12).ok_or_else(|| bad("short report"))?;
        let text = std::str::from_utf8(&payload[13..]).map_err(|_| bad("report is not text"))?;
        let mut fields = text.split('\0');
        let mut next = || fields.next().map(str::to_string).ok_or_else(|| bad("short report"));
        let (source, agent, state, message) = (next()?, next()?, next()?, next()?);
        let resume: Vec<String> = fields.map(str::to_string).collect();
        if flags & 2 == 0 && !resume.is_empty() || flags & 2 != 0 && resume.is_empty() {
            return Err(bad("bad report"));
        }
        Ok(ReportArgs { pane, source, agent, state, seq, message: (flags & 1 != 0).then_some(message), resume })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ClientMsg {
    Hello(Size),
    Input(Vec<u8>),
    Resize(Size),
    Kill,
    Want(Want),
    /// Ask for the list of sessions; answered with `ServerMsg::Text`.
    List,
    Caps(Caps),
    /// Save, restore, list or delete snapshots; answered with `ServerMsg::Text` or `Error`.
    Persist(PersistOp),
    /// Open a window on a git worktree; answered with `ServerMsg::Text` or `Error` when done.
    Worktree(WorktreeArgs),
    /// One line per pane that has a status record; answered with `ServerMsg::Text`.
    AgentList,
    /// The status record of a pane.
    AgentGet(u32),
    /// Answered when the pane's agent reaches one of the states, or with an error on timeout.
    AgentWait(AgentWaitArgs),
    /// A program reports about itself; the sender has to be running in that pane.
    PaneReport(ReportArgs),
    /// A program says it is gone (`pane`, `source`).
    PaneRelease(u32, String),
    /// Remove a git worktree and close its window; answered with `ServerMsg::Text` or `Error`.
    WorktreeRemove(WorktreeRemoveArgs),
    /// `imux list-keys`: one line per binding; answered with `ServerMsg::Text`.
    ListKeys,
    /// `imux <command> ...`: run a command line; answered with `ServerMsg::Text` (its output) or `Error`.
    Run(RunArgs),
    /// The variables of the client's environment named by `update-environment`, sent on attach.
    Env(Vec<(String, String)>),
    /// `imux events`: keep the connection open and get a `ServerMsg::Event` for each event of the
    /// listed types (none listed: all but the `after-` ones).
    Subscribe(Vec<String>),
    /// `imux agent prompt`: answered with `ServerMsg::Text` (when sent, or when the wait is over)
    /// or `Error`.
    AgentPrompt(PromptArgs),
    /// The picture protocols the client's terminal accepts, sent after `Caps`: bit 0 iTerm2, 1 Kitty,
    /// 2 Sixel (blocks always work). A client that does not send it is taken to accept all.
    Modes(u8),
    /// The protocol version of the client (`PROTO_VERSION`), the first thing it sends. A server of
    /// another version answers `ServerMsg::Error` and closes; a client that sends none is served.
    Version(u16),
    /// `imux attach -r`, sent before `Hello`: this client only watches. What it types and its mouse
    /// reports are dropped, except the keys that detach it. A guard against slips, not a boundary.
    ReadOnly,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServerMsg {
    Output(Vec<u8>),
    Exit(i32),
    Detached,
    Error(String),
    Text(String),
    /// `suspend-client`: give the terminal back and stop until the shell continues the job.
    Suspend,
    /// One event for a subscriber, as a line of JSON (without the line feed).
    Event(String),
}

impl Message for ClientMsg {
    fn encode(&self) -> (u8, Vec<u8>) {
        match self {
            ClientMsg::Hello(s) => {
                let mut p = Vec::new();
                s.put(&mut p);
                (1, p)
            }
            ClientMsg::Input(d) => (2, d.clone()),
            ClientMsg::Resize(s) => {
                let mut p = Vec::new();
                s.put(&mut p);
                (3, p)
            }
            ClientMsg::Kill => (4, Vec::new()),
            ClientMsg::Want(Want::Attach(name)) => (5, [&[0][..], name.as_bytes()].concat()),
            ClientMsg::Want(Want::Create(None)) => (5, vec![1]),
            ClientMsg::Want(Want::Create(Some(name))) => (5, [&[2][..], name.as_bytes()].concat()),
            ClientMsg::List => (6, Vec::new()),
            ClientMsg::Caps(c) => {
                let mut p = Vec::new();
                c.put(&mut p);
                (7, p)
            }
            ClientMsg::Persist(op) => {
                let (code, name) = match op {
                    PersistOp::Save(n) => (0u8, n.as_deref()),
                    PersistOp::Restore(n) => (1, n.as_deref()),
                    PersistOp::List => (2, None),
                    PersistOp::Delete(n) => (3, Some(n.as_str())),
                };
                let mut p = vec![code, u8::from(name.is_some())];
                p.extend_from_slice(name.unwrap_or_default().as_bytes());
                (8, p)
            }
            ClientMsg::Worktree(args) => {
                let mut p = Vec::new();
                args.put(&mut p);
                (9, p)
            }
            ClientMsg::AgentList => (10, Vec::new()),
            ClientMsg::AgentGet(pane) => (11, pane.to_be_bytes().to_vec()),
            ClientMsg::AgentWait(a) => {
                let mut p = a.pane.to_be_bytes().to_vec();
                p.push(a.until);
                p.extend_from_slice(&a.timeout_ms.to_be_bytes());
                (12, p)
            }
            ClientMsg::PaneReport(r) => {
                let mut p = Vec::new();
                r.put(&mut p);
                (13, p)
            }
            ClientMsg::PaneRelease(pane, source) => {
                let mut p = pane.to_be_bytes().to_vec();
                p.extend_from_slice(source.as_bytes());
                (14, p)
            }
            ClientMsg::WorktreeRemove(args) => {
                let mut p = Vec::new();
                args.put(&mut p);
                (15, p)
            }
            ClientMsg::ListKeys => (16, Vec::new()),
            ClientMsg::Run(args) => {
                let mut p = Vec::new();
                args.put(&mut p);
                (17, p)
            }
            ClientMsg::Env(vars) => {
                let mut p = Vec::new();
                put_env(vars, &mut p);
                (18, p)
            }
            ClientMsg::Subscribe(types) => (19, types.join("\0").into_bytes()),
            ClientMsg::AgentPrompt(args) => {
                let mut p = Vec::new();
                args.put(&mut p);
                (20, p)
            }
            ClientMsg::Modes(mask) => (21, vec![*mask]),
            ClientMsg::Version(v) => (22, v.to_be_bytes().to_vec()),
            ClientMsg::ReadOnly => (23, Vec::new()),
        }
    }

    fn decode(tag: u8, payload: &[u8]) -> io::Result<Self> {
        match tag {
            1 => Ok(ClientMsg::Hello(Size::get(payload)?)),
            2 => Ok(ClientMsg::Input(payload.to_vec())),
            3 => Ok(ClientMsg::Resize(Size::get(payload)?)),
            4 => Ok(ClientMsg::Kill),
            5 => {
                let name = || String::from_utf8_lossy(&payload[1..]).into_owned();
                match payload.first() {
                    Some(0) => Ok(ClientMsg::Want(Want::Attach(name()))),
                    Some(1) => Ok(ClientMsg::Want(Want::Create(None))),
                    Some(2) => Ok(ClientMsg::Want(Want::Create(Some(name())))),
                    _ => Err(bad("bad session request")),
                }
            }
            6 => Ok(ClientMsg::List),
            7 => Ok(ClientMsg::Caps(Caps::get(payload)?)),
            8 => {
                let [code, has_name, rest @ ..] = payload else { return Err(bad("short snapshot request")) };
                let name = (*has_name == 1).then(|| String::from_utf8_lossy(rest).into_owned());
                match (code, name) {
                    (0, n) => Ok(ClientMsg::Persist(PersistOp::Save(n))),
                    (1, n) => Ok(ClientMsg::Persist(PersistOp::Restore(n))),
                    (2, _) => Ok(ClientMsg::Persist(PersistOp::List)),
                    (3, Some(n)) => Ok(ClientMsg::Persist(PersistOp::Delete(n))),
                    _ => Err(bad("bad snapshot request")),
                }
            }
            9 => Ok(ClientMsg::Worktree(WorktreeArgs::get(payload)?)),
            10 if payload.is_empty() => Ok(ClientMsg::AgentList),
            11 if payload.len() == 4 => Ok(ClientMsg::AgentGet(be32(payload, 0)?)),
            12 if payload.len() == 9 => {
                Ok(ClientMsg::AgentWait(AgentWaitArgs { pane: be32(payload, 0)?, until: payload[4], timeout_ms: be32(payload, 5)? }))
            }
            13 => Ok(ClientMsg::PaneReport(ReportArgs::get(payload)?)),
            14 => {
                let source = std::str::from_utf8(payload.get(4..).ok_or_else(|| bad("short release"))?)
                    .map_err(|_| bad("release is not text"))?;
                Ok(ClientMsg::PaneRelease(be32(payload, 0)?, source.to_string()))
            }
            15 => Ok(ClientMsg::WorktreeRemove(WorktreeRemoveArgs::get(payload)?)),
            16 if payload.is_empty() => Ok(ClientMsg::ListKeys),
            17 => Ok(ClientMsg::Run(RunArgs::get(payload)?)),
            18 => Ok(ClientMsg::Env(get_env(payload)?)),
            19 => {
                let text = std::str::from_utf8(payload).map_err(|_| bad("event types are not text"))?;
                Ok(ClientMsg::Subscribe(if text.is_empty() { Vec::new() } else { text.split('\0').map(str::to_string).collect() }))
            }
            20 => Ok(ClientMsg::AgentPrompt(PromptArgs::get(payload)?)),
            21 if payload.len() == 1 => Ok(ClientMsg::Modes(payload[0] & 7)),
            21 => Err(bad("bad picture protocols payload")),
            22 if payload.len() == 2 => Ok(ClientMsg::Version(u16::from_be_bytes([payload[0], payload[1]]))),
            22 => Err(bad("bad protocol version payload")),
            23 if payload.is_empty() => Ok(ClientMsg::ReadOnly),
            23 => Err(bad("bad read-only payload")),
            16 => Err(bad("bad list-keys request")),
            10..=14 => Err(bad("bad agent request")),
            _ => Err(bad(&format!("unknown client message tag {tag}"))),
        }
    }
}

impl Message for ServerMsg {
    fn encode(&self) -> (u8, Vec<u8>) {
        match self {
            ServerMsg::Output(d) => (1, d.clone()),
            ServerMsg::Exit(code) => (2, code.to_be_bytes().to_vec()),
            ServerMsg::Detached => (3, Vec::new()),
            ServerMsg::Error(msg) => (4, msg.as_bytes().to_vec()),
            ServerMsg::Text(text) => (5, text.as_bytes().to_vec()),
            ServerMsg::Suspend => (6, Vec::new()),
            ServerMsg::Event(line) => (7, line.as_bytes().to_vec()),
        }
    }

    fn decode(tag: u8, payload: &[u8]) -> io::Result<Self> {
        match tag {
            1 => Ok(ServerMsg::Output(payload.to_vec())),
            2 => {
                let b: [u8; 4] = payload.try_into().map_err(|_| bad("exit payload must be 4 bytes"))?;
                Ok(ServerMsg::Exit(i32::from_be_bytes(b)))
            }
            3 => Ok(ServerMsg::Detached),
            4 => Ok(ServerMsg::Error(String::from_utf8_lossy(payload).into_owned())),
            5 => Ok(ServerMsg::Text(String::from_utf8_lossy(payload).into_owned())),
            6 => Ok(ServerMsg::Suspend),
            7 => Ok(ServerMsg::Event(String::from_utf8_lossy(payload).into_owned())),
            _ => Err(bad(&format!("unknown server message tag {tag}"))),
        }
    }
}

/// Frame layout: u32 BE length (tag + payload), u8 tag, payload.
pub fn frame<M: Message>(msg: &M, out: &mut Vec<u8>) {
    let (tag, payload) = msg.encode();
    out.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
    out.push(tag);
    out.extend_from_slice(&payload);
}

#[derive(Default)]
pub struct FrameBuf {
    buf: Vec<u8>,
}

impl FrameBuf {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next<M: Message>(&mut self) -> io::Result<Option<M>> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len == 0 || len > MAX_FRAME + 1 {
            return Err(bad(&format!("invalid frame length {len}")));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let msg = M::decode(self.buf[4], &self.buf[5..4 + len]);
        self.buf.drain(..4 + len);
        msg.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size { cols: 80, rows: 24, xpix: 1120, ypix: 720 };

    #[test]
    fn roundtrip_client_messages() {
        let msgs = [
            ClientMsg::Hello(SIZE),
            ClientMsg::Input(b"ls\r".to_vec()),
            ClientMsg::Resize(SIZE),
            ClientMsg::Kill,
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        assert_eq!(fb.next::<ClientMsg>().unwrap(), None);
    }

    #[test]
    fn an_absurd_terminal_size_is_refused_when_the_frame_is_read() {
        let huge = Size { cols: 65535, rows: 65535, xpix: 0, ypix: 0 };
        let mut wire = Vec::new();
        frame(&ClientMsg::Hello(huge), &mut wire);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        assert!(fb.next::<ClientMsg>().is_err());
        let edge = Size { cols: MAX_COLS, rows: MAX_ROWS, xpix: 0, ypix: 0 };
        let mut wire = Vec::new();
        frame(&ClientMsg::Resize(edge), &mut wire);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(ClientMsg::Resize(edge)));
    }

    #[test]
    fn roundtrip_server_messages() {
        let msgs = [
            ServerMsg::Output(vec![0, 1, 2, 255]),
            ServerMsg::Exit(-3),
            ServerMsg::Detached,
            ServerMsg::Error("boom".into()),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ServerMsg>().unwrap(), Some(m));
        }
    }

    #[test]
    fn a_command_request_round_trips_words_and_directory() {
        let msgs = [
            ClientMsg::Run(RunArgs::default()),
            ClientMsg::Run(RunArgs { words: vec!["list-windows".into(), "-F".into(), "#{window_name}".into()], cwd: None }),
            ClientMsg::Run(RunArgs { words: vec!["send-keys".into(), String::new(), "é ü".into()], cwd: Some("/tmp/a b".into()) }),
            ClientMsg::Run(RunArgs { words: vec![String::new()], cwd: Some(String::new()) }),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
    }

    #[test]
    fn environment_messages_round_trip_and_bad_ones_are_errors() {
        let msgs = [
            ClientMsg::Env(Vec::new()),
            ClientMsg::Env(vec![("SSH_AUTH_SOCK".into(), "/tmp/a b/agent".into()), ("EMPTY".into(), String::new())]),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        for payload in [&[0, 0, 0][..], &[0, 0, 0, 9, b'a'][..], &[0, 0, 0, 1, b'a'][..], &[0, 0, 0, 1, 0xff, 0, 0, 0, 0][..]] {
            assert!(get_env(payload).is_err(), "{payload:?}");
        }
        let many: Vec<(String, String)> = (0..=MAX_ENV_VARS).map(|i| (format!("V{i}"), String::new())).collect();
        let mut p = Vec::new();
        put_env(&many, &mut p);
        assert!(get_env(&p).is_err());
    }

    #[test]
    fn a_bad_command_request_is_an_error_not_a_panic() {
        for payload in [&[][..], &[2, 0, 0, 0, 0][..], &[0][..], &[0, 0, 0, 0, 5, b'a'][..], &[0, 0, 0, 0, 1, 0xff][..], &[1, 0, 0][..]] {
            assert!(RunArgs::get(payload).is_err(), "{payload:?}");
        }
        let mut many = vec![0u8];
        for _ in 0..(MAX_RUN_WORDS + 3) {
            many.extend_from_slice(&[0, 0, 0, 0]);
        }
        assert!(RunArgs::get(&many).is_err());
    }

    #[test]
    fn suspend_is_a_server_message_without_payload() {
        let mut wire = Vec::new();
        frame(&ServerMsg::Suspend, &mut wire);
        assert_eq!(wire, [0, 0, 0, 1, 6]);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        assert_eq!(fb.next::<ServerMsg>().unwrap(), Some(ServerMsg::Suspend));
    }

    #[test]
    fn roundtrip_session_requests_and_listing() {
        let msgs = [
            ClientMsg::Want(Want::Attach("work".into())),
            ClientMsg::Want(Want::Create(None)),
            ClientMsg::Want(Want::Create(Some("new one".into()))),
            ClientMsg::Want(Want::Create(Some(String::new()))),
            ClientMsg::List,
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        frame(&ServerMsg::Text("0: 1 window\n".into()), &mut wire);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        assert_eq!(fb.next::<ServerMsg>().unwrap(), Some(ServerMsg::Text("0: 1 window\n".into())));
    }

    #[test]
    fn a_forced_mode_goes_first_and_the_rest_follow_in_order_without_what_the_terminal_refuses() {
        use ImageMode::{Blocks, Iterm2, Kitty, Sixel};
        let all = 7;
        assert_eq!(pick_mode(None, all, Kitty), Kitty, "nothing forced: what was found");
        assert_eq!(pick_mode(Some(Sixel), all, Iterm2), Sixel, "forced and accepted");
        assert_eq!(pick_mode(Some(Sixel), 1 | 2, Iterm2), Iterm2, "Sixel refused: what was found");
        assert_eq!(pick_mode(Some(Sixel), 2, Iterm2), Kitty, "forced and detected refused: the usual order");
        assert_eq!(pick_mode(Some(Iterm2), 4, Sixel), Sixel, "iTerm2 refused, Sixel found");
        assert_eq!(pick_mode(Some(Iterm2), 4 | 2, Blocks), Kitty, "what was found does not count when it is the worst: the usual order");
        assert_eq!(pick_mode(None, all, Blocks), Blocks, "nothing forced and nothing found: nothing is invented");
        assert_eq!(pick_mode(Some(Iterm2), 0, Iterm2), Blocks, "nothing accepted: blocks");
        assert_eq!(pick_mode(Some(Blocks), all, Iterm2), Blocks, "blocks can be forced");
    }

    #[test]
    fn the_picture_protocols_a_terminal_accepts_travel_as_one_byte() {
        let mut wire = Vec::new();
        for m in [ClientMsg::Modes(0), ClientMsg::Modes(5), ClientMsg::Modes(7)] {
            frame(&m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for want in [0, 5, 7] {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(ClientMsg::Modes(want)));
        }
        let mut fb = FrameBuf::default();
        fb.push(&[0, 0, 0, 3, 21, 1, 1]);
        assert!(fb.next::<ClientMsg>().is_err());
    }

    #[test]
    fn roundtrip_capabilities_and_refuse_bad_ones() {
        let msgs = [
            ClientMsg::Caps(Caps::BASIC),
            ClientMsg::Caps(Caps { images: ImageMode::Iterm2, truecolor: true }),
            ClientMsg::Caps(Caps { images: ImageMode::Kitty, truecolor: true }),
            ClientMsg::Caps(Caps { images: ImageMode::Sixel, truecolor: false }),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        for bad_payload in [&[0, 0, 0, 1, 7][..], &[0, 0, 0, 3, 7, 9, 0], &[0, 0, 0, 4, 7, 0, 0, 0]] {
            let mut fb = FrameBuf::default();
            fb.push(bad_payload);
            assert!(fb.next::<ClientMsg>().is_err());
        }
    }

    #[test]
    fn roundtrip_snapshot_requests_and_refuse_bad_ones() {
        let msgs = [
            ClientMsg::Persist(PersistOp::Save(None)),
            ClientMsg::Persist(PersistOp::Save(Some("work".into()))),
            ClientMsg::Persist(PersistOp::Restore(None)),
            ClientMsg::Persist(PersistOp::Restore(Some("a b".into()))),
            ClientMsg::Persist(PersistOp::List),
            ClientMsg::Persist(PersistOp::Delete("old".into())),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        for bad_payload in [&[0, 0, 0, 1, 8][..], &[0, 0, 0, 2, 8, 0], &[0, 0, 0, 3, 8, 9, 0], &[0, 0, 0, 3, 8, 3, 0]] {
            let mut fb = FrameBuf::default();
            fb.push(bad_payload);
            assert!(fb.next::<ClientMsg>().is_err(), "{bad_payload:?}");
        }
    }

    #[test]
    fn roundtrip_worktree_requests_and_refuse_bad_ones() {
        let msgs = [
            ClientMsg::Worktree(WorktreeArgs { name: "ABC-1".into(), ..WorktreeArgs::default() }),
            ClientMsg::Worktree(WorktreeArgs {
                name: "ABC-2".into(),
                repo: Some("/w/proj".into()),
                base: Some("develop".into()),
                detach: Some(false),
                start: Some("/start ABC-2".into()),
                slug: None,
                from: Some("/w".into()),
                setup: None,
            }),
            ClientMsg::Worktree(WorktreeArgs {
                name: "x".into(),
                repo: Some(String::new()),
                detach: Some(true),
                start: Some(String::new()),
                ..WorktreeArgs::default()
            }),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        for bad_payload in [&[0, 0, 0, 1, 9][..], &[0, 0, 0, 3, 9, 0, b'a'], &[0, 0, 0, 7, 9, 0x30, 0, 0, 0, 0, 0], &[0, 0, 0, 7, 9, 0, 0xff, 0, 0, 0, 0]] {
            let mut fb = FrameBuf::default();
            fb.push(bad_payload);
            assert!(fb.next::<ClientMsg>().is_err(), "{bad_payload:?}");
        }
    }

    #[test]
    fn agent_requests_roundtrip_and_bad_ones_are_refused() {
        let msgs = [
            ClientMsg::AgentList,
            ClientMsg::AgentGet(7),
            ClientMsg::AgentWait(AgentWaitArgs { pane: 3, until: UNTIL_DONE | UNTIL_BLOCKED, timeout_ms: 5000 }),
            ClientMsg::PaneReport(ReportArgs {
                pane: 2,
                source: "hook".into(),
                agent: "claude".into(),
                state: "working".into(),
                seq: u64::MAX,
                message: Some("thinking".into()),
                resume: vec!["claude".into(), "--resume".into(), "abc".into()],
            }),
            ClientMsg::PaneReport(ReportArgs {
                pane: 0,
                source: "s".into(),
                agent: "a".into(),
                state: "idle".into(),
                seq: 1,
                message: None,
                resume: vec![],
            }),
            ClientMsg::PaneRelease(4, "hook".into()),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        let bad_payloads: [&[u8]; 9] = [
            &[0, 0, 0, 2, 10, 0],
            &[0, 0, 0, 2, 11, 0],
            &[0, 0, 0, 6, 11, 0, 0, 0, 0, 0],
            &[0, 0, 0, 4, 12, 0, 0, 0],
            &[0, 0, 0, 1, 13],
            &[0, 0, 0, 14, 13, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0],
            &[0, 0, 0, 3, 14, 0, 0],
            &[0, 0, 0, 6, 14, 0, 0, 0, 1, 0xff],
            &[0, 0, 0, 17, 13, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 2, b'a', 0, b'b'],
        ];
        for bad_payload in bad_payloads {
            let mut fb = FrameBuf::default();
            fb.push(bad_payload);
            assert!(fb.next::<ClientMsg>().is_err(), "{bad_payload:?}");
        }
    }

    #[test]
    fn a_worktree_slug_survives_the_wire() {
        let msg = ClientMsg::Worktree(WorktreeArgs {
            name: "ABC-3".into(),
            slug: Some("login fix".into()),
            detach: Some(false),
            ..WorktreeArgs::default()
        });
        let mut wire = Vec::new();
        frame(&msg, &mut wire);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(msg));
    }

    #[test]
    fn a_worktree_setup_and_no_setup_survive_the_wire() {
        for setup in [Some("npm ci && cp ../.env .".to_string()), Some(String::new()), None] {
            let msg = ClientMsg::Worktree(WorktreeArgs { name: "ABC-4".into(), setup, ..WorktreeArgs::default() });
            let mut wire = Vec::new();
            frame(&msg, &mut wire);
            let mut fb = FrameBuf::default();
            fb.push(&wire);
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(msg));
        }
    }

    #[test]
    fn worktree_remove_requests_roundtrip_and_bad_ones_are_refused() {
        let msgs = [
            ClientMsg::WorktreeRemove(WorktreeRemoveArgs::default()),
            ClientMsg::WorktreeRemove(WorktreeRemoveArgs {
                name: Some("ABC-1".into()),
                repo: Some("/w/proj".into()),
                from: Some("/w".into()),
                force: true,
            }),
            ClientMsg::WorktreeRemove(WorktreeRemoveArgs { name: Some(String::new()), force: true, ..WorktreeRemoveArgs::default() }),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        for bad_payload in [&[0, 0, 0, 1, 15][..], &[0, 0, 0, 3, 15, 0, 0], &[0, 0, 0, 4, 15, 0x10, 0, 0], &[0, 0, 0, 4, 15, 0, 0xff, 0]] {
            let mut fb = FrameBuf::default();
            fb.push(bad_payload);
            assert!(fb.next::<ClientMsg>().is_err(), "{bad_payload:?}");
        }
    }

    #[test]
    fn list_keys_roundtrips_and_takes_no_payload() {
        let mut wire = Vec::new();
        frame(&ClientMsg::ListKeys, &mut wire);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(ClientMsg::ListKeys));
        let mut fb = FrameBuf::default();
        fb.push(&[0, 0, 0, 2, 16, 1]);
        assert!(fb.next::<ClientMsg>().is_err());
    }

    #[test]
    fn rejects_a_bad_session_request() {
        let mut fb = FrameBuf::default();
        fb.push(&[0, 0, 0, 1, 5]);
        assert!(fb.next::<ClientMsg>().is_err());
        let mut fb = FrameBuf::default();
        fb.push(&[0, 0, 0, 2, 5, 9]);
        assert!(fb.next::<ClientMsg>().is_err());
    }

    #[test]
    fn partial_frames_wait_for_more_bytes() {
        let mut wire = Vec::new();
        frame(&ClientMsg::Input(b"hello".to_vec()), &mut wire);
        let mut fb = FrameBuf::default();
        for &b in &wire[..wire.len() - 1] {
            fb.push(&[b]);
            assert_eq!(fb.next::<ClientMsg>().unwrap(), None);
        }
        fb.push(&wire[wire.len() - 1..]);
        assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(ClientMsg::Input(b"hello".to_vec())));
    }

    #[test]
    fn rejects_oversized_zero_and_unknown() {
        let mut fb = FrameBuf::default();
        fb.push(&u32::MAX.to_be_bytes());
        assert!(fb.next::<ClientMsg>().is_err());

        let mut fb = FrameBuf::default();
        fb.push(&0u32.to_be_bytes());
        assert!(fb.next::<ClientMsg>().is_err());

        let mut fb = FrameBuf::default();
        fb.push(&[0, 0, 0, 1, 99]);
        assert!(fb.next::<ClientMsg>().is_err());
    }

    #[test]
    fn subscriptions_prompts_and_events_round_trip() {
        let prompt = PromptArgs { pane: 7, wait: true, timeout_ms: 30_000, text: "fix the test\nthen run it é".into() };
        let msgs = [
            ClientMsg::Subscribe(Vec::new()),
            ClientMsg::Subscribe(vec!["window-renamed".into(), "agent-state-changed".into()]),
            ClientMsg::AgentPrompt(prompt.clone()),
            ClientMsg::AgentPrompt(PromptArgs { wait: false, timeout_ms: 0, text: String::new(), ..prompt }),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            frame(m, &mut wire);
        }
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        for m in msgs {
            assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(m));
        }
        let mut fb = FrameBuf::default();
        let mut one = Vec::new();
        frame(&ServerMsg::Event("{\"event\":\"x\"}".into()), &mut one);
        fb.push(&one);
        assert_eq!(fb.next::<ServerMsg>().unwrap(), Some(ServerMsg::Event("{\"event\":\"x\"}".into())));
    }

    #[test]
    fn bad_subscriptions_and_prompts_are_refused() {
        for (tag, payload) in [
            (19u8, vec![0xff, 0xfe]),
            (20, vec![0, 0, 0, 1, 0, 0, 0, 0]),
            (20, vec![0, 0, 0, 1, 0, 0, 0, 0, 2, b'x']),
            (20, vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 0xff]),
            (20, Vec::new()),
        ] {
            assert!(ClientMsg::decode(tag, &payload).is_err(), "tag {tag} {payload:?}");
        }
        assert_eq!(ClientMsg::decode(19, b"").unwrap(), ClientMsg::Subscribe(Vec::new()));
        assert_eq!(ClientMsg::decode(20, &[0, 0, 0, 1, 0, 0, 0, 0, 0]).unwrap(), ClientMsg::AgentPrompt(PromptArgs { pane: 1, wait: false, timeout_ms: 0, text: String::new() }));
    }

    #[test]
    fn the_version_message_round_trips_and_a_bad_one_is_refused() {
        let mut wire = Vec::new();
        frame(&ClientMsg::Version(0x0102), &mut wire);
        assert_eq!(wire, [0, 0, 0, 3, 22, 1, 2]);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(ClientMsg::Version(0x0102)));
        for payload in [&[][..], &[1][..], &[1, 2, 3][..]] {
            assert!(ClientMsg::decode(22, payload).is_err(), "{payload:?}");
        }
    }

    #[test]
    fn the_read_only_message_has_no_payload() {
        let mut wire = Vec::new();
        frame(&ClientMsg::ReadOnly, &mut wire);
        assert_eq!(wire, [0, 0, 0, 1, 23]);
        let mut fb = FrameBuf::default();
        fb.push(&wire);
        assert_eq!(fb.next::<ClientMsg>().unwrap(), Some(ClientMsg::ReadOnly));
        assert!(ClientMsg::decode(23, &[1]).is_err());
    }

    #[test]
    fn this_process_speaks_the_version_of_the_source_unless_a_test_says_otherwise() {
        // Under `cargo test` the variable is unset; `version()` is then the constant.
        if std::env::var("IMUX_TEST_PROTO_VERSION").is_err() {
            assert_eq!(version(), PROTO_VERSION);
        }
    }

    #[test]
    fn the_mismatch_text_names_both_versions_the_way_out_and_the_server_to_stop() {
        let text = mismatch_text(3, 2, 4242);
        assert!(text.contains("protocol 2") && text.contains("server speaks 3"), "{text}");
        assert!(text.contains("imux kill-server") && text.contains("kill 4242"), "{text}");
        assert!(!text.contains('\n'), "one line, it is shown in the status line and on stderr");
    }

    /// Every message in a fixed form, as the bytes they travel in.
    fn every_frame() -> Vec<u8> {
        let size = Size { cols: 80, rows: 24, xpix: 8, ypix: 16 };
        let client = vec![
            ClientMsg::Hello(size),
            ClientMsg::Input(b"abc".to_vec()),
            ClientMsg::Resize(size),
            ClientMsg::Kill,
            ClientMsg::Want(Want::Attach("a".into())),
            ClientMsg::Want(Want::Create(None)),
            ClientMsg::Want(Want::Create(Some("b".into()))),
            ClientMsg::List,
            ClientMsg::Caps(Caps::BASIC),
            ClientMsg::Persist(PersistOp::Save(Some("s".into()))),
            ClientMsg::Worktree(WorktreeArgs::default()),
            ClientMsg::AgentList,
            ClientMsg::AgentGet(7),
            ClientMsg::AgentWait(AgentWaitArgs { pane: 7, until: 2, timeout_ms: 500 }),
            ClientMsg::PaneReport(ReportArgs::default()),
            ClientMsg::PaneRelease(7, "src".into()),
            ClientMsg::WorktreeRemove(WorktreeRemoveArgs::default()),
            ClientMsg::ListKeys,
            ClientMsg::Run(RunArgs { words: vec!["a".into(), "b".into()], cwd: Some("/".into()) }),
            ClientMsg::Env(vec![("K".into(), "V".into())]),
            ClientMsg::Subscribe(vec!["x".into(), "y".into()]),
            ClientMsg::AgentPrompt(PromptArgs { pane: 1, wait: true, timeout_ms: 9, text: "t".into() }),
            ClientMsg::Modes(5),
            ClientMsg::Version(1),
            ClientMsg::ReadOnly,
        ];
        let server = vec![
            ServerMsg::Output(b"out".to_vec()),
            ServerMsg::Exit(-2),
            ServerMsg::Detached,
            ServerMsg::Error("e".into()),
            ServerMsg::Text("t".into()),
            ServerMsg::Suspend,
            ServerMsg::Event("{}".into()),
        ];
        let mut wire = Vec::new();
        for m in &client {
            frame(m, &mut wire);
        }
        for m in &server {
            frame(m, &mut wire);
        }
        wire
    }

    /// If this fails you changed a tag or a payload. Bump `PROTO_VERSION`, then put the new
    /// numbers here: a server and a client of different versions must never talk.
    #[test]
    fn the_wire_format_is_pinned_to_the_protocol_version() {
        let wire = every_frame();
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in &wire {
            hash = (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3);
        }
        assert_eq!((PROTO_VERSION, wire.len(), hash), (2, EXPECTED_BYTES, EXPECTED_HASH), "the wire format changed: bump PROTO_VERSION");
    }

    const EXPECTED_BYTES: usize = 288;
    const EXPECTED_HASH: u64 = 6809655820897926344;

    #[test]
    fn rejects_short_size_payload() {
        let mut fb = FrameBuf::default();
        fb.push(&[0, 0, 0, 4, 1, 0, 0, 0]);
        assert!(fb.next::<ClientMsg>().is_err());
    }
}
