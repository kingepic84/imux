//! `display-popup`: opening from a key, what the box shows, keys going to the program, -E closing
//! at the end of the program, a popup that stays until Escape, -EE and the status, the working
//! directory and variables, one popup per window, -C, the three-prefix escape hatch, a command on
//! the socket that waits for the popup and returns its status, a popup asked for by a program in
//! a pane (origin marker, popup-from-panes), and the plugin and parse errors.
//! (The Rust version of tests/q_popup.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

/// How many popups the server has closed so far, and how many programs of popups have ended and
/// been kept on the screen (the server notes both in its log). A step that must wait for a popup to
/// go, or for its program to end, waits for the count to grow.
fn closed(e: &Env) -> usize {
    e.server_log().lines().filter(|l| l.contains("\"popup ") && l.ends_with(" closed\"}")).count()
}

fn kept(e: &Env) -> usize {
    e.server_log().lines().filter(|l| l.contains("program ended with status") && l.contains(", kept")).count()
}

fn closed_after(e: &Env, before: usize) -> bool {
    wait_until(LONG, || closed(e) > before)
}

fn kept_after(e: &Env, before: usize) -> bool {
    wait_until(LONG, || kept(e) > before)
}

/// A client on session `pop`; the closure drives it; then it goes away (a popup that is still open
/// goes with its client).
fn with_client(e: &Env, f: impl FnOnce(&mut Client)) -> String {
    let mut c = e.client(100, 30, &["attach", "-t", "pop"]);
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

#[test]
fn q_popup() {
    let mut e = Env::new("qp");
    let d = p(&e.dir);
    let conf = e.write(
        "imux.conf",
        &format!(
            "bind p display-popup -E -T \"pop\" -w 40 -h 8 \"echo POPUP-OUT; sleep 30\"\n\
             bind c display-popup -E \"cat > {d}/typed\"\n\
             bind f display-popup -E \"echo FAST\"\n\
             bind s display-popup \"echo STAYED\"\n\
             bind x display-popup -E -E \"exit 3\"\n\
             bind y display-popup -E -E \"true\"\n\
             bind e display-popup -E -d /tmp -e FOO=bar \"sh -c 'env > {d}/env.out; pwd > {d}/pwd.out'\"\n\
             bind w display-popup -E -T tt \"sleep 30\"\n"
        ),
    );
    e.set("IMUX_CONFIG", &p(&conf));
    e.set("SHELL", "/bin/sh");
    let mut t = Check::new("q_popup");

    e.session("pop", "w");
    t.ok("the bindings with a popup loaded", e.all(&["list-keys"]).contains("display-popup"));
    assert!(e.ready("%0", Duration::from_millis(600)), "the pane never settled");

    // a popup shows its title and what its program printed
    let out = with_client(&e, |c| {
        c.send(b"\x02p");
        c.wait_seen("POPUP-OUT", LONG);
        // its program sleeps for 30 seconds; close the popup instead of waiting that long
        let n = closed(&e);
        e.run(&["display-popup", "-C"]);
        assert!(closed_after(&e, n), "the popup did not close");
    });
    t.ok("a popup shows its title and what its program printed", out.contains("pop") && out.contains("POPUP-OUT"));

    // typed keys go to the program of the popup
    with_client(&e, |c| {
        let n = closed(&e);
        c.send(b"\x02c");
        c.send(b"hello\r");
        c.send(b"\x04");
        assert!(closed_after(&e, n), "the popup did not close");
        c.send_str("echo ZZTYPED\r");
        t.ok("the end of the program closes a popup with -E and the keys go back to the pane", e.shows_line("%0", "ZZTYPED"));
    });
    t.ok("typed keys go to the program of the popup", e.read("typed").contains("hello"));
    t.ok("they do not reach the pane", !e.has_line("%0", "hello"));

    // a popup whose program is over at once is gone
    with_client(&e, |c| {
        let n = closed(&e);
        c.send(b"\x02f");
        assert!(closed_after(&e, n), "the popup did not close");
        c.send_str("echo ZZFAST\r");
        t.ok("a popup whose program is over at once is gone", e.shows_line("%0", "ZZFAST"));
    });

    // without -E the popup stays
    let out = with_client(&e, |c| {
        let (n, k) = (closed(&e), kept(&e));
        c.send(b"\x02s");
        c.wait_seen("STAYED", LONG);
        assert!(kept_after(&e, k), "the program of the popup did not end");
        c.send_str("echo ZZGONE\r");
        c.send(b"\x1b");
        c.escape_pause();
        assert!(closed_after(&e, n), "Escape did not close the popup");
        c.send_str("echo ZZBACK\r");
        t.ok("Escape closes it", e.shows_line("%0", "ZZBACK"));
    });
    t.ok("without -E the popup stays after its program, showing what it printed", out.contains("STAYED"));
    t.ok("a key that is not Escape is dropped while it stays", !e.has_line("%0", "ZZGONE"));

    // -E -E keeps a failed popup
    with_client(&e, |c| {
        let (n, k) = (closed(&e), kept(&e));
        c.send(b"\x02x");
        assert!(kept_after(&e, k), "the failed popup was not kept");
        c.send_str("echo ZZFAIL\r");
        c.send(b"\x1b");
        c.escape_pause();
        assert!(closed_after(&e, n), "Escape did not close the popup");
        c.send_str("echo ZZOK\r");
        t.ok("and Escape closes it", e.shows_line("%0", "ZZOK"));
    });
    t.ok("-E -E keeps a popup whose program failed", !e.has_line("%0", "ZZFAIL"));

    // -E -E closes a successful popup
    with_client(&e, |c| {
        let n = closed(&e);
        c.send(b"\x02y");
        assert!(closed_after(&e, n), "the popup did not close");
        c.send_str("echo ZZSUCCESS\r");
        t.ok("-E -E closes a popup whose program succeeded", e.shows_line("%0", "ZZSUCCESS"));
    });

    // -e and -d
    with_client(&e, |c| {
        let n = closed(&e);
        c.send(b"\x02e");
        assert!(closed_after(&e, n), "the popup did not close");
    });
    t.ok("-e is a variable of the program and -d its directory", e.read("env.out").lines().any(|l| l == "FOO=bar") && e.read("pwd.out").contains("tmp"));

    // a second popup, the three-prefix escape hatch
    let out = with_client(&e, |c| {
        let n = closed(&e);
        c.send(b"\x02w");
        assert!(c.wait_screen("┌", LONG), "the popup did not open");
        let r = e.run_for(&["display-popup", "-E", "echo SECONDX"], Duration::from_secs(8));
        e.write("second.rc", &format!("{}\n", r.code));
        c.send(b"\x02\x02");
        c.send_str("echo ZZSTILL\r");
        c.send(b"\x02\x02\x02");
        assert!(closed_after(&e, n), "three presses of the prefix did not close it");
        c.send_str("echo ZZCHORD\r");
        t.ok("three presses of the prefix close it whatever the program does", e.shows_line("%0", "ZZCHORD"));
    });
    t.ok("a second popup in the window is ignored and answered at once", e.read("second.rc").trim() == "0" && !out.contains("SECONDX"));
    t.ok("two presses of the prefix do not close it", !e.has_line("%0", "ZZSTILL"));

    // display-popup -C
    with_client(&e, |c| {
        let n = closed(&e);
        c.send(b"\x02w");
        assert!(c.wait_screen("┌", LONG), "the popup did not open");
        e.run(&["display-popup", "-C"]);
        assert!(closed_after(&e, n), "the popup did not close");
        c.send_str("echo ZZCLOSED\r");
        t.ok("display-popup -C closes it", e.shows_line("%0", "ZZCLOSED"));
    });

    // a command on the socket waits for the popup and returns its status
    with_client(&e, |_c| {
        let w7 = e.run_for(&["display-popup", "-E", "sh -c 'exit 7'"], Duration::from_secs(8));
        t.ok("a command on the socket waits and fails with the status of the program", w7.code == 1 && w7.all().contains("status 7"));
        let w0 = e.run_for(&["display-popup", "-E", "sleep 0.5"], Duration::from_secs(8));
        t.ok("and succeeds when the program does", w0.code == 0);
    });

    // a popup asked for by a program in a pane
    let out = with_client(&e, |c| {
        c.send_str(&format!("{BIN} display-popup -E -T tt 'sleep 1.5'; echo ZZPANEDONE\r"));
        c.wait_seen(r"\[from %0\]", LONG);
        t.ok("and the program that asked waited for it", e.shows_line("%0", "ZZPANEDONE"));
    });
    t.ok("a popup asked for by a program in a pane shows where it came from", out.contains("[from %0]"));

    // popup-from-panes off
    e.run(&["set", "-g", "popup-from-panes", "off"]);
    e.run(&["send-keys", "-t", "%0", "-l", &format!("{BIN} display-popup -E x 2>&1 | grep -c 'popup-from-panes' | sed 's/^/PFP:/'")]);
    e.run(&["send-keys", "-t", "%0", "Enter"]);
    let re = regex_lite::Regex::new("PFP:[1-9]").unwrap();
    t.ok("with popup-from-panes off a pane is refused and told which option", wait_until(Duration::from_secs(10), || re.is_match(&e.capture("%0"))));

    t.ok("an unknown border is an error", e.all(&["display-popup", "-b", "wavy", "x"]).contains("display-popup: -b is one of"));
    t.ok("a size of zero is an error", e.all(&["display-popup", "-w", "0", "x"]).contains("display-popup: -w"));
    t.ok("with no client attached it says so", e.all(&["display-popup", "x"]).contains("no client is attached"));
    t.finish();
}
