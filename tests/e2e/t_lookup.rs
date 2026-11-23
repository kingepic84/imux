//! Looking things up from a client: list-keys, show-messages, display-message, find-window, the way
//! back to the last session and window numbers. (The Rust version of tests/t_lookup.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(500);

fn new_client(e: &Env) -> Client {
    let c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("w:", Duration::from_millis(600)), "the pane never settled");
    c
}

fn leave(e: &Env, mut c: Client) {
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn t_lookup() {
    let mut e = Env::new("tlk");
    e.unset("SHELL");
    let mut t = Check::new("t_lookup");

    t.section("list-keys, show-messages, display-message");
    let mut c = new_client(&e);
    c.send(b"\x02?");
    t.ok("? opens the list of keys with a title", c.wait_seen("keys", LONG));
    t.ok("which has the split binding", c.wait_seen("bind-key -T prefix % +split-window -h", LONG));
    c.type_chunks(&["\x1b", "\x02:"], GAP);
    c.send_str("display-message \"hello-msg-#S\"\r");
    t.ok("display-message fills in variables", c.wait_seen("hello-msg-w", LONG));
    c.send(b"\x02~");
    t.ok("~ opens the list of messages", c.wait_seen("messages", LONG));
    t.ok("which has the message with a time", c.wait_seen("[0-9][0-9]:[0-9][0-9]:[0-9][0-9] hello-msg-w", LONG));
    c.type_chunks(&["\x1b", "\x02i"], GAP);
    t.ok("i shows where the client is", c.wait_seen(r"\[w\] 0:[a-z]+, pane 0", LONG));
    leave(&e, c);

    t.section("imux list-keys");
    let c = new_client(&e);
    let keys = e.all(&["list-keys"]);
    t.matches("the subcommand lists the bindings", "^bind-key -T prefix % +split-window -h$", &keys);
    t.matches("with the repeatable ones marked", "^bind-key -r -T prefix Up +select-pane -U$", &keys);
    leave(&e, c);

    t.section("find-window by name and by what a window shows");
    let mut c = new_client(&e);
    c.send_str("echo uniquemark-77\r");
    assert!(e.shows_line("w:", "uniquemark-77"));
    c.send(b"\x02c");
    assert!(wait_until(LONG, || e.out(&["list-windows"]).lines().count() == 2));
    std::thread::sleep(GAP);
    c.type_chunks(&["\x02:"], GAP);
    c.send_str("rename-window zzzname\r");
    assert!(wait_until(LONG, || e.out(&["list-windows", "-F", "#{window_name}"]).contains("zzzname")));
    c.type_chunks(&["\x02f"], GAP);
    c.send_str("uniquemark\r");
    std::thread::sleep(GAP);
    c.send_str("echo screen-$IMUX_PANE_ID\r");
    t.ok("a text on a window's screen finds that window", c.wait_seen("screen-0", LONG));
    c.type_chunks(&["\x02f"], GAP);
    c.send_str("zzzname\r");
    std::thread::sleep(GAP);
    c.send_str("echo name-$IMUX_PANE_ID\r");
    t.ok("a window name finds the window", c.wait_seen("name-1", LONG));
    c.type_chunks(&["\x02f"], GAP);
    c.send_str("nosuchthing\r");
    t.ok("no match is said", c.wait_seen("no windows matching: nosuchthing", LONG));
    leave(&e, c);

    t.section("the way back to the last session, and window numbers");
    let mut c = new_client(&e);
    c.type_chunks(&["export S=one\r", "\x02N"], GAP);
    assert!(wait_until(LONG, || e.out(&["ls"]).lines().count() == 2), "the new session did not appear");
    std::thread::sleep(GAP);
    c.type_chunks(&["export S=two\r", "\x02L"], GAP);
    c.send_str("echo session-$S\r");
    t.ok("L goes back to the last session", c.wait_seen("session-one", LONG));
    c.type_chunks(&["\x02L"], GAP);
    c.send_str("echo session-$S\r");
    t.ok("and again goes forward", c.wait_seen("session-two", LONG));
    c.type_chunks(&["\x02c", "\x02'"], GAP);
    c.send_str("0\r");
    std::thread::sleep(GAP);
    c.send_str("echo number-$IMUX_PANE_ID\r");
    t.ok("' selects a window by number", c.wait_seen("number-[0-9]", LONG));
    c.type_chunks(&["\x02r"], GAP);
    c.send_str("echo refreshed\r");
    t.ok("refresh-client keeps the session working", c.wait_seen("refreshed", LONG));
    leave(&e, c);

    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
