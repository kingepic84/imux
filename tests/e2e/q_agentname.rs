//! P10.6, names of agents: `imux agent name PANE NAME|--clear`, the name in `agent get|list`, the
//! agent commands and every `-t` taking a name (`agent:NAME`), uniqueness, the charset, a name going
//! with its record (release, another program, a new record), the event, a snapshot round trip, and
//! the rules for a program in a pane. (The Rust version of tests/q_agentname.sh.)
use crate::support::*;
use std::process::Stdio;
use std::time::Duration;

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

fn report(e: &Env, pane: &str, state: &str, agent: &str) {
    in_pane(e, pane, &format!("{BIN} pane report-agent --source t --agent {agent} --state {state}"));
}

#[test]
fn q_agentname() {
    let mut e = Env::new("qan");
    let mut t = Check::new("q_agentname");
    let conf = e.write("imux.conf", "set -g status off\nset -g default-size 100x30\nset -g agent-control on\nset -g restore-commands \"claude\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    e.session("an", "w");
    e.run(&["split-window", "-d"]);
    e.run(&["split-window", "-d"]);
    for pane in ["%0", "%1", "%2"] {
        assert!(e.ready(pane, Duration::from_millis(500)));
    }
    hold(&e, "0");
    hold(&e, "1");
    report(&e, "0", "idle", "fake");
    report(&e, "1", "idle", "fake");
    let r = |args: &[&str]| e.run(args);

    t.section("giving a name");
    let x = r(&["agent", "name", "0", "alpha"]);
    t.ok("agent name answers ok", x.code == 0 && has_line(&x.out, "ok"));
    t.ok("agent get shows the name", has_line(&r(&["agent", "get", "0"]).out, "name: alpha"));
    t.ok("agent get --json has name next to agent", r(&["agent", "get", "0", "--json"]).out.contains("\"agent\":\"fake\",\"name\":\"alpha\""));
    t.ok("agent list shows the name for the pane", r(&["agent", "list"]).out.contains("alpha"));
    let x = r(&["agent", "name", "0", "alpha", "--json"]);
    t.matches("the same name on the same pane is fine, --json says it", r#"^\{"pane":"%0","name":"alpha"(,"seq":[0-9]+)?\}$"#, x.out.trim_end());
    let x = r(&["agent", "name", "1", "alpha", "--json"]);
    t.ok("a name in use is invalid_argument and says whose", x.code == 1 && x.err.contains("\"code\":\"invalid_argument\"") && x.err.contains("pane 0"));
    t.ok("another name for another pane", r(&["agent", "name", "1", "beta"]).code == 0);
    for bad in ["Bad", "1a", "a:b", "a.b", "a b", "-x", "_x", "%1"] {
        t.ok(&format!("the name '{bad}' is refused"), r(&["agent", "name", "1", "--", bad]).code != 0);
    }
    let x = r(&["agent", "name", "1", &"a".repeat(33), "--json"]);
    t.ok("a name of 33 characters is refused", x.code == 1 && x.err.contains("\"code\":\"invalid_argument\""));
    let x = r(&["agent", "name", "2", "gamma", "--json"]);
    t.ok("a pane with no record cannot be named", x.code == 1 && x.err.contains("\"code\":\"agent_not_running\""));
    let x = r(&["agent", "name", "99", "gamma", "--json"]);
    t.ok("a missing pane is not_found", x.code == 1 && x.err.contains("\"code\":\"not_found\""));
    t.ok("no name and no --clear is a usage error, exit 2", r(&["agent", "name", "1"]).code == 2);

    t.section("the agent commands take the name");
    let x = r(&["agent", "get", "beta"]);
    t.ok("agent get by name", x.code == 0 && has_line(&x.out, "pane: 1") && has_line(&x.out, "name: beta"));
    let x = r(&["agent", "get", "--json", "beta"]);
    t.ok("flags may come before the name", x.code == 0 && x.out.contains("\"pane\":\"%1\""));
    let x = r(&["agent", "wait", "beta", "--until", "idle", "--timeout", "3"]);
    t.ok("agent wait by name", x.code == 0 && has_line(&x.out, "idle"));
    t.ok("agent read by name reads that pane", r(&["agent", "read", "alpha", "--lines", "3"]).code == 0);
    t.ok("and the right one", r(&["agent", "read", "beta", "--lines", "20"]).out.contains("fifo1"));
    let x = r(&["agent", "prompt", "beta", "hello", "there"]);
    t.ok("agent prompt by name", x.code == 0 && has_line(&x.out, "sent"));
    let x = r(&["agent", "get", "nosuch", "--json"]);
    t.ok("a name that matches nothing is not_found", x.code == 1 && x.err.contains("\"code\":\"not_found\"") && x.err.contains("nosuch"));
    let x = r(&["agent", "get", "nosuch"]);
    t.matches("plain: the same text, no code, exit 1", "(?m)^imux: can.t find agent: nosuch", &x.out);
    t.ok("(exit 1)", x.code == 1);
    let x = r(&["agent", "get", "2"]);
    t.ok("a number still means a pane", x.code == 1 && x.out.contains("pane 2 has no agent record"));

    t.section("-t agent:NAME");
    let x = r(&["capture-pane", "-p", "-t", "agent:beta"]);
    t.ok("capture-pane -t agent:NAME reads that pane", x.code == 0 && x.out.contains("fifo1") && !x.out.contains("fifo0"));
    let x = r(&["display-message", "-p", "-t", "agent:alpha", "#{pane_id} #{agent_name}"]);
    t.ok("display-message sees the pane and #{agent_name}", x.code == 0 && has_line(&x.out, "%0 alpha"));
    r(&["send-keys", "-t", "agent:beta", "-l", "ZZTOP-MARK"]);
    t.ok("send-keys -t agent:NAME types into that pane", e.shows("%1", "ZZTOP-MARK"));
    t.ok("and not into the other", !e.capture("%0").contains("ZZTOP-MARK"));
    let x = r(&["capture-pane", "-p", "-t", "agent:nosuch"]);
    t.matches("a missing name is an error, not another target", "can.t find agent: nosuch", &x.all());
    r(&["new-session", "-d", "-s", "agent"]);
    let x = r(&["display-message", "-p", "-t", "agent:alpha", "#{pane_id}"]);
    t.ok("a session called agent does not take the target", x.code == 0 && has_line(&x.out, "%0"));
    r(&["kill-session", "-t", "agent"]);

    t.section("the name goes with the record");
    t.ok("--clear takes the name off", r(&["agent", "name", "1", "--clear"]).code == 0);
    let x = r(&["agent", "get", "1"]);
    t.ok("the record has none", x.code == 0 && !x.out.lines().any(|l| l.starts_with("name:")));
    let x = r(&["agent", "get", "beta", "--json"]);
    t.ok("and the name matches nothing", x.code == 1 && x.err.contains("not_found"));
    r(&["agent", "name", "1", "delta"]);
    report(&e, "1", "working", "fake");
    let x = r(&["agent", "get", "delta"]);
    t.ok("a new state of the same program keeps the name", x.code == 0 && has_line(&x.out, "state: working"));
    report(&e, "1", "idle", "other");
    let x = r(&["agent", "get", "1"]);
    t.ok("another program taking the pane takes the name", x.code == 0 && !x.out.lines().any(|l| l.starts_with("name:")));
    r(&["agent", "name", "1", "delta"]);
    in_pane(&e, "1", &format!("{BIN} pane release-agent --source t"));
    let x = r(&["agent", "get", "delta", "--json"]);
    t.ok("a released record takes the name with it", x.code == 1 && x.err.contains("not_found"));
    report(&e, "1", "idle", "fake");
    let x = r(&["agent", "get", "1"]);
    t.ok("a new record starts without one", x.code == 0 && !x.out.lines().any(|l| l.starts_with("name:")));

    t.section("the event");
    let events = e.path("events");
    let mut ev = e
        .command(BIN)
        .args(["events", "--types", "agent-state-changed"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::fs::File::create(&events).unwrap()))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // a name set before the subscriber is known to the server is lost, so it is set (and changed
    // back and forth) until the event has come out
    wait_until(Duration::from_secs(20), || {
        r(&["agent", "name", "1", "zeta"]);
        r(&["agent", "name", "1", "epsilon"]);
        wait_until(Duration::from_millis(500), || e.read("events").contains("\"name\":\"epsilon\""))
    });
    t.matches("agent-state-changed carries the name when it is set", "\"event\":\"agent-state-changed\".*\"name\":\"epsilon\"", &e.read("events"));
    let _ = ev.kill();
    let _ = ev.wait();

    t.section("from a pane");
    r(&["agent", "name", "1", "--clear"]);
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} agent name 0 alpha"));
    t.ok("a program may name the agent in its own pane", rc == 0);
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent name 1 zed"));
    t.ok("not the agent in a pane it did not spawn", rc != 0 && o.contains("not one this pane spawned"));
    t.ok("and nothing changed", !r(&["agent", "get", "1"]).out.lines().any(|l| l.starts_with("name:")));
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent get alpha"));
    t.ok("naming and looking up is free to look at", rc == 0 && has_line(&o, "name: alpha"));
    r(&["set", "-g", "command-from-panes", "off"]);
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} agent name 0 other"));
    t.ok("with command-from-panes off it only looks", rc != 0);
    r(&["set", "-g", "command-from-panes", "spawned"]);

    t.section("a snapshot keeps the name");
    r(&["agent", "name", "1", "keeper"]);
    let kidx = e.out(&["list-panes", "-t", "an", "-F", "#{pane_id}"]).lines().position(|l| l == "%1").unwrap() + 1;
    r(&["save", "snap1"]);
    let snap = e.read("state/snapshots/snap1.imux");
    t.ok("the snapshot file has the names", snap.contains(" +name alpha") && snap.contains(" +name keeper"));
    e.kill_server();
    e.run(&["new-session", "-d", "-s", "other"]);
    t.ok("the snapshot restores", wait_until(Duration::from_secs(15), || r(&["restore", "snap1"]).code == 0));
    let listing = e.out(&["list-panes", "-a", "-F", "#{session_name} #{pane_id}"]);
    let restored: Vec<String> = listing.lines().filter_map(|l| l.strip_prefix("an ")).map(|s| s.trim_start_matches('%').to_string()).collect();
    let (p0, p1) = (restored.first().cloned().unwrap_or_default(), restored.get(kidx - 1).cloned().unwrap_or_default());
    t.ok("two panes came back", !p0.is_empty() && !p1.is_empty());
    t.ok("no record yet, so the name matches nothing", r(&["agent", "get", "alpha"]).code == 1);
    e.ready(&format!("%{p0}"), Duration::from_millis(500));
    e.ready(&format!("%{p1}"), Duration::from_millis(500));
    let done = format!("{BIN} pane report-agent --source t --agent fake --state done");
    r(&["pane", "run", &p0, &done]);
    let got = wait_until(Duration::from_secs(15), || {
        let x = r(&["agent", "get", "alpha", "--json"]);
        x.code == 0 && x.out.contains(&format!("\"pane\":\"%{p0}\""))
    });
    t.ok("the first record of the restored pane gets its name back", got);
    t.ok("the other one still waits for its program", r(&["agent", "get", "keeper", "--json"]).code == 1);
    r(&["pane", "run", &p1, &done]);
    let got = wait_until(Duration::from_secs(15), || {
        let x = r(&["agent", "get", "keeper", "--json"]);
        x.code == 0 && x.out.contains(&format!("\"pane\":\"%{p1}\""))
    });
    t.ok("and gets it too", got);
    e.kill_server();
    t.finish();
}
