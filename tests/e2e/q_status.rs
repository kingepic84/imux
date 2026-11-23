//! `status off` and `status-position top|bottom`: where the bar is drawn, how many rows the panes
//! get, and that a click on a top bar's tab selects the window. (The Rust version of tests/q_status.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn height(e: &Env) -> String {
    e.all(&["display-message", "-p", "-t", "s:one", "#{pane_height}"]).trim_end().to_string()
}

fn until_height(e: &Env, want: &str) -> bool {
    wait_until(LONG, || height(e) == want)
}

fn active(e: &Env) -> String {
    e.all(&["list-windows", "-t", "s", "-F", "#{window_name}=#{window_active}"]).replace('\n', " ")
}

fn until_active(e: &Env, want: &str) -> bool {
    wait_until(LONG, || active(e).contains(want))
}

fn client(e: &Env) -> Client {
    let c = e.client(100, 30, &["attach", "-t", "s"]);
    assert!(e.has_client(), "the client did not attach");
    std::thread::sleep(Duration::from_millis(700));
    c
}

fn leave(e: &Env, mut c: Client) -> String {
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
    assert!(e.no_clients());
    c.text()
}

#[test]
fn q_status() {
    let e = Env::new("qst");
    let mut t = Check::new("q_status");

    t.section("a bar at the bottom is the default");
    e.session("s", "one");
    e.run(&["new-window", "-t", "s", "-d", "-n", "two"]);
    e.run(&["set", "-g", "status-left", "BAR"]);
    e.run(&["set", "-g", "status-right", ""]);
    e.run(&["set", "-g", "window-status-format", "#W"]);
    e.run(&["set", "-g", "window-status-current-format", "#W"]);
    t.ok("status-position shows its default", e.all(&["show-options", "status-position"]).lines().any(|l| l == "status-position bottom"));
    t.ok("status shows its default", e.all(&["show-options", "status"]).lines().any(|l| l == "status on"));
    let c = client(&e);
    t.ok("the panes get every row but the bar", until_height(&e, "29"));
    e.run(&["set", "-g", "status-position", "top"]);
    t.ok("a top bar takes the same one row", until_height(&e, "29"));
    std::thread::sleep(Duration::from_secs(1));
    e.run(&["set", "-g", "status", "off"]);
    t.ok("no bar gives the panes the whole screen", until_height(&e, "30"));
    e.run(&["set", "-g", "status", "on"]);
    t.ok("on again takes the row back", until_height(&e, "29"));
    e.run(&["set", "-g", "status", "3"]);
    t.ok("a row count is accepted as on", until_height(&e, "29"));
    e.run(&["set", "-g", "status", "on"]);
    let out = leave(&e, c);
    t.matches("a bar on top was drawn on the first row", "\x1b\\[1;1H(\x1b\\[[0-9;]*m)*BAR", &out);
    t.matches("and one at the bottom on the last", "\x1b\\[30;1H(\x1b\\[[0-9;]*m)*BAR", &out);

    t.section("a client that starts with the bar off never sees it");
    e.run(&["set", "-g", "status", "off"]);
    let c = client(&e);
    t.ok("its panes have all 30 rows", until_height(&e, "30"));
    std::thread::sleep(Duration::from_secs(1));
    let out = leave(&e, c);
    t.ok("the bar text is nowhere in its output", !out.contains("BAR"));
    e.run(&["set", "-g", "status", "on"]);

    t.section("a click on a tab of a top bar");
    e.run(&["set", "-g", "status-position", "top"]);
    e.run(&["select-window", "-t", "s:one"]);
    t.ok("window one is current", active(&e).contains("one=1"));
    let mut c = client(&e);
    c.send(b"\x1b[<0;9;1M\x1b[<0;9;1m");
    t.ok("clicking the second tab on the first row selects it", until_active(&e, "two=1"));
    leave(&e, c);
    e.run(&["select-window", "-t", "s:one"]);
    let mut c = client(&e);
    c.send(b"\x1b[<0;9;30M\x1b[<0;9;30m");
    std::thread::sleep(Duration::from_secs(1));
    t.ok("the same click on the last row (now a pane row) selects nothing", active(&e).contains("one=1"));
    leave(&e, c);

    t.section("the bar at the bottom still takes clicks there");
    e.run(&["set", "-g", "status-position", "bottom"]);
    e.run(&["select-window", "-t", "s:one"]);
    let mut c = client(&e);
    c.send(b"\x1b[<0;9;30M\x1b[<0;9;30m");
    t.ok("a click on the last row selects the second tab", until_active(&e, "two=1"));
    leave(&e, c);

    t.section("mistakes");
    let r = e.run(&["set", "-g", "status-position", "middle"]);
    t.ok("a bad position is refused", !r.success() && r.all().contains("status-position"));
    t.ok("and the option keeps its value", e.all(&["show-options", "status-position"]).lines().any(|l| l == "status-position bottom"));
    e.kill_server();
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
