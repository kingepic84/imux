//! The tmux-compatible commands and flags that were gaps: set -q/-o, directional targets, =exact,
//! split-window -l/-p, respawn-pane, join-pane, wait-for, pipe-pane, command-alias, capture-pane
//! -e/-N, list-keys -T, kill-pane/kill-window -a and show-options.
//! (The Rust version of tests/b_compat.sh.)
use crate::support::*;
use std::process::{Child, Stdio};
use std::time::Duration;

fn spawn(e: &Env, args: &[&str]) -> Child {
    e.command(BIN).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
}

/// A command that waits in the server: its connection has been taken, and one more request has gone
/// through the server after it (so a wait that was going to answer at once has been answered).
fn spawn_waiting(e: &Env, args: &[&str]) -> Child {
    let connections = || e.server_log().matches(" connected\"}").count();
    let before = connections();
    let child = spawn(e, args);
    assert!(wait_until(Duration::from_secs(10), || connections() > before), "the server never took the connection of {args:?}");
    e.run(&["list-sessions"]);
    child
}

fn ended(c: &mut Child, secs: u64) -> bool {
    wait_until(Duration::from_secs(secs), || matches!(c.try_wait(), Ok(Some(_))))
}

fn running(c: &mut Child) -> bool {
    matches!(c.try_wait(), Ok(None))
}

