//! `prefix C-s` saves the sessions as the snapshot `resurrect` and `prefix C-r` brings them back, also
//! in a server that was started again after `kill-server`; `bind` and `unbind` change the keys.
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

#[test]
fn q_resurrect() {
    let mut e = Env::new("qrs");
    e.set("IMUX_CONFIG", "/dev/null");
    let mut t = Check::new("q_resurrect");
    let dir = e.path("projects");
    std::fs::create_dir_all(&dir).unwrap();
    let snap = e.path("state/snapshots/resurrect.imux");

    t.section("the default keys");
    e.run(&["new-session", "-d", "-s", "first"]);
    t.matches("list-keys shows the save key", "(?m)^bind-key -T prefix C-s +save-session resurrect$", &e.all(&["list-keys"]));
    t.matches("and the restore key", "(?m)^bind-key -T prefix C-r +restore-session resurrect$", &e.all(&["list-keys"]));

    e.kill_server();
    let mut c = e.client_in(&dir, 100, 30, &["new", "-s", "work"], &[]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c.send(b"\x02\x13");
    t.ok("prefix C-s saves, and says so", c.wait_seen("saved 1 session as resurrect", LONG));
    t.ok("the snapshot is a file", wait_until(LONG, || snap.exists()));
    t.ok("and it holds the session and its directory", std::fs::read_to_string(&snap).map(|s| s.contains("work") && s.contains("projects")).unwrap_or(false));
    // the terminal's own flow control did not swallow the key, and the client still draws
    c.send_str("echo still-drawing-$((20+22))\r");
    t.ok("the screen still works after C-s", c.wait_seen("still-drawing-42", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);

    t.section("after the server is gone");
    e.kill_server();
    t.ok("the snapshot stays", snap.exists());
    let mut c = e.client(100, 30, &["new", "-s", "other"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c.send(b"\x02\x12");
    t.ok("prefix C-r restores, and says so", c.wait_seen("restored 1 session", LONG));
    t.ok("the saved session is back beside the new one", wait_until(LONG, || {
        let ls = e.all(&["ls"]);
        ls.contains("work:") && ls.contains("other:")
    }));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();

    t.section("overrides");
    let conf = e.write("imux.conf", "unbind C-r\nbind C-s save-session mine\n");
    e.set("IMUX_CONFIG", &p(&conf));
    t.lacks("unbind removes the restore key", "(?m)^bind-key -T prefix C-r ", &e.all(&["list-keys"]));
    let mut c = e.client(100, 30, &["new", "-s", "work2"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c.send(b"\x02\x13");
    t.ok("bind gives the save key another snapshot", c.wait_seen("saved 1 session as mine", LONG));
    t.ok("and writes that file, not the other", e.path("state/snapshots/mine.imux").exists());
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    t.finish();
}
