//! attach -d, new -A, new -A -D, and detach-client -s / -t / -a -t on the socket.
//! (The Rust version of tests/q_attachflags.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

fn count(e: &Env, session: &str) -> usize {
    e.out(&["list-clients", "-F", "#{client_session}"]).lines().filter(|l| *l == session).count()
}

fn total(e: &Env) -> usize {
    e.out(&["list-clients", "-F", "x"]).lines().filter(|l| l.contains('x')).count()
}

fn eventually(f: impl FnMut() -> bool) -> bool {
    wait_until(LONG, f)
}

#[test]
fn q_attachflags() {
    let e = Env::new("qaf");
    let mut t = Check::new("q_attachflags");
    e.session("ws", "one");
    e.session("other", "two");

    let mut a = e.client(100, 30, &["attach", "-t", "ws"]);
    t.ok("one client on ws", eventually(|| count(&e, "ws") == 1));
    let mut b = e.client(100, 30, &["attach", "-d", "-t", "ws"]);
    t.ok("attach -d leaves only the new client", eventually(|| count(&e, "ws") == 1 && total(&e) == 1));
    t.ok("the first client was told it is detached", a.wait_for("detached", LONG));
    a.wait_exit(LONG);
    let mut c = e.client(100, 30, &["new", "-A", "-s", "ws"]);
    t.ok("new -A -s of an existing name attaches", eventually(|| count(&e, "ws") == 2));
    t.ok("and makes no second session", e.out(&["list-sessions", "-F", "x"]).lines().count() == 2);
    let mut d = e.client(100, 30, &["new", "-A", "-D", "-s", "ws"]);
    t.ok("new -A -D attaches and detaches the others", eventually(|| count(&e, "ws") == 1 && total(&e) == 1));
    let mut f = e.client(100, 30, &["new", "-A", "-s", "fresh"]);
    t.ok("new -A -s of a new name creates it", eventually(|| e.run(&["has-session", "-t", "=fresh"]).success() && count(&e, "fresh") == 1));
    let bad = e.run(&["attach", "-t", "ws", "--bogus"]);
    t.ok("a flag the form does not know is refused", !bad.success() && bad.all().contains("unexpected argument"));
    for client in [&mut b, &mut c, &mut d, &mut f] {
        client.send(b"\x02d");
        client.wait_exit(LONG);
    }
    t.ok("the clients are gone when their terminals close", eventually(|| total(&e) == 0));

    let _f1 = e.client(100, 30, &["attach", "-t", "ws"]);
    let _g = e.client(100, 30, &["attach", "-t", "ws"]);
    let _h = e.client(100, 30, &["attach", "-t", "other"]);
    t.ok("three clients", eventually(|| total(&e) == 3));
    e.run(&["detach-client", "-s", "other"]);
    t.ok("detach-client -s detaches every client of that session only", eventually(|| count(&e, "other") == 0 && count(&e, "ws") == 2));
    let keep = e.out(&["list-clients", "-F", "#{client_name}"]).lines().next().unwrap_or("").to_string();
    e.run(&["detach-client", "-a", "-t", &keep]);
    t.ok("detach-client -a -t keeps just that client", eventually(|| total(&e) == 1 && e.out(&["list-clients", "-F", "#{client_name}"]) == keep));
    e.run(&["detach-client", "-t", &keep]);
    t.ok("detach-client -t detaches the named client", eventually(|| total(&e) == 0));
    let bad = e.run(&["detach-client", "-t", "client-9999"]);
    t.ok("an unknown client is an error", !bad.success() && regex_lite::Regex::new("can.t find client").unwrap().is_match(&bad.all()));
    e.kill_server();
    t.finish();
}
