//! pane-border-status and pane-border-format: with top or bottom, every pane gives one row to a
//! label row (its program gets one row less), the label is the format, and off gives the row back.
//! (The Rust version of tests/q_borderstatus.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_borderstatus() {
    let e = Env::new("qbs");
    let mut t = Check::new("q_borderstatus");
    let height = |pane: &str| e.out(&["display-message", "-p", "-t", pane, "#{pane_height}"]).parse::<i32>().unwrap_or(-1);
    let reaches = |pane: &str, want: i32| wait_until(Duration::from_secs(10), || height(pane) == want);
    e.session("sty", "one");
    e.run(&["split-window", "-d", "-t", "sty:one"]);
    let ids = e.out(&["list-panes", "-t", "sty:one", "-F", "#{pane_id}"]);
    let (p0, p1) = (ids.lines().next().unwrap().to_string(), ids.lines().last().unwrap().to_string());
    let (h0, h1) = (height(&p0), height(&p1));
    e.run(&["set", "-g", "pane-border-format", "LBL#{pane_index}X"]);
    e.run(&["set", "-g", "pane-border-status", "top"]);
    t.ok("top: each pane loses one row", reaches(&p0, h0 - 1) && reaches(&p1, h1 - 1));
    let all = e.with_client("sty", |c| e.barrier(c, "sty"));
    t.ok("top: the label is drawn for the first pane", all.contains("LBL0X"));
    t.ok("top: and for the second", all.contains("LBL1X"));
    let t0 = height(&p0);
    e.run(&["set", "-g", "pane-border-status", "bottom"]);
    t.ok("bottom: the same height as top", reaches(&p0, t0));
    e.run(&["split-window", "-d", "-t", "sty:one"]);
    let p2 = e.out(&["list-panes", "-t", "sty:one", "-F", "#{pane_id}"]).lines().last().unwrap().to_string();
    t.ok("a pane made while it is on has a height", height(&p2) >= 1);
    e.run(&["kill-pane", "-t", &p2]);
    e.run(&["set", "-g", "pane-border-status", "off"]);
    t.ok("off: the rows are given back", reaches(&p0, t0 + 1));
    let r = e.run(&["set", "-g", "pane-border-status", "sideways"]);
    t.ok("a bad value is refused", r.all().to_lowercase().contains("pane-border-status"));
    e.kill_server();
    t.finish();
}
