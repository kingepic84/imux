//! P10.3, spawning from a pane: the new pane's id comes back through -P -F, the focus stays, -c and
//! -e reach the new pane, a split that would leave a pane under pane-min-width or pane-min-height
//! is refused for a program in a pane (limit_reached, naming the options) and not for a split you
//! type yourself, and IMUX_ names are refused by -e. (The Rust version of tests/q_spawnmin.sh.)
use crate::support::*;
use std::time::Duration;

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn q_spawnmin() {
    let mut e = Env::new("qsm");
    let mut t = Check::new("q_spawnmin");
    let conf = e.write("imux.conf", "set -g status off\nset -g default-size 80x24\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    e.session("sm", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    hold(&e, "0");
    let active = || e.out(&["list-panes", "-F", "#{pane_id} #{pane_active}"]).lines().filter_map(|l| l.strip_suffix(" 1")).collect::<Vec<_>>().join(" ");
    let width = |pane: &str| e.var(pane, "#{pane_width}").parse::<i32>().unwrap_or(-1);

    t.section("a spawn from a pane");
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} split-window -h -P -F '#{{pane_id}}'"));
    t.ok("the id of the new pane comes back", rc == 0 && has_line(&o, "%1"));
    t.ok("the focus did not move", active() == "%0");
    t.ok("the maker is recorded", e.var("%1", "#{pane_spawned_by}") == "%0");
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} split-window -v -c /tmp -P -F '#{{pane_id}}'"));
    t.ok("a second spawn gets the next id", rc == 0 && has_line(&o, "%2"));
    t.ok("-c is where the new pane starts", wait_until(Duration::from_secs(10), || e.var("%2", "#{pane_current_path}").ends_with("tmp")));
    t.ok("the focus still did not move", active() == "%0");

    t.section("the minimum size");
    t.ok("the side by side panes are about half of 80 columns", (30..=45).contains(&width("%1")));
    assert!(e.ready("%1", Duration::from_millis(500)));
    hold(&e, "1");
    let (rc, o) = in_pane(&e, "1", &format!("{BIN} split-window -h -P -F '#{{pane_id}}'"));
    t.ok("a split that leaves under 20 columns is refused", rc != 0 && o.contains("pane-min-width"));
    t.ok("and says it is a limit, not a syntax problem", !o.contains("usage"));
    t.ok("no pane was made", e.out(&["list-panes", "-F", "#{pane_id}"]).lines().count() == 3);
    t.ok("the layout is as it was", width("%1") >= 30);
    let user = e.all(&["split-window", "-h", "-t", "%1", "-P", "-F", "#{pane_id}"]);
    t.matches("a split you type yourself has no such limit", "^%[0-9]*$", user.lines().next().unwrap_or(""));
    e.run(&["kill-pane", "-t", user.lines().next().unwrap_or("")]);
    std::thread::sleep(Duration::from_millis(500));
    e.run(&["set", "-g", "pane-min-width", "10"]);
    let (rc, o) = in_pane(&e, "1", &format!("{BIN} split-window -h -P -F '#{{pane_id}}'"));
    t.ok("with a lower pane-min-width the same split is allowed", rc == 0 && o.lines().any(|l| l.starts_with('%')));
    e.run(&["set", "-g", "pane-min-width", "20"]);
    e.run(&["set", "-g", "pane-min-height", "30"]);
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} split-window -v -P -F '#{{pane_id}}'"));
    t.ok("pane-min-height refuses a split that leaves too few rows", rc != 0 && o.contains("pane-min-height"));
    e.run(&["set", "-g", "pane-min-height", "5"]);
    t.ok("both options are options", e.out(&["show-options", "-gv", "pane-min-width"]) == "20" && e.out(&["show-options", "-gv", "pane-min-height"]) == "5");
    t.ok("and are bounded", !e.run(&["set", "-g", "pane-min-width", "0"]).success());

    t.section("-e from a pane");
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} new-window -d -e FOO=bar -P -F '#{{pane_id}}' \"sh -c 'echo ZZ\\$FOO; sleep 30'\""));
    let newp = o.lines().next().unwrap_or("").to_string();
    t.ok("a window with -e starts", rc == 0 && !newp.is_empty());
    t.ok("the variable is in its environment", e.shows(&newp, "^ZZbar"));
    t.ok("the window did not become the current one", e.out(&["display-message", "-p", "#{window_index}"]) == "0");
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} new-window -d -e IMUX_PANE_ID=9 -P -F '#{{pane_id}}'"));
    t.ok("-e never takes an IMUX_ name", rc != 0 && o.contains("IMUX_"));
    e.kill_server();
    t.finish();
}
