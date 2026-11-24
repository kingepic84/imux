//! The event stream and agent prompt (M38): `imux events` (lines, the types filter, bad types, a
//! reader that stalls is told it lost events and the server goes on), `imux api version`, and
//! `imux agent prompt` against a fake agent (refused while agent-control is off, typed when on,
//! --wait, refused while blocked or without an agent record, refused from inside a pane unless
//! command-from-panes is on). (The Rust version of tests/f_events.sh.)
use crate::support::*;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

fn follow(e: &Env, args: &[&str], file: &str) -> Child {
    e.command(BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::fs::File::create(e.path(file)).unwrap()))
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn f_events() {
    let e = Env::new("fev");
    let mut t = Check::new("f_events");
    let d = p(&e.dir);
    let wait = |secs: u64, f: &dyn Fn() -> bool| wait_until(Duration::from_secs(secs), f);
    t.section("api version");
    t.ok("api version needs no server", e.out(&["api", "version"]) == "api=1");
    e.session("a", "first");

    t.section("the stream");
    let mut all = follow(&e, &["events"], "all.out");
    let mut some = follow(&e, &["events", "--types", "window-renamed,after-new-window"], "some.out");
    let mut agentev = follow(&e, &["events", "--types", "agent-state-changed"], "agent.out");
    t.ok("a subscription starts with a hello line", wait(10, &|| e.read("all.out").contains("subscribed")) && e.read("all.out").contains("\"api\":1"));
    // every reader has to be connected before anything happens, or a late one misses the event it is
    // asked about (on a busy machine the second and third connect after the first)
    t.ok("the other subscriptions are connected too", wait(10, &|| e.read("some.out").contains("subscribed") && e.read("agent.out").contains("subscribed")));
    e.run(&["rename-window", "-t", "a:0", "named \"one\""]);
    e.run(&["new-window", "-d", "-t", "a", "-n", "second"]);
    t.ok(
        "a rename is an event with its names",
        wait(10, &|| e.read("all.out").contains("window-renamed")) && e.read("all.out").lines().any(|l| l.contains("window-renamed") && l.contains("named \\\"one\\\"")),
    );
    let allout = e.read("all.out");
    t.matches("every line is one JSON object with seq and time", "(?m)^\\{\"event\":\"window-renamed\",\"seq\":[0-9]+,\"time\":[0-9]{13}", &allout);
    t.ok("the session and window ids are there", allout.lines().any(|l| l.contains("window-renamed") && l.contains("\"session\":\"$")));
    t.ok("no types: the after- events are not sent", !allout.contains("\"event\":\"after-"));
    t.ok("a new window is an event", wait(10, &|| e.read("all.out").contains("window-linked")));
    t.ok("named types only", wait(10, &|| e.read("some.out").contains("window-renamed")) && !e.read("some.out").contains("window-linked"));
    t.ok("an after- event when asked for by name", wait(10, &|| e.read("some.out").contains("\"event\":\"after-new-window\"")));
    let seqs: Vec<String> = e.read("all.out").split("\"seq\":").skip(1).map(|s| s.chars().take_while(|c| c.is_ascii_digit()).collect()).collect();
    let mut uniq = seqs.clone();
    uniq.sort();
    uniq.dedup();
    t.ok("the sequence goes up", uniq.len() == seqs.len());
    t.ok("an unknown type is refused", e.all(&["events", "--types", "nonsense"]).contains("unknown event type"));
    t.ok("a bad flag shows the usage", e.all(&["events", "--nope"]).contains("usage: imux events"));

    t.section("a reader that stalls");
    let mut stalled = e.command(BIN).args(["events", "--types", "after-set-option"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    // Its first line shows the server has it as a subscriber; after that it is not read again. The
    // reader stays open (it goes into `_unread`): a closed pipe would be a different way to go.
    let (first_line, got) = std::sync::mpsc::channel();
    let mut stalled_out = std::io::BufReader::new(stalled.stdout.take().unwrap());
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut stalled_out, &mut line);
        let _ = first_line.send(stalled_out);
    });
    let mut probes = 0;
    let _unread = loop {
        probes += 1;
        e.run(&["set", "-g", "@stall-probe", &probes.to_string()]);
        if let Ok(reader) = got.recv_timeout(Duration::from_millis(500)) {
            break reader;
        }
        assert!(probes < 60, "the stalled reader was never told of an event");
    };
    // The server drops a subscriber once 64 KB is queued behind the kernel's socket buffers and the
    // pipe. Linux's default 208 KB per socket swallows 3000 events, so it gets four times more.
    let pad = "x".repeat(200);
    let flood: String = (1..=12000).map(|i| format!("set -g @pad{i} {pad}\n")).collect();
    let flood_file = e.write("flood.conf", &flood);
    e.run(&["source-file", &p(&flood_file)]);
    t.ok("a subscriber that does not read is told and dropped", wait(10, &|| e.server_log().contains("does not read")));
    t.ok("the server goes on", e.run(&["has-session", "-t", "a"]).success());
    e.run(&["rename-window", "-t", "a:0", "after-the-flood"]);
    t.ok("and the others did not notice", wait(10, &|| e.read("all.out").contains("after-the-flood")) && !e.read("all.out").contains("events_lost"));
    let _ = stalled.kill();
    let _ = stalled.wait();

    t.section("agent prompt");
    let script = format!(
        "n=0\nr() {{ n=$((n + 1)); \"{BIN}\" pane report-agent --source fake --agent fake --state \"$1\" --seq \"$n\" >/dev/null 2>&1; }}\nr idle\n: >\"{d}/agent.ready\"\nwhile IFS= read -r line </dev/tty; do\n  printf '%s\\n' \"$line\" >>\"{d}/agent.got\"\n  case \"$line\" in\n    *block*) r blocked ;;\n    *) r working; sleep 1; r done ;;\n  esac\ndone\n"
    );
    let agent_sh = e.write("agent.sh", &script);
    std::fs::set_permissions(&agent_sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let ap = e.out(&["new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "a", "-n", "agent"]).trim_start_matches('%').to_string();
    let pp = e.out(&["new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "a", "-n", "plain"]).trim_start_matches('%').to_string();
    e.ready(&format!("%{ap}"), Duration::from_millis(400));
    e.ready(&format!("%{pp}"), Duration::from_millis(400));
    e.run(&["send-keys", "-t", &format!("%{ap}"), &p(&agent_sh), "Enter"]);
    t.ok("the fake agent is up", wait(10, &|| e.path("agent.ready").exists()));
    t.ok("its state is known", wait(10, &|| e.out(&["agent", "get", &ap]).contains("state: idle")));
    let r = e.run(&["agent", "prompt", &ap, "hello"]);
    t.ok("refused while agent-control is off", r.all().contains("agent-control") && !e.path("agent.got").exists());
    e.run(&["set-option", "-g", "agent-control", "on"]);
    let r = e.run(&["agent", "prompt", &ap, "hello", "world"]);
    t.ok("typed when it is on", r.all().lines().any(|l| l.starts_with("sent")) && wait(10, &|| has_line(&e.read("agent.got"), "hello world")));
    t.ok("the agent took it up", wait(10, &|| e.out(&["agent", "get", &ap]).contains("state: working")));
    // the turn lasts a second; the next prompt is for an agent that is waiting again
    t.ok("and its turn ends", wait(10, &|| e.out(&["agent", "get", &ap]).contains("state: done")));
    let began = Instant::now();
    let r = e.run(&["agent", "prompt", "--wait", "--timeout", "20", &ap, "second"]);
    let took = began.elapsed();
    t.ok(
        &format!("--wait returns when the turn is over, not at once (took {}s, the turn lasts 1s)", took.as_secs()),
        has_line(&r.all(), "done") && has_line(&e.read("agent.got"), "second") && took >= Duration::from_millis(800),
    );
    let r = e.run(&["agent", "prompt", "--wait", "--timeout", "1", &ap, "please", "block"]);
    t.ok("--wait also ends in blocked", has_line(&r.all(), "blocked"));
    let r = e.run(&["agent", "prompt", &ap, "more"]);
    t.ok("refused while the agent is blocked", r.all().contains("blocked") && !has_line(&e.read("agent.got"), "more"));
    t.ok("refused for a pane without an agent", e.all(&["agent", "prompt", &pp, "hello"]).contains("no agent record"));
    t.ok("refused for a pane that is not there", e.all(&["agent", "prompt", "999", "hello"]).contains("no pane 999"));
    t.ok("no text shows the usage", e.all(&["agent", "prompt", &ap]).contains("usage: imux agent prompt"));
    let big = "y".repeat(9000);
    t.ok("text over 8 KiB is refused", e.all(&["agent", "prompt", &ap, &big]).contains("longer than"));
    e.run(&["set-option", "-g", "command-from-panes", "off"]);
    e.run(&["send-keys", "-t", &format!("%{pp}"), &format!("{BIN} agent prompt {ap} from-pane > {d}/p6.out 2>&1"), "Enter"]);
    t.ok("refused from inside a pane", wait(10, &|| e.read("p6.out").contains("inside a pane")));
    e.run(&["set-option", "-g", "command-from-panes", "on"]);
    e.run(&["send-keys", "-t", &format!("%{pp}"), &format!("{BIN} agent prompt {ap} from-pane > {d}/p7.out 2>&1"), "Enter"]);
    t.ok(
        "from inside a pane it gets as far as the agent state when command-from-panes is on",
        wait(10, &|| e.read("p7.out").contains("blocked")),
    );
    let ev = e.read("agent.out");
    t.ok("the state changes of the fake agent were events", ev.contains("\"state\":\"blocked\"") && ev.contains("\"state\":\"working\""));
    for c in [&mut all, &mut some, &mut agentev] {
        let _ = c.kill();
        let _ = c.wait();
    }
    e.kill_server();
    t.ok("no errors in the server log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
