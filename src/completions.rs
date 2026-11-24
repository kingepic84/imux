//! `imux completions zsh|bash|fish`: a completion script for the shell, built from the words imux
//! knows. Command and sub-command words come from the tables below (checked against the command
//! parser by the tests), option names from the options table and hook names from the hook list, so
//! those two cannot drift. Targets (`-t`) are left to the shell.

use crate::config::options::Options;

/// The shells a script can be made for.
pub const SHELLS: [&str; 5] = ["zsh", "bash", "fish", "pwsh", "nu"];

/// Words that start an `imux` command line outside the tmux-style command set: the client's own
/// commands, each with the words or flags that may follow it.
const CLI: &[(&str, &[&str])] = &[
    ("new", &["-s", "-A", "-D"]),
    ("attach", &["-t", "-d", "-r"]),
    ("ls", &[]),
    ("kill-server", &[]),
    ("save", &[]),
    ("restore", &[]),
    ("snapshots", &["--history"]),
    ("delete-snapshot", &[]),
    ("worktree", &["--detach", "--branch", "--start", "--setup", "--no-setup", "--slug"]),
    ("worktree-remove", &["--force"]),
    ("agent", &["list", "get", "wait", "prompt", "read", "name", "start"]),
    ("events", &["--types", "--since", "--wait", "--match", "--timeout"]),
    ("api", &["version", "schema"]),
    ("plugin", &["link", "unlink", "list", "enable", "disable", "logs"]),
    ("pane", &["report-agent", "release-agent", "run", "wait-output"]),
    ("skill", &["path"]),
    ("setup", &["--man-dir", "--skill-path", "--no-man", "--no-skill", "--shell", "--no-shell"]),
    ("doctor", &[]),
    ("version", &[]),
    ("completions", &SHELLS),
    ("init", &crate::init::SHELLS),
    ("check-config", &["--summary"]),
];

/// The tmux-style commands `imux <command>` runs on the server, by their long names.
const COMMANDS: &[&str] = &[
    "list-sessions", "list-windows", "list-panes", "list-clients", "has-session", "display-message", "capture-pane", "send-keys",
    "kill-session", "kill-window", "kill-pane", "select-window", "select-pane", "rename-session", "rename-window", "new-session",
    "new-window", "split-window", "set-environment", "show-environment", "set-option", "set-window-option", "bind-key", "unbind-key",
    "source-file", "respawn-pane", "respawn-window", "join-pane", "move-pane", "wait-for", "pipe-pane", "list-keys", "show-options",
    "show-window-options", "set-hook", "show-hooks", "save-session", "restore-session", "list-snapshots", "run-shell", "if-shell",
    "repo-wizard", "agent-panel", "send-prefix", "next-window", "previous-window", "last-window", "last-pane", "detach-client",
    "display-panes", "copy-mode", "resize-pane", "select-layout", "next-layout", "break-pane", "swap-pane", "rotate-window",
    "swap-window", "move-window", "show-messages", "refresh-client", "command-prompt", "suspend-client", "choose-client", "clock-mode", "display-menu", "menu", "display-popup", "popup",
    "find-window", "switch-client", "choose-tree", "list-buffers", "choose-buffer", "delete-buffer", "show-buffer", "set-buffer",
    "paste-buffer", "save-buffer", "load-buffer", "unlink-window", "choose-window", "choose-session", "api-agent-list", "api-agent-get", "api-pane-run", "api-pane-wait", "api-agent-name", "api-agent-session", "api-agent-resolve", "api-agent-start", "api-seq",
];

/// The commands the server runs, by their long names, for the help of `imux`.
pub fn server_commands() -> &'static [&'static str] {
    COMMANDS
}

/// The command names `list-commands` of the `tmux` shim prints.
pub fn tmux_commands() -> &'static [&'static str] {
    COMMANDS
}

/// The commands that take an option name, with every name they go by.
const OPTION_COMMANDS: &[&str] = &["set-option", "set", "set-window-option", "setw", "show-options", "show", "show-window-options", "showw"];

/// What may follow a word: the words, and whether only right after the command.
struct Rule {
    names: Vec<&'static str>,
    words: Vec<String>,
    first_only: bool,
}

