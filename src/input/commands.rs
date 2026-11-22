//! Parsing of one command (a list of words, see `config::lex`) typed at the `:` prompt or bound to
//! a key into a `Command`. Only the commands imux implements are accepted.

use super::bindings::{AskSpec, BufferOp, Command, MenuItemSpec, MenuSpec, PopupSpec, ShellIf, ShellRun, SourceSpec, WindowOptOp};
use crate::proto::{AgentWaitArgs, PersistOp, PromptArgs, ReportArgs, WorktreeArgs, WorktreeRemoveArgs};
#[cfg(test)]
use crate::proto::{UNTIL_BLOCKED, UNTIL_DONE};
use crate::session::layout::{Direction, Preset, Split};

fn direction(flag: &str) -> Option<Direction> {
    match flag {
        "-L" => Some(Direction::Left),
        "-R" => Some(Direction::Right),
        "-U" => Some(Direction::Up),
        "-D" => Some(Direction::Down),
        _ => None,
    }
}

/// A first word that names a repository rather than a worktree: a path, or a directory with `.git`.
fn looks_like_repo(word: &str) -> bool {
    let path_like = word.starts_with(['/', '~']) || word.starts_with("./") || word.starts_with("../");
    path_like || std::path::Path::new(word).join(".git").exists()
}

/// `[--detach|--branch] [--start text] [--setup text|--no-setup] [repo] name [base]`; the repo
/// is the first word when it looks like a path or is a directory holding `.git`.
pub fn worktree_args(args: &[String]) -> Result<WorktreeArgs, String> {
    const USAGE: &str =
        "usage: worktree [--detach|--branch] [--start text] [--setup text|--no-setup] [--slug text] [repo] name [base]";
    let mut out = WorktreeArgs::default();
    let mut words: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--detach" => out.detach = Some(true),
            "--branch" => out.detach = Some(false),
            "--start" => out.start = Some(it.next().ok_or(USAGE)?.clone()),
            "--slug" => out.slug = Some(it.next().ok_or(USAGE)?.clone()),
            "--setup" => out.setup = Some(it.next().ok_or(USAGE)?.clone()),
            "--no-setup" => out.setup = Some(String::new()),
            flag if flag.starts_with("--") => return Err(format!("worktree: unknown flag {flag}")),
            _ => words.push(a),
        }
    }
    let (repo, rest) = match words.split_first() {
        Some((first, rest)) if !rest.is_empty() && looks_like_repo(first) => (Some((*first).clone()), rest),
        _ => (None, &words[..]),
    };
    out.repo = repo;
    match rest {
        [name] if name.starts_with(['/', '~']) || name.starts_with("./") || name.starts_with("../") => {
            return Err("worktree: a repository needs a name after it".to_string());
        }
        [name] => out.name = (*name).clone(),
        [name, base] => {
            out.name = (*name).clone();
            out.base = Some((*base).clone());
        }
        _ => return Err(USAGE.to_string()),
    }
    Ok(out)
}

/// `[--force] [repo] [name]`: with two words the first is the repository, which has to look like
/// one; with no name the worktree of the current window (or directory) is meant.
pub fn remove_args(args: &[String]) -> Result<WorktreeRemoveArgs, String> {
    const USAGE: &str = "usage: worktree-remove [--force] [repo] [name]";
    let mut out = WorktreeRemoveArgs::default();
    let mut words: Vec<&String> = Vec::new();
    for a in args {
        match a.as_str() {
            "--force" => out.force = true,
            flag if flag.starts_with('-') => return Err(format!("worktree-remove: unknown flag {flag}")),
            _ => words.push(a),
        }
    }
    match words.as_slice() {
        [] => {}
        [name] => out.name = Some((*name).clone()),
        [repo, name] if looks_like_repo(repo) => {
            out.repo = Some((*repo).clone());
            out.name = Some((*name).clone());
        }
        _ => return Err(USAGE.to_string()),
    }
    Ok(out)
}

/// The leading flags of `run-shell` / `if-shell` (`known` are the ones that exist) and the words
/// after them. A flag tmux has but imux does not is refused by name.
fn shell_flags<'a>(name: &str, args: &'a [String], known: &str) -> Result<(Vec<char>, &'a [String]), String> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(arg) = args.get(at).filter(|a| a.len() > 1 && a.starts_with('-')) {
        at += 1;
        if arg == "--" {
            break;
        }
        for c in arg.chars().skip(1) {
            if !known.contains(c) {
                let why = if matches!(c, 'd' | 't') { " (imux has no delay or target; the current pane is used)" } else { "" };
                return Err(format!("{name}: unsupported flag -{c}{why}"));
            }
            found.push(c);
        }
    }
    Ok((found, &args[at..]))
}

/// `[-OM] [-b lines] [-C n|-] [-H style] [-s style] [-S style] [-t target] [-T title] [-x pos]
/// [-y pos] name key command ...`; an empty name is a separator and stands alone. Flag values may
/// be attached (`-xC`); `--` ends the flags, which a name that starts with a dash (a disabled
/// item) needs.
pub fn display_menu_args(name: &str, args: &[String]) -> Result<MenuSpec, String> {
    const USAGE: &str = "usage: display-menu [-OM] [-b lines] [-C choice] [-H style] [-s style] [-S style] [-t target] [-T title] [-x position] [-y position] name key command ...";
    let mut out = MenuSpec::default();
    let mut at = 0;
    while let Some(arg) = args.get(at).filter(|a| a.len() > 1 && a.starts_with('-')) {
        at += 1;
        if arg == "--" {
            break;
        }
        let letters = &arg[1..];
        for (i, c) in letters.char_indices() {
            match c {
                'O' => out.stay = true,
                'M' => out.mouse = true,
                'b' | 'C' | 'H' | 's' | 'S' | 't' | 'T' | 'x' | 'y' => {
                    let attached = &letters[i + 1..];
                    let value = if attached.is_empty() {
                        let v = args.get(at).ok_or_else(|| format!("{name}: -{c} needs a value"))?;
                        at += 1;
                        v.clone()
                    } else {
                        attached.to_string()
                    };
                    match c {
                        'b' => {
                            if !crate::config::options::BORDER_LINES.contains(&value.as_str()) {
                                return Err(format!("{name}: -b is one of {}", crate::config::options::BORDER_LINES.join(", ")));
                            }
                            out.lines = Some(value);
                        }
                        'C' => {
                            if value != "-" && value.parse::<usize>().is_err() {
                                return Err(format!("{name}: -C is a number or -"));
                            }
                            out.start = Some(value);
                        }
                        'H' | 's' | 'S' => {
                            crate::config::options::parse_style(&value).map_err(|e| format!("{name}: -{c}: {e}"))?;
                            match c {
                                'H' => out.border_style = Some(value),
                                's' => out.style = Some(value),
                                _ => out.selected_style = Some(value),
                            }
                        }
                        't' => out.target = Some(value),
                        'T' => out.title = value,
                        'x' => out.x = Some(value),
                        _ => out.y = Some(value),
                    }
                    break;
                }
                _ => return Err(format!("{name}: unknown flag -{c}")),
            }
        }
    }
    let mut words = args[at..].iter();
    while let Some(item) = words.next() {
        if item.is_empty() {
            out.items.push(MenuItemSpec { name: String::new(), key: String::new(), command: String::new() });
            continue;
        }
        let (Some(key), Some(command)) = (words.next(), words.next()) else {
            return Err(format!("{name}: not enough arguments: every item is a name, a key and a command"));
        };
        out.items.push(MenuItemSpec { name: item.clone(), key: key.clone(), command: command.clone() });
    }
    if out.items.is_empty() {
        return Err(USAGE.to_string());
    }
    Ok(out)
}

/// `[-BCEkN] [-b lines] [-c client] [-d dir] [-e NAME=value] [-h size] [-s style] [-S style]
/// [-t target] [-T title] [-w size] [-x position] [-y position] [command [argument ...]]`. `-E` may
/// be repeated and `-N` takes the `-E` and `-k` before it back. Flag values may be attached
/// (`-xC`); the first word that is not a flag, or `--`, starts the program.
pub fn display_popup_args(name: &str, args: &[String]) -> Result<PopupSpec, String> {
    let mut out = PopupSpec::default();
    let mut at = 0;
    while let Some(arg) = args.get(at).filter(|a| a.len() > 1 && a.starts_with('-')) {
        at += 1;
        if arg == "--" {
            break;
        }
        let letters = &arg[1..];
        for (i, c) in letters.char_indices() {
            match c {
                'B' => out.no_border = true,
                'C' => out.close = true,
                'E' => out.exit_flags = out.exit_flags.saturating_add(1),
                'k' => out.any_key = true,
                'N' => {
                    out.exit_flags = 0;
                    out.any_key = false;
                }
                'b' | 'c' | 'd' | 'e' | 'h' | 's' | 'S' | 't' | 'T' | 'w' | 'x' | 'y' => {
                    let attached = &letters[i + 1..];
                    let value = if attached.is_empty() {
                        let v = args.get(at).ok_or_else(|| format!("{name}: -{c} needs a value"))?;
                        at += 1;
                        v.clone()
                    } else {
                        attached.to_string()
                    };
                    match c {
                        'b' => {
                            if !crate::config::options::BORDER_LINES.contains(&value.as_str()) {
                                return Err(format!("{name}: -b is one of {}", crate::config::options::BORDER_LINES.join(", ")));
                            }
                            out.lines = Some(value);
                        }
                        'c' => {}
                        'd' => out.dir = Some(value),
                        'e' => {
                            if !value.contains('=') || value.starts_with('=') {
                                return Err(format!("{name}: -e is NAME=value"));
                            }
                            out.env.push(value);
                        }
                        'h' | 'w' => {
                            crate::session::popup::size_spec(Some(&value), 100).map_err(|e| format!("{name}: -{c}: {e}"))?;
                            if c == 'h' {
                                out.height = Some(value);
                            } else {
                                out.width = Some(value);
                            }
                        }
                        's' | 'S' => {
                            crate::config::options::parse_style(&value).map_err(|e| format!("{name}: -{c}: {e}"))?;
                            if c == 's' {
                                out.style = Some(value);
                            } else {
                                out.border_style = Some(value);
                            }
                        }
                        't' => out.target = Some(value),
                        'T' => out.title = value,
                        'x' => out.x = Some(value),
                        _ => out.y = Some(value),
                    }
                    break;
                }
                _ => return Err(format!("{name}: unknown flag -{c}")),
            }
        }
    }
    out.command = args[at..].to_vec();
    if out.command.len() == 1 && out.command[0].is_empty() {
        out.command.clear();
    }
    Ok(out)
}

