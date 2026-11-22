//! `imux doctor`: why pictures or the status line look the way they do. It reads and asks, it
//! changes nothing: no server is started, no file is written. The report is a list of
//! `key: value` lines built by `report` from `Facts`, so the logic is testable without a terminal;
//! `run` gathers the facts.

use std::collections::HashMap;
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::time::Duration;

use crate::config::cond;
use crate::config::lex::{self, Vars};
use crate::config::options::{Options, Set};
use crate::config::stmt::{self, Stmt};
use crate::log;
use crate::paths;
use crate::plugins;
use crate::proto::{self, Caps, ImageMode, Size};
use crate::sanitize;
use crate::tty::pty::{set_nonblocking, window_size};
use crate::tty::raw::RawMode;

use super::probe::{self, Probe};

/// Same cap as the server puts on a configuration file.
const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;

/// What asking the terminal came to.
pub enum Asked {
    /// Not asked, and why (`image-probe`, `image-probe-timeout`, no terminal).
    Skipped(String),
    Failed(String),
    Answer(Probe),
}

pub enum SocketDir {
    Ok,
    /// Nothing there: no server has run for this user.
    Missing,
    Bad(String),
}

pub enum Server {
    Down,
    /// Running; the pid when the system tells it.
    Up(Option<i32>),
    Unreachable(String),
}

/// What the configuration file says when it is read without a server: every `set` line is applied
/// to a fresh set of options, every line is parsed. `source-file` is not followed, and a command
/// line (`bind`, `run-shell`) is checked for its syntax only.
pub struct ConfigCheck {
    pub path: Option<String>,
    pub problems: Vec<String>,
    /// Options the file sets that are accepted and have no effect.
    pub ignored: Vec<String>,
    pub options: Options,
}

impl ConfigCheck {
    pub fn none() -> ConfigCheck {
        ConfigCheck { path: None, problems: Vec::new(), ignored: Vec::new(), options: Options::new() }
    }
}

pub fn check_file(path: &Path) -> ConfigCheck {
    let text = paths::read_regular(path, MAX_CONFIG_BYTES)
        .and_then(|b| String::from_utf8(b).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not valid UTF-8")));
    match text {
        Ok(text) => check_text(&path.to_string_lossy(), &text),
        Err(e) => {
            log::warn(&format!("doctor: cannot read {}: {e}", path.display()));
            ConfigCheck { path: Some(path.to_string_lossy().into_owned()), problems: vec![format!("{}: {e}", path.display())], ..ConfigCheck::none() }
        }
    }
}

pub fn check_text(label: &str, text: &str) -> ConfigCheck {
    let mut check = ConfigCheck { path: Some(label.to_string()), ..ConfigCheck::none() };
    let (mut vars, mut aliases) = (Vars::new(), HashMap::new());
    let mut conditions = cond::Conditions::default();
    for (line_no, line) in lex::logical_lines(text) {
        let line = match conditions.step(&line, &|c| cond::holds(&check.options, c, &|name| std::env::var(name).ok())) {
            cond::Step::Run(l) => l,
            cond::Step::Skip => continue,
            cond::Step::Error(e) => {
                check.problems.push(format!("{label}:{line_no}: {e}"));
                continue;
            }
        };
        let stmts = match stmt::parse_line_aliased(&line, &mut vars, &aliases) {
            Ok(stmts) => stmts,
            Err(e) => {
                check.problems.push(format!("{label}:{line_no}: {e}"));
                continue;
            }
        };
        for s in stmts {
            let (name, value, append, quiet, keep) = match s {
                Stmt::Set { name, value, append } => (name, value, append, false, false),
                Stmt::SetGuarded { name, value, append, quiet, keep } => (name, value, append, quiet, keep),
                _ => continue,
            };
            if name == "command-alias" || name.starts_with("command-alias[") {
                match value.as_deref().and_then(|v| v.split_once('=')) {
                    Some((alias, command)) => {
                        aliases.insert(alias.to_string(), command.to_string());
                    }
                    None if name == "command-alias" => aliases.clear(),
                    None => {}
                }
                continue;
            }
            if keep && check.options.was_set(&name) {
                continue;
            }
            if name.starts_with('@') {
                let _ = check.options.set_user(&name, value.as_deref(), append, false);
                continue;
            }
            match check.options.set(&name, value.as_deref(), append) {
                Ok(Set::Done) => {}
                Ok(Set::Ignored) => {
                    if !name.starts_with('@') && !check.ignored.contains(&name) {
                        check.ignored.push(name);
                    }
                }
                Err(_) if quiet => {}
                Err(e) => check.problems.push(format!("{label}:{line_no}: {e}")),
            }
        }
    }
    check
}

pub struct Facts {
    pub version: &'static str,
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub env_caps: Caps,
    pub asked: Asked,
    /// The window size with its pixel size, `None` without a terminal.
    pub size: Option<Size>,
    pub config: ConfigCheck,
    pub socket_dir: (String, SocketDir),
    pub server: Server,
    /// One line per installed plugin, empty when there are none.
    pub plugins: Vec<String>,
}

struct Lines(Vec<(String, String)>);

impl Lines {
    fn put(&mut self, key: &str, value: impl Into<String>) {
        self.0.push((key.to_string(), sanitize::clean(&value.into())));
    }
}

fn mode_name(mode: ImageMode) -> &'static str {
    match mode {
        ImageMode::Blocks => "blocks",
        ImageMode::Iterm2 => "iterm2",
        ImageMode::Kitty => "kitty",
        ImageMode::Sixel => "sixel",
    }
}

