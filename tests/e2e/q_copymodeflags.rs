//! copy-mode -t pane (any pane, the active one unchanged), -u (a page back), -H (no position at
//! the top right) and -e, from the socket. (The Rust version of tests/q_copymodeflags.sh.)
use crate::support::*;
use std::time::Duration;

fn fresh(e: &Env) {
    e.run(&["kill-session", "-t", "cp"]);
    e.run(&["new-session", "-d", "-s", "cp", "-n", "main", "seq 1 200; sleep 100"]);
    e.run(&["new-window", "-d", "-t", "cp", "-n", "other"]);
    e.shows("cp:main", "^200$");
}

fn mode(e: &Env, t: &str) -> String {
    e.out(&["display-message", "-p", "-t", t, "#{pane_in_mode}"])
}

/// Watch a client on window main while `f` runs a copy-mode command, and return what it drew.
fn watch(e: &Env, args: &[&str]) -> String {
    e.run(&["select-window", "-t", "cp:main"]);
    e.with_client("cp", |c| {
        e.run(args);
        c.wait_seen(r"\[|copy", Duration::from_secs(5));
        e.barrier(c, "cp");
    })
}

#[test]
fn q_copymodeflags() {
    let e = Env::new("qcmf");
    let mut t = Check::new("q_copymodeflags");
    fresh(&e);
    let r = e.run(&["copy-mode", "-t", "cp:other"]);
    t.ok("copy-mode -t puts a pane of another window in copy mode", mode(&e, "cp:other") == "1");
    t.ok(
        "and leaves the current window alone",
        e.out(&["display-message", "-p", "-t", "cp:", "#{window_name}"]) == "main" && mode(&e, "cp:main") == "0",
    );
    let _ = r;
    let r = e.run(&["copy-mode", "-t", "cp:other"]);
    t.ok("again is no error", r.all().is_empty());
    let r = e.run(&["copy-mode", "-t", "cp:nowhere"]);
    t.matches("an unknown target is an error", "can.t find", &r.all());
    let r = e.run(&["copy-mode", "-x"]);
    t.matches("an unknown flag is an error", "unknown flag|usage", &r.all());
    fresh(&e);
    let all = watch(&e, &["copy-mode"]);
    t.matches("plain: the position is [0/n]", r"\[0/[0-9]+\]", &all);
    fresh(&e);
    let all = watch(&e, &["copy-mode", "-u"]);
    t.matches("-u starts a page back", r"\[[1-9][0-9]*/[0-9]+\]", &all);
    t.lacks("-u is not at the bottom", r"\[0/[0-9]+\]", &all);
    fresh(&e);
    let all = watch(&e, &["copy-mode", "-H"]);
    t.lacks("-H leaves the position out", r"\[[0-9]+/[0-9]+\]", &all);
    t.ok("-H still is copy mode", mode(&e, "cp:main") == "1");
    e.kill_server();
    t.finish();
}
