//! A reported resume command is typed again for every restored pane when the snapshot is restored,
//! whether its window is in front or not and whether anyone looks at its session or not: the
//! commands do not wait for a focus. (The Rust version of tests/q_restoreresume.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_restoreresume() {
    let mut e = Env::new("qrr");
    let mut t = Check::new("q_restoreresume");
    let conf = e.write("conf", "set autosave-interval 0\nset restore-commands \"sleep\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.set("BIN", BIN);
    let fix = format!("{}/tests/fixtures/report_resume.sh", env!("CARGO_MANIFEST_DIR"));
    e.session("w", "w");
    e.run(&["new-window", "-d", "-t", "w"]);
    wait_until(Duration::from_secs(10), || e.out(&["list-panes", "-a", "-F", "#{pane_id}", "-t", "w"]).lines().count() == 2);
    let panes = e.out(&["list-panes", "-a", "-F", "#{pane_id}", "-t", "w"]);
    let ids: Vec<&str> = panes.lines().collect();
    t.ok("two panes in two windows, the first window in front", ids.len() == 2 && ids[0] != ids[1]);
    for id in &ids {
        e.ready(id, Duration::from_millis(500));
    }
    e.run(&["send-keys", "-t", ids[0], &format!("{fix} 31341"), "Enter"]);
    e.run(&["send-keys", "-t", ids[1], &format!("{fix} 31342"), "Enter"]);
    let snap = e.path("state/snapshots/resumeB.imux");
    let reported = |snap: &std::path::Path| {
        e.run(&["save", "resumeB"]);
        std::fs::read_to_string(snap).map(|s| s.matches("+resume").count()).unwrap_or(0)
    };
    t.ok("both reports are in the snapshot", wait_until(Duration::from_secs(15), || reported(&snap) == 2));
    e.kill_server();
    t.section("restore with a client on another session");
    let mut c = e.client(100, 30, &["new", "-s", "other"]);
    t.ok("a client is attached", e.has_client());
    // A restore needs a client that has finished attaching; ask until the server takes it.
    wait_until(Duration::from_secs(15), || e.run(&["restore", "resumeB"]).code == 0);
    t.ok("the pane of the window in front is resumed", wait_until(Duration::from_secs(20), || pgrep("sleep 31341")));
    t.ok("the pane of the window behind it is resumed too", wait_until(Duration::from_secs(20), || pgrep("sleep 31342")));
    t.ok(
        "the restored session is not the one the client looks at",
        !e.out(&["list-clients", "-F", "#{client_session}"]).lines().any(|l| l == "w"),
    );
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
    e.kill_server();
    t.finish();
}