fn forced_mode(name: &str) -> Option<ImageMode> {
    [ImageMode::Blocks, ImageMode::Iterm2, ImageMode::Kitty, ImageMode::Sixel].into_iter().find(|m| mode_name(*m) == name)
}

fn mode_list(mask: u8) -> String {
    let names: Vec<_> = [ImageMode::Iterm2, ImageMode::Kitty, ImageMode::Sixel]
        .into_iter()
        .filter(|m| mask & proto::mode_bit(*m) != 0)
        .map(mode_name)
        .collect();
    if names.is_empty() { "none".to_string() } else { names.join(" ") }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

pub fn report(f: &Facts) -> Vec<(String, String)> {
    let mut out = Lines(Vec::new());
    let unset = || "(unset)".to_string();
    let opts = &f.config.options;
    out.put("version", format!("imux {}", f.version));
    out.put(
        "terminal",
        format!("TERM={} TERM_PROGRAM={}", f.term.clone().unwrap_or_else(unset), f.term_program.clone().unwrap_or_else(unset)),
    );

    let (accepted, caps) = match &f.asked {
        Asked::Answer(p) => {
            out.put("terminal name", if p.name.is_empty() { "no answer to XTVERSION" } else { p.name.as_str() });
            out.put("kitty graphics", yes_no(p.kitty));
            out.put("sixel", yes_no(p.sixel));
            if !p.done {
                out.put("terminal probe", "the terminal did not finish answering in time (image-probe-timeout)");
            }
            (probe::accepted_modes(f.env_caps, p), probe::merge_preferring(f.env_caps, p, opts.text("image-prefer")))
        }
        Asked::Skipped(why) => {
            out.put("terminal probe", format!("not asked: {why}"));
            (7, f.env_caps)
        }
        Asked::Failed(e) => {
            out.put("terminal probe", format!("failed: {e}"));
            (7, f.env_caps)
        }
    };

    match f.size {
        Some(s) if s.xpix > 0 && s.ypix > 0 && s.cols > 0 && s.rows > 0 => {
            out.put("cell size", format!("{}x{} px ({}x{} cells)", s.xpix / s.cols, s.ypix / s.rows, s.cols, s.rows));
        }
        Some(s) => out.put("cell size", format!("unknown: the terminal reports no pixel size ({}x{} cells)", s.cols, s.rows)),
        None => out.put("cell size", "unknown: no terminal"),
    }
    out.put("truecolor", format!("{} (a guess from the environment and the picture protocols)", yes_no(caps.truecolor)));
    let accepted_text = match &f.asked {
        Asked::Answer(p) if p.done => mode_list(accepted),
        Asked::Answer(_) => "everything (the terminal did not answer, so nothing is ruled out)".to_string(),
        _ => "everything (the terminal was not asked, so nothing is ruled out)".to_string(),
    };
    out.put("picture protocols accepted", accepted_text);

    let forced = forced_mode(opts.text("image-mode"));
    let used = proto::pick_mode(forced, accepted, caps.images);
    out.put(
        "picture mode",
        format!(
            "{} (found: {}; image-mode {}; image-prefer {})",
            mode_name(used),
            mode_name(caps.images),
            opts.text("image-mode"),
            opts.text("image-prefer")
        ),
    );
    if let Some(want) = forced
        && want != used
    {
        out.put("hint", format!("image-mode {} is not accepted by this terminal, {} is used instead", mode_name(want), mode_name(used)));
    }
    if used == ImageMode::Blocks && !matches!(f.asked, Asked::Answer(_)) && f.size.is_some() {
        out.put("hint", "pictures are drawn as blocks and the terminal was not asked; `set -g image-probe always` asks it");
    }
    if let Some(s) = f.size
        && (s.xpix == 0 || s.ypix == 0)
    {
        out.put("hint", "without a pixel size pictures are cropped, not scaled to the cells");
    }

    match &f.config.path {
        Some(p) => out.put("config", p.as_str()),
        None => out.put("config", "none (built-in defaults)"),
    }
    if f.config.path.is_some() {
        let n = f.config.problems.len();
        if n == 0 {
            out.put("config check", "no problems (lines are parsed and options applied; source-file is not followed)");
        } else {
            out.put("config check", format!("{n} problem(s)"));
        }
    }
    for p in &f.config.problems {
        out.put("config problem", p.as_str());
    }
    for name in &f.config.ignored {
        out.put("config option without effect", name.as_str());
    }

    let (dir, state) = &f.socket_dir;
    match state {
        SocketDir::Ok => out.put("socket directory", format!("{dir} (private, ours)")),
        SocketDir::Missing => out.put("socket directory", format!("{dir} (does not exist: no server has run)")),
        SocketDir::Bad(why) => out.put("socket directory", format!("{dir} PROBLEM: {why}")),
    }
    match &f.server {
        Server::Down => out.put("server", "not running"),
        Server::Up(Some(pid)) => out.put("server", format!("running (pid {pid})")),
        Server::Up(None) => out.put("server", "running"),
        Server::Unreachable(e) => out.put("server", format!("not reachable: {e}")),
    }

    if f.plugins.is_empty() {
        out.put("plugins", "none");
    }
    for line in &f.plugins {
        out.put("plugin", line.as_str());
    }
    out.put(
        "agent glyphs",
        format!(
            "working={} blocked={} done={} error={}",
            opts.text("agent-glyph-working"),
            opts.text("agent-glyph-blocked"),
            opts.text("agent-glyph-done"),
            opts.text("agent-glyph-error")
        ),
    );
    out.0
}

/// Ask the terminal the way an attach does. The terminal is in raw mode only while it answers.
fn ask_terminal(tty_in: &std::fs::File, tty_out: &std::fs::File, patience: Duration) -> Asked {
    if let Err(e) = set_nonblocking(tty_in.as_fd()) {
        return Asked::Failed(e.to_string());
    }
    let _raw = match RawMode::enter(tty_out.as_fd()) {
        Ok(raw) => raw,
        Err(e) => return Asked::Failed(e.to_string()),
    };
    match probe::run(tty_in, tty_out, patience) {
        Ok((found, _typed)) => Asked::Answer(found),
        Err(e) => {
            log::warn(&format!("doctor: terminal probe failed: {e}"));
            Asked::Failed(e.to_string())
        }
    }
}

fn socket_dir() -> (String, SocketDir) {
    let dir = paths::socket_dir();
    let name = dir.to_string_lossy().into_owned();
    if matches!(std::fs::symlink_metadata(&dir), Err(e) if e.kind() == io::ErrorKind::NotFound) {
        return (name, SocketDir::Missing);
    }
    match paths::check_socket_dir() {
        Ok(()) => (name, SocketDir::Ok),
        Err(e) => (name, SocketDir::Bad(e.to_string())),
    }
}

fn server_state(dir: &SocketDir) -> Server {
    if !matches!(dir, SocketDir::Ok) {
        return Server::Down;
    }
    match super::dial(&paths::socket_path()) {
        Ok(stream) => Server::Up(crate::tty::proc::peer_pid(stream.as_fd())),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => Server::Down,
        Err(e) => Server::Unreachable(e.to_string()),
    }
}

/// What the server says to this imux's protocol version.
#[derive(Debug, PartialEq, Eq)]
pub enum Protocol {
    Same,
    /// The server refused; its words.
    Differs(String),
    /// The server hung up on the version: it is older than the handshake.
    Older,
    Unknown(String),
}

/// The `protocol` line: this imux's version, and what a running server makes of it.
pub fn protocol_line(ours: u16, server: Option<&Protocol>) -> String {
    match server {
        None => format!("{ours}"),
        Some(Protocol::Same) => format!("{ours} (the server speaks it too)"),
        Some(Protocol::Differs(why)) => format!("{ours} PROBLEM: {why}"),
        Some(Protocol::Older) => format!("{ours} PROBLEM: the server is older than the version check; run `imux kill-server` (the sessions end) and start imux again"),
        Some(Protocol::Unknown(why)) => format!("{ours} (could not ask the server: {why})"),
    }
}

/// Send the version and a harmless request, and see whether the server answers or refuses.
fn probe_protocol() -> Protocol {
    use crate::proto::{ClientMsg, FrameBuf, ServerMsg};
    let mut stream = match super::dial(&paths::socket_path()) {
        Ok(s) => s,
        Err(e) => return Protocol::Unknown(e.to_string()),
    };
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
        return Protocol::Unknown(e.to_string());
    }
    for msg in [ClientMsg::Version(crate::proto::version()), ClientMsg::ListKeys] {
        let mut wire = Vec::new();
        crate::proto::frame(&msg, &mut wire);
        if let Err(e) = stream.write_all(&wire) {
            return if e.kind() == io::ErrorKind::BrokenPipe { Protocol::Older } else { Protocol::Unknown(e.to_string()) };
        }
    }
    let mut frames = FrameBuf::default();
    let mut buf = vec![0u8; 4096];
    loop {
        let n = match io::Read::read(&mut stream, &mut buf) {
            Ok(0) => return Protocol::Older,
            Ok(n) => n,
            Err(e) => return Protocol::Unknown(e.to_string()),
        };
        frames.push(&buf[..n]);
        match frames.next::<ServerMsg>() {
            Ok(Some(ServerMsg::Text(_))) => return Protocol::Same,
            Ok(Some(ServerMsg::Error(why))) => return Protocol::Differs(why),
            Ok(_) => {}
            Err(e) => return Protocol::Unknown(e.to_string()),
        }
    }
}

