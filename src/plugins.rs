//! Plugins: programs that imux starts, supervises and lets use the socket API (`imux events`,
//! `imux agent ...`, `imux <command>`), with the rights their manifest lists.
//!
//! A plugin is a directory with a manifest, `plugin.conf`, in the syntax of the configuration file:
//!
//! ```text
//! name notify-blocked
//! version 1
//! api 1
//! exec ./run.sh
//! events agent-state-changed
//! permissions read-panes send-keys
//! restart on-failure
//! ```
//!
//! `imux plugin link <dir>` checks the manifest and links the directory into the plugins directory
//! (`$IMUX_PLUGINS_DIR`, else `$XDG_CONFIG_HOME/imux/plugins`, else `~/.config/imux/plugins`),
//! `enable` and `disable` switch it on and off (a file in the state directory, so the choice
//! outlives the server), `list` shows what is there and `logs` what a plugin printed. There is no
//! download and no install from a URL: a plugin is a directory the user put there.
//!
//! This file holds what needs no server: the manifest, the permissions, the table of what a
//! command needs, the restart policy, and the files. `server/plugins.rs` runs the processes and
//! decides who is a plugin.
//!
//! The trust line: a linked plugin is code the user chose to run, with the user's rights. The
//! permissions keep a plugin from using the socket for more than it says it needs (so a bug or a
//! confused script cannot type into every pane); they are not a sandbox against a plugin that
//! means harm, which can do anything the user's account can.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::lex;
use crate::proto::API_VERSION;

pub const MANIFEST: &str = "plugin.conf";
const MAX_MANIFEST: usize = 16 * 1024;
const MAX_EVENTS: usize = 64;
const MAX_EXEC_WORDS: usize = 16;
/// A log is cut when it passes this many bytes.
pub const MAX_LOG: u64 = 1024 * 1024;

/// What a plugin may ask of the server besides listening to events and `agent list`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Perm {
    /// Read what the panes show: `capture-pane`, `agent get` (an agent's message comes from the pane).
    ReadPanes,
    /// Type into panes: `send-keys`.
    SendKeys,
    /// Change options, key bindings and hooks: `set-option`, `bind-key`, `set-hook`, `source-file`...
    /// A hook or a binding runs commands, so this reaches further than it sounds.
    SetOptions,
    /// `imux agent prompt` (the server option `agent-control` has to be on too).
    AgentControl,
    /// Create, close, rename and select windows, panes and sessions, and start programs in them.
    Windows,
}

impl Perm {
    pub const ALL: [Perm; 5] = [Perm::ReadPanes, Perm::SendKeys, Perm::SetOptions, Perm::AgentControl, Perm::Windows];

    pub fn name(self) -> &'static str {
        match self {
            Perm::ReadPanes => "read-panes",
            Perm::SendKeys => "send-keys",
            Perm::SetOptions => "set-options",
            Perm::AgentControl => "agent-control",
            Perm::Windows => "windows",
        }
    }

    pub fn parse(word: &str) -> Option<Perm> {
        Perm::ALL.into_iter().find(|p| p.name() == word)
    }
}

/// What a command line sent over the socket needs from a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Anything may do it: it only looks, or only waits.
    Free,
    Perm(Perm),
    /// No plugin may do it.
    Never,
}

/// What `imux <command>` needs. Commands that look are free; the ones that type, read panes or
/// change settings have a permission of their own; every other command changes sessions, windows
/// or panes and needs `windows`. A command that starts a shell command or restarts the plugins is
/// not for plugins.
pub fn needs(command: &str) -> Need {
    match command {
        "list-sessions" | "ls" | "list-windows" | "lsw" | "list-panes" | "lsp" | "list-clients" | "lsc" | "has-session" | "has"
        | "list-keys" | "lsk" | "show-options" | "show" | "show-window-options" | "showw" | "show-environment" | "showenv" | "show-hooks"
        | "display-message" | "display" | "wait-for" | "wait" | "api-agent-list" | "api-agent-resolve" | "api-seq" => Need::Free,
        "capture-pane" | "capturep" | "api-agent-get" | "api-pane-wait" =>Need::Perm(Perm::ReadPanes),
        "send-keys" | "send" | "api-pane-run" => Need::Perm(Perm::SendKeys),
        "set-option" | "set" | "set-window-option" | "setw" | "bind-key" | "bind" | "unbind-key" | "unbind" | "source-file" | "source"
        | "set-hook" | "set-environment" | "setenv" => Need::Perm(Perm::SetOptions),
        "plugin-reload" | "pipe-pane" | "pipep" | "run-shell" | "run" | "if-shell" | "if" | "display-menu" | "menu" | "display-popup" | "popup" => Need::Never,
        _ => Need::Perm(Perm::Windows),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    OnFailure,
    Never,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// The program and its arguments; the program is a path inside the plugin directory.
    pub exec: Vec<String>,
    /// The events the plugin may subscribe to (none: it may not subscribe).
    pub events: Vec<String>,
    pub permissions: Vec<Perm>,
    pub restart: Restart,
}

/// A name a plugin may carry: it becomes a directory and file names.
pub fn valid_name(name: &str) -> bool {
    name.len() <= 32
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

fn one<'a>(key: &str, rest: &'a [String], line: usize) -> Result<&'a str, String> {
    match rest {
        [value] => Ok(value),
        _ => Err(format!("line {line}: {key} takes one value")),
    }
}

