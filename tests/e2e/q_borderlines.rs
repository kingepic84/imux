//! pane-border-lines: the characters a client is sent for the divider between two panes.
//! (The Rust version of tests/q_borderlines.sh.)
use crate::support::*;

#[test]
fn q_borderlines() {
    let e = Env::new("qbl");
    let mut t = Check::new("q_borderlines");
    e.session("sty", "one");
    e.run(&["split-window", "-d", "-h", "-t", "sty:one"]);
    let record = || e.with_client("sty", |_| std::thread::sleep(std::time::Duration::from_millis(500)));
    let all = record();
    t.ok("the default divider is the single line", all.contains('│') && !all.contains('║'));
    e.run(&["set", "-g", "pane-border-lines", "double"]);
    let all = record();
    t.ok("double draws the double line", all.contains('║') && !all.contains('│'));
    e.run(&["set", "-g", "pane-border-lines", "heavy"]);
    let all = record();
    t.ok("heavy draws the heavy line", all.contains('┃'));
    e.run(&["set", "-g", "pane-border-lines", "simple"]);
    let all = record();
    t.ok("simple draws plain characters", !all.contains('│') && !all.contains('┃'));
    let r = e.run(&["set", "-g", "pane-border-lines", "wavy"]);
    t.ok("a name that is not a line set is refused", r.all().to_lowercase().contains("pane-border-lines"));
    let r = e.run(&["show-options", "-g", "pane-border-lines"]);
    t.ok("the refused value changed nothing", r.all().lines().any(|l| l == "pane-border-lines simple"));
    e.kill_server();
    t.finish();
}
