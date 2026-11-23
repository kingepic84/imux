//! User options (`@name`): set, append, unset, scope-free reads in formats (display-message, list-*,
//! the status bar), show-options, limits, names, and the taint of a value a pane gave when it is
//! used in shell text. (The Rust version of tests/q_useropt.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn dm(e: &Env, f: &str) -> String {
    e.out(&["display-message", "-p", f])
}

fn nap(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[test]
fn q_useropt() {
    let mut e = Env::new("quo");
    let conf = e.write("imux.conf", "set -g @from_config cfg-value\nset -gq @quiet_one q1\nset -g status-left \"L:#{@from_config}:\"\nset -g status-right \"\"\nset -g command-from-panes on\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_useropt");
    e.session("s", "one");
    let d = p(&e.dir);

    t.section("values from the configuration");
    t.ok("a configuration value reads back in display-message", dm(&e, "#{@from_config}") == "cfg-value");
    t.ok("set -gq on a user option stores it", dm(&e, "#{@quiet_one}") == "q1");
    t.ok("an unset user option is empty", dm(&e, "#{@nope}").is_empty());
    t.ok("a conditional sees it", e.all(&["display-message", "-p", "#{?@from_config,yes,no}"]).trim() == "yes");

    t.section("set, append, unset through the socket");
    e.run(&["set", "-g", "@a", "one"]);
    t.ok("set stores", dm(&e, "#{@a}") == "one");
    e.run(&["set", "-ag", "@a", "-two"]);
    t.ok("set -a appends", dm(&e, "#{@a}") == "one-two");
    e.run(&["set", "-g", "@a", "with space"]);
    t.ok("a value with a space", dm(&e, "#{@a}") == "with space");
    e.run(&["set", "-gu", "@a"]);
    t.ok("set -u removes it", dm(&e, "#{@a}").is_empty());
    e.run(&["setw", "-g", "@w", "winval"]);
    t.ok("setw -g stores a user option too", dm(&e, "#{@w}") == "winval");

    t.section("show-options");
    t.ok("show-options -v prints the value", e.all(&["show-options", "-gv", "@from_config"]).lines().any(|l| l == "cfg-value"));
    t.ok("show-options prints name and value", e.all(&["show-options", "-g", "@from_config"]).lines().any(|l| l == "@from_config cfg-value"));
    let all = e.all(&["show-options"]);
    t.ok("the full listing has the user options too", all.lines().any(|l| l == "@from_config cfg-value") && all.lines().any(|l| l.starts_with("status-left ")));
    let q = e.run(&["show-options", "-gqv", "@missing"]);
    t.ok("-q on a missing user option is quiet and empty", q.success() && q.all().is_empty());
    let m = e.run(&["show-options", "-gv", "@missing"]);
    t.ok("a missing user option without -q is an error", !m.success() || m.all().to_lowercase().contains("unknown"));

    t.section("formats elsewhere");
    t.ok("list-windows -F reads it", e.out(&["list-windows", "-t", "s", "-F", "#{window_name}:#{@from_config}"]) == "one:cfg-value");
    t.ok("list-panes -F reads it", e.out(&["list-panes", "-t", "s", "-F", "#{@from_config}"]) == "cfg-value");
    e.run(&["display-menu", "-t", "s:one", "-T", "#{@from_config}", "x", "x", "display-message ok"]);

    t.section("the status bar");
    let out = e.with_client("s", |c| {
        c.wait_seen("L:cfg-value:", LONG);
    });
    t.ok("status-left shows the configuration value", out.contains("L:cfg-value:"));
    e.run(&["set", "-g", "@from_config", "changed"]);
    let out = e.with_client("s", |c| {
        c.wait_seen("L:changed:", LONG);
    });
    t.ok("a change shows on the next draw", out.contains("L:changed:"));

    t.section("names and limits");
    t.ok("a name with a space is refused", e.all(&["set", "-g", "@bad name", "x"]).to_lowercase().contains("bad user option name"));
    t.ok("a bare @ is refused", e.all(&["set", "-g", "@", "x"]).to_lowercase().contains("bad user option name"));
    let big = "x".repeat(5000);
    t.ok("a value over 4 KiB is refused", e.all(&["set", "-g", "@big", &big]).to_lowercase().contains("longer than 4096"));
    e.run(&["set", "-g", "@ctl", "a\x1b[31mb"]);
    t.ok("control characters are dropped from a value", dm(&e, "#{@ctl}") == "a[31mb");

    t.section("shell text and the taint");
    e.run(&["set", "-g", "@mine", "trusted"]);
    assert!(e.ready("s:one", Duration::from_millis(600)));
    e.run(&["send-keys", "-t", "s:one", &format!("{BIN} set -g @theirs from-a-pane"), "Enter"]);
    t.ok("a pane may set one when command-from-panes is on", wait_until(LONG, || dm(&e, "#{@theirs}") == "from-a-pane"));
    e.run(&["bind", "x", "run-shell", &format!("echo #{{@mine}} >{d}/rs1.out")]);
    e.run(&["bind", "y", "run-shell", &format!("echo #{{@theirs}} >{d}/rs2.out")]);
    e.run(&["bind", "z", "run-shell", &format!("echo #{{q:@theirs}} >{d}/rs3.out")]);
    let out = e.with_client("s", |c| {
        for k in [b"\x02x", b"\x02y", b"\x02z"] {
            c.send(k);
            nap(900);
        }
    });
    t.ok("a value the user set may be used in shell text", e.read("rs1.out").lines().any(|l| l == "trusted"));
    t.ok("a value a pane set is refused unquoted, naming the way out", e.read("rs2.out").is_empty() && out.contains("can be set by a program in the pane"));
    t.ok("written #{q:...} it is allowed", e.read("rs3.out").lines().any(|l| l == "from-a-pane"));
    e.run(&["set", "-g", "@theirs", "mine-now"]);
    e.with_client("s", |c| {
        c.send(b"\x02y");
        wait_until(LONG, || e.read("rs2.out").lines().any(|l| l == "mine-now"));
    });
    t.ok("the user setting it again makes it trusted", e.read("rs2.out").lines().any(|l| l == "mine-now"));

    t.section("a dry read keeps nothing and starts nothing");
    e.kill_server();
    nap(800);
    t.ok("check-config still lists them as information", e.all(&["check-config", &p(&conf)]).contains("user-option: @from_config"));
    t.finish();
}
