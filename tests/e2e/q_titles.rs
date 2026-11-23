//! set-titles / set-titles-string: the terminal title a client is sent. Off by default; with it on
//! a title goes out at attach and again when the text changes, never twice in a row, and what a
//! pane put in its title cannot carry a control character into it.
//! (The Rust version of tests/q_titles.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_titles() {
    let e = Env::new("qti");
    let mut t = Check::new("q_titles");
    e.session("s", "one");
    t.section("off by default");
    t.ok("set-titles is off", e.all(&["show-options", "set-titles"]).lines().any(|l| l == "set-titles off"));
    let off = e.with_client("s", |c| e.barrier(c, "s"));
    t.ok("no title is sent", !off.contains("\x1b]2;"));
    t.section("on");
    e.run(&["set", "-g", "set-titles", "on"]);
    e.run(&["set", "-g", "set-titles-string", "TT:#S:#I:#W:#{window_index}"]);
    let on = e.with_client("s", |c| {
        c.wait_seen("\x1b]2;TT:s:0:one:0", Duration::from_secs(5));
        e.run(&["rename-window", "-t", "s:one", "two"]);
        c.wait_seen("\x1b]2;TT:s:0:two:0", Duration::from_secs(5));
    });
    t.ok("a title goes out at attach", on.contains("\x1b]2;TT:s:0:one:0\x1b\\"));
    t.ok("and a new one when the window is renamed", on.contains("\x1b]2;TT:s:0:two:0\x1b\\"));
    t.ok("the same title is not sent twice", on.matches("\x1b]2;TT:s:0:one:0").count() <= 1);
    t.section("a title from a pane");
    e.run(&["set", "-g", "set-titles-string", "[#T]"]);
    e.run(&["send-keys", "-t", "s:two", "printf \"\\033]2;pa\\342\\200\\256ne\\007\"", "Enter"]);
    wait_until(Duration::from_secs(10), || e.var("s:two", "#{pane_title}").contains("ne"));
    let pane = e.with_client("s", |c| {
        c.wait_seen("\x1b]2;[pane]", Duration::from_secs(5));
    });
    t.ok("the pane's title is in it, without the bidirectional override", pane.contains("\x1b]2;[pane]\x1b\\"));
    t.ok("and the override itself is nowhere in the output", !pane.contains('\u{202e}'));
    e.kill_server();
    t.finish();
}
