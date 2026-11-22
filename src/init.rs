//! `imux init zsh|bash|fish`: a few lines for the end of the shell's startup file that keep the
//! `tmux` shim (`tmux-compat`) first on `PATH` inside imux panes. A pane shell is a login shell,
//! and the startup files of a login shell rebuild `PATH` (macOS `path_helper`, Homebrew's
//! `shellenv`), which puts the shim behind the real `tmux`. Nothing outside the server can reorder
//! a `PATH` after that, so the shell does it itself, now and before each prompt. Outside imux
//! panes `IMUX_SHIM_DIR` is not set and the lines do nothing.

/// The shells a snippet exists for.
pub const SHELLS: [&str; 3] = ["zsh", "bash", "fish"];

const ZSH: &str = r#"if [[ -n ${IMUX_SHIM_DIR-} ]]; then
  _imux_shim_first() {
    [[ ${path[1]-} == $IMUX_SHIM_DIR ]] || path=("$IMUX_SHIM_DIR" ${path:#$IMUX_SHIM_DIR})
  }
  _imux_shim_first
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd _imux_shim_first
fi
"#;

const BASH: &str = r#"if [ -n "${IMUX_SHIM_DIR-}" ]; then
  _imux_shim_first() {
    [ "${PATH%%:*}" = "$IMUX_SHIM_DIR" ] && return 0
    local rest=":$PATH:"
    rest="${rest//:$IMUX_SHIM_DIR:/:}"
    rest="${rest#:}"
    rest="${rest%:}"
    PATH="$IMUX_SHIM_DIR${rest:+:$rest}"
  }
  _imux_shim_first
  case ";${PROMPT_COMMAND-};" in
    *";_imux_shim_first;"*) ;;
    *) PROMPT_COMMAND="_imux_shim_first${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
  esac
fi
"#;

const FISH: &str = r#"if set -q IMUX_SHIM_DIR
  function _imux_shim_first --on-event fish_prompt
    if test "$PATH[1]" != "$IMUX_SHIM_DIR"
      set -gx PATH $IMUX_SHIM_DIR (string match -v -- $IMUX_SHIM_DIR $PATH)
    end
  end
  _imux_shim_first
end
"#;

/// The snippet for `shell`, or why there is none.
pub fn script(shell: &str) -> Result<&'static str, String> {
    match shell {
        "zsh" => Ok(ZSH),
        "bash" => Ok(BASH),
        "fish" => Ok(FISH),
        other => Err(format!("{other:?} is not a shell imux init has a snippet for ({})", SHELLS.join(", "))),
    }
}

/// The line to add to the startup file of `shell`.
pub fn line(shell: &str) -> String {
    match shell {
        "fish" => "imux init fish | source".to_string(),
        other => format!("eval \"$(imux init {other})\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn every_shell_has_a_snippet_and_others_are_refused() {
        for shell in SHELLS {
            assert!(script(shell).unwrap().contains("IMUX_SHIM_DIR"), "{shell}");
        }
        assert!(script("csh").unwrap_err().contains("zsh, bash, fish"));
    }

    #[test]
    fn the_line_to_add_matches_the_shell() {
        assert_eq!(line("zsh"), "eval \"$(imux init zsh)\"");
        assert_eq!(line("fish"), "imux init fish | source");
    }

    fn available(shell: &str) -> bool {
        Command::new(shell).arg("--version").output().is_ok_and(|o| o.status.success())
    }

    #[test]
    fn the_snippets_are_valid_for_their_shell() {
        for shell in SHELLS {
            if !available(shell) {
                continue;
            }
            let out = Command::new(shell).args(["-n", "-c", script(shell).unwrap()]).output();
            let out = if shell == "fish" { Command::new(shell).args(["--no-execute", "-c", script(shell).unwrap()]).output() } else { out };
            assert!(out.unwrap().status.success(), "{shell} refused its snippet");
        }
    }

    #[test]
    fn bash_puts_the_shim_first_and_keeps_the_rest_in_order() {
        if !available("bash") {
            return;
        }
        let program = format!("{}\n_imux_shim_first\necho \"$PATH\"", script("bash").unwrap());
        let out = Command::new("bash")
            .args(["-c", &program])
            .env("IMUX_SHIM_DIR", "/s/shims")
            .env("PATH", "/usr/bin:/s/shims:/opt/bin:/bin")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/s/shims:/usr/bin:/opt/bin:/bin");
    }

    #[test]
    fn zsh_puts_the_shim_first_and_keeps_the_rest_in_order() {
        if !available("zsh") {
            return;
        }
        let program = format!("{}\n_imux_shim_first\necho \"$PATH\"", script("zsh").unwrap());
        let out = Command::new("zsh")
            .args(["-f", "-c", &program])
            .env("IMUX_SHIM_DIR", "/s/shims")
            .env("PATH", "/usr/bin:/s/shims:/opt/bin:/bin")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/s/shims:/usr/bin:/opt/bin:/bin");
    }

    #[test]
    fn outside_a_pane_nothing_changes() {
        if !available("bash") {
            return;
        }
        let program = format!("{}\necho \"$PATH\"", script("bash").unwrap());
        let out = Command::new("bash").args(["-c", &program]).env_remove("IMUX_SHIM_DIR").env("PATH", "/usr/bin:/bin").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/usr/bin:/bin");
    }
}
