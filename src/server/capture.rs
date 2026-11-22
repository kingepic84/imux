//! Reading the live sessions into a `Snapshot`: for each pane its directory and what runs in it.

use std::os::fd::AsFd;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::persist::{SPane, SSession, SWindow, Snapshot};
use crate::session::{Pane, Session, Window};
use crate::shell::ShellKind;
use crate::tty::proc;

/// What the snapshot needs to know besides the sessions.
pub struct Capture<'a> {
    /// The `claude-programs` option: program names that are Claude Code, by the executable's file
    /// name or `argv[0]` (a wrapper around it is named here, never in the source).
    pub claude: &'a [String],
    /// The `restore-commands` option: only these programs keep their arguments in the snapshot,
    /// for any other the file holds the program alone, because arguments carry secrets
    /// (`mysql -pSECRET`, `--token`) and a program that is not restored does not need them.
    pub keep: &'a [String],
    /// The pane shell (`default-shell`), whose plain appearance in the foreground means no command.
    pub shell: &'a str,
    /// Panes restored into the home directory because their saved one was gone: the saved
    /// directory and the home directory the pane started in. While such a pane is still in that
    /// home directory, the snapshot keeps the saved path.
    pub kept: &'a std::collections::HashMap<crate::session::layout::PaneId, (String, String)>,
    /// The agent names of restored panes whose program has not reported yet.
    pub names: &'a std::collections::HashMap<crate::session::layout::PaneId, String>,
}

/// The pane's shell and the foreground process, as far as the system says (PowerShell runs its
/// programs in its own process group, so its single child stands for the foreground one).
fn processes(p: &Pane, shell: &str) -> (i32, i32) {
    let child = p.pty.child.as_raw();
    let mut fg = proc::foreground_pid(p.pty.master.as_fd()).unwrap_or(child);
    if fg == child
        && ShellKind::of(shell) == ShellKind::Pwsh
        && let [only] = proc::child_pids(child)[..]
    {
        fg = only;
    }
    (child, fg)
}

/// Whether the pane's own shell is what runs in the foreground, so a line typed now reaches its
/// prompt and not a program (`agent start` checks this before it types).
pub fn shell_is_foreground(p: &Pane, shell: &str) -> bool {
    let (child, fg) = processes(p, shell);
    child == fg
}

