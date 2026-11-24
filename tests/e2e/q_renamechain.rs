//! `automatic-rename-format` with several substitutions chained by semicolons, as in
//! `#{s/a/b/;s/c/d/:pane_current_command}`: it names the window, also when the configuration is
//! loaded again into a running server. (Before, a chain gave an empty name.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_renamechain() {
    let mut e = Env::new("qrc");
    let mut t = Check::new("q_renamechain");
    // programs that all mean one thing: copies of `sleep` under other names (started with `exec`, so the
    // program and not the shell that starts it is what the pane runs)
    let bin = e.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["2.1.325", "aca-claude-go", "other-program"] {
        std::fs::copy("/bin/sleep", bin.join(name)).unwrap();
    }
    let chain = "set -g automatic-rename-format \"#{s/2.1.325/claude/;s/2.1.327/claude/;s/aca-claude-go/claude/:pane_current_command}\"\n";
    let conf = e.write("imux.conf", chain);
    e.set("IMUX_CONFIG", &p(&conf));
    e.session("s", "first");
    let name_of = |id: &str| {
        e.out(&["list-windows", "-t", "s:", "-F", "#{window_id} #{window_name}"])
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{id} ")).map(str::to_string))
            .unwrap_or_default()
    };
    let run = |program: &str| {
        let id = e.out(&["new-window", "-d", "-P", "-F", "#{window_id}", "-t", "s:", &format!("exec {} 60", p(&bin.join(program)))]);
        id.trim().to_string()
    };

    t.section("the configuration loaded at start");
    t.matches("the format is kept as written", "s/2.1.325/claude/;s/2.1.327/claude/;s/aca-claude-go/claude/", &e.out(&["show-options", "-gv", "automatic-rename-format"]));
    let a = run("aca-claude-go");
    t.ok("the first substitution of the chain's last link names the window", wait_until(Duration::from_secs(20), || name_of(&a) == "claude"));
    let b = run("2.1.325");
    t.ok("the first link names a window", wait_until(Duration::from_secs(20), || name_of(&b) == "claude"));
    let c = run("other-program");
    t.ok("a program no link matches keeps its own name", wait_until(Duration::from_secs(20), || name_of(&c) == "other-program"));

    t.section("the configuration loaded again");
    e.run(&["set", "-g", "automatic-rename-format", "#{pane_current_command}"]);
    let d = run("aca-claude-go");
    t.ok("with the plain format the program keeps its name", wait_until(Duration::from_secs(20), || name_of(&d) == "aca-claude-go"));
    let r = e.run(&["source-file", &p(&conf)]);
    t.ok("source-file reports nothing wrong", r.success() && r.all().trim().is_empty());
    let f = run("aca-claude-go");
    t.ok("after the reload the chain names a new window", wait_until(Duration::from_secs(20), || name_of(&f) == "claude"));
    t.ok("and an older window is renamed by it too", wait_until(Duration::from_secs(20), || name_of(&d) == "claude"));
    e.kill_server();
    t.finish();
}
