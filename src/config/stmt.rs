//! Configuration statements: `set`, `bind`, `unbind`, and plain commands, as typed at the `:`
//! prompt or written in `~/.imux.conf`.

use super::lex::{self, Vars};
use crate::copymode::Op;
use crate::input::bindings::{Binding, Command};
use crate::input::commands;
use crate::input::keys::Key;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    Set { name: String, value: Option<String>, append: bool },
    /// `set -q` (an unknown option is no error) or `set -o` (an option that was set already keeps
    /// its value).
    SetGuarded { name: String, value: Option<String>, append: bool, quiet: bool, keep: bool },
    Bind { root: bool, key: Key, binding: Binding },
    Unbind { root: bool, key: Key },
    /// `bind -T copy-mode-vi` (`vi`) or `-T copy-mode` (emacs keys): a key for a copy mode action.
    BindCopy { vi: bool, key: Key, op: Op },
    UnbindCopy { vi: bool, key: Key },
    Run(Command),
    /// `set-hook [-a] [-u] [-R] name[slot] [command]`: what runs when an event fires.
    SetHook { name: String, command: Option<String>, append: bool, unset: bool, run: bool },
    /// Understood but without effect; the text says why (empty: nothing to report).
    Ignored(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Table {
    Prefix,
    Root,
    /// `copy-mode-vi` (true) or `copy-mode` (false).
    Copy(bool),
}

fn table(name: &str) -> Result<Table, String> {
    match name {
        "prefix" => Ok(Table::Prefix),
        "root" => Ok(Table::Root),
        "copy-mode-vi" => Ok(Table::Copy(true)),
        "copy-mode" => Ok(Table::Copy(false)),
        other => Err(format!("unknown key table: {other}")),
    }
}

/// `-a`, `-g` and friends before the arguments; `takes` lists flags that consume the next word.
fn flags<'a>(args: &'a [String], takes: &str) -> (Vec<(char, Option<&'a str>)>, &'a [String]) {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i).filter(|a| a.len() > 1 && a.starts_with('-')) {
        for (at, c) in arg.char_indices().skip(1) {
            if takes.contains(c) {
                // The rest of the word is the value (`-Tcopy-mode`), else the next word.
                let attached = &arg[at + c.len_utf8()..];
                if attached.is_empty() {
                    out.push((c, args.get(i + 1).map(String::as_str)));
                    i += 1;
                } else {
                    out.push((c, Some(attached)));
                }
                break;
            }
            out.push((c, None));
        }
        i += 1;
    }
    (out, &args[i.min(args.len())..])
}

fn set(args: &[String]) -> Result<Stmt, String> {
    let (flags, rest) = flags(args, "");
    let has = |f: char| flags.iter().any(|(c, _)| *c == f);
    let guarded = |stmt: Stmt| match stmt {
        Stmt::Set { name, value, append } if has('q') || has('o') => {
            Stmt::SetGuarded { name, value, append, quiet: has('q'), keep: has('o') }
        }
        other => other,
    };
    // The scope flags -g, -s, -w and -p are accepted: imux has one scope for every option.
    match (rest, has('u')) {
        ([name], true) => Ok(guarded(Stmt::Set { name: name.clone(), value: None, append: false })),
        ([name, value], _) => Ok(guarded(Stmt::Set { name: name.clone(), value: Some(value.clone()), append: has('a') })),
        ([name], false) => Err(format!("set: {name} needs a value")),
        _ => Err("usage: set [-agopqsuw] option value".to_string()),
    }
}

/// `set-hook [-agpRuw] name [command]`. The scope flags are accepted: imux has one scope.
fn set_hook(args: &[String]) -> Result<Stmt, String> {
    const USAGE: &str = "usage: set-hook [-agpRuw] hook-name [command]";
    let (flags, rest) = flags(args, "");
    let has = |f: char| flags.iter().any(|(c, _)| *c == f);
    if let Some((bad, _)) = flags.iter().find(|(c, _)| !"agpRuw".contains(*c)) {
        return Err(format!("set-hook: unknown flag -{bad}"));
    }
    match rest {
        [name] if has('u') || has('R') => {
            Ok(Stmt::SetHook { name: name.clone(), command: None, append: false, unset: has('u'), run: has('R') })
        }
        [name, command] if !has('u') && !has('R') => {
            Ok(Stmt::SetHook { name: name.clone(), command: Some(command.clone()), append: has('a'), unset: false, run: false })
        }
        _ => Err(USAGE.to_string()),
    }
}

