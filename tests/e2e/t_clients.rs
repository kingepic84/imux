//! Clients: clock-mode, mark-pane and swap-pane, choose-client, suspend-client, each driven through
//! a real client on a pseudo-terminal. (The Rust version of tests/t_clients.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

fn attached(e: &Env) -> bool {
    wait_until(LONG, || !e.out(&["list-clients"]).is_empty())
}

/// A client on `new -s w`, attached and with the pane's shell ready.
fn new_client(e: &Env, args: &[&str]) -> Client {
    let c = e.client(100, 30, args);
    assert!(attached(e), "the client did not attach");
    let name = args.last().copied().unwrap_or("w");
    assert!(e.ready(&format!("{name}:"), Duration::from_millis(600)), "the pane never settled");
    c
}

fn detach(c: &mut Client) {
    c.send(b"\x02d");
    c.wait_exit(LONG);
}

fn fresh(e: &Env) {
    e.kill_server();
}

#[test]
fn t_clients() {
    let mut e = Env::new("tcl");
    let conf = e.write("imux.conf", "set clock-mode-color colour196\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("t_clients");

    t.section("clock-mode");
    {
        let mut c = new_client(&e, &["new", "-s", "w"]);
        c.send(b"\x02t");
        t.ok("the clock is drawn in the color of clock-mode-color", c.wait_for("48;5;196", LONG));
        c.send(b"x");
        c.send_str("echo clk-$((1+2))\r");
        t.ok("a key leaves the clock and is not typed into the pane", c.wait_for("clk-3", LONG));
        t.ok("the key that left the clock did not reach the shell", !c.text().contains("xecho clk"));
        detach(&mut c);
    }
    fresh(&e);

    t.section("mark pane and swap-pane");
    {
        let mut c = new_client(&e, &["new", "-s", "w"]);
        c.send(b"\x02m");
        t.ok("m marks the pane", c.wait_screen("marked pane %0", LONG));
        c.send(b"\x02%");
        assert!(wait_until(LONG, || e.out(&["list-panes", "-t", "w:"]).lines().count() == 2), "the split did not happen");
        assert!(e.ready("w:.1", Duration::from_millis(600)), "the new pane never settled");
        c.send_str("echo right-$(stty size | tr \" \" x)\r");
        t.ok("the new pane is on the right with 49 columns", c.wait_for("right-29x49", LONG));
        c.send(b"\x02:");
        c.send_str("swap-pane\r");
        assert!(wait_until(LONG, || e.out(&["display-message", "-p", "-t", "%0", "#{pane_left}"]) != "0"), "the panes did not swap");
        c.send_str("echo swapped-$(stty size | tr \" \" x)\r");
        t.ok("swap-pane puts the active pane where the marked one was", c.wait_for("swapped-29x50", LONG));
        c.send(b"\x02M");
        t.ok("M clears the mark", c.wait_screen("mark cleared", LONG));
        detach(&mut c);
    }
    fresh(&e);

    t.section("swap-pane without a mark");
    {
        let mut c = new_client(&e, &["new", "-s", "w"]);
        c.send(b"\x02%");
        assert!(wait_until(LONG, || e.out(&["list-panes", "-t", "w:"]).lines().count() == 2), "the split did not happen");
        c.send(b"\x02:");
        c.send_str("swap-pane\r");
        t.ok("it says there is no marked pane", c.wait_screen("no marked pane", LONG));
        detach(&mut c);
    }
    fresh(&e);

    t.section("choose-client");
    {
        let mut c = new_client(&e, &["new", "-s", "w"]);
        c.send(b"\x02D");
        t.ok("the list shows the client with its size and session, and marks the one asking", c.wait_for(r"client [0-9]+: 100x30 \[w\] \*", LONG));
        detach(&mut c);
    }
    fresh(&e);
    {
        let mut c = new_client(&e, &["new", "-s", "w"]);
        c.send(b"\x02D");
        assert!(c.wait_for(r"client [0-9]+: 100x30 \[w\] \*", LONG), "the list did not open");
        c.send(b"\r");
        t.ok("choosing a client detaches it", c.wait_for(r"\[detached\]", LONG));
        c.wait_exit(LONG);
    }
    fresh(&e);

    t.section("suspend-client");
    {
        let mut c = new_client(&e, &["new", "-s", "susp"]);
        c.send_str("echo before-s\r");
        assert!(c.wait_for("before-s", LONG));
        let pid = c.pid().expect("the client has a pid").to_string();
        c.send(b"\x02\x1a");
        let stopped = wait_until(Duration::from_secs(10), || e.sh(&format!("ps -o stat= -p {pid}")).out.contains('T'));
        if stopped {
            e.sh(&format!("kill -CONT {pid}"));
        }
        t.ok("the client stops like a job when it is told to suspend", stopped);
        c.send_str("echo after-$((1+2))\r");
        t.ok("after it continues the screen is drawn again and works", c.wait_for("after-3", LONG));
        detach(&mut c);
    }
    fresh(&e);

    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
