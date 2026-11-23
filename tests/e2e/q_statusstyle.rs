//! status-left-style, status-right-style, window-status-style, -current-style, -last-style, and
//! the old status-bg / status-fg spelling: each color is unique, so the bytes a client writes show
//! which option reached the bar. (The Rust version of tests/q_statusstyle.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_statusstyle() {
    let mut e = Env::new("qss");
    let mut t = Check::new("q_statusstyle");
    let conf = e.write(
        "imux.conf",
        "set -g status-left LEFT\nset -g status-right RIGHT\nset -g status-left-style fg=colour201\nset -g status-right-style fg=colour202\nset -g window-status-style fg=colour203\nset -g window-status-current-style fg=colour204\nset -g window-status-last-style fg=colour205\nset -g status-bg colour206\nset -g status-justify centre\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    e.session("sty", "one");
    e.run(&["new-window", "-d", "-t", "sty", "-n", "two"]);
    e.run(&["new-window", "-d", "-t", "sty", "-n", "three"]);
    e.run(&["select-window", "-t", "sty:two"]);
    e.run(&["select-window", "-t", "sty:three"]);
    t.ok("status-justify is a real option", e.all(&["show-options", "-g", "status-justify"]).lines().any(|l| l == "status-justify centre"));
    t.ok("status-bg changed status-style", e.all(&["show-options", "-g", "status-style"]).contains("bg=colour206"));
    let all = e.with_client("sty", |c| {
        c.wait_seen("RIGHT", Duration::from_secs(5));
        e.barrier(c, "sty");
    });
    t.ok("status-left-style reaches the left segment", all.contains("38;5;201"));
    t.ok("status-right-style reaches the right segment", all.contains("38;5;202"));
    t.ok("window-status-style reaches the other tabs", all.contains("38;5;203"));
    t.ok("window-status-current-style reaches the current tab", all.contains("38;5;204"));
    t.ok("window-status-last-style reaches the last tab", all.contains("38;5;205"));
    t.ok("status-bg colors the bar", all.contains("48;5;206"));
    e.kill_server();
    t.finish();
}
