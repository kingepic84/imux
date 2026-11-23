//! prefix2: a second key that starts a command like the prefix, `set prefix2 None` takes it away, and
//! `send-prefix -2` types it into the pane. Also `BTab`, tmux's name of Shift-Tab, as a bound key.
//! (The Rust version of tests/q_prefix2.sh.)
use crate::support::*;
use std::time::Duration;

fn windows(e: &Env) -> usize {
    e.out(&["list-windows", "-t", "s"]).lines().count()
}

/// A client that presses `keys` and goes away. The detach key goes after them on the same
/// connection, so the server has handled `keys` first.
fn keys(e: &Env, bytes: &[u8]) {
    e.with_client("s", |c| {
        c.send(bytes);
        c.sync();
    });
}

#[test]
fn q_prefix2() {
    let e = Env::new("qp2");
    let mut t = Check::new("q_prefix2");
    e.session("s", "one");
    t.ok("show-options has prefix2 off by default", e.all(&["show-options", "prefix2"]).lines().any(|l| l == "prefix2 None"));
    keys(&e, b"\x01c");
    t.ok("without prefix2, C-a is not a prefix: no new window", windows(&e) == 1);
    e.run(&["set", "-g", "prefix2", "C-a"]);
    keys(&e, b"\x01c");
    t.ok("with prefix2 C-a, C-a c makes a window", wait_until(Duration::from_secs(5), || windows(&e) == 2));
    keys(&e, b"\x02c");
    t.ok("the first prefix still works", wait_until(Duration::from_secs(5), || windows(&e) == 3));
    e.run(&["bind-key", "x", "send-prefix", "-2"]);
    e.run(&["send-keys", "-t", "s:0", "C-u", "cat -v", "Enter"]);
    e.run(&["select-window", "-t", "s:0"]);
    assert!(wait_until(Duration::from_secs(10), || e.var("s:0", "#{pane_current_command}") == "cat"), "cat -v did not start in the pane");
    keys(&e, b"\x02x");
    t.ok("send-prefix -2 types the second prefix into the pane", wait_until(Duration::from_secs(5), || e.capture("s:0").contains("^A")));
    e.run(&["set", "-g", "prefix2", "None"]);
    keys(&e, b"\x01c");
    t.ok("after prefix2 None, C-a starts nothing again", windows(&e) == 3);
    e.run(&["bind-key", "BTab", "new-window"]);
    keys(&e, b"\x02\x1b[Z");
    t.ok("BTab as a bound key matches Shift-Tab after the prefix", wait_until(Duration::from_secs(5), || windows(&e) == 4));
    e.kill_server();
    t.finish();
}