/// `[-b] [-C] text`.
pub fn run_shell_args(args: &[String]) -> Result<ShellRun, String> {
    let (flags, rest) = shell_flags("run-shell", args, "bC")?;
    match rest {
        [text] => Ok(ShellRun { background: flags.contains(&'b'), imux: flags.contains(&'C'), text: text.clone() }),
        _ => Err("usage: run-shell [-bC] command".to_string()),
    }
}

/// `[-F] condition then [else]`.
pub fn if_shell_args(args: &[String]) -> Result<ShellIf, String> {
    let (flags, rest) = shell_flags("if-shell", args, "bF")?;
    let (background, format) = (flags.contains(&'b'), flags.contains(&'F'));
    match rest {
        [condition, then] => Ok(ShellIf { background, format, condition: condition.clone(), then: then.clone(), otherwise: None }),
        [condition, then, otherwise] => Ok(ShellIf {
            background,
            format,
            condition: condition.clone(),
            then: then.clone(),
            otherwise: Some(otherwise.clone()),
        }),
        _ => Err("usage: if-shell [-F] condition command [command]".to_string()),
    }
}

/// `[pane] --source s --agent a --state st [--seq n] [--message m] [-- command...]`; without a
/// pane `default_pane` (`$IMUX_PANE_ID`) is used and without `--seq` `now`.
pub fn report_args(args: &[String], default_pane: Option<u32>, now: u64) -> Result<ReportArgs, String> {
    crate::cli::report_from(&crate::cli::parse_leaf(crate::cli::report_command(), args)?, default_pane, now)
}

/// `[pane] --source s` for `release-agent`.
pub fn release_args(args: &[String], default_pane: Option<u32>) -> Result<(u32, String), String> {
    crate::cli::release_from(&crate::cli::parse_leaf(crate::cli::release_command(), args)?, default_pane)
}

/// `pane [--until idle,done,...] [--timeout seconds]` for `agent wait`.
pub fn wait_args(args: &[String]) -> Result<AgentWaitArgs, String> {
    crate::cli::wait_from(&crate::cli::parse_leaf(crate::cli::wait_command(), args)?)
}

/// `pane [--wait] [--timeout seconds] [--] text...`: the words after the pane are the text, joined
/// by spaces; the text `-` stands for what the command reads on its standard input.
pub fn prompt_args(args: &[String]) -> Result<PromptArgs, String> {
    crate::cli::prompt_from(&crate::cli::parse_leaf(crate::cli::prompt_command(), args)?)
}

/// `[--types name,name...]`: the event types to follow; none means all but the `after-` ones.
pub fn events_args(args: &[String]) -> Result<Vec<String>, String> {
    Ok(crate::cli::events_from(&crate::cli::parse_leaf(crate::cli::events_command(), args)?).types)
}

/// Flags and words of a buffer command. `allowed` lists the flag letters the command takes:
/// `b` (buffer name) and `s` (separator) with a value, `a`, `d` and `r` without.
#[derive(Default)]
struct BufferFlags {
    name: Option<String>,
    sep: Option<String>,
    append: bool,
    delete: bool,
    raw: bool,
    words: Vec<String>,
}

fn buffer_flags(name: &str, args: &[String], allowed: &str) -> Result<BufferFlags, String> {
    let mut out = BufferFlags::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let letter = match arg.as_bytes() {
            [b'-', c] if allowed.contains(*c as char) => *c,
            [b'-', b'-'] => {
                out.words.extend(it.by_ref().cloned());
                break;
            }
            _ if arg.starts_with('-') && arg.len() > 1 => return Err(format!("{name}: unknown flag {arg}")),
            _ => {
                out.words.push(arg.clone());
                continue;
            }
        };
        match letter {
            b'b' => out.name = Some(it.next().ok_or_else(|| format!("{name}: -b needs a buffer name"))?.clone()),
            b's' => out.sep = Some(it.next().ok_or_else(|| format!("{name}: -s needs a separator"))?.clone()),
            b'a' => out.append = true,
            b'd' => out.delete = true,
            // `paste-buffer -p`: a paste is always bracketed when the pane asked for it.
            b'p' => {}
            _ => out.raw = true,
        }
    }
    Ok(out)
}

fn buffer_command(name: &str, args: &[String]) -> Result<Command, String> {
    let take = |allowed: &str, words: usize| -> Result<BufferFlags, String> {
        let f = buffer_flags(name, args, allowed)?;
        if f.words.len() == words { Ok(f) } else { Err(format!("usage: {name} {}", if words == 0 { "" } else { "[flags] data" })) }
    };
    let op = match name {
        "list-buffers" | "lsb" => {
            take("", 0)?;
            BufferOp::List
        }
        "choose-buffer" => {
            take("", 0)?;
            BufferOp::Choose
        }
        "delete-buffer" | "deleteb" => BufferOp::Delete(take("b", 0)?.name),
        "show-buffer" | "showb" => BufferOp::Show(take("b", 0)?.name),
        "set-buffer" | "setb" => {
            let f = take("ab", 1)?;
            BufferOp::Set { name: f.name, append: f.append, data: f.words.into_iter().next().unwrap_or_default() }
        }
        "paste-buffer" | "pasteb" => {
            let f = take("bdrsp", 0)?;
            if f.name.is_none() && !f.delete && !f.raw && f.sep.is_none() {
                return Ok(Command::PasteBuffer);
            }
            BufferOp::Paste { name: f.name, delete: f.delete, raw: f.raw, sep: f.sep }
        }
        "save-buffer" | "saveb" => {
            let f = take("ab", 1)?;
            BufferOp::Save { name: f.name, append: f.append, path: f.words.into_iter().next().unwrap_or_default() }
        }
        _ => {
            let f = take("b", 1)?;
            BufferOp::Load { name: f.name, path: f.words.into_iter().next().unwrap_or_default() }
        }
    };
    Ok(Command::Buffer(op))
}

/// `command-prompt [-1] [-I initial] [-p prompt] [template]` and `confirm-before [-p prompt]
/// command`. One prompt only: a comma list of prompts is refused. Without a template, what is typed
/// runs as it is.
fn ask_args(name: &str, args: &[String], confirm: bool) -> Result<AskSpec, String> {
    let mut spec = AskSpec { confirm, ..AskSpec::default() };
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-1" if !confirm => spec.single = true,
            "-I" | "-p" if !(confirm && a == "-I") => {
                let v = it.next().ok_or_else(|| format!("{name}: {a} needs a value"))?;
                if a == "-I" {
                    spec.initial = Some(v.clone());
                } else {
                    spec.label = Some(v.clone());
                }
            }
            "-T" if !confirm => {
                // The kind of completion (command, search, target, window-target): none is offered.
                it.next().ok_or_else(|| format!("{name}: {a} needs a value"))?;
            }
            w if w.starts_with('-') && w.len() > 1 && words.is_empty() => return Err(format!("{name}: unknown flag {w}")),
            _ => words.push(a.clone()),
        }
    }
    if words.len() > 1 {
        return Err(format!("usage: {name} [flags] {}", if confirm { "command" } else { "[template]" }));
    }
    match (words.pop(), confirm) {
        (Some(t), _) => spec.template = t,
        (None, false) => spec.template = "%%".to_string(),
        (None, true) => return Err(format!("usage: {name} [-p prompt] command")),
    }
    Ok(spec)
}

fn no_args(name: &str, args: &[String], command: Command) -> Result<Command, String> {
    if args.is_empty() { Ok(command) } else { Err(format!("{name}: unexpected argument {}", args[0])) }
}

/// `next-window` and `previous-window`: no arguments, or `-a` for the next window with an alert.
fn alert_flag(name: &str, args: &[String], plain: Command, alert: Command) -> Result<Command, String> {
    match args {
        [flag] if flag == "-a" => Ok(alert),
        _ => no_args(name, args, plain),
    }
}

/// One command line; `Ok(None)` when it is empty.
#[cfg(test)]
pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let mut statements = crate::config::lex::statements(line, &mut Default::default())?;
    match statements.len() {
        0 => Ok(None),
        1 => parse_words(&statements.remove(0)).map(Some),
        _ => Err("one command at a time".to_string()),
    }
}

/// `split-window` and `new-window` flags: `-c dir` is accepted because a new pane always starts in
/// the directory of the active one.
fn window_flags(name: &str, args: &[String]) -> Result<Option<Split>, String> {
    let mut kind = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" => kind = Some(Split::SideBySide),
            "-v" => kind = Some(Split::Stacked),
            "-c" => {
                it.next().ok_or_else(|| format!("{name}: -c needs a directory"))?;
            }
            other => return Err(format!("{name}: unknown flag {other}")),
        }
    }
    Ok(kind)
}

