//! Building windows from a description, and bringing a saved snapshot back with them.
//! Nothing here runs a program the user did not have in the pane, and only those on the
//! `restore-commands` list (plus Claude Code, resumed) are typed again.

use std::io;
use std::path::{Path, PathBuf};

use super::{Server, cell_px};
use crate::log;
use crate::persist::{SPane, SWindow, Snapshot};
use crate::session::layout::{Layout, PaneId, Shape};
use crate::session::{Pane, Session, TypeAhead, Window};
use crate::shell::ShellKind;

/// A pane to start: where, and a command line to type into the shell once it is up.
pub struct PaneSpec {
    pub cwd: String,
    pub run: Option<String>,
    /// The agent name the pane had, given to the first record it gets.
    pub name: Option<String>,
}

/// A window to build: the single description used by restore (and later by `worktree`).
pub struct WindowSpec {
    pub name: String,
    pub auto_rename: bool,
    pub tree: Shape<PaneSpec>,
    pub active_leaf: usize,
    pub meta: Vec<(String, String)>,
}

/// What may be started again, from `restore-commands` and `restore-claude`.
pub struct Rules {
    pub commands: Vec<String>,
    pub claude: bool,
    /// Syntax of the pane shell, for quoting what is typed.
    pub shell: ShellKind,
}

/// `argv` as a command line in the syntax of `shell`, each word quoted; `None` when the shell is
/// not one imux knows how to quote for.
fn command_line(shell: ShellKind, argv: &[String]) -> Option<String> {
    let words: Option<Vec<String>> = argv.iter().map(|a| shell.quote(a)).collect();
    words.map(|w| w.join(" "))
}

/// Command to type for a saved pane, or `None` for a plain shell or a program that is not allowed.
pub fn run_command(p: &SPane, rules: &Rules) -> Option<String> {
    if let Some(resume) = &p.resume {
        // The file is untrusted input: the command is checked again, as when it was reported.
        match crate::session::agent::validate_resume(resume, &rules.commands) {
            Ok(()) if resume.first().is_some_and(|c| c == "claude") && !rules.claude => return None,
            Ok(()) => match command_line(rules.shell, resume) {
                Some(line) => return Some(line),
                None => log::warn("restore: the shell is not one imux can quote for (see shell-syntax), resume command not typed"),
            },
            Err(e) => log::warn(&format!("restore: reported resume command refused: {e}")),
        }
    }
    if p.claude {
        return rules.claude.then(|| "claude --continue".to_string());
    }
    p.argv.first()?;
    // The programs the pane's program runs inside come first, each typed once the one before it
    // (a nested shell) is waiting for input; one that is not allowed stops it all.
    let mut lines = Vec::new();
    for level in p.outer.iter().chain(std::iter::once(&p.argv)) {
        lines.push(level_line(level, rules)?);
    }
    Some(lines.join("\r"))
}

/// A shell started inside the pane: a known shell's name, alone. Needs no entry in
/// `restore-commands`, and is typed as the bare name so the file cannot choose which file runs.
fn plain_shell(argv: &[String]) -> Option<&str> {
    let [one] = argv else { return None };
    let name = one.rsplit('/').next().unwrap_or(one).trim_start_matches('-');
    (ShellKind::of(name) != ShellKind::Unknown).then_some(name)
}

/// The line that starts one level of a saved pane, or `None` when it must not be typed.
fn level_line(argv: &[String], rules: &Rules) -> Option<String> {
    let first = argv.first()?;
    let name = first.rsplit('/').next().unwrap_or(first);
    if argv.iter().any(|a| a.chars().any(char::is_control)) {
        log::warn(&format!("restore: {name} has control characters in its arguments, not started"));
        return None;
    }
    let line = if let Some(shell) = plain_shell(argv).filter(|_| !rules.commands.iter().any(|c| c == name)) {
        command_line(rules.shell, &[shell.to_string()])
    } else if rules.commands.iter().any(|c| c == name) {
        command_line(rules.shell, argv)
    } else {
        log::info(&format!("restore: {name} is not in restore-commands, starting a shell instead"));
        return None;
    };
    if line.is_none() {
        log::warn(&format!("restore: {name} not typed, the shell is not one imux can quote for (see shell-syntax)"));
    }
    line
}

