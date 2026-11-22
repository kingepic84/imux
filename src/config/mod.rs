//! The configuration file: `~/.imux.conf` in tmux syntax, read at server start and by `source-file`.

pub mod cond;
pub mod format;
pub mod lex;
pub mod options;
pub mod stmt;

use std::path::PathBuf;

/// `$IMUX_CONFIG`, else `~/.imux.conf`, else `~/.config/imux/imux.conf`; `None` when none exists.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("IMUX_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [home.join(".imux.conf"), home.join(".config/imux/imux.conf")].into_iter().find(|p| p.exists())
}

/// The options a configuration text sets with plain `set` lines, for the client, which has to
/// know a few of them (such as `image-probe-timeout`) before it talks to the server. Lines that
/// are not a `set`, and mistakes, are left to the server to report; `source-file` is not followed.
pub fn options_from_text(text: &str) -> options::Options {
    let (mut vars, mut options) = (lex::Vars::new(), options::Options::new());
    for (_, line) in lex::logical_lines(text) {
        let Ok(stmts) = stmt::parse_line(&line, &mut vars) else { continue };
        for s in stmts {
            if let stmt::Stmt::Set { name, value, append } = s {
                let _ = options.set(&name, value.as_deref(), append);
            }
        }
    }
    options
}

/// `options_from_text` for the configuration file in use; the defaults when there is none.
pub fn client_options() -> options::Options {
    match default_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(text) => options_from_text(&text),
        None => options::Options::new(),
    }
}

#[cfg(test)]
mod client_tests {
    use super::*;

    #[test]
    fn the_client_reads_set_lines_and_skips_the_rest() {
        let o = options_from_text("# comment\nset image-probe-timeout 1500\nbind x new-window\nset bogus-option 1\nset -g mouse off\n");
        assert_eq!(o.num("image-probe-timeout"), 1500);
        assert!(!o.flag("mouse"));
    }

    #[test]
    fn a_bad_value_leaves_the_default() {
        assert_eq!(options_from_text("set image-probe-timeout soon\n").num("image-probe-timeout"), 600);
        assert_eq!(options_from_text("").num("image-probe-timeout"), 600);
    }
}

#[cfg(test)]
mod tests {
    use super::lex::{self, Vars};
    use super::options::Options;
    use super::stmt::{self, Stmt};

    #[test]
    fn the_example_conf_loads_without_errors() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/example.imux.conf")).unwrap();
        let (mut vars, mut options, mut errors, mut sets) = (Vars::new(), Options::new(), Vec::new(), 0);
        for (n, line) in lex::logical_lines(&text) {
            match stmt::parse_line(&line, &mut vars) {
                Ok(stmts) => {
                    for s in stmts {
                        if let Stmt::Set { name, value, append } = s {
                            sets += 1;
                            if let Err(e) = options.set(&name, value.as_deref(), append) {
                                errors.push(format!("line {n}: {e}"));
                            }
                        }
                    }
                }
                Err(e) => errors.push(format!("line {n}: {e}")),
            }
        }
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(sets > 20, "only {sets} set statements");
    }
}
