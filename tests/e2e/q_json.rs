//! P10.2, the machine-readable output: `--json` on agent list|get|wait|prompt (field names of the
//! event stream, ids as strings), errors as {"error":{"code","message"}} on the standard error
//! with exit 1 while the plain text of the same errors is unchanged, usage errors exit 2 in every
//! mode, the pane defaulting to $IMUX_PANE_ID, the new `agent get` fields, and `imux api schema`.
//! (The Rust version of tests/q_json.sh.)
use crate::support::*;
use regex_lite::Regex;
use std::time::Duration;

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

fn re(p: &str, text: &str) -> bool {
    Regex::new(p).unwrap().is_match(text)
}

#[test]
fn q_json() {
    let mut e = Env::new("qjson");
    let mut t = Check::new("q_json");
    let conf = e.write("imux.conf", "set -g status off\nset -g agent-control on\nset -g restore-commands \"claude\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    let run = |args: &[&str]| e.run_for(args, Duration::from_secs(20));

    t.section("no server");
    let r = run(&["agent", "list", "--json"]);
    t.ok("agent list --json with no server is server_unavailable on stderr, exit 1", r.code == 1 && r.out.is_empty() && r.err.contains("\"code\":\"server_unavailable\""));
    let r = run(&["agent", "list"]);
    t.ok("the plain error is unchanged text on stdout", r.code == 1 && re(r"(?m)^imux: no server running", &r.out));
    e.session("js", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    hold(&e, "0");

    t.section("the schema");
    let r = run(&["api", "schema"]);
    t.ok("api schema is one JSON line", r.code == 0 && r.out.lines().count() == 1 && r.out.starts_with("{\"api\":"));
    for code in [
        "not_found", "ambiguous_target", "invalid_argument", "permission_denied", "limit_reached", "timeout", "agent_blocked", "agent_not_running", "prompt_stalled",
        "invalid_regex", "not_ready", "server_unavailable", "protocol_mismatch",
    ] {
        t.ok(&format!("the schema lists the code {code}"), r.out.contains(&format!("\"code\":\"{code}\"")));
    }
    t.ok("api version is unchanged", re(r"(?m)^api=[0-9]*$", &run(&["api", "version"]).out));

    t.section("a record");
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} pane report-agent --source t --agent fake --state idle --message 'say \"hi\"' -- claude --continue"));
    t.ok("the pane reported", rc == 0);
    let r = run(&["agent", "get", "0", "--json"]);
    t.ok("agent get --json is one object", r.code == 0 && r.out.lines().count() == 1 && r.out.starts_with('{'));
    for field in ["session", "session_name", "window", "window_name", "pane", "state", "source", "agent", "message", "state_since", "seen", "seqs", "resume", "pid", "cwd"] {
        t.ok(&format!("agent get --json has {field}"), r.out.contains(&format!("\"{field}\":")));
    }
    t.ok(
        "ids are strings of the event forms",
        re(r#""session":"\$[0-9]+""#, &r.out) && re(r#""window":"@[0-9]+""#, &r.out) && r.out.contains("\"pane\":\"%0\""),
    );
    t.ok("the state and the agent", r.out.contains("\"state\":\"idle\"") && r.out.contains("\"agent\":\"fake\""));
    t.ok("a quote in the message is escaped", r.out.contains("\"message\":\"say \\\"hi\\\"\""));
    t.ok("the resume command is an array of words", r.out.contains("\"resume\":[\"claude\",\"--continue\"]"));
    t.ok("the source has its sequence", re(r#""seqs":\{"t":[0-9]*\}"#, &r.out));
    t.ok("nobody has looked yet", r.out.contains("\"seen\":false"));
    let r = run(&["agent", "get", "0"]);
    t.ok("the plain text keeps its lines", r.code == 0 && has_line(&r.out, "pane: 0") && has_line(&r.out, "state: idle") && r.out.lines().any(|l| l.starts_with("message: say")));
    t.ok("and has the new ones", has_line(&r.out, "seen: no") && re(r"(?m)^seq t: [0-9]*$", &r.out));
    let r = run(&["agent", "list", "--json"]);
    t.ok("agent list --json is an array of those objects", r.code == 0 && r.out.starts_with("[{") && r.out.contains("\"pane\":\"%0\"") && r.out.trim_end().ends_with("}]"));
    let r = run(&["agent", "list"]);
    t.ok("agent list plain is unchanged", r.code == 0 && r.out.lines().any(|l| l.starts_with("0\tjs:")));

    t.section("errors");
    let r = run(&["agent", "get", "99", "--json"]);
    t.ok(
        "a missing pane is not_found on stderr, exit 1, nothing on stdout",
        r.code == 1 && r.out.is_empty() && r.err.contains("\"code\":\"not_found\"") && r.err.contains("\"message\":\"no pane 99\""),
    );
    let r = run(&["agent", "get", "99"]);
    t.ok("the plain text has no code", r.code == 1 && has_line(&r.out, "imux: no pane 99"));
    e.run(&["split-window", "-d"]);
    let r = run(&["agent", "get", "1", "--json"]);
    t.ok("a pane with no record is agent_not_running and says why", r.code == 1 && r.err.contains("\"code\":\"agent_not_running\"") && r.err.contains("no program has reported"));
    let r = run(&["agent", "get", "1"]);
    t.ok("plain: the same words, no code", r.code == 1 && re(r"(?m)^imux: pane 1 has no agent record", &r.out) && !r.out.contains("agent_not_running"));
    let r = run(&["agent", "wait", "0", "--until", "working", "--timeout", "1", "--json"]);
    t.ok("a wait that runs out is timeout", r.code == 1 && r.err.contains("\"code\":\"timeout\""));
    let r = run(&["agent", "wait", "0", "--until", "working", "--timeout", "1"]);
    t.ok("plain: timed out, exit 1", r.code == 1 && r.out.contains("timed out waiting for the agent"));
    let r = run(&["agent", "wait", "99", "--json"]);
    t.ok("wait on a missing pane is not_found", r.code == 1 && r.err.contains("\"code\":\"not_found\""));

    t.section("wait and prompt");
    let r = run(&["agent", "wait", "0", "--until", "idle", "--json"]);
    t.ok("wait --json answers at once when the state is there", r.code == 0 && has_line(&r.out, "{\"pane\":\"%0\",\"state\":\"idle\"}"));
    let r = run(&["agent", "prompt", "--json", "0", "hello", "there"]);
    t.ok("prompt --json says sent", r.code == 0 && re(r#"(?m)^\{"pane":"%0","sent":true(,"seq":[0-9]+)?\}$"#, &r.out));
    let r = run(&["agent", "prompt", "0", "hello", "--json"]);
    t.ok("after the text, --json is text like --wait", r.code == 0 && has_line(&r.out, "sent"));
    let r = run(&["agent", "prompt", "--json", "1", "hello"]);
    t.ok("prompt into a pane with no agent is agent_not_running", r.code == 1 && r.err.contains("\"code\":\"agent_not_running\""));
    e.run(&["set", "-g", "agent-control", "off"]);
    let r = run(&["agent", "prompt", "--json", "0", "hello"]);
    t.ok("prompt with agent-control off is permission_denied naming the option", r.code == 1 && r.err.contains("\"code\":\"permission_denied\"") && r.err.contains("agent-control"));
    e.run(&["set", "-g", "agent-control", "on"]);
    in_pane(&e, "0", &format!("{BIN} pane report-agent --source t --agent fake --state blocked --message ask"));
    let r = run(&["agent", "prompt", "--json", "0", "hello"]);
    t.ok("prompt into a blocked agent is agent_blocked", r.code == 1 && r.err.contains("\"code\":\"agent_blocked\""));
    in_pane(&e, "0", &format!("{BIN} pane report-agent --source t --agent fake --state idle"));
    t.ok("a blocked state shows no kind and no progress", !run(&["agent", "get", "0", "--json"]).out.contains("\"kind\""));

    t.section("the pane defaults to the pane of the caller");
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent get"));
    t.ok("agent get with no pane in a pane means that pane", rc == 0 && has_line(&o, "pane: 0"));
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent get --json"));
    t.ok("and with --json too", rc == 0 && o.contains("\"pane\":\"%0\""));
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent get --current"));
    t.ok("--current is the same", rc == 0 && has_line(&o, "pane: 0"));
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} agent wait --until idle"));
    t.ok("agent wait too", rc == 0 && has_line(&o, "idle"));
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} agent get 0 --current"));
    t.ok("a pane and --current together are refused", rc == 2);

    t.section("usage errors exit 2 in every mode");
    let r = run(&["agent", "get"]);
    t.ok("no pane and no environment: usage, exit 2", r.code == 2 && r.out.contains("usage: imux agent get"));
    let r = run(&["agent", "get", "--json"]);
    t.ok("the same with --json (the usage is text on stdout, not a JSON error)", r.code == 2 && r.out.contains("usage: imux agent get") && !r.err.contains("{\"error\":{"));
    let r = run(&["agent", "get", "--current"]);
    t.ok("--current outside a pane is a usage error", r.code == 2 && r.out.contains("IMUX_PANE_ID"));
    let r = run(&["agent", "wait", "--json"]);
    t.ok("agent wait with no pane: usage, exit 2", r.code == 2 && r.out.contains("usage: imux agent wait"));
    let r = run(&["agent", "prompt", "--json", "0"]);
    t.ok("agent prompt with no text: usage, exit 2", r.code == 2 && r.out.contains("usage: imux agent prompt"));
    e.kill_server();
    t.finish();
}
