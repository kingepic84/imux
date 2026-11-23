//! Sessions through a client: a new session, previous and next, the session picker by keys and by a
//! click, rename, the window picker, the last window, a session ending, and the server ending with
//! the last one. (The Rust version of tests/m6_sessions.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(25);

#[test]
fn m6_sessions() {
    let mut e = Env::new("m6s");
    if let Some(z) = which("zsh") {
        e.set("SHELL", &z);
    }
    let uid = unsafe { libc::getuid() };
    let sock = e.path(&format!("imux-{uid}/default"));
    let mut t = Check::new("m6_sessions");

    let mut c = e.client(100, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(800)));
    c.type_steps(
        &e,
        &[
            "export S=one\r", "\x02N", "export S=two\r", "echo ses-$S\r", "\x02(", "echo prev-$S\r", "\x02)", "echo next-$S\r", "\x02s", "k", "\r", "echo picked-$S\r", "\x02,",
        ],
    );
    // the prompt is drawn before what is typed into it closes it
    c.wait_seen("rename-window", LONG);
    c.type_steps(
        &e,
        &[
            "\x15renamed\r", "echo after-rename\r", "\x02w", "\x1b", "echo esc-ok-$((3+4))\r", "\x1b[<0;2;30M\x1b[<0;2;30m", "j", "\r", "echo clicked-$S\r",
            "\x02c", "export W=b\r", "\x02", "0", "echo first-win\r", "\x02\t", "echo last-$W\r", "exit\r", "exit\r", "echo home-$S\r", "exit\r", "exit\r",
        ],
    );
    c.wait_exit(LONG);
    t.ok("new session starts a fresh shell", c.seen("ses-two"));
    t.ok("prefix ( goes to the previous session", c.seen("prev-one"));
    t.ok("prefix ) goes to the next session", c.seen("next-two"));
    t.ok("session picker draws its title", c.seen("sessions"));
    t.ok("picker lists sessions with window counts", c.seen("1 window"));
    t.ok("picker selection switches session on Enter", c.seen("picked-one"));
    t.ok("rename prompt shows its label", c.seen("rename-window"));
    t.ok("renamed window shows in its tab", c.seen("0:renamed"));
    t.ok("window picker draws its title", c.seen("sessions and windows"));
    t.ok("escape closes the picker and keys reach the pane again", c.seen("esc-ok-7"));
    t.ok("clicking the session segment opens the picker", c.seen("clicked-two"));
    t.ok("prefix tab returns to the last window", c.seen("last-b"));
    t.ok("closing a session's last pane moves the client to another session", c.seen("home-one"));
    t.ok("server exits when the last session ends", wait_until(LONG, || !sock.exists()));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
