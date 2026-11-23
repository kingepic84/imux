//! The fuzzy filter in pickers (`/` then a query): the session picker, the repository wizard, a text
//! list, and the keys that end or clear a query. (The Rust version of tests/q_filter.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

fn session_of_client(e: &Env) -> String {
    e.out(&["list-clients", "-F", "#{client_session}"]).lines().next().unwrap_or("").to_string()
}

fn until_session(e: &Env, name: &str) -> bool {
    wait_until(LONG, || session_of_client(e) == name)
}

/// A client attached to `alpha` (or `session`), ready to take keys.
fn attach(e: &Env, session: &str) -> Client {
    let c = e.client(100, 30, &["attach", "-t", session]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c
}

fn keys(c: &mut Client, seq: &[&str]) {
    for k in seq {
        c.send(k.as_bytes());
        if *k == "\x1b" {
            c.escape_pause();
        }
    }
}

fn leave(e: &Env, mut c: Client) {
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
    assert!(e.no_clients(), "the client did not go away");
}

#[test]
fn q_filter() {
    let e = Env::new("qfl");
    let mut t = Check::new("q_filter");
    for s in ["alpha", "beta", "gamma"] {
        e.run(&["new-session", "-d", "-s", s]);
    }

    t.section("the session picker");
    let mut c = attach(&e, "alpha");
    keys(&mut c, &["\x02s", "/", "bet"]);
    // the box is drawn with the query on its border before Enter closes it
    t.ok("the query and its count are on the box's border", c.wait_seen("/bet_ 1/3", Duration::from_secs(15)));
    keys(&mut c, &["\r", "\r"]);
    t.ok("typing a query and Enter twice switches to the session that matches", until_session(&e, "beta"));
    leave(&e, c);

    t.section("keys of the picker are free until a slash");
    let mut c = attach(&e, "alpha");
    keys(&mut c, &["\x02s", "j", "j", "\r"]);
    t.ok("j and Enter still move and choose", until_session(&e, "gamma"));
    leave(&e, c);

    t.section("Escape clears the query, a second Escape closes");
    let mut c = attach(&e, "alpha");
    keys(&mut c, &["\x02s", "/", "gam", "\x1b", "k", "\r"]);
    t.ok("after Escape the whole list is back, still on gamma, and k moves up in it", until_session(&e, "beta"));
    leave(&e, c);
    let mut c = attach(&e, "alpha");
    keys(&mut c, &["\x02s", "/", "gam", "\x1b", "\x1b", "\x1b", "\r"]);
    c.sync();
    t.ok("Escape after Escape closes the picker, so the client stays where it was", session_of_client(&e) == "alpha");
    leave(&e, c);

    t.section("no match, then Enter");
    let mut c = attach(&e, "alpha");
    keys(&mut c, &["\x02s", "/", "zzz", "\r", "\r"]);
    c.sync();
    t.ok("choosing from an empty list closes it and changes nothing", session_of_client(&e) == "alpha");
    leave(&e, c);

    t.section("the repository wizard");
    for r in ["apple", "banana", "cherry"] {
        std::fs::create_dir_all(e.path(&format!("repos/{r}/.git"))).unwrap();
    }
    e.run(&["set", "-g", "repos-dir", &p(&e.path("repos"))]);
    let mut c = attach(&e, "alpha");
    keys(&mut c, &["\x02W", "/", "ban", "\r", "\r"]);
    t.ok("the filtered repository asks for a worktree name", c.wait_seen("worktree name", LONG));
    keys(&mut c, &["\x1b"]);
    leave(&e, c);

    t.section("a text list");
    let mut c = attach(&e, "alpha");
    keys(&mut c, &["\x02?", "/", "detach"]);
    t.ok("list-keys can be filtered too", c.wait_seen("/detach_ ", LONG));
    keys(&mut c, &["\x1b", "\x1b"]);
    leave(&e, c);

    t.section("the agent panel");
    for n in ["red", "green"] {
        let script = e.write(&format!("report-{n}.sh"), &format!("#!/bin/sh\n{BIN} pane report-agent --source hook --agent fake --state working --seq 1 --message 'status {n}'\nsleep 100\n"));
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let name = format!("ag-{n}");
        e.run(&["new-session", "-d", "-s", &name]);
        assert!(e.ready(&format!("{name}:"), Duration::from_millis(600)), "the pane of {name} never settled");
        e.run(&["send-keys", "-t", &format!("{name}:"), &p(&script), "Enter"]);
    }
    e.run(&["new-session", "-d", "-s", "home"]);
    assert!(wait_until(LONG, || e.out(&["agent", "list"]).matches("status ").count() >= 2), "the two agents did not report");
    let mut c = attach(&e, "home");
    keys(&mut c, &["\x02A", "/", "green", "\r", "\r"]);
    t.ok("a query in the agent panel picks the row whose message matches", until_session(&e, "ag-green"));
    leave(&e, c);

    e.kill_server();
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
