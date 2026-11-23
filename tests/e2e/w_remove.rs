//! `imux worktree-remove`: a clean worktree goes, its window closes and its branch stays; changes
//! and commits no remote has are refused; --force; no name from inside a worktree; what is never
//! removed. (The Rust version of tests/w_remove.sh.)
use crate::support::*;
use regex_lite::Regex;
use std::time::Duration;

#[test]
fn w_remove() {
    let mut e = Env::new("wrm");
    let mut t = Check::new("w_remove");
    let conf = e.write("conf", "set autosave-interval 0\nset worktree-base main\nset worktree-name-pattern \"[A-Z]+-[0-9]+\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@example.com"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@example.com"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_SYSTEM", "/dev/null")] {
        e.set(k, v);
    }
    let work = e.path("work");
    std::fs::create_dir_all(&work).unwrap();
    let origin = e.path("origin.git");
    assert!(git(&work, &["init", "-q", "--bare", "-b", "main", &p(&origin)]));
    assert!(git(&work, &["clone", "-q", &p(&origin), "proj"]));
    let proj = work.join("proj");
    let wt = work.join("proj-worktrees");
    assert!(git(&proj, &["checkout", "-q", "-b", "main"]));
    std::fs::write(proj.join("a.txt"), "one\n").unwrap();
    assert!(git(&proj, &["add", "a.txt"]));
    assert!(git(&proj, &["commit", "-q", "-m", "one"]));
    assert!(git(&proj, &["push", "-q", "origin", "main"]));
    let mut c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client());
    let windows = || {
        let ls = e.all(&["ls"]);
        Regex::new("([0-9]+) window").unwrap().captures(&ls).map(|c| c[1].to_string()).unwrap_or_default()
    };
    let run = |args: &[&str]| e.run_cwd(&proj, args).all();

    t.section("a clean worktree is removed, its window closes, its branch stays");
    // The client has to have finished attaching before there is a session to open a window in.
    wait_until(Duration::from_secs(15), || !run(&["worktree", "ABC-1"]).contains("no session to open"));
    t.ok("the window is open", wait_until(Duration::from_secs(10), || windows() == "2"));
    let r = run(&["worktree-remove", "ABC-1"]);
    t.ok("it says what was removed", r.contains("removed worktree"));
    t.ok("and that the branch is kept", r.contains("branch ABC-1 kept"));
    t.ok("the directory is gone", !wt.join("ABC-1").exists());
    t.ok("git no longer lists it", !git_out(&proj, &["worktree", "list"]).contains("ABC-1"));
    t.ok("the branch still exists", !git_out(&proj, &["branch", "--list", "ABC-1"]).is_empty());
    t.ok("the window is closed", wait_until(Duration::from_secs(10), || windows() == "1"));

    t.section("changes are refused, --force removes them");
    run(&["worktree", "ABC-2"]);
    std::fs::write(wt.join("ABC-2/new.txt"), "scratch\n").unwrap();
    t.ok("a worktree with an untracked file is refused", run(&["worktree-remove", "ABC-2"]).contains("--force"));
    t.ok("and still there", wt.join("ABC-2").is_dir());
    t.ok("--force removes it", run(&["worktree-remove", "--force", "ABC-2"]).contains("removed worktree"));
    t.ok("the directory is gone", !wt.join("ABC-2").exists());

    t.section("commits that no remote has are refused");
    run(&["worktree", "ABC-3"]);
    std::fs::write(wt.join("ABC-3/b.txt"), "two\n").unwrap();
    assert!(git(&wt.join("ABC-3"), &["add", "b.txt"]));
    assert!(git(&wt.join("ABC-3"), &["commit", "-q", "-m", "two"]));
    t.ok("an unpushed commit is refused", run(&["worktree-remove", "ABC-3"]).contains("no remote"));
    t.ok("and the worktree is still there", wt.join("ABC-3").is_dir());
    assert!(git(&wt.join("ABC-3"), &["push", "-q", "origin", "ABC-3"]));
    t.ok("once pushed it goes", run(&["worktree-remove", "ABC-3"]).contains("removed worktree"));

    t.section("without a name, from inside the worktree");
    run(&["worktree", "ABC-4"]);
    let r = e.run_cwd(&wt.join("ABC-4"), &["worktree-remove"]).all();
    t.ok("the worktree the directory is in is removed", r.contains("removed worktree"));
    t.ok("the directory is gone", !wt.join("ABC-4").exists());

    t.section("what is never removed");
    t.ok("the main checkout", run(&["worktree-remove", "--force"]).contains("main checkout"));
    t.ok("is still there", proj.join("a.txt").is_file());
    t.ok("a name that is not a worktree", run(&["worktree-remove", "--force", "ABC-99"]).contains("not a worktree"));
    run(&["worktree", "ABC-5"]);
    assert!(git(&proj, &["worktree", "lock", &p(&wt.join("ABC-5"))]));
    t.ok("a locked worktree, even with --force", run(&["worktree-remove", "--force", "ABC-5"]).contains("locked"));
    t.ok("is still there", wt.join("ABC-5").is_dir());
    t.ok("a name that is a path", run(&["worktree-remove", "../../etc"]).contains("name"));
    t.ok("too many words is a usage error", run(&["worktree-remove", "a", "b", "c"]).contains("usage"));
    c.kill();
    e.kill_server();
    t.ok("the server log has no errors", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
