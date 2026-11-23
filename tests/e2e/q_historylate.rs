//! A change that comes inside the interval of the last rolling copy still gets its copy, once the
//! interval is over, even if nothing else changes afterwards. (Before, the copy was left out and
//! the autosave, having nothing new to write, never asked for it again.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_historylate() {
    let mut e = Env::new("qhl");
    let mut t = Check::new("q_historylate");
    let conf = e.write("imux.conf", "set autosave-interval 1\nset autosave-history-interval 4\nset autosave-history 5\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let h = e.path("state/snapshots/history");
    let copies = || std::fs::read_dir(&h).map(|d| d.filter_map(|f| f.ok()).filter(|f| f.file_name().to_string_lossy().ends_with(".imux")).count()).unwrap_or(0);
    e.run(&["new-session", "-d", "-s", "alpha"]);
    t.ok("a first rolling copy appears", wait_until(Duration::from_secs(15), || copies() == 1));
    // inside the 4 s interval, and then nothing changes again
    e.run(&["new-window", "-t", "alpha:", "-n", "late"]);
    t.ok("the change gets its copy when the interval is over", wait_until(Duration::from_secs(20), || copies() == 2));
    let newest = std::fs::read_dir(&h).unwrap().filter_map(|f| f.ok()).map(|f| f.path()).max().unwrap();
    t.ok("and it holds the new window", std::fs::read_to_string(newest).unwrap_or_default().contains("late"));
    e.kill_server();
    t.finish();
}
