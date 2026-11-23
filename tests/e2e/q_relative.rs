//! Relative targets from a bound key: `swap-window -t -1|+1`, `select-pane -t :.+`, and the
//! clustered flags of `choose-tree -Zw`. (The Rust version of tests/q_relative.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn order(e: &Env) -> String {
    e.out(&["list-windows", "-t", "rel", "-F", "#{window_name}"]).lines().collect::<Vec<_>>().join(" ") + " "
}

fn current(e: &Env) -> String {
    e.out(&["display-message", "-p", "-t", "rel:", "#{window_name}"])
}

fn pane(e: &Env) -> String {
    e.out(&["display-message", "-p", "-t", "rel:", "#{pane_index}"])
}

#[test]
fn q_relative() {
    let mut e = Env::new("qre");
    let conf = e.write("imux.conf", "bind -n C-S-Left swap-window -t -1\nbind -n C-S-Right swap-window -t +1\nbind o select-pane -t :.+\nbind T choose-tree -Zw\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_relative");

    e.session("rel", "a");
    e.run(&["new-window", "-t", "rel", "-n", "b"]);
    e.run(&["new-window", "-t", "rel", "-n", "c"]);
    let keys = e.all(&["list-keys"]);
    t.ok("the bindings loaded", keys.contains("swap-window -t -1") && keys.contains("swap-window -t +1") && keys.contains("select-pane -t :.+"));

    e.with_client("rel", |c| {
        c.send(b"\x1b[1;6D");
        t.ok("swap-window -t -1 trades with the window before and follows it", wait_until(LONG, || order(&e) == "a c b " && current(&e) == "c"));
        c.send(b"\x1b[1;6D");
        t.ok("again", wait_until(LONG, || order(&e) == "c a b " && current(&e) == "c"));
        c.send(b"\x1b[1;6D");
        c.sync();
        t.ok("at the first window it stays and nothing is lost", order(&e) == "c a b ");
        c.send(b"\x1b[1;6C");
        t.ok("swap-window -t +1 trades with the window after", wait_until(LONG, || order(&e) == "a c b " && current(&e) == "c"));
    });

    e.run(&["select-window", "-t", "rel:0"]);
    e.run(&["split-window", "-d", "-t", "rel:0"]);
    e.run(&["split-window", "-d", "-t", "rel:0"]);
    e.run(&["select-pane", "-t", "rel:0.0"]);
    t.ok("three panes, first active", pane(&e) == "0");
    e.with_client("rel", |c| {
        c.send(b"\x02o");
        t.ok("select-pane -t :.+ moves to the next pane", wait_until(LONG, || pane(&e) == "1"));
        c.send(b"\x02o");
        assert!(wait_until(LONG, || pane(&e) == "2"));
        c.send(b"\x02o");
        t.ok("and wraps after the last", wait_until(LONG, || pane(&e) == "0"));
    });
    let out = e.with_client("rel", |c| {
        c.send(b"\x02T");
        c.wait_seen("sessions and windows", LONG);
        c.send(b"\x1b");
    });
    t.ok("choose-tree -Zw opens the window picker", out.contains("sessions and windows"));
    e.kill_server();
    t.finish();
}
