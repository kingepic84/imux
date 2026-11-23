//! The example plugins under examples/plugins (M40), linked and run for real: notify-blocked (a
//! fake osascript records the notification, hostile text stays data), worktree-names (a window
//! that holds a worktree gets the worktree's name back after it was renamed), git-branch (sets the
//! status segment once; branch.sh prints a branch or nothing). Needs a debug build.
//! (The Rust version of tests/f_examples.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn f_examples() {
    let mut e = Env::new("fex");
    let mut t = Check::new("f_examples");
    let src = format!("{}/examples/plugins", env!("CARGO_MANIFEST_DIR"));
    let d = p(&e.dir);
    e.set("IMUX_PLUGINS_DIR", &p(&e.path("linked")));
    let conf = e.write("conf", "set autosave-interval 0\nset worktree-base main\nset worktree-name-pattern \"[A-Z]+-[0-9]+\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@example.com"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@example.com"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_SYSTEM", "/dev/null")] {
        e.set(k, v);
    }
    std::fs::create_dir_all(e.path("bin")).unwrap();
    let osa = e.write("bin/osascript", &format!("#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >>\"{d}/osascript.log\"\nprintf -- '--end--\\n' >>\"{d}/osascript.log\"\n"));
    std::fs::set_permissions(&osa, std::fs::Permissions::from_mode(0o755)).unwrap();
    e.set("PATH", &format!("{d}/bin:{}", std::env::var("PATH").unwrap_or_default()));
    let w = |secs: u64, f: &dyn Fn() -> bool| wait_until(Duration::from_secs(secs), f);
    let names = || e.out(&["list-windows", "-t", "a", "-F", "#{window_name}"]);

    t.section("link");
    for n in ["notify-blocked", "worktree-names", "git-branch"] {
        let r = e.run(&["plugin", "link", &format!("{src}/{n}")]);
        t.ok(&format!("{n} links"), r.all().contains("linked"));
        e.run(&["plugin", "enable", n]);
    }
    let l = e.out(&["plugin", "list"]);
    t.matches("the permissions are the ones the manifests ask for (windows)", "(?m)^worktree-names.*permissions: windows", &l);
    t.matches("the permissions are the ones the manifests ask for (set-options)", "(?m)^git-branch.*permissions: set-options", &l);

    t.section("repository");
    let work = e.path("work");
    std::fs::create_dir_all(&work).unwrap();
    assert!(git(&work, &["init", "-q", "--bare", "-b", "main", &p(&e.path("origin.git"))]));
    assert!(git(&work, &["clone", "-q", &p(&e.path("origin.git")), "proj"]));
    let proj = work.join("proj");
    assert!(git(&proj, &["checkout", "-q", "-b", "main"]));
    std::fs::write(proj.join("a.txt"), "one\n").unwrap();
    assert!(git(&proj, &["add", "a.txt"]));
    assert!(git(&proj, &["commit", "-q", "-m", "one"]));
    assert!(git(&proj, &["push", "-q", "origin", "main"]));
    e.run(&["new-session", "-d", "-s", "a", "-n", "first"]);
    t.ok(
        "the server and the plugins are up",
        w(10, &|| pgrep_in("events --types agent-state-changed", &e.dir) && pgrep_in("events --types window-linked,window-renamed", &e.dir)),
    );

    t.section("git-branch");
    t.ok("it set the status segment", w(10, &|| e.out(&["show-options", "-g", "status-right"]).contains("branch.sh")));
    t.ok("with the directory quoted by imux", e.out(&["show-options", "-g", "status-right"]).contains("#{q:pane_current_path}"));
    let branch = |dir: &std::path::Path| String::from_utf8_lossy(&std::process::Command::new(format!("{src}/git-branch/branch.sh")).arg(dir).output().unwrap().stdout).trim().to_string();
    t.ok("branch.sh prints the branch", branch(&proj) == "main");
    t.ok("and nothing outside a repository", branch(&e.dir).is_empty() && branch(&e.path("missing")).is_empty());
    t.ok("plugin list shows it ended cleanly, not stopped", !e.out(&["plugin", "list"]).lines().any(|l| l.starts_with("git-branch") && l.contains("stopped")));

    t.section("worktree-names");
    let r = e.run_cwd(&proj, &["worktree", "ABC-1"]);
    t.ok("a worktree window exists", r.all().contains("ABC-1") && w(10, &|| has_line(&names(), "ABC-1")));
    let wid = e.out(&["list-windows", "-t", "a", "-F", "#{window_id} #{window_name}"]).lines().find_map(|l| l.strip_suffix(" ABC-1").map(String::from)).unwrap_or_default();
    e.run(&["rename-window", "-t", &wid, "something-else"]);
    t.ok("renamed away, the plugin gives the worktree's name back", w(10, &|| has_line(&names(), "ABC-1")));
    e.run(&["rename-window", "-t", "a:first", "plain"]);
    std::thread::sleep(Duration::from_secs(1));
    t.ok("a window without a worktree is left alone", has_line(&names(), "plain"));

    t.section("notify-blocked");
    let script = "n=0\nr() { n=$((n + 1)); \"@BIN@\" pane report-agent --source fake --agent fake --state \"$1\" --seq \"$n\" ${2:+--message \"$2\"} >/dev/null 2>&1; }\nr idle\n: >\"@T@/agent.ready\"\nwhile IFS= read -r line </dev/tty; do\n  case \"$line\" in\n    *block*) r blocked \"approve Bash: \\\"; do shell script \\\"touch @T@/pwned\\\" -- $line\" ;;\n    *) r working; sleep 1; r done ;;\n  esac\ndone\n".replace("@BIN@", BIN).replace("@T@", &d);
    let agent_sh = e.write("agent.sh", &script);
    std::fs::set_permissions(&agent_sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ap = e.out(&["new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "a", "-n", "agent"]);
    e.ready(&ap, Duration::from_millis(400));
    e.run(&["send-keys", "-t", &ap, &p(&agent_sh), "Enter"]);
    t.ok("the fake agent is up", w(10, &|| e.path("agent.ready").exists()));
    e.run(&["send-keys", "-t", &ap, "work", "Enter"]);
    std::thread::sleep(Duration::from_secs(2));
    t.ok("no notification for working or done", !e.path("osascript.log").exists());
    e.run(&["send-keys", "-t", &ap, "block", "Enter"]);
    t.ok("a blocked agent gives a notification", w(10, &|| e.read("osascript.log").contains("--end--")));
    let log = e.read("osascript.log");
    t.ok("with the window name in the title argument", has_line(&log, "agent"));
    t.ok("and the agent's message as one argument, text and not script", log.contains("approve Bash") && !e.path("pwned").exists());
    e.run_for(&["kill-server"], Duration::from_secs(20));
    t.ok("the plugins end with the server", w(10, &|| !pgrep_in("events --types agent-state-changed", &e.dir)));
    t.ok("no errors in the server log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
