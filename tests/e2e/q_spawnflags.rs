//! `-e NAME=VALUE` on new-window and split-window, and `split-window -b` (the new pane goes
//! before). (The Rust version of tests/q_spawnflags.sh.)
use crate::support::*;

#[test]
fn q_spawnflags() {
    let e = Env::new("qspf");
    let mut t = Check::new("q_spawnflags");
    e.session("sp", "one");
    let id = e.out(&["new-window", "-d", "-t", "sp", "-P", "-F", "#{pane_id}", "-e", "FOO=bar", "-e", "BAZ=a b", "echo got-$FOO-$BAZ; sleep 30"]);
    t.ok("new-window -e gives the program its variables", e.shows(&id, "got-bar-a b"));
    let id = e.out(&["split-window", "-d", "-t", "sp:0", "-P", "-F", "#{pane_id}", "-e", "ZED=zz", "echo split-$ZED; sleep 30"]);
    t.ok("split-window -e too", e.shows(&id, "split-zz"));
    let r = e.run(&["new-window", "-d", "-t", "sp", "-e", "IMUX_PANE_ID=%99"]);
    t.ok("an IMUX_ name is refused", r.all().contains("IMUX_PANE_ID is set by imux"));
    let r = e.run(&["new-window", "-d", "-t", "sp", "-e", "NOEQUALS"]);
    t.ok("a pair without = is refused", r.all().contains("NAME=VALUE"));
    let r = e.run(&["split-window", "-d", "-t", "sp:0", "-e", "bad-name=1"]);
    t.ok("a bad name is refused", r.all().contains("bad variable name"));
    t.ok("nothing was made by the refused ones", e.out(&["list-windows", "-t", "sp"]).lines().count() == 2);
    let first = e.out(&["list-panes", "-t", "sp:0", "-F", "#{pane_id}"]).lines().next().unwrap().to_string();
    e.run(&["kill-pane", "-a", "-t", &first]);
    e.run(&["select-pane", "-t", "sp:0.0"]);
    let newid = e.out(&["split-window", "-b", "-h", "-l", "20", "-t", "sp:0.0", "-P", "-F", "#{pane_id}"]);
    let first = e.out(&["list-panes", "-t", "sp:0", "-F", "#{pane_id}"]).lines().next().unwrap().to_string();
    t.ok("split-window -b puts the new pane first", first == newid);
    let width = e.out(&["list-panes", "-t", "sp:0", "-F", "#{pane_id} #{pane_width}"]).lines().find_map(|l| l.strip_prefix(&format!("{newid} ")).map(String::from)).unwrap_or_default();
    t.eq("-l sizes the new pane, not the old one", width, "20".to_string());
    e.kill_server();
    t.finish();
}
