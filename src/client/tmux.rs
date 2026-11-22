//! The `tmux` name inside imux panes (`tmux-compat`): the imux executable run through the link
//! `<socket dir>/shims/tmux` takes tmux's command line and runs those commands on the server of the
//! pane it was started in. It never starts a server and never reads a configuration; a program
//! that wants imux's own command line calls `imux`. The words are read the way tmux reads them:
//! global flags first, flag values may be attached (`-w80%`), `;` separates commands.

use std::io::{self, Write};

use crate::log;
use crate::session::popup::ENDED_WITH;

/// What `tmux -V` prints. Tools compare it with the version a feature came in (a popup needs
/// 3.2), so it names a tmux release whose commands imux covers; the number is not imux's own.
pub const VERSION_LINE: &str = "tmux 3.4";

/// Whether the program was started under the name `tmux`.
pub fn is_shim(argv0: &str) -> bool {
    std::path::Path::new(argv0).file_name().is_some_and(|n| n == "tmux")
}

/// What a command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Version,
    Commands(Vec<Vec<String>>),
}

/// Global flags that take a value (`-f file`, `-L name`, `-S path`, `-T features`, `-c command`)
/// and the ones that do not (`-2 -D -l -N -u -v`); both are read and ignored here (`-L` and `-S`
/// are acted on by `run`, through `socket_flags`). `-C` (control mode) is refused.
pub fn parse(args: &[String]) -> Result<Request, String> {
    let mut at = 0;
    while let Some(arg) = args.get(at).filter(|a| a.len() > 1 && a.starts_with('-')) {
        at += 1;
        let letters = &arg[1..];
        for (i, c) in letters.char_indices() {
            match c {
                'V' => return Ok(Request::Version),
                'C' => return Err("control mode is not supported".to_string()),
                '2' | 'D' | 'l' | 'N' | 'u' | 'v' => {}
                'f' | 'L' | 'S' | 'T' | 'c' => {
                    if letters[i + 1..].is_empty() {
                        at += 1;
                    }
                    break;
                }
                _ => return Err(format!("unknown option -- {c}")),
            }
        }
    }
    let commands = split(args.get(at..).unwrap_or_default());
    if commands.is_empty() {
        return Err("imux does not start a session through tmux: use imux".to_string());
    }
    Ok(Request::Commands(commands))
}

/// The `-L name` and `-S path` among the global flags, the same way `parse` walks them.
pub fn socket_flags(args: &[String]) -> (Option<String>, Option<String>) {
    let (mut name, mut path) = (None, None);
    let mut at = 0;
    while let Some(arg) = args.get(at).filter(|a| a.len() > 1 && a.starts_with('-')) {
        at += 1;
        let letters = &arg[1..];
        for (i, c) in letters.char_indices() {
            match c {
                'f' | 'L' | 'S' | 'T' | 'c' => {
                    let attached = &letters[i + 1..];
                    let value = if attached.is_empty() {
                        at += 1;
                        args.get(at - 1).cloned()
                    } else {
                        Some(attached.to_string())
                    };
                    match c {
                        'L' => name = value,
                        'S' => path = value,
                        _ => {}
                    }
                    break;
                }
                'V' | 'C' => return (name, path),
                _ => {}
            }
        }
    }
    (name, path)
}

/// Cut a command line at its `;` words. A word that ends in `\;` keeps a literal `;`, a word that
/// ends in a bare `;` ends the command.
fn split(args: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for arg in args {
        if arg == ";" {
            out.push(std::mem::take(&mut current));
        } else if let Some(word) = arg.strip_suffix("\\;") {
            current.push(format!("{word};"));
        } else if let Some(word) = arg.strip_suffix(';') {
            current.push(word.to_string());
            out.push(std::mem::take(&mut current));
        } else {
            current.push(arg.clone());
        }
    }
    out.push(current);
    out.retain(|c| !c.is_empty());
    out
}

/// The status a popup's program ended with, from the server's answer to a waiting
/// `display-popup`; `tmux display-popup` ends with it, the way the program it ran did.
pub fn popup_status(message: &str) -> Option<i32> {
    message.strip_prefix(ENDED_WITH)?.trim().parse().ok()
}

/// `list-commands [name]`: one line per command, `name` alone, or an error for an unknown name.
fn list_commands(args: &[String]) -> Result<String, String> {
    let names = crate::completions::tmux_commands();
    match args.first() {
        Some(name) if names.contains(&name.as_str()) => Ok(format!("{name}\n")),
        Some(name) => Err(format!("unknown command: {name}")),
        None => Ok(names.iter().map(|n| format!("{n}\n")).collect()),
    }
}

