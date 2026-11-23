//! display-panes-color and display-panes-active-color: the backgrounds of the pane numbers that
//! prefix q draws. Each color is unique, so the bytes a client writes show which label took which.
//! (The Rust version of tests/q_panescolor.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_panescolor() {
    let e = Env::new("qpc");
    let mut t = Check::new("q_panescolor");
    e.session("sty", "one");
    e.run(&["split-window", "-d", "-h", "-t", "sty:one"]);
    let record = || {
        e.with_client("sty", |c| {
            c.send(b"\x02q");
            c.wait_seen("0", Duration::from_secs(2));
            e.barrier(c, "sty");
            c.send(b"\x1b");
            c.escape_pause();
            c.sync();
        })
    };
    let all = record();
    t.lacks("built-in colors: neither option's color shows", "48;5;20[12]", &all);
    e.run(&["set", "-g", "display-panes-color", "colour201"]);
    e.run(&["set", "-g", "display-panes-active-color", "colour202"]);
    let all = record();
    t.ok("the other pane's label takes display-panes-color", all.contains("48;5;201"));
    t.ok("the active pane's label takes display-panes-active-color", all.contains("48;5;202"));
    let r = e.run(&["set", "-g", "display-panes-color", "not a color"]);
    t.ok("a bad color is refused", r.all().to_lowercase().contains("display-panes-color"));
    e.kill_server();
    t.finish();
}
