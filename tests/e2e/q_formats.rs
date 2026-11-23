//! Format modifiers (=N: b: d: n: l: m: m/r: E: T: e|op|:), the variables added with them (pid,
//! socket_path, version, pane_tty, window_last_flag, pane_at_*, one-letter forms, environment
//! names), the same in the status bar, and the %if / %elif / %else / %endif / %hidden lines of a
//! config. (The Rust version of tests/q_formats.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_formats() {
    let mut e = Env::new("qfm");
    let mut t = Check::new("q_formats");
    let conf = e.write(
        "imux.conf",
        r##"%hidden GREETING=hello
set -g @greet "${GREETING}"
%if "#{>=:#{version},3.0}"
set -g @ver new
%elif "#{==:1,1}"
set -g @ver mid
%else
set -g @ver old
%endif
%if "#{==:#{version},0.1}"
set -g @skipped yes
%endif
%if #{TEST_COND_ENV}
set -g @env yes
%else
set -g @env no
%endif
%if "#{m:*x*,axb}"
%if "#{e|>|:2,1}"
set -g @nest ok
%endif
%endif
set -g status-left "S:#{b:pane_current_path}:#{version}:#{pane_at_left}:"
set -g status-right ""
set -g @tpl "[#{session_name}]"
"##,
    );
    e.set("IMUX_CONFIG", &p(&conf));
    let proj = e.path("proj");
    std::fs::create_dir_all(proj.join("sub")).unwrap();
    assert!(e.run_env(&["new-session", "-d", "-s", "s", "-n", "one", "-c", &p(&proj)], &[("TEST_COND_ENV", "1")]).success());
    e.run(&["new-window", "-t", "s", "-n", "two", "-c", &p(&proj.join("sub"))]);
    e.run(&["new-window", "-t", "s", "-n", "three"]);
    e.run(&["select-window", "-t", "s:two"]);
    e.run(&["select-window", "-t", "s:one"]);
    let fm = |f: &str| e.var("s:one", f);

    t.section("modifiers");
    t.ok("=N: keeps the first characters", fm("#{=3:session_name}") == "s" && fm("#{=3:window_name}") == "one");
    t.eq("=-N: keeps the last ones", fm("#{=-2:window_name}"), "ne".to_string());
    t.eq("b: is the last path component", fm("#{b:pane_current_path}"), "proj".to_string());
    t.eq("d: is the rest", fm("#{b:#{d:pane_current_path}}"), e.dir.file_name().unwrap().to_string_lossy().into_owned());
    t.eq("n: counts characters", fm("#{n:window_name}"), "3".to_string());
    t.eq("l: is a literal", fm("#{l:window_name}"), "window_name".to_string());
    t.ok("m: matches a glob", fm("#{m:o*,#{window_name}}") == "1" && fm("#{m:x*,#{window_name}}") == "0");
    t.eq("m/r: matches a regular expression", fm("#{m/r:^o.e$,#{window_name}}"), "1".to_string());
    t.eq("m/ri: ignores case", fm("#{m/ri:ONE,#{window_name}}"), "1".to_string());
    let sum = fm("#{e|+|:#{window_index},10}");
    t.ok("e|op|: does arithmetic", sum == "10" || sum == "11");
    t.eq("a conditional can test a match", fm("#{?#{m:*one*,#{window_name}},yes,no}"), "yes".to_string());
    t.ok("E: expands a user option as a format", fm("#{E:@tpl}") == "[s]" && fm("#{@tpl}") == "[#{session_name}]");
    t.eq("T: expands the clock too", fm("#{T:@tpl}"), "[s]".to_string());

    t.section("variables");
    t.matches("pid is a number", "^[0-9]+$", &fm("#{pid}"));
    t.ok("socket_path ends in the socket name", fm("#{socket_path}").ends_with("/default"));
    t.eq("version is the one the shim reports", fm("#{version}"), "3.4".to_string());
    t.ok("pane_tty is a device", fm("#{pane_tty}").starts_with("/dev/"));
    t.ok("#D is the pane id", fm("#D") == fm("#{pane_id}") && fm("#D").starts_with('%'));
    t.ok("#h and #H are the host", !fm("#h").is_empty() && fm("#H") == fm("#{host}") && fm("#h") == fm("#{host_short}"));
    t.ok("#F and #T are the flags and the title", fm("#F") == fm("#{window_flags}") && fm("#T") == fm("#{pane_title}"));
    let edges = fm("#{pane_at_left}#{pane_at_top}#{pane_at_right}#{pane_at_bottom}");
    t.ok("pane_at_* tell the edges of a lone pane", edges == "0000" || edges == "1111");
    t.ok("pane_mode is empty outside copy mode", fm("#{pane_mode}").is_empty());
    t.eq("session_many_attached is a flag", fm("#{session_many_attached}"), "0".to_string());
    e.run(&["split-window", "-h", "-t", "s:one"]);
    t.ok(
        "a split pane has an inner edge",
        e.var("s:one.0", "#{pane_at_left}#{pane_at_right}") == "10" || e.var("s:one.1", "#{pane_at_left}#{pane_at_right}") == "01",
    );
    e.run(&["kill-pane", "-t", "s:one.1"]);
    e.run(&["select-window", "-t", "s:one"]);
    let lastflags: String = e.out(&["list-windows", "-t", "s", "-F", "#{window_name}=#{window_last_flag}"]).lines().map(|l| format!("{l} ")).collect();
    t.eq("window_last_flag marks the window that was current before", lastflags, "one=0 two=1 three=0 ".to_string());

    t.section("environment names");
    e.run(&["set-environment", "-g", "IMUX_FMT_TEST", "hello-env"]);
    t.eq("a name that is no variable reads the environment set for panes", fm("#{IMUX_FMT_TEST}"), "hello-env".to_string());
    t.ok("an unset one is empty", fm("#{IMUX_FMT_NOT_SET_ANYWHERE}").is_empty());

    t.section("the configuration with %if lines");
    t.eq("%hidden makes a variable of the file", fm("#{@greet}"), "hello".to_string());
    t.eq("a true %if runs its branch and not the %elif", fm("#{@ver}"), "new".to_string());
    t.ok("a false %if is skipped", fm("#{@skipped}").is_empty());
    t.eq("a condition can read the environment", fm("#{@env}"), "yes".to_string());
    t.eq("conditions nest", fm("#{@nest}"), "ok".to_string());

    t.section("the status bar");
    let mut c = e.client(100, 24, &["attach", "-t", "s"]);
    c.wait_seen("S:proj:3.4:", Duration::from_secs(10));
    t.matches("status-left expands a modifier, a new variable and a pane variable", r"S:proj:3\.4:[01]:", &c.text());
    c.send(b"\x02d");
    c.wait_exit(Duration::from_secs(3));
    c.kill();

    t.section("a config that is wrong");
    e.kill_server();
    e.write("imux.conf", "%if yes\nset -g @a 1\n");
    let cc = e.all(&["check-config", &p(&conf)]);
    t.ok("check-config reads the conditional lines without calling them commands", !cc.contains("unknown-command: %if"));
    e.write("imux.conf", "%endif\n");
    e.run(&["new-session", "-d", "-s", "e", "-n", "x"]);
    e.run(&["show-options"]);
    t.ok("a server with a stray %endif still starts", e.run(&["ls"]).success());
    e.kill_server();
    t.finish();
}
