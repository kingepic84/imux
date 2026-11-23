//! Phase 7 (S4/S7): a client that speaks nonsense, floods or hoards connections must not hurt the
//! server or the other clients. (The Rust version of tests/s_socket.sh.)
use crate::support::*;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn s_socket() {
    let mut e = Env::new("ssock");
    let mut t = Check::new("s_socket");
    let conf = e.write("conf", "set autosave-interval 0\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let uid = unsafe { libc::getuid() };
    let sock: PathBuf = e.path(&format!("imux-{uid}/default"));
    let alive = || e.run_for(&["ls"], Duration::from_secs(10)).out.lines().any(|l| l.starts_with("s:"));
    let send = |bytes: &[u8]| {
        if let Ok(mut s) = UnixStream::connect(&sock) {
            let _ = s.write_all(bytes);
        }
    };
    // the server notes each connection that comes and goes: a step that sent something to it waits
    // for the connection to be over, so that what the server did with it has been done
    let gone = || e.server_log().matches(" disconnected\"}").count();
    let seen_connect = || e.server_log().matches(" connected\"}").count();
    let r = e.run(&["new-session", "-d", "-s", "s"]);
    assert!(r.success(), "cannot start a session: {}", r.all());
    t.ok("the server answers", alive());

    t.section("malformed frames");
    for (label, h) in [
        ("frame of length 0", "00000000"),
        ("frame of length 4 GiB", "ffffffff01"),
        ("unknown tag", "0000000163"),
        ("truncated hello payload", "00000002010a"),
        ("odd payload for a known tag", "00000005 02 41414141"),
        ("hello with a 65535 x 65535 terminal", "0000000901ffffffff00000000"),
    ] {
        let before = gone();
        send(&hex(h));
        assert!(wait_until(Duration::from_secs(10), || gone() > before), "the server never closed the connection of {label}");
        t.ok(label, alive());
    }
    t.ok("the huge size is refused and logged", wait_until(Duration::from_secs(5), || e.server_log().contains("terminal size too large")));
    let mut x: u64 = 0x2545F4914F6CDD1D;
    let junk: Vec<u8> = (0..65536)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    let before = gone();
    if let Ok(mut s) = UnixStream::connect(&sock) {
        for _ in 0..16 {
            if s.write_all(&junk).is_err() {
                break;
            }
        }
    }
    assert!(wait_until(Duration::from_secs(10), || gone() > before), "the server never closed the connection of the random bytes");
    t.ok("1 MiB of random bytes", alive());
    t.ok("protocol errors are logged", wait_until(Duration::from_secs(5), || e.server_log().contains("protocol error")));

    t.section("a frame that announces 16 MiB and stops");
    let sk = sock.clone();
    // the connection stays open, half a frame sent, until the test has looked at the server
    let (release, hold) = mpsc::channel::<()>();
    let before = seen_connect();
    let holder = std::thread::spawn(move || {
        if let Ok(mut s) = UnixStream::connect(&sk) {
            let _ = s.write_all(&(16u32 * 1024 * 1024).to_be_bytes());
            let _ = s.write_all(&vec![b'A'; 4 * 1024 * 1024]);
            let _ = hold.recv_timeout(Duration::from_secs(60));
        }
    });
    assert!(wait_until(Duration::from_secs(10), || seen_connect() > before), "the server never took the connection");
    t.ok("other clients are served meanwhile", alive());
    let _ = release.send(());
    let _ = holder.join();

    t.section("more connections than the limit");
    let sk = sock.clone();
    let (release, hold) = mpsc::channel::<()>();
    let many = std::thread::spawn(move || {
        let held: Vec<UnixStream> = (0..320).filter_map(|_| UnixStream::connect(&sk).ok()).collect();
        let _ = hold.recv_timeout(Duration::from_secs(60));
        drop(held);
    });
    t.ok(
        "connections past the limit were refused",
        wait_until(Duration::from_secs(10), || e.server_log().matches("connection refused: 256 clients").count() >= 1),
    );
    let _ = release.send(());
    let _ = many.join();
    t.ok("the server answers after the connections are gone", wait_until(Duration::from_secs(15), alive));

    t.section("a client that never reads");
    let sk = sock.clone();
    let (release, hold) = mpsc::channel::<()>();
    let silent = std::thread::spawn(move || {
        if let Ok(mut s) = UnixStream::connect(&sk) {
            let mut v = 1u32.to_be_bytes().to_vec();
            v.push(9);
            let _ = s.write_all(&v);
            let _ = hold.recv_timeout(Duration::from_secs(60));
        }
    });
    for i in 1..=200 {
        e.run(&["rename-window", "-t", "s", &format!("n{i}")]);
    }
    t.ok("renames go on while one client does not read", alive());
    let _ = release.send(());
    let _ = silent.join();
    e.kill_server();
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
