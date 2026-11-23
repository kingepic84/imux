//! Two clients on one session: both see both clients' output, and the shell's size is the smaller
//! client's. (The Rust version of tests/m2_multiclient.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

#[test]
fn m2_multiclient() {
    let e = Env::new("m2m");
    let mut t = Check::new("m2_multiclient");
    let mut a = e.client(100, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(600)));
    let mut b = e.client(40, 12, &["attach"]);
    assert!(wait_until(LONG, || e.out(&["list-clients"]).lines().count() == 2), "the second client did not attach");
    std::thread::sleep(Duration::from_millis(700));
    a.send_str("echo from-a-$((3*3))\r");
    assert!(a.wait_for("from-a-9", LONG));
    a.send_str("echo size-$(stty size | tr \" \" x)\r");
    b.send_str("echo from-b-$((4*4))\r");
    t.ok("A sees its own command output", a.wait_for("from-a-9", LONG));
    t.ok("B sees A's command output", b.wait_for("from-a-9", LONG));
    t.ok("A sees B's command output", a.wait_for("from-b-16", LONG));
    t.ok("B sees its own command output", b.wait_for("from-b-16", LONG));
    t.ok("pty uses the smaller client's size minus the status row (A, 30x100 vs B, 12x40)", a.wait_for("size-11x40", LONG));
    t.ok("same size output reached B", b.wait_for("size-11x40", LONG));
    a.send(b"\x02d");
    b.send(b"\x02d");
    a.wait_exit(LONG);
    b.wait_exit(LONG);
    e.kill_server();
    t.finish();
}
