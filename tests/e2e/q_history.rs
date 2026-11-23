//! autosave-history: timestamped rolling copies of the autosave, kept to a count, listed by
//! `imux snapshots --history` and brought back by `imux restore history/NAME`; never restored by
//! themselves; off at 0. (The Rust version of tests/q_history.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn q_history() {
    let mut e = Env::new("qhist");
    let mut t = Check::new("q_history");
    let conf = e.write("imux.conf", "set autosave-interval 1\nset autosave-history-interval 1\nset autosave-history 2\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let h = e.path("state/snapshots/history");
    let names = || -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&h).map(|d| d.filter_map(|f| f.ok()).map(|f| f.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let count = || names().iter().filter(|n| n.ends_with(".imux")).count();
    let sess = |name: &str| e.out(&["ls"]).lines().any(|l| l.starts_with(&format!("{name}:")));
    e.run(&["new-session", "-d", "-s", "alpha"]);
    t.ok("a first rolling copy appears", wait_until(Duration::from_secs(15), || count() >= 1));
    let first = names().into_iter().find(|n| n.ends_with(".imux")).unwrap_or_default();
    let mode = std::fs::metadata(h.join(&first)).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
    t.ok("its file is private", mode == 0o600);
    // each change of state makes a new rolling copy once the interval has passed since the last one
    let newest = || names().into_iter().rfind(|n| n.ends_with(".imux")).unwrap_or_default();
    for n in ["w1", "w2", "w3"] {
        let before = newest();
        e.run(&["new-window", "-t", "alpha:", "-n", n]);
        assert!(wait_until(Duration::from_secs(40), || newest() != before), "no rolling copy was made for {n}");
    }
    t.ok("the count limits the copies", wait_until(Duration::from_secs(15), || count() == 2));
    let listing = e.out(&["snapshots", "--history"]);
    t.ok("imux snapshots --history lists them", listing.lines().filter(|l| l.starts_with("history/")).count() == 2);
    t.ok("the newest copy shows the session", listing.lines().next().unwrap_or("").contains("alpha"));
    let newest = listing.lines().next().unwrap_or("").split(' ').next().unwrap_or("").to_string();
    t.matches("the name is history/NAME", "^history/[0-9]+$", &newest);
    e.kill_server();
    t.ok("kill-server leaves the rolling copies", count() == 2);
    t.ok("and no autosave", !e.path("state/autosave.imux").exists());
    e.run(&["new-session", "-d", "-s", "beta"]);
    e.run(&["set", "-g", "autosave-history", "10"]);
    t.ok("a new server does not restore a rolling copy by itself", !sess("alpha"));
    e.run(&["restore", &newest]);
    t.ok("restore history/NAME brings the session back", sess("alpha"));
    let r = e.run(&["restore", "history/../x"]);
    t.ok("a bad name is refused", r.all().contains("no rolling copy named"));
    e.kill_server();

    t.section("off at 0");
    let _ = std::fs::remove_dir_all(e.path("state"));
    e.write("imux.conf", "set autosave-interval 1\nset autosave-history 0\n");
    e.run(&["new-session", "-d", "-s", "gamma"]);
    t.ok(
        "an autosave is written",
        wait_until(Duration::from_secs(15), || std::fs::metadata(e.path("state/autosave.imux")).map(|m| m.len() > 0).unwrap_or(false)),
    );
    // the next change of state is written to the autosave, and a rolling copy would be written with it
    e.run(&["new-window", "-t", "gamma:", "-n", "marker"]);
    assert!(wait_until(Duration::from_secs(15), || std::fs::read_to_string(e.path("state/autosave.imux")).unwrap_or_default().contains("marker")), "the autosave did not follow the change");
    t.ok("no rolling copy is made", count() == 0 && e.out(&["snapshots", "--history"]) == "no rolling copies");
    e.kill_server();
    t.finish();
}
