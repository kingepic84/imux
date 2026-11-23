//! imux worktree list [repo]: path, branch, the window that shows it, notes; works without a
//! server; a worktree whose branch already exists is checked out, not recreated.
//! (The Rust version of tests/q_worktreelist.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_worktreelist() {
    let mut e = Env::new("qwl");
    let mut t = Check::new("q_worktreelist");
    let conf = e.write("conf", "set autosave-interval 0\nset worktree-base main\nset worktree-name-pattern \"[A-Z]+-[0-9]+\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@example.com"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@example.com"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_SYSTEM", "/dev/null")] {
        e.set(k, v);
    }
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
    assert!(git(&proj, &["branch", "ABC-9"]));
    let list = || e.run_cwd(&proj, &["worktree", "list"]).all();

    t.section("no server");
    t.matches("the main checkout is listed with its branch and a main note", "(?m)proj\tmain\t-\tmain$", &list());
    t.ok("from anywhere with a repository argument", e.run_cwd(&e.dir, &["worktree", "list", &p(&proj)]).all().contains("proj\tmain"));
    t.ok("outside a repository it says so", e.run_cwd(&e.dir, &["worktree", "list"]).all().contains("not in a git repository"));
    let r = e.run(&["worktree", "list", "a", "b"]);
    t.ok("two words are a usage error", !r.success() && r.all().contains("usage: worktree list"));

    t.section("with a server and windows");
    let mut c = e.client(100, 30, &["new", "-s", "w"]);
    assert!(e.has_client());
    wait_until(Duration::from_secs(15), || {
        let r = e.run_cwd(&proj, &["worktree", "ABC-1"]).all();
        !r.contains("no session to open") && !r.contains("no server running")
    });
    t.ok(
        "the new worktree is listed on its branch",
        wait_until(Duration::from_secs(10), || regex_lite::Regex::new("ABC-1\tABC-1\tw:[0-9]*\t").unwrap().is_match(&list())),
    );
    let l3 = list();
    t.ok("with the window that shows it", l3.contains("ABC-1\tABC-1\tw:1"));
    t.matches("and the main checkout shows no window", "(?m)proj\tmain\t-\tmain$", &l3);

    t.section("an existing branch is checked out");
    e.run_cwd(&proj, &["worktree", "ABC-9"]);
    t.ok("the worktree is on the branch that existed", wait_until(Duration::from_secs(10), || list().contains("ABC-9\tABC-9\tw:")));
    t.ok("and that branch still points where it did", git_out(&proj, &["rev-parse", "ABC-9"]) == git_out(&proj, &["rev-parse", "main"]));
    c.kill();
    e.kill_server();
    t.finish();
}
