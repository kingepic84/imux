//! A pane shell inherits nothing of the server: no pipes, no sockets, no other pane's pty, a
//! default SIGPIPE and the same limits; and the socket is private.
//! (The Rust version of tests/s_fds.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn s_fds() {
    let mut e = Env::new("sfds");
    let mut t = Check::new("s_fds");
    let conf = e.write("conf", "set autosave-interval 0\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let d = p(&e.dir);
    let script = e.write("fds.sh", "lsof -a -p $PPID -d 3-999 -Fftn >\"$1\" 2>&1\nperl -e 'print defined $SIG{PIPE} ? $SIG{PIPE} : \"default\"' >\"$2\" 2>&1\n{ ulimit -n; ulimit -c; } >\"$3\" 2>&1\n");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let here = std::process::Command::new("sh").arg("-c").arg("ulimit -n; ulimit -c").output().unwrap();
    let here = String::from_utf8_lossy(&here.stdout).into_owned();
    t.section("a pane shell inherits nothing of the server");
    let mut c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client());
    // The session exists once the client has finished attaching.
    assert!(wait_until(Duration::from_secs(15), || e.all(&["ls"]).lines().any(|l| l.starts_with("w:"))));
    let pane = e.out(&["new-window", "-P", "-F", "#{pane_id}", "-t", "w"]);
    assert!(e.ready(&pane, Duration::from_millis(500)));
    e.run(&["send-keys", "-t", &pane, &format!("{d}/fds.sh {d}/fds.txt {d}/pipe.txt {d}/limits-pane.txt"), "Enter"]);
    wait_until(Duration::from_secs(20), || !e.read("limits-pane.txt").is_empty());
    let fds = e.read("fds.txt");
    t.ok("the listing ran", fds.contains('f'));
    t.ok("no pipe of the server (the signal pipe)", !fds.contains("tPIPE"));
    t.ok("no listening or client socket", !fds.contains("tunix"));
    t.ok("no event queue", !fds.contains("tKQUEUE"));
    t.ok("no pty master of another pane", !fds.contains("n/dev/ptmx"));
    t.ok("SIGPIPE is not left ignored", !e.read("pipe.txt").contains("IGNORE"));
    t.eq("the pane gets the open file and core limits the server was started with", e.read("limits-pane.txt"), here);
    let uid = unsafe { libc::getuid() };
    let mode = std::fs::metadata(e.path(&format!("imux-{uid}/default"))).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
    t.ok("the socket is private", mode == 0o600);
    c.kill();
    e.kill_server();
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
