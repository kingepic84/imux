//! A client attached from inside a pane of its own server is refused unless IMUX_ALLOW_NESTED=1;
//! commands that only talk to the server still work there. (The Rust version of tests/q_nested.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_nested() {
    let mut e = Env::new("qnest");
    let mut t = Check::new("q_nested");
    let conf = e.write("imux.conf", "set -g @none 1\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("IMUX_ALLOW_NESTED");
    let d = p(&e.dir);
    let cmd = [
        format!("{BIN} attach -t nest >{d}/o1 2>&1; echo rc=$? >>{d}/o1"),
        format!("{BIN} >{d}/o2 2>&1; echo rc=$? >>{d}/o2"),
        format!("{BIN} new -s other >{d}/o3 2>&1; echo rc=$? >>{d}/o3"),
        format!("{BIN} ls >{d}/o4 2>&1; echo rc=$? >>{d}/o4"),
        format!("IMUX_ALLOW_NESTED=1 timeout 2 {BIN} attach -t nest >{d}/o5 2>&1; echo rc=$? >>{d}/o5"),
        format!("IMUX_SOCKET_PATH=/elsewhere/sock timeout 2 {BIN} attach -t nest >{d}/o7 2>&1; echo rc=$? >>{d}/o7"),
        format!("{BIN} doctor >{d}/o6 2>&1; sleep 20"),
    ]
    .join("; ");
    e.run(&["new-session", "-d", "-s", "nest", &cmd]);
    t.ok("the commands ran", wait_until(Duration::from_secs(25), || !e.read("o6").is_empty()));
    let (o1, o2, o3, o4, o5, o6, o7) = (e.read("o1"), e.read("o2"), e.read("o3"), e.read("o4"), e.read("o5"), e.read("o6"), e.read("o7"));
    t.ok("attach is refused in a pane", o1.contains("inside an imux pane") && o1.contains("rc=1"));
    t.ok("so is a bare imux", o2.contains("inside an imux pane"));
    t.ok("and imux new -s", o3.contains("inside an imux pane"));
    t.ok("the refusal names the way out", o1.contains("IMUX_ALLOW_NESTED=1"));
    t.ok("a command that only talks to the server works", o4.lines().any(|l| l.starts_with("nest:")) && o4.contains("rc=0"));
    t.ok("IMUX_ALLOW_NESTED=1 lets a client attach", !o5.contains("inside an imux pane"));
    t.ok("variables inherited from another server do not count as nesting", !o7.contains("inside an imux pane"));
    t.ok("no second session was made by the refused new", !e.out(&["ls"]).lines().any(|l| l.starts_with("other:")));
    t.ok("doctor says it", o6.lines().any(|l| l.starts_with("nested attach: refused")));
    e.kill_server();
    t.finish();
}
