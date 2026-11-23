//! Commands a key can run that have no typed form (send-keys, new-window -a/-d/-n, set, ...) and the
//! questions of `command-prompt` (template, -I, -1) and `confirm-before`: each is driven by a bound
//! key through a client on a pseudo-terminal. (The Rust version of tests/q_bindcmds.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn names(e: &Env) -> String {
    e.out(&["list-windows", "-F", "#{window_name}"]).lines().collect::<Vec<_>>().join(" ")
}

fn current(e: &Env) -> String {
    e.out(&["display-message", "-p", "#{window_name}"])
}

fn current_is(e: &Env, want: &str) -> bool {
    wait_until(LONG, || current(e) == want)
}

/// A client on `bnd` that runs the closure and goes away.
fn with_client(e: &Env, f: impl FnOnce(&mut Client)) -> String {
    let mut c = e.client(100, 30, &["attach", "-t", "bnd"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    f(&mut c);
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
    assert!(e.no_clients(), "the client did not go away");
    c.text()
}

fn lines_of_numbers(e: &Env) -> usize {
    e.out(&["capture-pane", "-p", "-S", "-1000", "-t", "%0"]).lines().filter(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit())).count()
}

#[test]
fn q_bindcmds() {
    let mut e = Env::new("qbc");
    let runkey = p(&e.path("runkey.out"));
    let conf = e.write(
        "imux.conf",
        &format!(
            "bind a command-prompt -p \"name:\" \"rename-window %%\"\n\
             bind i command-prompt -I pre -p \"x:\" \"rename-window %%\"\n\
             bind o command-prompt -1 -p \"one:\" \"rename-window k%%\"\n\
             bind y confirm-before -p \"sure?\" \"rename-window sure\"\n\
             bind w new-window -a -n after\n\
             bind b new-window -d -n bg\n\
             bind e set -g @marker yes\n\
             bind s send-keys \"echo ran-$((20+22))\" Enter\n\
             bind H clear-history\n\
             bind r run \"echo run-in-key > {runkey}\"\n"
        ),
    );
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_bindcmds");

    e.session("bnd", "start");
    let keys = e.all(&["list-keys"]);
    t.ok("the bindings loaded", keys.contains("confirm-before") && keys.contains("new-window -a"));
    assert!(e.ready("bnd:", Duration::from_millis(600)));

    let out = with_client(&e, |c| {
        c.send(b"\x02a");
        c.wait_seen("name:", LONG);
        c.send_str("typed\r");
        t.ok("what is typed replaces %% in the template", current_is(&e, "typed"));
    });
    t.ok("the label is drawn", out.contains("name:"));

    with_client(&e, |c| {
        c.send(b"\x02a");
        c.send_str("nope");
        c.send(b"\x1b");
        c.escape_pause();
        c.sync();
    });
    t.ok("Escape cancels and nothing runs", current(&e) == "typed");

    with_client(&e, |c| {
        c.send(b"\x02i");
        c.send(b"\r");
        t.ok("the initial text is what runs when Enter is pressed at once", current_is(&e, "pre"));
    });

    with_client(&e, |c| {
        c.send(b"\x02o");
        c.send(b"z");
        t.ok("-1 takes one key and runs without Enter", current_is(&e, "kz"));
    });

    with_client(&e, |c| {
        c.send(b"\x02y");
        c.send(b"n");
        c.sync();
    });
    t.ok("a confirmation answered with anything but y does nothing", current(&e) == "kz");

    let out = with_client(&e, |c| {
        c.send(b"\x02y");
        c.wait_seen("sure\\?", LONG);
        c.send(b"y");
        t.ok("a confirmation answered with y runs the command", current_is(&e, "sure"));
    });
    t.ok("the confirmation shows its label", out.contains("sure?"));

    e.run(&["select-window", "-t", ":0"]);
    with_client(&e, |c| {
        c.send(b"\x02w");
        wait_until(LONG, || names(&e).contains("after"));
    });
    t.ok("new-window -a puts the window after the current one", e.out(&["list-windows", "-F", "#{window_name}"]).lines().nth(1) == Some("after") && current(&e) == "after");

    let before = e.out(&["list-windows"]).lines().count();
    with_client(&e, |c| {
        c.send(b"\x02b");
        wait_until(LONG, || e.out(&["list-windows"]).lines().count() == before + 1);
    });
    t.ok("new-window -d makes a window and does not go to it", e.out(&["list-windows"]).lines().count() == before + 1 && current(&e) != "bg");

    with_client(&e, |c| {
        c.send(b"\x02e");
        wait_until(LONG, || e.out(&["show-options", "-gv", "@marker"]) == "yes");
    });
    t.ok("set -g in a binding sets the option", e.out(&["show-options", "-gv", "@marker"]) == "yes");

    with_client(&e, |c| {
        c.send(b"\x02s");
        wait_until(LONG, || e.out(&["capture-pane", "-p"]).contains("ran-42"));
    });
    t.ok("send-keys in a binding types into the pane", e.out(&["capture-pane", "-p"]).contains("ran-42"));

    with_client(&e, |c| {
        c.send(b"\x02r");
        wait_until(LONG, || e.read("runkey.out").trim() == "run-in-key");
    });
    t.ok("run in a binding is run-shell", e.read("runkey.out").trim() == "run-in-key");

    e.run(&["select-window", "-t", "bnd:0"]);
    e.run(&["send-keys", "-t", "%0", "C-u"]);
    e.run(&["send-keys", "-t", "%0", "-l", "seq 1 300"]);
    e.run(&["send-keys", "-t", "%0", "Enter"]);
    assert!(wait_until(LONG, || lines_of_numbers(&e) > 100), "the numbers did not arrive");
    t.ok("the scrollback holds the numbers", lines_of_numbers(&e) > 100);
    with_client(&e, |c| {
        c.send(b"\x02H");
        wait_until(LONG, || lines_of_numbers(&e) < 40);
    });
    let after = lines_of_numbers(&e);
    t.ok("clear-history empties the scrollback and keeps the screen", after < 40 && after > 0);
    t.finish();
}