/// Run what the command line says. Returns the exit status: 0, 1 for an error (its text on the
/// standard error), or the status of the program of a `display-popup`.
pub fn run(args: &[String]) -> io::Result<i32> {
    let (name, path) = socket_flags(args);
    if let Err(e) = crate::paths::choose_socket(name.as_deref(), path.as_deref()) {
        let _ = writeln!(io::stderr(), "{e}");
        return Ok(1);
    }
    let commands = match parse(args) {
        Ok(Request::Version) => {
            let _ = writeln!(io::stdout(), "{VERSION_LINE}");
            return Ok(0);
        }
        Ok(Request::Commands(c)) => c,
        Err(e) => {
            let _ = writeln!(io::stderr(), "{e}");
            return Ok(1);
        }
    };
    for words in commands {
        let name = words[0].as_str();
        if matches!(name, "list-commands" | "lscm") {
            match list_commands(&words[1..]) {
                Ok(text) => {
                    let _ = io::stdout().write_all(text.as_bytes());
                    continue;
                }
                Err(e) => {
                    let _ = writeln!(io::stderr(), "{e}");
                    return Ok(1);
                }
            }
        }
        if matches!(name, "start-server" | "start") && words.len() == 1 {
            // The server is running when a command reaches it: there is nothing to start. Plugin
            // managers write `tmux start-server \; show-environment ...`.
            continue;
        }
        match super::command(words.clone()) {
            Ok(0) => {}
            Ok(code) => return Ok(code),
            Err(e) => {
                let text = e.to_string();
                if matches!(name, "display-popup" | "popup")
                    && let Some(code) = popup_status(&text)
                {
                    return Ok(code);
                }
                log::debug(&format!("tmux shim: {name}: {text}"));
                let _ = writeln!(io::stderr(), "{text}");
                return Ok(1);
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_socket_flags_are_found_among_the_global_ones() {
        assert_eq!(socket_flags(&words("-L work ls")), (Some("work".into()), None));
        assert_eq!(socket_flags(&words("-u -Lwork -S /p/s list-sessions")), (Some("work".into()), Some("/p/s".into())));
        assert_eq!(socket_flags(&words("-f /x/conf new-window -L nope")), (None, None));
        assert_eq!(socket_flags(&words("list-sessions")), (None, None));
        assert_eq!(socket_flags(&words("-V -L work")), (None, None));
    }

    #[test]
    fn the_name_tmux_makes_the_shim() {
        assert!(is_shim("tmux"));
        assert!(is_shim("/tmp/imux-501/shims/tmux"));
        assert!(!is_shim("imux"));
        assert!(!is_shim("/usr/bin/tmux-foo"));
    }

    #[test]
    fn global_flags_come_before_the_command_and_are_read() {
        assert_eq!(parse(&words("-V")), Ok(Request::Version));
        assert_eq!(parse(&words("-u -2 -L x display-message -p hi")), Ok(Request::Commands(vec![words("display-message -p hi")])));
        assert_eq!(parse(&words("-Lx -f /dev/null ls")), Ok(Request::Commands(vec![words("ls")])));
        assert!(parse(&words("-C attach")).is_err(), "control mode");
        assert!(parse(&words("-Z ls")).is_err(), "an unknown flag");
        assert!(parse(&[]).is_err(), "a bare tmux would start a session");
    }

    #[test]
    fn a_semicolon_word_separates_commands() {
        let line: Vec<String> = ["display-message", "-p", "x", ";", "list-commands", "new-pane"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse(&line), Ok(Request::Commands(vec![words("display-message -p x"), words("list-commands new-pane")])));
        let end: Vec<String> = ["a", "b;", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(split(&end), vec![words("a b"), words("c")]);
        let literal: Vec<String> = ["a", "b\\;", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(split(&literal), vec![vec!["a".to_string(), "b;".to_string(), "c".to_string()]]);
    }

    #[test]
    fn flag_values_stay_attached_in_the_command() {
        let line = words("display-popup -E -d /tmp -xC -yC -w80% -h50% sh /tmp/f");
        assert_eq!(parse(&line), Ok(Request::Commands(vec![line])));
    }

    #[test]
    fn the_status_of_a_popup_program_comes_from_the_answer() {
        assert_eq!(popup_status(&format!("{ENDED_WITH}130")), Some(130));
        assert_eq!(popup_status("display-popup: no client is attached"), None);
    }

    #[test]
    fn list_commands_knows_the_popup_and_not_the_floating_pane() {
        assert!(list_commands(&words("display-popup")).is_ok());
        assert!(list_commands(&words("new-pane")).is_err());
        let all = list_commands(&[]).unwrap();
        assert!(all.contains("display-popup\n") && !all.contains("new-pane"));
    }
}