fn dir_of(child: i32, fg: i32) -> String {
    proc::cwd(fg).or_else(|| proc::cwd(child)).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The directory the pane is in now, empty when the system does not say.
pub fn current_dir(p: &Pane, shell: &str) -> String {
    let (child, fg) = processes(p, shell);
    dir_of(child, fg)
}

pub fn snapshot(sessions: &[Session], recent_id: usize, cap: &Capture) -> Snapshot {
    let saved_at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let recent = sessions.iter().position(|s| s.id == recent_id).unwrap_or(0);
    let sessions = sessions.iter().map(|s| session(s, cap)).collect();
    Snapshot { saved_at, recent, sessions }
}

fn session(s: &Session, cap: &Capture) -> SSession {
    SSession { name: s.name.clone(), current: s.current, windows: s.windows.iter().map(|w| window(w, cap)).collect() }
}

fn window(w: &Window, cap: &Capture) -> SWindow {
    let order = w.layout.panes();
    SWindow {
        name: w.name.clone(),
        auto_rename: w.auto_rename,
        active_leaf: order.iter().position(|&p| p == w.active).unwrap_or(0),
        tree: w.layout.shape(|id| w.panes.get(&id).map_or_else(blank, |p| pane(p, cap, id))),
        meta: w.meta.clone(),
    }
}

/// `argv` as it may be written to disk: whole for a program in `keep`, else just the program.
fn redact(argv: Vec<String>, keep: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else { return argv };
    if keep.iter().any(|k| k == base(first)) {
        return argv;
    }
    argv.into_iter().take(1).collect()
}

fn blank() -> SPane {
    SPane { cwd: String::new(), argv: Vec::new(), claude: false, resume: None, outer: Vec::new(), name: None }
}

/// Programs between the pane's shell (`child`) and the foreground one (`fg`), outermost first, as
/// far as `OUTER_DEPTH` allows; empty when `fg` is not a descendant of the shell (a program that
/// left its parent behind is not inside anything).
fn enclosing(fg: i32, child: i32) -> Vec<i32> {
    let mut between = Vec::new();
    let mut at = fg;
    for _ in 0..=OUTER_DEPTH {
        match proc::parent_pid(at) {
            Some(parent) if parent == child => {
                between.reverse();
                return between;
            }
            Some(parent) if parent > 1 && parent != at => {
                between.push(parent);
                at = parent;
            }
            _ => break,
        }
    }
    Vec::new()
}

/// How many programs deep a pane's program may be and still be written down.
const OUTER_DEPTH: usize = 4;

fn pane(p: &Pane, cap: &Capture, id: crate::session::layout::PaneId) -> SPane {
    let (child, fg) = processes(p, cap.shell);
    let mut cwd = dir_of(child, fg);
    if let Some((saved, home)) = cap.kept.get(&id)
        && cwd == *home
    {
        cwd.clone_from(saved);
    }
    // Only the pane's own shell is "no command"; the same shell started inside it is a program.
    let own_shell = if fg == child { cap.shell } else { "" };
    let (argv, mut claude) =
        classify(proc::command_name(fg).as_deref(), &proc::argv(fg).unwrap_or_default(), own_shell, cap.claude);
    let mut argv = redact(argv, cap.keep);
    let mut outer = Vec::new();
    if fg != child {
        for pid in enclosing(fg, child) {
            let level = proc::argv(pid).unwrap_or_default();
            let (_, is_claude) = classify(proc::command_name(pid).as_deref(), &level, "", cap.claude);
            if is_claude {
                // Whatever Claude Code runs belongs to it, and restore resumes the conversation.
                (claude, argv) = (true, Vec::new());
                outer.clear();
                break;
            }
            if level.is_empty() {
                // A level that cannot be read cannot be entered again: leave a plain shell.
                (argv, outer) = (Vec::new(), Vec::new());
                break;
            }
            outer.push(redact(level, cap.keep));
        }
    }
    let resume = p.agent.as_ref().and_then(|a| a.resume.clone());
    let name = p.agent.as_ref().and_then(|a| a.name.clone()).or_else(|| cap.names.get(&id).cloned());
    SPane { cwd, argv, claude, resume, outer, name }
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What runs in the foreground: `(argv, claude)`. A plain shell (the login shell, also with the
/// `-zsh` form of `argv[0]`) is no command, and Claude Code drops its arguments because restore
/// resumes the conversation instead of replaying them.
fn classify(exe: Option<&str>, argv: &[String], shell: &str, claude: &[String]) -> (Vec<String>, bool) {
    let first = argv.first().map(|a| base(a).trim_start_matches('-'));
    let is = |names: &[&str]| exe.is_some_and(|n| names.contains(&n)) || first.is_some_and(|n| names.contains(&n));
    let claude: Vec<&str> = claude.iter().map(String::as_str).collect();
    if is(&claude) {
        return (Vec::new(), true);
    }
    if !shell.is_empty() && is(&[base(shell)]) {
        return (Vec::new(), false);
    }
    (argv.to_vec(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_login_shell_is_no_command_in_either_spelling() {
        assert_eq!(classify(Some("zsh"), &v(&["-zsh"]), "/bin/zsh", &v(&["claude"])), (v(&[]), false));
        assert_eq!(classify(None, &v(&["/bin/zsh", "-l"]), "/bin/zsh", &v(&["claude"])), (v(&[]), false));
    }

    #[test]
    fn claude_is_recognized_by_executable_or_argv0_and_keeps_no_arguments() {
        let names = v(&["claude"]);
        assert_eq!(classify(Some("claude"), &v(&["claude", "--model", "x"]), "/bin/zsh", &names), (v(&[]), true));
        assert_eq!(classify(Some("2.1.0"), &v(&["claude"]), "/bin/zsh", &names), (v(&[]), true));
    }

    #[test]
    fn a_wrapper_is_claude_only_when_the_option_names_it() {
        let wrapper = v(&["claude-wrapper"]);
        let argv = v(&["/x/claude-wrapper"]);
        assert_eq!(classify(Some("claude-wrapper"), &argv, "/bin/zsh", &v(&["claude"])), (argv.clone(), false));
        assert_eq!(classify(Some("claude-wrapper"), &argv, "/bin/zsh", &v(&["claude", "claude-wrapper"])), (v(&[]), true));
        assert_eq!(classify(Some("claude-wrapper"), &argv, "/bin/zsh", &wrapper), (v(&[]), true));
    }

    #[test]
    fn any_other_program_keeps_its_command_line() {
        assert_eq!(classify(Some("ssh"), &v(&["ssh", "host"]), "/bin/zsh", &v(&["claude"])), (v(&["ssh", "host"]), false));
        assert_eq!(classify(None, &[], "", &v(&["claude"])), (v(&[]), false));
    }

    #[test]
    fn arguments_reach_the_file_only_for_programs_that_are_restored() {
        let keep = v(&["vim", "ssh"]);
        assert_eq!(redact(v(&["vim", "a.rs"]), &keep), v(&["vim", "a.rs"]));
        assert_eq!(redact(v(&["/usr/bin/ssh", "host"]), &keep), v(&["/usr/bin/ssh", "host"]));
        assert_eq!(redact(v(&["mysql", "-pSECRET", "db"]), &keep), v(&["mysql"]));
        assert_eq!(redact(v(&["curl", "-H", "Authorization: Bearer x"]), &keep), v(&["curl"]));
        assert_eq!(redact(v(&["mysql", "-pSECRET"]), &[]), v(&["mysql"]));
        assert_eq!(redact(v(&[]), &keep), v(&[]));
    }

    /// Wait until `parent` has a child, up to two seconds.
    fn first_child(parent: i32) -> Option<i32> {
        for _ in 0..200 {
            if let Some(c) = proc::child_pids(parent).first() {
                return Some(*c);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn the_programs_between_the_shell_and_the_foreground_are_found_outermost_first() {
        let mut outer = std::process::Command::new("sh")
            .args(["-c", "sh -c 'sh -c \"sleep 5; sleep 0\"; sleep 0'; sleep 0"])
            .spawn()
            .expect("spawn sh");
        let top = outer.id() as i32;
        let middle = first_child(top).expect("middle shell");
        let inner = first_child(middle).expect("inner shell");
        let leaf = first_child(inner).expect("sleep");
        assert_eq!(enclosing(leaf, top), vec![middle, inner]);
        assert_eq!(enclosing(inner, top), vec![middle]);
        assert_eq!(enclosing(middle, top), Vec::<i32>::new(), "a direct child is inside nothing");
        assert_eq!(enclosing(leaf, 1_999_999), Vec::<i32>::new(), "not a descendant of that shell");
        let _ = outer.kill();
        let _ = outer.wait();
    }
}
