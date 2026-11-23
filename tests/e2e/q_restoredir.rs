//! A pane restored into the home directory because its saved one is gone keeps the saved path in
//! the next autosaves until the pane moves, says so once, and shows it in
//! #{pane_restore_missing}. (The Rust version of tests/q_restoredir.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_restoredir() {
    let mut e = Env::new("qrd");
    let mut t = Check::new("q_restoredir");
    let conf = e.write("imux.conf", "set autosave-interval 1\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    e.set("HOME", &p(&home));
    let home_real = p(&std::fs::canonicalize(&home).unwrap());
    let work = e.path("work");
    std::fs::create_dir_all(&work).unwrap();
    let w = |secs: u64, f: &dyn Fn() -> bool| wait_until(Duration::from_secs(secs), f);
    e.run(&["new-session", "-d", "-s", "r", "-c", &p(&work)]);
    w(15, &|| e.read("state/autosave.imux").contains("/work"));
    t.ok("the autosave names the working directory", e.read("state/autosave.imux").contains("/work"));
    let spid: i32 = e.out(&["display-message", "-p", "#{pid}"]).parse().unwrap();
    unsafe { libc::kill(spid, libc::SIGTERM) };
    w(15, &|| e.all(&["ls"]).contains("no server running"));
    std::fs::remove_dir_all(&work).unwrap();
    let mut c = e.client(100, 30, &["new", "-s", "r"]);
    t.ok("the session came back", w(15, &|| e.all(&["ls"]).lines().any(|l| l.starts_with("r:"))));
    let missing = || e.out(&["list-panes", "-t", "r:", "-F", "#{pane_restore_missing}"]).lines().next().unwrap_or("").to_string();
    t.eq("the pane is flagged", missing(), "1".to_string());
    t.ok("the pane started in the home directory", w(15, &|| e.var("r:", "#{pane_current_path}") == home_real));
    t.matches(
        "a message said the directory is gone",
        "client [0-9]*: restore: .*/work is gone, the pane started in the home directory",
        &e.server_log(),
    );
    e.run(&["save"]);
    t.ok("the next autosave still has the saved path", e.read("state/autosave.imux").contains("/work"));
    t.lacks("and not the home directory in its place", "(?m)^pane .*/home", &e.read("state/autosave.imux"));
    e.run(&["send-keys", "-t", "r:", "cd /tmp", "Enter"]);
    w(15, &|| e.var("r:", "#{pane_current_path}") != home_real);
    e.run(&["save"]);
    t.ok("once the pane moved the saved path is gone from the autosave", !e.read("state/autosave.imux").contains("/work"));
    t.eq("and the flag is off", missing(), "0".to_string());
    c.kill();
    e.kill_server();
    t.finish();
}