fn plugin_lines() -> Vec<String> {
    match plugins::Dirs::current() {
        Ok(dirs) => {
            let text = plugins::list(&dirs);
            if text.starts_with("no plugins in") {
                Vec::new()
            } else {
                text.lines().map(|l| l.replace('\t', "  ")).collect()
            }
        }
        Err(e) => vec![format!("cannot read the plugin directory: {e}")],
    }
}

/// The `tmux` line, for a program run in a pane: whether a plain `tmux` is imux's shim, and the line
/// to add to the shell's startup file when it is not. `shim_dir` is `$IMUX_SHIM_DIR` (set in panes
/// with `tmux-compat on`), `first` the first `tmux` on `PATH`.
pub fn shim_line(in_pane: bool, shim_dir: Option<&str>, first: Option<&str>, shell: &str) -> Option<(String, String)> {
    let value = match (in_pane, shim_dir) {
        (false, _) => return None,
        (true, None) => "this pane has no shim (tmux-compat is off, or the pane started before it was on)".to_string(),
        (true, Some(dir)) => match first {
            Some(f) if Path::new(f).starts_with(dir) => "a plain tmux is the imux shim".to_string(),
            other => {
                let name = Path::new(shell).file_name().and_then(|n| n.to_str()).filter(|n| crate::init::SHELLS.contains(n)).unwrap_or("zsh");
                format!(
                    "a plain tmux is {}, not the imux shim; add this at the end of your {name} startup file: {}",
                    other.unwrap_or("not found"),
                    crate::init::line(name)
                )
            }
        },
    };
    Some(("tmux".to_string(), value))
}

