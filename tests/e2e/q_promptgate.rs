//! P10.8, `agent prompt --wait` hardening: a settled state answers only after the agent was seen
//! working or blocked since the text (or was working when it came), an agent that shows neither in
//! 5 seconds is prompt_stalled, blocked and error answer at once, another program taking the pane
//! ends the wait, and one timeout covers typing and waiting and says delivery is unknown.
//! (The Rust version of tests/q_promptgate.sh.)
use crate::support::*;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

const LONG: Duration = Duration::from_secs(20);

fn report(e: &Env, pane: &str, state: &str, agent: &str) {
    in_pane(e, pane, &format!("{BIN} pane report-agent --source t --agent {agent} --state {state}"));
}

struct Prompt {
    child: Child,
    started: Instant,
}

fn prompt_bg(e: &Env, pane: &str, timeout: u32) -> Prompt {
    let child = e
        .command(BIN)
        .args(["agent", "prompt", "--wait", "--json", "--timeout", &timeout.to_string(), pane, "hello"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The text has been typed once the pane echoes it; reports sent before that would not count.
    wait_until(Duration::from_secs(15), || e.capture(&format!("%{pane}")).contains("hello"));
    Prompt { child, started: Instant::now() }
}

impl Prompt {
    fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Does it end within `window`? (Something that does not happen can only be watched for.)
    fn finishes_within(&mut self, window: Duration) -> bool {
        wait_until(window, || self.finished())
    }

    /// Wait up to `secs`; the exit status, standard output and standard error.
    fn result(mut self, secs: u64) -> (i32, String, String) {
        let end = self.started + Duration::from_secs(secs);
        while Instant::now() < end && !self.finished() {
            std::thread::sleep(Duration::from_millis(100));
        }
        if !self.finished() {
            let _ = self.child.kill();
        }
        let out = self.child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[test]
fn q_promptgate() {
    let mut e = Env::new("qpg");
    let conf = e.write("imux.conf", "set -g status off\nset -g default-size 100x30\nset -g agent-control on\nset -g agent-stall-time 60\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_promptgate");

    e.session("pg", "w");
    for _ in 0..7 {
        e.run(&["new-window", "-d", "-t", "pg"]);
    }
    assert!(wait_until(LONG, || e.out(&["list-panes", "-a", "-F", "#{pane_id}"]).lines().count() == 8));
    let ids: Vec<String> = e.out(&["list-panes", "-a", "-F", "#{pane_id}"]).lines().map(|l| l.trim_start_matches('%').to_string()).collect();
    let (a, b, c, d, f_, g, h) = (&ids[0], &ids[1], &ids[2], &ids[3], &ids[4], &ids[5], &ids[6]);
    for pane in [a, b, c, d, f_, g, h] {
        assert!(e.ready(&format!("%{pane}"), Duration::from_millis(500)));
        hold(&e, pane);
        report(&e, pane, "idle", "fake");
    }

    t.section("a quick turn");
    let pr = prompt_bg(&e, a, 120);
    report(&e, a, "working", "fake");
    report(&e, a, "idle", "fake");
    let (rc, out, _) = pr.result(90);
    t.ok("working then idle answers idle", rc == 0 && out.contains("\"state\":\"idle\""));

    t.section("a settled state with no activity stalls");
    e.run(&["set", "-g", "agent-stall-time", "5"]);
    let mut pr = prompt_bg(&e, b, 120);
    report(&e, b, "done", "fake");
    // an answer would come out within a moment of the report
    t.ok("an idle or done after the text does not answer", !pr.finishes_within(Duration::from_millis(500)));
    let (rc, _, err) = pr.result(90);
    e.run(&["set", "-g", "agent-stall-time", "60"]);
    t.ok("five seconds with no working or blocked is prompt_stalled", rc == 1 && err.contains("\"code\":\"prompt_stalled\""));
    t.ok("and says delivery is unknown", err.contains("not delivered"));

    t.section("blocked answers at once");
    let pr = prompt_bg(&e, c, 120);
    report(&e, c, "blocked", "fake");
    let (rc, out, _) = pr.result(90);
    t.ok("blocked after the text answers blocked", rc == 0 && out.contains("\"state\":\"blocked\""));

    t.section("the one timeout");
    let pr = prompt_bg(&e, d, 2);
    let (rc, _, err) = pr.result(90);
    t.ok("a timeout under five seconds is timeout, not a stall", rc == 1 && err.contains("\"code\":\"timeout\"") && err.contains("not delivered"));

    t.section("another program takes the pane");
    let pr = prompt_bg(&e, f_, 120);
    report(&e, f_, "working", "other");
    let (rc, _, err) = pr.result(90);
    t.ok("another agent taking the pane ends the wait", rc == 1 && err.contains("\"code\":\"agent_not_running\"") && err.contains("another agent"));

    t.section("an agent that was working when the text came");
    report(&e, g, "working", "fake");
    // the stall time is short here so that watching for a stall takes a few seconds, and not a minute
    e.run(&["set", "-g", "agent-stall-time", "2"]);
    let mut pr = prompt_bg(&e, g, 120);
    t.ok("no stall while it works", !pr.finishes_within(Duration::from_secs(4)));
    e.run(&["set", "-g", "agent-stall-time", "60"]);
    report(&e, g, "done", "fake");
    let (rc, out, _) = pr.result(90);
    t.ok("the end of the turn answers", rc == 0 && out.contains("\"state\":\"done\""));

    t.section("error answers at once");
    let pr = prompt_bg(&e, h, 120);
    report(&e, h, "error", "fake");
    let (rc, out, _) = pr.result(90);
    t.ok("error after the text answers error", rc == 0 && out.contains("\"state\":\"error\""));
    e.kill_server();
    t.finish();
}
