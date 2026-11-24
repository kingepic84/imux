//! A restore may replace a session of the same name that nobody has used (what a new server starts
//! with, so a saved session called `0` can come back after `kill-server`), and the clients in it
//! follow; a session that has been used is still left alone.
//! (Found on a real run: `C-r` answered "1 left alone (name in use)" against the empty session.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

#[test]
fn q_restoreempty() {
    let mut e = Env::new("qre");
    e.set("IMUX_CONFIG", "/dev/null");
    let mut t = Check::new("q_restoreempty");

    t.section("a saved session of two windows");
    let mut c = e.client(100, 30, &["new", "-s", "zero"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c.send(b"\x02c");
    t.ok("a second window is made", wait_until(LONG, || e.all(&["list-windows", "-t", "zero"]).lines().count() == 2));
    let saved = e.all(&["save", "snap"]);
    t.matches("it is saved", "saved 1 session as snap", &saved);
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();

    t.section("an empty session of that name in the new server");
    let mut c = e.client(100, 30, &["new", "-s", "zero"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    t.ok("the new server has one session of one window", e.all(&["list-windows", "-t", "zero"]).lines().count() == 1);
    let r = e.all(&["restore", "snap"]);
    t.matches("restore brings the saved one back", "restored 1 session", &r);
    t.lacks("and leaves nothing alone", "left alone", &r);
    t.ok("the session has its two windows", e.all(&["list-windows", "-t", "zero"]).lines().count() == 2);
    t.ok("and there is one session of that name", e.all(&["ls"]).lines().filter(|l| l.starts_with("zero:")).count() == 1);
    t.ok("the client is still there", e.has_client());
    c.send_str("echo after-$((20+22))\r");
    t.ok("and works in it", c.wait_seen("after-42", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();

    t.section("a shell that printed a lot by itself is still unused");
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".profile"), "for i in 1 2 3 4 5 6 7 8; do echo greeting-$i; done\nPS1='one\\ntwo\\nthree> '\n").unwrap();
    e.set("HOME", &p(&home));
    let conf = e.write("imux.conf", "set -g default-shell /bin/bash\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut c = e.client(100, 30, &["new", "-s", "zero"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    t.ok("the shell greeted by itself", c.wait_seen("greeting-8", LONG));
    let r = e.all(&["restore", "snap"]);
    t.matches("restore still replaces it", "restored 1 session", &r);
    t.lacks("and leaves nothing alone", "left alone", &r);
    t.ok("the session has its two windows", e.all(&["list-windows", "-t", "zero"]).lines().count() == 2);
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    e.set("IMUX_CONFIG", "/dev/null");

    t.section("a session that was used");
    let mut c = e.client(100, 30, &["new", "-s", "zero"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c.send_str("echo used-$((20+22))\r");
    t.ok("something is run in it", c.wait_seen("used-42", LONG));
    let r = e.all(&["restore", "snap"]);
    t.matches("restore leaves it alone and says so", "restored 0 sessions, 1 left alone", &r);
    t.ok("it keeps its one window", e.all(&["list-windows", "-t", "zero"]).lines().count() == 1);
    t.ok("and what was run is still on the screen", c.wait_seen("used-42", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    t.finish();
}
