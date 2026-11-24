//! `imux setup`: put the manual, the agent skill, and the Claude Code hook where they are read, if
//! they are not there yet.
//!
//! The texts are built into the binary, so what gets installed is the copy that matches this imux.
//! The hook is a script (`examples/claude-hook.sh`) in Claude Code's directory plus the entries
//! that call it in Claude Code's `settings.json`. imux writes that file only when it is not there:
//! it has no business rewriting a file that holds a person's own settings, so an existing one is
//! left as it is and the entries to add are printed.
//! This is the only place imux writes outside its own state directory, and only when the user runs
//! the command. A file that is a symlink is left alone (a dotfiles manager owns it), one that
//! differs from this imux's copy is brought up to date, and one that is the same is not touched.
//! The shell startup file gets one line, `eval "$(imux init zsh)"` (or the shell's own spelling),
//! added once at the end.

use std::io;
use std::path::{Path, PathBuf};

/// The manual, as shipped (`docs/imux.1`).
pub const MAN: &str = include_str!("../docs/imux.1");

/// The Claude Code hook script, as shipped (`examples/claude-hook.sh`).
pub const CLAUDE_HOOK: &str = include_str!("../examples/claude-hook.sh");

/// What the hook script is called in Claude Code's directory.
pub const CLAUDE_HOOK_NAME: &str = "imux-claude-hook.sh";

/// Claude Code's directory (`~/.claude`), where the hook script and `settings.json` go, and the
/// home directory (to write paths as `~/...` in the settings).
#[derive(Debug, Clone)]
pub struct ClaudeHooks {
    pub dir: PathBuf,
    pub home: Option<PathBuf>,
}

/// The shells `imux init` can print a line for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

impl Shell {
    /// The shell named by a program path or a name (`/bin/zsh`, `fish`); `None` for any other.
    pub fn named(text: &str) -> Option<Shell> {
        match text.rsplit('/').next().unwrap_or(text).trim_start_matches('-') {
            "zsh" => Some(Shell::Zsh),
            "bash" => Some(Shell::Bash),
            "fish" => Some(Shell::Fish),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
            Shell::Fish => "fish",
        }
    }

    /// The line the startup file gets.
    pub fn line(self) -> String {
        match self {
            Shell::Fish => "imux init fish | source".to_string(),
            other => format!("eval \"$(imux init {})\"", other.name()),
        }
    }
}

/// The startup file of `shell`: zsh reads `$ZDOTDIR/.zshrc` (else `~/.zshrc`), fish
/// `$XDG_CONFIG_HOME/fish/config.fish` (else `~/.config/fish/config.fish`), and bash `~/.bashrc`,
/// or `~/.bash_profile` when only that one exists. `exists` says whether a path is there.
pub fn rc_file(shell: Shell, home: &Path, zdotdir: Option<&str>, xdg_config_home: Option<&str>, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    match shell {
        Shell::Zsh => match zdotdir {
            Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir).join(".zshrc"),
            _ => home.join(".zshrc"),
        },
        Shell::Fish => match xdg_config_home {
            Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir).join("fish/config.fish"),
            _ => home.join(".config/fish/config.fish"),
        },
        Shell::Bash => {
            let rc = home.join(".bashrc");
            let profile = home.join(".bash_profile");
            if !exists(&rc) && exists(&profile) { profile } else { rc }
        }
    }
}

/// The shell line to add, and the file to add it to.
#[derive(Debug, Clone)]
pub struct ShellRc {
    pub shell: Shell,
    pub path: PathBuf,
}

/// What `imux setup` was asked to do.
#[derive(Debug, Default)]
pub struct Plan {
    pub man: Option<PathBuf>,
    /// A manpage that came with this binary (see `bundled_manpage`). When the destination is not
    /// there yet, the manpage step then does nothing and says nothing.
    pub man_present: Option<PathBuf>,
    pub skill: Option<PathBuf>,
    pub claude: Option<ClaudeHooks>,
    pub shell: Option<ShellRc>,
    /// The user's shell, when `imux init` has no line for it: said in the report, nothing added.
    pub unsupported_shell: Option<String>,
}

/// What happened to one file.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    Appended,
    Updated,
    Unchanged,
    Skipped(&'static str),
}

/// Where the manual goes by default: `$XDG_DATA_HOME/man/man1/imux.1`, else
/// `~/.local/share/man/man1/imux.1`. Both arguments are the values of the variables.
pub fn default_man_path(xdg_data_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let base = match (xdg_data_home, home) {
        (Some(x), _) if Path::new(x).is_absolute() => PathBuf::from(x),
        (_, Some(h)) if !h.is_empty() => PathBuf::from(h).join(".local/share"),
        _ => return None,
    };
    Some(base.join("man/man1/imux.1"))
}

