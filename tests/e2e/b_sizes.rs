//! Panes follow the smallest client that looks at their own session, not the smallest client of the
//! whole server: a 100-column client on session a is not squeezed by a 60-column client on b.
//! (The Rust version of tests/b_sizes.sh.)
use crate::support::*;
use std::time::Duration;

fn width_of(e: &Env, session: &str) -> String {
    for line in e.out(&["list-panes", "-a", "-F", "#{session_name} #{pane_width}"]).lines() {
        let mut it = line.split(' ');
        if it.next() == Some(session) {
            return it.next().unwrap_or("").to_string();
        }
    }
    String::new()
}

fn width_is(e: &Env, session: &str, want: &str) -> bool {
    wait_until(Duration::from_secs(15), || width_of(e, session) == want)
}

#[test]
fn b_sizes() {
    let e = Env::new("bsz");
    let mut t = Check::new("b_sizes");

    let mut a = e.client(100, 30, &["new", "-s", "a"]);
    assert!(wait_until(Duration::from_secs(15), || !width_of(&e, "a").is_empty()), "session a did not start");
    let mut b = e.client(60, 20, &["new", "-s", "b"]);
    assert!(wait_until(Duration::from_secs(15), || !width_of(&e, "b").is_empty()), "session b did not start");
    t.ok("session a keeps the width of its own client", width_is(&e, "a", "100"));
    t.ok("session b follows the narrower client that looks at it", width_is(&e, "b", "60"));
    e.run(&["new-session", "-d", "-s", "d"]);
    t.ok("a session nobody looks at stays 80 columns", width_is(&e, "d", "80"));

    t.section("client a switches to session b");
    a.send(b"\x02)");
    t.ok(
        "client a moved to session b",
        wait_until(Duration::from_secs(25), || e.out(&["list-clients", "-F", "#{client_session}"]).lines().filter(|l| *l == "b").count() == 2),
    );
    t.ok("both clients on b: the narrower one decides", width_of(&e, "b") == "60");
    t.ok("session a keeps its last size when nobody looks at it", width_of(&e, "a") == "100");
    t.ok("session d is untouched by the switch", width_of(&e, "d") == "80");

    t.section("the narrow client leaves");
    b.send(b"\x02d");
    b.wait_exit(Duration::from_secs(10));
    t.ok("b grows to the remaining client", width_is(&e, "b", "100"));
    e.kill_server();
    a.wait_exit(Duration::from_secs(10));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
