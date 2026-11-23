//! P10.9, waiting for events without a race: `imux events --wait EVENT [--match key=value]...
//! [--timeout MS]`, `--since SEQ` (replay of the kept events, events_lost when some are gone), the
//! sequence number `api-seq` gives and that ends the `--json` result of a command that changes
//! something. (The Rust version of tests/q_eventswait.sh.)
use crate::support::*;
use std::process::Stdio;
use std::time::Duration;

fn num(text: &str) -> i64 {
    text.trim().parse().unwrap_or(-1)
}

#[test]
fn q_eventswait() {
    let mut e = Env::new("qew");
    let mut t = Check::new("q_eventswait");
    let conf = e.write("imux.conf", "set -g status off\nset -g default-size 100x30\nset -g agent-control on\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    e.session("ew", "first");
    assert!(e.ready("%0", Duration::from_millis(500)));
    let rename = |n: &str| e.run(&["rename-window", "-t", "ew:0", n]);

    t.section("the sequence number");
    let r = e.run(&["api-seq"]);
    t.ok("api-seq prints a number", r.code == 0 && r.text().chars().all(|c| c.is_ascii_digit()) && !r.text().is_empty());
    let s0 = num(&r.text());
    rename("one");
    let s1 = wait_until(Duration::from_secs(5), || num(&e.out(&["api-seq"])) > s0);
    t.ok("an event raises it", s1);

    t.section("--wait");
    let mut w1 = e
        .command(BIN)
        .args(["events", "--wait", "window-renamed", "--match", "window_name=waited"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));
    rename("other");
    std::thread::sleep(Duration::from_millis(500));
    t.ok("an event that does not match is not the answer", matches!(w1.try_wait(), Ok(None)));
    rename("waited");
    let done = wait_until(Duration::from_secs(10), || matches!(w1.try_wait(), Ok(Some(_))));
    let o = w1.wait_with_output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    t.ok(
        "the matching event is printed and the exit status is 0",
        done && o.status.code() == Some(0) && out.contains("\"event\":\"window-renamed\"") && out.contains("\"window_name\":\"waited\""),
    );
    t.ok("and only that line is printed", out.lines().count() == 1);
    let r = e.run(&["events", "--wait", "pane-exited", "--timeout", "700"]);
    t.ok("no event in time is a timeout, exit 1", r.code == 1 && r.all().contains("in time"));
    t.ok("a match is key=value", e.run(&["events", "--wait", "window-renamed", "--match", "nokey"]).code == 2);
    t.ok("--match needs --wait", e.run(&["events", "--match", "window_name=x"]).code == 2);
    t.ok("--wait and --types do not mix", e.run(&["events", "--wait", "window-renamed", "--types", "pane-exited"]).code == 2);
    t.ok("an event name that is not one is an error", e.run(&["events", "--wait", "nosuch-event", "--timeout", "500"]).code != 0);

    t.section("--since");
    let n = e.out(&["api-seq"]);
    rename("early");
    std::thread::sleep(Duration::from_millis(300));
    rename("late");
    std::thread::sleep(Duration::from_millis(500));
    let r = e.run(&["events", "--wait", "window-renamed", "--match", "window_name=early", "--since", &n, "--timeout", "3000"]);
    t.ok("an event that came before the wait started is found", r.code == 0 && r.out.contains("\"window_name\":\"early\""));
    let r = e.run(&["events", "--wait", "window-renamed", "--match", "window_name=late", "--since", &n, "--timeout", "3000"]);
    t.ok("so is a later one", r.code == 0 && r.out.contains("\"window_name\":\"late\""));
    let r = e.run(&["events", "--wait", "window-renamed", "--match", "window_name=early", "--timeout", "700"]);
    t.ok("without --since the past is not seen", r.code == 1);
    let r = e.run_for(&["events", "--since", &n, "--types", "window-renamed"], Duration::from_secs(2));
    let replay = r.all();
    t.ok(
        "a replay starts with the hello line and has the kept events of the named type",
        replay.lines().next().unwrap_or("").contains("\"event\":\"subscribed\"")
            && replay.matches("\"event\":\"window-renamed\"").count() == 2
            && !replay.contains("events_lost"),
    );
    let first = replay
        .lines()
        .find(|l| l.contains("window-renamed"))
        .and_then(|l| l.split("\"seq\":").nth(1))
        .map(|s| s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .map(|s| num(&s))
        .unwrap_or(-1);
    t.ok("the replay is in order and after the number", first > num(&n));
    let r = e.run(&["events", "--since", "99999999", "--types", "window-renamed"]);
    t.ok("a number from a longer life is events_lost, exit 1", r.code == 1 && r.out.contains("\"event\":\"events_lost\""));
    t.ok("a --since that is not a number is a usage error", e.run(&["events", "--since", "x"]).code == 2);

    t.section("the sequence number of a change");
    let r = e.run(&["pane", "run", "--json", "0", "true"]);
    t.matches("pane run --json ends with seq", r#"^\{"pane":"%0","sent":true,"seq":[0-9]+\}$"#, r.out.trim_end());
    let r = e.run(&["pane", "run", "0", "true"]);
    t.ok("plain output has no seq", r.code == 0 && r.out.lines().any(|l| l == "sent"));
    let r = e.run(&["pane", "run", "--json", "0", "true"]);
    let seq: String = r.out.split("\"seq\":").nth(1).unwrap_or("").chars().take_while(|c| c.is_ascii_digit()).collect();
    rename("afterrun");
    let r = e.run(&["events", "--wait", "window-renamed", "--match", "window_name=afterrun", "--since", &seq, "--timeout", "3000"]);
    t.ok("act, then wait from the noted number: the event is found", r.code == 0);
    t.ok("the schema names seq", e.all(&["api", "schema"]).contains("last event before the change"));
    e.kill_server();
    t.finish();
}
