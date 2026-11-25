//! `imux restart-server [--yes] [--detach]`: stops the server in the way that saves its sessions, so
//! that the next one (this imux) brings them back. It asks first (with what the server holds and what
//! is lost), takes --yes for scripts, refuses without a terminal or inside a pane, says so when no
//! server runs, and warns when autosave is off.
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

/// The process number `imux doctor` gives for the running server, if there is one.
fn server_pid(e: &Env) -> Option<String> {
    let out = e.all(&["doctor"]);
    out.lines().find_map(|l| l.strip_prefix("server: running (pid ")).and_then(|r| r.strip_suffix(')')).map(str::to_string)
}

#[test]
fn q_restartserver() {
    let mut e = Env::new("qrs2");
    let mut t = Check::new("q_restartserver");
    let conf = e.write("imux.conf", "set -g status off\n");
    e.set("IMUX_CONFIG", &p(&conf));

    t.section("no server is running");
    let r = e.run(&["restart-server", "--yes"]);
    t.ok("it says so and succeeds", r.code == 0 && r.out.contains("no server is running"));

    t.section("what it refuses");
    e.run(&["new-session", "-d", "-s", "work"]);
    e.run(&["new-window", "-d", "-t", "work"]);
    e.run(&["new-session", "-d", "-s", "other"]);
    let old = server_pid(&e);
    t.ok("a server runs", old.is_some());
    let r = e.run(&["restart-server"]);
    t.ok("without a terminal and without --yes", r.code == 1 && r.err.contains("asks before it stops the server"));
    t.ok("and the server is still there", server_pid(&e) == old);
    let r = e.run_env(&["restart-server", "--yes"], &[("IMUX_ENV", "1"), ("IMUX_PANE_ID", "1")]);
    t.ok("inside a pane, even with --yes", r.code == 1 && r.err.contains("runs inside an imux pane"));
    t.ok("the server is still there", server_pid(&e) == old);

    t.section("the question");
    let mut c = e.client(100, 30, &["restart-server"]);
    t.ok("it tells what the server holds", c.wait_seen("2 sessions with 3 windows", LONG));
    t.ok("and what is lost", c.seen("other programs end, and the scrollback is lost"));
    t.ok("and asks", c.wait_seen("Restart it\\? \\[y/N\\]", LONG));
    c.send(b"n\r");
    t.ok("no changes anything", c.wait_seen("nothing was changed", LONG));
    c.wait_exit(LONG);
    t.ok("the server is still there", server_pid(&e) == old);
    let mut c = e.client(100, 30, &["restart-server"]);
    t.ok("an empty answer is no too", c.wait_seen("Restart it\\? \\[y/N\\]", LONG));
    c.send(b"\r");
    t.ok("nothing was changed", c.wait_seen("nothing was changed", LONG));
    c.wait_exit(LONG);
    t.ok("the server is still there after that", server_pid(&e) == old);

    t.section("yes, with a terminal: the server is replaced and the client attaches to the sessions");
    let mut c = e.client(100, 30, &["restart-server"]);
    assert!(c.wait_seen("Restart it\\? \\[y/N\\]", LONG));
    c.send(b"y\r");
    t.ok("it says it started this imux", c.wait_seen("the server has ended; starting this imux", LONG));
    t.ok("the client attaches", c.wait_drawn(LONG));
    t.ok("the server is another process", wait_until(LONG, || server_pid(&e).is_some() && server_pid(&e) != old));
    // the attach of the client is what brings the sessions back
    t.ok("both sessions are back, with their windows", wait_until(LONG, || {
        let ls = e.all(&["ls"]);
        ls.lines().any(|l| l.starts_with("work: 2 windows")) && ls.lines().any(|l| l.starts_with("other: 1 window"))
    }));
    c.send(b"\x02d");
    c.wait_exit(LONG);

    t.section("--yes --detach: stopped, and the next attach restores");
    let before = server_pid(&e);
    let r = e.run(&["restart-server", "--yes", "--detach"]);
    t.ok("it ends the server and says what happens next", r.code == 0 && r.out.contains("its sessions are saved"));
    t.ok("and the autosave was written (no warning that it was not)", !r.err.contains("no new autosave was written"));
    t.ok("no server runs now", wait_until(LONG, || server_pid(&e).is_none()) && before.is_some());
    let mut c = e.client(100, 30, &["new", "-s", "fresh"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    t.ok("the next imux brings the sessions back beside the new one", wait_until(LONG, || {
        let ls = e.all(&["ls"]);
        ls.contains("work:") && ls.contains("other:") && ls.contains("fresh:")
    }));
    c.send(b"\x02d");
    c.wait_exit(LONG);

    t.section("with autosave off it warns");
    e.run(&["set", "-g", "autosave-interval", "0"]);
    let mut c = e.client(100, 30, &["restart-server"]);
    t.ok("that nothing is saved", c.wait_seen("autosave-interval is 0", LONG) && c.seen("will NOT come back"));
    c.send(b"n\r");
    c.wait_exit(LONG);
    e.kill_server();
    t.finish();
}
