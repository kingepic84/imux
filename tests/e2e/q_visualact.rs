//! visual-activity: output in a window nobody views shows a message with the window number, once,
//! when the flag goes up; with the option off the message is not shown.
//! (The Rust version of tests/q_visualact.sh.)
use crate::support::*;
use std::time::Duration;

fn attach_and_print(e: &Env, name: &str, look_for: &str) -> String {
    e.with_client("m", |c| {
        e.run(&["new-window", "-d", "-t", "m", "-n", name]);
        e.ready(&format!("m:{name}"), Duration::from_millis(400));
        e.run(&["send-keys", "-t", &format!("m:{name}"), "echo noise", "Enter"]);
        if look_for.is_empty() {
            // nothing is to be shown: the activity has happened (the pane has printed) and the client
            // has drawn everything that came before the sync message
            e.shows(&format!("m:{name}"), "^noise$");
            e.barrier(c, "m");
        } else {
            c.wait_seen(look_for, Duration::from_secs(8));
        }
    })
}

#[test]
fn q_visualact() {
    let e = Env::new("qva");
    let mut t = Check::new("q_visualact");
    e.session("m", "cur");
    e.run(&["setw", "-g", "monitor-activity", "on"]);
    let off = attach_and_print(&e, "bg", "");
    t.ok("visual-activity off: no message", !off.contains("Activity in window"));
    e.run(&["set", "-g", "visual-activity", "on"]);
    let on = attach_and_print(&e, "bg2", "Activity in window 2");
    t.ok("visual-activity on: the message names the window", on.contains("Activity in window 2"));
    e.kill_server();
    t.finish();
}
