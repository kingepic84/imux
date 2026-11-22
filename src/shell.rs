//! The shell a pane runs and the shell syntax imux has to write for it. Some commands are typed
//! into a pane as text (a restored program, `worktree-command` with its start text, a setup
//! command chained before it), and quoting and "run this only if that worked" differ between
//! shells. The shell is told apart by its name; one that is not known gets nothing typed that
//! needs quoting or chaining, because a guess would be an injection waiting to happen.

/// How a shell reads quotes and sequences commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    /// sh, bash, zsh, dash, ksh and relatives.
    Posix,
    Fish,
    Nu,
    /// PowerShell, `pwsh` or `powershell`.
    Pwsh,
    /// Anything else, csh and tcsh included (`!` is still expanded inside their single quotes).
    Unknown,
}

/// The shell to start in panes: the `default-shell` option, else `$SHELL`, else `/bin/sh`.
pub fn path(configured: &str) -> String {
    if !configured.is_empty() {
        return configured.to_string();
    }
    match std::env::var("SHELL") {
        Ok(shell) if !shell.is_empty() => shell,
        _ => "/bin/sh".to_string(),
    }
}

impl ShellKind {
    /// Kind of the shell at `path`: its file name, in the login form (`-zsh`) or with `.exe` too.
    pub fn of(path: &str) -> ShellKind {
        let name = path.rsplit('/').next().unwrap_or(path).trim_start_matches('-').to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        match name {
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "yash" | "busybox" => ShellKind::Posix,
            "fish" => ShellKind::Fish,
            "nu" => ShellKind::Nu,
            "pwsh" | "powershell" => ShellKind::Pwsh,
            _ => ShellKind::Unknown,
        }
    }

    /// The value of the `shell-syntax` option: a kind by name, `None` for `auto` and anything else.
    pub fn named(name: &str) -> Option<ShellKind> {
        match name {
            "posix" => Some(ShellKind::Posix),
            "fish" => Some(ShellKind::Fish),
            "nu" => Some(ShellKind::Nu),
            "pwsh" => Some(ShellKind::Pwsh),
            _ => None,
        }
    }

    /// Text that runs `first` and then `second` only if `first` succeeded. `None` for a shell
    /// that is not known.
    pub fn chain(self, first: &str, second: &str) -> Option<String> {
        match self {
            ShellKind::Posix => Some(format!("{first} && {second}")),
            // `&&` only exists from fish 3.0, `; and` works in every version.
            ShellKind::Fish => Some(format!("{first}; and {second}")),
            ShellKind::Nu => Some(format!("{first}; if $env.LAST_EXIT_CODE == 0 {{ {second} }}")),
            // `&&` only exists from PowerShell 7, this form also works in Windows PowerShell 5.1.
            ShellKind::Pwsh => Some(format!("{first}; if ($?) {{ {second} }}")),
            ShellKind::Unknown => None,
        }
    }

    /// `word` as one word of a command line. `None` for a shell that is not known.
    pub fn quote(self, word: &str) -> Option<String> {
        match self {
            ShellKind::Posix => Some(posix(word)),
            ShellKind::Fish => Some(fish(word)),
            ShellKind::Nu => Some(nu(word)),
            ShellKind::Pwsh => Some(pwsh(word)),
            ShellKind::Unknown => None,
        }
    }
}

