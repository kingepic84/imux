//! `examples/claude-hook.sh`, the script `imux setup` puts in Claude Code's directory: `session` tells imux
//! the name of the Claude Code session in the pane (and never changes the agent's own name), `resume` tells imux how to bring the
//! conversation back (claude --resume SESSION_ID), it takes the id from `session_id` or else from the
//! name of the transcript, and it prints nothing and succeeds outside imux and on odd input.
use crate::support::*;
use std::process::Command;
use std::time::Duration;

#[test]
fn q_claudehook() {
    let mut e = Env::new("qch");
    e.set("IMUX_CONFIG", "/dev/null");
    let mut t = Check::new("q_claudehook");
    let script = format!("{}/examples/claude-hook.sh", env!("CARGO_MANIFEST_DIR"));
    // the script calls `imux`: a directory with that name first on PATH
    let bin = e.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("imux")).unwrap();
    e.set("PATH", &format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
    let transcript = e.write("sessions/0b1f6c3e-aaaa-bbbb-cccc-123456789abc.jsonl", "{\"type\":\"user\"}\n{\"type\":\"custom-title\",\"customTitle\":\"Homelab and imux\",\"sessionId\":\"x\"}\n");
    let tr = p(&transcript);

    t.section("outside imux and on odd input it does nothing and succeeds");
    for what in ["session", "resume", "nonsense", ""] {
        let out = Command::new("sh")
            .arg(&script)
            .arg(what)
            .env_remove("IMUX_ENV")
            .env_remove("IMUX_PANE_ID")
            .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()))
            .output()
            .expect("sh runs");
        t.ok(&format!("'{what}' outside imux: status 0 and no output"), out.status.success() && out.stdout.is_empty() && out.stderr.is_empty());
    }

    t.section("in a pane that reports as an agent");
    // panes run login shells, which start from the profile: PATH has to be set there
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".profile"), format!("PATH=\"{}:$PATH\"\nexport PATH\n", bin.display())).unwrap();
    e.set("HOME", &p(&home));
    e.session("ch", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    hold(&e, "0");
    let report = "printf '\\033]7501;state=idle:app=claude-code\\033\\\\' >/dev/tty; n=0; until imux agent get \"$IMUX_PANE_ID\" >/dev/null 2>&1 || [ $n -ge 300 ]; do n=$((n+1)); done";
    let json = format!("{{\"session_id\":\"sess-111\",\"transcript_path\":\"{tr}\",\"hook_event_name\":\"SessionStart\"}}");
    // an agent name that something else gave stays whatever the hook says
    let (rc, o) = in_pane(&e, "0", &format!("{report}; imux agent name \"$IMUX_PANE_ID\" boss; printf '%s' '{json}' | sh {script} session; echo session-rc=$?; imux agent get \"$IMUX_PANE_ID\""));
    t.ok("session succeeds", rc == 0 && o.contains("session-rc=0"));
    t.ok("the agent has the session's name as text", o.contains("agent_session: Homelab and imux"));
    t.ok("and keeps the name it was given", o.contains("name: boss"));
    t.ok("there is no name before it yet", !o.contains("previous_agent_session"));
    // the same name on every prompt changes nothing: the name before it is not pushed
    let (_, o) = in_pane(&e, "0", &format!("printf '%s' '{json}' | sh {script} session; printf '%s' '{json}' | sh {script} session; imux agent get \"$IMUX_PANE_ID\""));
    t.ok("the same name again leaves the current and the previous alone", o.contains("agent_session: Homelab and imux") && !o.contains("previous_agent_session"));
    // a rename shows, and the name before it is kept apart
    let second = e.write("sessions/second.jsonl", "{\"type\":\"custom-title\",\"customTitle\":\"Second name\",\"sessionId\":\"x\"}\n");
    let json2 = format!("{{\"session_id\":\"sess-111\",\"transcript_path\":\"{}\"}}", p(&second));
    let (_, o) = in_pane(&e, "0", &format!("printf '%s' '{json2}' | sh {script} session; imux agent get \"$IMUX_PANE_ID\""));
    t.ok("a rename is the current name", o.contains("agent_session: Second name"));
    t.ok("and the name before it is kept", o.contains("previous_agent_session: Homelab and imux"));
    let (_, o) = in_pane(&e, "0", &format!("printf '%s' '{json2}' | sh {script} session; imux agent get --json \"$IMUX_PANE_ID\""));
    t.ok("saying the new name again keeps both as they are", o.contains("\"agent_session\":\"Second name\"") && o.contains("\"previous_agent_session\":\"Homelab and imux\""));
    let (rc, o) = in_pane(&e, "0", &format!("{report}; printf '%s' '{json}' | sh {script} resume; echo resume-rc=$?"));
    t.ok("resume succeeds", rc == 0 && o.contains("resume-rc=0") && o.trim() == "resume-rc=0");
    let r = e.run(&["save", "hooksnap"]);
    t.ok("saving works", r.code == 0);
    let snap = std::fs::read_to_string(e.path("state/snapshots/hooksnap.imux")).unwrap_or_default();
    t.ok("the snapshot holds the way back, with the id from session_id", snap.contains("--resume") && snap.contains("sess-111"));

    t.section("the id from the name of the transcript");
    let only_path = format!("{{\"transcript_path\":\"{tr}\"}}");
    let (rc, o) = in_pane(&e, "0", &format!("{report}; printf '%s' '{only_path}' | sh {script} resume; echo resume-rc=$?"));
    t.ok("resume succeeds without session_id", rc == 0 && o.contains("resume-rc=0"));
    e.run(&["save", "hooksnap2"]);
    let snap = std::fs::read_to_string(e.path("state/snapshots/hooksnap2.imux")).unwrap_or_default();
    t.ok("and takes the id from the transcript's name", snap.contains("0b1f6c3e-aaaa-bbbb-cccc-123456789abc"));
    e.kill_server();
    t.finish();
}