/// The words that start a command line, each once, in a stable order.
fn top_words() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = CLI.iter().map(|(name, _)| *name).chain(COMMANDS.iter().copied()).collect();
    all.sort_unstable();
    all.dedup();
    all
}

fn rules(options: &[&str], hooks: &[&str]) -> Vec<Rule> {
    let mut rules: Vec<Rule> = CLI
        .iter()
        .filter(|(_, words)| !words.is_empty())
        .map(|(name, words)| Rule {
            names: vec![name],
            words: words.iter().map(|w| w.to_string()).collect(),
            first_only: !words.iter().all(|w| w.starts_with('-')),
        })
        .collect();
    rules.push(Rule {
        names: OPTION_COMMANDS.to_vec(),
        words: ["-g", "-u", "-a", "-w"].iter().map(|w| w.to_string()).chain(options.iter().map(|o| o.to_string())).collect(),
        first_only: false,
    });
    rules.push(Rule { names: vec!["set-hook"], words: hooks.iter().map(|h| h.to_string()).collect(), first_only: false });
    rules
}

/// The completion script for `shell`, or why there is none.
pub fn script(shell: &str, hooks: &[&str]) -> Result<String, String> {
    let options = Options::new();
    let names: Vec<&str> = options.listing().into_iter().map(|(name, _)| name).collect();
    let top = top_words();
    let rules = rules(&names, hooks);
    match shell {
        "zsh" => Ok(zsh(&top, &rules)),
        "bash" => Ok(bash(&top, &rules)),
        "fish" => Ok(fish(&top, &rules)),
        "pwsh" => Ok(pwsh(&top, &rules)),
        "nu" => Ok(nu(&top, &rules)),
        "ksh" => Err("ksh has no programmable completion, so there is nothing to generate for it".to_string()),
        other => Err(format!("{other:?} is not a shell imux has completions for ({})", SHELLS.join(", "))),
    }
}

fn pwsh(top: &[&str], rules: &[Rule]) -> String {
    let list = |words: &[String]| words.iter().map(|w| format!("'{w}'")).collect::<Vec<_>>().join(",");
    let mut out = String::from("# imux completions: imux completions pwsh | Out-String | Invoke-Expression  in $PROFILE.\n");
    out.push_str("Register-ArgumentCompleter -Native -CommandName imux -ScriptBlock {\n  param($wordToComplete, $commandAst, $cursorPosition)\n");
    out.push_str("  $words = @($commandAst.CommandElements | ForEach-Object { $_.ToString() })\n");
    out.push_str("  $index = if ($wordToComplete -eq '') { $words.Count } else { $words.Count - 1 }\n  $candidates = @()\n");
    let top: Vec<String> = top.iter().map(|w| w.to_string()).collect();
    out.push_str(&format!("  if ($index -eq 1) {{\n    $candidates = @({})\n  }} else {{\n    switch ($words[1]) {{\n", list(&top)));
    for rule in rules {
        let names: Vec<String> = rule.names.iter().map(|n| n.to_string()).collect();
        let set = format!("$candidates = @({})", list(&rule.words));
        let body = if rule.first_only { format!("if ($index -eq 2) {{ {set} }}") } else { set };
        out.push_str(&format!("      {{ $_ -in {} }} {{ {body} }}\n", list(&names)));
    }
    out.push_str("    }\n  }\n  $candidates | Where-Object { $_.StartsWith($wordToComplete, [System.StringComparison]::Ordinal) } | ForEach-Object {\n");
    out.push_str("    [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)\n  }\n}\n");
    out
}

fn nu(top: &[&str], rules: &[Rule]) -> String {
    let list = |words: &[String]| words.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(" ");
    let top: Vec<String> = top.iter().map(|w| w.to_string()).collect();
    let mut out = String::from("# imux completions: save as a file and `source` it from config.nu.\n");
    out.push_str(&format!("def \"nu-complete imux top\" [] {{ [ {} ] }}\n", list(&top)));
    for rule in rules {
        out.push_str(&format!("def \"nu-complete imux {}\" [] {{ [ {} ] }}\n", rule.names[0], list(&rule.words)));
    }
    out.push_str("extern \"imux\" [ command?: string@\"nu-complete imux top\" ...rest: string ]\n");
    for rule in rules {
        for name in &rule.names {
            if rule.first_only {
                out.push_str(&format!("extern \"imux {name}\" [ word?: string@\"nu-complete imux {}\" ...rest: string ]\n", rule.names[0]));
            } else {
                out.push_str(&format!("extern \"imux {name}\" [ ...rest: string@\"nu-complete imux {}\" ]\n", rule.names[0]));
            }
        }
    }
    out
}

