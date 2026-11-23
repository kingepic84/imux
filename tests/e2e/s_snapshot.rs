//! Snapshots keep the arguments of a program only when that program is restored: a restored program
//! keeps them, any other is recorded by name, and neither the named snapshot nor the autosave holds
//! the arguments of the second. (The Rust version of tests/s_snapshot.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn s_snapshot() {
    let mut e = Env::new("ssnap");
    let mut t = Check::new("s_snapshot");
    let conf = e.write("conf", "set autosave-interval 1\nset restore-commands \"sleep\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    t.section("arguments are saved only for programs that are restored");
    let mut c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client());
    assert!(e.ready("%0", Duration::from_millis(500)));
    c.send_str("sleep 31341\r");
    c.wait_for("sleep 31341", Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(500));
    c.send(b"\x02c");
    assert!(wait_until(Duration::from_secs(10), || e.out(&["list-panes", "-a", "-F", "#{pane_id}"]).lines().count() == 2));
    assert!(e.ready("%1", Duration::from_millis(500)));
    c.send_str("bash -c 'sleep 60; :' SECRETARG123\r");
    t.ok("both programs are running", wait_until(Duration::from_secs(10), || pgrep("sleep 31341") && pgrep("SECRETARG123")));
    let r = e.run(&["save", "snap1"]);
    t.matches("save reports what it wrote", "saved 1 session as snap1", &r.all());
    let snap = e.read("state/snapshots/snap1.imux");
    t.ok("a restored program keeps its arguments", snap.contains("sleep 31341"));
    t.ok("a program that is not restored is recorded by name", snap.contains(" 0 1 bash"));
    t.ok("its arguments are not in the named snapshot", !snap.contains("SECRETARG123"));
    t.ok(
        "the autosave has the restored program",
        wait_until(Duration::from_secs(15), || e.read("state/autosave.imux").contains("31341")),
    );
    std::thread::sleep(Duration::from_secs(1));
    t.ok("nor in the autosave", !e.read("state/autosave.imux").contains("SECRETARG123"));
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
    e.kill_server();
    pkill("sleep 31341");
    pkill("SECRETARG123");
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
