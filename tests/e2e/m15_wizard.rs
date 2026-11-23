//! The repository wizard (prefix W): a picker of the repositories under repos-dir, then a worktree
//! name, the branch format with a slug, and an unreadable repos-dir. (The Rust version of tests/m15_wizard.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(15);
const GAP: Duration = Duration::from_millis(600);

fn client(e: &Env, args: &[&str]) -> Client {
    let c = e.client(100, 30, args);
    assert!(e.has_client(), "the client did not attach");
    std::thread::sleep(Duration::from_millis(700));
    c
}

fn leave(mut c: Client) {
    c.send(b"\x02d");
    if c.wait_exit(Duration::from_secs(3)).is_none() {
        c.kill();
    }
}

fn branch(dir: &std::path::Path) -> String {
    let out = std::process::Command::new("git").args(["-C", dir.to_str().unwrap(), "symbolic-ref", "--short", "HEAD"]).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn m15_wizard() {
    let mut e = Env::new("m15");
    let repos = e.path("repos");
    let conf = e.write("conf", &format!("set autosave-interval 0\nset repos-dir \"{}\"\nset worktree-base main\n", repos.display()));
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("m15_wizard");
    for d in ["beta-src", "plain", ".hidden"] {
        std::fs::create_dir_all(repos.join(d)).unwrap();
    }
    for r in ["alpha", "beta"] {
        let bare = e.path(&format!("{r}.git"));
        assert!(git(&e.dir, &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]));
        let work = repos.join(r);
        assert!(git(&e.dir, &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]));
        assert!(git(&work, &["checkout", "-q", "-b", "main"]));
        std::fs::write(work.join("f"), "x\n").unwrap();
        assert!(git(&work, &["add", "f"]));
        assert!(git(&work, &["commit", "-q", "-m", "init"]));
        assert!(git(&work, &["push", "-q", "origin", "main"]));
    }
    std::fs::create_dir_all(repos.join(".hidden/.git")).unwrap();

    t.section("list, cancel");
    let mut c = client(&e, &["new", "-s", "w"]);
    c.send(b"\x02W");
    t.ok("the picker is titled", c.wait_seen("repositories", LONG));
    t.ok("lists a repository", c.wait_seen("alpha", LONG));
    t.ok("lists another", c.seen("beta"));
    t.ok("plain directories are not listed", !c.seen("plain"));
    t.ok("hidden ones are not listed", !c.text().contains(".hidden"));
    c.send(b"q");
    std::thread::sleep(GAP);
    t.ok("cancelling creates nothing", !repos.join("alpha-worktrees").exists() && !repos.join("beta-worktrees").exists());
    leave(c);

    t.section("pick the second repository, name a worktree");
    let mut c = client(&e, &["attach", "-t", "w"]);
    c.type_chunks(&["\x02W", "j", "\r"], GAP);
    t.ok("the name prompt shows", c.wait_seen("worktree name", LONG));
    c.send_str("ABC-9\r");
    t.ok("the worktree is made in the picked repository", wait_until(LONG, || repos.join("beta-worktrees/ABC-9").is_dir()));
    t.ok("and not in the other", !repos.join("alpha-worktrees").exists());
    t.ok("on its new branch", branch(&repos.join("beta-worktrees/ABC-9")) == "ABC-9");
    t.ok("a window was added", wait_until(LONG, || e.all(&["ls"]).contains("w: 2 windows")));
    leave(c);

    t.section("branch format with a slug");
    let mut c = client(&e, &["attach", "-t", "w"]);
    c.type_chunks(&["\x02:"], GAP);
    c.send_str("set worktree-branch-format feature/#{name}-#{slug}\r");
    std::thread::sleep(GAP);
    c.type_chunks(&["\x02W", "\r"], GAP);
    c.send_str("ABC-10 login-fix\r");
    t.ok("the directory is the bare name", wait_until(LONG, || repos.join("alpha-worktrees/ABC-10").is_dir()));
    t.ok("the branch has prefix and slug", wait_until(LONG, || branch(&repos.join("alpha-worktrees/ABC-10")) == "feature/ABC-10-login-fix"));
    leave(c);
    let mut c = client(&e, &["attach", "-t", "w"]);
    c.type_chunks(&["\x02W", "\r"], GAP);
    c.send_str("ABC-11\r");
    t.ok("without a slug the dangling dash is dropped", wait_until(LONG, || branch(&repos.join("alpha-worktrees/ABC-11")) == "feature/ABC-11"));
    leave(c);

    t.section("an empty repos-dir");
    let mut c = client(&e, &["attach", "-t", "w"]);
    c.type_chunks(&["\x02:"], GAP);
    c.send_str("set repos-dir /nonexistent-dir-x\r");
    std::thread::sleep(GAP);
    c.send(b"\x02W");
    t.ok("a message says it cannot read it", c.wait_seen("cannot read /nonexistent-dir-x", LONG));
    leave(c);
    e.kill_server();
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
