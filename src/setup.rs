//! `imux setup`: put the manual and the agent skill where they are read, if they are not there yet.
//!
//! Both texts are built into the binary, so what gets installed is the copy that matches this imux.
//! This is the only place imux writes outside its own state directory, and only when the user runs
//! the command. A file that is a symlink is left alone (a dotfiles manager owns it), one that
//! differs from this imux's copy is brought up to date, and one that is the same is not touched.
//! The shell startup file gets one line, `eval "$(imux init zsh)"` (or the shell's own spelling),
//! added once at the end.

use std::io;
use std::path::{Path, PathBuf};

/// The manual, as shipped (`docs/imux.1`).
pub const MAN: &str = include_str!("../docs/imux.1");

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
}
