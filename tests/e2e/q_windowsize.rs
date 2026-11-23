//! window-size smallest|largest|latest|manual with two attached clients of different sizes,
//! default-size for sessions nobody looks at, and new-session -x -y.
//! (The Rust version of tests/q_windowsize.sh.)
use crate::support::*;
use std::time::Duration;

fn width(e: &Env, target: &str) -> String {
    e.out(&["list-panes", "-t", target, "-F", "#{pane_width}"]).lines().next().unwrap_or("").to_string()
}

fn width_is(e: &Env, target: &str, want: &str) -> bool {
    wait_until(Duration::from_secs(15), || width(e, target) == want)
}

/// A new server with this configuration and a session `ws`.
fn start(e: &Env, conf: &str) {
    e.write("imux.conf", &format!("{conf}\n"));
    e.kill_server();
    e.session("ws", "w");
}

/// Two clients of different sizes attached to `ws`.
fn clients(e: &Env) -> (Client, Client) {
    let a = e.client(100, 30, &["attach", "-t", "ws"]);
    let b = e.client(60, 20, &["attach", "-t", "ws"]);
    assert!(wait_until(Duration::from_secs(15), || e.out(&["list-clients"]).lines().count() >= 2), "two clients did not attach");
    (a, b)
}

#[test]
fn q_windowsize() {
    let mut e = Env::new("qws");
    let conf = p(&e.path("imux.conf"));
    e.set("IMUX_CONFIG", &conf);
    let mut t = Check::new("q_windowsize");

    t.section("smallest (the default)");
    start(&e, "set -g @none 1");
    {
        let _c = clients(&e);
        t.ok("the window follows the smaller client", width_is(&e, "ws:", "60"));
    }

    t.section("largest");
    start(&e, "set -g window-size largest");
    {
        let _c = clients(&e);
        t.ok("the window follows the larger client", width_is(&e, "ws:", "100"));
    }

    t.section("latest");
    start(&e, "set -g window-size latest");
    {
        let (mut big, mut small) = clients(&e);
        small.send_str("y");
        t.ok("the client that typed last sets the size (the small one)", width_is(&e, "ws:", "60"));
        big.send_str("x");
        t.ok("and the other one when it types after", width_is(&e, "ws:", "100"));
    }

    t.section("manual and default-size");
    start(&e, "set -g window-size manual\nset -g default-size 90x30");
    {
        let _c = clients(&e);
        t.ok("manual ignores the clients", width_is(&e, "ws:", "90"));
        e.run(&["set", "-g", "window-size", "smallest"]);
        t.ok("changing the option applies at once", width_is(&e, "ws:", "60"));
    }

    t.section("sessions nobody looks at");
    start(&e, "set -g default-size 110x30");
    e.run(&["new-session", "-d", "-s", "two"]);
    e.run(&["new-session", "-d", "-s", "three", "-x", "120", "-y", "40"]);
    e.run(&["new-session", "-d", "-s", "four", "-x", "150"]);
    t.ok("default-size sizes a new detached session", width(&e, "two:") == "110");
    let height_three: i32 = e.out(&["list-panes", "-t", "three:", "-F", "#{pane_height}"]).lines().next().unwrap_or("0").parse().unwrap_or(0);
    t.ok("-x -y size it explicitly", width(&e, "three:") == "120" && height_three >= 38);
    t.ok("-x alone keeps the default height", width(&e, "four:") == "150");
    let bad = e.run(&["new-session", "-d", "-s", "five", "-x", "5"]);
    t.ok("a size out of range is refused", bad.all().contains("bad width") && !e.run(&["has-session", "-t", "five"]).success());
    let bad2 = e.run(&["new-session", "-d", "-s", "six", "-y", "abc"]);
    t.ok("and so is one that is no number", bad2.all().contains("bad height"));
    t.finish();
}