/// `send-keys -X action [argument]` in a copy mode table. An action imux does not have is
/// reported and the key left alone; the argument of `copy-pipe` is not used, the copy goes through
/// `copy-command`.
fn copy_binding(vi: bool, key_name: &str, key: Key, command: &[String]) -> Result<Stmt, String> {
    const USAGE: &str = "usage: bind -T copy-mode[-vi] key send-keys -X action";
    let [name, flag, action, ..] = command else { return Err(USAGE.to_string()) };
    if !matches!(name.as_str(), "send" | "send-keys") || flag != "-X" {
        return Err(USAGE.to_string());
    }
    Ok(match Op::parse(action) {
        Some(op) => Stmt::BindCopy { vi, key, op },
        None => Stmt::Ignored(format!("copy-mode action {action} is not supported, {key_name} left as it was")),
    })
}

/// What a key runs: the words of one command, or a single word that is a command line or the text
/// of a `{ }` block, which may hold several commands (they run one after the other, like
/// `\;` between commands). Variables in a block are put in when the key is bound.
fn bound_command(command: &[String], vars: &Vars) -> Result<Command, String> {
    let [text] = command else { return commands::parse_words(command) };
    if !text.contains(|c: char| c.is_whitespace() || c == ';') {
        return commands::parse_words(command);
    }
    let mut parsed = Vec::new();
    for words in lex::statements(text, &mut vars.clone())? {
        parsed.push(commands::parse_words(&words)?);
    }
    match parsed.len() {
        0 => Err("bind: the command is empty".to_string()),
        1 => Ok(parsed.remove(0)),
        _ => Ok(Command::Chain(parsed)),
    }
}

fn bind(args: &[String], vars: &Vars) -> Result<Stmt, String> {
    let (flags, rest) = flags(args, "TN");
    let has = |f: char| flags.iter().any(|(c, _)| *c == f);
    let named = flags.iter().find(|(c, _)| *c == 'T').and_then(|(_, v)| *v);
    let tab = match (named, has('n')) {
        (Some(name), _) => table(name)?,
        (None, true) => Table::Root,
        (None, false) => Table::Prefix,
    };
    let Some((key_name, command)) = rest.split_first() else { return Err("usage: bind [-nr] [-T table] key command".to_string()) };
    let key = Key::parse(key_name).ok_or_else(|| format!("unknown key: {key_name}"))?;
    if let Table::Copy(vi) = tab {
        return copy_binding(vi, key_name, key, command);
    }
    let binding = Binding { command: bound_command(command, vars)?, repeat: has('r') };
    Ok(Stmt::Bind { root: tab == Table::Root, key, binding })
}

fn unbind(args: &[String]) -> Result<Stmt, String> {
    let (flags, rest) = flags(args, "T");
    let has = |f: char| flags.iter().any(|(c, _)| *c == f);
    let named = flags.iter().find(|(c, _)| *c == 'T').and_then(|(_, v)| *v);
    let tab = match (named, has('n')) {
        (Some(name), _) => table(name)?,
        (None, true) => Table::Root,
        (None, false) => Table::Prefix,
    };
    let [key_name] = rest else { return Err("usage: unbind [-n] [-T table] key".to_string()) };
    let key = Key::parse(key_name).ok_or_else(|| format!("unknown key: {key_name}"))?;
    if let Table::Copy(vi) = tab {
        return Ok(Stmt::UnbindCopy { vi, key });
    }
    Ok(Stmt::Unbind { root: tab == Table::Root, key })
}

