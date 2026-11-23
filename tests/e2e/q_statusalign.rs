//! #[align=left|center|right] in status-format[N]: the three parts are drawn, in that order on the
//! row, and the directive itself is never shown. (The Rust version of tests/q_statusalign.sh.)
use crate::support::*;

#[test]
fn q_statusalign() {
    let e = Env::new("qsa");
    let mut t = Check::new("q_statusalign");
    e.session("sty", "one");
    e.run(&["set", "-g", "status-format[0]", "#[align=left]LEFTPART#[align=centre]MIDPART#[align=right]RIGHTPART"]);
    let out = e.with_client("sty", |c| {
        c.wait_seen("RIGHTPART", std::time::Duration::from_secs(10));
    });
    t.ok("the left part is drawn", out.contains("LEFTPART"));
    t.ok("the center part is drawn", out.contains("MIDPART"));
    t.ok("the right part is drawn", out.contains("RIGHTPART"));
    t.ok("the directive is not shown", !out.contains("align="));
    e.kill_server();
    t.finish();
}