/// Read a manifest. Unknown keys are ignored; an unknown permission, a wrong `api`, a missing
/// `name` or `exec`, or an `exec` that leaves the plugin directory is an error.
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    if text.len() > MAX_MANIFEST {
        return Err(format!("the manifest is longer than {MAX_MANIFEST} bytes"));
    }
    let (mut name, mut version, mut api, mut exec) = (None, None, None, None);
    let (mut events, mut permissions, mut restart) = (Vec::new(), Vec::new(), Restart::OnFailure);
    let mut vars = lex::Vars::new();
    for (n, line) in lex::logical_lines(text) {
        for words in lex::statements(&line, &mut vars).map_err(|e| format!("line {n}: {e}"))? {
            let Some((key, rest)) = words.split_first() else { continue };
            match key.as_str() {
                "name" => name = Some(one(key, rest, n)?.to_string()),
                "version" => version = Some(one(key, rest, n)?.to_string()),
                "api" => api = Some(one(key, rest, n)?.parse::<u32>().map_err(|_| format!("line {n}: api is a whole number"))?),
                "exec" => exec = Some(rest.to_vec()),
                "events" => events.extend(rest.iter().cloned()),
                "permissions" => {
                    for word in rest {
                        permissions.push(Perm::parse(word).ok_or_else(|| {
                            let known: Vec<_> = Perm::ALL.iter().map(|p| p.name()).collect();
                            format!("line {n}: unknown permission {word:?} (known: {})", known.join(" "))
                        })?);
                    }
                }
                "restart" => {
                    restart = match one(key, rest, n)? {
                        "on-failure" => Restart::OnFailure,
                        "never" => Restart::Never,
                        other => return Err(format!("line {n}: restart is on-failure or never, not {other:?}")),
                    }
                }
                _ => {}
            }
        }
    }
    let name = name.ok_or("the manifest has no name")?;
    if !valid_name(&name) {
        return Err(format!("bad name {name:?}: lowercase letters, digits, . _ - and at most 32 characters"));
    }
    match api {
        Some(v) if v == API_VERSION => {}
        Some(v) => return Err(format!("the plugin needs api {v}, this imux speaks api {API_VERSION}")),
        None => return Err("the manifest has no api".to_string()),
    }
    let exec = exec.filter(|e| !e.is_empty()).ok_or("the manifest has no exec")?;
    if exec.len() > MAX_EXEC_WORDS {
        return Err(format!("exec has more than {MAX_EXEC_WORDS} words"));
    }
    let program = Path::new(&exec[0]);
    if program.is_absolute() || program.components().any(|c| matches!(c, Component::ParentDir)) || exec[0].contains('\0') {
        return Err(format!("exec {:?} must be a path inside the plugin directory", exec[0]));
    }
    if events.len() > MAX_EVENTS {
        return Err(format!("more than {MAX_EVENTS} events"));
    }
    if let Some(bad) = events.iter().find(|e| e.is_empty() || e.len() > 64 || !e.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')) {
        return Err(format!("bad event name {bad:?}"));
    }
    permissions.sort();
    permissions.dedup();
    Ok(Manifest { name, version: version.unwrap_or_else(|| "0".to_string()), exec, events, permissions, restart })
}

// Restarting.

