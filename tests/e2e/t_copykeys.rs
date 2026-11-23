//! Copy mode keys: emacs search, select and copy, a binding in the copy mode table, list-keys for
//! the tables, and mode-keys following the editor. (The Rust version of tests/t_copykeys.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(500);

fn client_with(e: &Env, set: &[(&str, &str)], unset: &[&str]) -> Client {
    let c = e.client_env(100, 30, &["new", "-s", "w"], set, unset);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("w:", Duration::from_millis(600)), "the pane never settled");
    c
}

fn leave(e: &Env, mut c: Client) {
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
    e.kill_server();
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn t_copykeys() {
    let mut e = Env::new("tck");
    let conf = p(&e.path("imux.conf"));
    e.set("IMUX_CONFIG", &conf);
    e.unset("SHELL");
    let mut t = Check::new("t_copykeys");

    t.section("emacs keys: search, select, copy, paste");
    e.write("imux.conf", "set copy-command \"\"\nset mode-keys emacs\n");
    let mut c = client_with(&e, &[], &[]);
    c.send_str("echo copy-me-$((6*7))\r");
    assert!(e.shows_line("w:", "copy-me-42"));
    c.type_chunks(&["\x02[", "\x12", "copy-me-4", "\r", "\x00", "\x05", "\x1bw"], GAP);
    c.send_str("echo got:");
    c.type_chunks(&["\x02]"], GAP);
    c.send(b"\r");
    t.ok("C-r finds the text, C-Space and C-e select to the end of the line, M-w copies", c.wait_for("got:copy-me-42", LONG));
    leave(&e, c);

    t.section("a binding in the copy mode table");
    e.write("imux.conf", "set copy-command \"\"\nset mode-keys vi\nbind -T copy-mode-vi Z send -X cancel\nunbind -T copy-mode-vi q\n");
    let mut c = client_with(&e, &[], &[]);
    c.type_chunks(&["\x02[", "Z"], GAP);
    c.send_str("echo bound-$((1+2))\r");
    t.ok("a key bound with bind -T copy-mode-vi works", c.wait_for("bound-3", LONG));
    c.type_chunks(&["\x02[", "q"], GAP);
    c.send_str("echo unbound-$((1+2))\r");
    std::thread::sleep(Duration::from_secs(1));
    t.ok("an unbound key does nothing (the mode is still on, the typed line is not run)", !c.text().contains("unbound-3"));
    leave(&e, c);

    t.section("list-keys shows the copy mode tables");
    e.write("imux.conf", "set copy-command \"\"\n");
    let c = client_with(&e, &[], &[]);
    let keys = e.all(&["list-keys"]);
    t.matches("the vi table is listed", "^bind-key -T copy-mode-vi y +send-keys -X copy-selection-and-cancel$", &keys);
    t.matches("and the emacs table", "^bind-key -T copy-mode C-Space +send-keys -X begin-selection$", &keys);
    leave(&e, c);

    t.section("mode-keys follows the editor when the configuration does not say");
    let mut c = client_with(&e, &[("EDITOR", "vim")], &["VISUAL"]);
    c.type_chunks(&["\x02[", "y"], GAP);
    c.send_str("echo ed-$((1+2))\r");
    t.ok("with vim as the editor the vi keys are on (y leaves)", c.wait_for("ed-3", LONG));
    leave(&e, c);
    let mut c = client_with(&e, &[("EDITOR", "nano")], &["VISUAL"]);
    c.type_chunks(&["\x02[", "y"], GAP);
    c.send_str("echo ed-$((1+2))\r");
    std::thread::sleep(Duration::from_secs(1));
    t.ok("with another editor the emacs keys are on (y does nothing, the mode stays)", !c.text().contains("ed-3"));
    leave(&e, c);

    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
