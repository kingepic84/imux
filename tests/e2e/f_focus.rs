//! remain-on-exit (a pane whose program ended stays, pane-died, respawn-pane without -k) and
//! focus-events (client-focus and pane-focus hooks, reports passed on to panes that asked).
//! (The Rust version of tests/f_focus.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn f_focus() {
    let mut e = Env::new("ffocus");
    let mut t = Check::new("f_focus");
    let d = p(&e.dir);
    let ev = format!("{d}/ev.out");
    std::fs::write(&ev, "").unwrap();
    let conf = e.write(
        "imux.conf",
        &format!(
            "set -g automatic-rename off\nset -g remain-on-exit on\nset-hook -g pane-died 'run-shell \"echo pane-died:#{{hook_exit_status}} >> {ev}\"'\nset-hook -g client-focus-out 'run-shell \"echo client-focus-out >> {ev}\"'\nset-hook -g client-focus-in 'run-shell \"echo client-focus-in >> {ev}\"'\nset-hook -g pane-focus-out 'run-shell \"echo pane-focus-out:#{{hook_pane}} >> {ev}\"'\nset-hook -g pane-focus-in 'run-shell \"echo pane-focus-in:#{{hook_pane}} >> {ev}\"'\n"
        ),
    );
    e.set("IMUX_CONFIG", &p(&conf));
    let seen = |line: &str| wait_until(Duration::from_secs(10), || e.read("ev.out").lines().any(|l| l == line));
    let names = || e.out(&["list-windows", "-t", "f", "-F", "#{window_name}"]);
    let has = |n: &str| names().lines().any(|l| l == n);

    t.section("remain-on-exit");
    e.session("f", "base");
    e.run(&["new-window", "-d", "-t", "f", "-n", "dead", "echo hi; exit 3"]);
    t.ok("pane-died fires with the exit status", seen("pane-died:3"));
    t.ok("the pane is still there, marked dead with its status", e.out(&["list-panes", "-t", "f:dead", "-F", "#{pane_dead} #{pane_dead_status}"]) == "1 3");
    let c = e.capture("f:dead");
    t.ok("it keeps what the program printed", c.lines().any(|l| l.starts_with("hi")));
    t.ok("and says it is dead", c.contains("Pane is dead (status 3)"));
    t.ok("typing into it is an error", e.all(&["send-keys", "-t", "f:dead", "x"]).contains("dead"));
    e.run(&["respawn-pane", "-t", "f:dead", "sleep 30"]);
    t.ok("respawn-pane needs no -k for a dead pane, and it lives again", e.out(&["list-panes", "-t", "f:dead", "-F", "#{pane_dead}"]) == "0");
    e.run(&["kill-window", "-t", "f:dead"]);
    e.run(&["set", "-g", "remain-on-exit", "failed"]);
    e.run(&["new-window", "-d", "-t", "f", "-n", "ok", "exit 0"]);
    wait_until(Duration::from_secs(10), || !has("ok"));
    t.ok("with failed, a program that succeeded takes its pane with it", !has("ok"));
    e.run(&["new-window", "-d", "-t", "f", "-n", "bad", "exit 2"]);
    t.ok("with failed, one that failed stays", seen("pane-died:2"));
    e.run(&["kill-window", "-t", "f:bad"]);
    t.ok("kill-window removes a dead pane", !has("bad"));
    e.run(&["set", "-g", "remain-on-exit", "off"]);
    e.run(&["new-window", "-d", "-t", "f", "-n", "gone", "exit 1"]);
    wait_until(Duration::from_secs(10), || !has("gone"));
    t.ok("off is the old behavior", !has("gone"));

    t.section("focus-events");
    e.run(&["new-window", "-t", "f", "-n", "ask", &format!("printf '\\033[?1004h'; stty raw -echo; head -c 6 | od -c >{d}/got.out; sleep 30")]);
    std::thread::sleep(Duration::from_secs(1));
    e.with_client("f", |c| {
        std::thread::sleep(Duration::from_secs(1));
        c.send(b"\x1b[O");
        std::thread::sleep(Duration::from_secs(1));
        c.send(b"\x1b[I");
        std::thread::sleep(Duration::from_secs(1));
    });
    t.ok("client-focus-out fires when the terminal loses the focus", seen("client-focus-out"));
    t.ok("client-focus-in fires when it gets it back", seen("client-focus-in"));
    let got = wait_until(Duration::from_secs(10), || {
        let g = e.read("got.out");
        regex_lite::Regex::new("033 +\\[ +O").unwrap().is_match(&g) && regex_lite::Regex::new("033 +\\[ +I").unwrap().is_match(&g)
    });
    t.ok("the reports reach a program that asked for them", got);
    e.run(&["split-window", "-d", "-t", "f:ask"]);
    let ids = e.out(&["list-panes", "-t", "f:ask", "-F", "#{pane_id}"]);
    let (first, second) = (ids.lines().next().unwrap_or("").to_string(), ids.lines().nth(1).unwrap_or("").to_string());
    e.run(&["select-pane", "-t", &second]);
    t.ok("pane-focus-out names the pane that lost it", seen(&format!("pane-focus-out:{first}")));
    t.ok("pane-focus-in names the pane that got it", seen(&format!("pane-focus-in:{second}")));
    e.kill_server();
    t.finish();
}
