//! strip-environment: per-window identity and agent-session variables of the launch terminal are
//! not handed to new panes; capability hints and other settings still are; set-environment and an
//! emptied option put things back. (The Rust version of tests/q_stripenv.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_stripenv() {
    let mut e = Env::new("qse");
    let mut t = Check::new("q_stripenv");
    let conf = e.write("imux.conf", "");
    e.set("IMUX_CONFIG", &p(&conf));
    let d = p(&e.dir);
    let launch = |config: &str| {
        e.write("imux.conf", &format!("{config}\n"));
        e.kill_server();
        let _ = std::fs::remove_file(e.path("env.out"));
        e.run_env(
            &["new-session", "-d", "-s", "se", &format!("env >{d}/env.out; sleep 20")],
            &[
                ("ITERM_SESSION_ID", "w0t0p0"), ("KITTY_WINDOW_ID", "7"), ("ZELLIJ_SESSION_NAME", "z"), ("STY", "123.pts"), ("WINDOW", "3"), ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_SESSION_ID", "abc"), ("CLAUDE_CODE_USE_BEDROCK", "1"), ("CLAUDE_CONFIG_DIR", "/tmp/cfg"), ("TERM_PROGRAM", "iTerm.app"),
                ("LC_TERMINAL", "iTerm2"), ("MYVAR_ONE", "1"),
            ],
        );
        wait_until(Duration::from_secs(10), || !e.read("env.out").is_empty());
    };
    let has = |name: &str| e.read("env.out").lines().any(|l| l.starts_with(&format!("{name}=")));
    launch("set -g @none 1");
    t.ok("ITERM_SESSION_ID is not handed on", !has("ITERM_SESSION_ID"));
    t.ok("KITTY_WINDOW_ID is not handed on", !has("KITTY_WINDOW_ID"));
    t.ok("ZELLIJ* is not handed on", !has("ZELLIJ_SESSION_NAME"));
    t.ok("STY is not handed on", !has("STY"));
    t.ok("WINDOW goes with STY", !has("WINDOW"));
    t.ok("CLAUDECODE is not handed on", !has("CLAUDECODE"));
    t.ok("CLAUDE_CODE_SESSION_ID is not handed on", !has("CLAUDE_CODE_SESSION_ID"));
    t.ok("TERM_PROGRAM stays", has("TERM_PROGRAM"));
    t.ok("LC_TERMINAL stays", has("LC_TERMINAL"));
    t.ok("CLAUDE_CODE_USE_BEDROCK stays", has("CLAUDE_CODE_USE_BEDROCK"));
    t.ok("CLAUDE_CONFIG_DIR stays", has("CLAUDE_CONFIG_DIR"));
    t.ok("an unlisted variable stays", has("MYVAR_ONE"));
    t.ok("IMUX_ENV is set", has("IMUX_ENV"));
    launch("set -g strip-environment \"\"");
    t.ok("an empty option strips nothing", has("ITERM_SESSION_ID") && has("CLAUDECODE") && has("STY"));
    launch("set -g strip-environment \"MYVAR_*\"");
    t.ok("a start with a star is stripped", !has("MYVAR_ONE"));
    t.ok("and the defaults are replaced, not added to", has("ITERM_SESSION_ID"));
    launch("set -g @none 1");
    e.run(&["set-environment", "ITERM_SESSION_ID", "mine"]);
    let _ = std::fs::remove_file(e.path("env.out"));
    e.run(&["new-window", "-d", "-t", "se:", &format!("env >{d}/env.out; sleep 20")]);
    wait_until(Duration::from_secs(10), || !e.read("env.out").is_empty());
    t.ok("set-environment puts a stripped name back", e.read("env.out").lines().any(|l| l == "ITERM_SESSION_ID=mine"));
    t.ok("the others are still stripped", !has("KITTY_WINDOW_ID"));
    launch("set-environment -g ITERM_SESSION_ID fromconf");
    t.ok("set-environment in the configuration reaches the first pane", e.read("env.out").lines().any(|l| l == "ITERM_SESSION_ID=fromconf"));
    e.kill_server();
    t.finish();
}
