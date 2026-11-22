//! Running plugins (the manifest, the permissions and the files are in `crate::plugins`).
//!
//! The server starts every enabled plugin after the configuration is read and looks after them in
//! the housekeeping turn, without a thread of its own: a plugin is a child process in a process
//! group of its own, with the standard input closed and its output in `<state>/plugins/<name>.log`.
//! One that ends is started again after a growing delay unless its manifest says `restart never`
//! or it ended cleanly; one that ends too often is stopped for good and marked in `plugin list`.
//! Stopping (disable, unlink, the server ending) is SIGTERM to the group and SIGKILL after two
//! seconds.
//!
//! Who is a plugin: the process on the other end of a connection (`LOCAL_PEERPID`) is a plugin when
//! it descends from the process the server started. Everyone else, a pane program or the user, is
//! judged as before (`command-from-panes`). A plugin may do what its permissions say: the check
//! is `plugin_gate` (per request), `plugin_allows_command` (per `imux <command>`) and
//! `plugin_event_types` (what it may follow), called by the handlers. A
//! process that leaves the tree on purpose (a double fork) is not recognized and looks like the
//! user: the permissions guard against mistakes, not against a plugin that means harm.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use super::Server;
use crate::log;
use crate::plugins::{self as lib, Decision, Dirs, Manifest, Need, Perm, Restart, Restarts};
use crate::proto::ClientMsg;
use crate::tty::proc;

/// How long a plugin has to end after SIGTERM before it is killed.
const TERM_GRACE: Duration = Duration::from_secs(2);

/// What a plugin may see of the environment of the server.
const KEEP_ENV: [&str; 10] = ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "TERM"];

enum State {
    Running { child: Child, since: Instant },
    /// Ended badly; starts again at `until`.
    Waiting { until: Instant },
    /// Told to end (disabled, unlinked, the server ends); killed if it does not.
    Stopping { child: Child, since: Instant },
    /// Ended and not to be started again.
    Ended,
}

struct Plugin {
    manifest: Manifest,
    dir: PathBuf,
    state: State,
    restarts: Restarts,
    /// Enabled again while it was stopping: start it once it has ended.
    revive: bool,
}

impl Plugin {
    /// The pid of the process the server started, while there is one.
    fn pid(&self) -> Option<i32> {
        match &self.state {
            State::Running { child, .. } | State::Stopping { child, .. } => i32::try_from(child.id()).ok(),
            State::Waiting { .. } | State::Ended => None,
        }
    }
}

#[derive(Default)]
pub(super) struct Runtime {
    list: Vec<Plugin>,
    dirs: Option<Dirs>,
    /// The plugin (by name) each connection belongs to, `None` for a connection that is not one.
    /// Worked out once per connection, and only while a plugin runs.
    peers: HashMap<usize, Option<String>>,
}

/// What a request from a plugin needs. `Run` is free here and judged per command (`lib::needs`).
fn message_need(msg: &ClientMsg) -> Need {
    match msg {
        ClientMsg::List
        | ClientMsg::ListKeys
        | ClientMsg::AgentList
        | ClientMsg::AgentWait(_)
        | ClientMsg::Subscribe(_)
        | ClientMsg::Run(_)
        | ClientMsg::Version(_) => Need::Free,
        ClientMsg::AgentGet(_) => Need::Perm(Perm::ReadPanes),
        ClientMsg::AgentPrompt(_) => Need::Perm(Perm::AgentControl),
        _ => Need::Never,
    }
}

fn signal_group(child: &Child, signal: Signal) {
    let group = Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
    // ESRCH: gone already. EPERM: the leader ended and is waiting to be collected (macOS).
    if let Err(e) = killpg(group, signal)
        && !matches!(e, nix::errno::Errno::ESRCH | nix::errno::Errno::EPERM)
    {
        log::warn(&format!("plugin: {signal} to group {group}: {e}"));
    }
}

