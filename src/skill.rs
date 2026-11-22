//! The agent skill: `skills/imux/SKILL.md`, built into the binary (`imux skill` prints it), so the
//! copy an agent reads is the one that matches this imux. imux writes it only when the user runs
//! `imux setup` (see `setup.rs`; or `imux skill > FILE` by hand), and `imux doctor` says whether the
//! installed file is there and the same. The tests keep it true to the command tables.

use std::path::PathBuf;

/// The skill file, as shipped.
pub const TEXT: &str = include_str!("../skills/imux/SKILL.md");

/// Where an agent looks for it.
pub fn install_path() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".claude/skills/imux/SKILL.md"))
}

/// How the installed file compares with this imux's copy.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Missing,
    Same,
    Differs,
}

pub fn compare(installed: Option<&str>) -> Installed {
    match installed {
        None => Installed::Missing,
        Some(text) if text == TEXT => Installed::Same,
        Some(_) => Installed::Differs,
    }
}

/// The `skill` line of `imux doctor`.
pub fn doctor_line(state: &Installed, path: Option<&str>) -> (String, String) {
    let at = path.unwrap_or("~/.claude/skills/imux/SKILL.md");
    let text = match state {
        Installed::Same => format!("installed at {at}, the same as this imux"),
        Installed::Differs => format!("installed at {at} but different from this imux (imux skill > {at} brings it up to date)"),
        Installed::Missing => format!("not installed (agents learn to use imux from it: imux skill > {at})"),
    };
    ("skill".to_string(), text)
}

/// Read the installed copy and say how it compares; no file, or one that cannot be read, is missing.
pub fn doctor() -> (String, String) {
    let path = install_path();
    let text = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    doctor_line(&compare(text.as_deref()), path.as_ref().map(|p| p.to_string_lossy().into_owned()).as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli;

    /// The words of a shell line: whitespace separated, single and double quotes group.
    fn split(line: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut cur = String::new();
        let mut quote: Option<char> = None;
        let mut any = false;
        for c in line.chars() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), c) => cur.push(c),
                (None, '\'' | '"') => {
                    quote = Some(c);
                    any = true;
                }
                (None, c) if c.is_whitespace() => {
                    if any || !cur.is_empty() {
                        words.push(std::mem::take(&mut cur));
                        any = false;
                    }
                }
                (None, c) => cur.push(c),
            }
        }
        if any || !cur.is_empty() {
            words.push(cur);
        }
        words
    }

    /// The lines of the fenced blocks.
    fn code_lines() -> Vec<&'static str> {
        let mut inside = false;
        let mut out = Vec::new();
        for line in TEXT.lines() {
            if line.starts_with("```") {
                inside = !inside;
            } else if inside {
                out.push(line);
            }
        }
        out
    }

    fn long_flags(cmd: &clap::Command, out: &mut Vec<String>) {
        out.extend(cmd.get_arguments().filter_map(|a| a.get_long().map(str::to_string)));
        for sub in cmd.get_subcommands() {
            long_flags(sub, out);
        }
    }

    #[test]
    fn the_skill_is_small_and_has_its_front_matter() {
        assert!(TEXT.len() < 16 * 1024, "{} bytes", TEXT.len());
        assert!(TEXT.starts_with("---\nname: imux\ndescription: "));
        let end = TEXT[4..].find("\n---\n").expect("the front matter ends");
        let front = &TEXT[..4 + end];
        assert!(front.contains("IMUX_ENV"), "the description limits the use");
        assert!(!front.contains("herdr"), "no other product's name");
        assert!(!TEXT.to_lowercase().contains("herdr") && !TEXT.contains("tmux"), "the skill names no other multiplexer");
    }

    #[test]
    fn every_imux_line_of_a_code_block_is_a_command_imux_has() {
        let mut checked = 0;
        for line in code_lines() {
            let mut words = split(line);
            if words.first().map(String::as_str) != Some("imux") {
                continue;
            }
            words.remove(0);
            if matches!(words.first().map(String::as_str), Some("-L" | "-S")) {
                words.drain(..2.min(words.len()));
            }
            let Some(first) = words.first().cloned() else { panic!("{line}: no command") };
            let cmd = cli::tree();
            if !first.starts_with('-') && (cli::goes_to_server(&words) || cmd.find_subcommand(&first).is_none()) {
                assert!(crate::server::knows_command(&first), "{line}: {first} is not a command of imux");
            } else {
                if let Some(at) = cli::agent_ref_index(&words)
                    && !cli::is_pane_number(&words[at])
                    && words[at] != "--current"
                {
                    words[at] = "3".to_string();
                }
                let parsed = cmd.try_get_matches_from(std::iter::once("imux".to_string()).chain(words.iter().cloned()));
                if let Err(e) = parsed {
                    assert!(matches!(e.kind(), clap::error::ErrorKind::DisplayHelp), "{line}: {e}");
                }
            }
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} lines were checked");
    }

    #[test]
    fn every_flag_the_skill_names_exists() {
        let mut known = Vec::new();
        long_flags(&cli::tree(), &mut known);
        known.push("help".to_string());
        let mut named = 0;
        for line in TEXT.lines().filter(|l| !l.starts_with("---")) {
            let before_resume = line.split(" -- ").next().unwrap_or(line);
            for word in before_resume.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).filter(|w| w.starts_with("--") && w.len() > 2) {
                named += 1;
                assert!(known.iter().any(|k| k == &word[2..]), "{word} is not a flag of any imux command: {line}");
            }
        }
        assert!(named >= 15, "only {named} flags were checked");
    }

    #[test]
    fn the_error_codes_are_all_listed() {
        for (code, _) in crate::errcode::CODES {
            assert!(TEXT.contains(&format!("`{code}`")), "{code} is missing from the skill");
        }
    }

    #[test]
    fn the_installed_copy_is_compared_whole() {
        assert_eq!(compare(None), Installed::Missing);
        assert_eq!(compare(Some(TEXT)), Installed::Same);
        assert_eq!(compare(Some("older")), Installed::Differs);
        assert!(doctor_line(&Installed::Same, Some("/h/s")).1.contains("the same as this imux"));
        assert!(doctor_line(&Installed::Differs, Some("/h/s")).1.contains("imux skill > /h/s"));
        assert!(doctor_line(&Installed::Missing, None).1.contains("not installed"));
    }

    #[test]
    fn the_shipped_file_is_what_the_binary_prints() {
        let on_disk = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/skills/imux/SKILL.md")).unwrap();
        assert_eq!(on_disk, TEXT);
    }

    #[test]
    fn the_shell_line_splitter_keeps_quoted_words_whole() {
        assert_eq!(split("imux pane run 7 'npm test'"), ["imux", "pane", "run", "7", "npm test"]);
        assert_eq!(split(r#"imux split-window -c "$PWD" -P"#), ["imux", "split-window", "-c", "$PWD", "-P"]);
        assert_eq!(split("a '' b"), ["a", "", "b"]);
    }
}
