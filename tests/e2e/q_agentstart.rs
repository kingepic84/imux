//! P10.7, `imux agent start NAME [--kind K] [--pane P] [--timeout MS] [-- args...]`: the command
//! comes from worktree-command or the agent-kind table (never from the caller), arguments are
//! quoted for the pane's shell, the pane has to be at its prompt with no agent, ready is the first
//! idle report, a blocked start is not_ready and keeps its name, a silent program times out and
//! loses the name, and the rules for a program in a pane. (The Rust version of tests/q_agentstart.sh.)
use crate::support::*;
use regex_lite::Regex;
use std::time::Duration;

fn newpane(e: &Env) -> String {
    let id = e.out(&["new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "as"]);
    assert!(e.ready(&id, Duration::from_millis(400)), "pane {id} never became ready");
    id
}

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn q_agentstart() {
    let mut e = Env::new("qas");
    let mut t = Check::new("q_agentstart");
    let fixdir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let conf = e.write(
        "imux.conf",
        "set -g status off\nset -g default-size 100x30\nset -g agent-control on\nset -g restore-commands \"claude fake_agent.sh\"\nset -g agent-kind \"fake=fake_agent.sh bad=rm\"\nset -g worktree-command fake_agent.sh\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".profile"), format!("PATH=\"{fixdir}:$PATH\"\nexport PATH\n")).unwrap();
    e.set("HOME", &p(&home));
    e.set("PATH", &format!("{fixdir}:{}", std::env::var("PATH").unwrap_or_default()));
    e.set("FAKE_IMUX", BIN);
    let args_file = e.path("args");
    e.set("FAKE_ARGS_FILE", &p(&args_file));
    let d = p(&e.dir);
    e.session("as", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));

    t.section("the default kind (worktree-command)");
    let r = e.run(&["agent", "start", "alpha", "--pane", "%0"]);
    t.ok("start answers ready and the pane", r.code == 0 && has_line(&r.out, "ready %0"));
    let r = e.run(&["agent", "get", "alpha"]);
    t.ok("the agent has the name and is idle", r.code == 0 && has_line(&r.out, "name: alpha") && has_line(&r.out, "state: idle"));
    let args = std::fs::read_to_string(&args_file).unwrap_or_default();
    t.ok("no arguments reached it", args.lines().count() <= 1);

    t.section("a kind from agent-kind, --json");
    let p1 = newpane(&e);
    let r = e.run(&["agent", "start", "beta", "--kind", "fake", "--pane", &p1, "--json"]);
    let re = Regex::new(&format!(r#"^\{{"pane":"{p1}","name":"beta","state":"idle"(,"seq":[0-9]+)?\}}$"#)).unwrap();
    t.ok("the JSON has pane, name and state", r.code == 0 && r.out.lines().any(|l| re.is_match(l)));
    let r = e.run(&["agent", "get", "beta", "--json"]);
    t.ok("agent get finds it by name", r.code == 0 && r.out.contains(&format!("\"pane\":\"{p1}\"")));

    t.section("arguments are quoted for the pane shell");
    let p2 = newpane(&e);
    let (pw1, pw2) = (e.path("PWNED"), e.path("PWNED2"));
    let a3 = format!("$(touch {d}/PWNED)");
    let a4 = format!(";touch {d}/PWNED2");
    let r = e.run(&["agent", "start", "hostile", "--kind", "fake", "--pane", &p2, "--", "a b", "it's", &a3, &a4, "--mode", "x"]);
    t.ok("start succeeds", r.code == 0);
    let args = std::fs::read_to_string(&args_file).unwrap_or_default();
    t.ok(
        "each argument arrived as one word",
        ["a b", "it's", a3.as_str(), a4.as_str(), "x"].iter().all(|w| has_line(&args, w)),
    );
    t.ok("nothing was executed from them", !pw1.exists() && !pw2.exists());
    let r = e.run(&["agent", "start", "ctl", "--kind", "fake", "--pane", &p2, "--json", "--", "a\tb"]);
    t.ok("a control character in an argument is refused (and this pane has an agent anyway)", r.code == 1);
    let p2b = newpane(&e);
    let r = e.run(&["agent", "start", "ctl", "--kind", "fake", "--pane", &p2b, "--json", "--", "a\tb"]);
    t.ok("a control character is invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\""));
    let many: Vec<String> = (1..=70).map(|n| n.to_string()).collect();
    let mut argv = vec!["agent", "start", "many", "--kind", "fake", "--pane", p2b.as_str(), "--json", "--"];
    argv.extend(many.iter().map(|s| s.as_str()));
    let r = e.run(&argv);
    t.ok("more than 64 arguments are invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\""));

    t.section("the pane must be at its prompt, with no agent");
    let p3 = newpane(&e);
    e.run(&["send-keys", "-t", &p3, "-l", "sleep 600"]);
    e.run(&["send-keys", "-t", &p3, "Enter"]);
    e.shows(&p3, "sleep 600");
    std::thread::sleep(Duration::from_millis(700));
    let r = e.run(&["agent", "start", "busy", "--pane", &p3, "--json"]);
    t.ok("a busy pane is not_ready", r.code == 1 && r.err.contains("\"code\":\"not_ready\"") && r.err.contains("not at its shell prompt"));
    let r = e.run(&["agent", "get", "busy", "--json"]);
    t.ok("and the name was not taken", r.code == 1 && r.err.contains("not_found"));
    let r = e.run(&["agent", "start", "again", "--pane", "%0", "--json"]);
    t.ok("a pane that has an agent is not_ready", r.code == 1 && r.err.contains("\"code\":\"not_ready\"") && r.err.contains("already has an agent"));
    let r = e.run(&["agent", "start", "nopane", "--pane", "%999", "--json"]);
    t.ok("a missing pane is not_found", r.code == 1 && r.err.contains("\"code\":\"not_found\""));

    t.section("names and kinds");
    let p4 = newpane(&e);
    let r = e.run(&["agent", "start", "alpha", "--pane", &p4, "--json"]);
    t.ok("a name in use is invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\"") && r.err.contains("already taken"));
    for bad in ["Bad", "1a", "a.b", "a:b", "-x"] {
        let r = e.run(&["agent", "start", bad, "--pane", &p4, "--json"]);
        t.ok(&format!("the name '{bad}' is refused"), r.code != 0);
    }
    let r = e.run(&["agent", "start", "nokind", "--kind", "nosuch", "--pane", &p4, "--json"]);
    t.ok("a kind that is not in agent-kind is invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\"") && r.err.contains("no kind nosuch"));
    let r = e.run(&["agent", "start", "nokind", "--kind", "bad", "--pane", &p4, "--json"]);
    t.ok(
        "a kind whose program is not in restore-commands is permission_denied",
        r.code == 1 && r.err.contains("\"code\":\"permission_denied\"") && r.err.contains("restore-commands"),
    );
    t.ok("and nothing was typed", !e.capture(&p4).contains("rm"));
    let r = e.run(&["agent", "start", "t1", "--pane", &p4, "--timeout", "1000", "--json"]);
    t.ok("a timeout under 3000 is invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\""));
    let r = e.run(&["agent", "start", "t1", "--pane", &p4, "--timeout", "999999", "--json"]);
    t.ok("a timeout over 300000 is invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\""));
    let r = e.run(&["agent", "start", "t1", "--pane", &p4, "--timeout", "x"]);
    t.ok("a timeout that is not a number is a usage error", r.code == 2);
    e.run(&["set", "-g", "worktree-command", ""]);
    let r = e.run(&["agent", "start", "empty", "--pane", &p4, "--json"]);
    t.ok(
        "no kind and an empty worktree-command is invalid_argument",
        r.code == 1 && r.err.contains("\"code\":\"invalid_argument\"") && r.err.contains("worktree-command"),
    );
    e.run(&["set", "-g", "worktree-command", "fake_agent.sh"]);
    e.run(&["set", "-g", "agent-control", "off"]);
    let r = e.run(&["agent", "start", "nocontrol", "--pane", &p4, "--json"]);
    t.ok("with agent-control off it is permission_denied", r.code == 1 && r.err.contains("\"code\":\"permission_denied\"") && r.err.contains("agent-control"));
    e.run(&["set", "-g", "agent-control", "on"]);
    t.ok("nothing was typed into the pane through all of these", !e.capture(&p4).contains("fake_agent"));

    t.section("a blocked start is not_ready and keeps the name");
    let r = e.run(&["agent", "start", "gamma", "--kind", "fake", "--pane", &p4, "--json", "--", "--mode", "blocked"]);
    t.ok("blocked at startup is not_ready", r.code == 1 && r.err.contains("\"code\":\"not_ready\"") && r.err.contains("blocked at startup"));
    let r = e.run(&["agent", "get", "gamma"]);
    t.ok("the agent keeps the name and is blocked", r.code == 0 && has_line(&r.out, "state: blocked"));

    t.section("a silent program times out and loses the name");
    let p5 = newpane(&e);
    let r = e.run(&["agent", "start", "quiet", "--kind", "fake", "--pane", &p5, "--timeout", "3000", "--json", "--", "--mode", "late"]);
    t.ok("no report in time is timeout and says why", r.code == 1 && r.err.contains("\"code\":\"timeout\"") && r.err.contains("never the screen"));
    let r = e.run(&["agent", "get", "quiet", "--json"]);
    t.ok("and the name is not kept", r.code == 1 && r.err.contains("not_found"));
    t.ok(
        "the program reports after the timeout",
        wait_until(Duration::from_secs(15), || e.run(&["agent", "list", "--json"]).out.contains(&format!("\"pane\":\"{p5}\""))),
    );
    let r = e.run(&["agent", "get", "quiet", "--json"]);
    t.ok("and the name does not come back", r.code == 1 && r.err.contains("not_found"));
    let r = e.run(&["agent", "start", "quiet", "--pane", &p5, "--json"]);
    t.ok("the pane now has an agent", r.code == 1 && r.err.contains("already has an agent"));

    t.section("the client leaving ends the wait, not the program");
    let p6 = newpane(&e);
    e.run_for(&["agent", "start", "leaver", "--kind", "fake", "--pane", &p6, "--timeout", "60000", "--", "--mode", "late"], Duration::from_secs(2));
    std::thread::sleep(Duration::from_secs(1));
    t.ok("the server is still there", e.run(&["list-panes", "-a"]).code == 0);
    let r = e.run(&["agent", "start", "other", "--pane", &p6, "--json"]);
    t.ok("the pane is busy running the program", r.code == 1 && r.err.contains("not_ready"));
    t.ok(
        "the first report gives the pane the name it was started with",
        wait_until(Duration::from_secs(15), || {
            let r = e.run(&["agent", "get", "leaver"]);
            r.code == 0 && has_line(&r.out, "name: leaver")
        }),
    );

    t.section("from a pane");
    let p7 = newpane(&e);
    let n7 = p7.trim_start_matches('%').to_string();
    hold(&e, &n7);
    let p8 = newpane(&e);
    let (rc, o) = in_pane(&e, &n7, &format!("{BIN} agent start x1 --kind fake --pane {p8} --json"));
    t.ok("a program in a user-made pane may not start an agent in a pane it did not spawn", rc != 0 && o.contains("permission_denied"));
    let (rc, o) = in_pane(&e, &n7, &format!("{BIN} agent start x1 --kind fake --json"));
    t.ok("from a pane the pane must be named", rc != 0 && o.contains("invalid_argument") && o.contains("which pane"));
    let (rc, o) = in_pane(&e, &n7, &format!("{BIN} agent start x1 --kind fake --pane {p7} --json"));
    t.ok("never its own pane", rc != 0 && o.contains("permission_denied"));
    let (_, o) = in_pane(&e, &n7, &format!("{BIN} new-window -d -P -F '#{{pane_id}}' -t as"));
    let spawned = o.trim().to_string();
    t.ok("it can spawn a pane", Regex::new(r"^%[0-9]+$").unwrap().is_match(&spawned));
    e.ready(&spawned, Duration::from_millis(400));
    let (rc, o) = in_pane(&e, &n7, &format!("{BIN} agent start x2 --kind fake --pane {spawned} --json"));
    t.ok("and start an agent in the pane it spawned", rc == 0 && o.contains("\"name\":\"x2\""));
    let r = e.run(&["agent", "get", "x2", "--json"]);
    t.ok("that agent has the name", r.code == 0 && r.out.contains(&format!("\"pane\":\"{spawned}\"")));
    e.run(&["set", "-g", "command-from-panes", "off"]);
    let p9 = newpane(&e);
    let (rc, _) = in_pane(&e, &n7, &format!("{BIN} agent start x3 --kind fake --pane {p9} --json"));
    t.ok("with command-from-panes off it only looks", rc != 0);
    e.run(&["set", "-g", "command-from-panes", "spawned"]);
    e.kill_server();
    t.finish();
}
