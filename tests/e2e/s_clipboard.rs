//! OSC 52 from a pane: by default and with set-clipboard off a pane cannot set the clipboard; with
//! it on, well formed writes pass (at most one a second) and nothing else does.
//! (The Rust version of tests/s_clipboard.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

fn osc52(rest: &str) -> Vec<u8> {
    format!("\x1b]52;{rest}\x07").into_bytes()
}

fn run(e: &Env, conf: &str, args: &str, hold_ms: u64) -> Client {
    e.write("conf", conf);
    let mut c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client());
    assert!(e.ready("w:", Duration::from_millis(600)));
    c.send_str(&format!("{} {args}\r", p(&e.path("clip.sh"))));
    std::thread::sleep(Duration::from_millis(hold_ms));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    std::thread::sleep(Duration::from_millis(500));
    c
}

#[test]
fn s_clipboard() {
    let mut e = Env::new("scb");
    let conf = p(&e.path("conf"));
    e.set("IMUX_CONFIG", &conf);
    e.unset("SHELL");
    let script = e.write("clip.sh", "for step in \"$@\"; do\n  case $step in\n    wait:*) sleep \"${step#wait:}\" ;;\n    query) printf '\\033]52;c;?\\007' ;;\n    *) printf '\\033]52;%s;%s\\007' \"${step%%:*}\" \"${step#*:}\" ;;\n  esac\ndone\nsleep 5\n");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut t = Check::new("s_clipboard");

    t.section("by default a pane cannot set the clipboard");
    let c = run(&e, "set autosave-interval 0\n", "c:Zmlyc3Q=", 2000);
    t.ok("set-clipboard external ignores the pane's request", !c.has_bytes(&osc52("c;Zmlyc3Q=")));

    t.section("off ignores it too");
    let c = run(&e, "set autosave-interval 0\nset set-clipboard off\n", "c:Zmlyc3Q=", 2000);
    t.ok("set-clipboard off ignores the pane's request", !c.has_bytes(&osc52("c;Zmlyc3Q=")));

    t.section("on passes well formed writes, at most one a second, and nothing else");
    let c = run(&e, "set autosave-interval 0\nset set-clipboard on\n", "c:Zmlyc3Q= c:c2Vjb25k query c:a_b x:aGk= wait:1.3 :dGhpcmQ=", 3500);
    t.ok("a valid write is passed on", c.has_bytes(&osc52("c;Zmlyc3Q=")));
    t.ok("a second write within a second is dropped", !c.has_bytes(&osc52("c;c2Vjb25k")));
    t.ok("the read form is dropped", !c.has_bytes(&osc52("c;?")));
    t.ok("a payload that is not base64 is dropped", !c.has_bytes(&osc52("c;a_b")));
    t.ok("a target that is not a selection letter is dropped", !c.has_bytes(&osc52("x;aGk=")));
    t.ok("an empty target means the clipboard, and a later write passes", c.has_bytes(&osc52("c;dGhpcmQ=")));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
