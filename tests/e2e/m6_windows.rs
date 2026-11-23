//! Windows through a client: a new window, a click on a tab, next, previous, a window by number,
//! killing a window, the tab names, and the server ending with its last window.
//! (The Rust version of tests/m6_windows.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

#[test]
fn m6_windows() {
    let mut e = Env::new("m6w");
    let zsh = which("zsh");
    if let Some(z) = &zsh {
        e.set("SHELL", z);
    }
    let uid = unsafe { libc::getuid() };
    let sock = e.path(&format!("imux-{uid}/default"));
    let mut t = Check::new("m6_windows");

    let mut c = e.client(100, 30, &[]);
    assert!(e.has_client());
    assert!(e.ready("0:", Duration::from_millis(800)));
    c.type_steps(
        &e,
        &[
            "export W=one\r", "\x02c", "export W=two\r", "echo now-$W\r", "\x1b[<0;8;30M\x1b[<0;8;30m", "echo clicked-$W\r", "\x02n", "echo next-$W\r", "\x02p", "echo prev-$W\r",
            "\x02c", "export W=three\r", "\x02", "1", "echo number-$W\r", "\x02", "2", "\x02&", "echo killed-$W\r", "exit\r", "exit\r",
        ],
    );
    c.wait_exit(LONG);
    t.ok("new window starts a fresh shell", c.seen("now-two"));
    t.ok("clicking the first tab selects window 0", c.seen("clicked-one"));
    t.ok("prefix n goes to the next window", c.seen("next-two"));
    t.ok("prefix p goes back", c.seen("prev-one"));
    t.ok("prefix 1 selects window 1", c.seen("number-two"));
    t.ok("prefix & kills the window and returns to the previous one", c.seen("killed-two"));
    if zsh.is_some() {
        t.ok("status line shows the tab names", c.seen("zsh"));
    } else {
        println!("skip: status line tab names need zsh, which is not installed");
    }
    t.ok("server exits when the last window closes", wait_until(LONG, || !sock.exists()));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