fn plain(word: &str, extra: &[u8]) -> bool {
    !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

fn posix(word: &str) -> String {
    if plain(word, b"_./+=:@,%-") { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

/// Inside single quotes fish treats `\\` and `\'` as escapes and every other backslash as itself.
fn fish(word: &str) -> String {
    if plain(word, b"_./+=:@,%-") {
        return word.to_string();
    }
    format!("'{}'", word.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Nushell single quotes hold anything but a single quote; for those a raw string with enough `#`.
/// A word that starts with `-` is quoted too: bare, a nushell command would take it for a flag.
fn nu(word: &str) -> String {
    if plain(word, b"_./+:@-") && !word.starts_with('-') {
        return word.to_string();
    }
    if !word.contains('\'') {
        return format!("'{word}'");
    }
    let mut hashes = 1;
    while word.contains(&format!("'{}", "#".repeat(hashes))) {
        hashes += 1;
    }
    let h = "#".repeat(hashes);
    format!("r{h}'{word}'{h}")
}

/// PowerShell doubles a single quote inside single quotes, and also reads the typographic
/// single quotes as quotes. A comma would make an array, so it is not a plain character.
fn pwsh(word: &str) -> String {
    if plain(word, b"_./+:@-") {
        return word.to_string();
    }
    let mut out = String::from("'");
    for c in word.chars() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn the_kind_comes_from_the_name_of_the_shell() {
        for (path, kind) in [
            ("/bin/zsh", ShellKind::Posix),
            ("/usr/bin/bash", ShellKind::Posix),
            ("-zsh", ShellKind::Posix),
            ("sh", ShellKind::Posix),
            ("/opt/homebrew/bin/fish", ShellKind::Fish),
            ("/opt/homebrew/bin/nu", ShellKind::Nu),
            ("/usr/local/bin/pwsh", ShellKind::Pwsh),
            ("C:/Program Files/PowerShell/pwsh.exe", ShellKind::Pwsh),
            ("powershell", ShellKind::Pwsh),
            ("/bin/tcsh", ShellKind::Unknown),
            ("/bin/csh", ShellKind::Unknown),
            ("/usr/bin/xonsh", ShellKind::Unknown),
            ("", ShellKind::Unknown),
        ] {
            assert_eq!(ShellKind::of(path), kind, "{path}");
        }
    }

    #[test]
    fn the_option_names_a_kind_or_means_auto() {
        assert_eq!(ShellKind::named("fish"), Some(ShellKind::Fish));
        assert_eq!(ShellKind::named("posix"), Some(ShellKind::Posix));
        assert_eq!(ShellKind::named("auto"), None);
        assert_eq!(ShellKind::named("tcsh"), None);
    }

    #[test]
    fn the_configured_shell_wins_and_the_fallback_is_sh() {
        assert_eq!(path("/opt/fish"), "/opt/fish");
        assert!(!path("").is_empty());
    }

    #[test]
    fn each_shell_chains_in_its_own_way_and_an_unknown_one_not_at_all() {
        assert_eq!(ShellKind::Posix.chain("npm ci", "claude").as_deref(), Some("npm ci && claude"));
        assert_eq!(ShellKind::Fish.chain("npm ci", "claude").as_deref(), Some("npm ci; and claude"));
        assert_eq!(
            ShellKind::Nu.chain("npm ci", "claude").as_deref(),
            Some("npm ci; if $env.LAST_EXIT_CODE == 0 { claude }")
        );
        assert_eq!(ShellKind::Pwsh.chain("npm ci", "claude").as_deref(), Some("npm ci; if ($?) { claude }"));
        assert_eq!(ShellKind::Unknown.chain("a", "b"), None);
    }

    #[test]
    fn quoting_per_shell_has_the_expected_shape() {
        let q = |k: ShellKind, w: &str| k.quote(w).unwrap();
        assert_eq!(q(ShellKind::Posix, "vim"), "vim");
        assert_eq!(q(ShellKind::Posix, "my file"), "'my file'");
        assert_eq!(q(ShellKind::Posix, "it's"), "'it'\\''s'");
        assert_eq!(q(ShellKind::Posix, ""), "''");
        assert_eq!(q(ShellKind::Fish, "it's"), "'it\\'s'");
        assert_eq!(q(ShellKind::Fish, "a\\"), "'a\\\\'");
        assert_eq!(q(ShellKind::Nu, "my file"), "'my file'");
        assert_eq!(q(ShellKind::Nu, "it's"), "r#'it's'#");
        assert_eq!(q(ShellKind::Nu, "a'#b"), "r##'a'#b'##");
        assert_eq!(q(ShellKind::Pwsh, "it's"), "'it''s'");
        assert_eq!(q(ShellKind::Pwsh, "\u{2018}x\u{2019}"), "'\u{2018}\u{2018}x\u{2019}\u{2019}'");
        assert_eq!(q(ShellKind::Pwsh, "a,b"), "'a,b'");
        assert_eq!(ShellKind::Unknown.quote("x"), None);
    }

    const HOSTILE: &[&str] = &[
        "plain",
        "a b",
        "it's",
        "'",
        "''",
        "$HOME",
        "$(id)",
        "`id`",
        "a;b",
        "a&&b",
        "a|b",
        ">f",
        "*",
        "~",
        "!x",
        "#c",
        "a#b",
        "{a,b}",
        "-n",
        "\\",
        "a\\",
        "\\'",
        "\"q\"",
        "caf\u{e9}",
        "\u{2018}x\u{2019}",
        "100%",
        "",
    ];

    /// Run `command` in `shell` (found on `PATH`); `None` when the shell is not installed.
    fn run(shell: &str, flag: &[&str], command: &str) -> Option<String> {
        let out = Command::new(shell).args(flag).arg(command).stdin(Stdio::null()).output().ok()?;
        assert!(out.status.success(), "{shell} {command:?}: {}", String::from_utf8_lossy(&out.stderr));
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Every hostile word must reach the program as exactly one argument, unchanged.
    fn round_trip(kind: ShellKind, shell: &str, flag: &[&str], echo: impl Fn(&str) -> String) {
        for word in HOSTILE {
            let command = echo(&kind.quote(word).unwrap());
            let Some(got) = run(shell, flag, &command) else { return };
            assert_eq!(&got, word, "{shell}: {command:?}");
        }
    }

    #[test]
    fn hostile_words_survive_every_installed_posix_shell() {
        for shell in ["sh", "bash", "zsh", "dash", "ksh"] {
            round_trip(ShellKind::Posix, shell, &["-c"], |w| format!("printf %s {w}"));
        }
    }

    #[test]
    fn hostile_words_survive_fish_nu_and_pwsh_when_they_are_installed() {
        round_trip(ShellKind::Fish, "fish", &["-c"], |w| format!("printf %s {w}"));
        round_trip(ShellKind::Nu, "nu", &["-c"], |w| format!("print -n {w}"));
        round_trip(ShellKind::Pwsh, "pwsh", &["-NoProfile", "-Command"], |w| format!("& /usr/bin/printf %s {w}"));
    }

    #[test]
    fn chaining_runs_the_second_command_only_after_success_in_every_installed_shell() {
        for (kind, shell, flag) in [
            (ShellKind::Posix, "sh", &["-c"][..]),
            (ShellKind::Posix, "bash", &["-c"][..]),
            (ShellKind::Posix, "zsh", &["-c"][..]),
            (ShellKind::Fish, "fish", &["-c"][..]),
            (ShellKind::Nu, "nu", &["-c"][..]),
            (ShellKind::Pwsh, "pwsh", &["-NoProfile", "-Command"][..]),
        ] {
            let (yes, no) = if kind == ShellKind::Nu { ("^true", "^false") } else { ("true", "false") };
            let ok = kind.chain(yes, "echo second").unwrap();
            if let Some(out) = run(shell, flag, &ok) {
                assert_eq!(out.trim(), "second", "{shell}: {ok}");
                let bad = kind.chain(no, "echo second").unwrap();
                let out = Command::new(shell).args(flag).arg(&bad).stdin(Stdio::null()).output().unwrap();
                assert!(!String::from_utf8_lossy(&out.stdout).contains("second"), "{shell}: {bad}");
            }
        }
    }

    /// What imux types is a command line for external programs (`claude --resume x`), so the words
    /// are checked as arguments of one: each hostile word must arrive as exactly one argument.
    #[test]
    fn hostile_words_reach_an_external_program_as_one_argument_in_every_installed_shell() {
        let printf = "/usr/bin/printf";
        if !std::path::Path::new(printf).exists() {
            return;
        }
        for (kind, shell, flag, call) in [
            (ShellKind::Posix, "sh", &["-c"][..], ""),
            (ShellKind::Posix, "bash", &["-c"][..], ""),
            (ShellKind::Posix, "zsh", &["-c"][..], ""),
            (ShellKind::Fish, "fish", &["-c"][..], ""),
            (ShellKind::Nu, "nu", &["-c"][..], "^"),
            (ShellKind::Pwsh, "pwsh", &["-NoProfile", "-Command"][..], "& "),
        ] {
            for word in HOSTILE {
                let command = format!("{call}{printf} '<%s>' x {} y", kind.quote(word).unwrap());
                let Some(got) = run(shell, flag, &command) else { break };
                assert_eq!(got, format!("<x><{word}><y>"), "{shell}: {command:?}");
            }
        }
    }

    #[test]
    fn nushell_chains_on_the_exit_status_of_an_external_program() {
        let ok = ShellKind::Nu.chain("^true", "echo second").unwrap();
        let Some(out) = run("nu", &["-c"], &ok) else { return };
        assert_eq!(out.trim(), "second", "{ok}");
        let bad = ShellKind::Nu.chain("^false", "echo second").unwrap();
        let out = Command::new("nu").args(["-c"]).arg(&bad).stdin(Stdio::null()).output().unwrap();
        assert!(!String::from_utf8_lossy(&out.stdout).contains("second"), "{bad}");
    }

    #[test]
    fn a_word_that_starts_with_a_dash_is_quoted_for_nushell_only() {
        assert_eq!(ShellKind::Nu.quote("--resume").unwrap(), "'--resume'");
        assert_eq!(ShellKind::Nu.quote("-n").unwrap(), "'-n'");
        assert_eq!(ShellKind::Nu.quote("a-b").unwrap(), "a-b");
        assert_eq!(ShellKind::Posix.quote("--resume").unwrap(), "--resume");
        assert_eq!(ShellKind::Fish.quote("--resume").unwrap(), "--resume");
    }
}
