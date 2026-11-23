//! move-window -a (after the target), -k (kill the window in the way) and -r (renumber, nothing to
//! do). (The Rust version of tests/q_movewin.sh.)
use crate::support::*;

#[test]
fn q_movewin() {
    let e = Env::new("qmw");
    let mut t = Check::new("q_movewin");
    let order = |s: &str| e.out(&["list-windows", "-t", s, "-F", "#{window_name}"]).lines().map(|l| format!("{l} ")).collect::<String>();
    let fresh = |s: &str| {
        e.run(&["kill-session", "-t", s]);
        e.run(&["new-session", "-d", "-s", s, "-n", "a"]);
        for n in ["b", "c", "d"] {
            e.run(&["new-window", "-d", "-t", s, "-n", n]);
        }
    };
    e.session("keep", "k");
    fresh("m");
    e.run(&["move-window", "-a", "-s", "m:a", "-t", "m:c"]);
    t.eq("-a moves a window after a later target", order("m"), "b c a d ".to_string());
    fresh("m");
    e.run(&["move-window", "-a", "-s", "m:d", "-t", "m:a"]);
    t.eq("-a moves a window after an earlier target", order("m"), "a d b c ".to_string());
    fresh("m");
    e.run(&["move-window", "-s", "m:d", "-t", "m:b"]);
    t.eq("without -a the window takes the place of the target", order("m"), "a d b c ".to_string());
    fresh("m");
    e.run(&["move-window", "-k", "-s", "m:d", "-t", "m:b"]);
    t.eq("-k kills the window in the way", order("m"), "a d c ".to_string());
    fresh("m");
    let r = e.run(&["move-window", "-r", "-t", "m:"]);
    t.ok("-r with a target is accepted and changes nothing", order("m") == "a b c d " && r.all().is_empty());
    let r = e.run(&["move-window", "-r"]);
    t.ok("-r alone is accepted", r.code == 0 && r.all().is_empty());
    fresh("x");
    e.run(&["new-session", "-d", "-s", "y", "-n", "r"]);
    e.run(&["new-window", "-d", "-t", "y", "-n", "s"]);
    e.run(&["move-window", "-a", "-s", "y:r", "-t", "x:b"]);
    t.eq("-a across sessions", order("x"), "a b r c d ".to_string());
    fresh("m");
    e.run(&["move-window", "-a", "-k", "-s", "m:a", "-t", "m:c"]);
    t.eq("-a with -k kills nothing", order("m"), "b c a d ".to_string());
    fresh("m");
    let r = e.run(&["swap-window", "-k", "-s", "m:a", "-t", "m:b"]);
    t.ok("swap-window takes no -k", !r.all().is_empty() && order("m") == "a b c d ");
    e.kill_server();
    t.finish();
}