pub const MAX_RESTARTS: usize = 5;
pub const RESTART_WINDOW: Duration = Duration::from_secs(600);
const FIRST_DELAY: Duration = Duration::from_secs(1);
const LAST_DELAY: Duration = Duration::from_secs(60);
/// A plugin that ran this long before it ended starts over with the first delay.
const STABLE: Duration = Duration::from_secs(60);

/// How much shorter the delays are made for tests: `IMUX_TEST_PLUGIN_BACKOFF_MS` in a debug build
/// is the first delay in milliseconds instead of a second.
pub fn delay_scale() -> u32 {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("IMUX_TEST_PLUGIN_BACKOFF_MS").ok().and_then(|v| v.parse::<u32>().ok()).filter(|ms| (1..=1000).contains(ms)) {
        return 1000 / ms;
    }
    1
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    RestartAfter(Duration),
    /// Too many exits too close together: stop for good.
    GiveUp,
}

/// The restarts of one plugin: the delay doubles from one second to a minute, and at most
/// `MAX_RESTARTS` restarts are made in `RESTART_WINDOW`.
#[derive(Default)]
pub struct Restarts {
    times: VecDeque<Instant>,
    delay: Option<Duration>,
}

impl Restarts {
    /// The plugin has just ended after running for `ran_for`.
    pub fn on_exit(&mut self, now: Instant, ran_for: Duration, scale: u32) -> Decision {
        let scale = scale.max(1);
        while self.times.front().is_some_and(|t| now.saturating_duration_since(*t) > RESTART_WINDOW / scale) {
            self.times.pop_front();
        }
        if ran_for >= STABLE / scale {
            self.delay = None;
        }
        if self.times.len() >= MAX_RESTARTS {
            return Decision::GiveUp;
        }
        let delay = self.delay.map_or(FIRST_DELAY, |d| (d * 2).min(LAST_DELAY));
        self.delay = Some(delay);
        self.times.push_back(now);
        Decision::RestartAfter(delay / scale)
    }
}

// Files.

#[derive(Clone, Debug)]
pub struct Dirs {
    /// Where the plugin directories are linked.
    pub plugins: PathBuf,
    /// Enabled marks, crash notes and logs.
    pub state: PathBuf,
}

impl Dirs {
    pub fn from_env(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> io::Result<Dirs> {
        let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let plugins = if let Some(d) = set("IMUX_PLUGINS_DIR") {
            d
        } else if let Some(d) = set("XDG_CONFIG_HOME") {
            d.join("imux/plugins")
        } else if let Some(h) = set("HOME") {
            h.join(".config/imux/plugins")
        } else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no plugins directory: HOME is not set"));
        };
        let state = crate::persist::state_dir(&env)?.join("plugins");
        Ok(Dirs { plugins, state })
    }

    pub fn current() -> io::Result<Dirs> {
        Dirs::from_env(|k| std::env::var_os(k))
    }

    pub fn log(&self, name: &str) -> PathBuf {
        self.state.join(format!("{name}.log"))
    }

    fn enabled(&self, name: &str) -> PathBuf {
        self.state.join(format!("{name}.enabled"))
    }

    fn crashed(&self, name: &str) -> PathBuf {
        self.state.join(format!("{name}.crashed"))
    }

