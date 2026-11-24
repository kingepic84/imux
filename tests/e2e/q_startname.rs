//! A name given by `imux agent start` stays with the program it started: when that program changes
//! its agent record on the way up (another program name, or the record dropped and made again, as
//! happens behind a dialog), the name goes to the next record. A name set by hand ends that.
//! (Found on a real run: the name was lost once a program had answered its trust dialog.)
use crate::support::*;
use std::process::Command;
use std::time::Duration;

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn q_startname() {
    let mut e = Env::new("qsn");
    let mut t = Check::new("q_startname");
    let fixdir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let conf = e.write(
        "imux.conf",
        "set -g status off\nset -g default-size 100x30\nset -g agent-control on\nset -g restore-commands \"fake_agent_rebirth.sh\"\nset -g agent-kind \"rebirth=fake_agent_rebirth.sh\"\n",
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
    let go = e.path("go");
    assert!(Command::new("mkfifo").arg(&go).status().expect("mkfifo runs").success(), "mkfifo failed");
    e.set("FAKE_GO", &p(&go));
    e.session("sn", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    let step = || std::fs::write(&go, "go\n").unwrap();

    t.section("a start that is blocked keeps the name");
    let r = e.run(&["agent", "start", "rebirth", "--kind", "rebirth", "--pane", "%0", "--json"]);
    t.ok("blocked at startup is not_ready", r.code == 1 && r.err.contains("not_ready"));
    let r = e.run(&["agent", "get", "rebirth"]);
    t.ok("the agent has the name and is blocked", r.code == 0 && has_line(&r.out, "state: blocked"));

    t.section("another program name takes over the record");
    step();
    t.ok(
        "the name is still there",
        wait_until(Duration::from_secs(15), || {
            let r = e.run(&["agent", "get", "rebirth"]);
            r.code == 0 && has_line(&r.out, "state: idle") && has_line(&r.out, "name: rebirth")
        }),
    );

    t.section("the record dropped and made again");
    step();
    t.ok(
        "the name is still there",
        wait_until(Duration::from_secs(15), || {
            let r = e.run(&["agent", "get", "rebirth"]);
            r.code == 0 && has_line(&r.out, "state: working") && has_line(&r.out, "name: rebirth")
        }),
    );

    t.section("a name set by hand ends it");
    let r = e.run(&["agent", "name", "0", "mine"]);
    t.ok("renaming works", r.code == 0);
    step();
    t.ok(
        "the program replaces its record again and the pane keeps no name from the start",
        wait_until(Duration::from_secs(15), || e.run(&["agent", "get", "0"]).out.contains("state: idle")),
    );
    let r = e.run(&["agent", "get", "rebirth", "--json"]);
    t.ok("the start name is not given back", r.code == 1 && r.err.contains("not_found"));
    let r = e.run(&["agent", "get", "mine", "--json"]);
    t.ok("and the hand-made name was lost with the record, as it always was", r.code == 1 && r.err.contains("not_found"));
    e.kill_server();
    t.finish();
}
