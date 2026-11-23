//! Named servers: -L name, -S path and $IMUX_SOCKET_NAME give separate servers with their own
//! sockets, logs and state; a pane talks to its own server; the tmux shim honors -L.
//! (The Rust version of tests/q_namedserver.sh.)
use crate::support::*;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::time::Duration;

#[test]
fn q_namedserver() {
    let mut e = Env::new("qns");
    let mut t = Check::new("q_namedserver");
    let conf = e.write("imux.conf", "set autosave-interval 1\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for v in ["IMUX_SOCKET_PATH", "IMUX_SOCKET_NAME", "IMUX_SERVER_SOCKET"] {
        e.unset(v);
    }
    let d = p(&e.dir);
    let uid = unsafe { libc::getuid() };
    let sd = e.path(&format!("imux-{uid}"));
    let ls = |args: &[&str]| e.out(args);
    let has = |out: &str, name: &str| out.lines().any(|l| l.starts_with(&format!("{name}:")));
    let w = |secs: u64, f: &dyn Fn() -> bool| wait_until(Duration::from_secs(secs), f);
    e.run(&["new-session", "-d", "-s", "d", &format!("{BIN} ls >{d}/d.ls 2>&1; sleep 3; tmux -L two list-sessions >{d}/d.shim 2>&1; sleep 30")]);
    e.run(&["-L", "two", "new-session", "-d", "-s", "t", &format!("{BIN} ls >{d}/t.ls 2>&1; sleep 30")]);
    let (def, two) = (ls(&["ls"]), ls(&["-L", "two", "ls"]));
    t.ok("the default server lists its own session only", has(&def, "d") && !has(&def, "t"));
    t.ok("the named server lists its own session only", has(&two, "t") && !has(&two, "d"));
    let is_sock = |n: &str| std::fs::metadata(sd.join(n)).map(|m| m.file_type().is_socket()).unwrap_or(false);
    t.ok("two sockets", is_sock("default") && is_sock("two"));
    let nonempty = |p: std::path::PathBuf| std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false);
    t.ok("the named server has its own log", nonempty(sd.join("server-two.log")) && nonempty(sd.join("server.log")));
    w(15, &|| !e.read("t.ls").is_empty() && !e.read("d.ls").is_empty() && !e.read("d.shim").is_empty());
    let (tls, dls) = (e.read("t.ls"), e.read("d.ls"));
    t.ok("a pane of the named server reaches its own server", has(&tls, "t") && !has(&tls, "d"));
    t.ok("a pane of the default server reaches the default server", has(&dls, "d") && !has(&dls, "t"));
    t.ok("tmux -L from a pane reaches the named server", has(&e.read("d.shim"), "t"));
    w(15, &|| nonempty(e.path("state/two/autosave.imux")) && nonempty(e.path("state/autosave.imux")));
    t.ok("each server has its own state", nonempty(e.path("state/two/autosave.imux")) && nonempty(e.path("state/autosave.imux")));
    let a2 = e.read("state/two/autosave.imux");
    t.ok("and the named one's autosave is its own", a2.lines().any(|l| l.starts_with("session t ")) && !a2.lines().any(|l| l.starts_with("session d ")));
    t.ok("the name comes from the environment too", has(&e.run_env(&["ls"], &[("IMUX_SOCKET_NAME", "two")]).out, "t"));
    t.ok("and a flag beats it", has(&e.run_env(&["-L", "default", "ls"], &[("IMUX_SOCKET_NAME", "two")]).out, "d"));
    t.ok("-S names a socket by path", has(&ls(&["-S", &p(&sd.join("two")), "ls"]), "t"));
    let r = e.run(&["-L", "a/b", "ls"]);
    t.ok("a bad name is refused with status 2", r.code == 2 && r.all().contains("server name"));
    t.ok("a flag with no value is refused", e.run(&["-L"]).code == 2);
    let priv_dir = e.path("priv");
    std::fs::create_dir_all(&priv_dir).unwrap();
    std::fs::set_permissions(&priv_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let sock = p(&priv_dir.join("sock"));
    e.run(&["-S", &sock, "new-session", "-d", "-s", "p", "sleep 30"]);
    t.ok("-S starts a server at a private path", priv_dir.join("sock").exists() && has(&ls(&["-S", &sock, "ls"]), "p"));
    t.ok("with its own state", w(15, &|| nonempty(e.path("state/sock/autosave.imux"))));
    e.run(&["-S", &sock, "kill-server"]);
    e.run(&["-L", "two", "kill-server"]);
    t.ok("kill-server -L ends only that server", has(&ls(&["ls"]), "d") && !e.run(&["-L", "two", "ls"]).success());
    t.ok("and removes its autosave only", !e.path("state/two/autosave.imux").exists() && nonempty(e.path("state/autosave.imux")));
    e.kill_server();
    t.finish();
}
