//! By default a client asks its terminal what it supports; with image-probe-timeout 0 it does not.
//! (The Rust version of tests/t_probe.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

fn run_client(e: &Env) -> Client {
    let mut c = e.client_env(100, 30, &["new", "-s", "p"], &[("TERM_PROGRAM", "iTerm.app")], &["LC_TERMINAL", "COLORTERM", "KITTY_WINDOW_ID"]);
    assert!(e.has_client());
    assert!(e.ready("p:", Duration::from_millis(600)));
    c.send_str("echo up-$((1+2))\r");
    assert!(c.wait_for("up-3", LONG));
    e.barrier(&c, "p");
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    c
}

#[test]
fn t_probe() {
    let mut e = Env::new("tpr");
    let conf = e.write("imux.conf", "set image-probe-timeout 0\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("t_probe");
    let esc = "\x1b";
    let off = run_client(&e);
    e.write("imux.conf", "");
    let on = run_client(&e);
    t.ok("by default the terminal is asked what it supports", on.has_bytes(format!("{esc}_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA{esc}\\{esc}[c").as_bytes()));
    t.ok("with image-probe-timeout 0 it is not asked", !off.has_bytes(format!("{esc}_Gi=31").as_bytes()));
    t.ok("and the session works as before", off.seen("up-3"));
    t.finish();
}
