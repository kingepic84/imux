//! `worktree` on the server: git runs on a helper thread (a fetch can take seconds and the event
//! loop must not stall), and the result wakes the loop, which builds the window and answers.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use mio::{Registry, Token, Waker};

use super::Server;
use super::modes::{Entry, Mode, Picker, Prompt, PromptKind};
use super::restore::{PaneSpec, WindowSpec};
use crate::config::format;
use crate::log;
use crate::proto::{ServerMsg, WorktreeArgs, WorktreeRemoveArgs};
use crate::shell::ShellKind;
use crate::worktree::{self, PaneKind, Prepared, RemoveRequest, Removed, Request, Settings};

pub(super) const WAKE: Token = Token(1);

/// Who is told how it went.
#[derive(Clone, Copy, Debug)]
pub(super) enum Reply {
    /// A client at its keyboard: a status message.
    Client(usize),
    /// A one-shot `imux worktree` connection: text or error, then closed.
    Cli(usize),
}

/// What a finished helper thread hands back.
enum Work {
    Open { settings: Box<Settings>, result: Result<Prepared, String> },
    Remove(Result<Removed, String>),
}

pub(super) struct Done {
    reply: Reply,
    session: usize,
    work: Work,
}

pub(super) struct Jobs {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    waker: Arc<Waker>,
}

impl Jobs {
    /// The waker other helper threads (`run-shell`) use to get the loop's attention.
    pub(super) fn waker(&self) -> Arc<Waker> {
        Arc::clone(&self.waker)
    }

    pub(super) fn new(registry: &Registry) -> std::io::Result<Jobs> {
        let (tx, rx) = channel();
        Ok(Jobs { tx, rx, waker: Arc::new(Waker::new(registry, WAKE)?) })
    }
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME").map_or_else(|| PathBuf::from(path), PathBuf::from);
    }
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map_or_else(|| PathBuf::from(path), |h| PathBuf::from(h).join(rest)),
        None => PathBuf::from(path),
    }
}

impl Server {
    fn settings(&self) -> Settings {
        let o = &self.options;
        Settings {
            base: o.text("worktree-base").to_string(),
            detach: o.flag("worktree-detach"),
            pattern: o.text("worktree-name-pattern").to_string(),
            path: o.text("worktree-path").to_string(),
            branch_format: o.text("worktree-branch-format").to_string(),
            layout: o.text("worktree-layout").to_string(),
            command: o.text("worktree-command").to_string(),
            start: o.text("worktree-start").to_string(),
            setup: o.text("worktree-setup").to_string(),
            setup_forced: false,
        }
    }

    fn answer(&mut self, reply: Reply, result: Result<String, String>) {
        match reply {
            Reply::Client(id) => {
                let text = result.unwrap_or_else(|e| e);
                self.show_message(id, text.replace('\n', ", "));
            }
            Reply::Cli(id) => {
                let msg = match result {
                    Ok(t) => ServerMsg::Text(t + "\n"),
                    Err(e) => ServerMsg::Error(e),
                };
                self.queue(id, &msg);
                self.close_after_flush(id);
            }
        }
    }

    /// Validate the request, then hand the git work to a thread.
    pub(super) fn start_worktree(&mut self, reply: Reply, args: WorktreeArgs) {
        let mut settings = self.settings();
        if let Some(d) = args.detach {
            settings.detach = d;
        }
        if let Some(s) = &args.start {
            settings.start = s.clone();
        }
        if let Some(s) = &args.setup {
            settings.setup = s.clone();
            settings.setup_forced = !s.is_empty();
        }
        let session = match reply {
            Reply::Client(c) => self.client_session(c),
            Reply::Cli(_) => self.sessions.iter().position(|s| s.id == self.recent),
        };
        let Some(si) = session else { return self.answer(reply, Err("no session to open the window in".into())) };
        let from = match (&args.from, reply) {
            (Some(f), _) => Some(expand_home(f)),
            (None, Reply::Client(_)) => self.active_cwd(si),
            (None, Reply::Cli(_)) => None,
        };
        let Some(from) = from else { return self.answer(reply, Err("no directory to look for a repository in".into())) };
        if let Err(e) = worktree::check_name(&args.name, &settings.pattern) {
            return self.answer(reply, Err(e));
        }
        let repo = args.repo.as_deref().map(expand_home).map(|r| if r.is_absolute() { r } else { from.join(r) });
        let req = Request {
            repo,
            from,
            name: args.name,
            base: args.base,
            detach: settings.detach,
            pattern: settings.pattern.clone(),
            path_format: settings.path.clone(),
            branch_format: settings.branch_format.clone(),
            slug: args.slug.filter(|s| !s.is_empty()),
            base_option: settings.base.clone(),
        };
        let session_id = self.sessions[si].id;
        let spawned = self.spawn_work(reply, session_id, move || {
            let result = std::panic::catch_unwind(|| worktree::prepare(&req))
                .unwrap_or_else(|p| Err(format!("worktree failed: {}", log::panic_text(p.as_ref()))));
            Work::Open { settings: Box::new(settings), result }
        });
        match spawned {
            Ok(()) => self.show_pending(reply),
            Err(e) => self.answer(reply, Err(format!("cannot start worktree thread: {e}"))),
        }
    }

