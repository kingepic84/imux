//! Event hooks (M36): set-hook / show-hooks, after-<command>, events found by the server
//! (sessions, windows, panes, clients, agents), arrays, the depth limit, and the rule that a name
//! a pane could have set is never pasted into shell text. (The Rust version of tests/f_hooks.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn f_hooks() {
    let mut e = Env::new("fhooks");
    let mut t = Check::new("f_hooks");
    let d = p(&e.dir);
    let ev = format!("{d}/ev.out");
    let conf = r##"set -g automatic-rename off
set-hook -g session-created "run-shell 'echo session-created:\$IMUX_HOOK_SESSION_NAME >> @EV@'"
set-hook -g after-new-window 'run-shell "echo after-new-window >> @EV@"'
set-hook -g window-linked 'run-shell "echo window-linked >> @EV@"'
set-hook -g window-renamed "run-shell 'echo window-renamed:\$IMUX_HOOK_WINDOW_NAME >> @EV@'"
set-hook -ga window-renamed 'run-shell "echo second-command >> @EV@"'
set-hook -g pane-exited 'run-shell "echo pane-exited >> @EV@"'
set-hook -g pane-title-changed 'run-shell "echo bad:#{hook_window_name} >> @T@/bad.out"'
set-hook -g session-renamed 'run-shell "echo session-renamed:#{q:hook_session_name} >> @EV@"'
set-hook -g client-attached 'run-shell "echo client-attached >> @EV@"'
set-hook -g client-detached 'run-shell "echo client-detached >> @EV@"'
set-hook -g after-set-option 'run-shell "echo loop >> @T@/loop.out" ; set -g display-panes-time 5'
set-hook -g worktree-created 'run-shell "echo worktree >> @EV@"'
set-hook -g nonsense-event 'display-message x'
set-hook -g alert-bell 'display-message x'
"##
    .replace("@EV@", &ev)
    .replace("@T@", &d);
    let confp = e.write("imux.conf", &conf);
    e.set("IMUX_CONFIG", &p(&confp));
    let seen = |line: &str| wait_until(Duration::from_secs(10), || e.read("ev.out").lines().any(|l| l == line));

    t.section("sessions, windows, panes");
    e.run(&["new-session", "-d", "-s", "h'1", "-n", "w one"]);
    t.ok("session-created fires, with the name in the environment", seen("session-created:h'1"));
    t.ok("window-linked fires for its window", seen("window-linked"));
    e.run(&["new-window", "-t", "h'1", "-n", "second"]);
    t.ok("after-new-window fires for a command from the shell", seen("after-new-window"));
    e.run(&["rename-window", "-t", "h'1:1", "x y"]);
    t.ok("window-renamed gets the new name in the environment", seen("window-renamed:x y"));
    t.ok("a second command of the same hook runs too (-a)", seen("second-command"));
    e.run(&["rename-session", "-t", "h'1", "r'2"]);
    t.ok("#{q:hook_session_name} is one quoted word", seen("session-renamed:r'2"));
    e.run(&["send-keys", "-t", "r'2:1", "printf '\\033]2;title\\007'", "Enter"]);
    // the hook for a title change fires as the title changes: once the title is there, it has had its say
    assert!(wait_until(Duration::from_secs(10), || e.var("r'2:1", "#{pane_title}") == "title"), "the title never changed");
    t.ok("a window name in shell text is refused, the command does not run", !e.path("bad.out").exists());
    e.run(&["kill-pane", "-t", "r'2:1"]);
    t.ok("pane-exited fires", seen("pane-exited"));

    t.section("show-hooks and set-hook -u");
    let h = e.all(&["show-hooks"]);
    t.matches("show-hooks lists names with slots (0)", "(?m)^window-renamed\\[0\\] ", &h);
    t.matches("show-hooks lists names with slots (1)", "(?m)^window-renamed\\[1\\] ", &h);
    t.matches("names imux does not know are kept", "(?m)^nonsense-event\\[0\\] ", &h);
    t.matches("names imux does not know are kept (alert-bell)", "(?m)^alert-bell\\[0\\] ", &h);
    e.run(&["set-hook", "-u", "window-renamed[1]"]);
    let h = e.all(&["show-hooks"]);
    t.ok(
        "set-hook -u name[slot] removes one command",
        !h.contains("\nwindow-renamed[1] ") && !h.starts_with("window-renamed[1] ") && h.contains("window-renamed[0] "),
    );
    e.run(&["set-hook", "-u", "pane-exited"]);
    t.lacks("set-hook -u name removes the hook", "(?m)^pane-exited", &e.all(&["show-hooks"]));
    t.ok("a bad hook name is an error", e.all(&["set-hook", "-g", "Bad Name", "x"]).contains("bad hook name"));
    t.ok("set-hook without a command is an error", e.all(&["set-hook", "-g", "pane-exited"]).contains("usage"));

    t.section("a hook that sets an option fires itself: the depth limit stops it");
    e.run(&["set", "-g", "display-time", "1111"]);
    assert!(wait_until(Duration::from_secs(10), || lines_of(&e.path("loop.out")) >= 1), "the hook never ran");
    // the runs come one after the other and then stop: counted once they have stood still
    let n = stable(Duration::from_secs(2), || lines_of(&e.path("loop.out")));
    t.ok(&format!("it ran, and not forever ({n} runs)"), (1..=8).contains(&n));
    t.ok("the server still answers", e.run(&["list-sessions"]).success());
    t.ok("and it has stopped", stable(Duration::from_secs(2), || lines_of(&e.path("loop.out"))) == n);
    e.run(&["set-hook", "-u", "after-set-option"]);

    t.section("clients");
    e.with_client("r'2", |c| e.barrier(c, "r'2"));
    t.ok("client-attached fires", seen("client-attached"));
    t.ok("client-detached fires", seen("client-detached"));
    e.kill_server();
    t.finish();
}
