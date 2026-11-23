//! paste-buffer -p: taken as a flag, and a paste into a pane that asked for bracketed paste carries
//! the markers (cat -v shows them as ^[[200~ and ^[[201~); into a pane that did not, it does not.
//! (The Rust version of tests/q_pastep.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn paste(e: &Env, command: &str, session: &str) {
    e.with_client(session, |c| {
        c.send(b"\x02:");
        std::thread::sleep(Duration::from_millis(500));
        c.send_str(&format!("{command}\r"));
        std::thread::sleep(Duration::from_millis(1500));
    });
}

#[test]
fn q_pastep() {
    let e = Env::new("qpp");
    let mut t = Check::new("q_pastep");
    e.session("on", "one");
    e.session("off", "one");
    e.run(&["send-keys", "-t", "on:one", "printf '\\033[?2004h'; cat -v", "Enter"]);
    e.run(&["send-keys", "-t", "off:one", "cat -v", "Enter"]);
    std::thread::sleep(Duration::from_secs(1));
    paste(&e, "set-buffer -b mine hello ; paste-buffer -p -b mine", "on");
    t.ok("a pane that asked gets the markers", wait_until(LONG, || { let c = e.capture("on:one"); c.contains("^[[200~hello") && c.contains("^[[201~") }));
    paste(&e, "set-buffer -b mine hello ; paste-buffer -p -b mine", "off");
    let off = e.capture("off:one");
    t.ok("a pane that did not gets the text alone", off.contains("hello") && !off.contains("200~"));
    e.kill_server();
    t.finish();
}