    /// Run `job` on a helper thread; its result comes back through the channel and the waker.
    fn spawn_work(&self, reply: Reply, session: usize, job: impl FnOnce() -> Work + Send + 'static) -> std::io::Result<()> {
        let (tx, waker) = (self.jobs.tx.clone(), Arc::clone(&self.jobs.waker));
        std::thread::Builder::new().name("worktree".into()).spawn(move || {
            let work = job();
            if tx.send(Done { reply, session, work }).is_err() {
                log::warn("worktree finished after the server stopped listening");
            }
            if let Err(e) = waker.wake() {
                log::error(&format!("waking the server: {e}"));
            }
        })?;
        Ok(())
    }

    /// `worktree-remove`: check, then remove the worktree on a helper thread; the window follows.
    pub(super) fn start_remove(&mut self, reply: Reply, args: WorktreeRemoveArgs) {
        let session = match reply {
            Reply::Client(c) => self.client_session(c),
            Reply::Cli(_) => self.sessions.iter().position(|s| s.id == self.recent),
        };
        let Some(si) = session else { return self.answer(reply, Err("no session".into())) };
        let meta = |key: &str| {
            self.sessions[si].window().meta.iter().find(|(k, _)| k == key).map(|(_, v)| PathBuf::from(v))
        };
        let (from, mut path, mut repo) = (
            match (&args.from, reply) {
                (Some(f), _) => Some(expand_home(f)),
                (None, Reply::Client(_)) => self.active_cwd(si),
                (None, Reply::Cli(_)) => None,
            },
            None,
            args.repo.as_deref().map(expand_home),
        );
        if args.name.is_none() && matches!(reply, Reply::Client(_)) {
            path = meta("worktree.path");
            if repo.is_none() {
                repo = meta("worktree.repo");
            }
        }
        let Some(from) = from.or_else(|| path.clone()) else {
            return self.answer(reply, Err("no directory to look for a worktree in".into()));
        };
        let repo = repo.map(|r| if r.is_absolute() { r } else { from.join(r) });
        let req = RemoveRequest {
            repo,
            from,
            name: args.name,
            path,
            path_format: self.options.text("worktree-path").to_string(),
            force: args.force,
        };
        let session_id = self.sessions[si].id;
        let spawned = self.spawn_work(reply, session_id, move || {
            let result = std::panic::catch_unwind(|| worktree::remove(&req))
                .unwrap_or_else(|p| Err(format!("worktree-remove failed: {}", log::panic_text(p.as_ref()))));
            Work::Remove(result)
        });
        match spawned {
            Ok(()) => {
                if let Reply::Client(id) = reply {
                    self.show_message(id, "worktree-remove: working...".to_string());
                }
            }
            Err(e) => self.answer(reply, Err(format!("cannot start worktree thread: {e}"))),
        }
    }

