//! Agent status: the pane answers the status query, glyphs follow the reports, a finished pane stops
//! counting once looked at, the agent panel, reports over the socket from inside a pane (stale ones,
//! resume commands, other sources, spoofing), `agent wait`, and a reported resume command coming back
//! after a restart. (The Rust version of tests/m16_agents.sh.)
use crate::support::*;
use std::time::{Duration, Instant};

const LONG: Duration = Duration::from_secs(20);
const GAP: Duration = Duration::from_millis(700);

fn script(e: &Env, name: &str, text: &str) -> String {
    let f = e.write(name, text);
    std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    p(&f)
}

fn nap(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn client(e: &Env, args: &[&str]) -> Client {
    let c = e.client(100, 30, args);
    assert!(e.has_client(), "the client did not attach");
    c
}

fn done(e: &Env, mut c: Client) -> Client {
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    nap(600);
    c
}

#[test]
fn m16_agents() {
    let mut e = Env::new("m16");
    let d = p(&e.dir);
    let conf = e.write("conf", "set autosave-interval 0\nset agent-glyph-working \"WWW\"\nset agent-glyph-blocked \"BBB\"\nset agent-glyph-done \"DDD\"\nset status-right \" G:#{agent_glyph} N:#{agent_count} \"\nset window-status-format \" #I#{window_agent_glyph} \"\nset window-status-current-format \" #I*#{window_agent_glyph} \"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("m16_agents");
    let probe = script(&e, "probe.sh", "printf '\\033]7501;?\\007'\nIFS= read -rs -t 4 -d '\\' reply </dev/tty\nprintf '%s' \"$reply\" | cat -v >\"$1\"\n");
    let agent = script(&e, "agent.sh", "for step in \"$@\"; do\n  case $step in\n    wait:*) sleep \"${step#wait:}\" ;;\n    *) printf '\\033]7501;%s\\033\\\\' \"$step\" ;;\n  esac\ndone\nsleep 30\n");

    t.section("the pane answers the status query");
    let mut c = client(&e, &["new", "-s", "w"]);
    assert!(e.ready("w:", Duration::from_millis(600)));
    c.send_str(&format!("{probe} {d}/reply.txt\r"));
    assert!(wait_until(LONG, || e.read("reply.txt").contains("kinds=")));
    let reply = e.read("reply.txt");
    t.ok("reply names the states", reply.contains("states=idle,working,done,blocked,error"));
    t.ok("reply names the kinds", reply.contains("kinds=permission,question,auth"));
    done(&e, c);

    t.section("status glyphs follow the reports");
    let mut c = client(&e, &["new", "-s", "w"]);
    assert!(e.ready("w:", Duration::from_millis(600)));
    c.send_str(&format!("{agent} state=working:app=fake\r"));
    nap(1500);
    c.type_chunks(&["\x02c"], GAP);
    c.send_str(&format!("{agent} state=blocked:app=fake:kind=permission wait:2 state=working:app=fake\r"));
    nap(1500);
    c.type_chunks(&["\x02c"], GAP);
    c.send_str(&format!("{agent} wait:2 state=done:app=fake\r"));
    nap(800);
    c.type_chunks(&["\x02", "0"], GAP);
    t.ok("a working pane shows the working glyph", c.wait_seen("WWW", LONG));
    t.ok("a blocked pane shows the blocked glyph", c.wait_seen("BBB", LONG));
    t.ok("a finished background pane shows the done glyph", c.wait_seen("DDD", LONG));
    t.ok("several panes in one state are counted", c.wait_seen("N:2", LONG));
    t.ok("the window list carries a glyph per window", c.seen(r"\[30;([1-7][0-9]|[0-9])H[^W]{0,80}WWW"));
    done(&e, c);

    t.section("a finished pane stops counting once it has been looked at");
    let mut c = client(&e, &["new", "-s", "w2"]);
    assert!(e.ready("w2:", Duration::from_millis(600)));
    c.type_chunks(&["\x02c"], GAP);
    c.send_str(&format!("{agent} wait:2 state=done:app=fake\r"));
    nap(800);
    c.type_chunks(&["\x02", "0"], GAP);
    t.ok("the done glyph shows while the pane is out of sight", c.wait_seen("DDD", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    let looked = e.with_client("w2", |c| {
        c.send(b"\x02");
        nap(300);
        c.send(b"1");
        nap(1500);
        c.clear();
        nap(1500);
    });
    t.ok("after looking at it the glyph is gone from what follows", !looked.contains("DDD"));
    e.kill_server();
    nap(600);

    t.section("the agent panel lists panes and jumps to the chosen one");
    let reader = script(&e, "reader.sh", "printf '\\033]7501;state=blocked:app=fake:kind=permission:msg=YXBwcm92ZS10aGluZw==\\033\\\\'\nIFS= read -r line </dev/tty\nprintf '%s' \"$line\" >\"$1\"\nsleep 30\n");
    let mut c = client(&e, &["new", "-s", "w3"]);
    assert!(e.ready("w3:", Duration::from_millis(600)));
    c.send(b"\x02A");
    t.ok("with no agents a message says so", c.wait_seen("no agents reporting", LONG));
    done(&e, c);
    let mut c = client(&e, &["new", "-s", "w4"]);
    assert!(e.ready("w4:", Duration::from_millis(600)));
    c.type_chunks(&["\x02c"], GAP);
    c.send_str(&format!("{reader} {d}/got.txt\r"));
    nap(1200);
    c.type_chunks(&["\x02c"], GAP);
    c.send_str(&format!("{agent} state=working:app=fake\r"));
    nap(1200);
    c.type_chunks(&["\x02c", "\x02", "A"], GAP);
    t.ok("the panel has a title", c.wait_seen("agents", LONG));
    t.ok("it shows the message of a blocked agent", c.wait_seen("approve-thing", LONG));
    t.ok("it shows a working agent too", c.wait_seen("fake: working", LONG));
    c.type_chunks(&["\r"], GAP);
    c.send_str("hello-from-the-panel\r");
    t.ok("Enter jumps to the first row, the blocked agent, and keys go there", wait_until(LONG, || std::fs::read_to_string(e.path("got.txt")).unwrap_or_default() == "hello-from-the-panel"));
    done(&e, c);

    t.section("reports over the socket, from inside a pane");
    let conf2 = e.write("conf2", "set autosave-interval 0\nset restore-commands \"claude vim\"\n");
    e.set("IMUX_CONFIG", &p(&conf2));
    let reports = script(
        &e,
        "reports.sh",
        &format!(
            "echo \"env-$IMUX_ENV-$IMUX_PANE_ID\" >\"{d}/env.txt\"\nR=\"{BIN} pane report-agent --source hook --agent fake\"\n$R --state working --seq 5 --message hi >\"{d}/r1.txt\" 2>&1\n$R --state idle --seq 3 >\"{d}/r2.txt\" 2>&1\n$R --state blocked --seq 6 -- rm -rf x >\"{d}/r3.txt\" 2>&1\n$R --state blocked --seq 7 -- claude --resume abc >\"{d}/r4.txt\" 2>&1\n{BIN} pane report-agent --source other --agent fake --state done --seq 1 >\"{d}/r5.txt\" 2>&1\n{BIN} pane release-agent --source nobody >\"{d}/r6.txt\" 2>&1\nsleep 40\n"
        ),
    );
    let c = client(&e, &["new", "-s", "w5"]);
    assert!(e.ready("w5:", Duration::from_millis(600)));
    let mut c = c;
    c.send_str(&format!("{reports}\r"));
    assert!(wait_until(LONG, || !e.read("r6.txt").is_empty()), "the reports did not finish");
    t.matches("panes get the environment", "env-1-0", &e.read("env.txt"));
    t.matches("a report is accepted", "ok", &e.read("r1.txt"));
    t.matches("a stale report is ignored", "stale", &e.read("r2.txt"));
    t.matches("a resume command that is not allowed is refused", "resume command refused", &e.read("r3.txt"));
    t.matches("an allowed one is taken", "ok", &e.read("r4.txt"));
    t.matches("another source may write too", "ok", &e.read("r5.txt"));
    t.matches("the list shows the pane and its state", "done", &e.all(&["agent", "list"]));
    let get = e.all(&["agent", "get", "0"]);
    t.matches("get shows the writer", "source: other", &get);
    t.matches("get shows the resume command", "resume: claude --resume abc", &get);
    t.matches("a source that did not write the record cannot release it", "nothing to release", &e.read("r6.txt"));
    t.matches("a report from outside the pane is refused", "is not where this command runs", &e.all(&["pane", "report-agent", "0", "--source", "x", "--agent", "a", "--state", "idle"]));
    t.matches("waiting past the timeout fails", "timed out", &e.all(&["agent", "wait", "0", "--until", "working", "--timeout", "1"]));
    t.matches("waiting for a state the agent is already in answers at once", "done", &e.all(&["agent", "wait", "0", "--until", "done"]));
    t.matches("an unknown pane is an error", "no pane 99", &e.all(&["agent", "get", "99"]));
    done(&e, c);

    t.section("agent wait is answered when the state arrives");
    let mut c = client(&e, &["new", "-s", "w6"]);
    assert!(e.ready("w6:", Duration::from_millis(600)));
    c.send_str(&format!("{BIN} pane report-agent --source hook --agent fake --state working --seq 1 >/dev/null 2>&1\r"));
    nap(800);
    c.send_str(&format!("(sleep 3; {BIN} pane report-agent --source hook --agent fake --state done --seq 2) >/dev/null 2>&1 &\r"));
    assert!(wait_until(LONG, || !e.all(&["agent", "get", "0"]).contains("no agent record")), "the first report was not recorded");
    let start = Instant::now();
    let r = e.all(&["agent", "wait", "0", "--until", "done", "--timeout", "20"]);
    t.matches("the wait returns the state", "done", &r);
    t.ok("and returns when it arrives, not at the timeout", start.elapsed() < Duration::from_secs(15));
    done(&e, c);

    t.section("a reported resume command comes back after a restart");
    let conf3 = e.write("conf3", "set autosave-interval 0\nset restore-commands \"sleep\"\n");
    e.set("IMUX_CONFIG", &p(&conf3));
    let resumer = script(&e, "resumer.sh", &format!("{BIN} pane report-agent --source hook --agent fake --state idle -- sleep 31339 >\"{d}/rr.txt\" 2>&1\nsleep 31341\n"));
    let mut c = client(&e, &["new", "-s", "w7"]);
    assert!(e.ready("w7:", Duration::from_millis(600)));
    c.send_str(&format!("{resumer}\r"));
    assert!(wait_until(LONG, || !e.read("rr.txt").is_empty() || e.read("rr.txt").is_empty() && pgrep("sleep 31341")));
    nap(1000);
    e.run(&["save", "resumeA"]);
    let snap = std::fs::read_to_string(e.path("state/snapshots/resumeA.imux")).unwrap_or_default();
    t.ok("the snapshot holds the reported command", snap.contains("+resume"));
    t.ok("and its arguments", snap.contains("31339"));
    done(&e, c);
    pkill("sleep 31341");
    let mut other = client(&e, &["new", "-s", "other"]);
    assert!(wait_until(LONG, || e.out(&["ls"]).lines().any(|l| l.starts_with("other:"))));
    assert!(e.ready("other:", Duration::from_millis(600)));
    e.run(&["restore", "resumeA"]);
    t.ok("the reported command runs again in the restored pane", wait_until(LONG, || pgrep("sleep 31339")));
    other.send(b"\x02d");
    other.wait_exit(LONG);
    e.kill_server();
    pkill("sleep 31339");
    pkill("sleep 31341");
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
