//! Dragging the divider between two panes with the mouse resizes them, and a click focuses a pane.
//! (The Rust version of tests/m7_drag.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

#[test]
fn m7_drag() {
    let mut e = Env::new("m7d");
    e.unset("SHELL");
    let mut t = Check::new("m7_drag");
    let mut c = e.client(100, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(600)));
    c.type_chunks(&["\x02%"], Duration::from_millis(700));
    assert!(wait_until(LONG, || e.out(&["list-panes"]).lines().count() == 2));
    c.send_str("\x1b[<0;51;5M\x1b[<32;41;5M\x1b[<32;31;5M\x1b[<0;31;5m");
    c.send_str("echo right-size-$(stty size | tr \" \" x)\r");
    t.ok("dragging the divider left narrows the left pane and widens the right one", c.wait_for("right-size-29x69", LONG));
    c.send_str("\x1b[<0;5;5M\x1b[<0;5;5m");
    c.send_str("echo left-size-$(stty size | tr \" \" x)\r");
    t.ok("the left pane followed the pointer to 30 columns", c.wait_for("left-size-29x30", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    e.kill_server();
    t.finish();
}