fn statement(words: &[String], vars: &Vars) -> Result<Stmt, String> {
    let Some((name, args)) = words.split_first() else { return Err("empty command".to_string()) };
    match name.as_str() {
        "set" | "set-option" | "set-window-option" | "setw" => match commands::window_option(args) {
            Some(parsed) => parsed.map(Stmt::Run),
            None => set(args),
        },
        "bind" | "bind-key" => bind(args, vars),
        "unbind" | "unbind-key" => unbind(args),
        "set-hook" => set_hook(args),
        // `run` is how plugin managers (tpm) start. It is `run-shell -b`: the script runs on a
        // helper thread, so it can call back into the server through the `tmux` shim while the
        // rest of the file is read. `run-shell` and `if-shell` are commands like the others.
        "run" => {
            let mut run = commands::run_shell_args(args).map_err(|e| e.replace("run-shell", "run"))?;
            run.background = true;
            Ok(Stmt::Run(Command::RunShell(run)))
        }
        _ => commands::parse_words(words).map(Stmt::Run),
    }
}

/// One statement from the words of a command given on the command line (`imux set -g ...`).
pub fn parse_words(words: &[String]) -> Result<Stmt, String> {
    statement(words, &Vars::new())
}

/// Parse one logical line into statements. Commands after a `bind` on the same line (`\;`) belong
/// to the binding.
pub fn parse_line(line: &str, vars: &mut Vars) -> Result<Vec<Stmt>, String> {
    parse_line_aliased(line, vars, &std::collections::HashMap::new())
}

/// The words of a command that the aliases give another name: the expansion of the alias, then
/// the rest. `None` when `words` does not start with an alias. An alias is looked up once, so one
/// that names another alias does not chain.
pub fn expand_alias(words: &[String], aliases: &std::collections::HashMap<String, String>) -> Result<Option<Vec<String>>, String> {
    let Some(expansion) = words.first().and_then(|w| aliases.get(w)) else { return Ok(None) };
    let mut statements = lex::statements(expansion, &mut Vars::new())?;
    if statements.len() != 1 {
        return Err(format!("alias {}: the expansion must be one command", words[0]));
    }
    let mut expanded = statements.remove(0);
    expanded.extend(words[1..].iter().cloned());
    Ok(Some(expanded))
}

