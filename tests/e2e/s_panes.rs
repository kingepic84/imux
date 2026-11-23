//! Phase 7 (S3/S7): what a hostile program in a pane can do to the server. An OSC string that never
//! ends must not grow the server, text that reorders or injects must not reach the status data,
//! and the server must go on answering. (The Rust version of tests/s_panes.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0)
}

fn type_line(e: &Env, text: &str) {
    e.run(&["send-keys", "-t", "%0", "-l", text]);
    e.run(&["send-keys", "-t", "%0", "Enter"]);
}

fn pane_has(e: &Env, text: &str) -> bool {
    e.capture("%0").contains(text)
}

fn wait_pane(e: &Env, secs: u64, text: &str) -> bool {
    wait_until(Duration::from_secs(secs), || pane_has(e, text))
}

fn rss_kb(pid: &str) -> i64 {
    let out = Command::new("ps").args(["-o", "rss=", "-p", pid]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

#[test]
fn s_panes() {
    let mut e = Env::new("sp");
    let conf = e.write("conf", "set autosave-interval 0\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let uid = unsafe { libc::getuid() };
    let sockdir = e.path(&format!("imux-{uid}"));
    let sock = sockdir.join("default");
    let log = sockdir.join("server.log");
    let mut t = Check::new("s_panes");

    assert!(e.run(&["new-session", "-d", "-s", "w"]).success(), "cannot start a session");
    assert!(e.ready("%0", Duration::from_millis(600)));
    let lsof = Command::new("lsof").arg("-t").arg(&sock).output().unwrap();
    let server = String::from_utf8_lossy(&lsof.stdout).lines().next().unwrap_or("").to_string();
    t.ok(&format!("found the server process ({server})"), !server.is_empty());

    t.section("the socket is private");
    t.ok("socket mode 600", mode(&sock) == 0o600);
    t.ok("socket directory mode 700", mode(&sockdir) == 0o700);
    t.ok("server log mode 600", mode(&log) == 0o600);

    t.section("an OSC string that never ends does not grow the server");
    // The server takes in this stream slowly in a debug build (about 0.13 MB/s), so 200 MB is not
    // waited out: the shell version never did either (the typed command line already contains
    // FLOODED). What it measures is that the server stays small and keeps answering while the flood
    // runs, and that the pane works again once the flood stops.
    let rss0 = rss_kb(&server);
    type_line(&e, "(printf '\\033]0;'; head -c 200000000 /dev/zero | tr '\\0' a); echo FLOODED");
    let mut answered = true;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(2));
        answered &= e.run_for(&["ls"], Duration::from_secs(5)).success();
    }
    let growth = (rss_kb(&server) - rss0) / 1024;
    println!("     server grew {growth} MB while it took in the flood (limit 64)");
    t.ok("the server grew less than 64 MB", growth < 64);
    t.ok("the server answered throughout the flood", answered);
    pkill("head -c 200000000 /dev/zero");
    // The pane itself is inside the string that never ends and swallows what follows (that is the
    // point); replacing it is how it comes back.
    e.run(&["respawn-pane", "-k", "-t", "%0"]);
    assert!(e.ready("%0", Duration::from_millis(600)), "the respawned pane never settled");
    t.ok("the server still answers", e.run(&["ls"]).success());
    type_line(&e, "printf 'after-%s\\n' flood");
    t.ok("the pane shows text after the flood", e.shows_line("%0", "after-flood"));

    t.section("a string that ends is still run, a long title is cut");
    type_line(&e, "printf '\\033]0;%s\\007' \"$(head -c 100000 /dev/zero | tr '\\0' t)\"; echo TITLED");
    t.ok("a 100 KB title does not disturb the pane", wait_pane(&e, 10, "TITLED"));

    t.section("bidirectional and control characters are refused in agent text");
    type_line(&e, &format!("{BIN} pane report-agent --source t --agent x --state working --message \"$(printf 'a\\342\\200\\256b')\" 2>&1 | sed 's/^/R1:/'"));
    t.ok("report with a right-to-left override answered", wait_pane(&e, 10, "R1:imux"));
    t.ok("refused for the bidirectional character", e.capture("%0").lines().any(|l| l.contains("R1:") && l.to_lowercase().contains("bidirectional")));
    type_line(&e, &format!("{BIN} pane report-agent --source t --agent x --state working --message fine 2>&1 | sed 's/^/R2:/'; echo R2DONE"));
    t.ok("a plain message passes", wait_pane(&e, 10, "R2DONE"));
    let refused = e.capture("%0").lines().any(|l| l.contains("R2:") && { let l = l.to_lowercase(); l.contains("refused") || l.contains("error") || l.contains("bidirectional") });
    t.ok("plain message not refused", !refused);

    t.section("events escape bidirectional characters");
    let ev = e.path("ev.txt");
    let mut child = e.command(BIN).args(["events", "--types", "window-renamed"]).stdout(Stdio::from(std::fs::File::create(&ev).unwrap())).stderr(Stdio::null()).spawn().unwrap();
    // renames until the stream has its first line, which shows the server has the subscriber
    let size = || std::fs::metadata(&ev).map(|m| m.len()).unwrap_or(0);
    let mut probes = 0;
    while size() == 0 {
        probes += 1;
        assert!(probes < 60, "the event stream never carried an event");
        e.run(&["rename-window", "-t", "w", &format!("probe{probes}")]);
        wait_until(Duration::from_millis(300), || size() > 0);
    }
    let lines_before = lines_of(&ev);
    e.run(&["rename-window", "-t", "w", "x\u{202e}y"]);
    assert!(wait_until(Duration::from_secs(10), || lines_of(&ev) > lines_before), "the rename made no event");
    let _ = child.kill();
    let _ = child.wait();
    let events = std::fs::read(&ev).unwrap_or_default();
    t.ok("the rename produced an event", String::from_utf8_lossy(&events).contains("window-renamed"));
    t.ok("no bidirectional character in the event stream", !events.windows(3).any(|w| w == b"\xe2\x80\xae"));

    t.section("a pane that never reads its input cannot freeze the server");
    let filler = e.out(&["new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "w", "stty -icanon -echo; i=0; while [ $i -lt 4000 ]; do printf '\\033[c\\033[6n'; i=$((i+1)); done; sleep 120"]);
    // the server is asked again and again while the pane fills its own input queue (a few hundred
    // replies fill it), until the pane has said all it has to say (it ends in `sleep`), but at least
    // four times and at most forty
    let mut stuck = false;
    let mut asked = 0;
    while asked < 4 || (asked < 40 && e.var(&filler, "#{pane_current_command}") != "sleep") {
        if !e.run_for(&["ls"], Duration::from_secs(5)).success() {
            stuck = true;
        }
        asked += 1;
    }
    t.ok("the server answered while a pane filled its own input queue", !stuck);
    type_line(&e, "echo STILLHERE");
    t.ok("the first pane still takes input", wait_pane(&e, 10, "STILLHERE"));
    e.kill_server();
    t.ok("no errors in server.log", !std::fs::read_to_string(&log).unwrap_or_default().contains("\"level\":\"error\""));
    t.finish();
}
