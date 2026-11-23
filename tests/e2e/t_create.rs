//! new-session -d starts the server and needs no terminal; new-window and split-window flags; a
//! program in a pane cannot create things; options and bindings from the shell.
//! (The Rust version of tests/t_create.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn t_create() {
    let e = Env::new("tcreate");
    let mut t = Check::new("t_create");
    let d = p(&e.dir);

    t.section("new-session -d starts the server and needs no terminal");
    t.ok("new-session -d succeeds with no server running", e.run(&["new-session", "-d", "-s", "a", "-n", "win1"]).success());
    t.matches("the session exists", "(?m)^a: 1 window$", &e.all(&["list-sessions"]));
    t.matches("-n names the window", "(?m)^win1$", &e.all(&["list-windows", "-t", "a", "-F", "#{window_name}"]));
    let r = e.run(&["new-session", "-d", "-s", "a"]);
    t.ok("a second session of the same name is refused", !r.success() && r.all().contains("duplicate session: a"));
    let r = e.run(&["new-session", "-s", "z"]);
    t.ok("without -d it says how to attach", !r.success() && r.all().contains("-d"));
    e.run(&["new-session", "-d", "-s", "c", "echo hi-$((1+2)); sleep 20"]);
    t.ok("a command is run instead of a shell prompt", e.shows("c:", "^hi-3$"));
    t.matches("-P prints the new session with -F", "(?m)^p:0$", &e.all(&["new-session", "-d", "-s", "p", "-P", "-F", "#{session_name}:#{window_index}"]));
    let r = e.run(&["new-session", "-d", "-s", "q", "-c", "/nonexistent-dir"]);
    t.ok("-c with a missing directory is an error", !r.success() && r.all().contains("not a directory"));

    t.section("new-window");
    t.matches("new-window -n -P prints the name", "(?m)^second$", &e.all(&["new-window", "-t", "a", "-n", "second", "-P", "-F", "#{window_name}"]));
    e.run(&["new-window", "-t", "a", "-d", "-n", "third"]);
    let lw = e.all(&["list-windows", "-t", "a", "-F", "#{window_index}#{window_name}#{window_active}"]);
    t.matches("a new window is selected", "(?m)^1second1$", &lw);
    t.matches("with -d it is not", "(?m)^2third0$", &lw);
    e.run(&["new-window", "-t", "a", "-n", "multi", "sh", "-c", "echo multi-$((2+2)); sleep 20"]);
    t.ok("several words are quoted for the shell", e.shows("a:multi", "^multi-4$"));
    t.ok("a missing session is an error", !e.run(&["new-window", "-t", "nosuch"]).success());

    t.section("split-window");
    t.matches("split-window prints the new pane", "(?m)^%[0-9]+$", &e.all(&["split-window", "-t", "a:0", "-h", "-P", "-F", "#{pane_id}"]));
    let lp = e.all(&["list-panes", "-t", "a:0", "-F", "#{pane_id}#{pane_active}"]);
    t.ok("the window has two panes", lp.lines().count() == 2);
    t.matches("the new pane is the active one", "(?m)^%[0-9]+1$", &lp);
    let new = e.out(&["split-window", "-t", "a:0", "-d", "-P", "-F", "#{pane_id}"]);
    let lp = e.all(&["list-panes", "-t", "a:0", "-F", "#{pane_active}#{pane_id}"]);
    t.ok("with -d the focus stays", lp.lines().filter(|l| l.starts_with('1')).count() == 1 && lp.lines().count() == 3);
    let cwd_pane = e.out(&["split-window", "-t", "a:0", "-c", "/tmp", "-P", "-F", "#{pane_id}"]);
    t.ok("-c sets the directory", wait_until(Duration::from_secs(10), || e.var(&cwd_pane, "#{pane_current_path}").ends_with("tmp")));
    let r = e.run(&["split-window", "-t", "a:0", "-l", "ten"]);
    t.ok("a bad size is refused, not ignored", !r.success() && r.all().contains("bad size"));
    e.run(&["kill-pane", "-t", &new]);

    t.section("a program in a pane cannot create things");
    e.run(&["set", "-g", "command-from-panes", "off"]);
    e.run(&["new-window", "-t", "a", "-n", "spawned", "sh", "-c", "sleep 1"]);
    e.ready("a:win1", Duration::from_millis(400));
    e.run(&["send-keys", "-t", "a:win1", &format!("{BIN} new-window -t a -n evil > {d}/evil.txt 2>&1"), "Enter"]);
    t.ok("new-window from inside a pane is refused", wait_until(Duration::from_secs(20), || e.read("evil.txt").contains("not allowed from inside a pane")));
    t.ok("and no window was made", !e.out(&["list-windows", "-t", "a", "-F", "#{window_name}"]).lines().any(|l| l == "evil"));

    t.section("options and bindings from the shell");
    t.ok("set-option works", e.run(&["set-option", "-g", "clock-mode-style", "12"]).success());
    let r = e.run(&["set", "-g", "no-such-option", "1"]);
    t.ok("an unknown option is an error", !r.success() && r.all().contains("unknown option"));
    e.run(&["bind-key", "X", "new-window"]);
    t.matches("bind-key puts the key in the table", "(?m)^bind-key -T prefix X +new-window$", &e.all(&["list-keys"]));
    e.run(&["unbind-key", "X"]);
    t.lacks("unbind-key takes it out", "(?m)^bind-key -T prefix X ", &e.all(&["list-keys"]));
    e.kill_server();
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
