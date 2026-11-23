//! Formats outside `-F` know what `-F` formats know: a display-message bound to a key, and menu and
//! popup titles, read pane and window variables, `@` options, option names and environment names.
//! (The Rust version of tests/q_fmtlookup.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

#[test]
fn q_fmtlookup() {
    let e = Env::new("qfl2");
    let mut t = Check::new("q_fmtlookup");
    e.session("s", "one");
    e.run(&["set", "-g", "@mark", "hello"]);
    e.run(&["set-environment", "-g", "FMTLOOKUP_TEST", "envvalue"]);
    e.run(&["bind-key", "x", "display-message", "M[#{window_width}|#{@mark}|#{status-position}|#{FMTLOOKUP_TEST}|#{session_name}]"]);
    e.run(&["bind-key", "y", "display-menu", "-T", "T[#{@mark}|#{status-position}|#{FMTLOOKUP_TEST}]", "one", "o", "display-message hi"]);
    let mut seen_m = false;
    e.with_client("s", |c| {
        c.send(b"\x02x");
        seen_m = c.wait_seen(r"M\[[0-9]+\|hello\|bottom\|envvalue\|s\]", LONG);
    });
    t.ok("a display-message on a key reads window variables, a user option, an option name, the environment", seen_m);
    let mut seen_t = false;
    e.with_client("s", |c| {
        c.send(b"\x02y");
        std::thread::sleep(Duration::from_millis(800));
        seen_t = c.text().contains("T[hello|bottom|envvalue]") || c.screen_text().contains("T[hello|bottom|envvalue]");
    });
    t.ok("a menu title reads a user option, an option name and the environment", seen_t);
    e.kill_server();
    t.finish();
}
