//! Format loops over panes, windows and sessions (#{P:fmt[,current]}, #{W:...}, #{S:...}) and the
//! search of a pane's screen (#{C:text}, #{C/r:...}, #{C/i:...}) in formats of the socket commands.
//! (The Rust version of tests/q_loops.sh.)
use crate::support::*;

#[test]
fn q_loops() {
    let e = Env::new("qloops");
    let mut t = Check::new("q_loops");
    e.session("s", "one");
    e.run(&["new-window", "-t", "s", "-n", "two"]);
    e.run(&["new-window", "-t", "s", "-n", "three"]);
    e.session("t", "other");
    e.run(&["select-window", "-t", "s:one"]);
    e.run(&["split-window", "-d", "-t", "s:one"]);
    e.ready("s:one", std::time::Duration::from_millis(400));
    e.run(&["send-keys", "-t", "s:one", "echo LOOPMARK-ZZZ", "Enter"]);
    e.shows("s:one", "^LOOPMARK-ZZZ$");
    let fm = |f: &str| e.var("s:one", f);

    t.section("loops");
    t.eq("P: goes over the panes of the window", fm("#{P:x}"), "xx".to_string());
    t.eq("P: sees each pane's own variables", fm("#{P:#{pane_index}.}"), "0.1.".to_string());
    t.eq("P: has a second format for the active pane", fm("#{P:[#{pane_index}],<#{pane_index}>}"), "<0>[1]".to_string());
    t.eq("W: goes over the windows of the session", fm("#{W:#{window_name} }"), "one two three ".to_string());
    t.eq("W: has a second format for the current window", fm("#{W:#{window_name},*#{window_name}}"), "*onetwothree".to_string());
    t.eq("S: goes over the sessions", fm("#{S:#{session_name} }"), "s t ".to_string());
    t.eq("S: has a second format for the session of the pane", fm("#{S:#{session_name},<#{session_name}>}"), "<s>t".to_string());
    t.eq("the loop flags are set", fm("#{P:#{?loop_first_flag,F,}#{?loop_last_flag,L,}}"), "FL".to_string());
    t.eq("a loop does not nest", fm("#{W:#{S:x}}"), "#{S:x}#{S:x}#{S:x}".to_string());
    t.eq("a name the loop lacks comes from outside", fm("#{P:#{session_name}}"), "ss".to_string());

    t.section("search");
    let n = fm("#{C:LOOPMARK-ZZZ}");
    t.ok("C: gives the row of the first line with the text", n.parse::<i32>().map(|v| v >= 1).unwrap_or(false));
    t.eq("C: gives 0 when nothing holds the text", fm("#{C:nothing-like-this}"), "0".to_string());
    t.eq("C: is case sensitive", fm("#{C:loopmark-zzz}"), "0".to_string());
    t.eq("C/i: ignores case", fm("#{C/i:loopmark-zzz}"), n.clone());
    t.ok("C/r: takes a regular expression", fm("#{C/r:LOOPMARK-ZZ+$}").parse::<i32>().map(|v| v >= 1).unwrap_or(false));
    t.eq("a pattern that is not a regular expression finds nothing", fm("#{C/r:[}"), "0".to_string());
    t.eq("C: works as a condition", fm("#{?#{C:LOOPMARK-ZZZ},found,missing}"), "found".to_string());
    e.kill_server();
    t.finish();
}