/// `swap-window [-d] -t -N|+N` (no `-s`): the offset and whether to select, `None` for any other form.
fn relative_swap(args: &[String]) -> Option<(i32, bool)> {
    let (mut by, mut select) = (None, true);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-d" => select = false,
            "-t" => {
                let t = it.next()?;
                if !(t.starts_with('-') || t.starts_with('+')) {
                    return None;
                }
                by = Some(t.parse::<i32>().ok().filter(|n| *n != 0 && n.abs() <= 1000)?);
            }
            _ => return None,
        }
    }
    by.map(|b| (b, select))
}

fn window_number(name: &str, target: &str) -> Result<usize, String> {
    target.trim_start_matches(':').parse().map_err(|_| format!("{name}: bad window {target}"))
}

/// `swap-window` and `move-window` flags: `-s src`, `-t dst`, `-d` (do not select). `-r` is not
/// needed because windows are always numbered contiguously.
fn reorder_flags(name: &str, args: &[String]) -> Result<(Option<usize>, usize, bool), String> {
    let (mut src, mut dst, mut select) = (None, None, true);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-d" => select = false,
            "-s" => src = Some(window_number(name, it.next().ok_or_else(|| format!("{name}: -s needs a window"))?)?),
            "-t" => dst = Some(window_number(name, it.next().ok_or_else(|| format!("{name}: -t needs a window"))?)?),
            other => return Err(format!("{name}: unknown flag {other}")),
        }
    }
    let dst = dst.ok_or_else(|| format!("usage: {name} [-d] [-s number] -t number"))?;
    Ok((src, dst, select))
}

/// `setw [-u] [-t target] name [on|off]` for a window option (`session::WINDOW_OPTIONS`): the
/// window gets a value of its own, or with no value the opposite of what it has, or with `-u` it
/// follows the global option again. `None` for anything else, which includes `-g` (that is the
/// global option, set the ordinary way) and `-s`.
pub fn window_option(args: &[String]) -> Option<Result<Command, String>> {
    let (mut target, mut unset, mut i, mut flaw) = (None, false, 0, None);
    while let Some(arg) = args.get(i).filter(|a| a.len() > 1 && a.starts_with('-')) {
        for c in arg.chars().skip(1) {
            match c {
                'g' | 's' => return None,
                't' => match args.get(i + 1) {
                    Some(t) => {
                        target = Some(t.clone());
                        i += 1;
                    }
                    None => flaw = Some("-t needs a target".to_string()),
                },
                'u' => unset = true,
                'q' | 'w' | 'p' | 'o' => {}
                other => flaw = Some(format!("unknown flag -{other}")),
            }
        }
        i += 1;
    }
    let rest = &args[i.min(args.len())..];
    let name = rest.first().filter(|n| crate::session::WINDOW_OPTIONS.contains(&n.as_str()))?.clone();
    if let Some(flaw) = flaw {
        return Some(Err(format!("set-window-option: {flaw}")));
    }
    let op = match (rest.get(1), rest.len(), unset) {
        (None, 1, true) => WindowOptOp::Unset,
        (None, 1, false) => WindowOptOp::Toggle,
        (Some(value), 2, false) => match crate::config::options::parse_bool(value) {
            Some(on) => WindowOptOp::Set(on),
            None => return Some(Err(format!("{name}: expected on or off, not {value}"))),
        },
        _ => return Some(Err("usage: set-window-option [-u] [-t target] option [on|off]".to_string())),
    };
    Some(Ok(Command::SetWindowOption { name, op, target }))
}

/// Whether `name` is a command name or alias imux has, exactly.
pub fn is_exact(name: &str) -> bool {
    crate::server::knows_exact(name) || !typed_words(&[name.to_string()]).is_err_and(|e| e.starts_with("unknown command"))
}

/// A command name that is not one imux has, but is the start of exactly one long name (tmux
/// accepts such prefixes: `show-option` for `show-options`). `Ok(None)` for a name imux has, or
/// one that is no prefix of anything; an error when several names start with it.
pub fn resolve_prefix(name: &str) -> Result<Option<&'static str>, String> {
    if name.len() < 3 || !name.starts_with(|c: char| c.is_ascii_alphabetic()) || is_exact(name) {
        return Ok(None);
    }
    let hits: Vec<&'static str> = crate::completions::tmux_commands().iter().copied().filter(|c| c.starts_with(name)).collect();
    match hits.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => Err(format!("ambiguous command: {name}, could be {}", many.join(", "))),
    }
}

/// Words of one command: its name, then its arguments. A command the typed form does not take
/// (`send-keys`, or `new-window -n name`) is still a command when the server can run it as
/// `imux <command>` does: it then keeps its words and its flags are checked when it runs.
pub fn parse_words(words: &[String]) -> Result<Command, String> {
    let resolved;
    let words = match words.first().map(|w| resolve_prefix(w)) {
        Some(Ok(Some(full))) => {
            resolved = std::iter::once(full.to_string()).chain(words[1..].iter().cloned()).collect::<Vec<_>>();
            &resolved[..]
        }
        Some(Err(e)) => return Err(e),
        _ => words,
    };
    parse_words_exact(words)
}

/// `parse_words` for the names as given: no unique-start guessing. A command alias is looked up
/// before a name is read as the start of another, so the alias check asks this one.
pub fn parse_words_exact(words: &[String]) -> Result<Command, String> {
    let typed = typed_words(words);
    let (Err(e), Some((name, args))) = (&typed, words.split_first()) else { return typed };
    let not_typed = e.starts_with("unknown command") || e.contains("only a window option of one window");
    if (not_typed && crate::server::runs_from_a_key(name)) || crate::server::bound_flags_fit(name, args) {
        return Ok(Command::Server(words.to_vec()));
    }
    typed
}

/// The words after `source-file`: flags `-q -n -v` (alone or together, before the path) and one path.
fn source_spec(name: &str, args: &[String]) -> Result<SourceSpec, String> {
    let usage = || format!("usage: {name} [-nqv] path");
    let mut spec = SourceSpec::default();
    let mut rest = args;
    while let Some((flag, tail)) = rest.split_first().filter(|(f, _)| f.len() > 1 && f.starts_with('-')) {
        for c in flag.chars().skip(1) {
            match c {
                'q' => spec.quiet = true,
                'n' => spec.parse_only = true,
                'v' => spec.verbose = true,
                _ => return Err(usage()),
            }
        }
        rest = tail;
    }
    match rest {
        [path] => spec.path.clone_from(path),
        _ => return Err(usage()),
    }
    Ok(spec)
}

