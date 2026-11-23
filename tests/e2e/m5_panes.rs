//! Panes through a client: sizes of splits, moving between panes, killing one, resizing with Alt-Right,
//! layouts, the divider, clicks that focus a pane, and the server ending with its last pane.
//! (The Rust version of tests/m5_panes.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);
const GAP: Duration = Duration::from_millis(600);

fn shell(e: &mut Env) {
    if let Some(z) = which("zsh") {
        e.set("SHELL", &z);
    }
}

fn run_client(e: &Env, chunks: &[&str]) -> Client {
    let mut c = e.client(100, 30, &[]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("0:", Duration::from_millis(800)), "the pane never settled");
    c.type_chunks(chunks, GAP);
    c
}

#[test]
fn m5_panes() {
    let mut e = Env::new("m5");
    shell(&mut e);
    let uid = unsafe { libc::getuid() };
    let sock = e.path(&format!("imux-{uid}/default"));
    let mut t = Check::new("m5_panes");
    let size = |label: &str| format!("echo {label}-$(stty size | tr \" \" x)\r");
    let (first, second, stacked, resized, even) = (size("first-size"), size("second-size"), size("stacked-size"), size("resized-size"), size("even-size"));

    let mut a = run_client(
        &e,
        &[&first, "\x02%", &second, "\x02\x1b[D", "echo back-in-first-$((6*7))\r", "\x02\"", &stacked, "\x02x", "echo after-kill-$((5*5))\r", "\x02\x1b[1;3C", &resized, "\x02:", "select-layout even-horizontal\r", &even, "\x02o", "exit\r", "exit\r", "exit\r"],
    );
    a.wait_exit(LONG);
    t.ok("single pane is the full area minus status row", a.seen("first-size-29x100"));
    t.ok("side-by-side split halves the width (49 cols for the right pane)", a.seen("second-size-29x49"));
    t.ok("prefix left arrow returns to the first pane", a.seen("back-in-first-42"));
    t.ok("stacked split halves the height", a.seen("stacked-size-14x50"));
    t.ok("prefix x closes the pane and the next one keeps working", a.seen("after-kill-25"));
    t.ok("prefix Alt-Right grows the pane to the right", a.seen("resized-size-29x55"));
    t.ok("divider glyph drawn", a.seen("│"));
    t.ok("server exits when the last pane exits", wait_until(LONG, || !sock.exists()));
    std::thread::sleep(Duration::from_secs(1));

    let clicked = size("clicked-size");
    let clicked_right = size("clicked-right-size");
    let mut b = run_client(&e, &["\x02%", "\x1b[<0;5;5M\x1b[<0;5;5m", &clicked, "\x1b[<0;90;5M\x1b[<0;90;5m", &clicked_right, "exit\r", "exit\r"]);
    b.wait_exit(LONG);
    t.ok("clicking the left pane focuses it", b.seen("clicked-size-29x50"));
    t.ok("clicking the right pane focuses it", b.seen("clicked-right-size-29x49"));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
