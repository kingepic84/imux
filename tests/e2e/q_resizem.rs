//! bind -n MouseDrag1Border resize-pane -M: the stock mouse line means "do what a border drag
//! does", so the built-in drag still works; any other command bound to that key takes the drag
//! over. (The Rust version of tests/q_resizem.sh.)
use crate::support::*;

/// Start a fresh server with `config`, drag the border between two panes with the mouse, and
/// return the width of the left pane before and after, and what the client drew.
fn run_with(e: &Env, config: &str) -> (i32, i32, String) {
    e.write("imux.conf", &format!("{config}\n"));
    e.kill_server();
    e.run(&["new-session", "-d", "-s", "rm", "-n", "w"]);
    e.run(&["split-window", "-h", "-d", "-t", "rm:"]);
    let width = || e.out(&["list-panes", "-t", "rm:", "-F", "#{pane_width}"]).lines().next().unwrap_or("0").parse::<i32>().unwrap_or(0);
    let w0 = width();
    let out = e.with_client("rm", |c| {
        c.send_str(&format!("\x1b[<0;{};5M", w0 + 1));
        c.send_str(&format!("\x1b[<32;{};5M", w0 + 6));
        c.send_str(&format!("\x1b[<32;{};5M", w0 + 11));
        c.send_str(&format!("\x1b[<0;{};5m", w0 + 11));
        c.sync();
    });
    (w0, width(), out)
}

#[test]
fn q_resizem() {
    let mut e = Env::new("qrm");
    let mut t = Check::new("q_resizem");
    let conf = e.write("imux.conf", "");
    e.set("IMUX_CONFIG", &p(&conf));
    let (w0, w1, out) = run_with(&e, "bind -n MouseDrag1Border resize-pane -M");
    t.ok("the binding loaded", e.out(&["list-keys"]).contains("MouseDrag1Border"));
    t.ok("the left pane got wider by the drag", w1 > w0);
    t.lacks("and no error was shown for the command", "unknown flag|usage: resize", &out);
    let (w0, w1, _) = run_with(&e, "set -g @none 1");
    t.ok("with no binding the drag resizes as before", w1 > w0);
    let r = e.run(&["resize-pane", "-M"]);
    t.matches("resize-pane -M from the shell is no resize", "unknown flag|usage", &r.all());
    e.kill_server();
    t.finish();
}
