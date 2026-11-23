//! #{W:fmt,current_fmt} in a status-format row: one turn per window, the second format for the
//! current window, loop_last_flag for the separator, window formats read through #{E:...}.
//! (The Rust version of tests/q_statusloop.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_statusloop() {
    let e = Env::new("qsl");
    let mut t = Check::new("q_statusloop");
    e.session("s", "one");
    e.run(&["new-window", "-d", "-t", "s", "-n", "two"]);
    e.run(&["new-window", "-d", "-t", "s", "-n", "three"]);
    e.run(&["select-window", "-t", "s:two"]);
    e.run(&["set", "-g", "status-format[0]", "<#{W:#I.#W#{?loop_last_flag,,+},[#I.#W]}>"]);
    e.run(&["set", "-g", "status-format[1]", "#{W:#{E:window-status-format}#{?loop_last_flag,,#{window-status-separator}},#{E:window-status-current-format}#{?loop_last_flag,,#{window-status-separator}}}"]);
    e.run(&["set", "-g", "window-status-format", "a#W"]);
    e.run(&["set", "-g", "window-status-current-format", "C#W"]);
    e.run(&["set", "-g", "window-status-separator", "|"]);
    let out = e.with_client("s", |c| {
        c.wait_seen(r"aone\|Ctwo\|athree", Duration::from_secs(8));
    });
    t.matches("each window takes a turn, the current one with its own format", r"<0\.one\+\[1\.two\]2\.three>", &out);
    t.matches("window formats and the separator are read through E:", r"aone\|Ctwo\|athree", &out);
    t.ok("an unterminated loop is only text", e.run(&["set", "-g", "status-format[0]", "#{W:x"]).code == 0);
    e.kill_server();
    t.finish();
}
