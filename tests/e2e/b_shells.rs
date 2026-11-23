//! What imux types into a pane shell must work in every shell it knows: the worktree setup chained
//! to the command (and a failing setup stopping it), and a restored program coming back with its
//! hostile arguments intact. One test for each of bash, zsh, fish, nu and pwsh; a shell that is not
//! installed is skipped. (The Rust version of tests/b_shells.sh.)
use crate::support::*;
use std::path::Path;
use std::time::Duration;

const HOSTILE: &str = "it's $HOME;`id` 'q' \"d\" -n";

/// The shell of a pane has finished starting. A quiet pane is not proof: PowerShell is silent for a
/// while (longer on a loaded machine) before it asks where the cursor is, and drops what is typed
/// until it has its answer. Its prompt (`PS /dir> `) is the sign that it reads input; the quiet
/// wait after it covers the second cursor request that follows the prompt.
fn shell_ready(e: &Env, name: &str, pane: &str) -> bool {
    if name == "pwsh" && !wait_until(Duration::from_secs(60), || e.capture(pane).contains("PS ")) {
        return false;
    }
    e.ready(pane, Duration::from_secs(2))
}

fn one_shell(name: &str, n: u32) {
    let Some(path) = which(name) else {
        println!("skip {name}: not installed");
        return;
    };
    let (run, touch, false_) = match name {
        "nu" => ("^/bin/sleep", "^/usr/bin/touch", "^/usr/bin/false"),
        "pwsh" => ("& /bin/sleep", "& /usr/bin/touch", "& /usr/bin/false"),
        _ => ("/bin/sleep", "/usr/bin/touch", "/usr/bin/false"),
    };
    let sleep_n = 31400 + n;
    let e = Env::new(&format!("shells-{name}"));
    let d = e.dir.clone();
    let mut t = Check::new(&format!("b_shells[{name}]"));

    // a project with a remote, as the worktree command wants
    let work = d.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let origin = d.join("origin.git");
    assert!(git(&d, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]));
    let proj = work.join("proj");
    assert!(git(&d, &["clone", "-q", origin.to_str().unwrap(), proj.to_str().unwrap()]));
    assert!(git(&proj, &["checkout", "-q", "-b", "main"]));
    std::fs::write(proj.join("a.txt"), "one\n").unwrap();
    assert!(git(&proj, &["add", "a.txt"]));
    assert!(git(&proj, &["commit", "-q", "-m", "one"]));
    assert!(git(&proj, &["push", "-q", "origin", "main"]));

    e.write(
        "conf",
        &format!(
            "set autosave-interval 0\nset default-shell {path}\nset worktree-base main\nset worktree-name-pattern \"[A-Z]+-[0-9]+\"\nset worktree-command \"{run} {sleep_n}\"\nset restore-commands \"perl\"\n"
        ),
    );
    let mut e = e;
    let conf = p(&d.join("conf"));
    e.set("IMUX_CONFIG", &conf);
    println!("== {name} ({path})");

    let mut client = e.client(100, 30, &["new", "-s", "w"]);
    assert!(wait_until(Duration::from_secs(20), || e.out(&["ls"]).lines().any(|l| l.starts_with("w:"))), "the session w did not start");

    assert!(shell_ready(&e, name, "w:"), "the first pane of w never settled");
    let marker = p(&d.join("ok.marker"));
    e.run_cwd(&proj, &["worktree", "--setup", &format!("{touch} {marker}"), &format!("ABC-{n}")]);
    t.ok(&format!("{name}: the setup ran"), wait_until(Duration::from_secs(30), || Path::new(&marker).exists()));
    t.ok(&format!("{name}: and the command started after it"), wait_until(Duration::from_secs(15), || pgrep(&format!("sleep {sleep_n}"))));
    pkill(&format!("sleep {sleep_n}"));

    e.run_cwd(&proj, &["worktree", "--setup", false_, &format!("ABD-{n}")]);
    // a worktree made after it goes through its setup and command; the command of the failed one
    // would have started before that
    e.run_cwd(&proj, &["worktree", "--setup", &format!("{touch} {}", p(&d.join("later.marker"))), &format!("ABE-{n}")]);
    t.ok(&format!("{name}: a later worktree's command runs"), wait_until(Duration::from_secs(30), || pgrep(&format!("sleep {sleep_n}"))));
    t.ok(&format!("{name}: a failing setup keeps the command from starting"), pgrep_count(&format!("sleep {sleep_n}")) == 1);
    pkill(&format!("sleep {sleep_n}"));

    t.section(&format!("{name}: a restored program keeps its arguments"));
    let args_txt = d.join("args.txt");
    let prog = d.join("prog.pl");
    std::fs::write(&prog, format!("open(F, \">>{}\"); print F $ARGV[0], \"\\n\", scalar(@ARGV), \"\\n\"; close(F); sleep {}\n", args_txt.display(), 31500 + n)).unwrap();
    let quoted = match name {
        "fish" => r#"'it\'s $HOME;`id` \'q\' "d" -n'"#.to_string(),
        "nu" => r##"r#'it's $HOME;`id` 'q' "d" -n'#"##.to_string(),
        "pwsh" => r#"'it''s $HOME;`id` ''q'' "d" -n'"#.to_string(),
        _ => r#"'it'\''s $HOME;`id` '\''q'\'' "d" -n'"#.to_string(),
    };
    e.run(&["new-session", "-d", "-s", "r"]);
    assert!(shell_ready(&e, name, "r:"), "the shell of r never settled");
    e.run(&["send-keys", "-t", "r:", "-l", &format!("perl {} {quoted}", p(&prog))]);
    e.run(&["send-keys", "-t", "r:", "Enter"]);
    t.ok(&format!("{name}: the program runs"), wait_until(Duration::from_secs(15), || lines_of(&args_txt) == 2));
    let lines = |f: &Path| -> Vec<String> { std::fs::read_to_string(f).unwrap_or_default().lines().map(String::from).collect() };
    let l = lines(&args_txt);
    t.ok(&format!("{name}: it got the hostile word as exactly one argument when typed by hand"), l.first().map(String::as_str) == Some(HOSTILE) && l.get(1).map(String::as_str) == Some("1"));
    e.run(&["save", "snap"]);
    pkill(&p(&prog));
    e.run(&["kill-session", "-t", "r"]);
    e.run(&["restore", "snap"]);
    t.ok(&format!("{name}: it is typed again after the restore"), wait_until(Duration::from_secs(30), || pgrep(&p(&prog))));
    t.ok(&format!("{name}: the second run wrote its arguments"), wait_until(Duration::from_secs(10), || lines_of(&args_txt) == 4));
    let l = lines(&args_txt);
    t.ok(
        &format!("{name}: both runs saw the hostile word as exactly one argument"),
        l.len() == 4 && l[0] == HOSTILE && l[1] == "1" && l[2] == HOSTILE && l[3] == "1",
    );
    pkill(&p(&prog));
    e.kill_server();
    client.wait_exit(Duration::from_secs(10));
    t.finish();
}

#[test]
fn b_shells_bash() {
    one_shell("bash", 1);
}

#[test]
fn b_shells_zsh() {
    one_shell("zsh", 2);
}

#[test]
fn b_shells_fish() {
    one_shell("fish", 3);
}

#[test]
fn b_shells_nu() {
    one_shell("nu", 4);
}

#[test]
fn b_shells_pwsh() {
    one_shell("pwsh", 5);
}