    /// The worktree is gone: close the windows opened on it, unless that would close the last
    /// window of the server, and say what happened.
    fn finish_remove(&mut self, result: Result<Removed, String>) -> Result<String, String> {
        let removed = result?;
        let on_it = |meta: &[(String, String)]| {
            meta.iter().any(|(k, v)| k == "worktree.path" && (Path::new(v) == removed.path || Path::new(v) == removed.canonical))
        };
        let (mut panes, mut windows, mut total) = (Vec::new(), 0, 0);
        for w in self.sessions.iter().flat_map(|s| &s.windows) {
            total += 1;
            if on_it(&w.meta) {
                windows += 1;
                panes.extend(w.panes.keys().copied());
            }
        }
        let keep_window = windows > 0 && windows == total;
        if !keep_window {
            for id in panes {
                self.close_pane(id, None);
            }
        }
        self.dirty = true;
        let branch = match &removed.branch {
            Some(b) => format!("branch {b} kept"),
            None => "it was detached, no branch to keep".to_string(),
        };
        let mut text = format!("removed worktree {}{}; {branch}", removed.path.display(), if removed.forced { " (forced)" } else { "" });
        if keep_window {
            text.push_str("\nthe window is left open: it is the last one");
        }
        log::info(&text.replace('\n', "; "));
        let vars = vec![
            ("hook_path", super::hooks::clean(&removed.path.to_string_lossy())),
            ("hook_branch", super::hooks::clean(removed.branch.as_deref().unwrap_or(""))),
        ];
        self.emit("worktree-removed", None, vars);
        Ok(text)
    }

    /// `repo-wizard`: list the repositories under `repos-dir` in a picker.
    pub(super) fn open_wizard(&mut self, id: usize) {
        let dir = expand_home(self.options.text("repos-dir"));
        let repos = match worktree::scan_repos(&dir) {
            Ok(r) if !r.is_empty() => r,
            Ok(_) => return self.show_message(id, format!("no repositories in {}", dir.display())),
            Err(e) => return self.show_message(id, e),
        };
        let Some(c) = self.clients.get_mut(&id) else { return };
        let entries = (0..repos.len()).map(Entry::Repo).collect();
        c.repos = repos;
        c.enter(Mode::Picker(Picker { entries, selected: 0 }));
        self.dirty = true;
    }

