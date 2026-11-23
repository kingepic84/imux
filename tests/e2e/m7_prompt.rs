//! The `:` command prompt, typed into a client started with no arguments: split-window, an unknown
//! command, pane numbers, resize-pane, a quoted name, new-window. (The Rust version of
//! tests/m7_prompt.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn m7_prompt() {
    let e = Env::new("m7p");
    let mut t = Check::new("m7_prompt");
    let mut c = e.client(100, 30, &[]);
    assert!(c.wait_drawn(Duration::from_secs(15)), "the client drew nothing");
    // The keys are handled in order, but a screen that changes again before the next frame is never
    // drawn, so each step waits until its result has been drawn before the next one is typed.
    let long = Duration::from_secs(15);
    c.type_chunks(&["\x02:", "split-window -h\r", "echo split-size-$(stty size | tr \" \" x)\r"], Duration::ZERO);
    c.wait_seen("split-size-29x49", long);
    c.type_chunks(&["\x02:", "frobnicate\r"], Duration::ZERO);
    c.wait_seen("unknown command: frobnicate", long);
    c.type_chunks(&["\x02q", "0", "echo picked-size-$(stty size | tr \" \" x)\r"], Duration::ZERO);
    c.wait_seen("picked-size-29x50", long);
    c.type_chunks(&["\x02:", "resize-pane -R 4\r", "echo resized-size-$(stty size | tr \" \" x)\r"], Duration::ZERO);
    c.wait_seen("resized-size-29x54", long);
    c.type_chunks(&["\x02:", "rename-window \"my build\"\r"], Duration::ZERO);
    c.wait_seen("my build", long);
    c.type_chunks(&["\x02:", "new-window\r", "\x02p"], Duration::ZERO);
    c.wait_seen("1:[a-z]", long);
    let all = c.text();
    t.matches("split-window -h at the prompt splits side by side", "split-size-29x49", &all);
    t.matches("an unknown command shows a message", "unknown command: frobnicate", &all);
    t.matches("pane numbers: pressing 0 focuses the first pane", "picked-size-29x50", &all);
    t.matches("resize-pane -R at the prompt", "resized-size-29x54", &all);
    t.matches("rename-window with a quoted name", "my build", &all);
    t.matches("new-window at the prompt adds a tab", "1:[a-z]", &all);
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    c.kill();
    e.kill_server();
    t.finish();
}
