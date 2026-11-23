//! P10.4, `imux pane run PANE text...` (one ordered write: the text as a paste, then Enter, with no
//! agent needed), `imux agent read PANE [--lines N] [--ansi]` (the recent text of a pane), the
//! rules for a program in a pane (only the panes it spawned), the agent rules (agent-control, a
//! blocked agent), and `send-keys` writing nothing when a word of it is refused.
//! (The Rust version of tests/q_panerun.sh.)
use crate::support::*;
use std::time::Duration;

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

fn file_has(e: &Env, name: &str, pattern: &str) -> bool {
    let re = regex_lite::Regex::new(&translate_pattern(pattern)).unwrap();
    wait_until(Duration::from_secs(10), || re.is_match(&e.read(name)))
}

#[test]
fn q_panerun() {
    let mut e = Env::new("qprun");
    let mut t = Check::new("q_panerun");
    let conf = e.write("imux.conf", "set -g status off\nset -g default-size 80x24\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    let d = p(&e.dir);
    e.session("pr", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    hold(&e, "0");
    e.run(&["split-window", "-d"]);
    assert!(e.ready("%1", Duration::from_millis(500)));
    let nlines = |s: &str| s.lines().count();

    t.section("typing a line into a pane");
    let r = e.run(&["pane", "run", "1", &format!("echo ran-$((1+1)) >{d}/f1")]);
    t.ok("pane run answers sent", r.code == 0 && has_line(&r.out, "sent"));
    t.ok("the shell in the pane ran the line", file_has(&e, "f1", "^ran-2$"));
    e.run(&["pane", "run", "1", &format!("echo A >>{d}/f2")]);
    e.run(&["pane", "run", "1", &format!("echo B >>{d}/f2")]);
    file_has(&e, "f2", "^B$");
    t.ok("two calls run in the order they were made", e.read("f2").replace('\n', "") == "AB");
    e.run(&["pane", "run", "1", &format!("echo L1 >>{d}/f3\necho L2 >>{d}/f3")]);
    file_has(&e, "f3", "^L2$");
    t.ok("a line break in the text is another Enter", e.read("f3").replace('\n', "") == "L1L2");
    e.run_in(&["pane", "run", "1", "-"], &format!("echo S >{d}/f4\n"));
    t.ok("a lone - reads the standard input", file_has(&e, "f4", "^S$"));
    let r = e.run(&["pane", "run", "1", "--json", &format!("echo J >{d}/f5")]);
    t.matches("--json says which pane and that it was sent", r#"^\{"pane":"%1","sent":true(,"seq":[0-9]+)?\}$"#, r.out.trim_end());
    file_has(&e, "f5", "^J$");
    let r = e.run(&["pane", "run", "1", "--", &format!("-n echo-dash >{d}/f6")]);
    t.ok("after -- the text may start with a dash", r.code == 0);

    t.section("the text is a paste");
    e.write("bp.sh", "case \"$2\" in on) printf \"\\033[?2004h\";; esac\nprintf \"\\033]2;ready\\007\"\nwhile IFS= read -r l; do printf \"%s\\n\" \"$l\" >>\"$1\"; done\n");
    let pon = e.out(&["split-window", "-d", "-P", "-F", "#{pane_id}", &format!("sh {d}/bp.sh {d}/bp-on on")]).trim_start_matches('%').to_string();
    let poff = e.out(&["split-window", "-d", "-P", "-F", "#{pane_id}", &format!("sh {d}/bp.sh {d}/bp-off off")]).trim_start_matches('%').to_string();
    // These programs print nothing on the screen; each sets its title when it has set up, after the
    // bracketed paste mode (the server reads their output in order).
    for pane in [&pon, &poff] {
        assert!(wait_until(Duration::from_secs(15), || e.var(&format!("%{pane}"), "#{pane_title}") == "ready"), "the program of pane {pane} did not start");
    }
    e.run(&["pane", "run", &pon, "hello"]);
    e.run(&["pane", "run", &poff, "hello"]);
    t.ok(
        "a program that asked for bracketed paste gets the markers",
        file_has(&e, "bp-on", "hello") && e.read("bp-on").contains("\x1b[200~hello\x1b[201~"),
    );
    t.ok("any other program gets the bare text", file_has(&e, "bp-off", "hello") && has_line(&e.read("bp-off"), "hello"));
    e.run(&["pane", "run", &poff, "x\x1b[201~y\x03z"]);
    file_has(&e, "bp-off", "x.*y.*z");
    t.ok("escape and control characters are not typed", !e.read("bp-off").contains('\x1b'));

    t.section("refusals");
    let r = e.run(&["pane", "run", "99", "--json", "echo x"]);
    t.ok("a missing pane is not_found on stderr, exit 1", r.code == 1 && r.out.is_empty() && r.err.contains("\"code\":\"not_found\""));
    let r = e.run(&["pane", "run", "99", "echo x"]);
    t.ok("plain: the text without the code", r.code == 1 && has_line(&r.out, "imux: no pane 99"));
    let r = e.run(&["pane", "run", "1", "--json", "   "]);
    t.ok("nothing to send is invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\""));
    let big = "a".repeat(9000);
    let r = e.run(&["pane", "run", "1", "--json", &big]);
    t.ok("text over 8 KiB is limit_reached", r.code == 1 && r.err.contains("\"code\":\"limit_reached\""));
    let r = e.run(&["pane", "run"]);
    t.ok("a bare pane run prints its usage and exits 2", r.code == 2);
    let r = e.run(&["pane", "run", "1"]);
    t.ok("a pane without text is a usage error, exit 2", r.code == 2 && r.out.contains("usage: imux pane run pane"));

    t.section("a pane that runs an agent");
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} pane report-agent --source t --agent fake --state idle"));
    t.ok("the holder pane reported", rc == 0);
    let r = e.run(&["pane", "run", "0", "--json", "echo x"]);
    t.ok("typing into an agent pane needs agent-control", r.code == 1 && r.err.contains("\"code\":\"permission_denied\"") && r.err.contains("agent-control"));
    e.run(&["set", "-g", "agent-control", "on"]);
    let r = e.run(&["pane", "run", "0", "echo x"]);
    t.ok("with agent-control on it is typed", r.code == 0 && has_line(&r.out, "sent"));
    in_pane(&e, "0", &format!("{BIN} pane report-agent --source t --agent fake --state blocked --message ok"));
    let r = e.run(&["pane", "run", "0", "--json", "echo x"]);
    t.ok("a blocked agent is refused, the text would answer it", r.code == 1 && r.err.contains("\"code\":\"agent_blocked\""));
    e.run(&["set", "-g", "agent-control", "off"]);

    t.section("reading a pane");
    e.run(&["pane", "run", "1", "echo MARK-$((20+22))"]);
    e.shows("%1", "^MARK-42$");
    let r = e.run(&["agent", "read", "1"]);
    t.ok("agent read prints the text of the pane", r.code == 0 && r.out.contains("MARK-42"));
    let r = e.run(&["agent", "read", "1", "--lines", "1"]);
    t.ok("--lines 1 is one line", r.code == 0 && nlines(&r.out) == 1);
    let r = e.run(&["agent", "read", "1", "--lines", "3"]);
    t.ok("--lines 3 is at most three", r.code == 0 && nlines(&r.out) <= 3);
    let r = e.run(&["agent", "read", "1"]);
    t.ok("the default is at most 100 lines", nlines(&r.out) <= 100);
    let r = e.run(&["agent", "read", "1", "--lines", "5", "--json"]);
    t.ok(
        "--json has the pane, the count and the text",
        r.code == 0 && r.out.starts_with("{\"pane\":\"%1\",\"lines\":5,\"text\":\"") && nlines(&r.out) == 1,
    );
    e.run(&["pane", "run", "1", "printf '\\033[31mred\\033[0m\\n'"]);
    e.shows("%1", "^red$");
    let r = e.run(&["agent", "read", "1", "--lines", "4"]);
    t.ok("plain text has no escape sequences", r.out.contains("red") && !r.out.contains('\x1b'));
    let r = e.run(&["agent", "read", "1", "--lines", "6", "--ansi"]);
    t.matches("--ansi keeps them", "\x1b\\[[0-9;]*31m", &r.out);
    let r = e.run(&["agent", "read", "99", "--json"]);
    t.ok("a missing pane is not_found", r.code == 1 && r.err.contains("\"code\":\"not_found\""));
    t.ok("--lines 0 is a usage error", e.run(&["agent", "read", "1", "--lines", "0"]).code == 2);
    t.ok("--lines over 10000 is a usage error", e.run(&["agent", "read", "1", "--lines", "10001"]).code == 2);

    t.section("from a pane");
    let (_, o) = in_pane(&e, "0", &format!("{BIN} new-window -d -P -F '#{{pane_id}}'"));
    let kid = o.lines().find(|l| l.starts_with('%')).unwrap_or("").trim_start_matches('%').to_string();
    t.ok("the pane in the holder spawned a pane", !kid.is_empty());
    e.ready(&format!("%{kid}"), Duration::from_millis(400));
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} pane run {kid} \"echo from-pane >{d}/f7\""));
    t.ok("it may type into the pane it spawned", rc == 0 && file_has(&e, "f7", "^from-pane$"));
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} pane run 1 \"echo no >{d}/f8\""));
    t.ok("it may not type into one it did not spawn", rc != 0 && o.contains("not one this pane spawned"));
    // a command that touches a file, typed to the same pane afterwards, runs after anything the refused call typed
    e.run(&["send-keys", "-t", "%1", "-l", &format!("touch {d}/after8")]);
    e.run(&["send-keys", "-t", "%1", "Enter"]);
    assert!(wait_until(Duration::from_secs(10), || e.path("after8").exists()), "the pane did not run its own command");
    t.ok("and nothing was typed", !e.path("f8").exists());
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} agent read {kid} --lines 3"));
    t.ok("it may read the pane it spawned", rc == 0);
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} agent read 1 --lines 3"));
    t.ok("it may not read another pane", rc != 0);
    e.run(&["set", "-g", "command-from-panes", "off"]);
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} pane run {kid} \"echo off >{d}/f9\""));
    e.run(&["send-keys", "-t", &format!("%{kid}"), "-l", &format!("touch {d}/after9")]);
    e.run(&["send-keys", "-t", &format!("%{kid}"), "Enter"]);
    assert!(wait_until(Duration::from_secs(10), || e.path("after9").exists()), "the pane did not run its own command");
    t.ok("with command-from-panes off it only looks", rc != 0 && o.contains("command-from-panes") && !e.path("f9").exists());
    e.run(&["set", "-g", "command-from-panes", "spawned"]);

    t.section("send-keys writes nothing when it refuses");
    e.run(&["send-keys", "-t", "%1", "-H", "7a", "7a"]);
    t.ok("control: valid hex bytes are typed", e.shows("%1", "zz"));
    e.run(&["send-keys", "-t", "%1", "C-u"]);
    e.run(&["send-keys", "-t", "%1", "C-l"]);
    e.run(&["send-keys", "-t", "%1", "-l", "echo cleared-1"]);
    e.run(&["send-keys", "-t", "%1", "Enter"]);
    assert!(e.shows("%1", "cleared-1"), "the pane did not take its own text");
    let r = e.run(&["send-keys", "-t", "%1", "-H", "71", "71", "5g"]);
    t.ok("a bad hex byte is refused", r.all().contains("hexadecimal"));
    let r = e.run(&["send-keys", "-t", "%1", "-N", "0", "qq"]);
    t.ok("a bad repeat count is refused", r.all().contains("1 to 1000"));
    // text typed afterwards shows after anything the refused calls had typed
    e.run(&["send-keys", "-t", "%1", "-l", "echo cleared-2"]);
    e.run(&["send-keys", "-t", "%1", "Enter"]);
    assert!(e.shows("%1", "cleared-2"), "the pane did not take its own text");
    t.ok("nothing of either was typed", !e.capture("%1").contains("qq"));
    e.kill_server();
    t.finish();
}
