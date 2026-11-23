//! Arranging panes: zoom, break-pane, swap-pane, rotate-window and the layout presets, each measured
//! by the size the pane's own tty reports. (The Rust version of tests/t_arrange.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(450);

fn new_client(e: &Env) -> Client {
    let c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("w:", Duration::from_millis(600)), "the pane never settled");
    c
}

fn leave(e: &Env, mut c: Client) {
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
}

fn panes(e: &Env, n: usize) -> bool {
    wait_until(LONG, || e.out(&["list-panes", "-t", "w:"]).lines().count() == n)
}

#[test]
fn t_arrange() {
    let mut e = Env::new("tar");
    e.unset("SHELL");
    let mut t = Check::new("t_arrange");
    let size = |label: &str| format!("echo {label}-$(stty size | tr \" \" x)\r");

    t.section("zoom");
    let mut c = new_client(&e);
    c.send(b"\x02%");
    assert!(panes(&e, 2));
    c.send_str(&size("before"));
    t.ok("the right pane is half the window before", c.wait_for("before-29x49", LONG));
    c.type_chunks(&["\x02z"], GAP);
    c.send_str(&size("zoomed"));
    t.ok("zoom gives it the whole window", c.wait_for("zoomed-29x100", LONG));
    t.ok("the status line marks the zoomed window (the Z is the one cell that changes)", c.wait_for("\\[[0-9]+;[0-9]+H.\\[[0-9;]*mZ", LONG));
    c.type_chunks(&["\x02z"], GAP);
    c.send_str(&size("after"));
    t.ok("zoom again gives the half back", c.wait_for("after-29x49", LONG));
    leave(&e, c);

    t.section("break-pane");
    let mut c = new_client(&e);
    c.send(b"\x02%");
    assert!(panes(&e, 2));
    c.type_chunks(&["\x02!"], GAP);
    c.send_str(&size("broken"));
    t.ok("the broken out pane has a window to itself", c.wait_for("broken-29x100", LONG));
    t.ok("so there are two windows", e.out(&["list-windows"]).lines().count() == 2);
    leave(&e, c);

    t.section("swap-pane and rotate-window");
    let mut c = new_client(&e);
    c.send(b"\x02%");
    assert!(panes(&e, 2));
    c.send_str(&size("right"));
    t.ok("the right pane is 49 wide", c.wait_for("right-29x49", LONG));
    c.type_chunks(&["\x02{"], GAP);
    c.send_str(&size("swapped"));
    t.ok("swap-pane -U moves the active pane to the left, where there are 50 columns", c.wait_for("swapped-29x50", LONG));
    c.type_chunks(&["\x02}"], GAP);
    c.send_str(&size("back"));
    t.ok("swap-pane -D moves it back", c.wait_for("back-29x49", LONG));
    leave(&e, c);
    let mut c = new_client(&e);
    c.send(b"\x02%");
    assert!(panes(&e, 2));
    c.type_chunks(&["\x02\x0f"], GAP);
    c.send_str(&size("rotated"));
    t.ok("rotate-window moves the active pane to the other place", c.wait_for("rotated-29x50", LONG));
    leave(&e, c);

    t.section("layouts");
    let mut c = new_client(&e);
    c.send(b"\x02%");
    assert!(panes(&e, 2));
    c.send(b"\x02%");
    assert!(panes(&e, 3));
    c.type_chunks(&["\x02\x1b4"], GAP);
    c.send_str(&size("mainv"));
    t.ok("main-vertical puts the last pane under the other small one", c.wait_for("mainv-14x33", LONG));
    c.type_chunks(&["\x02\x1b5"], GAP);
    c.send_str(&size("tiled"));
    t.ok("tiled gives three panes a 2 by 2 grid with the last one wide", c.wait_for("tiled-14x100", LONG));
    c.type_chunks(&["\x02\x1b1"], GAP);
    c.send_str(&size("evenh"));
    t.ok("even-horizontal makes three columns", c.wait_for("evenh-29x3[0-3]", LONG));
    c.type_chunks(&["\x02 "], GAP);
    c.send_str(&size("next"));
    t.ok("Space goes on to the next preset, even-vertical", c.wait_for("next-(9|10)x100", LONG));
    leave(&e, c);

    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