fn spawn(dirs: &Dirs, manifest: &Manifest, dir: &Path) -> Result<Child, String> {
    super::check_event_types(&manifest.events)?;
    let program = lib::check_files(dir, manifest)?;
    let log_file = lib::open_log(&dirs.log(&manifest.name)).map_err(|e| format!("cannot open the log: {e}"))?;
    let log_err = log_file.try_clone().map_err(|e| format!("cannot share the log: {e}"))?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot find imux itself: {e}"))?;
    let mut command = Command::new(&program);
    command.args(&manifest.exec[1..]).current_dir(dir).env_clear();
    for key in KEEP_ENV {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    // So that a script finds `imux` by name, and by `$IMUX_EXE`.
    if let Some(bin) = exe.parent() {
        let mut paths = vec![bin.to_path_buf()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        if let Ok(joined) = std::env::join_paths(paths) {
            command.env("PATH", joined);
        }
    }
    command
        .env("IMUX_PLUGIN", &manifest.name)
        .env("IMUX_SOCKET_PATH", crate::paths::socket_path())
        .env("IMUX_EXE", exe)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .process_group(0);
    command.spawn().map_err(|e| format!("cannot start {}: {e}", program.display()))
}

impl Server {
    /// Start the plugins that are enabled. Called once, after the configuration.
    pub(super) fn plugins_start(&mut self) {
        match Dirs::current() {
            Ok(dirs) => self.plugins.dirs = Some(dirs),
            Err(e) => return log::warn(&format!("plugins are off: {e}")),
        }
        match self.plugin_reload() {
            Ok(text) if !text.is_empty() => log::info(text.trim_end()),
            Ok(_) => {}
            Err(e) => log::warn(&format!("plugins: {e}")),
        }
    }

    /// `imux plugin-reload` (what `plugin enable`, `disable` and `unlink` call): start the enabled
    /// plugins that do not run, stop the ones that are not enabled any more.
    pub(super) fn plugin_reload(&mut self) -> Result<String, String> {
        let dirs = self.plugins.dirs.clone().ok_or("plugins are off")?;
        let found = lib::installed(&dirs);
        let mut said = String::new();
        let now = Instant::now();
        for p in self.plugins.list.iter_mut() {
            let wanted = found.iter().any(|f| f.name == p.manifest.name && f.enabled && f.manifest.is_ok());
            p.revive = wanted && matches!(p.state, State::Stopping { .. });
            if p.revive {
                said.push_str(&format!("{} starts again when it has stopped\n", p.manifest.name));
            }
            if !wanted {
                match std::mem::replace(&mut p.state, State::Ended) {
                    State::Running { child, .. } => {
                        signal_group(&child, Signal::SIGTERM);
                        p.state = State::Stopping { child, since: now };
                        said.push_str(&format!("stopping {}\n", p.manifest.name));
                    }
                    State::Stopping { child, since } => p.state = State::Stopping { child, since },
                    State::Waiting { .. } | State::Ended => said.push_str(&format!("stopped {}\n", p.manifest.name)),
                }
            }
        }
        self.plugins.list.retain(|p| !matches!(p.state, State::Ended));
        for f in found.into_iter().filter(|f| f.enabled) {
            let Ok(manifest) = f.manifest else {
                if let Err(e) = &f.manifest {
                    said.push_str(&format!("{}: {e}\n", f.name));
                }
                continue;
            };
            if self.plugins.list.iter().any(|p| p.manifest.name == manifest.name) {
                continue;
            }
            match spawn(&dirs, &manifest, &f.dir) {
                Ok(child) => {
                    log::info(&format!("plugin {} started (pid {})", manifest.name, child.id()));
                    said.push_str(&format!("started {}\n", manifest.name));
                    self.plugins.list.push(Plugin {
                        manifest,
                        dir: f.dir,
                        state: State::Running { child, since: now },
                        restarts: Restarts::default(),
                        revive: false,
                    });
                }
                Err(e) => {
                    log::error(&format!("plugin {}: {e}", manifest.name));
                    said.push_str(&format!("{}: {e}\n", manifest.name));
                }
            }
        }
        Ok(said)
    }

    /// Every housekeeping turn: collect the plugins that ended, start the ones whose delay is over,
    /// kill the ones that did not obey SIGTERM, keep the logs small.
    pub(super) fn plugins_tick(&mut self) {
        let Some(dirs) = self.plugins.dirs.clone() else { return };
        if self.plugins.list.is_empty() {
            return;
        }
        let (now, scale) = (Instant::now(), lib::delay_scale());
        for p in &mut self.plugins.list {
            let name = p.manifest.name.clone();
            match &mut p.state {
                State::Running { child, since } => match child.try_wait() {
                    Ok(None) => {
                        if let Err(e) = lib::cut_log(&dirs.log(&name)) {
                            log::warn(&format!("plugin {name}: cannot cut the log: {e}"));
                        }
                    }
                    Ok(Some(status)) => {
                        let ran = since.elapsed();
                        log::info(&format!("plugin {name} ended: {status} after {} s", ran.as_secs()));
                        p.state = if status.success() || p.manifest.restart == Restart::Never {
                            State::Ended
                        } else {
                            match p.restarts.on_exit(now, ran, scale) {
                                Decision::RestartAfter(delay) => State::Waiting { until: now + delay },
                                Decision::GiveUp => {
                                    let why = format!("ended {} times within ten minutes ({status}); see imux plugin logs {name}", lib::MAX_RESTARTS + 1);
                                    log::error(&format!("plugin {name} stopped for good: {why}"));
                                    lib::mark_crashed(&dirs, &name, &why);
                                    State::Ended
                                }
                            }
                        };
                    }
                    Err(e) => log::warn(&format!("plugin {name}: wait: {e}")),
                },
                State::Waiting { until } if *until <= now => match spawn(&dirs, &p.manifest, &p.dir) {
                    Ok(child) => {
                        log::info(&format!("plugin {name} started again (pid {})", child.id()));
                        p.state = State::Running { child, since: now };
                    }
                    Err(e) => {
                        log::error(&format!("plugin {name}: {e}"));
                        p.state = match p.restarts.on_exit(now, Duration::ZERO, scale) {
                            Decision::RestartAfter(delay) => State::Waiting { until: now + delay },
                            Decision::GiveUp => {
                                lib::mark_crashed(&dirs, &name, &format!("cannot be started: {e}"));
                                State::Ended
                            }
                        };
                    }
                },
                State::Waiting { .. } | State::Ended => {}
                State::Stopping { child, since } => match child.try_wait() {
                    Ok(Some(_)) => {
                        p.state = if std::mem::take(&mut p.revive) { State::Waiting { until: now } } else { State::Ended };
                    }
                    Ok(None) if since.elapsed() >= TERM_GRACE => {
                        log::warn(&format!("plugin {name} did not end after SIGTERM: killed"));
                        signal_group(child, Signal::SIGKILL);
                    }
                    Ok(None) => {}
                    Err(e) => log::warn(&format!("plugin {name}: wait: {e}")),
                },
            }
        }
        self.plugins.list.retain(|p| !matches!(p.state, State::Ended));
    }

    /// The server is ending: SIGTERM to every plugin, SIGKILL to the ones still there after the grace.
    pub(super) fn plugins_stop(&mut self) {
        for p in &self.plugins.list {
            if let State::Running { child, .. } | State::Stopping { child, .. } = &p.state {
                signal_group(child, Signal::SIGTERM);
            }
        }
        let deadline = Instant::now() + TERM_GRACE;
        loop {
            for p in &mut self.plugins.list {
                if let State::Running { child, .. } | State::Stopping { child, .. } = &mut p.state
                    && matches!(child.try_wait(), Ok(Some(_)) | Err(_))
                {
                    p.state = State::Ended;
                }
            }
            self.plugins.list.retain(|p| !matches!(p.state, State::Ended));
            if self.plugins.list.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        for p in &mut self.plugins.list {
            if let State::Running { child, .. } | State::Stopping { child, .. } = &mut p.state {
                log::warn(&format!("plugin {} did not end after SIGTERM: killed", p.manifest.name));
                signal_group(child, Signal::SIGKILL);
                let _ = child.wait();
            }
        }
        self.plugins.list.clear();
    }

    // Who is asking.

    /// The index in the list of the plugin that the connection `client` belongs to.
    fn plugin_of(&mut self, client: usize) -> Option<usize> {
        if self.plugins.list.is_empty() {
            return None;
        }
        let name = match self.plugins.peers.get(&client) {
            Some(known) => known.clone(),
            None => {
                let peer = self.clients.get(&client).and_then(|c| c.peer);
                let found = peer.and_then(|peer| {
                    self.plugins.list.iter().find(|p| p.pid().is_some_and(|pid| proc::descends_from(peer, pid))).map(|p| p.manifest.name.clone())
                });
                self.plugins.peers.insert(client, found.clone());
                found
            }
        };
        let name = name?;
        self.plugins.list.iter().position(|p| p.manifest.name == name)
    }

    pub(super) fn forget_plugin_peer(&mut self, client: usize) {
        self.plugins.peers.remove(&client);
    }

    fn denied(&self, index: usize, what: &str, perm: Perm) -> String {
        let name = &self.plugins.list[index].manifest.name;
        format!("plugin {name} may not {what}: it has no permission {} (add it to permissions in its plugin.conf and restart it)", perm.name())
    }

    /// Whether `imux <command>` may be run by the connection: everything but a plugin may, and a
    /// plugin may when its manifest lists the permission the command needs.
    pub(super) fn plugin_allows_command(&mut self, client: usize, command: &str) -> Result<(), String> {
        let Some(i) = self.plugin_of(client) else { return Ok(()) };
        match lib::needs(command) {
            Need::Free => Ok(()),
            Need::Perm(perm) if self.plugins.list[i].manifest.permissions.contains(&perm) => Ok(()),
            Need::Perm(perm) => Err(self.denied(i, command, perm)),
            Need::Never => Err(format!("plugin {} may not run {command}", self.plugins.list[i].manifest.name)),
        }
    }

    /// A plugin gets the requests that look, the ones its permissions cover and `Run` (judged per
    /// command later); it does not attach, type, kill the server, restore or report as a pane.
    pub(super) fn plugin_gate(&mut self, client: usize, msg: &ClientMsg) -> Result<(), String> {
        let Some(i) = self.plugin_of(client) else { return Ok(()) };
        match message_need(msg) {
            Need::Free => Ok(()),
            Need::Perm(perm) if self.plugins.list[i].manifest.permissions.contains(&perm) => Ok(()),
            Need::Perm(perm) => Err(self.denied(i, "do that", perm)),
            Need::Never => Err(format!("plugin {} may not do that", self.plugins.list[i].manifest.name)),
        }
    }

    /// The event types a subscription gets: a plugin only the ones its manifest lists.
    pub(super) fn plugin_event_types(&mut self, client: usize, asked: &[String]) -> Result<Vec<String>, String> {
        let Some(i) = self.plugin_of(client) else { return Ok(asked.to_vec()) };
        let m = &self.plugins.list[i].manifest;
        if m.events.is_empty() {
            return Err(format!("plugin {} lists no events in its plugin.conf", m.name));
        }
        if asked.is_empty() {
            return Ok(m.events.clone());
        }
        match asked.iter().find(|t| !m.events.contains(t)) {
            Some(t) => Err(format!("plugin {} may not follow {t}: it is not in events in its plugin.conf", m.name)),
            None => Ok(asked.to_vec()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Caps, PersistOp, PromptArgs, ReportArgs, RunArgs, Size, Want, WorktreeArgs, WorktreeRemoveArgs};

    fn every_message() -> Vec<(&'static str, ClientMsg)> {
        let size = Size { cols: 80, rows: 24, xpix: 0, ypix: 0 };
        vec![
            ("Hello", ClientMsg::Hello(size)),
            ("Input", ClientMsg::Input(b"x".to_vec())),
            ("Resize", ClientMsg::Resize(size)),
            ("Kill", ClientMsg::Kill),
            ("Want", ClientMsg::Want(Want::Create(None))),
            ("List", ClientMsg::List),
            ("Caps", ClientMsg::Caps(Caps::BASIC)),
            ("Persist", ClientMsg::Persist(PersistOp::List)),
            ("Worktree", ClientMsg::Worktree(WorktreeArgs::default())),
            ("AgentList", ClientMsg::AgentList),
            ("AgentGet", ClientMsg::AgentGet(1)),
            ("AgentWait", ClientMsg::AgentWait(crate::proto::AgentWaitArgs { pane: 1, until: 0, timeout_ms: 0 })),
            ("PaneReport", ClientMsg::PaneReport(ReportArgs::default())),
            ("PaneRelease", ClientMsg::PaneRelease(1, "s".into())),
            ("WorktreeRemove", ClientMsg::WorktreeRemove(WorktreeRemoveArgs::default())),
            ("ListKeys", ClientMsg::ListKeys),
            ("Run", ClientMsg::Run(RunArgs::default())),
            ("Env", ClientMsg::Env(Vec::new())),
            ("Subscribe", ClientMsg::Subscribe(Vec::new())),
            ("AgentPrompt", ClientMsg::AgentPrompt(PromptArgs { pane: 1, wait: false, timeout_ms: 0, text: "x".into() })),
            ("Modes", ClientMsg::Modes(7)),
        ]
    }

    #[test]
    fn what_each_request_of_a_plugin_needs() {
        let need: HashMap<&str, Need> = every_message().iter().map(|(n, m)| (*n, message_need(m))).collect();
        for free in ["List", "ListKeys", "AgentList", "AgentWait", "Subscribe", "Run"] {
            assert_eq!(need[free], Need::Free, "{free}");
        }
        assert_eq!(need["AgentGet"], Need::Perm(Perm::ReadPanes));
        assert_eq!(need["AgentPrompt"], Need::Perm(Perm::AgentControl));
        for never in ["Hello", "Input", "Resize", "Kill", "Want", "Caps", "Modes", "Persist", "Worktree", "PaneReport", "PaneRelease", "WorktreeRemove", "Env"] {
            assert_eq!(need[never], Need::Never, "{never}: a plugin does not attach, type, kill, restore or report as a pane");
        }
    }

    #[test]
    fn the_version_every_client_starts_with_is_free_for_a_plugin_too() {
        assert_eq!(message_need(&ClientMsg::Version(1)), Need::Free, "a plugin runs `imux`, which says its version first");
    }

    #[test]
    fn each_permission_opens_only_its_own_requests() {
        for perm in Perm::ALL {
            let opens: Vec<&str> = every_message().iter().filter(|(_, m)| message_need(m) == Need::Perm(perm)).map(|(n, _)| *n).collect();
            let commands: Vec<&str> = ["capture-pane", "send-keys", "set-option", "new-window", "list-windows", "run-shell"]
                .into_iter()
                .filter(|c| lib::needs(c) == Need::Perm(perm))
                .collect();
            let expected: (&[&str], &[&str]) = match perm {
                Perm::ReadPanes => (&["AgentGet"], &["capture-pane"]),
                Perm::SendKeys => (&[], &["send-keys"]),
                Perm::SetOptions => (&[], &["set-option"]),
                Perm::AgentControl => (&["AgentPrompt"], &[]),
                Perm::Windows => (&[], &["new-window"]),
            };
            assert_eq!((opens.as_slice(), commands.as_slice()), expected, "{}", perm.name());
        }
    }

    #[test]
    fn a_plugin_gets_a_small_clean_environment() {
        assert!(!KEEP_ENV.iter().any(|k| k.starts_with("IMUX") || k.contains("TOKEN") || k.contains("KEY") || k.contains("SECRET")));
        assert!(KEEP_ENV.contains(&"PATH") && KEEP_ENV.contains(&"TMPDIR"), "TMPDIR is how `imux` finds the socket");
    }
}
