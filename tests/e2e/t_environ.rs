//! The environment of new panes: the SSH agent variables a client brings, set-environment and
//! show-environment. (The Rust version of tests/t_environ.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);

#[test]
fn t_environ() {
    let mut e = Env::new("tev");
    e.unset("SHELL");
    e.set("SSH_AUTH_SOCK", "/sock/a");
    e.set("TESTVAR", "from-the-server");
    let mut t = Check::new("t_environ");

    let mut a = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("w:", Duration::from_millis(600)));
    a.send_str("echo up\r");
    t.matches("the first client's SSH_AUTH_SOCK is kept for new panes", "^SSH_AUTH_SOCK=/sock/a$", &e.all(&["show-environment", "SSH_AUTH_SOCK"]));

    let mut b = e.client_env(100, 30, &["attach", "-t", "w"], &[("SSH_AUTH_SOCK", "/sock/b")], &[]);
    t.ok("a client that attaches later updates it", wait_until(LONG, || e.out(&["show-environment", "SSH_AUTH_SOCK"]) == "SSH_AUTH_SOCK=/sock/b"));
    e.run(&["new-window", "-t", "w", "-n", "e"]);
    assert!(e.ready("w:e", Duration::from_millis(600)));
    e.run(&["send-keys", "-t", "w:e", "echo env-$SSH_AUTH_SOCK-", "Enter"]);
    t.ok("and a pane made afterwards has the new value", e.shows_line("w:e", "env-/sock/b-"));

    t.section("set-environment");
    e.run(&["set-environment", "MYVAR", "hello"]);
    e.run(&["set-environment", "-u", "TESTVAR"]);
    let env = e.all(&["show-environment"]);
    t.matches("set-environment shows in show-environment", "^MYVAR=hello$", &env);
    t.matches("-u shows as -NAME", "^-TESTVAR$", &env);
    e.run(&["new-window", "-t", "w", "-n", "f"]);
    assert!(e.ready("w:f", Duration::from_millis(600)));
    e.run(&["send-keys", "-t", "w:f", "echo my-$MYVAR-${TESTVAR:-gone}-", "Enter"]);
    t.ok("new panes get the variable and lose the unset one", e.shows_line("w:f", "my-hello-gone-"));
    e.run(&["set-environment", "-r", "TESTVAR"]);
    t.matches("-r forgets the entry, the server's own value shows again", "^TESTVAR=from-the-server$", &e.all(&["show-environment", "TESTVAR"]));
    let r = e.run(&["set-environment", "A=B", "x"]);
    t.ok("a bad name is an error", !r.success() && r.all().contains("bad variable name"));
    let r = e.run(&["show-environment", "NOPE_NOT_SET"]);
    t.ok("an unknown variable is an error", !r.success() && r.all().contains("unknown variable"));

    b.send(b"\x02d");
    b.wait_exit(LONG);
    e.kill_server();
    a.wait_exit(LONG);
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
