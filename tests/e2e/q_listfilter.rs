//! -f FILTER on list-sessions, list-windows, list-panes and list-clients: a line stays when the
//! filter format expands to something other than nothing or 0. (The Rust version of
//! tests/q_listfilter.sh.)
use crate::support::*;

#[test]
fn q_listfilter() {
    let e = Env::new("qlf");
    let mut t = Check::new("q_listfilter");
    e.session("alpha", "one");
    e.session("beta", "two");
    e.run(&["new-window", "-t", "alpha", "-n", "three"]);
    e.run(&["split-window", "-d", "-t", "alpha:one"]);
    let flat = |args: &[&str]| e.out(args).lines().map(|l| format!("{l} ")).collect::<String>();
    t.eq("no filter lists every session", flat(&["list-sessions", "-F", "#{session_name}"]), "alpha beta ".to_string());
    t.eq("list-sessions -f keeps the match", flat(&["list-sessions", "-f", "#{==:#{session_name},beta}", "-F", "#{session_name}"]), "beta ".to_string());
    t.ok("a filter of 0 keeps nothing", flat(&["list-sessions", "-f", "0", "-F", "#{session_name}"]).is_empty());
    t.ok("an empty filter keeps nothing", flat(&["list-sessions", "-f", "", "-F", "#{session_name}"]).is_empty());
    t.eq("list-windows -f with a glob", flat(&["list-windows", "-t", "alpha", "-f", "#{m:t*,#{window_name}}", "-F", "#{window_name}"]), "three ".to_string());
    t.eq("list-windows -a -f picks across sessions", flat(&["list-windows", "-a", "-f", "#{==:#{session_name},beta}", "-F", "#{window_name}"]), "two ".to_string());
    let active = flat(&["list-panes", "-t", "alpha:one", "-f", "#{pane_active}", "-F", "#{pane_index}"]);
    t.ok("list-panes -f keeps the active pane", active == "1 " || active == "0 ");
    let inactive = flat(&["list-panes", "-t", "alpha:one", "-f", "#{?pane_active,0,1}", "-F", "#{pane_index}"]);
    t.ok("the two filters split the panes", !inactive.is_empty() && !active.is_empty() && inactive != active);
    t.ok("list-panes -a -f", flat(&["list-panes", "-a", "-f", "#{==:#{window_name},two}", "-F", "#{pane_id}"]).split_whitespace().count() == 1);
    let (mut all, mut yes, mut no) = (String::new(), String::new(), String::new());
    e.with_client("beta", |_| {
        all = e.out(&["list-clients", "-F", "#{client_session}"]);
        yes = e.out(&["list-clients", "-f", "#{==:#{client_session},beta}", "-F", "#{client_session}"]);
        no = e.out(&["list-clients", "-f", "#{==:#{client_session},alpha}", "-F", "#{client_session}"]);
    });
    t.ok("list-clients sees the client", all.contains("beta"));
    t.ok("list-clients -f keeps the matching client", yes.contains("beta"));
    t.ok("list-clients -f drops the other", no.is_empty());
    e.kill_server();
    t.finish();
}
