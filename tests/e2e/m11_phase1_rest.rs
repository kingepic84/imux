//! The rest of phase 1: swap-window, move-window, prompt history, rename-session, new-session,
//! kill-session, attach and new with errors, and the end of the server with its last session.
//! (The Rust version of tests/m11_phase1_rest.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(500);

#[test]
fn m11_phase1_rest() {
    let mut e = Env::new("m11");
    e.unset("SHELL");
    let uid = unsafe { libc::getuid() };
    let sock = e.path(&format!("imux-{uid}/default"));
    let mut t = Check::new("m11_phase1_rest");

    let mut a = e.client(100, 30, &["new", "-s", "alpha"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("alpha:", Duration::from_millis(600)));

    // clients that fail or join, from the side
    let mut e1 = e.client(100, 30, &["attach", "-t", "nosuch"]);
    t.ok("attach -t unknown is an error", e1.wait_for("can't find session: nosuch", LONG));
    e1.wait_exit(LONG);
    let mut e2 = e.client(100, 30, &["new", "-s", "alpha"]);
    t.ok("new -s with a used name is an error", e2.wait_for("duplicate session: alpha", LONG));
    e2.wait_exit(LONG);
    let mut att = e.client(100, 30, &["attach", "-t", "alpha"]);
    std::thread::sleep(Duration::from_millis(800));
    att.send(b"\x02d");
    t.ok("attach -t joins the named session", att.wait_for(r"\[detached\]", LONG));
    att.wait_exit(LONG);

    a.type_chunks(
        &[
            "export W=1\r", "\x02c", "export W=2\r", "\x02c", "export W=3\r", "$BIN ls >$TMPDIR/ls1.out\r",
            "\x02:", "swap-window -t 0\r", "\x022", "echo at3-$W >>$TMPDIR/pos.out\r", "\x020", "echo at1-$W >>$TMPDIR/pos.out\r",
            "\x02:", "move-window -t 2\r", "echo mv-$W >>$TMPDIR/pos.out\r", "\x020", "echo p1-$W >>$TMPDIR/pos.out\r",
            "\x02:", "swap-window -t 9\r",
            "\x02:", "rename-session histA\r", "\x02:", "rename-session histB\r",
            "\x02:", "\x1b[A", "\x1b[A", "\r", "$BIN ls >$TMPDIR/ls2.out\r",
        ],
        GAP,
    );
    assert!(wait_until(LONG, || e.read("ls2.out").contains("histA")), "the recalled command did not run");
    a.type_chunks(&["\x02:", "new-session\r", "\x02:", "kill-session -t nosuch\r", "\x02:", "kill-session -t histA\r", "$BIN ls >$TMPDIR/ls3.out\r"], GAP);
    assert!(wait_until(LONG, || !e.read("ls3.out").is_empty()), "the last ls did not run");
    a.type_chunks(&["\x02:", "kill-session\r"], GAP);

    let pos = e.read("pos.out");
    t.matches("swap-window -t 1 put the first window last", "^at3-1", &pos);
    t.matches("swap-window -t 1 put the last window first", "^at1-3", &pos);
    t.matches("move-window -t 3 keeps the moved window selected", "^mv-3", &pos);
    t.matches("move-window shifted the others", "^p1-2", &pos);
    t.ok("swap-window to a missing window reports it", a.seen("can't find window"));
    t.matches("ls shows windows and the attached mark", r"alpha: 3 windows \(attached\)", &e.read("ls1.out"));
    t.matches("up arrow at the prompt recalls older commands", "histA: 3 windows", &e.read("ls2.out"));
    t.lacks("the recalled line replaced the newer one", "histB", &e.read("ls2.out"));
    t.matches("new-session adds a numbered session", r"^0: 1 window", &e.read("ls3.out"));
    t.ok("kill-session -t unknown reports it", a.seen("can't find session"));
    t.lacks("kill-session -t removes that session", "histA", &e.read("ls3.out"));
    t.ok("killing the last session ends the server", wait_until(LONG, || !sock.exists()));
    a.wait_exit(LONG);
    t.ok("ls without a server says so", e.all(&["ls"]).contains("no server running"));
    t.ok("usage lists the new commands", regex_lite::Regex::new(r"usage: imux \[new").unwrap().is_match(&e.all(&["nonsense"])));
    let bad = e.server_log().lines().filter(|l| l.contains("\"level\":\"error\"") && !l.contains("can't find session") && !l.contains("duplicate session")).count();
    t.ok("no errors in server.log", bad == 0);
    t.finish();
}
