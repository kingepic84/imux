//! attach -r: a client that only watches. What it types and its mouse are dropped, the keys that
//! detach it still work, list-clients shows it; attach -x is accepted; a normal client still types.
//! (The Rust version of tests/q_readonly.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn total(e: &Env) -> usize {
    e.out(&["list-clients", "-F", "x"]).lines().filter(|l| l.contains('x')).count()
}

fn windows(e: &Env) -> usize {
    e.out(&["list-windows", "-t", "ws", "-F", "x"]).lines().filter(|l| l.contains('x')).count()
}

#[test]
fn q_readonly() {
    let e = Env::new("qro");
    let typed = e.path("typed");
    std::fs::write(&typed, "").unwrap();
    let mut t = Check::new("q_readonly");
    assert!(e.run(&["new-session", "-d", "-s", "ws", &format!("cat >{}", typed.display())]).success());
    std::thread::sleep(Duration::from_secs(1));

    t.section("a read-only client");
    let mut ro = e.client(100, 30, &["attach", "-r", "-t", "ws"]);
    t.ok("it is attached", wait_until(LONG, || total(&e) == 1));
    std::thread::sleep(Duration::from_millis(700));
    ro.send_str("hello\r");
    ro.send(b"\x1b[<0;5;5M\x1b[<0;5;5m");
    ro.send(b"\x02c");
    t.ok("list-clients says read-only in the default format", e.all(&["list-clients"]).contains("(read-only)"));
    t.ok("#{client_readonly} is 1", e.out(&["list-clients", "-F", "#{client_readonly}"]) == "1");
    t.ok("the filter can pick it", e.out(&["list-clients", "-f", "#{client_readonly}", "-F", "x"]).lines().filter(|l| l.contains('x')).count() == 1);
    std::thread::sleep(Duration::from_secs(2));
    t.ok("what it typed did not reach the pane", std::fs::read_to_string(&typed).unwrap_or_default().is_empty());
    t.ok("the prefix and c did not open a window", windows(&e) == 1);
    t.ok("it is still attached", total(&e) == 1);
    ro.send(b"\x02d");
    t.ok("the prefix and d detached it", wait_until(LONG, || total(&e) == 0));
    ro.wait_exit(LONG);

    t.section("a normal client");
    let mut rw = e.client(100, 30, &["attach", "-x", "-t", "ws"]);
    t.ok("attach -x is accepted", wait_until(LONG, || total(&e) == 1));
    std::thread::sleep(Duration::from_millis(700));
    t.ok("a normal client is not read-only", e.out(&["list-clients", "-F", "#{client_readonly}"]) == "0" && !e.all(&["list-clients"]).contains("(read-only)"));
    rw.send_str("world\r");
    t.ok("what it types reaches the pane", wait_until(LONG, || std::fs::read_to_string(&typed).unwrap_or_default().contains("world")));
    rw.send(b"\x02d");
    t.ok("and it detaches", wait_until(LONG, || total(&e) == 0));
    rw.wait_exit(LONG);

    t.section("flags");
    let bad = e.run(&["attach", "-t", "ws", "-q"]);
    t.ok("an unknown attach flag is still refused", !bad.success() && bad.all().contains("unexpected argument"));
    e.kill_server();
    t.finish();
}
