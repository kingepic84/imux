//! word-separators: where the vi `w` motion of copy mode stops. Built in, letters and digits make a
//! word and `-` is punctuation; with only a blank as separator the word runs to the blank.
//! (The Rust version of tests/q_wordsep.sh.)
use crate::support::*;
use std::time::Duration;

fn yank_word(e: &Env, buf: &std::path::Path) {
    e.with_client("s", |c| {
        // one client's keys are handled in the order they are typed: no waiting between them
        c.send(b"\x02[");
        c.send_str("?AA-BB");
        c.send(b"\r");
        for k in ["v", "w", "y"] {
            c.send_str(k);
        }
        c.send(b"\x02:");
        c.send_str(&format!("save-buffer {}\r", buf.display()));
        wait_until(Duration::from_secs(10), || buf.exists());
    });
}

#[test]
fn q_wordsep() {
    let mut e = Env::new("qwo");
    let conf = e.write("imux.conf", "set copy-command \"\"\nset mode-keys vi\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_wordsep");
    e.session("s", "one");
    assert!(e.ready("s:one", Duration::from_millis(600)));
    e.run(&["send-keys", "-t", "s:one", "printf \"AA-BB CC\\\\n\"", "Enter"]);
    assert!(e.shows_line("s:one", "AA-BB CC"));
    let buf = e.path("buf");
    yank_word(&e, &buf);
    t.ok("built in, w stops after the hyphen", std::fs::read(&buf).unwrap_or_default() == b"AA-");
    e.run(&["set", "-g", "word-separators", " "]);
    let _ = std::fs::remove_file(&buf);
    yank_word(&e, &buf);
    t.ok("with a blank as the only separator, w goes to the next word", std::fs::read(&buf).unwrap_or_default() == b"AA-BB C");
    e.kill_server();
    t.finish();
}
