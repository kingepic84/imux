//! Paste buffers through a real client: set-buffer, paste-buffer, list-buffers, delete-buffer,
//! choose-buffer, escapes that never reach the pane, save and load, buffer-limit.
//! (The Rust version of tests/t_buffers.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(350);

fn new_client(e: &Env) -> Client {
    let c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("w:", Duration::from_millis(600)), "the pane never settled");
    c
}

/// Type each chunk, a moment apart, as a person would.
fn type_all(c: &mut Client, chunks: &[&str]) {
    for chunk in chunks {
        c.send(chunk.as_bytes());
        std::thread::sleep(GAP);
    }
}

fn fresh(e: &Env, mut c: Client) {
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn t_buffers() {
    let mut e = Env::new("tbuf");
    let conf = e.write("imux.conf", "set copy-command \"\"\nset set-clipboard off\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("t_buffers");

    t.section("set-buffer, paste-buffer, list-buffers, delete-buffer");
    {
        let mut c = new_client(&e);
        type_all(&mut c, &["\x02:", "set-buffer \"'x'y\"\r", "echo ok-", "\x02]", "\r", "\x02:", "set-buffer -b mine hello\r", "\x02#"]);
        t.ok("a pasted buffer is typed into the pane (the shell strips the quotes)", c.wait_seen("ok-xy", LONG));
        t.ok("list-buffers shows an automatic buffer with its size", c.wait_seen("buffer0: 4 bytes: \"'x'y\"", LONG));
        t.ok("and a named one, newest first", c.wait_seen("mine: 5 bytes: \"hello\"", LONG));
        type_all(&mut c, &["\x1b", "\x02-", "\x02#", "\x1b", "\x02:", "paste-buffer -b mine\r"]);
        fresh(&e, c);
    }

    t.section("delete-buffer takes the newest, a named paste takes its own");
    {
        let mut c = new_client(&e);
        type_all(&mut c, &["\x02:", "set-buffer -b first one-text\r", "\x02:", "set-buffer -b second two-text\r", "\x02-", "echo gone-", "\x02:", "paste-buffer -b first\r", "\r", "\x02-", "\x02-", "\x02#"]);
        t.ok("paste-buffer -b pastes that buffer", c.wait_seen("gone-one-text", LONG));
        t.ok("a delete on an empty stack says so", c.wait_seen("no buffers", LONG));
        fresh(&e, c);
    }

    t.section("choose-buffer");
    {
        let mut c = new_client(&e);
        type_all(&mut c, &["\x02:", "set-buffer -b lower low-part\r", "\x02:", "set-buffer -b upper up-part\r", "\x02="]);
        t.ok("the chooser lists the buffers", c.wait_seen("lower: 8 bytes", LONG));
        type_all(&mut c, &["\x1b", "echo pick-", "\x02=", "j", "\r", "\r"]);
        t.ok("choosing one pastes it", c.wait_seen("pick-low-part", LONG));
        fresh(&e, c);
    }

    t.section("escapes in a buffer never reach the pane, save and load");
    {
        let esc = e.write("esc.txt", "a\x1bb\n");
        let saved = e.path("saved.txt");
        let mut c = new_client(&e);
        type_all(&mut c, &["\x02:", &format!("load-buffer {}\r", p(&esc)), "echo e-", "\x02]", "\r", "\x02:", &format!("save-buffer {}\r", p(&saved)), "\x02:", &format!("save-buffer -a {}\r", p(&saved))]);
        t.ok("the escape is gone and the rest is typed", c.wait_seen("e-ab", LONG));
        t.ok("save-buffer writes the buffer as it is, and -a adds to the file", wait_until(LONG, || std::fs::metadata(&saved).map(|m| m.len()).unwrap_or(0) == 8));
        t.ok("with the escape kept in the file (it is the paste that cleans)", std::fs::read(&saved).map(|b| b.windows(3).any(|w| w == b"a\x1bb")).unwrap_or(false));
        t.ok("and the file is private", std::fs::metadata(&saved).map(|m| m.permissions().mode() & 0o777 == 0o600).unwrap_or(false));
        fresh(&e, c);
    }

    t.section("buffer-limit");
    {
        let mut c = new_client(&e);
        type_all(&mut c, &["\x02:", "set buffer-limit 2\r", "\x02:", "set-buffer one\r", "\x02:", "set-buffer two\r", "\x02:", "set-buffer three\r", "\x02#"]);
        t.ok("the newest are kept", c.wait_seen("buffer2: 5 bytes: \"three\"", LONG));
        t.ok("and the one before", c.seen("buffer1: 3 bytes: \"two\""));
        t.ok("the oldest is dropped", !c.seen("buffer0: 3 bytes"));
        fresh(&e, c);
    }

    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
