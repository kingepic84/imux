//! The command line as a remote control: looking (list-*, display-message, has-session), typing and
//! reading (send-keys, capture-pane), copy mode from outside, naming and selecting, what a program
//! in a pane may and may not do, panes and windows, command-from-panes.
//! (The Rust version of tests/t_remote.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

fn first_client(e: &Env, args: &[&str]) -> Client {
    let c = e.client(100, 30, args);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c
}

#[test]
fn t_remote() {
    let mut e = Env::new("trm");
    let conf = e.write("imux.conf", "set copy-command \"\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("t_remote");

    t.section("looking: list, display-message, has-session");
    let mut a = first_client(&e, &["new", "-s", "w"]);
    assert!(e.ready("w:", Duration::from_millis(600)));
    a.send_str("echo up\r");
    t.matches("list-sessions", r"^w: 1 window \(attached\)$", &e.all(&["list-sessions"]));
    t.matches("list-windows shows index, name, flags and pane count", r"^0: [a-z]+\* \(1 pane\)$", &e.all(&["list-windows"]));
    t.matches("list-panes shows the size, the id and the active one", r"^0: \[100x29\] %0 \(active\)$", &e.all(&["list-panes"]));
    t.matches("-F fills in variables", r"^%0 0 @[0-9]+ \$0$", &e.all(&["list-panes", "-F", "#{pane_id} #{pane_index} #{window_id} #{session_id}"]));
    t.matches("display-message -p prints for the session used last", r"^w:0\.0$", &e.all(&["display-message", "-p", "#{session_name}:#{window_index}.#{pane_index}"]));
    t.matches("a full target names the pane", r"^100x29$", &e.all(&["display-message", "-p", "-t", "w:0.0", "#{pane_width}x#{pane_height}"]));
    t.matches("a pane id is a target", r"^%0$", &e.all(&["display-message", "-p", "-t", "%0", "#{pane_id}"]));
    t.ok("has-session finds the session", e.run(&["has-session", "-t", "w"]).success());
    let r = e.run(&["has-session", "-t", "nosuch"]);
    t.ok("and fails for another name", !r.success() && regex_lite::Regex::new("can.t find session: nosuch").unwrap().is_match(&r.all()));
    t.matches("list-clients", r"^client-[0-9]+: w \[100x30\]$", &e.all(&["list-clients"]));
    let r = e.run(&["list-panes", "-t", "nosuch"]);
    t.ok("a target that does not exist is an error", !r.success() && r.all().contains("find"));

    t.section("typing and reading: send-keys, capture-pane");
    e.run(&["send-keys", "-t", "%0", "echo sent-$((1+2))", "Enter"]);
    t.ok("send-keys types text and named keys, capture-pane prints the screen", e.shows("%0", "^sent-3$"));
    t.ok("a range of one line is one line", e.out(&["capture-pane", "-p", "-S", "0", "-E", "0"]).lines().count() == 1);
    e.run(&["send-keys", "-t", "%0", "-l", "echo lit-$((3+3))"]);
    e.run(&["send-keys", "-t", "%0", "Enter"]);
    t.ok("-l types key names as text, a key name alone is the key", e.shows("%0", "^lit-6$"));
    e.run(&["send-keys", "-t", "%0", "-N", "3", "x"]);
    t.ok("-N repeats", wait_until(LONG, || e.capture("%0").contains("xxx")));
    e.run(&["send-keys", "-t", "%0", "C-u"]);

    t.section("copy mode from outside");
    e.run(&["copy-mode"]);
    t.ok("a command of the prompt runs for the attached client", wait_until(LONG, || e.out(&["display-message", "-p", "#{pane_in_mode}"]) == "1"));
    e.run(&["send-keys", "-X", "-t", "%0", "cancel"]);
    t.ok("send-keys -X runs a copy mode action", wait_until(LONG, || e.out(&["display-message", "-p", "#{pane_in_mode}"]) == "0"));

    t.section("naming and selecting");
    e.run(&["rename-window", "-t", "w:0", "renamed"]);
    t.matches("rename-window", "^renamed$", &e.all(&["list-windows", "-F", "#{window_name}"]));
    e.run(&["rename-session", "-t", "w", "other"]);
    t.matches("rename-session", "^other$", &e.all(&["list-sessions", "-F", "#{session_name}"]));
    t.ok("the old name is gone", !e.run(&["has-session", "-t", "w"]).success());
    t.ok("a unique prefix of a name finds the session", e.run(&["has-session", "-t", "oth"]).success());
    t.matches("a window can be named in a target", "^renamed$", &e.all(&["display-message", "-p", "-t", "other:renamed.0", "#{window_name}"]));
    let r = e.run(&["frobnicate"]);
    t.ok("an unknown command is the usage text and a failure", !r.success() && r.all().contains("usage: imux"));

    t.section("from inside a pane");
    e.run(&["set", "-g", "command-from-panes", "off"]);
    e.run(&["send-keys", "-t", "%0", "rm -f \"$TMPDIR/inside.txt\"; "]);
    e.run(&["send-keys", "-t", "%0", "$BIN send-keys -t %0 x > \"$TMPDIR/inside.txt\" 2>&1; $BIN list-windows -F \"inside-#{window_name}\" >> \"$TMPDIR/inside.txt\" 2>&1; $BIN capture-pane -p -t %0 >> \"$TMPDIR/inside.txt\" 2>&1", "Enter"]);
    assert!(wait_until(LONG, || { let t = e.read("inside.txt"); t.contains("inside-renamed") && t.contains("inside.txt") }), "the pane did not write all of its file");
    let inside = e.read("inside.txt");
    t.matches("typing into panes is refused to a program in a pane", "not allowed from inside a pane", &inside);
    t.matches("looking is allowed", "inside-renamed", &inside);
    t.matches("and so is reading its own pane", r"inside\.txt", &inside);
    a.send(b"\x02d");
    a.wait_exit(LONG);
    e.kill_server();

    t.section("panes: select-pane, kill-pane, select-window");
    {
        let mut c = first_client(&e, &["new", "-s", "w"]);
        assert!(e.ready("w:", Duration::from_millis(600)));
        c.send_str("echo up\r");
        c.send(b"\x02%");
        assert!(wait_until(LONG, || e.out(&["list-panes", "-t", "w:0"]).lines().count() == 2), "the split did not happen");
        c.send(b"\x02c");
        assert!(wait_until(LONG, || e.out(&["list-windows"]).lines().count() == 2), "the new window did not appear");
        c.send(b"\x02p");
        assert!(wait_until(LONG, || e.out(&["list-windows", "-F", "#{window_index}#{window_active}"]).lines().next() == Some("01")), "the client did not go back");
        t.matches("the split made a second pane, which is active", "^%11$", &e.all(&["list-panes", "-t", "w:0", "-F", "#{pane_id}#{pane_active}"]));
        e.run(&["select-pane", "-t", "w:0.%0"]);
        e.run(&["select-pane", "-t", "%0"]);
        t.matches("select-pane -t focuses a pane", "^%01$", &e.all(&["list-panes", "-t", "w:0", "-F", "#{pane_id}#{pane_active}"]));
        e.run(&["select-pane", "-t", "w:0", "-R"]);
        t.matches("select-pane -R moves to the pane on the right", "^%11$", &e.all(&["list-panes", "-t", "w:0", "-F", "#{pane_id}#{pane_active}"]));
        t.matches("the client moved to the first window again", "^01$", &e.all(&["list-windows", "-F", "#{window_index}#{window_active}"]));
        e.run(&["select-window", "-t", "w:1"]);
        t.matches("select-window -t selects", "^11$", &e.all(&["list-windows", "-F", "#{window_index}#{window_active}"]));
        e.run(&["select-window", "-l"]);
        t.matches("select-window -l goes back", "^01$", &e.all(&["list-windows", "-F", "#{window_index}#{window_active}"]));
        e.run(&["kill-pane", "-t", "%1"]);
        t.ok("kill-pane closes it", wait_until(LONG, || e.out(&["list-panes", "-t", "w:0", "-F", "#{pane_id}"]).lines().count() == 1));
        e.run(&["kill-window", "-t", "w:1"]);
        t.ok("kill-window closes a window", wait_until(LONG, || e.out(&["list-windows"]).lines().count() == 1));
        c.send(b"\x02d");
        c.wait_exit(LONG);
    }
    e.kill_server();

    t.section("command-from-panes on");
    e.write("imux.conf", "set copy-command \"\"\nset command-from-panes on\n");
    {
        let mut c = first_client(&e, &["new", "-s", "w"]);
        assert!(e.ready("w:", Duration::from_millis(600)));
        c.send_str("$BIN send-keys -t %0 \"echo viapane-$((2+2))\" Enter\r");
        t.ok("with the option on a program in a pane can type into panes", c.wait_seen("viapane-4", LONG));
        c.send(b"\x02d");
        c.wait_exit(LONG);
    }
    e.kill_server();
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
