//! Copy mode through a client: the position indicator, the wheel entering it and scrolling five lines,
//! yank and paste, and a double click leaving it. (The Rust version of tests/m8_copy.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(600);

#[test]
fn m8_copy() {
    let mut e = Env::new("m8c");
    let conf = e.write("conf", "set copy-command \"\"\nset mode-keys vi\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("m8_copy");
    let mut c = e.client(100, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(600)));
    c.send_str("seq 1 60\r");
    assert!(e.shows_line("0:", "60"));
    c.type_chunks(&["\x02[", "g", "q"], GAP);
    c.send_str("echo copyme-$((6*7))\r");
    assert!(e.shows_line("0:", "copyme-42"));
    c.type_chunks(&["\x02[", "?copyme-4\r", "v$y"], GAP);
    c.send_str("echo pasted:");
    std::thread::sleep(GAP);
    c.type_chunks(&["\x02]", "\r"], GAP);
    t.ok("yank then paste buffer types the copied text", c.wait_for("pasted:copyme-42", LONG));
    c.type_chunks(&["\x1b[<64;10;5M"], GAP);
    t.ok("wheel up enters copy mode and scrolls five lines", c.wait_seen(r"\[5/", LONG));
    t.ok("copy mode shows a position indicator", c.seen(r"\[[0-9][0-9]*/[0-9][0-9]*\]"));
    c.type_chunks(&["\x1b[<0;10;5M\x1b[<0;10;5m\x1b[<0;10;5M\x1b[<0;10;5m"], GAP);
    c.send_str("echo dbl-$((6*7))\r");
    t.ok("a double click leaves copy mode so typing reaches the shell again", c.wait_for("dbl-42", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    e.kill_server();
    t.finish();
}
