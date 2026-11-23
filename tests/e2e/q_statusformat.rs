//! status-format[N]: a row drawn from its own text. A format on row 1 adds a row the panes give
//! up; a format on row 0 replaces the standard bar; clearing the formats brings the bar back.
//! (The Rust version of tests/q_statusformat.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_statusformat() {
    let e = Env::new("qsf");
    let mut t = Check::new("q_statusformat");
    let height = |want: &str| wait_until(Duration::from_secs(10), || e.var("s:one", "#{pane_height}") == want);
    e.session("s", "one");
    e.run(&["set", "-g", "status-left", "BAR"]);
    e.run(&["set", "-g", "status-right", ""]);
    let mut c = e.client(100, 30, &["attach", "-t", "s"]);
    t.ok("one bar row to begin with", height("29"));
    e.run(&["set", "-g", "status-format[1]", "SECOND ROW"]);
    t.ok("a format on row 1 adds a row", height("28"));
    e.run(&["set", "-g", "status-format[0]", "FIRST ROW"]);
    std::thread::sleep(Duration::from_millis(500));
    t.ok("a format on row 0 keeps both rows", height("28"));
    c.wait_seen("FIRST ROW", Duration::from_secs(5));
    e.run(&["set", "-gu", "status-format[1]"]);
    t.ok("clearing row 1 gives the row back", height("29"));
    e.run(&["set", "-gu", "status-format[0]"]);
    std::thread::sleep(Duration::from_millis(300));
    c.send(b"\x02d");
    c.wait_exit(Duration::from_secs(3));
    let out = c.text();
    t.matches("the second row was drawn on the last screen row", "\x1b\\[30;1H(\x1b\\[[0-9;]*m)*SECOND ROW", &out);
    t.matches("the first row text was drawn above it", "\x1b\\[29;1H(\x1b\\[[0-9;]*m)*FIRST ROW", &out);
    let r = e.run(&["set", "-g", "status-format[7]", "x"]);
    t.ok("an index past 4 is refused", !r.success() && r.all().contains("status-format"));
    e.kill_server();
    t.finish();
}
