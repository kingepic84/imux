//! Plugins (M39): link and its refusals, a plugin that follows events, permissions (a denied
//! request names the permission, a granted one works, a plugin cannot kill the server or follow an
//! event it did not list), a clean environment, restart with backoff and giving up, `restart
//! never`, a clean exit, enable and disable at run time, and stopping with the server (SIGTERM,
//! then SIGKILL for one that ignores it). Needs a debug build: IMUX_TEST_PLUGIN_BACKOFF_MS
//! shortens the delays. (The Rust version of tests/f_plugins.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn mk(e: &Env, name: &str, manifest: &str, body: &str) {
    let dir = e.path(&format!("src/{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plugin.conf"), format!("{manifest}\n")).unwrap();
    std::fs::write(dir.join("run.sh"), format!("#!/bin/sh\n{}\n", body.replace("@T@", &p(&e.dir)))).unwrap();
    std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(dir.join("plugin.conf"), std::fs::Permissions::from_mode(0o644)).unwrap();
}

fn runs(e: &Env, file: &str) -> usize {
    lines_of(&e.path(file))
}

#[test]
fn f_plugins() {
    let mut e = Env::new("fplug");
    let mut t = Check::new("f_plugins");
    e.set("IMUX_PLUGINS_DIR", &p(&e.path("linked")));
    e.set("IMUX_TEST_PLUGIN_BACKOFF_MS", "20");
    e.set("SECRET_TEST", "leaked");
    let d = p(&e.dir);
    let src = |n: &str| p(&e.path(&format!("src/{n}")));
    let w = |secs: u64, f: &dyn Fn() -> bool| wait_until(Duration::from_secs(secs), f);
    let list = || e.out(&["plugin", "list"]);

    t.section("link");
    mk(&e, "bad-api", "name bad-api\napi 2\nexec ./run.sh", "exit 0");
    mk(&e, "bad-perm", "name bad-perm\napi 1\nexec ./run.sh\npermissions shell", "exit 0");
    mk(&e, "bad-event", "name bad-event\napi 1\nexec ./run.sh\nevents no-such-event", "exit 0");
    mk(&e, "bad-exec", "name bad-exec\napi 1\nexec ../outside.sh", "exit 0");
    let out = |n: &str| e.run(&["plugin", "link", &src(n)]).all();
    t.ok("a manifest for another api is refused", out("bad-api").contains("needs api 2"));
    let o = out("bad-perm");
    t.ok("an unknown permission is refused and the known ones listed", o.contains("unknown permission") && o.contains("read-panes"));
    t.ok("an unknown event is refused", out("bad-event").contains("unknown event type"));
    t.ok("an exec outside the directory is refused", out("bad-exec").contains("inside the plugin directory"));
    t.ok("nothing was linked", std::fs::read_dir(e.path("linked")).map(|mut d| d.next().is_none()).unwrap_or(true));
    mk(&e, "writable", "name writable\napi 1\nexec ./run.sh", "exit 0");
    std::fs::set_permissions(e.path("src/writable/run.sh"), std::fs::Permissions::from_mode(0o666)).unwrap();
    t.ok("a program others can write is refused", out("writable").contains("can be written by others"));
    mk(&e, "watch", "name watch\napi 1\nexec ./run.sh\nevents window-linked", "\"$IMUX_EXE\" events --types window-linked | while read -r line; do echo \"$line\" >> @T@/watch.out; done");
    mk(
        &e,
        "nosend",
        "name nosend\napi 1\nexec ./run.sh\nevents window-linked",
        "echo \"plugin=$IMUX_PLUGIN secret=${SECRET_TEST:-none} socket=${IMUX_SOCKET_PATH:+set}\"\nuntil \"$IMUX_EXE\" list-sessions 2>/dev/null | grep -q '^a: '; do sleep 0.2; done\n\"$IMUX_EXE\" list-sessions\n\"$IMUX_EXE\" send-keys -t a: nope\n\"$IMUX_EXE\" new-window -d -t a -n sneaky\n\"$IMUX_EXE\" kill-server\n\"$IMUX_EXE\" events --types pane-died\n\"$IMUX_EXE\" run-shell 'touch @T@/ran-shell'\n\"$IMUX_EXE\" plugin-reload\necho finished\nsleep 3000",
    );
    mk(
        &e,
        "sender",
        "name sender\napi 1\nexec ./run.sh\npermissions send-keys windows",
        "sleep 1\n\"$IMUX_EXE\" send-keys -t a:0 'echo from-plugin > @T@/sender.out' Enter\n\"$IMUX_EXE\" new-window -d -t a -n by-plugin\nsleep 3000",
    );
    mk(&e, "crash", "name crash\napi 1\nexec ./run.sh", "echo x >> @T@/crash.count\necho boom >&2\nexit 1");
    mk(&e, "never", "name never\napi 1\nexec ./run.sh\nrestart never", "echo x >> @T@/never.count\nexit 1");
    mk(&e, "clean", "name clean\napi 1\nexec ./run.sh", "echo x >> @T@/clean.count\nexit 0");
    mk(&e, "sleeper", "name sleeper\napi 1\nexec ./run.sh", "echo x >> @T@/sleeper.count\nexec sleep 3333");
    mk(&e, "stubborn", "name stubborn\napi 1\nexec ./run.sh", "trap '' TERM\necho started >> @T@/stubborn.count\nsleep 4444 &\nwait");
    for n in ["watch", "nosend", "sender", "crash", "never", "clean", "stubborn"] {
        e.run(&["plugin", "link", &src(n)]);
        e.run(&["plugin", "enable", n]);
    }
    e.run(&["plugin", "link", &src("sleeper")]);
    let l = list();
    t.matches("a linked plugin is listed with its permissions", "(?m)^sender.*enabled.*permissions: send-keys windows", &l);
    t.matches("(and one that is not enabled says so)", "(?m)^sleeper.*disabled", &l);
    t.ok("linking again is harmless", e.run(&["plugin", "link", &src("sleeper")]).all().contains("linked already"));

    t.section("start with the server");
    e.run(&["new-session", "-d", "-s", "a", "-n", "first"]);
    t.ok("an enabled plugin runs", w(10, &|| pgrep_in("events --types window-linked", &e.dir)));
    t.ok("a plugin that is not enabled does not", !e.path("sleeper.count").exists());
    e.run(&["new-window", "-d", "-t", "a", "-n", "second"]);
    t.ok("a plugin follows the events it listed", w(10, &|| e.read("watch.out").contains("window-linked")));

    t.section("permissions");
    t.ok("a plugin that has run to its end", w(15, &|| e.out(&["plugin", "logs", "nosend"]).contains("finished")));
    let log = e.out(&["plugin", "logs", "nosend"]);
    t.ok("it has the plugin name, the socket and no secret from the server", log.contains("plugin=nosend secret=none socket=set"));
    t.ok("what only looks is allowed", log.lines().any(|l| l.starts_with("a: ")));
    t.ok("typing is denied and names the permission", log.contains("plugin nosend may not send-keys: it has no permission send-keys"));
    t.ok(
        "windows are denied and name the permission",
        log.contains("plugin nosend may not new-window: it has no permission windows") && !e.out(&["list-windows", "-t", "a", "-F", "#{window_name}"]).contains("sneaky"),
    );
    t.ok("killing the server is denied", log.contains("plugin nosend may not do that") && e.run(&["has-session", "-t", "a"]).code == 0);
    t.ok("an event it did not list is denied", log.contains("plugin nosend may not follow pane-died"));
    t.ok("a shell command is denied", log.contains("plugin nosend may not run run-shell") && !e.path("ran-shell").exists());
    t.ok("restarting plugins is denied", log.contains("plugin nosend may not run plugin-reload"));
    t.ok(
        "a plugin with the permissions can use them",
        w(15, &|| std::fs::metadata(e.path("sender.out")).map(|m| m.len() > 0).unwrap_or(false) && e.out(&["list-windows", "-t", "a", "-F", "#{window_name}"]).contains("by-plugin")),
    );
    let r = e.run(&["new-window", "-d", "-t", "a", "-n", "user-made"]);
    t.ok("the user can still do all of it", r.success() && e.out(&["list-windows", "-t", "a", "-F", "#{window_name}"]).contains("user-made"));

    t.section("restarts");
    t.ok("a plugin that fails is started again, five times, and then given up", w(15, &|| runs(&e, "crash.count") == 6));
    std::thread::sleep(Duration::from_secs(2));
    t.ok("it is not started a seventh time", runs(&e, "crash.count") == 6);
    t.matches("plugin list says it stopped and why", "(?m)^crash.*stopped: ended 6 times", &list());
    t.ok("what it printed is in its log", e.out(&["plugin", "logs", "crash"]).contains("boom"));
    t.ok("restart never means once", runs(&e, "never.count") == 1);
    t.ok("a plugin that ends cleanly is not started again", runs(&e, "clean.count") == 1);
    t.ok("the log of a stopped plugin says why in the server log", e.server_log().contains("plugin crash stopped for good"));

    t.section("enable and disable at run time");
    let en = e.run(&["plugin", "enable", "sleeper"]).all();
    t.ok("enable starts it", en.contains("started sleeper") && w(10, &|| pgrep_in("sleep 3333", &e.dir)));
    let dis = e.run(&["plugin", "disable", "sleeper"]).all();
    t.ok("disable stops it with its children", w(10, &|| !pgrep_in("sleep 3333", &e.dir)) && dis.contains("stopping sleeper"));
    t.matches("and it stays off", "(?m)^sleeper.*disabled", &list());
    e.run(&["plugin", "enable", "sleeper"]);
    t.ok("enable again starts it again", w(10, &|| runs(&e, "sleeper.count") == 2));
    e.run(&["plugin", "unlink", "sleeper"]);
    t.ok("unlink stops it and removes the link", w(10, &|| !pgrep_in("sleep 3333", &e.dir)) && !e.path("linked/sleeper").exists() && e.path("src/sleeper").is_dir());

    t.section("the server ends");
    t.ok("a plugin that ignores SIGTERM is running", w(10, &|| pgrep_in("sleep 4444", &e.dir)));
    t.ok("and so is the one that follows events", pgrep_in("events --types window-linked", &e.dir));
    let log = e.server_log();
    e.run_for(&["kill-server"], Duration::from_secs(20));
    t.ok(
        "the plugins end with the server, the stubborn one by SIGKILL",
        w(15, &|| !pgrep_in("sleep 4444", &e.dir) && !pgrep_in("events --types window-linked", &e.dir)),
    );
    let _ = (d, log);
    t.ok(
        "no errors in the server log but the ones about the crashing plugin",
        !e.server_log().lines().any(|l| l.contains("\"level\":\"error\"") && !l.contains("plugin crash")),
    );
    t.finish();
}