/// The saved directory if it still exists, else `$HOME`, else none (the server's own).
fn live_dir(saved: &str) -> Option<PathBuf> {
    if !saved.is_empty() && Path::new(saved).is_dir() {
        return Some(PathBuf::from(saved));
    }
    if !saved.is_empty() {
        log::info(&format!("restore: {saved} is gone, using the home directory"));
    }
    std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir())
}

/// The saved directory when it is gone and the home directory takes its place (as
/// `live_dir` decides), with the home directory as the system names it; none when nothing is
/// missing or there is no home directory to stand in.
fn missing_dir(saved: &str) -> Option<(String, String)> {
    if saved.is_empty() || Path::new(saved).is_dir() {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir())?;
    let real = std::fs::canonicalize(&home).unwrap_or(home);
    Some((saved.to_string(), real.to_string_lossy().into_owned()))
}

pub fn spec_of(w: &SWindow, rules: &Rules) -> WindowSpec {
    let tree = w.tree.clone().map(&mut |p: SPane| PaneSpec { run: run_command(&p, rules), name: p.name.clone(), cwd: p.cwd });
    // The links of spawned panes are read by `relink_spawned`, and are no window values.
    let meta = w.meta.iter().filter(|(k, _)| k != super::capture::SPAWN_KEY).cloned().collect();
    WindowSpec { name: w.name.clone(), auto_rename: w.auto_rename, tree, active_leaf: w.active_leaf, meta }
}

impl Server {
    pub(super) fn restore_rules(&self) -> Rules {
        Rules {
            commands: self.options.text("restore-commands").split_whitespace().map(str::to_string).collect(),
            claude: self.options.flag("restore-claude"),
            shell: self.shell_kind(),
        }
    }

    /// Start every pane of `spec` in a new window. A pane that cannot start is left out and its
    /// space goes to its neighbor; an error means none could. The caller adds the window to a
    /// session and calls `relayout`.
    pub(super) fn build_window(&mut self, spec: WindowSpec) -> io::Result<Window> {
        let Some(size) = self.size else { return Err(io::Error::other("no client size yet")) };
        let (area, cell) = (self.pane_area(size, self.first_status_rows()), cell_px(size));
        let mut leaves: Vec<(PaneId, PaneSpec)> = Vec::new();
        let next = &mut self.next_pane;
        let mut layout = Layout::from_shape(spec.tree, |p| {
            let id = *next;
            *next += 1;
            leaves.push((id, p));
            id
        });
        let active = leaves.get(spec.active_leaf).map(|(id, _)| *id);
        let rects = layout.rects(area);
        let mut started: Vec<(PaneId, Pane)> = Vec::new();
        for (id, p) in leaves {
            let Some((_, rect)) = rects.iter().find(|(r, _)| *r == id) else { continue };
            let spawned = self.spawn_pane(id, self.content_rect(*rect), cell, live_dir(&p.cwd).as_deref());
            match spawned {
                Ok(mut pane) => {
                    if let Some(gone) = missing_dir(&p.cwd) {
                        log::info(&format!("pane {id}: {} is gone, the pane starts in the home directory and keeps the path", gone.0));
                        self.missing_notice.push(gone.0.clone());
                        self.kept_dirs.insert(id, gone);
                    }
                    if let Some(name) = p.name.clone().filter(|n| crate::session::agent::valid_name(n)) {
                        self.pending_names.insert(id, name);
                    }
                    if let Some(cmd) = &p.run {
                        let text = format!("{cmd}\r").into_bytes();
                        if self.shell_kind() == ShellKind::Pwsh {
                            pane.typeahead = Some(TypeAhead::new(text));
                        } else if let Err(e) = pane.pty.write(&text) {
                            log::warn(&format!("pane {id}: typing the command: {e}"));
                        }
                    }
                    started.push((id, pane));
                }
                Err(e) => {
                    log::error(&format!("pane {id}: cannot start: {e}"));
                    layout.remove(id);
                }
            }
        }
        let mut started = started.into_iter();
        let Some((first_id, first)) = started.next() else {
            return Err(io::Error::other("no pane could be started"));
        };
        let mut win = Window::new(spec.name, first_id, first);
        win.layout = layout;
        for (id, pane) in started {
            win.panes.insert(id, pane);
        }
        win.auto_rename = spec.auto_rename;
        win.meta = spec.meta;
        win.active = active.filter(|id| win.panes.contains_key(id)).unwrap_or(first_id);
        Ok(win)
    }

