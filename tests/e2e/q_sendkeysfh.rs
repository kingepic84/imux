//! send-keys -H (bytes in hexadecimal) and -F (the words are formats), checked before anything is
//! typed. (The Rust version of tests/q_sendkeysfh.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_sendkeysfh() {
    let e = Env::new("qskfh");
    let mut t = Check::new("q_sendkeysfh");
    let out = p(&e.path("out"));
    e.run(&["new-session", "-d", "-s", "skeys", &format!("cat >{out}")]);
    let got = |line: &str| wait_until(Duration::from_secs(10), || e.read("out").lines().any(|l| l == line));
    e.run(&["send-keys", "-t", "skeys:", "-H", "68", "69", "0a"]);
    t.ok("-H types the bytes", got("hi"));
    e.run(&["send-keys", "-t", "skeys:", "-H", "0x41", "42", "0a"]);
    t.ok("-H takes 0x forms", got("AB"));
    let r = e.run(&["send-keys", "-t", "skeys:", "-H", "43", "zz"]);
    t.ok("-H with a word that is no byte is an error", r.all().contains("hexadecimal"));
    // the refused call typed nothing: the newline sent after it ends a line that has no `C` in it
    let before = e.read("out").lines().count();
    e.run(&["send-keys", "-t", "skeys:", "-H", "0a"]);
    assert!(wait_until(Duration::from_secs(10), || e.read("out").lines().count() > before), "the newline was not typed");
    t.ok("and nothing of it was typed", !e.read("out").contains('C'));
    let r = e.run(&["send-keys", "-t", "skeys:", "-H", "100"]);
    t.ok("a value over one byte is refused", r.all().contains("hexadecimal"));
    e.run(&["send-keys", "-t", "skeys:", "-F", "-l", "#{session_name}"]);
    e.run(&["send-keys", "-t", "skeys:", "Enter"]);
    t.ok("-F expands the words", got("skeys"));
    e.run(&["send-keys", "-t", "skeys:", "-l", "#{session_name}"]);
    e.run(&["send-keys", "-t", "skeys:", "Enter"]);
    t.ok("without -F a format is typed as it is", got("#{session_name}"));
    e.run(&["send-keys", "-t", "skeys:", "-F", "x#{window_index}", "Enter"]);
    t.ok("-F keeps key names working", got("x0"));
    e.kill_server();
    t.finish();
}