/// The `restore` line from the answer to `list-panes -a -F '#{pane_restore_missing}<tab>place'`:
/// the panes that started in the home directory because the directory saved for them is gone, or
/// `None` when there are none.
pub fn restore_line(listing: &str) -> Option<(String, String)> {
    let places: Vec<&str> = listing.lines().filter_map(|l| l.split_once('\t')).filter(|(flag, _)| *flag == "1").map(|(_, place)| place).collect();
    if places.is_empty() {
        return None;
    }
    let text = format!("{} pane(s) started in the home directory because their saved directory is gone: {}", places.len(), places.join(" "));
    Some(("restore".to_string(), text))
}

/// Ask the running server which panes `restore_line` should name; nothing when it cannot say.
fn restore_panes() -> Option<(String, String)> {
    let words = ["list-panes", "-a", "-F", "#{pane_restore_missing}\t#{session_name}:#{window_index}.#{pane_index}"].map(String::from).to_vec();
    let listing = super::request_text(super::Mode::Attach, &crate::proto::ClientMsg::Run(proto::RunArgs { words, cwd: None })).ok()?;
    restore_line(&listing)
}

/// The `pane commands` line from the answer to `show-options -g`: what a program in a pane may do
/// (`command-from-panes`), whether scripts may type into agents (`agent-control`) and the spawn
/// limits, or `None` when the listing has no `command-from-panes` line.
pub fn pane_power_line(listing: &str) -> Option<(String, String)> {
    let value = |name: &str| listing.lines().find_map(|l| l.strip_prefix(name).and_then(|rest| rest.strip_prefix(' '))).map(str::trim);
    let tier = value("command-from-panes")?;
    let reach = match tier {
        "on" => "a pane the user made may run any command but run-shell, a pane it spawned only acts on the panes it spawned".to_string(),
        "spawned" => format!(
            "a pane acts on the panes it spawned, at most {} levels deep and {} live panes under one pane",
            value("spawned-depth").unwrap_or("?"),
            value("spawned-panes-max").unwrap_or("?")
        ),
        _ => "a pane may only look".to_string(),
    };
    let control = if value("agent-control") == Some("on") { "typing into agents is on" } else { "typing into agents is off (agent-control)" };
    Some(("pane commands".to_string(), format!("command-from-panes {tier}: {reach}; {control}")))
}

