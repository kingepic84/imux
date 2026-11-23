//! split-window -f (the new pane spans the whole window), join-pane -b -f, and swap-pane -s -t
//! inside a window and between windows. (The Rust version of tests/q_fullsplit.sh.)
use crate::support::*;

#[test]
fn q_fullsplit() {
    let e = Env::new("qfs");
    let mut t = Check::new("q_fullsplit");
    e.session("fs", "w");
    let num = |s: String| s.parse::<i32>().unwrap_or(-1);
    let width = num(e.var("fs:w", "#{window_width}"));
    let height = num(e.var("fs:w", "#{window_height}"));
    let v = |pane: &str, var: &str| num(e.var(pane, &format!("#{{{var}}}")));
    let split = |args: &[&str]| e.out(args);
    let first = e.var("fs:w", "#{pane_id}");
    let side = split(&["split-window", "-h", "-d", "-t", &first, "-P", "-F", "#{pane_id}"]);
    t.ok("two panes side by side", v(&first, "pane_width") < width && v(&side, "pane_top") == 0);
    let full = split(&["split-window", "-v", "-f", "-d", "-t", &first, "-P", "-F", "#{pane_id}"]);
    t.ok("split-window -f -v spans the whole width", v(&full, "pane_width") == width);
    t.ok("and sits below both", v(&full, "pane_top") > 0 && v(&first, "pane_top") == 0);
    let lefty = split(&["split-window", "-h", "-f", "-b", "-d", "-l", "10", "-t", &first, "-P", "-F", "#{pane_id}"]);
    t.ok("split-window -f -b -h puts the pane on the far left", v(&lefty, "pane_left") == 0);
    t.ok("with the full height", v(&lefty, "pane_height") == height);
    t.ok("and the size asked for", v(&lefty, "pane_width") == 10);
    let top = split(&["split-window", "-v", "-f", "-b", "-d", "-l", "5", "-t", &first, "-P", "-F", "#{pane_id}"]);
    t.ok("split-window -f -b -v puts the pane on top, 5 rows", v(&top, "pane_top") == 0 && v(&top, "pane_height") == 5);
    t.ok("spanning the width", v(&top, "pane_width") == width);
    e.session("js", "a");
    e.run(&["new-window", "-d", "-t", "js", "-n", "b"]);
    let base = e.var("js:a", "#{pane_id}");
    let mover = e.var("js:b", "#{pane_id}");
    e.run(&["join-pane", "-h", "-b", "-s", &mover, "-t", &base]);
    t.ok("join-pane -b put the moved pane before the target", v(&mover, "pane_left") == 0 && v(&base, "pane_left") > 0);
    t.ok("the emptied window is gone", e.out(&["list-windows", "-t", "js", "-F", "x"]).lines().count() == 1);
    e.run(&["new-window", "-d", "-t", "js", "-n", "c"]);
    let other = e.var("js:c", "#{pane_id}");
    e.run(&["join-pane", "-v", "-f", "-s", &other, "-t", &base]);
    t.ok("join-pane -f spans the window", v(&other, "pane_width") == width && v(&other, "pane_top") > 0);
    e.session("sw", "x");
    let a = e.var("sw:x", "#{pane_id}");
    let b = split(&["split-window", "-h", "-d", "-t", &a, "-P", "-F", "#{pane_id}"]);
    let order = |tg: &str| e.out(&["list-panes", "-t", tg, "-F", "#{pane_id}"]).lines().map(|l| format!("{l} ")).collect::<String>();
    t.eq("before the swap a is first", order("sw:x"), format!("{a} {b} "));
    e.run(&["swap-pane", "-s", &a, "-t", &b]);
    t.eq("swap-pane -s -t trades places in a window", order("sw:x"), format!("{b} {a} "));
    t.ok("and the left pane is now b", v(&b, "pane_left") == 0);
    e.run(&["new-window", "-d", "-t", "sw", "-n", "y"]);
    let c = e.var("sw:y", "#{pane_id}");
    e.run(&["swap-pane", "-s", &a, "-t", &c]);
    t.ok("swap-pane between windows moves each into the other", order("sw:x") == format!("{b} {c} ") && order("sw:y") == format!("{a} "));
    t.ok("the moved panes have the room of their new place", v(&a, "pane_width") == width && v(&c, "pane_left") > 0);
    let r = e.run(&["swap-pane", "-s", &b, "-t", &b]);
    t.ok("a pane cannot swap with itself", r.all().is_empty() || r.all().contains("same pane"));
    let r = e.run(&["swap-pane", "-s", "%99999", "-t", &b]);
    t.matches("an unknown pane is an error", "can.t find", &r.all());
    e.kill_server();
    t.finish();
}