    /// A repository was chosen: ask for the worktree name.
    pub(super) fn wizard_pick(&mut self, id: usize, index: usize) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let Some(repo) = c.repos.get(index).cloned() else { return };
        c.wizard_repo = Some(repo);
        c.mode = Mode::Prompt(Prompt { kind: PromptKind::Worktree, text: Vec::new() });
        c.recall = None;
        self.dirty = true;
    }

    /// The name (and maybe a slug) was typed: run `worktree` in the chosen repository.
    pub(super) fn wizard_submit(&mut self, id: usize, text: &str) {
        let Some(repo) = self.clients.get_mut(&id).and_then(|c| c.wizard_repo.take()) else { return };
        let mut words = text.split_whitespace();
        let Some(name) = words.next() else { return };
        let slug = words.next().map(str::to_string);
        if words.next().is_some() {
            return self.show_message(id, "worktree: expected a name and at most a slug".to_string());
        }
        let args = WorktreeArgs {
            name: name.to_string(),
            repo: Some(repo.to_string_lossy().into_owned()),
            slug,
            ..WorktreeArgs::default()
        };
        self.start_worktree(Reply::Client(id), args);
    }

    fn show_pending(&mut self, reply: Reply) {
        if let Reply::Client(id) = reply {
            self.show_message(id, "worktree: working...".to_string());
        }
    }

    /// Open the windows of every finished job.
    pub(super) fn finish_worktrees(&mut self) {
        while let Ok(done) = self.jobs.rx.try_recv() {
            let reply = done.reply;
            let result = match done.work {
                Work::Open { settings, result } => self.open_worktree(done.session, &settings, result),
                Work::Remove(result) => self.finish_remove(result),
            };
            self.answer(reply, result);
        }
    }

    fn open_worktree(&mut self, session: usize, s: &Settings, result: Result<Prepared, String>) -> Result<String, String> {
        let p = result?;
        let shape = worktree::parse_layout(&s.layout)?;
        let mut order = Vec::new();
        shape.clone().map(&mut |k| order.push(k));
        let active_leaf = order.iter().position(|k| *k == PaneKind::Claude).unwrap_or(0);

        let repo = p.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let branch = p.branch.clone().unwrap_or_default();
        let mut run = s.command.trim().to_string();
        let mut notes: Vec<String> = Vec::new();
        let value = |v: &str| match v {
            "repo" => Some(repo.clone()),
            "name" => Some(p.name.clone()),
            "branch" => Some(branch.clone()),
            "base" => Some(p.base.clone()),
            "path" => Some(p.path.to_string_lossy().into_owned()),
            _ => None,
        };
        if !run.is_empty() && !p.reused && !s.start.is_empty() {
            let start = format::expand(&s.start, &value);
            if start.chars().any(char::is_control) {
                log::warn("worktree-start has control characters, not typed");
            } else if let Some(word) = self.shell_kind().quote(&start) {
                run = format!("{run} {word}");
            } else {
                let shell = self.shell();
                let note = format!(
                    "{} is not a shell imux can quote for (set shell-syntax), so worktree-start was not passed",
                    shell.rsplit('/').next().unwrap_or(&shell)
                );
                log::warn(&note);
                notes.push(note);
            }
        }
        if !s.setup.is_empty() && (!p.reused || s.setup_forced) {
            let kind = self.shell_kind();
            match expand_setup(kind, &s.setup, &value).and_then(|setup| compose_run(kind, &setup, &run)) {
                Ok(line) => run = line.unwrap_or_default(),
                Err(e) => {
                    let note = format!("worktree-setup: {e}; nothing was typed into the pane");
                    log::warn(&note);
                    notes.push(note);
                    run.clear();
                }
            }
        }
        let cwd = p.path.to_string_lossy().into_owned();
        let tree = shape.map(&mut |k| PaneSpec {
            cwd: cwd.clone(),
            run: (k == PaneKind::Claude && !run.is_empty()).then(|| run.clone()),
            name: None,
        });
        let meta = [
            ("worktree.repo", p.root.to_string_lossy().into_owned()),
            ("worktree.name", p.name.clone()),
            ("worktree.branch", branch),
            ("worktree.base", p.base.clone()),
            ("worktree.path", cwd),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let spec = WindowSpec { name: format!("{repo}:{}", p.name), auto_rename: false, tree, active_leaf, meta };

        let si = self
            .sessions
            .iter()
            .position(|s| s.id == session)
            .or_else(|| self.sessions.iter().position(|s| s.id == self.recent))
            .ok_or("no session to open the window in")?;
        let window = self.build_window(spec).map_err(|e| format!("worktree {}: {e}", p.name))?;
        self.sessions[si].add(window);
        self.relayout();
        self.dirty = true;

        let mut text = if p.reused {
            format!("worktree {} reused at {}", p.name, p.path.display())
        } else if p.base.is_empty() {
            format!("worktree {} checked out at {}", p.name, p.path.display())
        } else {
            format!("worktree {} from {} at {}", p.name, p.base, p.path.display())
        };
        for w in p.warnings.iter().chain(&notes) {
            text.push_str(&format!("\nwarning: {w}"));
        }
        log::info(&text.replace('\n', "; "));
        let vars = vec![
            ("hook_path", super::hooks::clean(&p.path.to_string_lossy())),
            ("hook_message", super::hooks::clean(&p.name)),
            ("hook_branch", super::hooks::clean(p.branch.as_deref().unwrap_or(""))),
        ];
        self.emit("worktree-created", None, vars);
        Ok(text)
    }
}