    /// Type the commands that waited for their shell to finish starting.
    pub(super) fn release_typeahead(&mut self) {
        let now = std::time::Instant::now();
        for (id, pane) in self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).flat_map(|w| w.panes.iter_mut()) {
            if pane.typeahead.as_ref().is_some_and(|t| t.due() <= now)
                && let Some(t) = pane.typeahead.take()
                && let Err(e) = pane.pty.write(&t.text)
            {
                log::warn(&format!("pane {id}: typing the command: {e}"));
            }
        }
    }

    /// When the soonest waiting command may be typed, for the event loop's poll.
    pub(super) fn typeahead_deadline(&self) -> Option<std::time::Instant> {
        self.sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.values())
            .filter_map(|p| p.typeahead.as_ref().map(TypeAhead::due))
            .min()
    }

    /// Why a session has been used, or `None` for one nobody has: one window with one pane that is
    /// a plain shell in front, nothing typed into it by a person or a script, no agent, no copy
    /// mode and nothing waiting to be typed. This is what a new server starts with, and a restore
    /// may replace it. What the shell printed on its own (a greeting, a long prompt) does not make
    /// it used, and a command run in it does.
    fn why_session_is_used(&self, session: &Session) -> Option<&'static str> {
        let [window] = &session.windows[..] else { return Some("it has more than one window") };
        let mut panes = window.panes.values();
        let (Some(pane), None) = (panes.next(), panes.next()) else { return Some("its window has more than one pane") };
        if pane.dead.is_some() {
            Some("its pane has ended")
        } else if pane.agent.is_some() {
            Some("an agent reported in its pane")
        } else if pane.start.is_some() {
            Some("its pane runs a command, not a plain shell")
        } else if pane.copy.is_some() || pane.typeahead.is_some() {
            Some("its pane is in use")
        } else if pane.pty.was_typed_into() {
            Some("something was typed into its pane")
        } else if !super::capture::shell_is_foreground(pane, &self.shell()) {
            Some("a program runs in its pane")
        } else {
            None
        }
    }

    /// Recreate the sessions of `snap`; returns how many came back and how many were left out
    /// because a session of that name already runs. Failures are per window.
    pub(super) fn restore(&mut self, snap: &Snapshot) -> (usize, usize) {
        let rules = self.restore_rules();
        let (mut restored, mut skipped) = (0, 0);
        let mut recent = None;
        // The panes of every window that came back, in layout order, by their place in the snapshot.
        let mut placed: std::collections::HashMap<(usize, usize), Vec<PaneId>> = std::collections::HashMap::new();
        for (i, saved) in snap.sessions.iter().enumerate() {
            // A session of that name that nobody has used (the empty one a new server starts with)
            // gives way to the saved one; one that has been used is left alone.
            let mut replaces = None;
            if !saved.name.is_empty()
                && let Some(old) = self.sessions.iter().find(|s| s.name == saved.name)
            {
                match self.why_session_is_used(old) {
                    None => replaces = Some(old.id),
                    Some(why) => {
                        log::info(&format!("restore: session {:?} already exists, left alone: {why}", saved.name));
                        skipped += 1;
                        continue;
                    }
                }
            }
            let mut windows = Vec::new();
            let mut current = None;
            for (wi, w) in saved.windows.iter().enumerate() {
                match self.build_window(spec_of(w, &rules)) {
                    Ok(win) => {
                        if wi == saved.current {
                            current = Some(windows.len());
                        }
                        placed.insert((i, wi), win.layout.panes());
                        windows.push(win);
                    }
                    Err(e) => log::error(&format!("restore: window {:?} of {:?}: {e}", w.name, saved.name)),
                }
            }
            let mut windows = windows.into_iter();
            let Some(first) = windows.next() else { continue };
            let sid = self.next_session;
            self.next_session += 1;
            let name = if saved.name.is_empty() { self.free_session_name() } else { saved.name.clone() };
            let mut session = Session::new(sid, name, first);
            session.windows.extend(windows);
            session.current = current.unwrap_or(0);
            log::info(&format!("session {sid} ({}) restored with {} windows", session.name, session.windows.len()));
            self.sessions.push(session);
            if let Some(old) = replaces {
                // the clients looking at the empty session follow to the saved one, then it goes
                let looking: Vec<usize> = self.clients.iter().filter(|(_, c)| c.session == old).map(|(id, _)| *id).collect();
                for client in looking {
                    self.attach(client, sid);
                }
                if let Some(si) = self.sessions.iter().position(|s| s.id == old) {
                    self.kill_session(si);
                }
                log::info(&format!("restore: the untouched session {:?} gave way to the saved one", self.sessions.iter().find(|s| s.id == sid).map_or("", |s| s.name.as_str())));
            }
            if i == snap.recent {
                recent = Some(sid);
            }
            restored += 1;
        }
        if let Some(first) = self.sessions.first() {
            self.recent = recent.unwrap_or(first.id);
        }
        self.relink_spawned(snap, &placed);
        self.relayout();
        (restored, skipped)
    }

    /// Give the restored panes the makers they had: a program that spawned panes can act on them
    /// again after a restore, which numbers the panes anew. The links come from the snapshot (written
    /// by `capture`), by position; one that points at a pane that did not come back, at itself, or
    /// round a circle is dropped. The pane numbers are not trusted for anything: only places.
    fn relink_spawned(&mut self, snap: &Snapshot, placed: &std::collections::HashMap<(usize, usize), Vec<PaneId>>) {
        let mut linked = 0;
        for (&(si, wi), panes) in placed {
            let Some(value) = snap.sessions.get(si).and_then(|s| s.windows.get(wi)).and_then(|w| w.meta.iter().find(|(k, _)| k == super::capture::SPAWN_KEY)) else { continue };
            for entry in value.1.split(',').take(super::spawned::CHAIN_MAX * 4) {
                let Some((leaf, maker)) = entry.split_once(':') else { continue };
                let mut at = maker.split('.');
                let (Ok(leaf), Some(Ok(ms)), Some(Ok(mw)), Some(Ok(ml))) = (
                    leaf.parse::<usize>(),
                    at.next().map(str::parse::<usize>),
                    at.next().map(str::parse::<usize>),
                    at.next().map(str::parse::<usize>),
                ) else {
                    continue;
                };
                let (Some(&child), Some(&maker)) = (panes.get(leaf), placed.get(&(ms, mw)).and_then(|p| p.get(ml))) else { continue };
                // never a pane its own maker, never a circle
                if child == maker || self.maker_chain(maker).contains(&child) {
                    continue;
                }
                self.spawned_by.insert(child, maker);
                linked += 1;
            }
        }
        if linked > 0 {
            log::info(&format!("restore: {linked} pane{} got back the pane that made {}", if linked == 1 { "" } else { "s" }, if linked == 1 { "it" } else { "them" }));
        }
    }

    /// On the first client: bring back the last autosave when `restore-on-start` is on.
    pub(super) fn restore_on_start(&mut self) -> bool {
        if !self.options.flag("restore-on-start") {
            return false;
        }
        let Some(store) = &self.store else { return false };
        match store.load_autosave() {
            Ok(Some(snap)) => self.restore(&snap).0 > 0,
            Ok(None) => false,
            Err(e) => {
                log::error(&format!("restore: {e}"));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::layout::Split;

    fn rules() -> Rules {
        Rules { commands: ["vim", "ssh", "claude"].map(String::from).to_vec(), claude: true, shell: ShellKind::Posix }
    }

    fn pane(argv: &[&str], claude: bool) -> SPane {
        SPane { cwd: "/".into(), argv: argv.iter().map(|s| s.to_string()).collect(), claude, resume: None, outer: Vec::new(), name: None }
    }

    #[test]
    fn only_listed_programs_are_typed_again() {
        assert_eq!(run_command(&pane(&["vim", "a.rs"], false), &rules()).as_deref(), Some("vim a.rs"));
        assert_eq!(run_command(&pane(&["/usr/bin/ssh", "host"], false), &rules()).as_deref(), Some("/usr/bin/ssh host"));
        assert_eq!(run_command(&pane(&["rm", "-rf", "x"], false), &rules()), None);
        assert_eq!(run_command(&pane(&[], false), &rules()), None);
    }

    #[test]
    fn claude_resumes_when_allowed_and_otherwise_starts_a_shell() {
        assert_eq!(run_command(&pane(&[], true), &rules()).as_deref(), Some("claude --continue"));
        let off = Rules { claude: false, ..rules() };
        assert_eq!(run_command(&pane(&[], true), &off), None);
    }

    #[test]
    fn arguments_are_quoted_as_single_words_and_control_characters_refuse_the_command() {
        let cmd = run_command(&pane(&["vim", "my file", "it's", "$HOME;ls", ""], false), &rules());
        assert_eq!(cmd.as_deref(), Some("vim 'my file' 'it'\\''s' '$HOME;ls' ''"));
        assert_eq!(run_command(&pane(&["vim", "a\nrm x"], false), &rules()), None);
        assert_eq!(run_command(&pane(&["vim", "\u{1b}[200~"], false), &rules()), None);
    }

    #[test]
    fn a_saved_window_becomes_a_spec_with_commands_decided() {
        let tree = Shape::Branch {
            kind: Split::SideBySide,
            ratio: 0.5,
            a: Box::new(Shape::Leaf(pane(&["vim", "x"], false))),
            b: Box::new(Shape::Leaf(pane(&["top"], false))),
        };
        let w = SWindow {
            name: "w".into(),
            auto_rename: false,
            active_leaf: 1,
            tree,
            meta: vec![("k".into(), "v".into())],
        };
        let spec = spec_of(&w, &rules());
        assert_eq!((spec.name.as_str(), spec.auto_rename, spec.active_leaf), ("w", false, 1));
        assert_eq!(spec.meta, vec![("k".to_string(), "v".to_string())]);
        let Shape::Branch { a, b, .. } = spec.tree else { panic!("a split") };
        let (Shape::Leaf(a), Shape::Leaf(b)) = (*a, *b) else { panic!("two leaves") };
        assert_eq!(a.run.as_deref(), Some("vim x"));
        assert_eq!(b.run, None);
    }

    #[test]
    fn only_a_gone_directory_with_a_home_to_stand_in_is_remembered() {
        let tmp = std::env::temp_dir();
        assert_eq!(missing_dir(tmp.to_str().unwrap()), None, "it exists");
        assert_eq!(missing_dir(""), None, "nothing was saved");
        match std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir()) {
            Some(home) => {
                let (saved, stands_in) = missing_dir("/definitely/not/here").unwrap();
                assert_eq!(saved, "/definitely/not/here");
                assert_eq!(PathBuf::from(stands_in), std::fs::canonicalize(home).unwrap());
            }
            None => assert_eq!(missing_dir("/definitely/not/here"), None),
        }
    }

    #[test]
    fn a_vanished_directory_falls_back_to_home_and_an_existing_one_is_kept() {
        let tmp = std::env::temp_dir();
        assert_eq!(live_dir(tmp.to_str().unwrap()), Some(tmp));
        let gone = live_dir("/definitely/not/here");
        assert_eq!(gone, std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir()));
    }

    fn with_resume(argv: &[&str], claude: bool, resume: &[&str]) -> SPane {
        SPane { resume: Some(resume.iter().map(|s| s.to_string()).collect()), ..pane(argv, claude) }
    }

    fn listed(claude: bool) -> Rules {
        Rules { commands: vec!["claude".into(), "vim".into()], claude, shell: ShellKind::Posix }
    }

    #[test]
    fn a_reported_resume_command_is_typed_instead_of_the_guess() {
        let p = with_resume(&[], true, &["claude", "--resume", "abc 123"]);
        assert_eq!(run_command(&p, &listed(true)).as_deref(), Some("claude --resume 'abc 123'"));
        let p = with_resume(&["vim", "x"], false, &["vim", "-S", "s.vim"]);
        assert_eq!(run_command(&p, &listed(true)).as_deref(), Some("vim -S s.vim"));
    }

    #[test]
    fn a_resume_command_from_a_file_is_checked_again_and_falls_back_to_the_guess() {
        let evil = with_resume(&[], true, &["rm", "-rf", "x"]);
        assert_eq!(run_command(&evil, &listed(true)).as_deref(), Some("claude --continue"));
        let quote_break = with_resume(&[], false, &["vim", "it's"]);
        assert_eq!(run_command(&quote_break, &listed(true)), None);
        let path = with_resume(&[], false, &["/bin/sh", "-c", "x"]);
        assert_eq!(run_command(&path, &listed(true)), None);
    }

    fn nested(outer: &[&[&str]], argv: &[&str]) -> SPane {
        SPane { outer: outer.iter().map(|l| l.iter().map(|s| s.to_string()).collect()).collect(), ..pane(argv, false) }
    }

    #[test]
    fn a_program_inside_a_nested_shell_is_typed_after_the_shell() {
        let p = nested(&[&["bash"]], &["vim", "a.rs"]);
        assert_eq!(run_command(&p, &rules()).as_deref(), Some("bash\rvim a.rs"));
        let deeper = nested(&[&["/bin/bash"], &["-zsh"]], &["vim"]);
        assert_eq!(run_command(&deeper, &rules()).as_deref(), Some("bash\rzsh\rvim"));
    }

    #[test]
    fn an_idle_nested_shell_comes_back_as_that_shell() {
        assert_eq!(run_command(&pane(&["bash"], false), &rules()).as_deref(), Some("bash"));
        assert_eq!(run_command(&pane(&["/usr/bin/fish"], false), &rules()).as_deref(), Some("fish"));
    }

    #[test]
    fn a_nested_shell_is_never_started_from_a_path_or_with_arguments() {
        assert_eq!(run_command(&pane(&["/tmp/evil/bash"], false), &rules()).as_deref(), Some("bash"), "only the bare name is typed");
        assert_eq!(run_command(&pane(&["bash", "-c", "rm -rf x"], false), &rules()), None);
        assert_eq!(run_command(&nested(&[&["sh", "-c", "x"]], &["vim"]), &rules()), None);
    }

    #[test]
    fn one_program_that_is_not_allowed_stops_the_whole_chain() {
        assert_eq!(run_command(&nested(&[&["nix", "develop"]], &["vim"]), &rules()), None);
        assert_eq!(run_command(&nested(&[&["bash"]], &["htop"]), &rules()), None);
        let nix = Rules { commands: vec!["nix".into(), "vim".into()], ..rules() };
        assert_eq!(run_command(&nested(&[&["nix", "develop"], &["bash"]], &["vim", "x"]), &nix).as_deref(), Some("nix develop\rbash\rvim x"));
    }

    #[test]
    fn a_control_character_in_any_level_refuses_the_chain() {
        let nix = Rules { commands: vec!["nix".into(), "vim".into()], ..rules() };
        assert_eq!(run_command(&nested(&[&["nix", "a\rb"]], &["vim"]), &nix), None);
    }

    #[test]
    fn restore_claude_off_also_stops_a_reported_claude_resume() {
        let p = with_resume(&[], true, &["claude", "--resume", "abc"]);
        assert_eq!(run_command(&p, &listed(false)), None);
    }
}
