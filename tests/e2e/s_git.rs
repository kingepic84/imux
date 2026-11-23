//! Phase 7 (S5/S7): a repository that carries a command in its own config (core.fsmonitor, which
//! git runs on status and checkout) must not make `worktree` or `worktree-remove` run it.
//! (The Rust version of tests/s_git.sh.)
use crate::support::*;

#[test]
fn s_git() {
    let mut e = Env::new("sgit");
    let mut t = Check::new("s_git");
    let conf = e.write("conf", "set autosave-interval 0\nset worktree-base main\nset worktree-name-pattern \"[A-Z]+-[0-9]+\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@example.com"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@example.com"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_SYSTEM", "/dev/null")] {
        e.set(k, v);
    }
    let d = p(&e.dir);
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
    let mark = e.path("fsmonitor-ran");
    let hook = e.write("hook.sh", &format!("#!/bin/sh\ntouch \"{}\"\nprintf \"\\0\"\n", mark.display()));
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert!(git(&proj, &["config", "core.fsmonitor", &p(&hook)]));
    t.section("control: plain git runs the repository's command");
    git(&proj, &["status"]);
    t.ok("plain git status ran core.fsmonitor", mark.exists());
    let _ = std::fs::remove_file(&mark);
    e.session("w", "w");
    t.section("imux does not");
    e.run_cwd(&proj, &["worktree", "ABC-1"]);
    t.ok("the worktree was made", work.join("proj-worktrees/ABC-1").is_dir());
    t.ok("worktree did not run core.fsmonitor", !mark.exists());
    e.run_cwd(&proj, &["worktree-remove", "ABC-1"]);
    t.ok("the worktree was removed", !work.join("proj-worktrees/ABC-1").exists());
    t.ok("worktree-remove did not run core.fsmonitor", !mark.exists());
    t.section("hostile names never reach git as options");
    let evil = format!("A;touch${{IFS}}{d}/pwned");
    for name in ["-x", "--upload-pack=x", "..", "a b", "a\nb", evil.as_str()] {
        let r = e.run_cwd(&proj, &["worktree", name]);
        t.matches(&format!("refused: {name:?}"), "bad worktree name|does not match|usage|unknown", &r.all());
    }
    t.ok("nothing was run from a name", !e.path("pwned").exists());
    e.kill_server();
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
