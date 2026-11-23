//! #(command) in the status line (M37): the output shows up, a command that hangs is stopped and
//! does not freeze the bar, escapes and # in the output are text, a name a pane could have set is
//! refused (or quoted with q:), the output is refreshed every status-interval, and
//! status-commands-max limits the commands. (The Rust version of tests/f_status.sh.)
use crate::support::*;
use std::time::Duration;

fn run_client(e: &Env, cwd: &std::path::Path, conf: &std::path::Path, cols: u16, rows: u16, name: &str, secs: u64) -> Client {
    let mut c = e.client_in(cwd, cols, rows, &["new", "-s", name], &[("IMUX_CONFIG", &p(conf))]);
    std::thread::sleep(Duration::from_secs(secs));
    c.send(b"\x02d");
    c.wait_exit(Duration::from_secs(10));
    c
}

#[test]
fn f_status() {
    let e = Env::new("fst");
    let d = e.dir.clone();
    let ds = p(&d);
    let spaced = d.join("dir with space");
    std::fs::create_dir_all(&spaced).unwrap();
    let esc = e.write("esc.sh", "#!/bin/sh\nprintf '\\033[31mESC:#[bg=red]#{session_name}#(echo hacked)\\n'\n");
    std::fs::set_permissions(&esc, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let one = e.write(
        "one.conf",
        &format!(
            "set -g automatic-rename off\nset -g status-interval 1\nset -g status-command-timeout 1\n\
             set -g status-left 'L#(echo LEFTSEG)#(touch {ds}/refused.out #{{session_name}})'\n\
             set -g status-right '#(echo RIGHTSEG) #(sleep 3137; echo LATESEG) ENDBAR #(basename #{{q:pane_current_path}}) #({ds}/esc.sh) #(echo x >> {ds}/runs.out)'\n\
             set -g status-right-length 120\n"
        ),
    );
    let mut t = Check::new("f_status");

    t.section("the bar");
    let c = run_client(&e, &spaced, &one, 160, 24, "f", 6);
    let screen = format!("{}\n{}", c.text(), c.screen_text());
    t.ok("the output of a command in status-left shows", screen.contains("LEFTSEG"));
    t.ok("the output of a command in status-right shows", screen.contains("RIGHTSEG"));
    t.ok("the rest of the bar is drawn while a command hangs", screen.contains("ENDBAR"));
    t.ok("a command that hangs shows nothing", !screen.contains("LATESEG"));
    t.ok("the server is still there", e.run(&["has-session", "-t", "f"]).success());
    let runs = lines_of(&e.path("runs.out"));
    std::thread::sleep(Duration::from_secs(5));
    let later = lines_of(&e.path("runs.out"));
    std::thread::sleep(Duration::from_secs(3));
    t.ok("it is stopped with its children and, once nobody looks, not started again", !pgrep("sleep 3137"));
    t.ok(&format!("a bar nobody looks at any more is not refreshed ({later} runs, then {})", lines_of(&e.path("runs.out"))), lines_of(&e.path("runs.out")) == later);
    t.ok("the text of a command takes the directory of the pane, quoted with q:", screen.contains("dir with space"));
    t.ok("an escape in the output is dropped", screen.contains("ESC:#[bg=red]#{session_name}#(echo hacked)"));
    t.ok("and nothing in it ran as a command", !c.screen_text().lines().any(|l| l.trim_end().ends_with("hacked")));
    t.ok("a name a pane could have set is refused and the bar says so", screen.contains("[#{q:session_name}?]"));
    t.ok("and the refused command did not run", !e.path("refused.out").exists());
    t.ok(&format!("the output is refreshed every status-interval (ran {runs} times)"), (3..=9).contains(&runs));
    e.kill_server();
    std::thread::sleep(Duration::from_secs(1));

    t.section("the limit");
    let two = e.write("two.conf", &format!("set -g automatic-rename off\nset -g status-interval 1\nset -g status-commands-max 1\nset -g status-left '#(echo ONE >> {ds}/one.out)'\nset -g status-right '#(echo TWO >> {ds}/two.out)'\n"));
    run_client(&e, &d, &two, 80, 24, "f", 4);
    t.ok("the first command runs", std::fs::metadata(e.path("one.out")).map(|m| m.len() > 0).unwrap_or(false));
    t.ok("a second different one is over status-commands-max", !e.path("two.out").exists());
    e.kill_server();

    t.section("no client, no commands");
    let three = e.write("three.conf", &format!("set -g automatic-rename off\nset -g status-interval 1\nset -g status-left '#(echo NOBODY >> {ds}/nobody.out)'\n"));
    e.run_env(&["new-session", "-d", "-s", "g"], &[("IMUX_CONFIG", &p(&three))]);
    std::thread::sleep(Duration::from_secs(3));
    t.ok("nothing runs for a bar nobody looks at", !e.path("nobody.out").exists());
    e.kill_server();
    t.finish();
}