fn zsh(top: &[&str], rules: &[Rule]) -> String {
    let mut out = String::from("#compdef imux\n# imux completions: eval \"$(imux completions zsh)\" in .zshrc, or save the output as _imux in a directory of $fpath.\n_imux() {\n");
    out.push_str(&format!("  if (( CURRENT == 2 )); then\n    compadd -- {}\n    return\n  fi\n  case $words[2] in\n", top.join(" ")));
    for rule in rules {
        out.push_str(&format!("    {})\n", rule.names.join("|")));
        if rule.first_only {
            out.push_str("      (( CURRENT == 3 )) || return 0\n");
        }
        out.push_str(&format!("      compadd -- {}\n      ;;\n", rule.words.join(" ")));
    }
    out.push_str("  esac\n}\nif [ \"$funcstack[1]\" = \"_imux\" ]; then\n  _imux \"$@\"\nelse\n  compdef _imux imux\nfi\n");
    out
}

fn bash(top: &[&str], rules: &[Rule]) -> String {
    let mut out = String::from("# imux completions: eval \"$(imux completions bash)\" in .bashrc.\n_imux() {\n  local cur=\"${COMP_WORDS[COMP_CWORD]}\"\n");
    out.push_str(&format!("  if [ \"$COMP_CWORD\" -eq 1 ]; then\n    COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )\n    return 0\n  fi\n", top.join(" ")));
    out.push_str("  case \"${COMP_WORDS[1]}\" in\n");
    for rule in rules {
        out.push_str(&format!("    {})\n", rule.names.join("|")));
        if rule.first_only {
            out.push_str("      [ \"$COMP_CWORD\" -eq 2 ] || return 0\n");
        }
        out.push_str(&format!("      COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )\n      ;;\n", rule.words.join(" ")));
    }
    out.push_str("  esac\n  return 0\n}\ncomplete -F _imux imux\n");
    out
}

