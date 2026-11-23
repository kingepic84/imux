//! The documented notification recipe (examples/notify-agent.sh, set as an agent-state-changed hook)
//! notifies for an agent that is blocked or done in a pane nobody looks at, and for no other.
//! (The Rust version of tests/q_notifyrecipe.sh.)
use crate::support::*;
use std::time::Duration;

fn report(source: &str, state: &str) -> String {
    format!("sleep 1; {BIN} pane report-agent --source {source} --agent fake --state {state} --message m; sleep 8")
}

fn notes(e: &Env) -> String {
    e.read("notes")
}

fn has_note(e: &Env, line: &str) -> bool {
    wait_until(Duration::from_secs(10), || notes(e).lines().any(|l| l == line))
}

#[test]
fn q_notifyrecipe() {
    let mut e = Env::new("qnr");
    let recipe = format!("{}/examples/notify-agent.sh", env!("CARGO_MANIFEST_DIR"));
    let fake = e.write("fake-notify", "#!/bin/sh\nprintf \"%s|%s\\n\" \"$1\" \"$2\" >>\"$IMUX_TEST_NOTES\"\n");
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    e.set("IMUX_NOTIFY_CMD", &p(&fake));
    e.set("IMUX_TEST_NOTES", &p(&e.path("notes")));
    let conf = e.write("imux.conf", &format!("set-hook -g agent-state-changed \"run-shell -b '{recipe}'\"\n"));
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_notifyrecipe");

    t.section("a session nobody attached");
    e.run(&["new-session", "-d", "-s", "unseen", "-n", "w", &report("t", "blocked")]);
    t.ok("a blocked agent in an unattached session notifies", has_note(&e, "imux: blocked|unseen:w"));
    e.run(&["new-window", "-d", "-t", "unseen:", "-n", "busy", &report("u", "working")]);
    e.run(&["new-window", "-d", "-t", "unseen:", "-n", "fin", &report("v", "done")]);
    t.ok("a done agent notifies", has_note(&e, "imux: done|unseen:fin"));
    t.ok("a working agent does not", !notes(&e).contains("busy"));
    e.kill_server();

    t.section("a session being looked at");
    let _ = std::fs::remove_file(e.path("notes"));
    let mut c = e.client(100, 30, &["new", "-s", "seen"]);
    assert!(wait_until(Duration::from_secs(20), || e.out(&["ls"]).lines().any(|l| l.starts_with("seen:"))), "the session seen did not start");
    e.run(&["send-keys", "-t", "seen:0", &format!("{BIN} pane report-agent --source t --agent fake --state blocked --message m"), "Enter"]);
    e.run(&["new-window", "-d", "-t", "seen:", "-n", "elsewhere", &report("u", "blocked")]);
    t.ok("the window nobody looks at notifies", has_note(&e, "imux: blocked|seen:elsewhere"));
    // the pane being looked at reported first, so a note for it would be in the file before this one
    t.ok("the pane being looked at does not", notes(&e).lines().count() == 1);
    e.kill_server();
    c.wait_exit(Duration::from_secs(10));
    t.finish();
}
