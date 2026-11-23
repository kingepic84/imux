//! A configuration file: bind, unbind, options, a bad option logged with its line, a chained
//! command, and the same commands typed at the prompt. (The Rust version of tests/m9_config.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn m9_config() {
    let mut e = Env::new("m9c");
    let mut t = Check::new("m9_config");
    let conf = e.write(
        "test.conf",
        "# test configuration\nset -g base-index 1\nset -g prefix C-a\nbind v split-window -h\nunbind %\nset -g bogus-option 1\nbind r source-file /nonexistent/imux.conf \\; display \"reload tried\"\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    let mut c = e.client(100, 30, &[]);
    assert!(c.wait_drawn(Duration::from_secs(15)), "the client drew nothing");
    // The keys are handled in order; what the client draws is only what is on the screen when a
    // frame is made, so each step is waited for until it has been drawn before the next one.
    let long = Duration::from_secs(15);
    c.type_chunks(&["\x01v", "echo bound-size-$(stty size | tr \" \" x)\r"], Duration::ZERO);
    c.wait_seen("bound-size-29x49", long);
    c.type_chunks(&["\x01%", "echo unbound-size-$(stty size | tr \" \" x)\r"], Duration::ZERO);
    c.wait_seen("unbound-size-29x49", long);
    c.type_chunks(&["\x01:", "set -g base-index 7\r", "\x01c"], Duration::ZERO);
    c.wait_seen("8:[a-z]", long);
    c.type_chunks(&["\x01r"], Duration::ZERO);
    c.wait_seen("source-file|reload tried|error\\(s\\)", long);
    c.type_chunks(&["\x01:", "set -g nonsense 1\r"], Duration::ZERO);
    c.wait_seen("unknown option: nonsense", long);
    let all = c.text();
    t.matches("config: bind v splits side by side", "bound-size-29x49", &all);
    t.matches("config: unbound % does nothing", "unbound-size-29x49", &all);
    t.matches("config: base-index 1 numbers the first tab 1", "1:[a-z]", &all);
    t.matches("prompt: set base-index 7 renumbers tabs", "8:[a-z]", &all);
    t.matches("config: bind with chained display", "source-file|reload tried|error\\(s\\)", &all);
    t.matches("prompt: unknown option shows a message", "unknown option: nonsense", &all);
    t.ok(
        "config: bad option logged with line number",
        wait_until(Duration::from_secs(5), || e.server_log().contains("test.conf:6: unknown option: bogus-option")),
    );
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    c.kill();
    e.kill_server();
    t.finish();
}
