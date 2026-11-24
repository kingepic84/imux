//! The reminder that `imux setup` has not been run: a box when a client attaches (Escape closes it,
//! it comes once per version, and not at all when the shell line is there, setup has run, the shell
//! is one setup has no line for, the file is a symlink, or `IMUX_NO_SETUP_HINT` is set), one line
//! when the client detaches, and a line in `imux doctor`.
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);
const TITLE: &str = "Finish setting up imux";

/// An environment whose shell is bash and whose home directory has no startup file yet.
fn fresh(name: &str) -> (Env, std::path::PathBuf) {
    let mut e = Env::new(name);
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    e.set("HOME", &p(&home));
    e.set("SHELL", "/bin/bash");
    for v in ["IMUX_NO_SETUP_HINT", "ZDOTDIR", "XDG_CONFIG_HOME"] {
        e.unset(v);
    }
    (e, home)
}

/// Attach a client, and say whether the box showed. A box that is open takes the keys, so the
/// command typed afterwards only reaches the shell when there is none.
fn shows_box(e: &Env) -> bool {
    let mut c = e.client(100, 30, &["new", "-s", "s"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    c.send_str("echo reached-$((20+22))\r");
    let reached = c.wait_screen("reached-42", Duration::from_secs(4));
    let shown = c.seen(TITLE);
    if shown {
        // the box has the keys: close it first (Escape then the prefix would be read as one key)
        c.send(b"\x1b");
        assert!(wait_until(LONG, || !c.screen_text().contains(TITLE)), "Escape did not close the box");
    }
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    shown && !reached
}

#[test]
fn q_setuphint() {
    let mut t = Check::new("q_setuphint");

    t.section("setup has not been run");
    let (e, home) = fresh("qsh1");
    let mut c = e.client(100, 30, &["new", "-s", "a"]);
    assert!(e.has_client(), "the client did not attach");
    t.ok("a box shows the command to run", c.wait_screen(TITLE, LONG) && c.seen("imux setup"));
    t.ok("and says which file gets the line", c.seen("\\.bashrc"));
    c.send(b"\x1b");
    t.ok("Escape closes it", wait_until(LONG, || !c.screen_text().contains(TITLE)));
    c.send_str("echo after-$((20+22))\r");
    t.ok("and the keys reach the pane again", c.wait_screen("after-42", LONG));
    t.ok("the version that showed it is kept", std::fs::read_to_string(e.path("state/setup-hint-seen")).map(|s| s.trim() == env!("CARGO_PKG_VERSION")).unwrap_or(false));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    t.ok("detaching says once more that setup is not finished", c.seen("imux: setup is not finished") && c.seen("run `imux setup`"));

    t.section("a second attach does not show it again");
    t.ok("no box the second time", !shows_box(&e));

    t.section("imux doctor");
    let r = e.run(&["doctor"]);
    t.matches("says setup is not finished and names the file", "(?m)^setup: not finished: ~/\\.bashrc has no imux init line", &r.out);
    let r = e.run(&["setup", "--no-man", "--no-skill"]);
    t.ok("setup adds the line", r.code == 0 && std::fs::read_to_string(home.join(".bashrc")).unwrap_or_default().contains("imux init"));
    t.ok("and leaves the marker that it has run", e.path("state/setup-done").exists());
    let r = e.run(&["doctor"]);
    t.matches("doctor then says so", "(?m)^setup: imux setup has been run", &r.out);

    t.section("the cases where nothing is shown");
    let (e, home) = fresh("qsh2");
    std::fs::write(home.join(".bashrc"), "eval \"$(imux init bash)\"\n").unwrap();
    t.ok("the line is in the startup file", !shows_box(&e));
    let (mut e, _) = fresh("qsh3");
    e.set("IMUX_NO_SETUP_HINT", "1");
    t.ok("IMUX_NO_SETUP_HINT turns it off", !shows_box(&e));
    let (e, _) = fresh("qsh4");
    t.ok("setup has run, also with --no-shell", e.run(&["setup", "--no-man", "--no-skill", "--no-shell"]).code == 0 && !shows_box(&e));
    let (mut e, _) = fresh("qsh5");
    e.set("SHELL", "/bin/sh");
    t.ok("a shell that imux init has no line for", !shows_box(&e));
    let (e, home) = fresh("qsh6");
    std::fs::write(home.join("dotfiles-bashrc"), "# mine\n").unwrap();
    std::os::unix::fs::symlink(home.join("dotfiles-bashrc"), home.join(".bashrc")).unwrap();
    t.ok("a startup file that is a symlink (setup would leave it alone)", !shows_box(&e));
    let (mut e, _) = fresh("qsh7");
    e.set("IMUX_NO_SETUP_HINT", "0");
    t.ok("0 does not turn it off", shows_box(&e));
    t.finish();
}
