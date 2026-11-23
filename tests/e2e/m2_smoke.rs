//! Smoke test: a first client, detach, the server and its shell staying, a second client attaching to
//! the same shell, the socket going away with the last shell, and attach with no server.
//! (The Rust version of tests/m2_smoke.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

#[test]
fn m2_smoke() {
    let e = Env::new("m2s");
    let uid = unsafe { libc::getuid() };
    let sock = e.path(&format!("imux-{uid}/default"));
    let mut t = Check::new("m2_smoke");

    let mut a = e.client(80, 24, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(600)));
    a.type_chunks(&["export FOO=persist-$((20+1))\r"], Duration::from_millis(500));
    a.send_str("echo first-$FOO\r");
    t.ok("first client sees command output", a.wait_for("first-persist-21", LONG));
    a.send(b"\x02d");
    t.ok("detach message", a.wait_for(r"\[detached\]", LONG));
    a.wait_exit(LONG);
    t.ok("server still running after detach", sock.exists());

    let mut b = e.client(80, 24, &["attach"]);
    assert!(e.has_client());
    assert!(b.wait_drawn(LONG), "the client drew nothing");
    b.send_str("echo second-$FOO\r");
    t.ok("reattached shell kept its environment", b.wait_for("second-persist-21", LONG));
    b.send_str("exit\r");
    b.wait_exit(LONG);
    t.ok("socket removed after shell exit", wait_until(LONG, || !sock.exists()));

    let mut c = e.client(80, 24, &["attach"]);
    t.ok("attach with no server fails cleanly", c.wait_for("no server running", LONG));
    c.wait_exit(LONG);
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
