//! The agent dock: it takes room from the panes and gives it back, and Enter in the dock jumps to
//! the agent while the dock stays. (The Rust version of tests/m17_dock.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(700);

#[test]
fn m17_dock() {
    let mut e = Env::new("m17");
    let conf = e.write("conf", "set autosave-interval 0\nset agent-panel dock\nset agent-dock-position left\nset agent-dock-width 30\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let agent = e.write("agent.sh", "printf '\\033]7501;state=working:app=fake:msg=%s\\033\\\\' \"$(printf '%s' 'dock-msg' | base64)\"\nsleep 40\n");
    let reader = e.write("reader.sh", "printf '\\033]7501;state=blocked:app=fake:kind=permission\\033\\\\'\nIFS= read -r line </dev/tty\nprintf '%s' \"$line\" >\"$1\"\nsleep 30\n");
    for f in [&agent, &reader] {
        std::fs::set_permissions(f, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let mut t = Check::new("m17_dock");

    t.section("the dock takes room from the panes and gives it back");
    let mut c = e.client(100, 30, &["new", "-s", "d"]);
    assert!(e.has_client());
    assert!(e.ready("d:", Duration::from_millis(600)));
    c.send_str("echo before-$(tput cols)\r");
    t.ok("panes start at the full width", c.wait_for("before-100", LONG));
    c.type_chunks(&["\x02c"], GAP);
    c.send_str(&format!("{}\r", p(&agent)));
    std::thread::sleep(Duration::from_secs(1));
    c.type_chunks(&["\x020", "\x02A"], GAP);
    t.ok("the dock lists the agent with its message", c.wait_seen("dock-msg", LONG));
    t.ok("the dock has a title", c.wait_seen("agents", LONG));
    c.type_chunks(&["wait", "\x1b"], GAP);
    c.send_str("echo mid-$(tput cols)\r");
    t.ok("with the dock open the panes are narrower", c.wait_for("mid-70", LONG));
    c.type_chunks(&["\x02A", "A"], GAP);
    c.send_str("echo after-$(tput cols)\r");
    t.ok("hiding the dock gives the width back", c.wait_for("after-100", LONG));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    std::thread::sleep(Duration::from_millis(500));

    t.section("Enter in the dock jumps to the agent and the dock stays");
    let got = e.path("got.txt");
    let mut c = e.client(100, 30, &["new", "-s", "e"]);
    assert!(e.has_client());
    assert!(e.ready("e:", Duration::from_millis(600)));
    c.type_chunks(&["\x02c"], GAP);
    c.send_str(&format!("{} {}\r", p(&reader), p(&got)));
    std::thread::sleep(Duration::from_secs(1));
    c.type_chunks(&["\x02c", "\x02A", "\r"], GAP);
    c.send_str("hello-from-the-dock\r");
    t.ok("Enter jumps to the first row and keys go to that pane", wait_until(LONG, || std::fs::read_to_string(&got).unwrap_or_default() == "hello-from-the-dock"));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
