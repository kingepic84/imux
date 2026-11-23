//! unlink-window (a window is never in two sessions, so it is kill-window), and choose-window and
//! choose-session as the names of the two pickers. (The Rust version of tests/q_unlinkchoose.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_unlinkchoose() {
    let mut e = Env::new("quc");
    let mut t = Check::new("q_unlinkchoose");
    let conf = e.write("imux.conf", "bind W choose-window\nbind S choose-session\nbind U unlink-window\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let order = |s: &str| e.out(&["list-windows", "-t", s, "-F", "#{window_name}"]).lines().map(|l| format!("{l} ")).collect::<String>();
    e.session("uq", "a");
    for n in ["b", "c", "d"] {
        e.run(&["new-window", "-d", "-t", "uq", "-n", n]);
    }
    e.run(&["unlink-window", "-t", "uq:b"]);
    t.eq("unlink-window removes the window", order("uq"), "a c d ".to_string());
    e.run(&["unlink-window", "-k", "-t", "uq:c"]);
    t.eq("-k is accepted", order("uq"), "a d ".to_string());
    e.run(&["unlinkw", "-t", "uq:d"]);
    t.eq("the short name works", order("uq"), "a ".to_string());
    let keys = e.all(&["list-keys"]);
    t.ok("the three bindings loaded", keys.contains("choose") && (keys.contains("unlink-window") || keys.contains("kill-window")));
    let text = e.with_client("uq", |c| {
        c.send(b"\x02W");
        c.wait_seen("sessions and windows", Duration::from_secs(5));
        c.send(b"\x1b");
    });
    t.ok("choose-window opens the window picker", text.contains("sessions and windows"));
    let text = e.with_client("uq", |c| {
        c.send(b"\x02S");
        c.wait_seen("sessions", Duration::from_secs(5));
        c.send(b"\x1b");
    });
    t.ok("choose-session opens the session list, not the tree", text.contains("sessions") && !text.contains("sessions and windows"));
    e.kill_server();
    t.finish();
}
