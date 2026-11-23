//! The stock keys: the prefix is C-b, windows start at 0, the bar is bare, C-a is no prefix, the two
//! splits, and the personal keys (| and -) are not bound. (The Rust version of tests/t_stock_keys.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(500);

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
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn t_stock_keys() {
    let mut e = Env::new("tsk");
    e.unset("SHELL");
    let mut t = Check::new("t_stock_keys");

    t.section("the prefix is C-b, windows start at 0 and the bar is bare");
    let mut c = new_client(&e);
    c.send(b"\x02c");
    assert!(wait_until(LONG, || e.out(&["list-windows"]).lines().count() == 2), "c did not create a window");
    std::thread::sleep(GAP);
    c.type_chunks(&["\x02", "0"], GAP);
    c.send_str("echo id-$IMUX_PANE_ID\r");
    t.ok("0 selects the first window", c.wait_for("id-0", LONG));
    c.type_chunks(&["\x02l"], GAP);
    c.send_str("echo back-$IMUX_PANE_ID\r");
    t.ok("l goes back to the last window", c.wait_for("back-1", LONG));
    t.ok("the status line shows the session in brackets", c.seen(r"\[w\]"));
    t.ok("the first window is 0", c.seen("0:[a-z]+"));
    t.ok("the second is 1 and the current one is starred", c.seen(r"1:[a-z]+\*"));
    t.ok("c created a second window", e.out(&["list-windows"]).lines().count() == 2);
    leave(&e, c);

    t.section("C-a is no prefix by default");
    let mut c = new_client(&e);
    c.send(b"\x01c");
    std::thread::sleep(Duration::from_secs(1));
    t.ok("C-a c did not create a window", e.out(&["list-windows"]).lines().count() == 1);
    leave(&e, c);

    t.section("the two splits");
    let mut c = new_client(&e);
    c.send(b"\x02%");
    assert!(wait_until(LONG, || e.out(&["list-panes", "-t", "w:"]).lines().count() == 2));
    std::thread::sleep(GAP);
    c.send_str("echo side-$IMUX_PANE_ID\r");
    t.ok("% opens a second pane next to the first", c.wait_for("side-1", LONG));
    c.type_chunks(&["\x02o"], GAP);
    c.send_str("echo side-$IMUX_PANE_ID\r");
    t.ok("which has its own number", c.wait_for("side-0", LONG));
    leave(&e, c);
    let mut c = new_client(&e);
    c.send(b"\x02\"");
    assert!(wait_until(LONG, || e.out(&["list-panes", "-t", "w:"]).lines().count() == 2));
    std::thread::sleep(GAP);
    c.send_str("echo stack-$IMUX_PANE_ID\r");
    t.ok("\" opens a second pane below the first", c.wait_for("stack-1", LONG));
    c.type_chunks(&["\x02o"], GAP);
    c.send_str("echo stack-$IMUX_PANE_ID\r");
    t.ok("which has its own number", c.wait_for("stack-0", LONG));
    leave(&e, c);

    t.section("the personal keys are not bound");
    let mut c = new_client(&e);
    c.type_chunks(&["\x02|", "\x02-"], GAP);
    c.send_str("echo only-$IMUX_PANE_ID\r");
    t.ok("| and - did not split", c.wait_for("only-0", LONG));
    t.ok("so there is no pane 1", !c.text().contains("only-1"));
    leave(&e, c);

    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
