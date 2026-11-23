//! Layout strings (`#{window_layout}`, `select-layout "string"`), `select-layout -t`, `resize-pane
//! -t -x -y`, `move-window` and `swap-window` with `session:N` targets, from the socket with no
//! client attached. (The Rust version of tests/q_layout.sh.)
use crate::support::*;

#[test]
fn q_layout() {
    let e = Env::new("qlay");
    let mut t = Check::new("q_layout");
    e.session("lay", "a");
    e.run(&["split-window", "-d", "-t", "lay:0"]);
    e.run(&["split-window", "-d", "-t", "lay:0"]);
    let layout = |tg: &str| e.out(&["display-message", "-p", "-t", tg, "#{window_layout}"]);
    let join = |tg: &str, f: &str| e.out(&["list-panes", "-t", tg, "-F", f]).lines().map(|l| format!("{l} ")).collect::<String>();
    let widths = |tg: &str| join(tg, "#{pane_width}");
    let heights = |tg: &str| join(tg, "#{pane_height}");
    t.matches("window_layout is a classic layout string", r"^[0-9a-f]{4},[0-9]+x[0-9]+,0,0[\[{]", &layout("lay:0"));
    e.run(&["select-layout", "-t", "lay:0", "even-horizontal"]);
    t.ok("select-layout -t takes a window and a preset", layout("lay:0").contains('{'));
    let saved = layout("lay:0");
    let saved_widths = widths("lay:0");
    e.run(&["select-layout", "-t", "lay:0", "even-vertical"]);
    t.ok("another preset changes the layout", layout("lay:0") != saved);
    e.run(&["select-layout", "-t", "lay:0", &saved]);
    t.ok("the saved layout string puts the layout back", layout("lay:0") == saved && widths("lay:0") == saved_widths);
    e.run(&["select-layout", "-t", "lay:0", "0000,80x24,0,0{40x24,0,0,0,39x24,41,0,1}"]);
    t.ok("a layout string with a wrong checksum or pane count is refused and nothing changes", layout("lay:0") == saved);
    t.ok("an unknown layout name is an error", !e.run(&["select-layout", "-t", "lay:0", "nonsense"]).success());
    e.run(&["select-layout", "-t", "lay:0", "even-horizontal"]);
    let first = e.out(&["list-panes", "-t", "lay:0", "-F", "#{pane_id}"]).lines().next().unwrap().to_string();
    let before = widths("lay:0");
    e.run(&["resize-pane", "-t", &first, "-x", "50"]);
    let after = widths("lay:0");
    t.ok("resize-pane -t -x makes that pane 50 wide", after.split(' ').next() == Some("50") && before != after);
    e.run(&["select-layout", "-t", "lay:0", "even-vertical"]);
    e.run(&["resize-pane", "-t", &first, "-y", "12"]);
    t.ok("resize-pane -t -y makes that pane 12 high", heights("lay:0").split(' ').next() == Some("12"));
    e.run(&["resize-pane", "-t", &first, "-D", "2"]);
    t.ok("resize-pane -t -D 2 grows it by two", heights("lay:0").split(' ').next() == Some("14"));
    t.ok("resize-pane -x with no number is refused", !e.run(&["resize-pane", "-t", &first, "-x"]).success());
    t.ok("resize-pane -x 0 is refused", !e.run(&["resize-pane", "-t", &first, "-x", "0"]).success());
    e.run(&["new-window", "-t", "lay", "-n", "b"]);
    e.run(&["new-window", "-t", "lay", "-n", "c"]);
    let order = |s: &str| e.out(&["list-windows", "-t", s, "-F", "#{window_name}"]).lines().map(|l| format!("{l} ")).collect::<String>();
    t.eq("three windows a b c", order("lay"), "a b c ".to_string());
    e.run(&["move-window", "-s", "lay:0", "-t", "lay:2"]);
    t.eq("move-window -s lay:0 -t lay:2 moves a behind c", order("lay"), "b c a ".to_string());
    e.run(&["swap-window", "-s", "lay:0", "-t", "lay:2"]);
    t.eq("swap-window -s lay:0 -t lay:2 trades b and a", order("lay"), "a c b ".to_string());
    e.run(&["new-session", "-d", "-s", "other", "-n", "x"]);
    e.run(&["move-window", "-s", "lay:1", "-t", "other:5"]);
    t.eq("move-window across sessions takes the window out of the first", order("lay"), "a b ".to_string());
    t.eq("and puts it into the second", order("other"), "x c ".to_string());
    t.ok("swap-window across sessions is refused", !e.run(&["swap-window", "-s", "lay:0", "-t", "other:0"]).success());
    e.run(&["new-session", "-d", "-s", "solo", "-n", "only"]);
    t.ok("moving the only window of a session is refused", !e.run(&["move-window", "-s", "solo:0", "-t", "lay:5"]).success() && e.out(&["list-windows", "-t", "solo", "-F", "#{window_name}"]) == "only");
    e.kill_server();
    t.finish();
}
