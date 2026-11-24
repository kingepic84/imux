//! `imux agent session PANE NAME|--clear`: the name of the program's own session (Claude Code's
//! /rename), given by its hook. `agent get` shows it and the one before it, saying the same name
//! again changes nothing, the agent panel shows it for an agent with no name of its own, it is
//! cleaned and cut, and from a pane it is allowed only on the pane itself and the panes it spawned.
use crate::support::*;
use std::time::Duration;

#[test]
fn q_agentsession() {
    let mut e = Env::new("qas2");
    let conf = e.write("imux.conf", "set -g status off\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_agentsession");
    let b = BIN;
    e.session("as", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    let report = "printf '\\033]7501;state=idle:app=claude-code\\033\\\\' >/dev/tty; n=0; until imux agent get \"$IMUX_PANE_ID\" >/dev/null 2>&1 || [ $n -ge 300 ]; do n=$((n+1)); done";
    let _ = b;

    t.section("a pane with no record has no session name to set");
    let r = e.run(&["agent", "session", "0", "x", "--json"]);
    t.ok("agent_not_running", r.code == 1 && r.err.contains("agent_not_running"));
    let r = e.run(&["agent", "session", "99", "x", "--json"]);
    t.ok("and a pane that is not there is not_found", r.code == 1 && r.err.contains("not_found"));

    t.section("an agent that reported");
    hold(&e, "0");
    let (rc, _) = in_pane(&e, "0", report);
    t.ok("the pane reports as an agent", rc == 0);
    let r = e.run(&["agent", "session", "0", "Homelab and imux", "--json"]);
    t.ok("a session name is set", r.code == 0 && r.out.contains("\"session\":\"Homelab and imux\""));
    let r = e.run(&["agent", "get", "0"]);
    t.matches("agent get shows it", "(?m)^agent_session: Homelab and imux$", &r.out);
    t.lacks("and no name before it", "previous_agent_session", &r.out);
    let r = e.run(&["agent", "get", "0", "--json"]);
    t.ok("the JSON has it too", r.out.contains("\"agent_session\":\"Homelab and imux\"") && !r.out.contains("previous_agent_session"));
    t.ok("the agent has no agent name from it", !r.out.contains("\"name\":"));

    t.section("the same name again changes nothing");
    for _ in 0..3 {
        e.run(&["agent", "session", "0", "Homelab and imux"]);
    }
    let r = e.run(&["agent", "get", "0"]);
    t.ok("the previous name is still empty", r.out.contains("agent_session: Homelab and imux") && !r.out.contains("previous_agent_session"));

    t.section("a new name keeps the one before it");
    e.run(&["agent", "session", "0", "Second"]);
    let r = e.run(&["agent", "get", "0"]);
    t.ok("both are there", r.out.contains("agent_session: Second") && r.out.contains("previous_agent_session: Homelab and imux"));
    e.run(&["agent", "session", "0", "Second"]);
    let r = e.run(&["agent", "get", "0"]);
    t.ok("saying it again keeps both", r.out.contains("agent_session: Second") && r.out.contains("previous_agent_session: Homelab and imux"));
    t.ok("the agent panel shows the session name for an agent with no name", e.out(&["agent", "list"]).contains("\tSecond\t"));
    e.run(&["agent", "name", "0", "boss"]);
    t.ok("and the agent name when there is one", e.out(&["agent", "list"]).contains("\tboss\t") && !e.out(&["agent", "list"]).contains("\tSecond\t"));
    let r = e.run(&["agent", "get", "boss", "--json"]);
    t.ok("both are still readable by the agent name", r.out.contains("\"agent_session\":\"Second\"") && r.out.contains("\"name\":\"boss\""));

    t.section("text from a program is cleaned and cut");
    e.run(&["agent", "session", "0", &format!("a\u{1b}[31mb\u{202e}c{}", "x".repeat(300))]);
    let r = e.run(&["agent", "get", "0", "--json"]);
    let got = r.out.split("\"agent_session\":\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("").to_string();
    t.ok("controls and bidi marks are gone", got.starts_with("a[31mbc") && !got.contains('\u{1b}') && !got.contains('\u{202e}'));
    t.ok("and it is cut at 128 characters", got.chars().count() == 128);

    t.section("clearing, and the arguments");
    let r = e.run(&["agent", "session", "0", "--clear", "--json"]);
    t.ok("--clear takes it off", r.code == 0 && r.out.contains("\"session\":null"));
    let r = e.run(&["agent", "get", "0"]);
    t.ok("the name before it stays", !r.out.lines().any(|l| l.starts_with("agent_session:")) && r.out.contains("previous_agent_session:"));
    let r = e.run(&["agent", "session", "0"]);
    t.ok("a name or --clear is needed", r.code == 2);
    let r = e.run(&["agent", "session", "0", "--leading-dash-is-text"]);
    t.ok("a name may begin with a dash", r.code == 0 && e.out(&["agent", "get", "0"]).contains("agent_session: --leading-dash-is-text"));

    t.section("from a pane");
    e.run(&["set", "-g", "command-from-panes", "spawned"]);
    let user = e.out(&["new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "as"]);
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent session 0 mine"));
    t.ok("a program may say it for its own pane", rc == 0 || o.contains("has no agent record"));
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent session {} theirs", user.trim_start_matches('%')));
    t.ok("and not for a pane that is not its own or one it spawned", rc != 0 && o.contains("spawned"));
    e.kill_server();
    t.finish();
}