/// Ask the running server for `pane_power_line`; nothing when it cannot say.
fn pane_power() -> Option<(String, String)> {
    let words = ["show-options", "-g"].map(String::from).to_vec();
    let listing = super::request_text(super::Mode::Attach, &crate::proto::ClientMsg::Run(proto::RunArgs { words, cwd: None })).ok()?;
    pane_power_line(&listing)
}

/// The first `tmux` along `path`.
fn first_tmux(path: &str) -> Option<String> {
    path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("tmux")).find(|p| p.exists()).map(|p| p.to_string_lossy().into_owned())
}

/// `imux doctor`.
pub fn run() -> io::Result<i32> {
    let config = match crate::config::default_path() {
        Some(path) => check_file(&path),
        None => ConfigCheck::none(),
    };
    let env = |k: &str| std::env::var(k).ok();
    let patience = Duration::from_millis(config.options.num("image-probe-timeout"));
    let mode = config.options.text("image-probe");
    let (asked, size) = match (super::open_tty(false), super::open_tty(true)) {
        (Ok(tty_in), Ok(tty_out)) => {
            let size = window_size(tty_out.as_fd()).ok().map(|ws| Size::from_winsize(&ws));
            let asked = if patience.is_zero() {
                Asked::Skipped("image-probe-timeout is 0".to_string())
            } else if mode == "never" {
                Asked::Skipped("image-probe is never".to_string())
            } else if !probe::should_ask(mode, env) {
                Asked::Skipped("image-probe auto leaves this terminal out, it cannot answer".to_string())
            } else {
                ask_terminal(&tty_in, &tty_out, patience)
            };
            (asked, size)
        }
        _ => (Asked::Skipped("no terminal".to_string()), None),
    };
    let socket_dir = socket_dir();
    let server = server_state(&socket_dir.1);
    let facts = Facts {
        version: env!("CARGO_PKG_VERSION"),
        term: env("TERM"),
        term_program: env("TERM_PROGRAM"),
        env_caps: super::detect_caps(env),
        asked,
        size,
        config,
        socket_dir,
        server,
        plugins: plugin_lines(),
    };
    let running = matches!(facts.server, Server::Up(_));
    let protocol = running.then(probe_protocol);
    let mut lines = report(&facts);
    if let Some(at) = lines.iter().position(|(k, _)| k == "server") {
        lines.insert(at + 1, ("protocol".to_string(), protocol_line(crate::proto::version(), protocol.as_ref())));
        let name = paths::state_suffix().unwrap_or_else(|| "default".to_string());
        lines.insert(at, ("server name".to_string(), format!("{name} ({})", paths::socket_path().display())));
    }
    if matches!(protocol, Some(Protocol::Same)) {
        lines.extend(restore_panes());
        lines.extend(pane_power());
    }
    let first = first_tmux(&env("PATH").unwrap_or_default());
    let shell = env("SHELL").unwrap_or_default();
    lines.extend(shim_line(env("IMUX_PANE_ID").is_some(), env("IMUX_SHIM_DIR").as_deref(), first.as_deref(), &shell));
    if env("IMUX_ENV").is_some() {
        let allowed = env("IMUX_ALLOW_NESTED").is_some_and(|v| v == "1");
        let text = if allowed { "allowed here (IMUX_ALLOW_NESTED=1)" } else { "refused in a pane: imux, imux new and imux attach (IMUX_ALLOW_NESTED=1 allows)" };
        lines.push(("nested attach".to_string(), text.to_string()));
    }
    lines.push(crate::skill::doctor());
    let mut stdout = io::stdout().lock();
    for (key, value) in lines {
        writeln!(stdout, "{key}: {value}").map_err(|e| crate::tty::ctx(e, "write the report"))?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(asked: Asked, env_caps: Caps) -> Facts {
        Facts {
            version: "0.0.0",
            term: Some("xterm-256color".into()),
            term_program: Some("iTerm.app".into()),
            env_caps,
            asked,
            size: Some(Size { cols: 100, rows: 40, xpix: 1400, ypix: 1120 }),
            config: ConfigCheck::none(),
            socket_dir: ("/tmp/imux-1".into(), SocketDir::Ok),
            server: Server::Down,
            plugins: Vec::new(),
        }
    }

    fn get<'a>(lines: &'a [(String, String)], key: &str) -> Vec<&'a str> {
        lines.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
    }

    fn one<'a>(lines: &'a [(String, String)], key: &str) -> &'a str {
        let all = get(lines, key);
        assert_eq!(all.len(), 1, "{key}: {all:?}");
        all[0]
    }

    const ITERM: Caps = Caps { images: ImageMode::Iterm2, truecolor: true };

    #[test]
    fn panes_that_lost_their_saved_directory_are_named() {
        assert_eq!(restore_line(""), None);
        assert_eq!(restore_line("0\tw:1.1\n0\tw:2.1\n"), None);
        let (key, value) = restore_line("0\tw:1.1\n1\tw:2.1\n1\tx:1.2\n").unwrap();
        assert_eq!(key, "restore");
        assert!(value.starts_with("2 pane(s) started in the home directory"), "{value}");
        assert!(value.ends_with("w:2.1 x:1.2"), "{value}");
        assert_eq!(restore_line("1 no tab here"), None, "a line without a tab is not a pane");
    }

    #[test]
    fn the_pane_commands_line_says_what_a_pane_may_do() {
        assert_eq!(pane_power_line("mouse on\n"), None);
        let (key, off) = pane_power_line("agent-control off\ncommand-from-panes off\n").unwrap();
        assert_eq!(key, "pane commands");
        assert!(off.starts_with("command-from-panes off: a pane may only look"), "{off}");
        assert!(off.ends_with("typing into agents is off (agent-control)"), "{off}");
        let (_, spawned) = pane_power_line("agent-control on\ncommand-from-panes spawned\nspawned-depth 3\nspawned-panes-max 8\n").unwrap();
        assert!(spawned.contains("at most 3 levels deep and 8 live panes"), "{spawned}");
        assert!(spawned.ends_with("typing into agents is on"), "{spawned}");
        let (_, on) = pane_power_line("command-from-panes on\n").unwrap();
        assert!(on.contains("run-shell"), "{on}");
    }

    #[test]
    fn iterm2_answers_everything_and_gets_inline_pictures() {
        let p = Probe { name: "iTerm2 3.5.0".into(), kitty: true, sixel: true, done: true };
        let r = report(&facts(Asked::Answer(p), ITERM));
        assert_eq!(one(&r, "terminal name"), "iTerm2 3.5.0");
        assert_eq!(one(&r, "kitty graphics"), "yes");
        assert_eq!(one(&r, "picture protocols accepted"), "iterm2 kitty sixel");
        assert!(one(&r, "picture mode").starts_with("iterm2 "), "{r:?}");
        assert_eq!(one(&r, "cell size"), "14x28 px (100x40 cells)");
        assert!(get(&r, "hint").is_empty());
    }

    #[test]
    fn image_prefer_kitty_moves_iterm2_to_kitty() {
        let p = Probe { name: "iTerm2 3.5.0".into(), kitty: true, sixel: false, done: true };
        let mut f = facts(Asked::Answer(p), ITERM);
        f.config = check_text("c", "set -g image-prefer kitty\n");
        assert!(one(&report(&f), "picture mode").starts_with("kitty "));
    }

    #[test]
    fn foot_offers_sixel_only() {
        let p = Probe { name: "foot(1.17.2)".into(), kitty: false, sixel: true, done: true };
        let r = report(&facts(Asked::Answer(p), Caps { images: ImageMode::Sixel, truecolor: true }));
        assert_eq!(one(&r, "picture protocols accepted"), "sixel");
        assert!(one(&r, "picture mode").starts_with("sixel "));
    }

    #[test]
    fn a_silent_terminal_gets_what_the_environment_says() {
        let r = report(&facts(Asked::Answer(Probe::default()), Caps::BASIC));
        assert_eq!(one(&r, "terminal name"), "no answer to XTVERSION");
        assert!(one(&r, "picture protocols accepted").starts_with("everything (the terminal did not answer"));
        assert!(one(&r, "picture mode").starts_with("blocks "));
        assert!(one(&r, "terminal probe").contains("did not finish"));
    }

    #[test]
    fn a_terminal_that_answered_and_offers_nothing_accepts_nothing() {
        let apple = Probe { done: true, ..Probe::default() };
        let r = report(&facts(Asked::Answer(apple), Caps::BASIC));
        assert_eq!(one(&r, "picture protocols accepted"), "none");
        assert!(one(&r, "picture mode").starts_with("blocks "));
        assert!(get(&r, "terminal probe").is_empty());
    }

    #[test]
    fn terminal_app_is_not_asked_and_the_report_says_how_to_ask() {
        let mut f = facts(Asked::Skipped("image-probe auto leaves this terminal out, it cannot answer".into()), Caps::BASIC);
        f.term_program = Some("Apple_Terminal".into());
        let r = report(&f);
        assert!(one(&r, "terminal probe").starts_with("not asked: image-probe auto"));
        assert!(one(&r, "picture protocols accepted").starts_with("everything"));
        assert!(one(&r, "picture mode").starts_with("blocks "));
        assert!(get(&r, "hint").iter().any(|h| h.contains("image-probe always")));
    }

    #[test]
    fn a_forced_mode_the_terminal_refuses_is_explained() {
        let p = Probe { name: "foot(1.17.2)".into(), kitty: false, sixel: true, done: true };
        let mut f = facts(Asked::Answer(p), Caps { images: ImageMode::Sixel, truecolor: true });
        f.config = check_text("c", "set -g image-mode kitty\n");
        let r = report(&f);
        assert!(one(&r, "picture mode").starts_with("sixel "));
        assert!(get(&r, "hint").iter().any(|h| h.contains("image-mode kitty is not accepted")), "{r:?}");
    }

    #[test]
    fn no_pixel_size_means_cropped_pictures() {
        let mut f = facts(Asked::Skipped("no terminal".into()), Caps::BASIC);
        f.size = Some(Size { cols: 80, rows: 24, xpix: 0, ypix: 0 });
        let r = report(&f);
        assert!(one(&r, "cell size").starts_with("unknown: the terminal reports no pixel size"));
        assert!(get(&r, "hint").iter().any(|h| h.contains("cropped")));
        f.size = None;
        assert_eq!(one(&report(&f), "cell size"), "unknown: no terminal");
    }

    #[test]
    fn the_report_never_carries_a_control_character_from_the_terminal() {
        let p = Probe { name: "evil\x1b]52;c;AAAA\x07name".into(), done: true, ..Probe::default() };
        let r = report(&facts(Asked::Answer(p), Caps::BASIC));
        assert!(!one(&r, "terminal name").chars().any(char::is_control), "{r:?}");
    }

    #[test]
    fn the_config_check_names_file_and_line() {
        let c = check_text("~/.imux.conf", "set -g mouse on\nset -g image-probe-timeout soon\nnot-a-command x\nset -g no-such-option 1\n");
        assert_eq!(c.problems.len(), 3, "{:?}", c.problems);
        assert!(c.problems[0].starts_with("~/.imux.conf:2: "), "{:?}", c.problems);
        assert!(c.problems[1].starts_with("~/.imux.conf:3: "));
        assert!(c.problems[2].starts_with("~/.imux.conf:4: "));
        let mut f = facts(Asked::Skipped("no terminal".into()), Caps::BASIC);
        f.config = c;
        let r = report(&f);
        assert_eq!(one(&r, "config check"), "3 problem(s)");
        assert_eq!(get(&r, "config problem").len(), 3);
    }

    #[test]
    fn a_clean_config_and_no_config_are_both_said() {
        let mut f = facts(Asked::Skipped("no terminal".into()), Caps::BASIC);
        assert_eq!(one(&report(&f), "config"), "none (built-in defaults)");
        assert!(get(&report(&f), "config check").is_empty());
        f.config = check_text("c", "set -g mouse on\nbind x new-window\n");
        assert!(one(&report(&f), "config check").starts_with("no problems"));
    }

    #[test]
    fn aliases_set_earlier_are_known_later_and_quiet_sets_are_no_problem() {
        let c = check_text("c", "set -g command-alias nw=new-window\nnw\nset -gq no-such-option 1\nset -g @theme dark\n");
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        assert!(c.ignored.is_empty(), "user options are not 'without effect'");
    }

    #[test]
    fn an_option_that_is_accepted_and_does_nothing_is_listed_once() {
        let c = check_text("c", "set -g default-terminal tmux-256color\nset -g default-terminal screen\n");
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        assert_eq!(c.ignored, ["default-terminal"]);
        let mut f = facts(Asked::Skipped("no terminal".into()), Caps::BASIC);
        f.config = c;
        assert_eq!(get(&report(&f), "config option without effect"), ["default-terminal"]);
    }

    #[test]
    fn the_protocol_line_says_what_the_server_makes_of_the_version() {
        assert_eq!(protocol_line(3, None), "3");
        assert_eq!(protocol_line(3, Some(&Protocol::Same)), "3 (the server speaks it too)");
        let differs = protocol_line(3, Some(&Protocol::Differs("this imux speaks protocol 3 but the server speaks 2: run it".into())));
        assert!(differs.starts_with("3 PROBLEM: this imux speaks protocol 3 but the server speaks 2"), "{differs}");
        let older = protocol_line(3, Some(&Protocol::Older));
        assert!(older.contains("PROBLEM") && older.contains("imux kill-server"), "{older}");
        assert!(protocol_line(3, Some(&Protocol::Unknown("timed out".into()))).contains("timed out"));
    }

    #[test]
    fn the_server_directory_and_server_states_are_reported() {
        let mut f = facts(Asked::Skipped("no terminal".into()), Caps::BASIC);
        f.socket_dir = ("/tmp/imux-1".into(), SocketDir::Bad("must be a directory owned by uid 1 with mode 0700".into()));
        f.server = Server::Up(Some(4242));
        let r = report(&f);
        assert!(one(&r, "socket directory").contains("PROBLEM: must be a directory"));
        assert_eq!(one(&r, "server"), "running (pid 4242)");
        f.socket_dir.1 = SocketDir::Missing;
        f.server = Server::Down;
        let r = report(&f);
        assert!(one(&r, "socket directory").contains("no server has run"));
        assert_eq!(one(&r, "server"), "not running");
    }

    #[test]
    fn plugins_and_glyphs_are_listed() {
        let mut f = facts(Asked::Skipped("no terminal".into()), Caps::BASIC);
        assert_eq!(one(&report(&f), "plugins"), "none");
        f.plugins = vec!["notify  1.2  enabled  permissions: send-keys".into()];
        assert_eq!(get(&report(&f), "plugin").len(), 1);
        let glyphs = one(&report(&f), "agent glyphs").to_string();
        assert!(glyphs.starts_with("working=\u{f250} "), "{glyphs}");
    }

    #[test]
    fn the_tmux_line_says_whether_a_plain_tmux_is_the_shim() {
        assert_eq!(shim_line(false, None, None, "/bin/zsh"), None, "outside a pane there is nothing to say");
        let ok = shim_line(true, Some("/t/shims"), Some("/t/shims/tmux"), "/bin/zsh").unwrap().1;
        assert!(ok.contains("is the imux shim"), "{ok}");
        let bad = shim_line(true, Some("/t/shims"), Some("/opt/homebrew/bin/tmux"), "/bin/zsh").unwrap().1;
        assert!(bad.contains("/opt/homebrew/bin/tmux") && bad.contains("eval \"$(imux init zsh)\""), "{bad}");
        let fish = shim_line(true, Some("/t/shims"), None, "/usr/bin/fish").unwrap().1;
        assert!(fish.contains("imux init fish | source") && fish.contains("not found"), "{fish}");
        let off = shim_line(true, None, None, "/bin/zsh").unwrap().1;
        assert!(off.contains("no shim"), "{off}");
    }

    #[test]
    fn a_missing_config_file_is_one_problem() {
        let c = check_file(Path::new("/nonexistent/imux.conf"));
        assert_eq!(c.problems.len(), 1);
        assert!(c.problems[0].starts_with("/nonexistent/imux.conf: "), "{:?}", c.problems);
    }
}