fn typed_words(words: &[String]) -> Result<Command, String> {
    let Some((name, args)) = words.split_first() else { return Err("empty command".to_string()) };
    let command = match name.as_str() {
        "source-file" | "source" => match args {
            [path] if !path.starts_with('-') && !path.contains(['*', '?']) => Command::SourceFile(path.clone()),
            _ => Command::SourceWith(source_spec(name, args)?),
        },
        "display-message" | "display" => match args {
            [flag, text] if flag.starts_with('-') => Command::DisplayMessage(text.clone()),
            [text] => Command::DisplayMessage(text.clone()),
            _ => return Err(format!("usage: {name} text")),
        },
        "save-session" => match args {
            [] => Command::Persist(PersistOp::Save(None)),
            [snapshot] => Command::Persist(PersistOp::Save(Some(snapshot.clone()))),
            _ => return Err(format!("usage: {name} [name]")),
        },
        "restore-session" => match args {
            [] => Command::Persist(PersistOp::Restore(None)),
            [snapshot] => Command::Persist(PersistOp::Restore(Some(snapshot.clone()))),
            _ => return Err(format!("usage: {name} [name]")),
        },
        "list-snapshots" => no_args(name, args, Command::Persist(PersistOp::List))?,
        "delete-snapshot" => match args {
            [snapshot] => Command::Persist(PersistOp::Delete(snapshot.clone())),
            _ => return Err(format!("usage: {name} name")),
        },
        "show-hooks" => match args {
            [] => Command::ShowHooks,
            [flag] if flag == "-g" => Command::ShowHooks,
            _ => return Err(format!("usage: {name} [-g]")),
        },
        "set-window-option" | "setw" | "set-option" | "set" => match window_option(args) {
            Some(parsed) => parsed?,
            None => return Err(format!("{name}: only a window option of one window can be bound or typed here; set the others in the configuration")),
        },
        "display-menu" | "menu" => Command::DisplayMenu(display_menu_args(name, args)?),
        "display-popup" | "popup" => Command::DisplayPopup(display_popup_args(name, args)?),
        "run-shell" | "run" => Command::RunShell(run_shell_args(args)?),
        "if-shell" | "if" => Command::IfShell(if_shell_args(args)?),
        "worktree" => Command::Worktree(worktree_args(args)?),
        "worktree-remove" => Command::WorktreeRemove(remove_args(args)?),
        "repo-wizard" => no_args(name, args, Command::RepoWizard)?,
        "agent-panel" => no_args(name, args, Command::AgentPanel)?,
        "send-prefix" => match args {
            [flag] if flag == "-2" => Command::SendPrefix2,
            _ => no_args(name, args, Command::SendPrefix)?,
        },
        "show-options" | "show" => no_args(name, args, Command::ShowOptions)?,
        "new-window" | "neww" => {
            window_flags(name, args)?;
            Command::NewWindow
        }
        "kill-window" | "killw" => no_args(name, args, Command::KillWindow)?,
        "kill-pane" | "killp" => no_args(name, args, Command::KillPane)?,
        "next-window" | "next" => alert_flag(name, args, Command::NextWindow, Command::NextAlertWindow)?,
        "previous-window" | "prev" => alert_flag(name, args, Command::PreviousWindow, Command::PreviousAlertWindow)?,
        "last-window" | "last" => no_args(name, args, Command::LastWindow)?,
        "last-pane" | "lastp" => no_args(name, args, Command::LastPane)?,
        "detach-client" | "detach" => match args {
            [a] if a == "-a" => Command::DetachOthers,
            _ => no_args(name, args, Command::DetachClient)?,
        },
        "new-session" | "new" => no_args(name, args, Command::NewSession)?,
        "kill-session" => match args {
            [] => Command::KillSession(None),
            [flag, target] if flag == "-t" => Command::KillSession(Some(target.clone())),
            [a] if a == "-a" => Command::KillOtherSessions(None),
            [a, t, target] | [t, target, a] if a == "-a" && t == "-t" => Command::KillOtherSessions(Some(target.clone())),
            _ => return Err(format!("usage: {name} [-a] [-t name]")),
        },
        "display-panes" | "displayp" => match args {
            [] => Command::DisplayPanes,
            [flag, ms] if flag == "-d" => Command::DisplayPanesFor(ms.parse().map_err(|_| format!("{name}: bad duration {ms}"))?),
            [template] if !template.starts_with('-') => Command::DisplayPanesRun { ms: None, template: template.clone() },
            [flag, ms, template] if flag == "-d" && !template.starts_with('-') => {
                Command::DisplayPanesRun { ms: Some(ms.parse().map_err(|_| format!("{name}: bad duration {ms}"))?), template: template.clone() }
            }
            _ => return Err(format!("usage: {name} [-d milliseconds] [template]")),
        },
        "copy-mode" => no_args(name, args, Command::CopyMode)?,
        "clear-history" | "clearhist" => no_args(name, args, Command::ClearHistory)?,
        "paste-buffer" | "pasteb" | "list-buffers" | "lsb" | "choose-buffer" | "delete-buffer" | "deleteb" | "show-buffer"
        | "showb" | "set-buffer" | "setb" | "save-buffer" | "saveb" | "load-buffer" | "loadb" => buffer_command(name, args)?,
        "split-window" | "splitw" => Command::SplitWindow(window_flags(name, args)?.unwrap_or(Split::Stacked)),
        "select-pane" | "selectp" => match args {
            [flag] if flag == "-m" => Command::MarkPane(true),
            [flag] if flag == "-M" => Command::MarkPane(false),
            [flag] => match direction(flag) {
                Some(dir) => Command::SelectPane(dir),
                None => return Err(format!("{name}: unknown flag {flag}")),
            },
            _ => return Err(format!("usage: {name} -L|-R|-U|-D|-m|-M")),
        },
        "resize-pane" | "resizep" => {
            if args == ["-Z"] {
                return Ok(Command::ToggleZoom);
            }
            let (flag, count) = match args {
                [flag] => (flag, 1),
                [flag, n] => (flag, n.parse::<u16>().map_err(|_| format!("{name}: bad number {n}"))?),
                _ => return Err(format!("usage: {name} -L|-R|-U|-D [cells]")),
            };
            match direction(flag) {
                Some(dir) => Command::ResizePane(dir, count),
                None => return Err(format!("{name}: unknown flag {flag}")),
            }
        }
        "select-layout" | "selectl" => match args {
            [flag] if flag == "-E" => Command::SpreadLayout,
            [layout] if layout == "even-vertical" => Command::SelectLayout(Split::Stacked),
            [layout] if layout == "even-horizontal" => Command::SelectLayout(Split::SideBySide),
            [layout] => match Preset::parse(layout) {
                Some(preset) => Command::LayoutPreset(preset),
                None => return Err(format!("{name}: unknown layout {layout}")),
            },
            _ => return Err(format!("usage: {name} layout|-E (even-horizontal, even-vertical, main-horizontal, main-vertical, tiled, ...)")),
        },
        "next-layout" | "nextl" => no_args(name, args, Command::NextLayout)?,
        "break-pane" | "breakp" => match args {
            [d] if d == "-d" => Command::BreakPaneKeep,
            _ => no_args(name, args, Command::BreakPane)?,
        },
        "swap-pane" | "swapp" => match args {
            [] => Command::SwapWithMarked,
            [flag] if flag == "-U" => Command::SwapPane(false),
            [flag] if flag == "-D" => Command::SwapPane(true),
            _ => return Err(format!("usage: {name} [-U|-D]")),
        },
        "rotate-window" | "rotatew" => match args {
            [] => Command::RotateWindow(false),
            [flag] if flag == "-U" => Command::RotateWindow(false),
            [flag] if flag == "-D" => Command::RotateWindow(true),
            _ => return Err(format!("usage: {name} [-U|-D]")),
        },
        "select-window" | "selectw" => match args {
            [flag, target] if flag == "-t" => {
                let n = target.trim_start_matches(':');
                Command::SelectWindow(n.parse().map_err(|_| format!("{name}: bad window {target}"))?)
            }
            _ => return Err(format!("usage: {name} -t number")),
        },
        "swap-window" | "swapw" if relative_swap(args).is_some() => {
            let (by, select) = relative_swap(args).unwrap_or((0, true));
            Command::SwapWindowBy { by, select }
        }
        "swap-window" | "swapw" => {
            let (src, dst, select) = reorder_flags(name, args)?;
            Command::SwapWindow { src, dst, select }
        }
        "move-window" | "movew" => {
            let (src, dst, select) = reorder_flags(name, args)?;
            Command::MoveWindow { src, dst, select }
        }
        "rename-window" | "renamew" => match args {
            [] => Command::RenameWindow,
            [new] => Command::SetWindowName(new.clone()),
            _ => return Err(format!("usage: {name} [name]")),
        },
        "rename-session" | "rename" => match args {
            [] => Command::RenameSession,
            [new] => Command::SetSessionName(new.clone()),
            _ => return Err(format!("usage: {name} [name]")),
        },
        "list-keys" | "lsk" => no_args(name, args, Command::ListKeys)?,
        "show-messages" | "showmsgs" => no_args(name, args, Command::ShowMessages)?,
        "refresh-client" | "refresh" => no_args(name, args, Command::RefreshClient)?,
        "command-prompt" if args.is_empty() => Command::OpenPrompt,
        "command-prompt" => Command::Ask(ask_args(name, args, false)?),
        "confirm-before" | "confirm" => Command::Ask(ask_args(name, args, true)?),
        "suspend-client" | "suspendc" => no_args(name, args, Command::SuspendClient)?,
        "choose-client" => no_args(name, args, Command::ChooseClient)?,
        "clock-mode" => no_args(name, args, Command::ClockMode)?,
        "find-window" | "findw" => match args {
            [] => Command::FindWindow(None),
            [text] => Command::FindWindow(Some(text.clone())),
            _ => return Err(format!("usage: {name} [text]")),
        },
        "switch-client" | "switchc" => match args {
            [flag] if flag == "-n" => Command::NextSession,
            [flag] if flag == "-p" => Command::PreviousSession,
            [flag] if flag == "-l" => Command::LastSession,
            [flag, target] if flag == "-t" && !target.is_empty() => Command::SwitchTo(target.clone()),
            _ => return Err(format!("usage: {name} -n|-p|-l|-t target")),
        },
        "choose-tree" | "choose-window" | "choose-session" => match args {
            [flag] if flag == "-s" => Command::ChooseSession,
            [] if name == "choose-session" => Command::ChooseSession,
            [] => Command::ChooseWindow,
            [flag] if flag == "-w" => Command::ChooseWindow,
            [flags] if flags.len() > 2 && flags.starts_with('-') && flags.chars().skip(1).all(|c| "swZNGry".contains(c)) => {
                if flags.rfind('s') > flags.rfind('w') { Command::ChooseSession } else { Command::ChooseWindow }
            }
            _ => return Err(format!("usage: {name} [-s|-w]")),
        },
        other => return Err(format!("unknown command: {other}")),
    };
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(line: &str) -> Command {
        parse(line).unwrap().unwrap()
    }

    #[test]
    fn empty_line_is_nothing() {
        assert_eq!(parse("   ").unwrap(), None);
    }

    #[test]
    fn names_and_aliases() {
        assert_eq!(ok("new-window"), Command::NewWindow);
        assert_eq!(ok("neww"), Command::NewWindow);
        assert_eq!(ok("  kill-pane "), Command::KillPane);
    }

    #[test]
    fn snapshot_commands() {
        assert_eq!(ok("save-session"), Command::Persist(PersistOp::Save(None)));
        assert_eq!(ok("save-session work"), Command::Persist(PersistOp::Save(Some("work".to_string()))));
        assert_eq!(ok("restore-session"), Command::Persist(PersistOp::Restore(None)));
        assert_eq!(ok("restore-session work"), Command::Persist(PersistOp::Restore(Some("work".to_string()))));
        assert_eq!(ok("list-snapshots"), Command::Persist(PersistOp::List));
        assert_eq!(ok("delete-snapshot old"), Command::Persist(PersistOp::Delete("old".to_string())));
        for bad in ["save-session a b", "restore-session a b", "list-snapshots x", "delete-snapshot", "delete-snapshot a b"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn worktree_slug_flag() {
        let Command::Worktree(a) = ok("worktree --slug login-fix ABC-1") else { panic!("worktree") };
        assert_eq!((a.name.as_str(), a.slug.as_deref()), ("ABC-1", Some("login-fix")));
        assert!(parse("worktree ABC-1 --slug").is_err());
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn report_agent_arguments() {
        let r = report_args(&words("3 --source hook --agent claude --state working --seq 7 --message hi"), None, 99).unwrap();
        assert_eq!((r.pane, r.source.as_str(), r.agent.as_str(), r.state.as_str(), r.seq), (3, "hook", "claude", "working", 7));
        assert_eq!(r.message.as_deref(), Some("hi"));
        assert!(r.resume.is_empty());
        let r = report_args(&words("--source s --agent a --state idle -- claude --resume abc"), Some(5), 99).unwrap();
        assert_eq!((r.pane, r.seq), (5, 99));
        assert_eq!(r.resume, ["claude", "--resume", "abc"]);
        for bad in [
            "--source s --agent a --state idle",
            "1 --agent a --state idle",
            "1 --source s --state idle",
            "1 --source s --agent a",
            "x --source s --agent a --state idle",
            "1 --source s --agent a --state idle --seq many",
            "1 --source s --agent a --state idle --bogus",
            "1 --source",
        ] {
            assert!(report_args(&words(bad), None, 1).is_err(), "{bad}");
        }
    }

    #[test]
    fn release_and_wait_arguments() {
        assert_eq!(release_args(&words("4 --source hook"), None).unwrap(), (4, "hook".to_string()));
        assert_eq!(release_args(&words("--source hook"), Some(9)).unwrap(), (9, "hook".to_string()));
        assert!(release_args(&words("4"), None).is_err());
        assert!(release_args(&words("--source hook"), None).is_err());
        let w = wait_args(&words("2 --until done,blocked --timeout 30")).unwrap();
        assert_eq!((w.pane, w.until, w.timeout_ms), (2, UNTIL_DONE | UNTIL_BLOCKED, 30_000));
        let w = wait_args(&words("2")).unwrap();
        assert_eq!((w.until, w.timeout_ms), (0, 0));
        for bad in ["", "--until done", "2 --until sleeping", "2 --timeout soon", "2 --timeout 99999999999", "2 --nope"] {
            assert!(wait_args(&words(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn prompt_arguments() {
        let p = prompt_args(&words("3 fix the failing test")).unwrap();
        assert_eq!((p.pane, p.wait, p.timeout_ms, p.text.as_str()), (3, false, 0, "fix the failing test"));
        let p = prompt_args(&words("--wait 3 --timeout 90 run it")).unwrap();
        assert_eq!((p.pane, p.wait, p.timeout_ms, p.text.as_str()), (3, true, 90_000, "run it"));
        let p = prompt_args(&words("3 --wait go")).unwrap();
        assert!(p.wait && p.text == "go", "flags may follow the pane");
        let p = prompt_args(&words("3 go --wait")).unwrap();
        assert!(!p.wait && p.text == "go --wait", "after the text everything is text");
        let p = prompt_args(&words("3 -- --wait is text")).unwrap();
        assert!(!p.wait && p.text == "--wait is text");
        assert_eq!(prompt_args(&words("3 -")).unwrap().text, "-");
        for bad in ["", "3", "x text", "3 --wait", "3 --timeout 5 text", "3 --timeout soon --wait t", "3 --nope text", "--wait --timeout"] {
            assert!(prompt_args(&words(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn events_arguments() {
        assert_eq!(events_args(&[]).unwrap(), Vec::<String>::new());
        assert_eq!(events_args(&words("--types a,b --types c")).unwrap(), ["a", "b", "c"]);
        assert_eq!(events_args(&words("--types a,,b,")).unwrap(), ["a", "b"]);
        for bad in ["--types", "a", "--nope x"] {
            assert!(events_args(&words(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn agent_panel_takes_no_arguments() {
        assert_eq!(ok("agent-panel"), Command::AgentPanel);
        assert!(parse("agent-panel x").is_err());
    }

    #[test]
    fn repo_wizard_takes_no_arguments() {
        assert_eq!(ok("repo-wizard"), Command::RepoWizard);
        assert!(parse("repo-wizard x").is_err());
    }

    #[test]
    fn worktree_command_words() {
        let wt = |name: &str| WorktreeArgs { name: name.to_string(), ..WorktreeArgs::default() };
        assert_eq!(ok("worktree ABC-1"), Command::Worktree(wt("ABC-1")));
        assert_eq!(
            ok("worktree ABC-1 develop"),
            Command::Worktree(WorktreeArgs { base: Some("develop".to_string()), ..wt("ABC-1") })
        );
        assert_eq!(
            ok("worktree /w/proj ABC-1 develop --detach"),
            Command::Worktree(WorktreeArgs {
                repo: Some("/w/proj".to_string()),
                base: Some("develop".to_string()),
                detach: Some(true),
                ..wt("ABC-1")
            })
        );
        assert_eq!(
            ok("worktree --branch --start '/start X' ../proj X"),
            Command::Worktree(WorktreeArgs {
                repo: Some("../proj".to_string()),
                detach: Some(false),
                start: Some("/start X".to_string()),
                ..wt("X")
            })
        );
        for bad in ["worktree", "worktree a b c", "worktree --bogus a", "worktree a --start", "worktree /w/proj"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn split_window_flags_follow_tmux() {
        assert_eq!(ok("split-window -h"), Command::SplitWindow(Split::SideBySide));
        assert_eq!(ok("split-window -v"), Command::SplitWindow(Split::Stacked));
        assert_eq!(ok("split-window"), Command::SplitWindow(Split::Stacked));
    }

    #[test]
    fn next_and_previous_window_take_a_for_the_next_alert() {
        assert_eq!(ok("next-window"), Command::NextWindow);
        assert_eq!(ok("next-window -a"), Command::NextAlertWindow);
        assert_eq!(ok("next -a"), Command::NextAlertWindow);
        assert_eq!(ok("previous-window -a"), Command::PreviousAlertWindow);
        assert_eq!(ok("prev -a"), Command::PreviousAlertWindow);
        assert!(parse("next-window -z").is_err());
        assert!(parse("next-window -a -a").is_err());
        for command in [Command::NextAlertWindow, Command::PreviousAlertWindow] {
            assert_eq!(ok(&command.describe()), command, "{}", command.describe());
        }
    }

    #[test]
    fn pane_and_layout_commands() {
        assert_eq!(ok("select-pane -L"), Command::SelectPane(Direction::Left));
        assert_eq!(ok("resize-pane -D 3"), Command::ResizePane(Direction::Down, 3));
        assert_eq!(ok("resize-pane -R"), Command::ResizePane(Direction::Right, 1));
        assert_eq!(ok("select-layout even-horizontal"), Command::SelectLayout(Split::SideBySide));
        assert_eq!(ok("select-window -t :3"), Command::SelectWindow(3));
    }

    #[test]
    fn swap_and_move_window_take_tmux_flags() {
        assert_eq!(ok("swap-window -t 2"), Command::SwapWindow { src: None, dst: 2, select: true });
        assert_eq!(ok("swapw -d -s 1 -t :3"), Command::SwapWindow { src: Some(1), dst: 3, select: false });
        assert_eq!(ok("move-window -t 1"), Command::MoveWindow { src: None, dst: 1, select: true });
        assert!(parse("swap-window").is_err());
        assert!(parse("move-window -t x").is_err());
        assert!(parse("move-window -s").is_err());
    }

    #[test]
    fn kill_session_takes_an_optional_name() {
        assert_eq!(ok("kill-session"), Command::KillSession(None));
        assert_eq!(ok("kill-session -t work"), Command::KillSession(Some("work".to_string())));
        assert!(parse("kill-session work").is_err());
    }

    #[test]
    fn quoted_names_stay_whole() {
        assert_eq!(ok("rename-window \"my build\""), Command::SetWindowName("my build".to_string()));
        assert_eq!(ok("rename-session 'a b'"), Command::SetSessionName("a b".to_string()));
    }

    #[test]
    fn mistakes_are_errors_not_panics() {
        assert!(parse("frobnicate").unwrap_err().contains("unknown command"));
        assert!(parse("select-pane").is_err());
        assert!(parse("resize-pane -L x").is_err());
        assert!(parse("select-layout diagonal").is_err());
        assert!(parse("rename-window \"open").is_err());
        assert!(parse("kill-pane now").is_err());
    }

    #[test]
    fn the_arranging_commands_parse() {
        assert_eq!(ok("resize-pane -Z"), Command::ToggleZoom);
        assert_eq!(ok("break-pane"), Command::BreakPane);
        assert_eq!(ok("swap-pane -U"), Command::SwapPane(false));
        assert_eq!(ok("swap-pane -D"), Command::SwapPane(true));
        assert_eq!(ok("rotate-window"), Command::RotateWindow(false));
        assert_eq!(ok("rotate-window -D"), Command::RotateWindow(true));
        assert_eq!(ok("next-layout"), Command::NextLayout);
        assert_eq!(ok("select-layout -E"), Command::SpreadLayout);
        assert_eq!(ok("select-layout tiled"), Command::LayoutPreset(Preset::Tiled));
        assert_eq!(ok("select-layout main-vertical-mirrored"), Command::LayoutPreset(Preset::MainVerticalMirrored));
        assert_eq!(ok("select-layout even-horizontal"), Command::SelectLayout(Split::SideBySide));
        for bad in ["swap-pane -X", "rotate-window -Q", "next-layout now", "break-pane x", "select-layout"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        for command in [Command::ToggleZoom, Command::BreakPane, Command::SwapPane(true), Command::RotateWindow(true), Command::NextLayout, Command::SpreadLayout, Command::LayoutPreset(Preset::Tiled)] {
            assert_eq!(ok(&command.describe()), command, "{}", command.describe());
        }
    }

    #[test]
    fn the_lookup_commands_parse() {
        assert_eq!(ok("list-keys"), Command::ListKeys);
        assert_eq!(ok("show-messages"), Command::ShowMessages);
        assert_eq!(ok("refresh-client"), Command::RefreshClient);
        assert_eq!(ok("command-prompt"), Command::OpenPrompt);
        assert_eq!(ok("find-window"), Command::FindWindow(None));
        assert_eq!(ok("find-window main"), Command::FindWindow(Some("main".to_string())));
        assert_eq!(ok("switch-client -n"), Command::NextSession);
        assert_eq!(ok("switch-client -p"), Command::PreviousSession);
        assert_eq!(ok("switch-client -l"), Command::LastSession);
        assert_eq!(ok("choose-tree -s"), Command::ChooseSession);
        assert_eq!(ok("choose-tree"), Command::ChooseWindow);
        assert_eq!(ok("rename-window"), Command::RenameWindow);
        assert_eq!(ok("rename-session"), Command::RenameSession);
        for bad in ["switch-client", "switch-client -x", "choose-tree -x", "list-keys all", "find-window a b"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn what_list_keys_shows_parses_back_for_commands_with_an_argument_form() {
        for command in [
            Command::NewWindow,
            Command::SelectWindow(3),
            Command::SplitWindow(Split::SideBySide),
            Command::SelectPane(Direction::Left),
            Command::ResizePane(Direction::Right, 5),
            Command::SelectLayout(Split::Stacked),
            Command::NextSession,
            Command::LastSession,
            Command::ChooseSession,
            Command::ChooseWindow,
            Command::ListKeys,
            Command::FindWindow(None),
            Command::MoveWindow { src: Some(1), dst: 4, select: false },
            Command::KillSession(Some("work".to_string())),
        ] {
            assert_eq!(ok(&command.describe()), command, "{}", command.describe());
        }
    }

    #[test]
    fn worktree_remove_takes_force_an_optional_repo_and_an_optional_name() {
        let rm = |name: Option<&str>, repo: Option<&str>, force: bool| {
            Command::WorktreeRemove(WorktreeRemoveArgs {
                name: name.map(str::to_string),
                repo: repo.map(str::to_string),
                from: None,
                force,
            })
        };
        assert_eq!(ok("worktree-remove"), rm(None, None, false));
        assert_eq!(ok("worktree-remove ABC-1"), rm(Some("ABC-1"), None, false));
        assert_eq!(ok("worktree-remove --force ABC-1"), rm(Some("ABC-1"), None, true));
        assert_eq!(ok("worktree-remove ABC-1 --force"), rm(Some("ABC-1"), None, true));
        assert_eq!(ok("worktree-remove ../proj ABC-1"), rm(Some("ABC-1"), Some("../proj"), false));
        assert_eq!(ok("worktree-remove --force"), rm(None, None, true));
        for bad in ["worktree-remove --bogus", "worktree-remove a b", "worktree-remove ../p a b", "worktree-remove -f x"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn client_clock_and_mark_commands() {
        assert_eq!(ok("suspend-client"), Command::SuspendClient);
        assert_eq!(ok("choose-client"), Command::ChooseClient);
        assert_eq!(ok("clock-mode"), Command::ClockMode);
        assert_eq!(ok("select-pane -m"), Command::MarkPane(true));
        assert_eq!(ok("select-pane -M"), Command::MarkPane(false));
        assert_eq!(ok("swap-pane"), Command::SwapWithMarked);
        for bad in ["suspend-client now", "choose-client x", "clock-mode 1", "select-pane -m -M"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        for command in [
            Command::SuspendClient,
            Command::ChooseClient,
            Command::ClockMode,
            Command::MarkPane(true),
            Command::MarkPane(false),
            Command::SwapWithMarked,
        ] {
            assert_eq!(ok(&command.describe()), command, "{}", command.describe());
        }
    }

    #[test]
    fn buffer_commands_take_the_stock_flags() {
        let name = |n: &str| Some(n.to_string());
        assert_eq!(ok("paste-buffer"), Command::PasteBuffer);
        assert_eq!(ok("list-buffers"), Command::Buffer(BufferOp::List));
        assert_eq!(ok("lsb"), Command::Buffer(BufferOp::List));
        assert_eq!(ok("choose-buffer"), Command::Buffer(BufferOp::Choose));
        assert_eq!(ok("delete-buffer"), Command::Buffer(BufferOp::Delete(None)));
        assert_eq!(ok("deleteb -b a"), Command::Buffer(BufferOp::Delete(name("a"))));
        assert_eq!(ok("show-buffer -b a"), Command::Buffer(BufferOp::Show(name("a"))));
        assert_eq!(
            ok("paste-buffer -d -r -b a -s :"),
            Command::Buffer(BufferOp::Paste { name: name("a"), delete: true, raw: true, sep: name(":") })
        );
        assert_eq!(
            ok("set-buffer -a -b n hello"),
            Command::Buffer(BufferOp::Set { name: name("n"), append: true, data: "hello".to_string() })
        );
        assert_eq!(
            ok("set-buffer -- -dash"),
            Command::Buffer(BufferOp::Set { name: None, append: false, data: "-dash".to_string() })
        );
        assert_eq!(
            ok("save-buffer -a /tmp/x"),
            Command::Buffer(BufferOp::Save { name: None, append: true, path: "/tmp/x".to_string() })
        );
        assert_eq!(ok("load-buffer -b n /tmp/x"), Command::Buffer(BufferOp::Load { name: name("n"), path: "/tmp/x".to_string() }));
        for bad in [
            "list-buffers x",
            "list-buffers -a",
            "delete-buffer a",
            "set-buffer",
            "set-buffer a b",
            "set-buffer -b",
            "paste-buffer -x",
            "paste-buffer text",
            "save-buffer",
            "load-buffer -a x",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn send_prefix_takes_only_dash_two() {
        assert_eq!(ok("send-prefix"), Command::SendPrefix);
        assert_eq!(ok("send-prefix -2"), Command::SendPrefix2);
        assert_eq!(ok("send-prefix -2").describe(), "send-prefix -2");
        assert!(parse("send-prefix -3").is_err());
        assert!(parse("send-prefix -2 x").is_err());
    }

    #[test]
    fn paste_buffer_takes_p_and_it_asks_for_nothing_new() {
        assert_eq!(ok("paste-buffer -p"), Command::PasteBuffer);
        assert_eq!(ok("pasteb -p"), Command::PasteBuffer);
        assert_eq!(ok("paste-buffer -p -d"), ok("paste-buffer -d"));
        assert_eq!(ok("paste-buffer -p -b a"), ok("paste-buffer -b a"));
        assert!(parse("list-buffers -p").is_err(), "only paste-buffer takes it");
    }

    #[test]
    fn buffer_commands_parse_back_from_their_description() {
        for op in [
            BufferOp::List,
            BufferOp::Choose,
            BufferOp::Delete(None),
            BufferOp::Delete(Some("a b".to_string())),
            BufferOp::Show(Some("x".to_string())),
            BufferOp::Set { name: Some("n".to_string()), append: true, data: "two words".to_string() },
            BufferOp::Paste { name: None, delete: true, raw: false, sep: Some(" ".to_string()) },
            BufferOp::Save { name: None, append: false, path: "/tmp/a b".to_string() },
            BufferOp::Load { name: Some("n".to_string()), path: "/tmp/x".to_string() },
        ] {
            let command = Command::Buffer(op);
            assert_eq!(ok(&command.describe()), command, "{}", command.describe());
        }
    }

    fn run(background: bool, imux: bool, text: &str) -> Command {
        Command::RunShell(ShellRun { background, imux, text: text.to_string() })
    }

    #[test]
    fn run_shell_takes_flags_and_one_command() {
        assert_eq!(ok("run-shell 'echo hi'"), run(false, false, "echo hi"));
        assert_eq!(ok("run-shell -b 'echo hi'"), run(true, false, "echo hi"));
        assert_eq!(ok("run-shell -C 'display-message x'"), run(false, true, "display-message x"));
        assert_eq!(ok("run-shell -bC x"), run(true, true, "x"));
        assert_eq!(ok("run-shell -- -x"), run(false, false, "-x"), "after -- a word that starts with a dash is the command");
    }

    #[test]
    fn run_shell_refuses_what_it_cannot_do_by_name() {
        assert!(parse("run-shell").unwrap_err().contains("usage"));
        assert!(parse("run-shell a b").unwrap_err().contains("usage"));
        assert!(parse("run-shell -d 5 x").unwrap_err().contains("unsupported flag -d"));
        assert!(parse("run-shell -t %1 x").unwrap_err().contains("unsupported flag -t"));
        assert!(parse("run-shell -z x").unwrap_err().contains("unsupported flag -z"));
    }

    #[test]
    fn if_shell_takes_a_condition_and_one_or_two_commands() {
        let cond = |background, format, then: &str, otherwise: Option<&str>| {
            Command::IfShell(ShellIf {
                background,
                format,
                condition: "test -f x".to_string(),
                then: then.to_string(),
                otherwise: otherwise.map(str::to_string),
            })
        };
        assert_eq!(ok("if-shell 'test -f x' 'set -g a b'"), cond(false, false, "set -g a b", None));
        assert_eq!(ok("if 'test -f x' 'set -g a b' 'set -g a c'"), cond(false, false, "set -g a b", Some("set -g a c")));
        assert_eq!(ok("if-shell -F 'test -f x' 'set -g a b'"), cond(false, true, "set -g a b", None));
        assert_eq!(ok("if-shell -b 'test -f x' 'set -g a b'"), cond(true, false, "set -g a b", None));
        assert!(parse("if-shell x").unwrap_err().contains("usage"));
        assert!(parse("if-shell a b c d").unwrap_err().contains("usage"));
    }

    #[test]
    fn the_shell_commands_parse_back_from_their_description() {
        for command in [
            run(false, false, "echo \"quoted\" it's"),
            run(true, true, "a b"),
            Command::IfShell(ShellIf {
                background: false,
                format: true,
                condition: "#{pane_in_mode}".into(),
                then: "x y".into(),
                otherwise: Some("z".into()),
            }),
            Command::IfShell(ShellIf {
                background: true,
                format: false,
                condition: "test -d \"/a b\"".into(),
                then: "w".into(),
                otherwise: None,
            }),
        ] {
            assert_eq!(ok(&command.describe()), command, "{}", command.describe());
        }
    }

    #[test]
    fn a_window_option_command_describes_itself_so_that_it_parses_back() {
        for op in [WindowOptOp::Set(true), WindowOptOp::Set(false), WindowOptOp::Unset, WindowOptOp::Toggle] {
            for target in [None, Some("work:2".to_string())] {
                let command = Command::SetWindowOption { name: "synchronize-panes".into(), op, target };
                assert_eq!(ok(&command.describe()), command, "{}", command.describe());
            }
        }
    }
}

#[cfg(test)]
mod menu_popup_tests {
    use super::*;

    fn menu(line: &str) -> Result<MenuSpec, String> {
        match parse(line)? {
            Some(Command::DisplayMenu(m)) => Ok(m),
            other => Err(format!("not a menu: {other:?}")),
        }
    }

    fn popup(line: &str) -> Result<PopupSpec, String> {
        match parse(line)? {
            Some(Command::DisplayPopup(p)) => Ok(p),
            other => Err(format!("not a popup: {other:?}")),
        }
    }

    #[test]
    fn a_menu_is_flags_then_triples_and_a_rule_stands_alone() {
        let m = menu(r##"display-menu -T "Pick" -x C -y P -b rounded -C 1 "One" o "rename-window one" "" "-Two" t x"##).unwrap();
        assert_eq!((m.title.as_str(), m.x.as_deref(), m.y.as_deref()), ("Pick", Some("C"), Some("P")));
        assert_eq!((m.lines.as_deref(), m.start.as_deref()), (Some("rounded"), Some("1")));
        let names: Vec<&str> = m.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["One", "", "-Two"]);
        assert_eq!((m.items[0].key.as_str(), m.items[0].command.as_str()), ("o", "rename-window one"));
        assert_eq!((m.items[1].key.as_str(), m.items[1].command.as_str()), ("", ""));
    }

    #[test]
    fn menu_flag_values_may_be_attached_and_the_alias_works() {
        let m = menu("menu -xC -yS -Ttitle a b c").unwrap();
        assert_eq!((m.x.as_deref(), m.y.as_deref(), m.title.as_str()), (Some("C"), Some("S"), "title"));
        assert!(menu("display-menu -O -M a b c").map(|m| m.stay && m.mouse).unwrap());
    }

    #[test]
    fn a_menu_whose_first_name_starts_with_a_dash_needs_the_double_dash() {
        let m = menu("display-menu -- -off o x").unwrap();
        assert_eq!(m.items[0].name, "-off");
        assert!(menu("display-menu -off o x").unwrap_err().contains("unknown flag"));
    }

    #[test]
    fn menu_mistakes_are_named() {
        assert!(menu("display-menu").unwrap_err().contains("usage"));
        assert!(menu("display-menu a b").unwrap_err().contains("not enough arguments"));
        assert!(menu("display-menu -b wavy a b c").unwrap_err().contains("-b is one of"));
        assert!(menu("display-menu -C x a b c").unwrap_err().contains("-C is a number"));
        assert!(menu("display-menu -s bogus a b c").unwrap_err().contains("-s"));
        assert!(menu("display-menu -z a b c").unwrap_err().contains("unknown flag -z"));
        assert!(menu("display-menu -T").unwrap_err().contains("needs a value"));
    }

    #[test]
    fn a_menu_describes_itself_so_that_it_parses_back() {
        let m = menu(r##"display-menu -T "A \"b\"" -x 3 -y C -b double -C - -s "bg=red" "One" o "x y" "" "-Two" t z"##).unwrap();
        let again = menu(&Command::DisplayMenu(m.clone()).describe()).unwrap();
        assert_eq!(again, m);
    }

    #[test]
    fn a_popup_counts_its_exit_flags_and_k_and_n_takes_them_back() {
        let p = popup("display-popup -E -E -k fzf").unwrap();
        assert_eq!((p.exit_flags, p.any_key), (2, true));
        let p = popup("display-popup -EEk -N fzf").unwrap();
        assert_eq!((p.exit_flags, p.any_key), (0, false));
        let p = popup("popup -E -Ek -B -C").unwrap();
        assert_eq!((p.exit_flags, p.any_key, p.no_border, p.close), (2, true, true, true));
    }

    #[test]
    fn a_popup_takes_its_flags_and_leaves_the_program_alone() {
        let p = popup(r##"display-popup -d /tmp -e A=1 -e B=two -w 80% -h 20 -x C -y 5 -T "t" -b heavy -s fg=red -S bg=blue -t %1 -c cl sh -c 'echo -E hi'"##).unwrap();
        assert_eq!(p.dir.as_deref(), Some("/tmp"));
        assert_eq!(p.env, ["A=1", "B=two"]);
        assert_eq!((p.width.as_deref(), p.height.as_deref()), (Some("80%"), Some("20")));
        assert_eq!((p.x.as_deref(), p.y.as_deref(), p.title.as_str()), (Some("C"), Some("5"), "t"));
        assert_eq!((p.lines.as_deref(), p.style.as_deref(), p.border_style.as_deref(), p.target.as_deref()), (Some("heavy"), Some("fg=red"), Some("bg=blue"), Some("%1")));
        assert_eq!(p.command, ["sh", "-c", "echo -E hi"]);
        assert_eq!(p.exit_flags, 0, "a -E after the program's first word is the program's");
    }

    #[test]
    fn a_popup_without_a_program_is_a_shell_and_an_empty_word_is_none() {
        assert!(popup("display-popup").unwrap().command.is_empty());
        assert!(popup("display-popup -E \"\"").unwrap().command.is_empty());
        assert_eq!(popup("display-popup -- -weird").unwrap().command, ["-weird"]);
    }

    #[test]
    fn popup_mistakes_are_named() {
        assert!(popup("display-popup -b wavy x").unwrap_err().contains("-b is one of"));
        assert!(popup("display-popup -w 0 x").unwrap_err().contains("-w"));
        assert!(popup("display-popup -h big x").unwrap_err().contains("-h"));
        assert!(popup("display-popup -e NOEQUALS x").unwrap_err().contains("NAME=value"));
        assert!(popup("display-popup -s bogus x").unwrap_err().contains("-s"));
        assert!(popup("display-popup -z x").unwrap_err().contains("unknown flag -z"));
        assert!(popup("display-popup -d").unwrap_err().contains("needs a value"));
    }

    #[test]
    fn a_popup_describes_itself_so_that_it_parses_back() {
        let p = popup(r##"display-popup -E -E -k -B -d /tmp -e A=1 -w 50% -h 9 -x P -y 4 -T "a b" -s fg=red -S bg=blue -t %2 sh -c "echo hi""##).unwrap();
        let again = popup(&Command::DisplayPopup(p.clone()).describe()).unwrap();
        assert_eq!(again, p);
        let close = popup("display-popup -C").unwrap();
        assert_eq!(popup(&Command::DisplayPopup(close.clone()).describe()).unwrap(), close);
    }
}

#[cfg(test)]
mod ask_tests {
    use super::*;

    #[test]
    fn display_panes_takes_a_template() {
        assert_eq!(parse("display-panes").unwrap(), Some(Command::DisplayPanes));
        assert_eq!(parse("display-panes -d 500").unwrap(), Some(Command::DisplayPanesFor(500)));
        assert_eq!(parse(r#"display-panes "swap-pane -t %%""#).unwrap(), Some(Command::DisplayPanesRun { ms: None, template: "swap-pane -t %%".into() }));
        assert_eq!(parse(r#"display-panes -d 900 "x %%""#).unwrap(), Some(Command::DisplayPanesRun { ms: Some(900), template: "x %%".into() }));
        assert_eq!(parse(r#"display-panes -d 900 "x %%""#).unwrap().unwrap().describe(), r#"display-panes -d 900 "x %%""#);
        assert!(parse("display-panes -d soon x").is_err());
        assert!(parse("display-panes -z").is_err());
    }

    #[test]
    fn copy_mode_with_flags_runs_through_the_socket_handler() {
        assert_eq!(parse("copy-mode").unwrap(), Some(Command::CopyMode));
        for line in ["copy-mode -u", "copy-mode -eH", "copy-mode -t %1", "copy-mode -u -t work:1.2"] {
            let words: Vec<String> = line.split(' ').map(str::to_string).collect();
            assert_eq!(parse(line).unwrap(), Some(Command::Server(words)), "{line}");
        }
        assert!(parse("copy-mode -x").is_err());
    }

    #[test]
    fn source_file_takes_its_flags_and_a_pattern() {
        let spec = |q, n, v, p: &str| Command::SourceWith(SourceSpec { path: p.into(), quiet: q, parse_only: n, verbose: v });
        assert_eq!(parse("source-file /x").unwrap(), Some(Command::SourceFile("/x".into())));
        assert_eq!(parse("source-file -q /x").unwrap(), Some(spec(true, false, false, "/x")));
        assert_eq!(parse("source -nv /x").unwrap(), Some(spec(false, true, true, "/x")));
        assert_eq!(parse("source-file -q -n /x").unwrap(), Some(spec(true, true, false, "/x")));
        assert_eq!(parse("source-file /d/*.conf").unwrap(), Some(spec(false, false, false, "/d/*.conf")));
        assert!(parse("source-file -z /x").is_err());
        assert!(parse("source-file").is_err());
        assert!(parse("source-file -q").is_err());
        assert!(parse("source-file /a /b").is_err());
        assert_eq!(parse("source-file -q /x").unwrap().unwrap().describe(), "source-file -q /x");
    }

    fn ask(line: &str) -> Result<AskSpec, String> {
        match parse(line)? {
            Some(Command::Ask(a)) => Ok(a),
            other => Err(format!("not an ask: {other:?}")),
        }
    }

    #[test]
    fn a_plain_command_prompt_stays_the_colon_prompt() {
        assert_eq!(parse("command-prompt").unwrap(), Some(Command::OpenPrompt));
    }

    #[test]
    fn a_command_prompt_with_flags_and_a_template_is_a_question() {
        let a = ask(r#"command-prompt -1 -I "old" -p "name:" "rename-window '%%'""#).unwrap();
        assert_eq!(a, AskSpec { label: Some("name:".into()), initial: Some("old".into()), template: "rename-window '%%'".into(), confirm: false, single: true });
        assert_eq!(ask("command-prompt -p go").unwrap().template, "%%", "no template: what is typed runs as it is");
    }

    #[test]
    fn confirm_before_takes_a_command_and_an_optional_label() {
        let a = ask(r#"confirm-before -p "kill?" kill-window"#).unwrap();
        assert_eq!(a, AskSpec { label: Some("kill?".into()), initial: None, template: "kill-window".into(), confirm: true, single: false });
        assert!(ask("confirm-before").unwrap_err().contains("usage"));
        assert!(ask("confirm-before -I x kill-window").unwrap_err().contains("unknown flag -I"));
        assert!(ask("confirm-before -1 kill-window").unwrap_err().contains("unknown flag -1"));
    }

    #[test]
    fn a_question_has_one_prompt_and_one_template() {
        assert_eq!(ask("command-prompt -p a,b x").unwrap().label.as_deref(), Some("a,b"));
        assert!(ask("command-prompt -p").unwrap_err().contains("needs a value"));
        assert!(ask("command-prompt -z x").unwrap_err().contains("unknown flag -z"));
        assert!(ask("command-prompt a b").unwrap_err().contains("usage"));
        assert_eq!(ask(r#"command-prompt -I "a,b" x"#).unwrap().initial.as_deref(), Some("a,b"));
    }

    #[test]
    fn a_question_describes_itself_so_that_it_parses_back() {
        for line in [r#"command-prompt -1 -I "x y" -p "q:" "select-window -t :%%""#, r#"confirm-before -p "sure?" "kill-pane""#, "command-prompt -p go"] {
            let a = ask(line).unwrap();
            assert_eq!(ask(&Command::Ask(a.clone()).describe()).unwrap(), a, "{line}");
        }
    }

    #[test]
    fn a_command_the_typed_table_lacks_runs_through_the_server() {
        assert!(matches!(parse("send-keys -t :1 x").unwrap(), Some(Command::Server(_))));
        assert!(matches!(parse("new-window -a -n x").unwrap(), Some(Command::Server(_))));
        assert!(matches!(parse("set -g mouse off").unwrap(), Some(Command::Server(_))));
        assert!(parse("wait-for ready").is_err(), "a command that would block cannot be bound");
    }

    #[test]
    fn the_all_but_this_flags_parse_and_describe_themselves() {
        let cases = [
            ("detach-client -a", Command::DetachOthers),
            ("kill-session -a", Command::KillOtherSessions(None)),
            ("kill-session -a -t work", Command::KillOtherSessions(Some("work".into()))),
            ("kill-session -t work -a", Command::KillOtherSessions(Some("work".into()))),
            ("display-panes -d 2000", Command::DisplayPanesFor(2000)),
            ("break-pane -d", Command::BreakPaneKeep),
        ];
        for (line, want) in cases {
            let got = parse(line).unwrap().unwrap();
            assert_eq!(got, want, "{line}");
            assert_eq!(parse(&got.describe()).unwrap().unwrap(), want, "{line}");
        }
        assert_eq!(parse("kill-session").unwrap(), Some(Command::KillSession(None)));
        assert_eq!(parse("detach-client").unwrap(), Some(Command::DetachClient));
        assert!(parse("display-panes -d soon").is_err());
        assert!(parse("display-panes -d").is_err());
        assert!(parse("kill-session -a x").is_err());
    }

    #[test]
    fn kill_window_and_kill_pane_take_all_but_this_through_the_server() {
        assert!(matches!(parse("kill-window -a").unwrap(), Some(Command::Server(_))));
        assert!(matches!(parse("kill-pane -a -t %1").unwrap(), Some(Command::Server(_))));
        assert_eq!(parse("kill-window").unwrap(), Some(Command::KillWindow));
        assert_eq!(parse("kill-pane").unwrap(), Some(Command::KillPane));
    }

    #[test]
    fn swap_window_by_an_offset_parses_and_describes_itself() {
        assert_eq!(parse("swap-window -t -1").unwrap(), Some(Command::SwapWindowBy { by: -1, select: true }));
        assert_eq!(parse("swap-window -d -t +1").unwrap(), Some(Command::SwapWindowBy { by: 1, select: false }));
        for c in [Command::SwapWindowBy { by: -1, select: true }, Command::SwapWindowBy { by: 3, select: false }] {
            assert_eq!(parse(&c.describe()).unwrap().unwrap(), c, "{}", c.describe());
        }
        assert_eq!(parse("swap-window -t 2").unwrap(), Some(Command::SwapWindow { src: None, dst: 2, select: true }));
        assert!(parse("swap-window -t -0").is_err());
        assert!(parse("swap-window -s 1 -t -1").is_err());
    }

    #[test]
    fn split_before_and_environment_flags_run_through_the_server() {
        assert!(matches!(parse("split-window -b -h -l 20").unwrap(), Some(Command::Server(_))));
        assert!(matches!(parse("new-window -e A=1 -e B=2 sh").unwrap(), Some(Command::Server(_))));
        assert!(matches!(parse("split-window -h -e A=1").unwrap(), Some(Command::Server(_))));
    }

    #[test]
    fn run_in_a_binding_is_run_shell_and_clear_history_has_no_arguments() {
        assert_eq!(parse("run 'echo hi'").unwrap(), parse("run-shell 'echo hi'").unwrap());
        assert_eq!(parse("clear-history").unwrap(), Some(Command::ClearHistory));
        assert_eq!(parse(&Command::ClearHistory.describe()).unwrap(), Some(Command::ClearHistory));
        assert!(parse("clear-history now").is_err());
    }

    #[test]
    fn a_unique_start_of_a_long_name_is_that_command() {
        assert_eq!(resolve_prefix("show-option"), Ok(Some("show-options")));
        assert_eq!(resolve_prefix("set-environ"), Ok(Some("set-environment")));
        assert_eq!(resolve_prefix("show-options"), Ok(None), "a name imux has is left alone");
        assert_eq!(resolve_prefix("neww"), Ok(None), "so is an alias");
        assert_eq!(resolve_prefix("sh"), Ok(None), "two letters guess nothing");
        assert_eq!(resolve_prefix("frobnicate"), Ok(None));
        assert!(resolve_prefix("list-").unwrap_err().contains("ambiguous command: list-"));
        assert!(resolve_prefix("kill-").unwrap_err().contains("kill-pane"));
        assert!(matches!(parse("new-wind -a").unwrap(), Some(Command::Server(w)) if w[0] == "new-window"));
        assert!(parse("list- x").unwrap_err().contains("ambiguous"));
    }

    #[test]
    fn switch_client_takes_a_target() {
        assert_eq!(parse("switch-client -t work:2").unwrap(), Some(Command::SwitchTo("work:2".into())));
        let c = Command::SwitchTo("a b".into());
        assert_eq!(parse(&c.describe()).unwrap().unwrap(), c);
        assert!(parse("switch-client -t").is_err());
        assert!(parse("switch-client -t ''").is_err());
    }

    #[test]
    fn choose_tree_takes_clustered_flags_it_can_honor() {
        assert_eq!(parse("choose-tree -Zw").unwrap(), Some(Command::ChooseWindow));
        assert_eq!(parse("choose-tree -Zs").unwrap(), Some(Command::ChooseSession));
        assert_eq!(parse("choose-tree -sw").unwrap(), Some(Command::ChooseWindow));
        assert!(parse("choose-tree -Zq").is_err());
    }

    #[test]
    fn select_pane_with_a_relative_target_runs_through_the_server() {
        assert!(matches!(parse("select-pane -t :.+").unwrap(), Some(Command::Server(_))));
        assert!(parse("select-pane -L").unwrap().is_some_and(|c| !matches!(c, Command::Server(_))));
    }

    #[test]
    fn a_server_command_describes_itself_so_that_it_parses_back() {
        for line in ["send-keys -t :1 x", r#"new-window -a -n "a b""#] {
            let c = parse(line).unwrap().unwrap();
            assert_eq!(parse(&c.describe()).unwrap().unwrap(), c, "{line}");
        }
    }
}
