//! `imux worktree`: a worktree cut from the freshly fetched base, reuse, --detach, an explicit base,
//! the repository as the first word, an offline origin, and the refusals. A window for each success.
//! (The Rust version of tests/m14_worktree.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

#[test]
fn m14_worktree() {
    let mut e = Env::new("m14");
    let conf = e.write("conf", "set autosave-interval 0\nset worktree-base develop\nset worktree-name-pattern \"[A-Z]+-[0-9]+\"\nset worktree-command \"sleep 31338\"\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("m14_worktree");
    let d = e.dir.clone();
    let w = d.join("work");
    std::fs::create_dir_all(&w).unwrap();
    let origin = d.join("origin.git");
    let proj = w.join("proj");
    assert!(git(&d, &["init", "-q", "--bare", "-b", "develop", origin.to_str().unwrap()]));
    assert!(git(&d, &["clone", "-q", origin.to_str().unwrap(), proj.to_str().unwrap()]));
    assert!(git(&proj, &["checkout", "-q", "-b", "develop"]));
    std::fs::write(proj.join("a.txt"), "one\n").unwrap();
    assert!(git(&proj, &["add", "a.txt"]));
    assert!(git(&proj, &["commit", "-q", "-m", "one"]));
    assert!(git(&proj, &["push", "-q", "origin", "develop"]));
    assert!(git(&proj, &["checkout", "-q", "-b", "main"]));
    assert!(git(&proj, &["push", "-q", "origin", "main"]));
    assert!(git(&proj, &["checkout", "-q", "-b", "feature"]));
    std::fs::write(proj.join("a.txt"), "one\nmine\n").unwrap();
    std::fs::write(proj.join("untracked.txt"), "scratch\n").unwrap();
    let other = d.join("other");
    assert!(git(&d, &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]));
    assert!(git(&other, &["checkout", "-q", "develop"]));
    std::fs::write(other.join("b.txt"), "two\n").unwrap();
    assert!(git(&other, &["add", "b.txt"]));
    assert!(git(&other, &["commit", "-q", "-m", "two"]));
    assert!(git(&other, &["push", "-q", "origin", "develop"]));
    let newsha = git_out(&other, &["rev-parse", "HEAD"]);
    let mainsha = git_out(&origin, &["rev-parse", "main"]);
    let wt = |n: &str| w.join(format!("proj-worktrees/{n}"));

    let mut client = e.client(100, 30, &["new", "-s", "w"]);
    assert!(wait_until(LONG, || e.out(&["ls"]).lines().any(|l| l.starts_with("w:"))), "the session did not start");
    assert!(e.ready("w:", Duration::from_millis(600)));

    t.section("a worktree off the fetched base");
    let r = e.run_cwd(&proj, &["worktree", "ABC-1"]);
    t.matches("reports the base", "worktree ABC-1 from origin/develop", &r.all());
    t.ok("directory beside the repo", wt("ABC-1").is_dir());
    t.ok("branch is cut from the new origin/develop, not the checkout", git_out(&wt("ABC-1"), &["rev-parse", "HEAD"]) == newsha);
    t.ok("worktree is on the new branch", git_out(&wt("ABC-1"), &["symbolic-ref", "--short", "HEAD"]) == "ABC-1");
    t.ok("the main checkout is still on its branch", git_out(&proj, &["symbolic-ref", "--short", "HEAD"]) == "feature");
    t.ok("the main checkout keeps its changes", std::fs::read_to_string(proj.join("a.txt")).unwrap_or_default().contains("mine") && proj.join("untracked.txt").exists());
    t.ok("a window was added", wait_until(LONG, || e.all(&["ls"]).contains("w: 2 windows")));
    t.ok("the command runs in a pane", wait_until(LONG, || pgrep("sleep 31338")));

    t.section("again, detached, explicit base");
    t.matches("second call reuses", "reused", &e.run_cwd(&proj, &["worktree", "ABC-1"]).all());
    e.run_cwd(&proj, &["worktree", "--detach", "ABC-2"]);
    t.ok("detached head", !git(&wt("ABC-2"), &["symbolic-ref", "-q", "HEAD"]));
    t.ok("detached at origin/develop", git_out(&wt("ABC-2"), &["rev-parse", "HEAD"]) == newsha);
    e.run_cwd(&proj, &["worktree", "ABC-3", "main"]);
    t.ok("explicit base", git_out(&wt("ABC-3"), &["rev-parse", "HEAD"]) == mainsha);
    e.run_cwd(&w, &["worktree", "proj", "ABC-6"]);
    t.ok("repo given as the first word", wt("ABC-6").is_dir());

    t.section("origin offline");
    let gone = p(&d.join("gone.git"));
    assert!(git(&proj, &["remote", "set-url", "origin", &gone]));
    let r = e.run_cwd(&proj, &["worktree", "ABC-4"]);
    t.matches("warns about the fetch", "warning: fetch failed", &r.all());
    t.ok("still created from the last known origin", wt("ABC-4").is_dir());
    assert!(git(&proj, &["remote", "set-url", "origin", origin.to_str().unwrap()]));

    t.section("failures");
    t.matches("a name outside the pattern is refused", "does not match worktree-name-pattern", &e.run_cwd(&proj, &["worktree", "abc"]).all());
    t.matches("a missing base is named", "origin/nope", &e.run_cwd(&proj, &["worktree", "ABC-5", "nope"]).all());
    t.ok("nothing left behind for it", !wt("ABC-5").exists());
    t.matches("outside a repository", "not in a git repository", &e.run_cwd(&d, &["worktree", "ABC-7"]).all());
    t.matches("no name is a usage error", "usage: worktree", &e.all(&["worktree"]));
    t.ok("windows for the successful calls only", wait_until(LONG, || e.all(&["ls"]).contains("w: 7 windows")));

    e.kill_server();
    client.wait_exit(Duration::from_secs(10));
    pkill("sleep 31338");
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
