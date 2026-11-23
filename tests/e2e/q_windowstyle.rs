//! window-style (every pane) and window-active-style (the active pane on top of it): each color is
//! unique, so the bytes a client writes show which pane took which.
//! (The Rust version of tests/q_windowstyle.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_windowstyle() {
    let mut e = Env::new("qws");
    let mut t = Check::new("q_windowstyle");
    let conf = e.write("imux.conf", "set -g window-style bg=colour201\nset -g window-active-style bg=colour202\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.session("sty", "one");
    e.run(&["split-window", "-d", "-t", "sty:one"]);
    let record = || e.with_client("sty", |_| std::thread::sleep(Duration::from_secs(1)));
    t.ok("window-style is a real option", e.all(&["show-options", "-g", "window-style"]).contains("bg=colour201"));
    t.ok("window-active-style is a real option", e.all(&["show-options", "-g", "window-active-style"]).contains("bg=colour202"));
    let all = record();
    t.ok("the pane that is not active takes window-style", all.contains("48;5;201"));
    t.ok("the active pane takes window-active-style", all.contains("48;5;202"));
    e.run(&["set", "-g", "window-style", "default"]);
    e.run(&["set", "-g", "window-active-style", "default"]);
    let all = record();
    t.lacks("reset to default, no pane has the colors", "48;5;20[12]", &all);
    e.kill_server();
    t.finish();
}
