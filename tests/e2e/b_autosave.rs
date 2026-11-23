//! The autosave is written on a thread of its own: while a (deliberately slow) write is running the
//! server still answers at once, the file arrives in the end, and kill-server never leaves one behind
//! even when it comes while a write is in flight. Needs a debug build (IMUX_TEST_SAVE_DELAY_MS).
//! (The Rust version of tests/b_autosave.sh.)
use crate::support::*;
use std::time::{Duration, Instant};

#[test]
fn b_autosave() {
    let mut e = Env::new("bau");
    let conf = e.write("imux.conf", "set autosave-interval 1\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.set("IMUX_TEST_SAVE_DELAY_MS", "3000");
    let save = e.path("state/autosave.imux");
    let prev = e.path("state/autosave.prev");
    let mut t = Check::new("b_autosave");

    let mut c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(wait_until(Duration::from_secs(20), || e.out(&["ls"]).lines().any(|l| l.starts_with("w:"))));
    t.ok("the server is up", e.out(&["ls"]).lines().any(|l| l.starts_with("w:")));

    t.section("a slow write does not stall the loop");
    e.run(&["new-window", "-t", "w", "-n", "changed"]);
    let mut worst = 0u128;
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(4500) {
        let t0 = Instant::now();
        e.run(&["ls"]);
        worst = worst.max(t0.elapsed().as_millis());
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("     slowest answer while the write ran: {worst} ms");
    t.ok("the server answered within a second throughout", worst < 1000);
    t.ok("the autosave file arrived", wait_until(Duration::from_secs(10), || std::fs::metadata(&save).map(|m| m.len() > 0).unwrap_or(false)));

    t.section("kill-server while a write is in flight");
    e.run(&["new-window", "-t", "w", "-n", "again"]);
    // The write of this change starts when the autosave interval (1 s) has passed and then takes the
    // 3 s the test asks for: two seconds on is in the middle of it. Nothing shows a write begin.
    std::thread::sleep(Duration::from_secs(2));
    e.run(&["kill-server"]);
    t.ok("the server ends", wait_until(Duration::from_secs(15), || e.all(&["ls"]).contains("no server running")));
    t.ok("and no autosave is left behind", !save.exists() && !prev.exists());
    c.wait_exit(Duration::from_secs(10));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
