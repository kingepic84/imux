//! imux doctor names the panes that started in the home directory because their saved directory is
//! gone, and says nothing about them once they have moved. (The Rust version of
//! tests/q_doctorrestore.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_doctorrestore() {
    let mut e = Env::new("qdr");
    let mut t = Check::new("q_doctorrestore");
    let conf = e.write("imux.conf", "set autosave-interval 1\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    e.set("HOME", &p(&home));
    let work = e.path("work");
    std::fs::create_dir_all(&work).unwrap();
    e.run(&["new-session", "-d", "-s", "r", "-c", &p(&work)]);
    wait_until(Duration::from_secs(15), || e.read("state/autosave.imux").contains("/work"));
    let spid: i32 = e.out(&["display-message", "-p", "#{pid}"]).parse().unwrap();
    unsafe { libc::kill(spid, libc::SIGTERM) };
    wait_until(Duration::from_secs(15), || e.all(&["ls"]).contains("no server running"));
    t.ok("doctor says nothing about restore without a server", !e.all(&["doctor"]).lines().any(|l| l.starts_with("restore:")));
    std::fs::remove_dir_all(&work).unwrap();
    let mut c = e.client(100, 30, &["new", "-s", "r"]);
    t.ok("the session comes back", wait_until(Duration::from_secs(15), || e.all(&["ls"]).lines().any(|l| l.starts_with("r:"))));
    t.matches(
        "doctor names the pane that lost its directory",
        r"(?m)^restore: 1 pane\(s\) started in the home directory because their saved directory is gone: r:[0-9]*\.[0-9]*$",
        &e.all(&["doctor"]),
    );
    e.run(&["send-keys", "-t", "r:", "cd /tmp", "Enter"]);
    wait_until(Duration::from_secs(15), || e.out(&["list-panes", "-t", "r:", "-F", "#{pane_restore_missing}"]).lines().next() == Some("0"));
    e.run(&["save"]);
    t.ok("once the pane moved the line is gone", !e.all(&["doctor"]).lines().any(|l| l.starts_with("restore:")));
    c.kill();
    e.kill_server();
    t.finish();
}
