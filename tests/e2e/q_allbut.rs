//! The "all but this" and "keep where I am" flags: `kill-session -a`, `kill-window -a` and
//! `break-pane -d` from a bound key, `display-panes -d` and `detach-client -a` as bindings.
//! (The Rust version of tests/q_allbut.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_allbut() {
    let mut e = Env::new("qab");
    let mut t = Check::new("q_allbut");
    let conf = e.write("imux.conf", "bind K kill-window -a\nbind B break-pane -d\nbind P display-panes -d 1500\nbind D detach-client -a\nbind X kill-session -a\n");
    e.set("IMUX_CONFIG", conf.to_str().unwrap());
    e.session("keep", "one");
    e.run(&["new-session", "-d", "-s", "other1"]);
    e.run(&["new-session", "-d", "-s", "other2"]);
    let keys = e.out(&["list-keys"]);
    t.ok(
        "the bindings loaded and describe themselves",
        ["kill-window -a", "break-pane -d", "display-panes -d 1500", "detach-client -a", "kill-session -a"].iter().all(|k| keys.contains(k)),
    );
    e.run(&["kill-session", "-a", "-t", "keep"]);
    let sessions = || e.out(&["list-sessions", "-F", "#{session_name}"]).replace('\n', " ");
    t.ok("kill-session -a -t keeps the target and ends the rest", wait_until(Duration::from_secs(5), || sessions() == "keep"));
    e.run(&["new-window", "-t", "keep", "-n", "two"]);
    e.run(&["new-window", "-t", "keep", "-n", "three"]);
    e.run(&["select-window", "-t", "keep:1"]);
    e.run(&["split-window", "-d", "-t", "keep:1"]);
    let count = || e.out(&["list-windows", "-t", "keep", "-F", "#{window_name}"]).lines().count();
    let current = || e.out(&["display-message", "-p", "-t", "keep:", "#{window_name}"]);
    t.ok("three windows, the second current", count() == 3 && current() == "two");
    e.with_client("keep", |c| {
        c.send(b"\x02B");
        wait_until(Duration::from_secs(5), || count() == 4);
    });
    t.ok("break-pane -d makes a window and stays in the current one", count() == 4 && current() == "two");
    e.with_client("keep", |c| {
        c.send(b"\x02K");
        wait_until(Duration::from_secs(5), || count() == 1);
    });
    t.ok("kill-window -a ends every window but the current one", count() == 1 && current() == "two");
    e.run(&["new-session", "-d", "-s", "extra"]);
    e.with_client("keep", |c| {
        c.send(b"\x02X");
        wait_until(Duration::from_secs(5), || sessions() == "keep");
    });
    t.ok("kill-session -a from a key ends the other sessions", sessions() == "keep");
    e.kill_server();
    t.finish();
}
