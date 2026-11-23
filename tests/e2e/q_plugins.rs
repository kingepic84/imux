//! What the common tmux plugins (tpm, sensible, resurrect, continuum) need from the command line:
//! command names given by their unique start (`show-option`), `start-server` through the shim,
//! `show-options -t window`, `new-window -t session:N`, a pane title that is never empty, the
//! `start_time` variable, and a `display-message` with nobody looking.
//! (The Rust version of tests/q_plugins.sh.)
use crate::support::*;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn shim(e: &Env, shim_dir: &Path, args: &[&str]) -> Res {
    let out = e.command(&p(&shim_dir.join("tmux"))).args(args).output().expect("the shim runs");
    Res { code: out.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&out.stdout).into_owned(), err: String::from_utf8_lossy(&out.stderr).into_owned() }
}

#[test]
fn q_plugins() {
    let mut e = Env::new("qpl");
    let mut t = Check::new("q_plugins");
    // No window name: a named window stops automatic-rename, and a check below needs it on.
    e.run(&["new-session", "-d", "-s", "pl"]);
    let socket = e.out(&["display-message", "-p", "#{socket_path}"]);
    let shims = Path::new(&socket).parent().unwrap().join("shims");
    e.set("TMUX", &format!("{socket},1,0"));
    e.run(&["set", "-g", "@who", "me"]);
    t.eq("show-option (no s) is show-options", e.out(&["show-option", "-gqv", "@who"]), "me".to_string());
    t.eq("a longer prefix works too", e.out(&["show-options", "-gqv", "@who"]), "me".to_string());
    t.ok("a prefix of several commands is ambiguous", e.all(&["list"]).contains("ambiguous command: list"));
    t.ok("a name that starts nothing is unknown", shim(&e, &shims, &["frobnicate"]).all().contains("unknown command: frobnicate"));
    t.ok("a very short prefix is not guessed", shim(&e, &shims, &["sh"]).all().contains("unknown command: sh"));
    t.ok("display-message with nobody attached succeeds", e.run(&["display-message", "hello"]).success());
    t.ok("a pane nobody titled has a title (the host name)", !e.out(&["display-message", "-p", "#{pane_title}"]).is_empty());
    let start: u64 = e.out(&["display-message", "-p", "#{start_time}"]).parse().unwrap_or(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    t.ok("start_time is the server start in epoch seconds", start <= now && start > now - 120);
    t.eq("show-options -t window reads automatic-rename of that window (on)", e.out(&["show-options", "-vt", "pl:0", "automatic-rename"]), "on".to_string());
    e.run(&["rename-window", "-t", "pl:0", "fixed"]);
    t.eq("off after the window was named", e.out(&["show-options", "-vt", "pl:0", "automatic-rename"]), "off".to_string());
    e.run(&["setw", "-t", "pl:0", "synchronize-panes", "on"]);
    t.eq("a window option reads its own value", e.out(&["show-options", "-vt", "pl:0", "synchronize-panes"]), "on".to_string());
    t.eq("with no override it reads the global one", e.out(&["show-options", "-gv", "synchronize-panes"]), "off".to_string());
    e.run(&["new-window", "-d", "-t", "pl:1", "-n", "two"]);
    let names = || e.out(&["list-windows", "-t", "pl", "-F", "#{window_name}"]).lines().map(|l| format!("{l} ")).collect::<String>();
    t.eq("new-window -t session:N makes the window when N is next", names(), "fixed two ".to_string());
    let r = e.run(&["new-window", "-d", "-t", "pl:1", "-n", "three"]);
    t.ok("a number in use is refused", r.all().contains("index 1 in use"));
    t.ok("and nothing was made", e.out(&["list-windows", "-t", "pl"]).lines().count() == 2);
    t.ok("the shim directory exists", shims.join("tmux").exists());
    let r = shim(&e, &shims, &["start-server"]);
    t.ok("tmux start-server through the shim does nothing and succeeds", r.code == 0 && r.all().is_empty());
    let r = shim(&e, &shims, &["start-server;", "show-options", "-gqv", "@who"]);
    t.eq("the form tpm uses (start-server with a glued semicolon) then a command", r.all().trim_end().to_string(), "me".to_string());
    e.kill_server();
    t.finish();
}
