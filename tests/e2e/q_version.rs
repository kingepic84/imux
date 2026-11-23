//! The protocol version check: a client and a server of different versions say so instead of
//! misbehaving, `kill-server` and `doctor` work either way, and an older server (one that hangs
//! up on the version message) is recognized. A debug build lets IMUX_TEST_PROTO_VERSION change the
//! version a process speaks. (The Rust version of tests/q_version.sh.)
use crate::support::*;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[test]
fn q_version() {
    let e = Env::new("qver");
    let mut t = Check::new("q_version");
    let v99 = [("IMUX_TEST_PROTO_VERSION", "99")];

    t.section("the same version");
    e.run(&["new-session", "-d", "-s", "v"]);
    let r = e.run(&["ls"]);
    t.ok("a request works", r.code == 0 && r.all().lines().any(|l| l.starts_with('v')));
    t.matches("doctor says the server speaks the same version", "(?m)^protocol: [0-9]+ \\(the server speaks it too\\)$", &e.all(&["doctor"]));
    e.kill_server();
    t.ok("the server is gone", wait_until(Duration::from_secs(10), || e.all(&["ls"]).contains("no server running")));

    t.section("a server of another version");
    t.ok("its own kind of client is served", e.run_env(&["new-session", "-d", "-s", "w"], &v99).code == 0);
    t.ok("a client of its version works", e.run_env(&["ls"], &v99).all().lines().any(|l| l.starts_with('w')));
    let start = Instant::now();
    let r = e.run(&["ls"]);
    let took = start.elapsed();
    t.ok("a client of another version is refused", r.code != 0);
    t.ok("with both versions named", r.all().contains("protocol 2 but the server speaks 99"));
    t.matches("the way out and the server's pid", "kill [0-9]+", &r.all());
    t.ok("(it names imux kill-server)", r.all().contains("imux kill-server"));
    t.ok("and quickly", took <= Duration::from_secs(3));
    let r = e.run(&["list-windows"]);
    t.ok("a command is refused too", r.code != 0 && r.all().contains("server speaks 99"));
    let r = e.run(&["events"]);
    t.ok("events is refused", r.code != 0 && r.all().contains("server speaks 99"));
    let mut c = e.client(100, 30, &["attach", "-t", "w"]);
    c.wait_exit(Duration::from_secs(10));
    t.matches("attach shows the refusal and leaves the terminal alone", "error: .*server speaks 99", &c.text());
    let d = e.all(&["doctor"]);
    t.matches("doctor reports the mismatch as a problem", "(?m)^protocol: 2 PROBLEM: this imux speaks protocol 2 but the server speaks 99", &d);
    t.matches("and still says a server runs", "(?m)^server: running", &d);

    t.section("kill-server works against any version");
    let r = e.run(&["kill-server"]);
    t.ok("it is not refused", r.code == 0 && !r.all().contains("protocol"));
    t.ok("and the server is gone", wait_until(Duration::from_secs(10), || e.all(&["ls"]).contains("no server running")));
    e.run(&["new-session", "-d", "-s", "again"]);
    t.ok("a new one at this version works", e.all(&["ls"]).lines().any(|l| l.starts_with("again")));
    e.kill_server();
    wait_until(Duration::from_secs(10), || e.all(&["ls"]).contains("no server running"));

    t.section("an older server");
    if which("python3").is_some() {
        let uid = unsafe { libc::getuid() };
        let sockdir = e.path(&format!("imux-{uid}"));
        std::fs::create_dir_all(&sockdir).unwrap();
        std::fs::set_permissions(&sockdir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let sock = sockdir.join("default");
        let script = format!("{}/tests/fixtures/old_server.py", env!("CARGO_MANIFEST_DIR"));
        let mut fake = std::process::Command::new("python3")
            .args(["-I", &script, &p(&sock), "4"])
            .stdout(Stdio::from(std::fs::File::create(e.path("fake.out")).unwrap()))
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_until(Duration::from_secs(5), || e.read("fake.out").contains("ready"));
        let r = e.run(&["ls"]);
        t.ok("a hang-up on the version is taken for an older server", r.code != 0 && r.all().contains("older imux") && r.all().contains("imux kill-server"));
        let mut c = e.client(100, 30, &["attach", "-t", "x"]);
        c.wait_exit(Duration::from_secs(10));
        t.ok("so does attach", c.text().contains("older imux"));
        t.matches("doctor says the server predates the check", "(?m)^protocol: .*PROBLEM: the server is older than the version check", &e.all(&["doctor"]));
        let _ = fake.kill();
        let _ = fake.wait();
        let _ = std::fs::remove_file(&sock);
    } else {
        println!("skip no python3: the older-server checks");
    }
    e.kill_server();
    t.ok(
        "no unexpected errors in the server log",
        !e.server_log().lines().any(|l| l.contains("\"level\":\"error\"") && !l.contains("protocol") && !l.contains("older")),
    );
    t.finish();
}
