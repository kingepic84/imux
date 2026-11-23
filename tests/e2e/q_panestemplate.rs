//! display-panes [-d ms] template: the number chosen runs the template with %% the pane's id.
//! (The Rust version of tests/q_panestemplate.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_panestemplate() {
    let mut e = Env::new("qpt");
    let mut t = Check::new("q_panestemplate");
    let conf = e.write(
        "imux.conf",
        "bind Q display-panes \"set -g @picked %%\"\nbind W display-panes -d 3000 \"set -g @timed %%\"\nbind E display-panes\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    e.session("pt", "w");
    let first = e.var("pt:", "#{pane_id}");
    let _second = e.out(&["split-window", "-h", "-d", "-t", "pt:", "-P", "-F", "#{pane_id}"]);
    let _third = e.out(&["split-window", "-h", "-d", "-t", "pt:", "-P", "-F", "#{pane_id}"]);
    let active = || e.var("pt:", "#{pane_id}");
    let picked = |name: &str| e.out(&["show-options", "-gqv", name]);
    e.run(&["select-pane", "-t", &first]);
    e.with_client("pt", |c| {
        c.send(b"\x02Q");
        c.send(b"1");
        wait_until(Duration::from_secs(5), || !picked("@picked").is_empty());
    });
    let want = e.out(&["list-panes", "-t", "pt:", "-F", "#{pane_index} #{pane_id}"]).lines().find_map(|l| l.strip_prefix("1 ").map(String::from)).unwrap_or_default();
    t.ok("the chosen number runs the template with the pane id", !want.is_empty() && picked("@picked") == want);
    t.ok("and the pane is not selected", active() == first);
    e.with_client("pt", |c| {
        c.send(b"\x02W");
        c.send(b"2");
        wait_until(Duration::from_secs(5), || !picked("@timed").is_empty());
    });
    t.ok("-d with a template works", !picked("@timed").is_empty());
    e.with_client("pt", |c| {
        c.send(b"\x02E");
        c.send(b"2");
        wait_until(Duration::from_secs(5), || active() != first);
    });
    t.ok("without a template the pane is selected as before", active() != first);
    e.kill_server();
    t.finish();
}
