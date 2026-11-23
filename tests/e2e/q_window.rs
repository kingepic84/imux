//! Window-scoped options (setw), synchronize-panes in particular: what is typed into one pane goes
//! to every live pane of the window; a window without a value of its own follows the global option.
//! (The Rust version of tests/q_window.sh.)
use crate::support::*;
use std::time::Duration;

fn flags(e: &Env) -> String {
    e.all(&["list-windows", "-t", "m", "-F", "#{window_name}=#{window_flags}"])
}

fn until_flags(e: &Env, pattern: &str) -> bool {
    let re = regex_lite::Regex::new(&translate_pattern(pattern)).unwrap();
    wait_until(Duration::from_secs(10), || re.is_match(&flags(e).replace('\n', " ")))
}

fn file_has_line(e: &Env, rel: &str, line: &str) -> bool {
    wait_until(Duration::from_secs(10), || e.read(rel).lines().any(|l| l == line))
}

/// Wait until a client is attached to the server.
fn attached(e: &Env) -> bool {
    wait_until(Duration::from_secs(10), || !e.out(&["list-clients"]).is_empty())
}

#[test]
fn q_window() {
    let mut e = Env::new("qw");
    e.set("SHELL", "/bin/sh");
    let mut t = Check::new("q_window");

    e.session("a", "one");
    let a1 = e.out(&["list-panes", "-t", "a:one", "-F", "#{pane_id}"]).lines().next().unwrap_or("").to_string();
    let a2 = e.out(&["split-window", "-t", "a:one", "-d", "-P", "-F", "#{pane_id}"]);
    e.run(&["new-window", "-t", "a", "-d", "-n", "two"]);
    let b1 = e.out(&["list-panes", "-t", "a:two", "-F", "#{pane_id}"]).lines().next().unwrap_or("").to_string();
    let b2 = e.out(&["split-window", "-t", "a:two", "-d", "-P", "-F", "#{pane_id}"]);
    // every pane has its shell before anything is typed into it
    for pane in [&a1, &a2, &b1, &b2] {
        assert!(wait_until(Duration::from_secs(10), || !e.capture(pane).trim().is_empty()), "pane {pane} has no prompt");
    }

    t.section("off by default");
    t.ok("a pane is not synchronized", e.var(&a1, "#{pane_synchronized}") == "0");
    e.run(&["send-keys", "-t", &a1, "echo first-$((1+1))", "Enter"]);
    t.ok("typing reaches the pane it is sent to", e.shows(&a1, "^first-2$"));
    std::thread::sleep(Duration::from_millis(500));
    t.lacks("and no other", "^first-2$", &e.capture(&a2));

    t.section("setw on a window");
    let r = e.run(&["setw", "-t", "a:one", "synchronize-panes", "on"]);
    t.ok("setw -t succeeds", r.success());
    t.ok("the pane says so", e.var(&a1, "#{pane_synchronized}") == "1" && e.var(&a2, "#{window_synchronized}") == "1");
    e.run(&["send-keys", "-t", &a1, "echo second-$((1+2))", "Enter"]);
    t.ok("send-keys reaches the pane it names", e.shows(&a1, "^second-3$"));
    t.ok("and the other pane of the window", e.shows(&a2, "^second-3$"));
    e.run(&["send-keys", "-t", &b1, "echo other-$((2+2))", "Enter"]);
    t.ok("another window is not synchronized", e.shows(&b1, "^other-4$"));
    std::thread::sleep(Duration::from_millis(500));
    t.lacks("its other pane gets nothing", "^other-4$", &e.capture(&b2));
    t.lacks("and nothing from it reached the first window", "^other-4$", &e.capture(&a2));

    t.section("toggle and unset");
    e.run(&["setw", "-t", "a:one", "synchronize-panes"]);
    t.ok("no value flips it off", e.var(&a1, "#{pane_synchronized}") == "0");
    e.run(&["send-keys", "-t", &a1, "echo third-$((3+3))", "Enter"]);
    t.ok("off again, only the named pane gets the keys", e.shows(&a1, "^third-6$"));
    std::thread::sleep(Duration::from_millis(500));
    t.lacks("the other pane did not", "^third-6$", &e.capture(&a2));
    e.run(&["setw", "-t", "a:one", "synchronize-panes"]);
    t.ok("no value flips it on", e.var(&a1, "#{pane_synchronized}") == "1");
    e.run(&["setw", "-t", "a:one", "-u", "synchronize-panes"]);
    t.ok("-u gives the window the global option again", e.var(&a1, "#{pane_synchronized}") == "0");

    t.section("the global option and a window's own value");
    e.run(&["set", "-g", "synchronize-panes", "on"]);
    t.ok("set -g turns it on for every window without a value", e.var(&a1, "#{pane_synchronized}") == "1" && e.var(&b1, "#{pane_synchronized}") == "1");
    e.run(&["setw", "-t", "a:two", "synchronize-panes", "off"]);
    t.ok("a window's own off beats the global on", e.var(&b1, "#{pane_synchronized}") == "0" && e.var(&a1, "#{pane_synchronized}") == "1");
    e.run(&["setw", "-g", "synchronize-panes", "off"]);
    t.ok("setw -g is the global option", e.var(&a1, "#{pane_synchronized}") == "0");
    t.matches("and show-options shows it", "^synchronize-panes off$", &e.run(&["show-options", "synchronize-panes"]).all());

    t.section("mistakes");
    let r = e.run(&["setw", "-t", "a:one", "synchronize-panes", "maybe"]);
    t.ok("a bad value is refused", !r.success() && r.all().contains("expected on or off"));
    t.ok("a missing window is an error", !e.run(&["setw", "-t", "nosuch", "synchronize-panes", "on"]).success());

    t.section("a pane that is dead is skipped");
    e.run(&["set", "-g", "remain-on-exit", "on"]);
    e.run(&["setw", "-t", "a:one", "synchronize-panes", "off"]);
    e.run(&["send-keys", "-t", &a2, "exit", "Enter"]);
    t.ok("the pane died", wait_until(Duration::from_secs(10), || e.var(&a2, "#{pane_dead}") == "1"));
    e.run(&["setw", "-t", "a:one", "synchronize-panes", "on"]);
    e.run(&["send-keys", "-t", &a1, "echo fifth-$((5+5))", "Enter"]);
    t.ok("a dead pane does not stop the others", e.shows(&a1, "^fifth-10$"));
    t.ok("and send-keys to the live pane succeeds", e.run(&["send-keys", "-t", &a1, ""]).success());

    t.section("typed through a client");
    e.run(&["setw", "-t", "a:one", "synchronize-panes", "on"]);
    e.run(&["respawn-pane", "-k", "-t", &a2]);
    t.ok("the respawned pane has its shell", wait_until(Duration::from_secs(10), || !e.capture(&a2).trim().is_empty()));
    {
        let mut c = e.client(100, 30, &["attach", "-t", "a"]);
        assert!(attached(&e), "the client did not attach");
        c.send_str("echo typed-$((6+6))\r");
        t.ok("text typed at a client reaches the active pane", e.shows(&a1, "^typed-12$"));
        t.ok("and the other pane of a synchronized window", e.shows(&a2, "^typed-12$"));
        c.send(b"\x02d");
        c.wait_exit(Duration::from_secs(10));
    }

    t.section("a pane in copy mode is skipped");
    e.run(&["select-pane", "-t", &a1]);
    {
        let mut c = e.client(100, 30, &["attach", "-t", "a"]);
        assert!(attached(&e), "the client did not attach");
        let active = |e: &Env| e.out(&["display-message", "-p", "-t", "a:one", "#{pane_id}"]);
        c.send(b"\x02o");
        assert!(wait_until(Duration::from_secs(10), || active(&e) == a2), "prefix o did not select the other pane");
        c.send(b"\x02[");
        assert!(wait_until(Duration::from_secs(10), || e.var(&a2, "#{pane_in_mode}") == "1"), "copy mode did not start");
        c.send(b"\x02o");
        assert!(wait_until(Duration::from_secs(10), || active(&e) == a1), "prefix o did not come back");
        c.send_str("echo copyskip-$((7+7))\r");
        t.ok("the active pane gets what is typed", e.shows(&a1, "^copyskip-14$"));
        std::thread::sleep(Duration::from_millis(500));
        t.lacks("the pane in copy mode does not", "^copyskip-14$", &e.capture(&a2));
        c.send(b"\x02d");
        c.wait_exit(Duration::from_secs(10));
    }

    t.section("keys can be bound");
    let r = e.run(&["bind-key", "X", "setw", "synchronize-panes"]);
    t.ok("bind-key accepts setw", r.success());
    t.matches("list-keys shows it", "bind-key -T prefix X +set-window-option synchronize-panes", &e.run(&["list-keys"]).all());
    t.ok("another option cannot be bound", !e.run(&["bind-key", "Y", "wait-for", "ready"]).success());

    t.section("activity and bell flags");
    let eva = "alerts.out";
    let eva_path = p(&e.path(eva));
    e.session("m", "cur");
    e.run(&["new-window", "-t", "m", "-d", "-n", "bg"]);
    assert!(wait_until(Duration::from_secs(10), || !e.capture("m:bg").trim().is_empty()), "m:bg has no prompt");
    e.run(&["send-keys", "-t", "m:bg", "echo quiet-$((1+1))", "Enter"]);
    t.ok("monitors are off for activity by default", e.shows("m:bg", "^quiet-2$") && flags(&e).lines().any(|l| l == "bg="));
    e.run(&["set-hook", "-g", "alert-bell", &format!("run-shell \"echo bell:#{{q:hook_window_name}} >> {eva_path}\"")]);
    e.run(&["set-hook", "-g", "alert-activity", &format!("run-shell \"echo activity:#{{q:hook_window_name}} >> {eva_path}\"")]);
    e.run(&["set", "-g", "monitor-activity", "on"]);
    e.run(&["send-keys", "-t", "m:bg", "echo loud-$((2+2))", "Enter"]);
    t.ok("output in a background window flags it", until_flags(&e, "bg=# "));
    t.ok("the variable says so", e.var("m:bg", "#{window_activity_flag}#{window_bell_flag}") == "10");
    e.run(&["send-keys", "-t", "m:cur", "echo here-$((3+3))", "Enter"]);
    t.ok("with nobody attached the current window is not viewed either", until_flags(&e, "cur=#\\*"));
    t.ok("the hook fires with the window name", file_has_line(&e, eva, "activity:bg"));
    e.run(&["send-keys", "-t", "m:bg", "printf '\\007'", "Enter"]);
    t.ok("a bell flags the window too", until_flags(&e, "bg=#! "));
    t.ok("the bell variable", e.var("m:bg", "#{window_bell_flag}") == "1");
    t.ok("alert-bell fires", file_has_line(&e, eva, "bell:bg"));
    e.run(&["setw", "-t", "m:bg", "monitor-activity", "off"]);
    t.ok("switching a monitor off takes its flag away", until_flags(&e, "bg=! "));
    e.run(&["setw", "-t", "m:bg", "monitor-activity", "on"]);
    e.run(&["send-keys", "-t", "m:bg", "echo again-$((4+4))", "Enter"]);
    t.ok("and on flags again", until_flags(&e, "bg=#! "));
    e.run(&["setw", "-t", "m:bg", "monitor-bell", "off"]);
    t.ok("the bell monitor is per window as well", until_flags(&e, "bg=# "));
    e.run(&["setw", "-t", "m:bg", "monitor-bell", "on"]);
    e.run(&["send-keys", "-t", "m:bg", "printf '\\007'", "Enter"]);
    t.ok("bell on again", until_flags(&e, "bg=#! "));

    t.section("a client viewing a window clears it; M-n goes to the next alert");
    t.matches("list-keys shows the alert keys", "bind-key -T prefix M-n +next-window -a$", &e.run(&["list-keys"]).all());
    {
        let mut c = e.client(100, 30, &["attach", "-t", "m"]);
        assert!(attached(&e), "the client did not attach");
        t.ok("a viewed window loses its flag", until_flags(&e, "cur=\\* "));
        t.ok("while another keeps it", flags(&e).lines().any(|l| l == "bg=#!"));
        c.send(b"\x02\x1bn");
        t.ok("M-n selects the window with the alert and clears it", until_flags(&e, "bg=\\*"));
        t.ok("it is the current window now", e.out(&["list-windows", "-t", "m", "-F", "#{window_name}=#{window_active}"]).lines().any(|l| l == "bg=1"));
        c.send(b"\x02d");
        c.wait_exit(Duration::from_secs(10));
    }

    t.section("visual-bell and bell-action");
    e.run(&["set", "-g", "visual-bell", "on"]);
    e.run(&["set", "-g", "bell-action", "current"]);
    {
        let mut c = e.client(100, 30, &["attach", "-t", "m"]);
        assert!(attached(&e), "the client did not attach");
        std::thread::sleep(Duration::from_millis(500));
        e.run(&["send-keys", "-t", "m:cur", "printf '\\007'", "Enter"]);
        std::thread::sleep(Duration::from_millis(1500));
        t.ok("bell-action current: a bell elsewhere shows nothing", !c.text().contains("Bell in window"));
        c.send(b"\x02d");
        c.wait_exit(Duration::from_secs(10));
    }
    e.run(&["set", "-g", "bell-action", "any"]);
    {
        let mut c = e.client(100, 30, &["attach", "-t", "m"]);
        assert!(attached(&e), "the client did not attach");
        std::thread::sleep(Duration::from_millis(500));
        e.run(&["send-keys", "-t", "m:cur", "printf '\\007'", "Enter"]);
        t.ok("visual-bell shows a message with the window number", c.wait_for("Bell in window 0", Duration::from_secs(10)));
        c.send(b"\x02d");
        c.wait_exit(Duration::from_secs(10));
    }

    e.kill_server();
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
