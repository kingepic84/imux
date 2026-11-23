//! Many windows: the tabs wrap onto more status rows and the panes shrink, then grow back; the clock
//! stays visible. (The Rust version of tests/m6_statusrows.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(25);

#[test]
fn m6_statusrows() {
    let mut e = Env::new("m6r");
    e.unset("SHELL");
    let uid = unsafe { libc::getuid() };
    let sock = e.path(&format!("imux-{uid}/default"));
    let mut t = Check::new("m6_statusrows");
    let mut c = e.client(60, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(600)));
    c.send_str("echo one-row-$(stty size | tr \" \" x)\r");
    t.ok("one window: single status row", c.wait_for("one-row-29x60", LONG));
    for _ in 0..11 {
        c.send(b"\x02c");
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(wait_until(LONG, || e.out(&["list-windows"]).lines().count() == 12));
    std::thread::sleep(Duration::from_millis(800));
    c.send_str("echo wrapped-$(stty size | tr \" \" x)\r");
    t.ok("twelve windows: tabs wrap and the panes shrink", c.wait_for("wrapped-2[0-8]x60", LONG));
    for _ in 0..11 {
        c.send(b"\x02&");
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(wait_until(LONG, || e.out(&["list-windows"]).lines().count() == 1));
    std::thread::sleep(Duration::from_millis(800));
    c.send_str("echo back-$(stty size | tr \" \" x)\r");
    t.ok("back to one window: panes grow back", c.wait_for("back-29x60", LONG));
    t.ok("clock stays visible while tabs wrap", c.seen("[0-9][0-9]:[0-9][0-9] [0-9][0-9]-[A-Z][a-z][a-z]-[0-9][0-9]"));
    c.send_str("exit\r");
    c.wait_exit(LONG);
    t.ok("server exits when the last window closes", wait_until(LONG, || !sock.exists()));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
