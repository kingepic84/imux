//! select-pane -d (what a client types does not reach the pane) and -e (it does again); send-keys
//! is not held back; #{pane_input_off}. (The Rust version of tests/q_inputoff.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_inputoff() {
    let e = Env::new("qio");
    let mut t = Check::new("q_inputoff");
    let out = p(&e.path("out"));
    e.run(&["new-session", "-d", "-s", "io", &format!("cat >{out}")]);
    let flag = || e.var("io:", "#{pane_input_off}");
    let seen = |s: &str| e.read("out").contains(s);
    t.ok("input is on to start with", wait_until(Duration::from_secs(5), || flag() == "0"));
    let typed = |text: &str, expect: bool| {
        e.with_client("io", |c| {
            c.send_str(&format!("{text}\r"));
            if expect {
                wait_until(Duration::from_secs(5), || seen(text));
            } else {
                // everything the client typed has been handled; text sent to the pane after that
                // comes out of `cat` after anything that had got through
                c.sync();
                e.run(&["send-keys", "-t", "io:", "-l", "probe_after"]);
                e.run(&["send-keys", "-t", "io:", "Enter"]);
                wait_until(Duration::from_secs(5), || seen("probe_after"));
            }
        });
    };
    typed("typed_one", true);
    t.ok("a client types into the pane", seen("typed_one"));
    let r = e.run(&["select-pane", "-d", "-t", "io:"]);
    t.ok("select-pane -d turns it off", flag() == "1" && r.all().is_empty());
    typed("typed_two", false);
    t.ok("what a client types is dropped", !seen("typed_two"));
    e.run(&["send-keys", "-t", "io:", "-l", "sent_three"]);
    e.run(&["send-keys", "-t", "io:", "Enter"]);
    t.ok("send-keys still reaches the pane", wait_until(Duration::from_secs(5), || seen("sent_three")));
    e.run(&["select-pane", "-e", "-t", "io:"]);
    t.ok("select-pane -e turns it on", flag() == "0");
    typed("typed_four", true);
    t.ok("typing works again", seen("typed_four"));
    let r = e.run(&["select-pane", "-d", "-e", "-t", "io:"]);
    t.ok("-d with -e is an error", r.all().contains("not both"));
    e.run(&["select-pane", "-T", "my title", "-t", "io:"]);
    t.ok("select-pane -T sets the pane title", e.var("io:", "#{pane_title}") == "my title");
    e.run(&["select-pane", "-T", "x\x1b[31my", "-t", "io:"]);
    let title = e.var("io:", "#{pane_title}");
    t.ok("a title is cleaned of control characters", !title.contains('\x1b'));
    e.kill_server();
    t.finish();
}
