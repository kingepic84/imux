//! P10.5, `imux pane wait-output PANE (--match TEXT | --regex RE) [--source recent|visible]
//! [--lines N] [--timeout MS] [--json]`: a line that is already there, a line that comes later,
//! the timeout, a pattern that does not compile, a pane that goes away or whose program ended, the
//! limit of waits for one caller, a client that leaves, the two sources, escape sequences left
//! out, and the rules for a program in a pane. (The Rust version of tests/q_panewait.sh.)
use crate::support::*;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// A wait running in the background.
struct Bg(Child);

fn bgwait(e: &Env, args: &[&str]) -> Bg {
    let child = e
        .command(BIN)
        .args(["pane", "wait-output"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    Bg(child)
}

impl Bg {
    /// Its exit status and output if it ended within `secs`.
    fn within(mut self, secs: u64) -> Option<(i32, String)> {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Ok(Some(_)) = self.0.try_wait() {
                let o = self.0.wait_with_output().unwrap();
                return Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
        None
    }
}

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn q_panewait() {
    let mut e = Env::new("qpw");
    let mut t = Check::new("q_panewait");
    let conf = e.write("imux.conf", "set -g status off\nset -g default-size 100x30\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    e.session("pw", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    hold(&e, "0");
    e.run(&["split-window", "-d"]);
    assert!(e.ready("%1", Duration::from_millis(500)));
    let w = |args: &[&str]| {
        let mut v = vec!["pane", "wait-output"];
        v.extend_from_slice(args);
        e.run(&v)
    };

    t.section("a line that is already there");
    e.run(&["pane", "run", "1", "echo EARLY-$((40+2))"]);
    e.shows("%1", "^EARLY-42$");
    let r = w(&["1", "--match", "EARLY-42", "--timeout", "3000"]);
    t.ok("it answers at once with the line", r.code == 0 && has_line(&r.out, "EARLY-42"));
    let r = w(&["1", "--regex", "^EARLY-4[0-9]$", "--timeout", "3000"]);
    t.ok("a regular expression matches the line too", r.code == 0 && has_line(&r.out, "EARLY-42"));
    let r = w(&["1", "--match", "EARLY-42", "--json"]);
    t.ok(
        "--json has the pane, the line and what was searched",
        r.code == 0 && r.out.starts_with("{\"pane\":\"%1\",\"line\":\"EARLY-42\",\"text\":\"") && r.out.trim_end().lines().count() == 1,
    );

    t.section("a line that comes later");
    let bg = bgwait(&e, &["1", "--match", "LATE-42"]);
    std::thread::sleep(Duration::from_millis(700));
    e.run(&["pane", "run", "1", "echo LATE-$((40+2))"]);
    let res = bg.within(10);
    t.ok("it ends when the line is printed", matches!(&res, Some((0, o)) if has_line(o, "LATE-42")));
    let bg = bgwait(&e, &["1", "--regex", "^LATER-[0-9]+$"]);
    std::thread::sleep(Duration::from_millis(500));
    e.run(&["pane", "run", "1", "echo LATER-$((1+1))"]);
    let res = bg.within(10);
    t.ok("a regular expression waits for its line", matches!(&res, Some((_, o)) if has_line(o, "LATER-2")));
    e.run(&["pane", "run", "1", "printf '\\033[31mCOLOR-ME\\033[0m\\n'"]);
    e.shows("%1", "^COLOR-ME$");
    let r = w(&["1", "--regex", "^COLOR-ME$", "--timeout", "3000"]);
    t.ok("escape sequences are not part of the text", r.code == 0 && !r.out.contains('\x1b'));

    t.section("the timeout and the refusals");
    let start = Instant::now();
    let r = w(&["1", "--match", "NEVER-APPEARS-ZZ", "--timeout", "700", "--json"]);
    t.ok("a wait that finds nothing is timeout, exit 1, nothing on stdout", r.code == 1 && r.out.is_empty() && r.err.contains("\"code\":\"timeout\""));
    t.ok("and ends near the timeout", start.elapsed() <= Duration::from_secs(5));
    let r = w(&["1", "--regex", "(", "--json"]);
    t.ok("a pattern that does not compile is invalid_regex", r.code == 1 && r.err.contains("\"code\":\"invalid_regex\""));
    let long = "a".repeat(600);
    let r = w(&["1", "--regex", &long, "--json"]);
    t.ok("a pattern over 512 bytes is invalid_regex", r.code == 1 && r.err.contains("\"code\":\"invalid_regex\""));
    let r = w(&["1", "--match", &long, "--json"]);
    t.ok("so is text over 512 bytes, as invalid_argument", r.code == 1 && r.err.contains("\"code\":\"invalid_argument\""));
    let r = w(&["99", "--match", "x", "--json"]);
    t.ok("a missing pane is not_found", r.code == 1 && r.err.contains("\"code\":\"not_found\""));
    let r = w(&["99", "--match", "x"]);
    t.ok("plain: the text without the code", r.code == 1 && has_line(&r.out, "imux: no pane 99"));
    let r = w(&["1"]);
    t.ok("no pattern is a usage error, exit 2", r.code == 2 && r.out.contains("usage: imux pane wait-output pane"));
    let r = w(&["1", "--match", "a", "--regex", "b"]);
    t.ok("both patterns are a usage error, exit 2", r.code == 2);
    let r = w(&["1", "--match", "a", "--lines", "0"]);
    t.ok("--lines 0 is a usage error, exit 2", r.code == 2);

    t.section("the two sources");
    e.run(&["pane", "run", "1", "i=0; while [ $i -lt 60 ]; do echo SCROLL-$i; i=$((i+1)); done"]);
    e.shows("%1", "^SCROLL-59$");
    let r = w(&["1", "--regex", "^SCROLL-2$", "--source", "visible", "--timeout", "600", "--json"]);
    t.ok("a line that scrolled off the screen is not visible", r.code == 1 && r.err.contains("\"code\":\"timeout\""));
    let r = w(&["1", "--regex", "^SCROLL-2$", "--source", "recent", "--lines", "100", "--timeout", "3000"]);
    t.ok("recent looks above the screen", r.code == 0 && has_line(&r.out, "SCROLL-2"));
    let r = w(&["1", "--regex", "^SCROLL-2$", "--source", "recent", "--lines", "5", "--timeout", "600"]);
    t.ok("but only as far as --lines says", r.code == 1);
    let r = w(&["1", "--regex", "^SCROLL-59$", "--source", "visible", "--timeout", "3000"]);
    t.ok("the last line is on the screen", r.code == 0);

    t.section("a pane that goes away");
    let gone = e.out(&["split-window", "-d", "-P", "-F", "#{pane_id}", "sleep 2"]).trim_start_matches('%').to_string();
    let r = w(&[&gone, "--match", "NEVER-APPEARS-ZZ", "--timeout", "20000", "--json"]);
    t.ok("a pane that closes ends the wait with not_found", r.code == 1 && r.err.contains("\"code\":\"not_found\"") && r.err.contains("closed"));
    e.run(&["set", "-g", "remain-on-exit", "on"]);
    let dead = e.out(&["split-window", "-d", "-P", "-F", "#{pane_id}", "sh -c 'echo BYE-$((6*7)); exit 0'"]).trim_start_matches('%').to_string();
    wait_until(Duration::from_secs(10), || e.var(&format!("%{dead}"), "#{pane_dead}") == "1");
    let r = w(&[&dead, "--match", "NEVER-APPEARS-ZZ", "--timeout", "20000", "--json"]);
    t.ok("a pane whose program ended is not_ready", r.code == 1 && r.err.contains("\"code\":\"not_ready\""));
    let r = w(&[&dead, "--match", "BYE-", "--timeout", "3000"]);
    t.ok("but what it printed can still be found", r.code == 0 && r.out.contains("BYE-"));
    e.run(&["set", "-g", "remain-on-exit", "off"]);

    t.section("limits, and a client that leaves");
    let mut waits = Vec::new();
    for n in 1..=8 {
        let m = format!("NEVER-APPEARS-{n}");
        waits.push(bgwait(&e, &["1", "--match", &m]));
    }
    std::thread::sleep(Duration::from_secs(2));
    // Eight background processes under a loaded machine can take a while to register.
    let limited = wait_until(Duration::from_secs(60), || {
        let r = w(&["1", "--match", "NEVER-APPEARS-9", "--timeout", "600", "--json"]);
        r.code == 1 && r.err.contains("\"code\":\"limit_reached\"")
    });
    t.ok("a ninth wait of the same caller is limit_reached", limited);
    for mut b in waits {
        let _ = b.0.kill();
        let _ = b.0.wait();
    }
    let freed = wait_until(Duration::from_secs(15), || {
        let r = w(&["1", "--match", "NEVER-APPEARS-10", "--timeout", "600", "--json"]);
        r.code == 1 && r.err.contains("\"code\":\"timeout\"")
    });
    t.ok("after the clients left their waits are gone", freed);

    t.section("from a pane");
    let (_, o) = in_pane(&e, "0", &format!("{BIN} new-window -d -P -F '#{{pane_id}}'"));
    let kid = o.lines().find(|l| l.starts_with('%')).unwrap_or("").trim_start_matches('%').to_string();
    t.ok("the pane in the holder spawned a pane", !kid.is_empty());
    e.ready(&format!("%{kid}"), Duration::from_millis(400));
    e.run(&["pane", "run", &kid, "echo KID-$((6*7))"]);
    e.shows(&format!("%{kid}"), "^KID-42$");
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} pane wait-output {kid} --match KID-42 --timeout 3000"));
    t.ok("it may wait on the pane it spawned", rc == 0 && has_line(&o, "KID-42"));
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} pane wait-output 1 --match EARLY-42 --timeout 3000"));
    t.ok("not on one it did not spawn", rc != 0 && o.contains("not one this pane spawned"));
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} pane wait-output 0 --match pane_holder --timeout 3000"));
    t.ok("its own pane is fine", rc == 0);
    e.run(&["set", "-g", "command-from-panes", "off"]);
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} pane wait-output {kid} --match KID-42 --timeout 3000"));
    t.ok("with command-from-panes off it reads only its own pane", rc != 0 && o.contains("only read its own pane"));
    let (rc, _) = in_pane(&e, "0", &format!("{BIN} pane wait-output 0 --match pane_holder --timeout 3000"));
    t.ok("and that still works", rc == 0);
    e.run(&["set", "-g", "command-from-panes", "spawned"]);
    e.kill_server();
    t.finish();
}
