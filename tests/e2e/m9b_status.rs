//! The example configuration (tests/fixtures/example.imux.conf) drives the status line:
//! tab texts, the status-left glyph, the clock, a display message and the current tab's colors.
//! (The Rust version of tests/m9b_status.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

#[test]
fn m9b_status() {
    let mut e = Env::new("m9b");
    let zsh = which("zsh");
    if let Some(z) = &zsh {
        e.set("SHELL", z);
    }
    e.set("IMUX_CONFIG", &format!("{}/tests/fixtures/example.imux.conf", env!("CARGO_MANIFEST_DIR")));
    let mut t = Check::new("m9b_status");
    let mut c = e.client(120, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(800)));
    c.type_chunks(&["\x01c"], Duration::from_millis(900));
    assert!(wait_until(LONG, || e.out(&["list-windows"]).lines().count() == 2));
    c.type_chunks(&["\x01:"], Duration::from_millis(500));
    c.send_str("display \"hello msg\"\r");
    std::thread::sleep(Duration::from_millis(500));
    c.send_str("\x011");
    t.ok("example config: display message", c.wait_seen("hello msg", LONG));
    if zsh.is_some() {
        t.ok("example config: current tab text", c.wait_seen(" 1  zsh ", LONG));
        t.ok("example config: second tab text", c.wait_seen(" 2  zsh ", LONG));
    } else {
        println!("skip: the tab texts need zsh, which is not installed");
    }
    t.ok("example config: status-left shows the session", c.wait_seen("󰌪 0 ", LONG));
    t.ok("example config: clock shows AM or PM", c.wait_seen("[AP]M", LONG));
    let raw = c.text();
    t.ok("example config: current tab is bold on blue", raw.contains("1;38;2;0;0;0;48;2;137;180;250"));
    c.send(b"\x01d");
    c.wait_exit(LONG);
    let log = e.server_log();
    t.ok("no config problems in server.log", !log.lines().any(|l| l.contains("\"level\":\"warn\"") && l.contains("config: ")));
    t.ok("no errors in server.log", !log.contains("\"level\":\"error\""));
    e.kill_server();
    t.finish();
}