#[test]
fn b_compat() {
    let e = Env::new("bcompat");
    let mut t = Check::new("b_compat");
    let panes_of = |t: &str| e.out(&["list-panes", "-t", t, "-F", "#{pane_id}"]).lines().count();
    let windows_of = |t: &str| e.out(&["list-windows", "-t", t]).lines().count();
    let d = p(&e.dir);
    e.run(&["new-session", "-d", "-s", "base"]);

    t.section("set -q, -o and the scope flags; show-options");
    e.run(&["set", "-g", "mouse", "off"]);
    t.eq("show-options -v prints the value", e.out(&["show-options", "-v", "mouse"]), "off".to_string());
    e.run(&["set", "-o", "mouse", "on"]);
    t.eq("set -o leaves an option that was set", e.out(&["show-options", "-v", "mouse"]), "off".to_string());
    e.run(&["set", "-o", "escape-time", "20"]);
    t.eq("set -o sets an option nobody set", e.out(&["show-options", "-v", "escape-time"]), "20".to_string());
    t.ok("set -q ignores an unknown option", e.run(&["set", "-q", "no-such-option", "1"]).code == 0);
    let r = e.run(&["set", "no-such-option", "1"]);
    t.ok("without -q it is an error", r.code != 0 && r.all().contains("unknown option"));
    e.run(&["set", "-w", "mouse", "on"]);
    e.run(&["set", "-s", "escape-time", "15"]);
    t.eq("-w is accepted", e.out(&["show-options", "-v", "mouse"]), "on".to_string());
    let all = e.all(&["show-options"]);
    t.matches("show-options lists every option", "(?m)^history-limit 50000$", &all);
    t.matches("with quoting where a value needs it", "(?m)^status-right \".*\"$", &all);
    t.ok("an unknown option is an error", e.run(&["show-options", "nothing-here"]).code != 0);
    let r = e.run(&["show-options", "-q", "nothing-here"]);
    t.ok("-q makes it silent", r.code == 0 && r.all().is_empty());

    t.section("directional, edge and exact targets");
    e.run(&["new-session", "-d", "-s", "work", "-n", "one"]);
    e.run(&["new-session", "-d", "-s", "work2"]);
    t.ok("=name matches exactly", e.run(&["has-session", "-t", "=work"]).code == 0);
    t.ok("=name does not match a prefix", e.run(&["has-session", "-t", "=wor"]).code != 0);
    e.run(&["split-window", "-h", "-t", "work:0"]);
    let ids = e.out(&["list-panes", "-t", "work:0", "-F", "#{pane_id}"]);
    let (left, right) = (ids.lines().next().unwrap_or("").to_string(), ids.lines().nth(1).unwrap_or("").to_string());
    t.eq("{left} is the leftmost pane", e.out(&["display-message", "-p", "-t", "work:0.{left}", "#{pane_id}"]), left.clone());
    t.eq("{right} is the rightmost pane", e.out(&["display-message", "-p", "-t", "work:0.{right}", "#{pane_id}"]), right.clone());
    t.eq("{left-of} is beside the active pane", e.out(&["display-message", "-p", "-t", "work:0.{left-of}", "#{pane_id}"]), left.clone());
    let r = e.run(&["display-message", "-p", "-t", "work:0.{right-of}", "#{pane_id}"]);
    t.ok("{right-of} with nothing there is an error", r.code != 0 && regex_lite::Regex::new("can.t find pane").unwrap().is_match(&r.all()));
    t.eq("a corner names the pane that covers it", e.out(&["display-message", "-p", "-t", "work:0.{top-right}", "#{pane_id}"]), right.clone());

    t.section("split-window -l and -p");
    e.run(&["new-session", "-d", "-s", "sz"]);
    t.eq("-l is cells", e.out(&["split-window", "-h", "-l", "20", "-t", "sz:0", "-P", "-F", "#{pane_width}"]), "20".to_string());
    e.run(&["new-session", "-d", "-s", "sz2"]);
    t.eq("-l works for a stacked split", e.out(&["split-window", "-v", "-l", "5", "-t", "sz2:0", "-P", "-F", "#{pane_height}"]), "5".to_string());
    e.run(&["new-session", "-d", "-s", "sz3"]);
    t.matches("-p is a share of the room", "^(39|40)$", &e.out(&["split-window", "-h", "-p", "50", "-t", "sz3:0", "-P", "-F", "#{pane_width}"]));
    t.matches("-l takes a percentage too", "^(9|10|11)$", &e.out(&["split-window", "-h", "-l", "25%", "-t", "sz3:0.0", "-P", "-F", "#{pane_width}"]));
    let r = e.run(&["split-window", "-l", "ten", "-t", "sz3:0"]);
    t.ok("a bad size is an error", r.code != 0 && r.all().contains("bad size"));

    t.section("respawn-pane");
    e.run(&["new-session", "-d", "-s", "rs", "echo first-run; sleep 60"]);
    e.shows("rs:", "^first-run$");
    let id1 = e.var("rs:", "#{pane_id}");
    let r = e.run(&["respawn-pane", "-t", "rs:"]);
    t.ok("a running pane needs -k", r.code != 0 && r.all().contains("-k"));
    let r = e.run(&["respawn-pane", "-k", "-t", "rs:", "echo second-run-$((2+3)); sleep 60"]);
    t.ok("respawn-pane -k succeeds", r.code == 0);
    t.ok("the new command runs", e.shows("rs:", "^second-run-5$"));
    t.ok("the old output is gone", !e.capture("rs:").lines().any(|l| l.starts_with("first-run")));
    t.eq("the pane keeps its number", e.var("rs:", "#{pane_id}"), id1);
    e.run(&["respawn-window", "-k", "-t", "rs:", "echo window-again; sleep 60"]);
    t.ok("respawn-window does a window of one pane", e.shows("rs:", "^window-again$"));

    t.section("join-pane and move-pane");
    e.run(&["new-session", "-d", "-s", "jn", "-n", "a"]);
    e.run(&["new-window", "-t", "jn", "-n", "b"]);
    e.run(&["new-window", "-t", "jn", "-n", "c"]);
    t.ok("three windows to start", windows_of("jn") == 3);
    t.ok("join-pane succeeds", e.run(&["join-pane", "-s", "jn:b", "-t", "jn:a", "-h"]).code == 0);
    t.ok("the emptied window is gone", windows_of("jn") == 2);
    t.ok("and its pane sits in the target window", panes_of("jn:a") == 2);
    t.ok("a pane cannot join its own window", e.run(&["join-pane", "-s", "jn:a.0", "-t", "jn:a.1"]).code != 0);
    let r = e.run(&["move-pane", "-s", "jn:a.1", "-t", "jn:c", "-v", "-l", "5"]);
    t.ok("move-pane takes a size", r.code == 0 && panes_of("jn:c") == 2 && panes_of("jn:a") == 1);
    e.run(&["new-session", "-d", "-s", "only"]);
    t.ok("the only pane of the only window cannot leave", e.run(&["join-pane", "-s", "only:", "-t", "jn:a"]).code != 0);

    t.section("wait-for");
    let mut w1 = spawn_waiting(&e, &["wait-for", "chan1"]);
    t.ok("a wait blocks", running(&mut w1));
    e.run(&["wait-for", "-S", "chan1"]);
    t.ok("a signal lets it go", ended(&mut w1, 5));
    e.run(&["wait-for", "-S", "early"]);
    t.ok("a signal sent before the wait is kept for it", e.run_for(&["wait-for", "early"], Duration::from_secs(10)).code == 0);
    e.run(&["wait-for", "-L", "mtx"]);
    let mut w2 = spawn_waiting(&e, &["wait-for", "-L", "mtx"]);
    t.ok("a second lock waits", running(&mut w2));
    e.run(&["wait-for", "-U", "mtx"]);
    t.ok("unlocking hands the lock on", ended(&mut w2, 5));
    e.run(&["wait-for", "-U", "mtx"]);
    let r = e.run(&["wait-for", "-U", "mtx"]);
    t.ok("unlocking a free channel is an error", r.code != 0 && r.all().contains("not locked"));
    t.ok("a bad channel name is an error", e.run(&["wait-for", "bad name"]).code != 0);
    e.run(&["new-session", "-d", "-s", "wf"]);
    e.ready("wf:", Duration::from_millis(500));
    let mut w3 = spawn_waiting(&e, &["wait-for", "from-pane"]);
    e.run(&["send-keys", "-t", "wf:", &format!("{BIN} wait-for -S from-pane"), "Enter"]);
    t.ok("a program in a pane can signal", ended(&mut w3, 10));

    t.section("pipe-pane");
    e.run(&["new-session", "-d", "-s", "pp"]);
    e.ready("pp:", Duration::from_millis(500));
    e.run(&["pipe-pane", "-t", "pp:", &format!("cat > {d}/piped.txt")]);
    e.run(&["send-keys", "-t", "pp:", "echo piped-marker-1", "Enter"]);
    let piped = |m: &str| wait_until(Duration::from_secs(10), || e.read("piped.txt").contains(m));
    t.ok("pane output reaches the command", piped("piped-marker-1"));
    e.run(&["pipe-pane", "-o", "-t", "pp:", &format!("cat > {d}/ignored.txt")]);
    e.run(&["send-keys", "-t", "pp:", "echo piped-marker-2", "Enter"]);
    t.ok("-o leaves the pipe that is open", piped("piped-marker-2") && !e.path("ignored.txt").exists());
    e.run(&["pipe-pane", "-t", "pp:"]);
    e.run(&["send-keys", "-t", "pp:", "echo after-close", "Enter"]);
    // the pane shows the text; whatever was still piped would have been written by now
    assert!(e.shows("pp:", "^after-close$"), "the pane did not show its text");
    t.ok("no command closes the pipe", !e.read("piped.txt").contains("after-close"));
    e.run(&["pipe-pane", "-I", "-t", "pp:", "printf 'echo typed-by-pipe-%s\\n' 7"]);
    t.ok("-I types the output of the command into the pane", e.shows("pp:", "^typed-by-pipe-7$"));

    t.section("command-alias");
    e.run(&["set", "-g", "command-alias", "ll=list-sessions -F '#{session_name}'"]);
    t.matches("an alias runs its command", "(?m)^base$", &e.all(&["ll"]));
    e.run(&["set", "-g", "command-alias[7]", "lp=list-panes -a -F \"#{pane_id}\""]);
    t.matches("the indexed spelling works", "(?m)^%[0-9]+$", &e.all(&["lp"]));
    let r = e.run(&["set", "-g", "command-alias", "list-panes=list-sessions"]);
    t.ok("an alias cannot hide a command", r.code != 0 && r.all().contains("is a command already"));
    e.run(&["set", "-gu", "command-alias[7]"]);
    t.ok("unsetting a slot removes its alias", e.run(&["lp"]).code != 0);
    t.ok("the others stay", e.run(&["ll"]).code == 0);
    t.ok("a bad alias name is an error", e.run(&["set", "-g", "command-alias", "bad name=x"]).code != 0);

    t.section("capture-pane -e and -N");
    e.run(&["new-session", "-d", "-s", "cp"]);
    e.ready("cp:", Duration::from_millis(500));
    e.run(&["send-keys", "-t", "cp:", "printf '\\033[31mred-text\\033[0m plain\\n'", "Enter"]);
    t.ok("colors are there", wait_until(Duration::from_secs(10), || e.out(&["capture-pane", "-p", "-e", "-t", "cp:"]).contains("red-text")));
    t.matches("-e writes the escape sequence for them", "\x1b\\[0;31mred-text", &e.out(&["capture-pane", "-p", "-e", "-t", "cp:"]));
    t.ok("without -e there are none", !e.out(&["capture-pane", "-p", "-t", "cp:"]).contains('\x1b'));
    let n = e.run(&["capture-pane", "-p", "-N", "-t", "cp:"]).out;
    t.ok("-N keeps the blanks at the end of the lines", n.lines().map(|l| l.chars().count()).max().unwrap_or(0) >= 79);

    t.section("list-keys -T");
    let k = e.out(&["list-keys", "-T", "copy-mode-vi"]);
    t.ok("only the table asked for", !k.is_empty() && k.lines().all(|l| l.contains("-T copy-mode-vi")));
    let r = e.run(&["list-keys", "-T", "nonsense"]);
    t.ok("an unknown table is an error", r.code != 0 && r.all().contains("unknown key table"));

    t.section("kill-pane -a and kill-window -a");
    e.run(&["new-session", "-d", "-s", "kl", "-n", "k1"]);
    e.run(&["split-window", "-t", "kl:0"]);
    e.run(&["split-window", "-t", "kl:0"]);
    t.ok("three panes", panes_of("kl:0") == 3);
    e.run(&["kill-pane", "-a", "-t", "kl:0.0"]);
    t.ok("kill-pane -a keeps only the target", wait_until(Duration::from_secs(5), || panes_of("kl:0") == 1));
    e.run(&["new-window", "-t", "kl", "-n", "k2"]);
    e.run(&["new-window", "-t", "kl", "-n", "k3"]);
    e.run(&["kill-window", "-a", "-t", "kl:k2"]);
    t.ok("kill-window -a keeps only the target", wait_until(Duration::from_secs(5), || windows_of("kl") == 1));
    t.eq("and it is the one named", e.out(&["list-windows", "-t", "kl", "-F", "#{window_name}"]), "k2".to_string());
    e.kill_server();
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
