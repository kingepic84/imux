//! `{ }` command blocks in configuration files, `source-file` and the `:` prompt's command lines:
//! a binding with several commands, blocks over several lines with comments, nested blocks in
//! `if-shell`, a hook, and errors that name the line. (The Rust version of tests/q_blocks.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_blocks() {
    let mut e = Env::new("qblk");
    let mut t = Check::new("q_blocks");
    let conf = e.write(
        "imux.conf",
        "bind F5 { new-window; rename-window one }\nbind F6 {\n  # three commands, one per line\n  new-window\n  rename-window \"two words\"   # a trailing comment }\n  select-window -t 0\n}\nbind F7 { if-shell \"true\" { rename-window yes } { rename-window no } }\nset-hook -g session-renamed { display-message \"renamed\" ; select-window -t 0 }\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    e.session("blk", "w");
    let keys = e.all(&["list-keys"]);
    t.ok("the config with blocks loaded", !keys.is_empty());
    t.matches("a one line block binds a chain", r#"(?m)F5 +new-window \\; rename-window "?one"?$"#, &keys);
    t.matches("a block over several lines binds a chain", r#"(?m)F6 +new-window \\; rename-window "?two words"? \\; select-window"#, &keys);
    t.ok("the comment inside it is gone", !keys.contains("trailing comment"));
    t.ok("a nested block binds an if-shell", keys.contains("F7"));
    let hooks = e.all(&["show-hooks"]);
    t.ok("a hook takes a block", hooks.contains("session-renamed") && hooks.contains("display-message"));
    let src = |name: &str, text: &str| {
        let f = e.write(name, text);
        e.run(&["source-file", &p(&f)]).all()
    };
    src("ok.conf", "if-shell \"true\" {\n  set -g status-interval 7\n  set -g display-time 1234\n} {\n  set -g status-interval 9\n}\n");
    let o1 = || e.all(&["show-options"]);
    t.ok("if-shell runs its first block when the test passes", wait_until(Duration::from_secs(5), || o1().lines().any(|l| l == "status-interval 7")));
    t.ok("all the commands of the block", o1().lines().any(|l| l == "display-time 1234"));
    src("ok2.conf", "if-shell \"false\" {\n  set -g status-interval 11\n} {\n  set -g status-interval 13\n  set -g display-time 4321\n}\n");
    t.ok(
        "and its second block when it fails",
        wait_until(Duration::from_secs(5), || o1().lines().any(|l| l == "status-interval 13") && o1().lines().any(|l| l == "display-time 4321")),
    );
    let s = src("stray.conf", "set -g status on\ndisplay-message ok\n}\n");
    t.ok("a stray } is an error that names the line", s.contains("stray.conf:3") && s.contains("unexpected }"));
    let s = src("open.conf", "set -g status on\nbind F8 {\n  new-window\nset -g status off\n");
    t.ok("a block that never closes is an error that names the line it began on", s.contains("open.conf:2") && s.contains("no matching }"));
    let s = src("bad.conf", "bind F9 { new-window; frobnicate }\n");
    t.ok("a bad command inside a block is reported", s.contains("bad.conf:1") && s.contains("unknown command"));
    let k2 = e.all(&["list-keys"]);
    t.ok("the keys of the bad files are not bound", !k2.contains("F8") && !k2.contains("F9"));
    src("quote.conf", "bind F10 { display-message \"a } b\" ; display-message 'c ; d' }\n");
    let k3 = e.all(&["list-keys"]);
    t.ok("braces and semicolons inside quotes stay in the quotes", k3.contains("F10") && k3.contains("a } b"));
    e.with_client("blk", |c| {
        c.send(b"\x02:");
        std::thread::sleep(Duration::from_millis(300));
        c.send(b"rename-window { x }");
        std::thread::sleep(Duration::from_millis(300));
        c.send(b"\r");
        wait_until(Duration::from_secs(5), || e.out(&["list-windows", "-F", "#{window_name}"]).lines().any(|l| l == "x"));
    });
    t.ok("the prompt takes a block too", e.out(&["list-windows", "-F", "#{window_name}"]).lines().any(|l| l == "x"));
    e.kill_server();
    t.finish();
}