/// `parse_line` where a first word that is no command but an alias is replaced by its expansion.
pub fn parse_line_aliased(line: &str, vars: &mut Vars, aliases: &std::collections::HashMap<String, String>) -> Result<Vec<Stmt>, String> {
    let mut out: Vec<Stmt> = Vec::new();
    let mut chained = false;
    for words in lex::statements(line, vars)? {
        // An alias is looked up before a name is read as the start of a longer command name.
        let alias_first = words.first().is_some_and(|w| aliases.contains_key(w) && !commands::is_exact(w));
        let stmt = match if alias_first { Err("unknown command".to_string()) } else { statement(&words, vars) } {
            Err(e) if e.starts_with("unknown command") => match expand_alias(&words, aliases)? {
                Some(expanded) => statement(&expanded, vars)?,
                None => return Err(e),
            },
            other => other?,
        };
        match (chained, out.last_mut(), stmt) {
            (true, Some(Stmt::Bind { binding, .. }), Stmt::Run(next)) => {
                let first = std::mem::replace(&mut binding.command, Command::Chain(Vec::new()));
                binding.command = match first {
                    Command::Chain(mut list) => {
                        list.push(next);
                        Command::Chain(list)
                    }
                    one => Command::Chain(vec![one, next]),
                };
            }
            (true, Some(Stmt::Bind { .. }), _) => return Err("only commands can follow a bind on one line".to_string()),
            (_, _, stmt) => {
                chained = matches!(stmt, Stmt::Bind { .. });
                out.push(stmt);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::bindings::WindowOptOp;
    use crate::session::layout::{Direction, Split};

    fn one(line: &str) -> Stmt {
        let mut v = parse_line(line, &mut Vars::new()).unwrap();
        assert_eq!(v.len(), 1, "{line}");
        v.remove(0)
    }

    fn key(name: &str) -> Key {
        Key::parse(name).unwrap()
    }

    fn aliases(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs.iter().map(|(a, c)| (a.to_string(), c.to_string())).collect()
    }

    #[test]
    fn an_alias_stands_in_for_a_command_with_its_arguments_and_the_rest_of_the_line() {
        let a = aliases(&[("zoom", "resize-pane -Z"), ("split", "split-window -h")]);
        let line = |l: &str| parse_line_aliased(l, &mut Vars::new(), &a).unwrap();
        assert_eq!(line("zoom"), parse_line("resize-pane -Z", &mut Vars::new()).unwrap());
        assert_eq!(line("split -c /tmp"), parse_line("split-window -h -c /tmp", &mut Vars::new()).unwrap());
        assert_eq!(line("new-window ; zoom").len(), 2);
    }

    #[test]
    fn an_alias_is_looked_up_once_and_never_hides_a_command() {
        let a = aliases(&[("a", "b"), ("b", "resize-pane -Z"), ("new-window", "resize-pane -Z")]);
        assert!(parse_line_aliased("a", &mut Vars::new(), &a).is_err(), "an alias for an alias does not chain");
        let real = parse_line_aliased("new-window", &mut Vars::new(), &a).unwrap();
        assert_eq!(real, parse_line("new-window", &mut Vars::new()).unwrap());
        assert!(parse_line_aliased("nothing", &mut Vars::new(), &a).is_err());
    }

    #[test]
    fn an_alias_must_expand_to_one_command() {
        let a = aliases(&[("two", "new-window ; new-window")]);
        assert!(parse_line_aliased("two", &mut Vars::new(), &a).unwrap_err().contains("one command"));
        assert_eq!(expand_alias(&["x".to_string()], &a), Ok(None));
    }

    #[test]
    fn set_quiet_and_set_only_if_unset_are_guarded_statements() {
        assert_eq!(
            one("set -q nothing-like-it 1"),
            Stmt::SetGuarded { name: "nothing-like-it".into(), value: Some("1".into()), append: false, quiet: true, keep: false }
        );
        assert_eq!(
            one("set -go mouse on"),
            Stmt::SetGuarded { name: "mouse".into(), value: Some("on".into()), append: false, quiet: false, keep: true }
        );
        assert_eq!(
            one("set -gqa status-right x"),
            Stmt::SetGuarded { name: "status-right".into(), value: Some("x".into()), append: true, quiet: true, keep: false }
        );
    }

    #[test]
    fn the_scope_flags_are_accepted_and_change_nothing() {
        for flag in ["-g", "-s", "-w", "-p", "-gw", "-sg"] {
            assert_eq!(
                one(&format!("set {flag} mouse on")),
                Stmt::Set { name: "mouse".into(), value: Some("on".into()), append: false },
                "{flag}"
            );
        }
    }

    #[test]
    fn set_with_flags_and_values() {
        assert_eq!(one("set -g mouse on"), Stmt::Set { name: "mouse".into(), value: Some("on".into()), append: false });
        assert_eq!(one("set -ga terminal-overrides ',x:Tc'"), Stmt::Set { name: "terminal-overrides".into(), value: Some(",x:Tc".into()), append: true });
        assert_eq!(one("set -gu mouse"), Stmt::Set { name: "mouse".into(), value: None, append: false });
        assert!(parse_line("set -g mouse", &mut Vars::new()).is_err());
    }

    #[test]
    fn bind_forms() {
        assert_eq!(
            one("bind -r H resize-pane -L 5"),
            Stmt::Bind { root: false, key: key("H"), binding: Binding { command: Command::ResizePane(Direction::Left, 5), repeat: true } }
        );
        assert_eq!(
            one("bind | split-window -h -c \"#{pane_current_path}\""),
            Stmt::Bind { root: false, key: key("|"), binding: Binding { command: Command::SplitWindow(Split::SideBySide), repeat: false } }
        );
        assert_eq!(
            one("bind -n M-h select-pane -L"),
            Stmt::Bind { root: true, key: key("M-h"), binding: Binding { command: Command::SelectPane(Direction::Left), repeat: false } }
        );
        assert_eq!(
            one("bind C-a send-prefix"),
            Stmt::Bind { root: false, key: key("C-a"), binding: Binding { command: Command::SendPrefix, repeat: false } }
        );
    }

    #[test]
    fn chained_commands_belong_to_the_binding() {
        let s = one(r#"bind r source-file /x \; display "Config reloaded""#);
        let Stmt::Bind { binding, .. } = s else { panic!("not a bind") };
        assert_eq!(
            binding.command,
            Command::Chain(vec![Command::SourceFile("/x".into()), Command::DisplayMessage("Config reloaded".into())])
        );
    }

    #[test]
    fn unbind_forms() {
        assert_eq!(one("unbind C-b"), Stmt::Unbind { root: false, key: key("C-b") });
        assert_eq!(one("unbind '\"'"), Stmt::Unbind { root: false, key: key("\"") });
        assert_eq!(one("unbind -n M-x"), Stmt::Unbind { root: true, key: key("M-x") });
    }

    #[test]
    fn a_flag_value_may_be_attached_to_its_flag() {
        assert_eq!(one("unbind -Tcopy-mode C-g"), Stmt::UnbindCopy { vi: false, key: key("C-g") });
        assert_eq!(one("unbind -Tcopy-mode-vi x"), Stmt::UnbindCopy { vi: true, key: key("x") });
        assert_eq!(one("bind -Tcopy-mode-vi v send -X begin-selection"), Stmt::BindCopy { vi: true, key: key("v"), op: Op::BeginSelection });
        assert_eq!(one("unbind -nTroot M-x"), Stmt::Unbind { root: true, key: key("M-x") });
        assert!(parse_line("unbind -Tnonsense x", &mut Vars::new()).is_err());
    }

    #[test]
    fn copy_mode_bindings_set_an_action_in_the_named_table() {
        assert_eq!(one("bind -T copy-mode-vi v send -X begin-selection"), Stmt::BindCopy { vi: true, key: key("v"), op: Op::BeginSelection });
        assert_eq!(
            one("bind -T copy-mode-vi y send -X copy-pipe-and-cancel \"pbcopy\""),
            Stmt::BindCopy { vi: true, key: key("y"), op: Op::CopyAndCancel }
        );
        assert_eq!(one("bind -T copy-mode M-w send-keys -X cancel"), Stmt::BindCopy { vi: false, key: key("M-w"), op: Op::Cancel });
        assert_eq!(one("unbind -T copy-mode-vi x"), Stmt::UnbindCopy { vi: true, key: key("x") });
        assert_eq!(one("unbind -T copy-mode C-g"), Stmt::UnbindCopy { vi: false, key: key("C-g") });
    }

    #[test]
    fn copy_mode_bindings_with_an_unknown_action_or_form_do_no_harm() {
        assert!(matches!(one("bind -T copy-mode-vi x send -X frobnicate"), Stmt::Ignored(m) if m.contains("not supported")));
        assert!(parse_line("bind -T copy-mode-vi x new-window", &mut Vars::new()).is_err());
        assert!(parse_line("bind -T copy-mode-vi x send cancel", &mut Vars::new()).is_err());
        assert!(parse_line("bind -T nonsense x send -X cancel", &mut Vars::new()).is_err());
    }

    #[test]
    fn plain_commands_and_unsupported_ones() {
        assert_eq!(one("new-window"), Stmt::Run(Command::NewWindow));
        assert!(matches!(one("run '~/.tmux/plugins/tpm/tpm'"), Stmt::Run(Command::RunShell(r)) if r.background && !r.imux && r.text == "~/.tmux/plugins/tpm/tpm"));
        assert!(parse_line("frobnicate", &mut Vars::new()).is_err());
        assert!(parse_line("bind zz new-window", &mut Vars::new()).is_err());
    }

    #[test]
    fn run_is_run_shell_in_the_background() {
        assert!(matches!(one("run -b x"), Stmt::Run(Command::RunShell(r)) if r.background));
        assert!(matches!(one("run -C 'set -g mouse on'"), Stmt::Run(Command::RunShell(r)) if r.background && r.imux));
        assert!(parse_line("run", &mut Vars::new()).unwrap_err().contains("usage: run "));
        assert!(parse_line("run -t %1 x", &mut Vars::new()).unwrap_err().contains("unsupported flag -t"));
        assert!(parse_line("run a b", &mut Vars::new()).is_err());
    }

    #[test]
    fn a_colon_prompt_line_can_hold_several_commands() {
        let v = parse_line("new-window ; split-window -h", &mut Vars::new()).unwrap();
        assert_eq!(v, [Stmt::Run(Command::NewWindow), Stmt::Run(Command::SplitWindow(Split::SideBySide))]);
    }

    fn hook(name: &str, command: Option<&str>, append: bool, unset: bool, run: bool) -> Stmt {
        Stmt::SetHook { name: name.to_string(), command: command.map(str::to_string), append, unset, run }
    }

    #[test]
    fn set_hook_forms() {
        assert_eq!(one("set-hook -g after-new-window 'display-message hi'"), hook("after-new-window", Some("display-message hi"), false, false, false));
        assert_eq!(one("set-hook -ga pane-exited 'run-shell x'"), hook("pane-exited", Some("run-shell x"), true, false, false));
        assert_eq!(one("set-hook pane-exited[2] x"), hook("pane-exited[2]", Some("x"), false, false, false));
        assert_eq!(one("set-hook -u pane-exited"), hook("pane-exited", None, false, true, false));
        assert_eq!(one("set-hook -gR pane-exited"), hook("pane-exited", None, false, false, true));
        assert_eq!(one("set-hook -wp window-linked x"), hook("window-linked", Some("x"), false, false, false), "scope flags are accepted");
    }

    #[test]
    fn set_hook_mistakes() {
        for bad in ["set-hook", "set-hook x", "set-hook -u x y", "set-hook -R x y", "set-hook -z x y", "set-hook a b c"] {
            assert!(parse_line(bad, &mut Vars::new()).is_err(), "{bad}");
        }
    }

    fn window_option(name: &str, op: WindowOptOp, target: Option<&str>) -> Stmt {
        Stmt::Run(Command::SetWindowOption { name: name.to_string(), op, target: target.map(str::to_string) })
    }

    #[test]
    fn a_window_option_without_g_is_for_one_window_and_with_g_for_all() {
        assert_eq!(one("setw synchronize-panes on"), window_option("synchronize-panes", WindowOptOp::Set(true), None));
        assert_eq!(one("set-window-option synchronize-panes off"), window_option("synchronize-panes", WindowOptOp::Set(false), None));
        assert_eq!(one("set -w synchronize-panes yes"), window_option("synchronize-panes", WindowOptOp::Set(true), None));
        assert_eq!(one("setw synchronize-panes"), window_option("synchronize-panes", WindowOptOp::Toggle, None));
        assert_eq!(one("setw -u synchronize-panes"), window_option("synchronize-panes", WindowOptOp::Unset, None));
        assert_eq!(one("setw -t :2 synchronize-panes on"), window_option("synchronize-panes", WindowOptOp::Set(true), Some(":2")));
        assert_eq!(one("setw -g synchronize-panes on"), Stmt::Set { name: "synchronize-panes".into(), value: Some("on".into()), append: false });
        assert_eq!(one("set -g synchronize-panes on"), Stmt::Set { name: "synchronize-panes".into(), value: Some("on".into()), append: false });
        assert_eq!(one("setw -gu synchronize-panes"), Stmt::Set { name: "synchronize-panes".into(), value: None, append: false });
    }

    #[test]
    fn other_options_keep_their_one_scope() {
        assert_eq!(one("setw mouse off"), Stmt::Set { name: "mouse".into(), value: Some("off".into()), append: false });
        assert_eq!(one("set -as terminal-features x"), Stmt::Set { name: "terminal-features".into(), value: Some("x".into()), append: true });
    }

    #[test]
    fn window_option_mistakes() {
        for bad in ["setw synchronize-panes maybe", "setw -u synchronize-panes on", "setw synchronize-panes on off", "setw -z synchronize-panes on", "setw -t"] {
            assert!(parse_line(bad, &mut Vars::new()).is_err(), "{bad}");
        }
        assert!(parse_line("setw synchronize-panes maybe", &mut Vars::new()).unwrap_err().contains("expected on or off"));
        assert!(parse_line("setw -z synchronize-panes on", &mut Vars::new()).unwrap_err().contains("unknown flag -z"));
    }

    #[test]
    fn a_window_option_can_be_bound_to_a_key() {
        let Stmt::Bind { binding, .. } = one("bind S setw synchronize-panes") else { panic!("not a binding") };
        assert_eq!(binding.command, Command::SetWindowOption { name: "synchronize-panes".into(), op: WindowOptOp::Toggle, target: None });
        assert!(parse_line("bind S wait-for ready", &mut Vars::new()).is_err(), "a command that would block cannot be bound");
    }
}

#[cfg(test)]
mod block_tests {
    use super::*;

    fn binding(line: &str) -> Command {
        let mut v = parse_line(line, &mut Vars::new()).unwrap();
        assert_eq!(v.len(), 1, "{line}");
        let Stmt::Bind { binding, .. } = v.remove(0) else { panic!("not a binding: {line}") };
        binding.command
    }

    #[test]
    fn a_block_binds_its_commands_like_commands_chained_with_a_semicolon() {
        let chained = binding("bind x new-window \\; split-window -h");
        assert_eq!(binding("bind x { new-window; split-window -h }"), chained);
        assert_eq!(binding("bind x { new-window \\; split-window -h }"), chained);
        assert_eq!(binding("bind x { new-window }"), binding("bind x new-window"));
    }

    #[test]
    fn a_block_over_several_lines_binds_the_same_way() {
        let text = "bind x {\n  new-window  # a comment\n  split-window -h\n}\n";
        let all = lex::logical_lines(text);
        assert_eq!(all.len(), 1);
        assert_eq!(binding(&all[0].1), binding("bind x new-window \\; split-window -h"));
    }

    #[test]
    fn a_command_given_as_one_quoted_string_is_a_command_line() {
        assert_eq!(binding("bind x \"split-window -h\""), binding("bind x split-window -h"));
        assert_eq!(binding("bind x 'new-window ; kill-pane'"), binding("bind x new-window \\; kill-pane"));
    }

    #[test]
    fn a_bad_command_in_a_block_is_reported() {
        assert!(parse_line("bind x { new-window; frobnicate }", &mut Vars::new()).unwrap_err().contains("unknown command"));
        assert!(parse_line("bind x { }", &mut Vars::new()).is_err());
    }

    #[test]
    fn variables_in_a_block_are_put_in_when_the_key_is_bound() {
        let mut vars = Vars::new();
        parse_line("DIR=/tmp", &mut vars).unwrap();
        let Stmt::Bind { binding: b, .. } = parse_line("bind x { display-message \"$DIR\" }", &mut vars).unwrap().remove(0) else { panic!("not a binding") };
        assert_eq!(b.command, commands::parse_words(&["display-message".into(), "/tmp".into()]).unwrap());
    }

    #[test]
    fn a_hook_takes_a_block_as_its_command_text() {
        let Stmt::SetHook { command: Some(text), .. } = parse_line("set-hook -g after-new-window { display-message hi; select-pane -L }", &mut Vars::new()).unwrap().remove(0) else {
            panic!("not a hook")
        };
        assert_eq!(text, "display-message hi ; select-pane -L");
        let again = parse_line(&text, &mut Vars::new()).unwrap();
        assert_eq!(again.len(), 2);
    }

    #[test]
    fn if_shell_takes_blocks_for_both_branches() {
        let Stmt::Run(Command::IfShell(args)) = parse_line("if-shell \"test -d /tmp\" { new-window; kill-pane } { split-window }", &mut Vars::new()).unwrap().remove(0) else {
            panic!("not an if-shell")
        };
        assert_eq!(args.then, "new-window ; kill-pane");
        assert_eq!(args.otherwise.as_deref(), Some("split-window"));
    }

    #[test]
    fn a_nested_block_is_read_when_the_outer_one_runs() {
        let Stmt::Run(Command::IfShell(args)) = parse_line("if-shell true { if-shell false { a } { new-window } }", &mut Vars::new()).unwrap().remove(0) else {
            panic!("not an if-shell")
        };
        let inner = parse_line(&args.then, &mut Vars::new()).unwrap();
        let Stmt::Run(Command::IfShell(inner)) = &inner[0] else { panic!("not an inner if-shell") };
        assert_eq!(inner.otherwise.as_deref(), Some("new-window"));
    }
}