/// `worktree-setup` with each `#{repo|name|branch|base|path}` replaced by its value quoted for
/// the pane shell, and `##` as `#`. The values come from git refs and directory names, which can
/// hold `$`, `;` and backticks, so they never reach the shell raw and never pass through format
/// modifiers; any other `#{...}` is an error rather than a guess.
fn expand_setup(kind: ShellKind, text: &str, value: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' && chars.get(i + 1) == Some(&'#') {
            out.push('#');
            i += 2;
        } else if chars[i] == '#' && chars.get(i + 1) == Some(&'{') {
            let end = chars[i + 2..].iter().position(|&c| c == '}').ok_or("an unclosed #{ in the text")?;
            let name: String = chars[i + 2..i + 2 + end].iter().collect();
            let raw = value(&name).ok_or_else(|| format!("#{{{name}}} is not a variable of the setup text"))?;
            out.push_str(&kind.quote(&raw).ok_or("the shell is not one imux can quote for (see shell-syntax)")?);
            i += end + 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// What to type into the command pane: `setup`, and `command` after it only if the setup
/// worked, in the way the shell of the pane writes that. Either may be empty.
fn compose_run(kind: ShellKind, setup: &str, command: &str) -> Result<Option<String>, String> {
    let (setup, command) = (setup.trim(), command.trim());
    if setup.chars().any(char::is_control) {
        return Err("the text has control characters".to_string());
    }
    match (setup.is_empty(), command.is_empty()) {
        (true, true) => Ok(None),
        (true, false) => Ok(Some(command.to_string())),
        (false, true) => Ok(Some(setup.to_string())),
        (false, false) => kind
            .chain(setup, command)
            .map(Some)
            .ok_or_else(|| "the shell is not one imux can chain commands for (see shell-syntax)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(v: &str) -> Option<String> {
        match v {
            "name" => Some("ABC-1".to_string()),
            "branch" => Some("feature/x$(id);`id`".to_string()),
            "repo" | "base" | "path" => Some("it's".to_string()),
            _ => None,
        }
    }

    #[test]
    fn setup_and_command_are_chained_for_the_shell_of_the_pane() {
        let run = |k, s, c| compose_run(k, s, c).unwrap();
        assert_eq!(run(ShellKind::Posix, "npm ci", "claude 'x'").as_deref(), Some("npm ci && claude 'x'"));
        assert_eq!(run(ShellKind::Fish, "npm ci", "claude").as_deref(), Some("npm ci; and claude"));
        assert_eq!(run(ShellKind::Pwsh, "npm ci", "claude").as_deref(), Some("npm ci; if ($?) { claude }"));
        assert_eq!(run(ShellKind::Posix, "", "claude").as_deref(), Some("claude"));
        assert_eq!(run(ShellKind::Posix, "npm ci", "").as_deref(), Some("npm ci"));
        assert_eq!(run(ShellKind::Posix, "  ", " "), None);
        assert_eq!(run(ShellKind::Posix, " npm ci ", " claude ").as_deref(), Some("npm ci && claude"));
    }

    #[test]
    fn an_unknown_shell_is_not_chained_but_a_lone_command_or_setup_still_goes() {
        assert!(compose_run(ShellKind::Unknown, "npm ci", "claude").is_err());
        assert_eq!(compose_run(ShellKind::Unknown, "", "claude").unwrap().as_deref(), Some("claude"));
        assert_eq!(compose_run(ShellKind::Unknown, "npm ci", "").unwrap().as_deref(), Some("npm ci"));
    }

    #[test]
    fn control_characters_in_the_setup_refuse_it() {
        assert!(compose_run(ShellKind::Posix, "a\nb", "claude").is_err());
        assert!(compose_run(ShellKind::Posix, "a\u{1b}[200~b", "").is_err());
    }

    #[test]
    fn setup_variables_are_quoted_values_never_raw() {
        assert_eq!(expand_setup(ShellKind::Posix, "echo #{name}", &vars).unwrap(), "echo ABC-1");
        assert_eq!(expand_setup(ShellKind::Posix, "echo #{branch}", &vars).unwrap(), "echo 'feature/x$(id);`id`'");
        assert_eq!(expand_setup(ShellKind::Posix, "cd #{path}", &vars).unwrap(), "cd 'it'\\''s'");
        assert_eq!(expand_setup(ShellKind::Fish, "cd #{path}", &vars).unwrap(), "cd 'it\\'s'");
        assert_eq!(expand_setup(ShellKind::Pwsh, "cd #{path}", &vars).unwrap(), "cd 'it''s'");
    }

    #[test]
    fn setup_text_is_otherwise_left_alone() {
        assert_eq!(expand_setup(ShellKind::Posix, "npm ci # install", &vars).unwrap(), "npm ci # install");
        assert_eq!(expand_setup(ShellKind::Posix, "echo ##{name}", &vars).unwrap(), "echo #{name}");
        assert_eq!(expand_setup(ShellKind::Posix, "date +%Y", &vars).unwrap(), "date +%Y");
        assert_eq!(expand_setup(ShellKind::Posix, "echo $HOME", &vars).unwrap(), "echo $HOME");
    }

    #[test]
    fn unknown_variables_modifiers_and_unknown_shells_are_errors() {
        assert!(expand_setup(ShellKind::Posix, "echo #{nope}", &vars).is_err());
        assert!(expand_setup(ShellKind::Posix, "echo #{s/a/b/:name}", &vars).is_err());
        assert!(expand_setup(ShellKind::Posix, "echo #{name", &vars).is_err());
        assert!(expand_setup(ShellKind::Unknown, "echo #{name}", &vars).is_err());
        assert_eq!(expand_setup(ShellKind::Unknown, "echo hi", &vars).unwrap(), "echo hi");
    }
}