fn fish(top: &[&str], rules: &[Rule]) -> String {
    let mut out = String::from("# imux completions: imux completions fish | source, or save the output as ~/.config/fish/completions/imux.fish.\ncomplete -c imux -f\n");
    out.push_str(&format!("complete -c imux -n '__fish_use_subcommand' -a '{}'\n", top.join(" ")));
    for rule in rules {
        let seen = format!("__fish_seen_subcommand_from {}", rule.names.join(" "));
        let condition = if rule.first_only { format!("{seen}; and not __fish_seen_subcommand_from {}", rule.words.join(" ")) } else { seen };
        out.push_str(&format!("complete -c imux -n '{condition}' -a '{}'\n", rule.words.join(" ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks() -> &'static [&'static str] {
        crate::server::hook_names()
    }

    #[test]
    fn every_command_in_the_table_is_one_the_server_runs() {
        for name in COMMANDS {
            assert!(crate::server::knows_command(name), "{name} is not a command");
        }
    }

    #[test]
    fn every_command_the_socket_runs_is_in_the_table() {
        for name in crate::server::remote_command_names() {
            if name.contains('-') && *name != "plugin-reload" {
                assert!(COMMANDS.contains(name), "{name} is missing from the completions");
            }
        }
    }

    #[test]
    fn the_client_words_are_the_commands_of_the_clap_tree() {
        let tree = crate::cli::tree();
        for (name, _) in CLI {
            assert!(tree.find_subcommand(name).is_some_and(|c| !c.is_hide_set()), "{name} is not a command of the imux command line");
        }
        for sub in tree.get_subcommands().filter(|c| !c.is_hide_set()) {
            let name = sub.get_name();
            assert!(CLI.iter().any(|(n, _)| *n == name) || COMMANDS.contains(&name), "{name} is missing from the completions");
        }
    }

    #[test]
    fn the_flags_and_sub_commands_offered_exist_in_the_tree() {
        let tree = crate::cli::tree();
        for (name, words) in CLI {
            let cmd = tree.find_subcommand(name).unwrap();
            for word in *words {
                let found = if let Some(long) = word.strip_prefix("--") {
                    cmd.get_arguments().any(|a| a.get_long() == Some(long))
                } else if let Some(short) = word.strip_prefix('-') {
                    cmd.get_arguments().any(|a| short.chars().next().is_some_and(|c| a.get_short() == Some(c)))
                } else {
                    cmd.find_subcommand(word).is_some() || matches!(*name, "completions" | "init")
                };
                assert!(found, "{name} {word} is offered but the command line has no such flag or sub-command");
            }
        }
    }

    #[test]
    fn every_option_command_is_a_command() {
        for name in OPTION_COMMANDS {
            assert!(crate::server::knows_command(name), "{name}");
        }
    }

    #[test]
    fn the_client_words_are_not_listed_twice() {
        let top = top_words();
        let mut sorted = top.clone();
        sorted.dedup();
        assert_eq!(top, sorted);
        assert!(top.contains(&"completions") && top.contains(&"doctor") && top.contains(&"new-window"));
    }

    #[test]
    fn a_script_exists_for_every_shell_and_only_those() {
        for shell in SHELLS {
            assert!(script(shell, hooks()).is_ok(), "{shell}");
        }
        assert!(script("csh", hooks()).unwrap_err().contains("zsh, bash, fish, pwsh, nu"));
    }

    #[test]
    fn ksh_is_refused_with_the_reason() {
        assert!(script("ksh", hooks()).unwrap_err().contains("no programmable completion"));
    }

    #[test]
    fn the_powershell_and_nushell_scripts_register_themselves() {
        let pwsh = script("pwsh", hooks()).unwrap();
        assert!(pwsh.contains("Register-ArgumentCompleter -Native -CommandName imux"));
        assert!(pwsh.contains("{ $_ -in 'agent' } { if ($index -eq 2) { $candidates = @('list','get','wait','prompt','read','name','start') } }"));
        let nu = script("nu", hooks()).unwrap();
        assert!(nu.contains("extern \"imux\" [ command?: string@\"nu-complete imux top\" ...rest: string ]"));
        assert!(nu.contains("extern \"imux agent\" [ word?: string@\"nu-complete imux agent\" ...rest: string ]"));
        assert!(nu.contains("extern \"imux setw\" [ ...rest: string@\"nu-complete imux set-option\" ]"));
    }

    #[test]
    fn every_option_and_hook_is_offered() {
        let options = Options::new();
        for shell in SHELLS {
            let text = script(shell, hooks()).unwrap();
            for (name, _) in options.listing() {
                assert!(text.contains(name), "{shell}: option {name}");
            }
            for hook in hooks() {
                assert!(text.contains(hook), "{shell}: hook {hook}");
            }
        }
    }

    #[test]
    fn words_are_plain_so_no_quoting_is_needed() {
        let options = Options::new();
        let names: Vec<&str> = options.listing().into_iter().map(|(name, _)| name).collect();
        let mut words: Vec<String> = top_words().iter().map(|w| w.to_string()).collect();
        for rule in rules(&names, hooks()) {
            words.extend(rule.words.clone());
            words.extend(rule.names.iter().map(|n| n.to_string()));
        }
        for word in words {
            assert!(!word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'@' | b'.')), "{word:?}");
        }
    }

    #[test]
    fn each_script_has_its_header_and_its_hook_up() {
        assert!(script("zsh", hooks()).unwrap().starts_with("#compdef imux\n"));
        assert!(script("zsh", hooks()).unwrap().contains("compdef _imux imux"));
        assert!(script("bash", hooks()).unwrap().contains("complete -F _imux imux"));
        assert!(script("fish", hooks()).unwrap().contains("complete -c imux -f"));
    }

    #[test]
    fn a_sub_command_is_offered_only_right_after_its_command() {
        let text = script("bash", hooks()).unwrap();
        assert!(text.contains("agent)\n      [ \"$COMP_CWORD\" -eq 2 ] || return 0\n"));
        assert!(!text.contains("worktree)\n      [ \"$COMP_CWORD\" -eq 2 ]"), "flags may come at any place");
    }
}
