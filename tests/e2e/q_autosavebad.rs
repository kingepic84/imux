//! A server that starts on an autosave which does not read back sets it aside, so the first save
//! does not push the good previous copy out; the previous copy is what a restore uses.
//! (The Rust version of tests/q_autosavebad.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_autosavebad() {
    let mut e = Env::new("qasb");
    let mut t = Check::new("q_autosavebad");
    let conf = e.write("imux.conf", "set autosave-interval 1\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let nonempty = |name: &str| std::fs::metadata(e.path(&format!("state/{name}"))).map(|m| m.len() > 0).unwrap_or(false);
    e.session("first", "w");
    e.run(&["new-window", "-t", "first:", "-n", "a"]);
    wait_until(Duration::from_secs(15), || nonempty("autosave.imux"));
    e.run(&["new-window", "-t", "first:", "-n", "b"]);
    wait_until(Duration::from_secs(15), || nonempty("autosave.prev"));
    t.ok("a good autosave and previous copy exist", nonempty("autosave.imux") && nonempty("autosave.prev"));
    let spid: i32 = e.out(&["display-message", "-p", "#{pid}"]).parse().unwrap();
    unsafe { libc::kill(spid, libc::SIGTERM) };
    wait_until(Duration::from_secs(15), || e.all(&["ls"]).contains("no server running"));
    t.ok("the autosave stays after SIGTERM", nonempty("autosave.imux"));
    let prev_before = std::fs::read(e.path("state/autosave.prev")).unwrap();
    std::fs::write(e.path("state/autosave.imux"), "imux-snapshot 1\nsav").unwrap();
    e.run(&["new-session", "-d", "-s", "second"]);
    let bad = || -> Vec<std::path::PathBuf> {
        std::fs::read_dir(e.path("state")).map(|d| d.filter_map(|f| f.ok()).map(|f| f.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("autosave.bad.")).collect()).unwrap_or_default()
    };
    t.ok("the damaged file was kept aside", wait_until(Duration::from_secs(15), || !bad().is_empty()));
    t.ok(
        "with its bytes",
        bad().iter().any(|p| {
            let s = std::fs::read_to_string(p).unwrap_or_default();
            s.lines().any(|l| l == "sav") || s.contains("imux-snapshot 1")
        }),
    );
    t.ok("the good previous copy is untouched", std::fs::read(e.path("state/autosave.prev")).unwrap() == prev_before);
    e.kill_server();
    t.finish();
}
