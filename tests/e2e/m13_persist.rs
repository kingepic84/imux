//! Persistence: save a session by name, restore it into a fresh server, the autosave and what
//! SIGTERM and kill-server do to it, a damaged autosave, and delete-snapshot.
//! (The Rust version of tests/m13_persist.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0)
}

fn server_pid(sock: &Path) -> Option<String> {
    let out = std::process::Command::new("lsof").arg("-t").arg(sock).output().ok()?;
    for pid in String::from_utf8_lossy(&out.stdout).lines() {
        let cmd = std::process::Command::new("ps").args(["-o", "command=", "-p", pid]).output().ok()?;
        if String::from_utf8_lossy(&cmd.stdout).trim_end().ends_with(" server") {
            return Some(pid.to_string());
        }
    }
    None
}

#[test]
fn m13_persist() {
    let mut e = Env::new("m13");
    let conf = e.write("conf", "set autosave-interval 1\nset restore-commands \"sleep\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let d = e.dir.clone();
    let state = d.join("state");
    let uid = unsafe { libc::getuid() };
    let sock = d.join(format!("imux-{uid}/default"));
    let mut dirs = Vec::new();
    for n in 1..=3 {
        std::fs::create_dir_all(d.join(format!("d{n}"))).unwrap();
        dirs.push(std::fs::canonicalize(d.join(format!("d{n}"))).unwrap());
    }
    let (p1, p2, p3) = (p(&dirs[0]), p(&dirs[1]), p(&dirs[2]));
    let na = p(&d.join("na"));
    let cleanup = || {
        pkill("sleep 31337");
        pkill(&format!("tail -F {na}"));
    };
    let snap = state.join("snapshots/snap1.imux");
    let autosave = state.join("autosave.imux");
    let mut t = Check::new("m13_persist");

    t.section("build a session, save it by name");
    {
        let mut c = e.client(100, 30, &["new", "-s", "work"]);
        assert!(e.has_client(), "the client did not attach");
        assert!(e.ready("work:", Duration::from_millis(600)));
        // a new pane or window starts where the active pane is, so each `cd` must have taken effect
        // (and each split or new window exist) before the next step
        let cwd_is = |dir: &str| wait_until(LONG, || e.var("work:", "#{pane_current_path}") == dir);
        let count = |what: &str, n: usize| wait_until(LONG, || e.out(&[what, "-t", "work:"]).lines().count() == n);
        c.send_str(&format!("cd {p1}\r"));
        assert!(cwd_is(&p1), "the first cd did not happen");
        c.send(b"\x02%");
        assert!(count("list-panes", 2), "the split did not happen");
        c.send_str(&format!("cd {p2}\r"));
        assert!(cwd_is(&p2), "the second cd did not happen");
        c.send(b"\x02c");
        assert!(count("list-windows", 2), "the second window did not appear");
        c.send_str(&format!("cd {p3}\r"));
        assert!(cwd_is(&p3), "the third cd did not happen");
        c.send_str("sleep 31337\r");
        assert!(wait_until(LONG, || pgrep("sleep 31337")), "sleep did not start");
        c.send(b"\x02c");
        assert!(count("list-windows", 3), "the third window did not appear");
        c.send_str(&format!("tail -F {na}\r"));
        assert!(wait_until(LONG, || pgrep("sleep 31337") && pgrep(&format!("tail -F {na}"))), "the programs did not start");
        let save = e.run(&["save", "snap1"]);
        let snaps = e.run(&["snapshots"]);
        t.matches("save reports what it wrote", "saved 1 session as snap1", &save.all());
        t.matches("snapshots lists the saved name", "snap1", &snaps.all());
        let file = std::fs::read_to_string(&snap).unwrap_or_default();
        t.ok("the file records the first directory", file.contains(&p1));
        t.ok("the file records the second directory", file.contains(&p2));
        t.ok("the file records the running command", file.contains("sleep 31337"));
        t.ok("snapshot files are private", mode(&snap) == 0o600);
        t.ok("the state directory is private", mode(&state) == 0o700);
        c.send(b"\x02d");
        c.wait_exit(LONG);
    }
    e.kill_server();
    cleanup();
    t.ok("kill-server removes the autosave", !autosave.exists());

    t.section("restore the snapshot into a fresh server");
    {
        let mut other = e.client(100, 30, &["new", "-s", "other"]);
        assert!(e.has_client());
        assert!(wait_until(LONG, || e.out(&["ls"]).lines().any(|l| l.starts_with("other:"))), "the session other did not start");
        assert!(e.ready("other:", Duration::from_millis(600)));
        let r1 = e.run(&["restore", "snap1"]);
        let r2 = e.run(&["restore", "snap1"]);
        let ls = e.run(&["ls"]);
        t.matches("restore reports the session", "restored 1 session", &r1.all());
        t.matches("restoring again leaves the existing name alone", "left alone", &r2.all());
        t.matches("the restored session has its three windows", "work: 3 windows", &ls.all());
        t.ok("an allowed command runs again", wait_until(LONG, || pgrep("sleep 31337")));
        // the pane of the command that is not allowed has a shell at its prompt, and what would have
        // been typed there was typed at start
        assert!(e.ready("work:2", Duration::from_millis(600)), "the third window never settled");
        t.ok("a command that is not allowed does not", !pgrep(&format!("tail -F {na}")));
        other.send(b"\x02d");
        other.wait_exit(LONG);
    }
    {
        let mut c = e.client(100, 30, &["attach", "-t", "work"]);
        assert!(e.has_client());
        assert!(c.wait_drawn(LONG), "the client drew nothing");
        c.send(b"\x020");
        c.send_str("echo cwd-$(pwd -P)-a\r");
        c.send(b"\x02o");
        c.send_str("echo cwd-$(pwd -P)-b\r");
        t.ok("the active pane of a window is the one that was active", c.wait_seen(&format!("cwd-{p2}-a"), LONG));
        t.ok("the other pane of the window kept its directory", c.wait_seen(&format!("cwd-{p1}-b"), LONG));
        c.send(b"\x02d");
        c.wait_exit(LONG);
    }

    t.section("autosave, SIGTERM keeps it");
    t.ok("an autosave appears by itself", wait_until(LONG, || std::fs::metadata(&autosave).map(|m| m.len() > 0).unwrap_or(false)));
    t.ok("autosave files are private", mode(&autosave) == 0o600);
    let spid = server_pid(&sock);
    t.ok("found the server process", spid.is_some());
    if let Some(pid) = spid {
        e.sh(&format!("kill -TERM {pid}"));
    }
    t.ok("the server is gone after SIGTERM", wait_until(LONG, || !sock.exists()));
    cleanup();
    t.ok("SIGTERM leaves the autosave", std::fs::metadata(&autosave).map(|m| m.len() > 0).unwrap_or(false));
    t.ok("the autosave holds the session", std::fs::read_to_string(&autosave).unwrap_or_default().contains("work"));

    t.section("the next start brings it back");
    {
        let mut c = e.client(100, 30, &[]);
        assert!(e.has_client());
        assert!(wait_until(LONG, || e.all(&["ls"]).contains("work: 3 windows")), "the autosave was not restored");
        let ls = e.run(&["ls"]);
        t.matches("starting without arguments restores the autosave", "work: 3 windows", &ls.all());
        t.matches("and the other session with it", "other: 1 window", &ls.all());
        c.send(b"\x02d");
        c.wait_exit(LONG);
    }
    e.kill_server();
    cleanup();
    t.ok("kill-server removes both autosave files", !autosave.exists() && !state.join("autosave.prev").exists());
    t.ok("kill-server keeps named snapshots", std::fs::metadata(&snap).map(|m| m.len() > 0).unwrap_or(false));

    t.section("a damaged autosave is a clean start");
    std::fs::write(&autosave, "imux-snapshot 1\nsaved 1\nrec").unwrap();
    std::fs::write(state.join("autosave.prev"), "garbage\n").unwrap();
    {
        let mut c = e.client(100, 30, &["new", "-s", "fresh"]);
        assert!(e.has_client());
        assert!(wait_until(LONG, || e.all(&["ls"]).contains("fresh: 1 window")), "the requested session did not start");
        let ls = e.run(&["ls"]);
        t.matches("a damaged autosave starts the requested session", "fresh: 1 window", &ls.all());
        t.lacks("and restores nothing from it", "work", &ls.all());
        t.ok("the damage is logged", e.server_log().contains("autosave.imux unusable"));
        c.send(b"\x02d");
        c.wait_exit(LONG);
    }

    t.section("delete");
    t.matches("delete-snapshot reports it", "deleted snap1", &e.run(&["delete-snapshot", "snap1"]).all());
    t.matches("deleting a missing snapshot is an error", "no snapshot named snap1", &e.run(&["delete-snapshot", "snap1"]).all());
    t.matches("a name that is a path is refused", "bad snapshot name", &e.run(&["save", "../evil"]).all());
    t.ok("nothing was written outside the store", !state.join("evil.imux").exists() && !d.join("evil.imux").exists());
    e.kill_server();
    cleanup();
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
