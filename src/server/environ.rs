//! The environment new panes start with: `set-environment`, `show-environment`, and
//! `update-environment`, which copies chosen variables (the ssh agent socket, `DISPLAY`) from the
//! environment of each client that attaches, so a reattach over a new ssh login does not leave
//! new panes with a dead `SSH_AUTH_SOCK`.

use super::Server;
use super::remote::flags;

/// `Server::environment_value` over the `set-environment` entries alone, so a caller can hold it
/// while it uses other fields of the server.
pub(super) fn environment_in(pane_env: &[(String, Option<String>)], name: &str) -> Option<String> {
    let shaped = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !shaped {
        return None;
    }
    let value = match pane_env.iter().find(|(k, _)| k == name) {
        Some((_, value)) => value.clone()?,
        None => std::env::var(name).ok()?,
    };
    Some(crate::sanitize::clean(&value))
}

/// The names of `inherited` that `strip-environment` removes from a new pane: a name in the list,
/// or a start followed by `*`, unless `keep` says the pane gets a value of its own. `WINDOW` goes
/// with `STY` (a screen session sets both and `WINDOW` alone says nothing).
pub(super) fn stripped_names(list: &str, inherited: &[String], keep: &dyn Fn(&str) -> bool) -> Vec<String> {
    let words: Vec<&str> = list.split_whitespace().filter(|w| valid_name(w.trim_end_matches('*'))).collect();
    let hit = |name: &str| words.iter().any(|w| w.strip_suffix('*').map_or(*w == name, |start| name.starts_with(start)));
    let mut out: Vec<String> = inherited.iter().filter(|n| hit(n) && !keep(n)).cloned().collect();
    if out.iter().any(|n| n == "STY") && inherited.iter().any(|n| n == "WINDOW") && !keep("WINDOW") {
        out.push("WINDOW".to_string());
    }
    out
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 256 && !name.contains('=') && !name.chars().any(char::is_control)
}

impl Server {
    fn env_put(&mut self, name: &str, value: Option<String>) {
        match self.pane_env.iter_mut().find(|(k, _)| k == name) {
            Some(entry) => entry.1 = value,
            None => self.pane_env.push((name.to_string(), value)),
        }
    }

    /// A format variable that is not one of imux's reads the environment new panes get (what
    /// `set-environment` said, else the server's own), as `#{TERM_PROGRAM}` does for a
    /// configuration that tests it. Only names shaped like a variable; control characters dropped.
    pub(super) fn environment_value(&self, name: &str) -> Option<String> {
        environment_in(&self.pane_env, name)
    }

    fn env_remove(&mut self, name: &str) {
        self.pane_env.retain(|(k, _)| k != name);
    }

    /// What a client says about its environment on attach: the variables named by
    /// `update-environment` that it has are taken, the ones it has not are dropped. Anything
    /// else it sends is ignored, so a client cannot set arbitrary variables for new panes.
    pub(super) fn update_environment(&mut self, vars: Vec<(String, String)>) {
        let wanted: Vec<String> = self.options.text("update-environment").split_whitespace().map(str::to_string).collect();
        for name in wanted.iter().filter(|n| valid_name(n)) {
            match vars.iter().find(|(k, _)| k == name) {
                Some((_, value)) if !value.contains('\0') => self.env_put(name, Some(value.clone())),
                _ => self.env_remove(name),
            }
        }
    }

    /// `set-environment [-gru] name [value]`: `-u` keeps the variable out of new panes, `-r` forgets
    /// what was set so the server's own value (if any) is inherited again. `-g` is accepted: there
    /// is one environment for the whole server.
    pub(super) fn cmd_set_environment(&mut self, args: &[String]) -> Result<String, String> {
        let f = flags("set-environment", args, "", "gru")?;
        let (name, value) = match f.words.as_slice() {
            [name] if f.has('u') || f.has('r') => (name, None),
            [name, value] => (name, Some(value)),
            _ => return Err("usage: set-environment [-gru] name [value]".to_string()),
        };
        if !valid_name(name) {
            return Err(format!("set-environment: bad variable name {name:?}"));
        }
        if value.is_some_and(|v| v.contains('\0')) {
            return Err("set-environment: the value has a NUL in it".to_string());
        }
        if f.has('r') {
            self.env_remove(name);
        } else if f.has('u') {
            self.env_put(name, None);
        } else {
            self.env_put(name, value.cloned());
        }
        Ok(String::new())
    }

    /// `show-environment [-g] [name]`: `NAME=value`, and `-NAME` for a variable new panes do not get.
    pub(super) fn cmd_show_environment(&self, args: &[String]) -> Result<String, String> {
        let f = flags("show-environment", args, "", "gs")?;
        let line = |k: &str, v: &Option<String>| match v {
            Some(v) => format!("{k}={v}\n"),
            None => format!("-{k}\n"),
        };
        match f.words.as_slice() {
            [] => Ok(self.pane_env.iter().map(|(k, v)| line(k, v)).collect()),
            [name] => match self.pane_env.iter().find(|(k, _)| k == name) {
                Some((k, v)) => Ok(line(k, v)),
                None => match std::env::var(name) {
                    Ok(v) => Ok(format!("{name}={v}\n")),
                    Err(_) => Err(format!("unknown variable: {name}")),
                },
            },
            _ => Err("usage: show-environment [-g] [name]".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn listed_names_and_starts_are_stripped() {
        let inherited = names(&["PATH", "KITTY_WINDOW_ID", "ZELLIJ", "ZELLIJ_SESSION_NAME", "CLAUDE_CODE_USE_BEDROCK", "CLAUDECODE"]);
        let out = stripped_names("KITTY_WINDOW_ID ZELLIJ* CLAUDECODE", &inherited, &|_| false);
        assert_eq!(out, names(&["KITTY_WINDOW_ID", "ZELLIJ", "ZELLIJ_SESSION_NAME", "CLAUDECODE"]));
    }

    #[test]
    fn a_value_the_pane_was_given_is_kept() {
        let out = stripped_names("A B", &names(&["A", "B"]), &|n| n == "B");
        assert_eq!(out, names(&["A"]));
    }

    #[test]
    fn an_empty_list_strips_nothing_and_odd_words_are_ignored() {
        let inherited = names(&["A", "B"]);
        assert!(stripped_names("", &inherited, &|_| false).is_empty());
        assert!(stripped_names("A=1 * ", &inherited, &|_| false).is_empty());
    }

    #[test]
    fn window_goes_with_sty() {
        let inherited = names(&["STY", "WINDOW"]);
        assert_eq!(stripped_names("STY", &inherited, &|_| false), names(&["STY", "WINDOW"]));
        assert_eq!(stripped_names("STY", &names(&["WINDOW"]), &|_| false), Vec::<String>::new());
        assert_eq!(stripped_names("STY", &inherited, &|n| n == "WINDOW"), names(&["STY"]));
    }
}
