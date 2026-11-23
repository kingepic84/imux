//! run-shell and if-shell (M35): from the configuration and from keys, the time limit, and the two
//! lines that hold: the socket cannot run them, and a name a pane could have set is never pasted
//! into shell text. (The Rust version of tests/f_shell.sh.)
use crate::support::*;
use std::time::Duration;

fn file_is(e: &Env, name: &str, want: &str) -> bool {
    let path = e.path(name);
    wait_until(Duration::from_secs(10), || std::fs::read_to_string(&path).map(|s| s.trim_end() == want).unwrap_or(false))
}

#[test]
fn f_shell() {
    let mut e = Env::new("fshell");
    let mut t = Check::new("f_shell");
    let d = p(&e.dir);
    let conf = e.path("imux.conf");
    e.set("IMUX_CONFIG", conf.to_str().unwrap());
    e.write(
        "imux.conf",
        &format!(
            r##"set -g automatic-rename off
set -g run-shell-timeout 4
run-shell "echo from-config > {d}/cfg.out"
run-shell -b "echo quiet > {d}/quiet.out"
if-shell "true" "run-shell 'echo then > {d}/if1.out'" "run-shell 'echo else > {d}/if1.out'"
if-shell "false" "run-shell 'echo then > {d}/if2.out'" "run-shell 'echo else > {d}/if2.out'"
if-shell "false" "run-shell 'echo then > {d}/if3.out'"
if-shell -F "1" "run-shell 'echo F-yes > {d}/if4.out'" "run-shell 'echo F-no > {d}/if4.out'"
if-shell -F "#{{?0,1,}}" "run-shell 'echo F-yes > {d}/if5.out'" "run-shell 'echo F-no > {d}/if5.out'"
run-shell -C "set -g display-time 1234"
run-shell -b "sleep 31337"
set -g display-panes-time 1
if-shell "true" "set -g display-panes-time 2"
set -g display-panes-time 3
run 'echo ignored > {d}/run.out'
bind C run-shell "sleep 1; echo first > {d}/order.out" \; run-shell "echo second >> {d}/order.out" \; run-shell -b "echo third >> {d}/order.out"
bind T run-shell "echo #{{window_name}} > {d}/tainted.out"
bind Q run-shell "echo #{{q:window_name}} > {d}/quoted.out"
bind E run-shell 'printf "%s" "$IMUX_WINDOW_NAME" > {d}/env.out'
bind I run-shell "echo #{{pane_id}} #{{window_index}} > {d}/ids.out"
bind P run-shell "pwd > {d}/pwd.out"
bind K run-shell -C "set -g status-left #{{window_name}}"
bind L run-shell -C "set -g status-right #{{c:window_name}}"
"##
        ),
    );
    e.run(&["new-session", "-d", "-s", "s", "-n", "it's a name"]);
    t.ok("the long command is running at first", wait_until(Duration::from_secs(5), || pgrep("sleep 31337")));
    t.section("from the configuration");
    t.ok("run-shell in the configuration runs", file_is(&e, "cfg.out", "from-config"));
    t.ok("run-shell -b runs too", wait_until(Duration::from_secs(10), || e.path("quiet.out").exists()));
    t.ok("if-shell true takes the first command", file_is(&e, "if1.out", "then"));
    t.ok("if-shell false takes the second", file_is(&e, "if2.out", "else"));
    t.ok("if-shell -F tests a format", file_is(&e, "if4.out", "F-yes"));
    t.ok("a format that is empty is false", file_is(&e, "if5.out", "F-no"));
    t.ok("if-shell false without an else does nothing", !e.path("if3.out").exists());
    t.ok("run-shell -C runs an imux command", e.out(&["show-options", "-v", "display-time"]) == "1234");
    t.ok("run runs like run-shell -b", file_is(&e, "run.out", "ignored"));
    t.ok("a line after an if-shell sees its result first (3, not 2)", e.out(&["show-options", "-v", "display-panes-time"]) == "3");
    t.section("the time limit");
    t.ok(
        "a command over run-shell-timeout is stopped with its group",
        wait_until(Duration::from_secs(15), || !pgrep("sleep 31337")),
    );
    t.section("the socket cannot run shell commands");
    let o1 = e.run(&["run-shell", &format!("echo sock > {d}/sock.out")]).all();
    t.ok("run-shell from imux <command> is refused", o1.contains("only a configuration") && !e.path("sock.out").exists());
    let o2 = e.run(&["if-shell", "true", &format!("run-shell 'echo sock > {d}/sock2.out'")]).all();
    t.ok("if-shell from imux <command> is refused", o2.contains("only a configuration") && !e.path("sock2.out").exists());
    t.section("keys: a name from the pane");
    let left_before = e.out(&["show-options", "-v", "status-left"]);
    let right_before = e.out(&["show-options", "-v", "status-right"]);
    let mut c = e.client(100, 30, &["attach", "-t", "s"]);
    t.ok("attached", e.has_client());
    assert!(c.wait_drawn(Duration::from_secs(15)), "the client drew nothing");
    c.send(b"\x02T");
    t.ok(
        "a window name in shell text is refused",
        c.wait_seen("can be set by a program in the pane", Duration::from_secs(10)) && !e.path("tainted.out").exists(),
    );
    c.send(b"\x02Q");
    t.ok("#{q:name} gives one quoted word", file_is(&e, "quoted.out", "it's a name"));
    c.send(b"\x02E");
    t.ok("$IMUX_WINDOW_NAME carries the name as it is", file_is(&e, "env.out", "it's a name"));
    c.send(b"\x02I");
    let ids_ok = wait_until(Duration::from_secs(10), || {
        let s = std::fs::read_to_string(e.path("ids.out")).unwrap_or_default();
        let s = s.trim();
        let mut it = s.split(' ');
        match (it.next(), it.next(), it.next()) {
            (Some(a), Some(b), None) => a.trim_start_matches('%').chars().all(|c| c.is_ascii_digit()) && !a.is_empty() && b.chars().all(|c| c.is_ascii_digit()) && !b.is_empty(),
            _ => false,
        }
    });
    t.ok("ids and numbers are fine in the text", ids_ok);
    c.send(b"\x02P");
    t.ok("the command runs in the directory of the pane", wait_until(Duration::from_secs(10), || std::fs::metadata(e.path("pwd.out")).map(|m| m.len() > 0).unwrap_or(false)));
    c.send(b"\x02C");
    let order_ok = wait_until(Duration::from_secs(15), || lines_of(&e.path("order.out")) == 3);
    let first_two: Vec<String> = e.read("order.out").lines().take(2).map(String::from).collect();
    t.ok("a chain on a key waits for each shell command, not for -b", order_ok && first_two == ["first", "second"]);
    c.send(b"\x02K");
    t.ok(
        "run-shell -C refuses a name a pane could have set",
        c.wait_seen("run-shell -C: ", Duration::from_secs(10)) && e.out(&["show-options", "-v", "status-left"]) == left_before,
    );
    c.send(b"\x02L");
    t.ok(
        "run-shell -C takes #{c:name} as one quoted word",
        wait_until(Duration::from_secs(10), || {
            let r = e.out(&["show-options", "-v", "status-right"]);
            r != right_before && r.contains("a name")
        }),
    );
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
    e.kill_server();
    t.finish();
}
