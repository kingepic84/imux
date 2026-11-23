//! Input handling: a CSI u encoded prefix, a repeatable key pressed again within repeat-time, an
//! unbound key after the prefix, and a lone escape not glued to the next key.
//! (The Rust version of tests/m7_input.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

#[test]
fn m7_input() {
    let mut e = Env::new("m7i");
    e.unset("SHELL");
    let mut t = Check::new("m7_input");
    let mut c = e.client(100, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(600)));
    c.send(b"\x1b[98;5u");
    std::thread::sleep(Duration::from_millis(500));
    c.send(b"%");
    assert!(wait_until(LONG, || e.out(&["list-panes"]).lines().count() == 2));
    std::thread::sleep(Duration::from_millis(600));
    c.send_str("echo csiu-size-$(stty size | tr \" \" x)\r");
    t.ok("CSI u encoded prefix is recognized", c.wait_for("csiu-size-29x49", LONG));
    c.send(b"\x02\x1b[1;3D\x1b[1;3D");
    std::thread::sleep(Duration::from_millis(700));
    c.send_str("echo repeat-size-$(stty size | tr \" \" x)\r");
    t.ok("a repeatable key pressed again within repeat-time needs no prefix", c.wait_for("repeat-size-29x59", LONG));
    c.type_chunks(&["\x02y"], Duration::from_millis(600));
    c.send_str("echo unbound-$((2*21))\r");
    t.ok("an unbound key after the prefix is dropped", c.wait_for("unbound-42", LONG));
    c.type_chunks(&["\x1b"], Duration::from_millis(800));
    c.send(b"\x02\"");
    assert!(wait_until(LONG, || e.out(&["list-panes"]).lines().count() == 3));
    std::thread::sleep(Duration::from_millis(600));
    c.send_str("echo escape-$(stty size | tr \" \" x)\r");
    t.ok("a lone escape is not glued to the next key", c.wait_for("escape-14x59", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    e.kill_server();
    t.finish();
}
