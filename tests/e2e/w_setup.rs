//! `imux worktree` setup: the option runs before the command, --no-setup, the command waits for a slow
//! setup, a failing setup leaves it unstarted, reuse, variables in the setup text and what is
//! refused. (The Rust version of tests/w_setup.sh.)
use crate::support::*;
use std::path::Path;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);
const MARK: &str = "sleep 31342";

#[test]
fn w_setup() {
    let mut e = Env::new("wst");
    let d = e.dir.clone();
    let opt = d.join("opt.txt");
    let conf = e.write(
        "conf",
        &format!("set autosave-interval 0\nset worktree-base main\nset worktree-name-pattern \"[A-Z]+-[0-9]+\"\nset worktree-command \"{MARK}\"\nset worktree-setup \"echo option >> {}\"\n", opt.display()),
    );
    e.set("IMUX_CONFIG", &p(&conf));
    e.unset("SHELL");
    let mut t = Check::new("w_setup");
    let origin = d.join("origin.git");
    let proj = d.join("work/proj");
    std::fs::create_dir_all(d.join("work")).unwrap();
    assert!(git(&d, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]));
    assert!(git(&d, &["clone", "-q", origin.to_str().unwrap(), proj.to_str().unwrap()]));
    assert!(git(&proj, &["checkout", "-q", "-b", "main"]));
    std::fs::write(proj.join("a.txt"), "one\n").unwrap();
    assert!(git(&proj, &["add", "a.txt"]));
    assert!(git(&proj, &["commit", "-q", "-m", "one"]));
    assert!(git(&proj, &["push", "-q", "origin", "main"]));
    let stop = || {
        pkill(MARK);
        assert!(wait_until(LONG, || !pgrep(MARK)), "the command did not end");
    };
    let wt = |args: &[&str]| e.run_cwd(&proj, args);

    let mut client = e.client(100, 30, &["new", "-s", "w"]);
    assert!(wait_until(LONG, || e.out(&["ls"]).lines().any(|l| l.starts_with("w:"))));
    assert!(e.ready("w:", Duration::from_millis(600)));

    t.section("the option runs before the command");
    wt(&["worktree", "ABC-1"]);
    t.ok("the setup of the option ran in the new window", wait_until(LONG, || lines_of(&opt) == 1));
    t.ok("and the command started after it", wait_until(LONG, || pgrep(MARK)));
    stop();

    t.section("--no-setup runs the command alone");
    wt(&["worktree", "--no-setup", "ABC-2"]);
    t.ok("the command started", wait_until(LONG, || pgrep(MARK)));
    t.ok("no setup ran", lines_of(&opt) == 1);
    stop();

    t.section("the command waits for a slow setup");
    // the slow setup waits for a file, so it is over when the test says and not after a guessed time
    let slow = d.join("slow.marker");
    let started = d.join("slow.started");
    let gate = d.join("slow.gate");
    let setup = format!("touch {}; while [ ! -f {} ]; do sleep 0.05; done; touch {}", started.display(), gate.display(), slow.display());
    wt(&["worktree", "--setup", &setup, "ABC-3"]);
    t.ok("the slow setup started", wait_until(LONG, || started.exists()));
    t.ok("while the setup runs the marker is not there", !slow.exists());
    // another worktree goes all the way through its own setup and command; the command of the
    // waiting one would have been started long before that
    wt(&["worktree", "--no-setup", "ABC-8"]);
    t.ok("another worktree's command runs meanwhile", wait_until(LONG, || pgrep(MARK)));
    t.ok("and the command waiting for the setup has not started", pgrep_count(MARK) == 1);
    std::fs::write(&gate, "").unwrap();
    t.ok("afterwards the setup has finished", wait_until(LONG, || slow.exists()));
    t.ok("and the command runs", wait_until(LONG, || pgrep_count(MARK) == 2));
    stop();

    t.section("a failing setup leaves the command unstarted");
    wt(&["worktree", "--setup", "false", "ABC-4"]);
    // a worktree made after it goes through setup and command; the failed one's command would have started first
    wt(&["worktree", "--no-setup", "ABC-9"]);
    t.ok("a later worktree's command runs", wait_until(LONG, || pgrep(MARK)));
    t.ok("the command of the failed setup did not start", pgrep_count(MARK) == 1);
    stop();

    t.section("an existing worktree skips the option's setup unless --setup is given");
    let r = wt(&["worktree", "ABC-3"]);
    t.matches("it was reused", "reused", &r.all());
    wt(&["worktree", "--setup", &format!("echo forced >> {}", opt.display()), "ABC-3"]);
    t.ok("an explicit --setup runs on a reused worktree", wait_until(LONG, || lines_of(&opt) >= 2));
    // the option's setup, run again on the reuse, would have added a line of its own before this one
    t.ok("the option's setup did not run again", lines_of(&opt) == 2);
    stop();

    t.section("variables are quoted, text with control characters is refused");
    let vars = d.join("vars.txt");
    wt(&["worktree", "--setup", &format!("echo #{{name}} #{{branch}} >> {}", vars.display()), "ABC-5"]);
    t.ok("the variables are replaced", wait_until(LONG, || std::fs::read_to_string(&vars).unwrap_or_default().contains("ABC-5 ABC-5")));
    t.matches("a setup with a newline is refused with a reason", "control characters", &wt(&["worktree", "--setup", "echo a\necho b", "ABC-6"]).all());
    t.matches("an unknown variable is refused with a reason", "not a variable", &wt(&["worktree", "--setup", "echo #{nope}", "ABC-7"]).all());
    stop();
    let _ = Path::new(".");
    e.kill_server();
    client.wait_exit(Duration::from_secs(10));
    t.ok("no errors in server.log", !e.server_log().contains("\"level\":\"error\""));
    t.finish();
}