    fn make(dir: &Path) -> io::Result<()> {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
}

/// A plugin found in the plugins directory.
#[derive(Debug)]
pub struct Installed {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
    pub enabled: bool,
    /// Why the server stopped it, if it did.
    pub crashed: Option<String>,
}

fn read_manifest(dir: &Path, name: &str) -> Result<Manifest, String> {
    let path = dir.join(MANIFEST);
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let manifest = parse_manifest(&text)?;
    if manifest.name != name {
        return Err(format!("the directory is called {name} and the manifest says {}", manifest.name));
    }
    Ok(manifest)
}

/// Every plugin in the plugins directory, by name.
pub fn installed(dirs: &Dirs) -> Vec<Installed> {
    let Ok(entries) = fs::read_dir(&dirs.plugins) else { return Vec::new() };
    let mut out: Vec<Installed> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok().filter(|n| valid_name(n))?;
            let dir = e.path();
            dir.is_dir().then(|| Installed {
                manifest: read_manifest(&dir, &name),
                enabled: dirs.enabled(&name).exists(),
                crashed: fs::read_to_string(dirs.crashed(&name)).ok().map(|t| t.trim().to_string()),
                name,
                dir,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// What a start needs to be true of the files: the manifest and the program belong to the user and
/// nobody else can write them, and the program is an executable file.
pub fn check_files(dir: &Path, manifest: &Manifest) -> Result<PathBuf, String> {
    let uid = nix::unistd::getuid().as_raw();
    let program = dir.join(&manifest.exec[0]);
    for path in [dir.join(MANIFEST), program.clone()] {
        let meta = fs::metadata(&path).map_err(|e| format!("cannot use {}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        if meta.uid() != uid {
            return Err(format!("{} belongs to another user", path.display()));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(format!("{} can be written by others: chmod go-w", path.display()));
        }
    }
    let mode = fs::metadata(&program).map_err(|e| e.to_string())?.mode();
    if mode & 0o100 == 0 {
        return Err(format!("{} is not executable: chmod +x", program.display()));
    }
    Ok(program)
}

/// `imux plugin link dir`.
pub fn link(dirs: &Dirs, dir: &Path) -> Result<String, String> {
    let dir = fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("cannot read {}: {e}", dir.join(MANIFEST).display()))?;
    let manifest = parse_manifest(&text)?;
    crate::server::check_event_types(&manifest.events)?;
    check_files(&dir, &manifest)?;
    Dirs::make(&dirs.plugins).map_err(|e| format!("cannot create {}: {e}", dirs.plugins.display()))?;
    let at = dirs.plugins.join(&manifest.name);
    match fs::read_link(&at) {
        Ok(target) if target == dir => return Ok(format!("{} is linked already\n", manifest.name)),
        Ok(target) => return Err(format!("{} is linked to {} already: unlink it first", manifest.name, target.display())),
        Err(_) if at.exists() => return Err(format!("{} exists and is not a link", at.display())),
        Err(_) => {}
    }
    symlink(&dir, &at).map_err(|e| format!("cannot link {}: {e}", at.display()))?;
    let mut out = format!("linked {} {} ({})\n", manifest.name, manifest.version, dir.display());
    let perms: Vec<_> = manifest.permissions.iter().map(|p| p.name()).collect();
    let _ = writeln!(out, "permissions: {}", if perms.is_empty() { "none".to_string() } else { perms.join(" ") });
    out.push_str("it does not run until you enable it: imux plugin enable ");
    out.push_str(&manifest.name);
    out.push('\n');
    Ok(out)
}

/// `imux plugin unlink name`: removes the link, never the directory it points to.
pub fn unlink(dirs: &Dirs, name: &str) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!("bad plugin name {name:?}"));
    }
    let at = dirs.plugins.join(name);
    if fs::read_link(&at).is_err() {
        return Err(format!("{name} is not a linked plugin"));
    }
    fs::remove_file(&at).map_err(|e| format!("cannot remove {}: {e}", at.display()))?;
    for path in [dirs.enabled(name), dirs.crashed(name)] {
        let _ = fs::remove_file(path);
    }
    Ok(format!("unlinked {name}\n"))
}

/// `imux plugin enable|disable name`.
pub fn set_enabled(dirs: &Dirs, name: &str, on: bool) -> Result<String, String> {
    let plugin = installed(dirs).into_iter().find(|p| p.name == name).ok_or_else(|| format!("no plugin {name}: imux plugin link <dir> first"))?;
    Dirs::make(&dirs.state).map_err(|e| format!("cannot create {}: {e}", dirs.state.display()))?;
    if on {
        plugin.manifest.map_err(|e| format!("{name}: {e}"))?;
        fs::write(dirs.enabled(name), b"").map_err(|e| format!("cannot enable {name}: {e}"))?;
        let _ = fs::remove_file(dirs.crashed(name));
        Ok(format!("enabled {name}\n"))
    } else {
        match fs::remove_file(dirs.enabled(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot disable {name}: {e}")),
        }
        Ok(format!("disabled {name}\n"))
    }
}

/// The server stopped a plugin for good and says why; it shows in `plugin list`.
pub fn mark_crashed(dirs: &Dirs, name: &str, why: &str) {
    let _ = Dirs::make(&dirs.state);
    let _ = fs::write(dirs.crashed(name), why);
    let _ = fs::remove_file(dirs.enabled(name));
}

/// `imux plugin list`.
pub fn list(dirs: &Dirs) -> String {
    let all = installed(dirs);
    if all.is_empty() {
        return format!("no plugins in {}\n", dirs.plugins.display());
    }
    let mut out = String::new();
    for p in all {
        let state = match (&p.crashed, p.enabled) {
            (Some(why), _) => format!("stopped: {why}"),
            (None, true) => "enabled".to_string(),
            (None, false) => "disabled".to_string(),
        };
        match &p.manifest {
            Ok(m) => {
                let perms: Vec<_> = m.permissions.iter().map(|p| p.name()).collect();
                let _ = writeln!(out, "{}\t{}\t{}\tpermissions: {}", p.name, m.version, state, if perms.is_empty() { "none".to_string() } else { perms.join(" ") });
            }
            Err(e) => {
                let _ = writeln!(out, "{}\t-\tbroken: {e}", p.name);
            }
        }
    }
    out
}

/// `imux plugin logs name`: the last `lines` lines of the log.
pub fn logs(dirs: &Dirs, name: &str, lines: usize) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!("bad plugin name {name:?}"));
    }
    let path = dirs.log(name);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(format!("{name} has not printed anything\n")),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    Ok(tail(&text, lines))
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let from = all.len().saturating_sub(lines);
    let mut out = all[from..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Cut a log that has grown past `MAX_LOG`: the old part moves to `<log>.1` (one generation) and
/// the file is emptied in place, so a plugin that writes to it with O_APPEND goes on.
pub fn cut_log(path: &Path) -> io::Result<bool> {
    let len = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if len <= MAX_LOG {
        return Ok(false);
    }
    let mut old = path.as_os_str().to_owned();
    old.push(".1");
    fs::copy(path, &old)?;
    fs::OpenOptions::new().write(true).open(path)?.set_len(0)?;
    Ok(true)
}

/// Open a plugin's log for appending, private to the user, never following a link.
pub fn open_log(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        Dirs::make(parent)?;
    }
    let _ = cut_log(path);
    let file = fs::OpenOptions::new().create(true).append(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path)?;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    Ok(file)
}