/// The manpage that was installed together with the binary `exe`, if there is one. A package puts
/// the binary at `<prefix>/bin/imux` and the page at `<prefix>/share/man/man1/imux.1`, which is what
/// Homebrew and the distributions do; a symlink to the binary is followed, since Homebrew links
/// `bin/imux` into its Cellar.
pub fn bundled_manpage(exe: &Path) -> Option<PathBuf> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let prefix = exe.parent()?.parent()?;
    ["share/man/man1/imux.1", "share/man/man1/imux.1.gz", "man/man1/imux.1", "man/man1/imux.1.gz"]
        .iter()
        .map(|rel| prefix.join(rel))
        .find(|p| p.is_file())
}

/// Add `line` at the end of the startup file `path`, once: a file that already mentions `imux init`
/// is left as it is. The file is created when it is not there.
pub fn add_line(path: &Path, line: &str) -> io::Result<Outcome> {
    let existing = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => return Ok(Outcome::Skipped("it is a symlink")),
        Ok(meta) if !meta.is_file() => return Ok(Outcome::Skipped("it is not a regular file")),
        Ok(_) => Some(std::fs::read_to_string(path)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if existing.as_deref().is_some_and(|text| text.contains("imux init")) {
        return Ok(Outcome::Unchanged);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = existing.clone().unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str("# imux: shell integration, added by imux setup\n");
    text.push_str(line);
    text.push('\n');
    let dir = path.parent().ok_or_else(|| io::Error::other("no directory to write in"))?;
    let tmp = dir.join(format!(".imux-setup-{}.tmp", std::process::id()));
    std::fs::write(&tmp, &text)?;
    if let Ok(meta) = std::fs::metadata(path) {
        // Keep the file's mode: a startup file may be private.
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(Outcome::Appended)
}

/// Put `text` at `path` unless it is already there. The write goes through a temporary file in the
/// same directory, so a reader never sees half a file.
pub fn put(path: &Path, text: &str) -> io::Result<Outcome> {
    let was = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => return Ok(Outcome::Skipped("it is a symlink")),
        Ok(meta) if !meta.is_file() => return Ok(Outcome::Skipped("it is not a regular file")),
        Ok(_) => Some(std::fs::read(path)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if was.as_deref() == Some(text.as_bytes()) {
        return Ok(Outcome::Unchanged);
    }
    let dir = path.parent().ok_or_else(|| io::Error::other("no directory to write in"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".imux-setup-{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(if was.is_some() { Outcome::Updated } else { Outcome::Installed })
}

/// How the settings name the script: `~/.claude/imux-claude-hook.sh` when it is under the home
/// directory, else the whole path; quoted for the shell when it needs it.
fn script_reference(script: &Path, home: Option<&Path>) -> String {
    let text = match home.and_then(|h| script.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => script.display().to_string(),
    };
    if text.chars().all(|c| c.is_ascii_alphanumeric() || "/._~-".contains(c)) { text } else { format!("\"{}\"", text.replace('"', "\\\"")) }
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The `settings.json` that lists the hook: the name on every prompt and at the start of a
/// session, and the way back at the start of a session.
pub fn claude_settings(script: &Path, home: Option<&Path>) -> String {
    let reference = script_reference(script, home);
    let entry = |what: &str| format!("{{ \"type\": \"command\", \"command\": {} }}", json_string(&format!("sh {reference} {what}")));
    format!(
        "{{\n  \"hooks\": {{\n    \"SessionStart\": [\n      {{ \"hooks\": [\n        {},\n        {}\n      ] }}\n    ],\n    \"UserPromptSubmit\": [\n      {{ \"hooks\": [{}] }}\n    ]\n  }}\n}}\n",
        entry("session"),
        entry("resume"),
        entry("session")
    )
}

/// Install the Claude Code hook: the script (kept up to date like the skill) and, when
/// `settings.json` is not there, a `settings.json` that calls it. An existing one is never
/// changed: it is said whether it lists the hook, and if it does not, what to add.
fn install_claude(hooks: &ClaudeHooks) -> io::Result<String> {
    let mut report = String::new();
    let script = hooks.dir.join(CLAUDE_HOOK_NAME);
    let outcome = put(&script, CLAUDE_HOOK).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", script.display())))?;
    report.push_str(&line("claude hook", &script, &outcome));
    if matches!(outcome, Outcome::Skipped(_)) {
        return Ok(report);
    }
    let settings = hooks.dir.join("settings.json");
    let wanted = claude_settings(&script, hooks.home.as_deref());
    let existing = match std::fs::symlink_metadata(&settings) {
        Ok(meta) if meta.file_type().is_symlink() => {
            report.push_str(&format!("claude hook: left {} alone, it is a symlink; add the hooks to it yourself:\n{wanted}", settings.display()));
            return Ok(report);
        }
        Ok(meta) if !meta.is_file() => {
            report.push_str(&format!("claude hook: left {} alone, it is not a regular file\n", settings.display()));
            return Ok(report);
        }
        Ok(_) => Some(std::fs::read_to_string(&settings).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", settings.display())))?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", settings.display()))),
    };
    match existing {
        None => {
            put(&settings, &wanted).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", settings.display())))?;
            report.push_str(&format!("claude hook: installed {} (it calls the script)\n", settings.display()));
        }
        Some(text) if text.contains(CLAUDE_HOOK_NAME) => {
            report.push_str(&format!("claude hook: {} already calls the script\n", settings.display()));
        }
        Some(text) if text.contains("imux agent name") || text.contains("claude-hook") => {
            report.push_str(&format!("claude hook: {} has hooks for imux of its own; replace them with the entries below to use the script:\n{wanted}", settings.display()));
        }
        Some(_) => {
            report.push_str(&format!("claude hook: {} exists, and imux does not edit it; add these entries to its \"hooks\" (merge them into the lists you have):\n{wanted}", settings.display()));
        }
    }
    Ok(report)
}

fn line(what: &str, path: &Path, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Installed => format!("{what}: installed {}\n", path.display()),
        Outcome::Appended => format!("{what}: added to {}\n", path.display()),
        Outcome::Updated => format!("{what}: updated {}\n", path.display()),
        Outcome::Unchanged => format!("{what}: already installed {}\n", path.display()),
        Outcome::Skipped(why) => format!("{what}: left {} alone, {why}\n", path.display()),
    }
}

/// Do the plan and report, one line per file. The first failure stops it.
pub fn run(plan: &Plan) -> io::Result<String> {
    let mut report = String::new();
    // A manpage that came with the binary is already where man looks: there is nothing to do or say.
    let man_there = plan.man_present.is_some() && std::fs::symlink_metadata(plan.man.as_deref().unwrap_or(Path::new(""))).is_err();
    if let Some(path) = plan.man.as_ref().filter(|_| !man_there) {
        let outcome = put(path, MAN).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        report.push_str(&line("manual", path, &outcome));
        if matches!(outcome, Outcome::Installed | Outcome::Updated)
            && let Some(root) = path.parent().and_then(Path::parent).and_then(Path::parent)
        {
            report.push_str(&format!("  read it with: man imux  (if man does not find it, add {} to MANPATH)\n", root.join("man").display()));
        }
    }
    if let Some(path) = &plan.skill {
        let outcome = put(path, crate::skill::TEXT).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        report.push_str(&line("skill", path, &outcome));
    }
    if let Some(hooks) = &plan.claude {
        report.push_str(&install_claude(hooks)?);
    }
    if let Some(name) = &plan.unsupported_shell {
        report.push_str(&format!("shell: nothing added: imux init has a line for zsh, bash and fish, not for {name}\n"));
    }
    if let Some(rc) = &plan.shell {
        let line = rc.shell.line();
        let outcome = add_line(&rc.path, &line).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", rc.path.display())))?;
        report.push_str(&match outcome {
            Outcome::Appended => format!("shell: added to {}: {line}\n  open a new terminal, or run: {line}\n", rc.path.display()),
            Outcome::Unchanged => format!("shell: already set up in {}\n", rc.path.display()),
            Outcome::Skipped(why) => format!("shell: left {} alone, {why}; add this line to it yourself: {line}\n", rc.path.display()),
            other => line_for(&other, &rc.path),
        });
    }
    Ok(report)
}

fn line_for(outcome: &Outcome, path: &Path) -> String {
    line("shell", path, outcome)
}

/// Kept in the state directory by `imux setup` when it has run: nothing is left to remind of.
pub const DONE_MARKER: &str = "setup-done";
/// Kept in the state directory when the reminder was shown: the version of imux it was shown by.
pub const SEEN_MARKER: &str = "setup-hint-seen";

/// Where imux setup has not been finished, if it can say: the startup file of the user's shell
/// when it has no `imux init` line. Nothing is said (`None`) when the person turned the reminder
/// off (`IMUX_NO_SETUP_HINT` set to anything but empty or 0), when `imux setup` has run before
/// (the marker is in the state directory, also with `--no-shell`), when the shell is one that
/// `imux init` has no line for, when the startup file is a symlink or not a regular file (setup
/// leaves those alone, so a reminder could never be answered), and whenever it cannot tell. `env`
/// reads an environment variable.
pub fn unfinished(env: &dyn Fn(&str) -> Option<String>, state_dir: Option<&Path>) -> Option<PathBuf> {
    if env("IMUX_NO_SETUP_HINT").is_some_and(|v| !v.is_empty() && v != "0") {
        return None;
    }
    if state_dir.is_some_and(|d| d.join(DONE_MARKER).exists()) {
        return None;
    }
    let home = env("HOME").filter(|h| !h.is_empty())?;
    let shell = Shell::named(&env("SHELL")?)?;
    let rc = rc_file(shell, Path::new(&home), env("ZDOTDIR").as_deref(), env("XDG_CONFIG_HOME").as_deref(), &|p| p.exists());
    match std::fs::symlink_metadata(&rc) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => None,
        Ok(_) => match std::fs::read_to_string(&rc) {
            Ok(text) if text.contains("imux init") => None,
            Ok(_) => Some(rc),
            Err(_) => None,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(rc),
        Err(_) => None,
    }
}

/// Write the marker that says `imux setup` has run. Best effort: a state directory that cannot be
/// written only means the reminder may come again.
pub fn mark_done(state_dir: &Path) {
    if std::fs::create_dir_all(state_dir).is_ok() {
        let _ = std::fs::write(state_dir.join(DONE_MARKER), format!("{}\n", env!("CARGO_PKG_VERSION")));
    }
}

/// Whether the reminder was shown already by this version of imux.
pub fn seen_by_this_version(state_dir: &Path) -> bool {
    std::fs::read_to_string(state_dir.join(SEEN_MARKER)).is_ok_and(|v| v.trim() == env!("CARGO_PKG_VERSION"))
}

/// Remember that this version showed the reminder.
pub fn mark_seen(state_dir: &Path) {
    if std::fs::create_dir_all(state_dir).is_ok() {
        let _ = std::fs::write(state_dir.join(SEEN_MARKER), format!("{}\n", env!("CARGO_PKG_VERSION")));
    }
}

/// The rc file as the reminder names it: `~/.zshrc` under the home directory.
pub fn shown_path(rc: &Path, home: Option<&str>) -> String {
    match home.and_then(|h| rc.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => rc.display().to_string(),
    }
}

/// What `imux doctor` says about setup: one line, whatever the state.
pub fn doctor_line(env: &dyn Fn(&str) -> Option<String>, state_dir: Option<&Path>) -> (String, String) {
    let text = if state_dir.is_some_and(|d| d.join(DONE_MARKER).exists()) {
        "imux setup has been run".to_string()
    } else if let Some(shell) = env("SHELL").as_deref().and_then(Shell::named) {
        let home = env("HOME");
        let rc = rc_file(shell, Path::new(home.as_deref().unwrap_or("")), env("ZDOTDIR").as_deref(), env("XDG_CONFIG_HOME").as_deref(), &|p| p.exists());
        let shown = shown_path(&rc, home.as_deref());
        match std::fs::read_to_string(&rc) {
            Ok(text) if text.contains("imux init") => format!("the shell line is in {shown}"),
            _ => format!("not finished: {shown} has no imux init line (run imux setup)"),
        }
    } else {
        "imux init has no line for your shell, so there is nothing to add to it".to_string()
    };
    ("setup".to_string(), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imux-setup-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_manual_is_the_shipped_file() {
        assert!(MAN.starts_with(".\\\"") || MAN.contains(".TH"), "it is a man page");
        let on_disk = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/imux.1")).unwrap();
        assert_eq!(on_disk, MAN);
    }

    #[test]
    fn the_default_manual_path_follows_xdg_then_home() {
        assert_eq!(default_man_path(Some("/x/data"), Some("/h")), Some(PathBuf::from("/x/data/man/man1/imux.1")));
        assert_eq!(default_man_path(None, Some("/h")), Some(PathBuf::from("/h/.local/share/man/man1/imux.1")));
        assert_eq!(default_man_path(Some("relative"), Some("/h")), Some(PathBuf::from("/h/.local/share/man/man1/imux.1")), "a relative XDG value is ignored");
        assert_eq!(default_man_path(None, Some("")), None);
        assert_eq!(default_man_path(None, None), None);
    }

    #[test]
    fn a_missing_file_is_installed_with_its_directories_then_left_alone() {
        let dir = scratch("install");
        let path = dir.join("a/b/c/file");
        assert_eq!(put(&path, "one").unwrap(), Outcome::Installed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        assert_eq!(put(&path, "one").unwrap(), Outcome::Unchanged);
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temporary file stays behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_differs_is_brought_up_to_date() {
        let dir = scratch("update");
        let path = dir.join("file");
        std::fs::write(&path, "old").unwrap();
        assert_eq!(put(&path, "new").unwrap(), Outcome::Updated);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlink_and_a_directory_are_left_alone() {
        let dir = scratch("links");
        let target = dir.join("target");
        std::fs::write(&target, "mine").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(put(&link, "imux").unwrap(), Outcome::Skipped(_)));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "mine", "what the link points at is not written");
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert!(matches!(put(&sub, "imux").unwrap(), Outcome::Skipped(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plan_installs_what_it_names_and_reports_each_file() {
        let dir = scratch("plan");
        let plan = Plan { man: Some(dir.join("share/man/man1/imux.1")), skill: Some(dir.join("skills/imux/SKILL.md")), ..Plan::default() };
        let first = run(&plan).unwrap();
        assert!(first.contains("manual: installed") && first.contains("skill: installed"), "{first}");
        assert!(first.contains("MANPATH"), "the first install says how to read it");
        assert_eq!(std::fs::read_to_string(plan.skill.as_ref().unwrap()).unwrap(), crate::skill::TEXT);
        let second = run(&plan).unwrap();
        assert!(second.contains("manual: already installed") && second.contains("skill: already installed"), "{second}");
        assert!(!second.contains("MANPATH"));
        let only_skill = Plan { skill: plan.skill.clone(), ..Plan::default() };
        assert!(!run(&only_skill).unwrap().contains("manual"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_names_the_file() {
        let dir = scratch("fail");
        let blocker = dir.join("file");
        std::fs::write(&blocker, "x").unwrap();
        let plan = Plan { man: Some(blocker.join("under/imux.1")), ..Plan::default() };
        let err = run(&plan).unwrap_err();
        assert!(err.to_string().contains("imux.1"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_shell_is_told_from_its_name_and_gets_its_own_line() {
        assert_eq!(Shell::named("/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::named("/opt/homebrew/bin/fish"), Some(Shell::Fish));
        assert_eq!(Shell::named("-bash"), Some(Shell::Bash), "a login shell's name");
        assert_eq!(Shell::named("/usr/bin/nu"), None);
        assert_eq!(Shell::named(""), None);
        assert_eq!(Shell::Zsh.line(), "eval \"$(imux init zsh)\"");
        assert_eq!(Shell::Bash.line(), "eval \"$(imux init bash)\"");
        assert_eq!(Shell::Fish.line(), "imux init fish | source");
    }

    #[test]
    fn each_shell_has_its_startup_file() {
        let home = Path::new("/h");
        let none = |_: &Path| false;
        assert_eq!(rc_file(Shell::Zsh, home, None, None, &none), PathBuf::from("/h/.zshrc"));
        assert_eq!(rc_file(Shell::Zsh, home, Some("/z"), None, &none), PathBuf::from("/z/.zshrc"));
        assert_eq!(rc_file(Shell::Zsh, home, Some("rel"), None, &none), PathBuf::from("/h/.zshrc"), "a relative ZDOTDIR is ignored");
        assert_eq!(rc_file(Shell::Fish, home, None, None, &none), PathBuf::from("/h/.config/fish/config.fish"));
        assert_eq!(rc_file(Shell::Fish, home, None, Some("/x"), &none), PathBuf::from("/x/fish/config.fish"));
        assert_eq!(rc_file(Shell::Bash, home, None, None, &none), PathBuf::from("/h/.bashrc"));
        let only_profile = |p: &Path| p == Path::new("/h/.bash_profile");
        assert_eq!(rc_file(Shell::Bash, home, None, None, &only_profile), PathBuf::from("/h/.bash_profile"));
        let both = |_: &Path| true;
        assert_eq!(rc_file(Shell::Bash, home, None, None, &both), PathBuf::from("/h/.bashrc"));
    }

    #[test]
    fn the_line_is_added_once_at_the_end_and_the_rest_is_kept() {
        let dir = scratch("rc");
        let path = dir.join("sub/.zshrc");
        let line = Shell::Zsh.line();
        assert_eq!(add_line(&path, &line).unwrap(), Outcome::Appended, "a missing file is created");
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.ends_with(&format!("{line}\n")), "{first}");
        assert_eq!(add_line(&path, &line).unwrap(), Outcome::Unchanged);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        let theirs = dir.join("theirs");
        std::fs::write(&theirs, "export A=1").unwrap();
        assert_eq!(add_line(&theirs, &line).unwrap(), Outcome::Appended);
        let text = std::fs::read_to_string(&theirs).unwrap();
        assert!(text.starts_with("export A=1\n"), "their text comes first, with its line ended: {text}");
        assert!(text.ends_with(&format!("{line}\n")));

        let had = dir.join("had");
        std::fs::write(&had, "eval \"$(imux init bash)\"\n").unwrap();
        assert_eq!(add_line(&had, &line).unwrap(), Outcome::Unchanged, "any imux init line counts");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_startup_file_keeps_its_mode_and_a_symlink_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("rcmode");
        let path = dir.join("rc");
        std::fs::write(&path, "x\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        add_line(&path, "imux init zsh").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let target = dir.join("target");
        std::fs::write(&target, "mine\n").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(add_line(&link, "imux init zsh").unwrap(), Outcome::Skipped(_)));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "mine\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_report_says_what_was_added_and_what_could_not_be() {
        let dir = scratch("rcreport");
        let rc = ShellRc { shell: Shell::Fish, path: dir.join("config.fish") };
        let plan = Plan { shell: Some(rc.clone()), ..Plan::default() };
        let first = run(&plan).unwrap();
        assert!(first.contains("shell: added to") && first.contains("imux init fish | source"), "{first}");
        assert!(run(&plan).unwrap().contains("shell: already set up"));
        let other = Plan { unsupported_shell: Some("/usr/bin/nu".into()), ..Plan::default() };
        assert!(run(&other).unwrap().contains("not for /usr/bin/nu"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_manpage_next_to_the_binary_is_found_through_a_symlink() {
        let dir = scratch("bundled");
        std::fs::create_dir_all(dir.join("keg/bin")).unwrap();
        std::fs::create_dir_all(dir.join("keg/share/man/man1")).unwrap();
        std::fs::create_dir_all(dir.join("prefix/bin")).unwrap();
        std::fs::write(dir.join("keg/bin/imux"), "x").unwrap();
        assert_eq!(bundled_manpage(&dir.join("keg/bin/imux")), None, "no page, nothing found");
        std::fs::write(dir.join("keg/share/man/man1/imux.1"), ".TH IMUX 1\n").unwrap();
        let found = bundled_manpage(&dir.join("keg/bin/imux")).expect("found next to the binary");
        assert!(found.ends_with("keg/share/man/man1/imux.1"), "{}", found.display());
        std::os::unix::fs::symlink(dir.join("keg/bin/imux"), dir.join("prefix/bin/imux")).unwrap();
        assert!(bundled_manpage(&dir.join("prefix/bin/imux")).is_some(), "a symlink to the binary is followed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_manpage_that_came_with_the_binary_is_skipped_without_a_word() {
        let dir = scratch("present");
        let dest = dir.join("home/.local/share/man/man1/imux.1");
        let plan = Plan { man: Some(dest.clone()), man_present: Some(dir.join("elsewhere/imux.1")), skill: Some(dir.join("skills/imux/SKILL.md")), ..Plan::default() };
        let report = run(&plan).unwrap();
        assert!(!report.contains("manual") && !report.contains("MANPATH"), "{report}");
        assert!(!dest.exists(), "nothing was written");
        assert!(report.contains("skill: installed"), "the other steps still run: {report}");
        // a copy of our own that is already there is still kept up to date
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, "old").unwrap();
        assert!(run(&plan).unwrap().contains("manual: updated"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn hooks_in(dir: &Path) -> Plan {
        Plan { claude: Some(ClaudeHooks { dir: dir.join("home/.claude"), home: Some(dir.join("home")) }), ..Plan::default() }
    }

    #[test]
    fn the_claude_hook_is_the_shipped_script_and_a_new_settings_file_calls_it() {
        let on_disk = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/claude-hook.sh")).unwrap();
        assert_eq!(on_disk, CLAUDE_HOOK);
        let dir = scratch("claude-new");
        let report = run(&hooks_in(&dir)).unwrap();
        let script = dir.join("home/.claude").join(CLAUDE_HOOK_NAME);
        assert_eq!(std::fs::read_to_string(&script).unwrap(), CLAUDE_HOOK);
        assert!(report.contains("claude hook: installed") && report.contains("settings.json (it calls the script)"), "{report}");
        let settings = std::fs::read_to_string(dir.join("home/.claude/settings.json")).unwrap();
        assert!(settings.contains("\"command\": \"sh ~/.claude/imux-claude-hook.sh session\""), "{settings}");
        assert!(settings.contains("sh ~/.claude/imux-claude-hook.sh resume"), "{settings}");
        assert!(settings.contains("\"SessionStart\"") && settings.contains("\"UserPromptSubmit\""), "{settings}");
        // the second run changes nothing and says so
        let again = run(&hooks_in(&dir)).unwrap();
        assert!(again.contains("claude hook: already installed") && again.contains("already calls the script"), "{again}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_settings_file_is_never_changed_and_the_entries_are_printed() {
        let dir = scratch("claude-existing");
        let claude = dir.join("home/.claude");
        std::fs::create_dir_all(&claude).unwrap();
        let mine = "{\n  \"permissions\": { \"allow\": [\"Bash(ls)\"] }\n}\n";
        std::fs::write(claude.join("settings.json"), mine).unwrap();
        let report = run(&hooks_in(&dir)).unwrap();
        assert_eq!(std::fs::read_to_string(claude.join("settings.json")).unwrap(), mine, "not one byte of it changed");
        assert!(report.contains("does not edit it") && report.contains("sh ~/.claude/imux-claude-hook.sh resume"), "{report}");
        assert!(claude.join(CLAUDE_HOOK_NAME).exists(), "the script is installed anyway");
        // a file that already lists the script, and one with hooks of imux's own from before
        let listed = "{\"hooks\":{\"SessionStart\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"sh ~/.claude/imux-claude-hook.sh session\"}]}]}}";
        std::fs::write(claude.join("settings.json"), listed).unwrap();
        assert!(run(&hooks_in(&dir)).unwrap().contains("already calls the script"));
        let old = "{\"hooks\":{\"SessionStart\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"imux agent name 1 x\"}]}]}}";
        std::fs::write(claude.join("settings.json"), old).unwrap();
        let report = run(&hooks_in(&dir)).unwrap();
        assert!(report.contains("has hooks for imux of its own"), "{report}");
        assert_eq!(std::fs::read_to_string(claude.join("settings.json")).unwrap(), old);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlinked_script_or_settings_file_is_left_alone() {
        let dir = scratch("claude-links");
        let claude = dir.join("home/.claude");
        std::fs::create_dir_all(&claude).unwrap();
        let theirs = dir.join("dotfiles-settings.json");
        std::fs::write(&theirs, "{}").unwrap();
        std::os::unix::fs::symlink(&theirs, claude.join("settings.json")).unwrap();
        let report = run(&hooks_in(&dir)).unwrap();
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "{}");
        assert!(report.contains("it is a symlink"), "{report}");
        let other = dir.join("dotfiles-hook.sh");
        std::fs::write(&other, "mine").unwrap();
        std::fs::remove_file(claude.join(CLAUDE_HOOK_NAME)).unwrap();
        std::os::unix::fs::symlink(&other, claude.join(CLAUDE_HOOK_NAME)).unwrap();
        let report = run(&hooks_in(&dir)).unwrap();
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "mine");
        assert!(report.contains("left") && report.contains("alone"), "{report}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_outside_home_or_with_a_space_is_written_so_the_shell_and_json_read_it() {
        let script = Path::new("/opt/my claude/imux-claude-hook.sh");
        let text = claude_settings(script, Some(Path::new("/home/me")));
        assert!(text.contains("\"command\": \"sh \\\"/opt/my claude/imux-claude-hook.sh\\\" session\""), "{text}");
        let under = claude_settings(Path::new("/home/me/.claude/imux-claude-hook.sh"), Some(Path::new("/home/me")));
        assert!(under.contains("sh ~/.claude/imux-claude-hook.sh session"), "{under}");
    }

    type EnvFn = Box<dyn Fn(&str) -> Option<String>>;

    fn env_for(home: &str, shell: &str) -> EnvFn {
        let (home, shell) = (home.to_string(), shell.to_string());
        Box::new(move |k| match k {
            "HOME" => Some(home.clone()),
            "SHELL" => Some(shell.clone()),
            _ => None,
        })
    }

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn setup_is_unfinished_when_the_shell_has_no_line_and_nothing_says_otherwise() {
        let dir = scratch("unfinished");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let h = home.to_str().unwrap().to_string();
        let state = dir.join("state");
        let env = |shell: &str| env_for(&h, shell);
        // no startup file yet: the one setup would create is named
        assert_eq!(unfinished(&env("/bin/zsh"), Some(&state)), Some(home.join(".zshrc")));
        assert_eq!(unfinished(&env("/usr/bin/fish"), Some(&state)), Some(home.join(".config/fish/config.fish")));
        // a file that has other things but no line
        std::fs::write(home.join(".bashrc"), "export A=1\n").unwrap();
        assert_eq!(unfinished(&env("/bin/bash"), Some(&state)), Some(home.join(".bashrc")));
        // the line is there (however it was put there)
        std::fs::write(home.join(".bashrc"), "eval \"$(imux init bash)\"\n").unwrap();
        assert_eq!(unfinished(&env("/bin/bash"), Some(&state)), None);
        // a shell imux init has no line for, and no shell at all
        assert_eq!(unfinished(&env("/bin/sh"), Some(&state)), None);
        assert_eq!(unfinished(&env("/usr/bin/nu"), Some(&state)), None);
        assert_eq!(unfinished(&env_of(&[("HOME", h_str(&home))]), Some(&state)), None, "no SHELL");
        assert_eq!(unfinished(&env_of(&[("SHELL", "/bin/zsh")]), Some(&state)), None, "no HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn h_str(p: &Path) -> &str {
        // the tests' scratch paths are plain UTF-8
        Box::leak(p.to_str().unwrap().to_string().into_boxed_str())
    }

    #[test]
    fn the_reminder_is_off_after_setup_ran_with_the_variable_and_for_a_file_setup_would_not_touch() {
        let dir = scratch("quiet");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let state = dir.join("state");
        let h = home.to_str().unwrap().to_string();
        let base = |extra: Option<(&'static str, &'static str)>| {
            let h = h.clone();
            move |k: &str| match (k, extra) {
                ("HOME", _) => Some(h.clone()),
                ("SHELL", _) => Some("/bin/zsh".to_string()),
                (name, Some((n, v))) if name == n => Some(v.to_string()),
                _ => None,
            }
        };
        assert!(unfinished(&base(None), Some(&state)).is_some());
        for off in ["1", "yes", "true"] {
            assert_eq!(unfinished(&base(Some(("IMUX_NO_SETUP_HINT", off))), Some(&state)), None, "{off}");
        }
        for on in ["", "0"] {
            assert!(unfinished(&base(Some(("IMUX_NO_SETUP_HINT", on))), Some(&state)).is_some(), "{on:?} does not turn it off");
        }
        mark_done(&state);
        assert!(state.join(DONE_MARKER).exists());
        assert_eq!(unfinished(&base(None), Some(&state)), None, "setup has run");
        std::fs::remove_file(state.join(DONE_MARKER)).unwrap();
        // a symlinked startup file and a directory in its place are left alone by setup, so no reminder
        std::fs::write(home.join("dot-zshrc"), "# mine\n").unwrap();
        std::os::unix::fs::symlink(home.join("dot-zshrc"), home.join(".zshrc")).unwrap();
        assert_eq!(unfinished(&base(None), Some(&state)), None);
        std::fs::remove_file(home.join(".zshrc")).unwrap();
        std::fs::create_dir(home.join(".zshrc")).unwrap();
        assert_eq!(unfinished(&base(None), Some(&state)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_reminder_is_kept_by_version_and_paths_are_shown_under_home() {
        let dir = scratch("seen");
        let state = dir.join("state");
        assert!(!seen_by_this_version(&state), "nothing yet");
        mark_seen(&state);
        assert!(seen_by_this_version(&state));
        std::fs::write(state.join(SEEN_MARKER), "0.0.1\n").unwrap();
        assert!(!seen_by_this_version(&state), "an older version's mark does not count");
        assert_eq!(shown_path(Path::new("/home/me/.zshrc"), Some("/home/me")), "~/.zshrc");
        assert_eq!(shown_path(Path::new("/etc/zshrc"), Some("/home/me")), "/etc/zshrc");
        assert_eq!(shown_path(Path::new("/home/me/.zshrc"), None), "/home/me/.zshrc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn doctor_says_which_of_the_three_it_is() {
        let dir = scratch("doctor");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let state = dir.join("state");
        let h = home.to_str().unwrap().to_string();
        let env = |shell: &str| env_for(&h, shell);
        let (key, text) = doctor_line(&env("/bin/zsh"), Some(&state));
        assert_eq!(key, "setup");
        assert!(text.starts_with("not finished: ~/.zshrc has no imux init line"), "{text}");
        std::fs::write(home.join(".zshrc"), "eval \"$(imux init zsh)\"\n").unwrap();
        assert_eq!(doctor_line(&env("/bin/zsh"), Some(&state)).1, "the shell line is in ~/.zshrc");
        assert!(doctor_line(&env("/bin/sh"), Some(&state)).1.contains("no line for your shell"));
        mark_done(&state);
        assert_eq!(doctor_line(&env("/bin/zsh"), Some(&state)).1, "imux setup has been run");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