/// `imux plugin ...` words to the text to print.
pub fn run_cli(dirs: &Dirs, words: &[String]) -> Result<String, String> {
    const USAGE: &str = "usage: imux plugin link dir | unlink name | list | enable name | disable name | logs name";
    match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["link", dir] => link(dirs, Path::new(dir)),
        ["unlink", name] => unlink(dirs, name),
        ["list"] => Ok(list(dirs)),
        ["enable", name] => set_enabled(dirs, name, true),
        ["disable", name] => set_enabled(dirs, name, false),
        ["logs", name] => logs(dirs, name, 100),
        _ => Err(USAGE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "name notify-blocked\nversion 1.2\napi 1\nexec ./run.sh --quiet\nevents agent-state-changed pane-died\npermissions send-keys read-panes\nrestart never\n";

    #[test]
    fn a_manifest_is_read() {
        let m = parse_manifest(GOOD).unwrap();
        assert_eq!(m.name, "notify-blocked");
        assert_eq!(m.version, "1.2");
        assert_eq!(m.exec, ["./run.sh", "--quiet"]);
        assert_eq!(m.events, ["agent-state-changed", "pane-died"]);
        assert_eq!(m.permissions, [Perm::ReadPanes, Perm::SendKeys], "sorted");
        assert_eq!(m.restart, Restart::Never);
    }

    #[test]
    fn defaults_comments_quotes_and_unknown_keys() {
        let m = parse_manifest("# a plugin\nname p\napi 1\nexec \"my run.sh\"  # trailing\nfuture-key a b c\n").unwrap();
        assert_eq!((m.version.as_str(), m.restart, m.permissions.len(), m.events.len()), ("0", Restart::OnFailure, 0, 0));
        assert_eq!(m.exec, ["my run.sh"]);
        let m = parse_manifest("name p\napi 1\nexec x\npermissions windows windows send-keys\n").unwrap();
        assert_eq!(m.permissions, [Perm::SendKeys, Perm::Windows], "once each");
    }

    #[test]
    fn a_bad_manifest_says_why() {
        let table = [
            ("api 1\nexec x\n", "no name"),
            ("name p\nexec x\n", "no api"),
            ("name p\napi 1\n", "no exec"),
            ("name p\napi 1\nexec\n", "no exec"),
            ("name p\napi 2\nexec x\n", "needs api 2"),
            ("name p\napi one\nexec x\n", "whole number"),
            ("name P\napi 1\nexec x\n", "bad name"),
            ("name ../x\napi 1\nexec x\n", "bad name"),
            ("name p\napi 1\nexec /bin/sh\n", "inside the plugin directory"),
            ("name p\napi 1\nexec ../run.sh\n", "inside the plugin directory"),
            ("name p\napi 1\nexec a/../../b\n", "inside the plugin directory"),
            ("name p\napi 1\nexec x\npermissions shell\n", "unknown permission"),
            ("name p\napi 1\nexec x\nrestart sometimes\n", "on-failure or never"),
            ("name p\nname q\napi 1 2\nexec x\n", "takes one value"),
            ("name p\napi 1\nexec x\nevents Bad_Name\n", "bad event name"),
            ("name 'p\napi 1\nexec x\n", "line"),
        ];
        for (text, want) in table {
            let e = parse_manifest(text).unwrap_err();
            assert!(e.contains(want), "{text:?}: {e}");
        }
        assert!(parse_manifest(&format!("name p\napi 1\nexec x\n# {}\n", "x".repeat(MAX_MANIFEST))).is_err());
        let many = format!("name p\napi 1\nexec x\nevents {}\n", vec!["a"; MAX_EVENTS + 1].join(" "));
        assert!(parse_manifest(&many).is_err());
    }

    #[test]
    fn plugin_names() {
        for ok in ["a", "notify-blocked", "x.y_z", "9lives", &"a".repeat(32)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", ".a", "A", "a b", "a/b", "..", &"a".repeat(33)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn what_a_command_needs() {
        for free in ["list-windows", "lsw", "has-session", "display-message", "show-options", "wait-for", "list-keys"] {
            assert_eq!(needs(free), Need::Free, "{free}");
        }
        assert_eq!(needs("capture-pane"), Need::Perm(Perm::ReadPanes));
        assert_eq!(needs("capturep"), Need::Perm(Perm::ReadPanes));
        assert_eq!(needs("send-keys"), Need::Perm(Perm::SendKeys));
        assert_eq!(needs("send"), Need::Perm(Perm::SendKeys));
        for set in ["set-option", "set", "bind-key", "unbind", "source-file", "set-hook", "set-environment"] {
            assert_eq!(needs(set), Need::Perm(Perm::SetOptions), "{set}");
        }
        for win in ["new-window", "kill-pane", "rename-window", "select-window", "split-window", "respawn-pane", "kill-session", "something-new"] {
            assert_eq!(needs(win), Need::Perm(Perm::Windows), "{win}");
        }
        for never in ["plugin-reload", "pipe-pane", "run-shell", "if-shell"] {
            assert_eq!(needs(never), Need::Never, "{never}");
        }
    }

    #[test]
    fn every_command_the_socket_runs_has_a_decision() {
        for name in crate::server::remote_command_names() {
            let _ = needs(name);
        }
        for name in crate::server::remote_command_names().iter().filter(|n| n.starts_with("kill") || n.starts_with("new") || n.starts_with("split")) {
            assert_eq!(needs(name), Need::Perm(Perm::Windows), "{name}");
        }
    }

    #[test]
    fn restarts_back_off_and_give_up() {
        let (t0, mut r) = (Instant::now(), Restarts::default());
        let at = |s: u64| t0 + Duration::from_secs(s);
        let quick = Duration::from_secs(1);
        assert_eq!(r.on_exit(at(0), quick, 1), Decision::RestartAfter(Duration::from_secs(1)));
        assert_eq!(r.on_exit(at(2), quick, 1), Decision::RestartAfter(Duration::from_secs(2)));
        assert_eq!(r.on_exit(at(5), quick, 1), Decision::RestartAfter(Duration::from_secs(4)));
        assert_eq!(r.on_exit(at(10), quick, 1), Decision::RestartAfter(Duration::from_secs(8)));
        assert_eq!(r.on_exit(at(20), quick, 1), Decision::RestartAfter(Duration::from_secs(16)));
        assert_eq!(r.on_exit(at(40), quick, 1), Decision::GiveUp, "the sixth exit within ten minutes");
    }

    #[test]
    fn the_delay_stops_at_a_minute_and_restarts_older_than_ten_minutes_do_not_count() {
        let (t0, mut r) = (Instant::now(), Restarts::default());
        let at = |s: u64| t0 + Duration::from_secs(s);
        let ran = Duration::from_secs(1);
        let mut last = Duration::ZERO;
        for i in 0..5 {
            if let Decision::RestartAfter(d) = r.on_exit(at(i * 700), ran, 1) {
                last = d;
            }
        }
        assert_eq!(last, Duration::from_secs(16), "spread out, so all five are allowed");
        let mut r = Restarts { delay: Some(Duration::from_secs(40)), ..Restarts::default() };
        assert_eq!(r.on_exit(at(0), ran, 1), Decision::RestartAfter(Duration::from_secs(60)), "capped");
        assert_eq!(r.on_exit(at(1), ran, 1), Decision::RestartAfter(Duration::from_secs(60)));
    }

    #[test]
    fn a_plugin_that_ran_a_while_starts_over() {
        let (t0, mut r) = (Instant::now(), Restarts::default());
        r.on_exit(t0, Duration::from_secs(1), 1);
        r.on_exit(t0 + Duration::from_secs(2), Duration::from_secs(1), 1);
        assert_eq!(r.on_exit(t0 + Duration::from_secs(300), Duration::from_secs(120), 1), Decision::RestartAfter(Duration::from_secs(1)));
    }

    #[test]
    fn the_scale_shortens_delays_and_windows() {
        let (t0, mut r) = (Instant::now(), Restarts::default());
        assert_eq!(r.on_exit(t0, Duration::ZERO, 1000), Decision::RestartAfter(Duration::from_millis(1)));
        assert_eq!(r.on_exit(t0, Duration::ZERO, 1000), Decision::RestartAfter(Duration::from_millis(2)));
        let later = t0 + Duration::from_secs(1);
        for _ in 0..MAX_RESTARTS {
            assert!(matches!(r.on_exit(later, Duration::ZERO, 1000), Decision::RestartAfter(_)));
        }
        assert_eq!(r.on_exit(later, Duration::ZERO, 1000), Decision::GiveUp);
        let much_later = t0 + Duration::from_secs(5);
        assert!(matches!(r.on_exit(much_later, Duration::ZERO, 1000), Decision::RestartAfter(_)), "a window of 600 ms has passed");
    }

    #[test]
    fn directories_come_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).into());
        let d = Dirs::from_env(env(&[("IMUX_PLUGINS_DIR", "/p"), ("IMUX_STATE_DIR", "/s")])).unwrap();
        assert_eq!((d.plugins, d.state), (PathBuf::from("/p"), PathBuf::from("/s/plugins")));
        let d = Dirs::from_env(env(&[("XDG_CONFIG_HOME", "/x"), ("IMUX_STATE_DIR", "/s")])).unwrap();
        assert_eq!(d.plugins, PathBuf::from("/x/imux/plugins"));
        let d = Dirs::from_env(env(&[("HOME", "/h")])).unwrap();
        assert_eq!((d.plugins, d.state), (PathBuf::from("/h/.config/imux/plugins"), PathBuf::from("/h/.local/state/imux/plugins")));
        assert!(Dirs::from_env(env(&[])).is_err());
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imux-plugins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plugin_dir(root: &Path, name: &str, manifest: &str) -> PathBuf {
        let dir = root.join(format!("src-{name}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MANIFEST), manifest).unwrap();
        fs::write(dir.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(dir.join(MANIFEST), fs::Permissions::from_mode(0o644)).unwrap();
        dir
    }

    #[test]
    fn link_enable_list_disable_unlink() {
        let root = scratch("flow");
        let dirs = Dirs { plugins: root.join("plugins"), state: root.join("state") };
        let src = plugin_dir(&root, "hello", "name hello\napi 1\nexec ./run.sh\npermissions send-keys\n");
        let out = link(&dirs, &src).unwrap();
        assert!(out.contains("linked hello") && out.contains("permissions: send-keys") && out.contains("imux plugin enable hello"), "{out}");
        assert!(link(&dirs, &src).unwrap().contains("linked already"));
        let listing = list(&dirs);
        assert!(listing.starts_with("hello\t0\tdisabled\tpermissions: send-keys"), "{listing}");
        set_enabled(&dirs, "hello", true).unwrap();
        assert!(list(&dirs).contains("\tenabled\t"));
        assert!(installed(&dirs)[0].enabled);
        mark_crashed(&dirs, "hello", "exited 5 times in 10 minutes");
        let listing = list(&dirs);
        assert!(listing.contains("stopped: exited 5 times"), "{listing}");
        assert!(!installed(&dirs)[0].enabled, "stopping for good switches it off");
        set_enabled(&dirs, "hello", true).unwrap();
        assert!(installed(&dirs)[0].crashed.is_none(), "enabling clears the note");
        set_enabled(&dirs, "hello", false).unwrap();
        set_enabled(&dirs, "hello", false).unwrap();
        assert!(!installed(&dirs)[0].enabled);
        assert!(unlink(&dirs, "hello").unwrap().contains("unlinked"));
        assert!(src.join(MANIFEST).exists(), "the directory itself stays");
        assert!(unlink(&dirs, "hello").is_err());
        assert!(set_enabled(&dirs, "hello", true).is_err());
        assert!(list(&dirs).starts_with("no plugins"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn linking_refuses_what_start_would_refuse() {
        let root = scratch("refuse");
        let dirs = Dirs { plugins: root.join("plugins"), state: root.join("state") };
        let src = plugin_dir(&root, "w", "name w\napi 1\nexec ./run.sh\n");
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o757)).unwrap();
        assert!(link(&dirs, &src).unwrap_err().contains("can be written by others"));
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(link(&dirs, &src).unwrap_err().contains("not executable"));
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(src.join(MANIFEST), "name w\napi 1\nexec ./run.sh\nevents no-such-event\n").unwrap();
        assert!(link(&dirs, &src).unwrap_err().contains("unknown event type"));
        fs::write(src.join(MANIFEST), "name other\napi 1\nexec ./run.sh\n").unwrap();
        link(&dirs, &src).unwrap();
        assert!(list(&dirs).contains("broken") || list(&dirs).contains("other"));
        assert!(link(&dirs, &root.join("missing")).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unlink_never_deletes_a_real_directory() {
        let root = scratch("real");
        let dirs = Dirs { plugins: root.join("plugins"), state: root.join("state") };
        fs::create_dir_all(dirs.plugins.join("precious")).unwrap();
        assert!(unlink(&dirs, "precious").unwrap_err().contains("not a linked plugin"));
        assert!(dirs.plugins.join("precious").is_dir());
        assert!(unlink(&dirs, "../precious").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn logs_show_the_tail_and_are_cut_in_place() {
        let root = scratch("logs");
        let dirs = Dirs { plugins: root.join("plugins"), state: root.join("state") };
        assert!(logs(&dirs, "p", 3).unwrap().contains("has not printed"));
        let mut file = open_log(&dirs.log("p")).unwrap();
        for i in 0..10 {
            io::Write::write_all(&mut file, format!("line {i}\n").as_bytes()).unwrap();
        }
        assert_eq!(logs(&dirs, "p", 3).unwrap(), "line 7\nline 8\nline 9\n");
        assert_eq!(fs::metadata(dirs.log("p")).unwrap().mode() & 0o777, 0o600);
        assert!(!cut_log(&dirs.log("p")).unwrap(), "small logs stay");
        io::Write::write_all(&mut file, &vec![b'x'; MAX_LOG as usize]).unwrap();
        assert!(cut_log(&dirs.log("p")).unwrap());
        io::Write::write_all(&mut file, b"after\n").unwrap();
        assert_eq!(fs::read_to_string(dirs.log("p")).unwrap(), "after\n", "the writer goes on at the new start");
        assert!(fs::metadata(root.join("state/p.log.1")).unwrap().len() > MAX_LOG);
        assert!(logs(&dirs, "../p", 3).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_log_that_is_a_link_is_not_followed() {
        let root = scratch("linklog");
        let dirs = Dirs { plugins: root.join("plugins"), state: root.join("state") };
        fs::create_dir_all(&dirs.state).unwrap();
        let target = root.join("victim");
        fs::write(&target, "keep").unwrap();
        symlink(&target, dirs.log("p")).unwrap();
        assert!(open_log(&dirs.log("p")).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_cli_words() {
        let root = scratch("cli");
        let dirs = Dirs { plugins: root.join("plugins"), state: root.join("state") };
        let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert!(run_cli(&dirs, &w("list")).unwrap().starts_with("no plugins"));
        for bad in ["", "link", "enable", "enable a b", "frobnicate x", "list now"] {
            assert!(run_cli(&dirs, &w(bad)).unwrap_err().starts_with("usage:"), "{bad}");
        }
        let _ = fs::remove_dir_all(&root);
    }
}
