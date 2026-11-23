//! Command line tools that need no server: --version, doctor and completions.
//! (The Rust version of tests/q_tools.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_tools() {
    let mut e = Env::new("qtools");
    let mut t = Check::new("q_tools");
    let uid = unsafe { libc::getuid() };
    let sockdir = e.path(&format!("imux-{uid}"));

    t.section("--version");
    for word in ["--version", "-V", "version"] {
        let r = e.run(&[word]);
        t.ok(&format!("{word} exits 0"), r.code == 0);
        t.matches(&format!("{word} prints the name and a version"), r"^imux [0-9]+\.[0-9]+\.[0-9]+", &r.all());
        t.ok(&format!("{word} prints one line"), r.all().lines().count() == 1);
    }
    t.ok("no server was started", !sockdir.exists());
    let r = e.run(&["--version", "extra"]);
    t.ok("extra words after --version are refused", r.code != 0);
    t.ok("and the usage names --version", r.all().contains("--version"));

    t.section("doctor");
    let bad = e.write("bad.conf", "set -g mouse on\nset -g image-probe-timeout soon\nbogus-command x\nset -g default-terminal tmux-256color\n");
    let r = e.run_env(&["doctor"], &[("IMUX_CONFIG", &p(&bad))]);
    let d1 = r.all();
    t.ok("doctor exits 0 without a terminal", r.code == 0);
    t.matches("it says there is no terminal to ask", "(?m)^terminal probe: not asked: no terminal$", &d1);
    t.matches("a bad value is named with file and line", r"bad\.conf:2: ", &d1);
    t.matches("an unknown command is named with file and line", r"bad\.conf:3: ", &d1);
    t.matches("the problems are counted", r"(?m)^config check: 2 problem\(s\)$", &d1);
    t.matches("an option without effect is listed", "(?m)^config option without effect: default-terminal$", &d1);
    t.matches("the glyphs are shown", "(?m)^agent glyphs: working=", &d1);
    t.ok("doctor started no server", !sockdir.exists());
    t.ok("doctor wrote no state", !e.path("state").exists());
    let clean_unset = ["LC_TERMINAL", "COLORTERM", "TERM_PROGRAM", "KITTY_WINDOW_ID"];
    let doctor_in_terminal = |conf: &str| {
        let mut c = e.client_env(100, 30, &["doctor"], &[("IMUX_CONFIG", conf), ("TERM", "xterm-256color")], &clean_unset);
        c.wait_exit(Duration::from_secs(30));
        c.text()
    };
    let never = e.write("never.conf", "set -g image-probe never\n");
    let d2 = doctor_in_terminal(&p(&never));
    t.matches("image-probe never is explained", "terminal probe: not asked: image-probe is never", &d2);
    t.matches("no pixel size is explained under a terminal", r"cell size: unknown: the terminal reports no pixel size \(100x30 cells\)", &d2);
    t.matches("so is the hint about cropped pictures", "hint: without a pixel size pictures are cropped", &d2);
    t.matches("an unprobed unknown terminal gets blocks", "picture mode: blocks ", &d2);
    t.matches("and is told how to ask", "image-probe always", &d2);
    let always = e.write("always.conf", "set -g image-probe always\nset -g image-probe-timeout 300\n");
    let d3 = doctor_in_terminal(&p(&always));
    t.matches("a terminal that never answers is reported", "terminal probe: the terminal did not finish answering", &d3);
    t.matches("and nothing is ruled out then", r"picture protocols accepted: everything \(the terminal did not answer", &d3);
    e.run(&["new-session", "-d", "-s", "a"]);
    let d4 = e.all(&["doctor"]);
    t.matches("a running server is reported with its pid", r"(?m)^server: running \(pid [0-9]+\)$", &d4);
    t.matches("and the socket directory is checked", "private, ours", &d4);
    e.kill_server();
    wait_until(Duration::from_secs(10), || e.all(&["ls"]).contains("no server running"));

    t.section("completions");
    e.unset("IMUX_ENV");
    let mut scripts = std::collections::HashMap::new();
    for sh in ["zsh", "bash", "fish", "pwsh", "nu"] {
        let r = e.run(&["completions", sh]);
        if sh == "zsh" {
            t.ok("completions zsh exits 0", r.code == 0);
        }
        e.write(&format!("c.{sh}"), &r.all());
        scripts.insert(sh, r.all());
    }
    t.matches("pwsh registers a completer", "(?m)^Register-ArgumentCompleter -Native -CommandName imux", &scripts["pwsh"]);
    t.matches("nu defines the extern", "(?m)^extern \"imux\" ", &scripts["nu"]);
    t.matches("zsh starts with its header", "(?m)^#compdef imux$", &scripts["zsh"]);
    t.matches("bash hooks the function up", "(?m)^complete -F _imux imux$", &scripts["bash"]);
    t.matches("fish switches file completion off", "(?m)^complete -c imux -f$", &scripts["fish"]);
    for sh in ["zsh", "bash", "fish", "pwsh", "nu"] {
        let s = &scripts[sh];
        t.ok(&format!("{sh} offers commands"), s.contains("new-window"));
        t.ok(&format!("{sh} offers client words"), s.contains("kill-server"));
        t.ok(&format!("{sh} offers an option"), s.contains("status-position"));
        t.ok(&format!("{sh} offers a hook"), s.contains("alert-bell"));
        t.ok(&format!("{sh} offers the shells of completions"), s.contains("zsh"));
    }
    for sh in ["zsh", "bash", "fish"] {
        if which(sh).is_some() {
            let st = std::process::Command::new(sh).arg("-n").arg(e.path(&format!("c.{sh}"))).output().unwrap();
            t.ok(&format!("{sh} accepts its script"), st.status.success());
        } else {
            println!("skip no {sh}: its syntax check");
        }
    }
    let r = e.run(&["completions", "csh"]);
    t.ok("an unknown shell is refused", r.code != 0);
    t.ok("with the shells named", r.all().contains("zsh, bash, fish, pwsh, nu"));
    let r = e.run(&["completions", "ksh"]);
    t.ok("ksh is refused", r.code != 0);
    t.ok("with the reason", r.all().contains("no programmable completion"));
    t.ok("no shell word is refused", e.run(&["completions"]).code != 0);
    t.ok("completions started no server", !sockdir.join("default").exists());
    if which("bash").is_some() {
        let cb = p(&e.path("c.bash"));
        let reply = |words: &str, cword: u32| {
            let script = format!("source '{cb}'; COMP_WORDS=({words}); COMP_CWORD={cword}; _imux; printf '%s\\n' \"${{COMPREPLY[@]}}\"");
            let o = std::process::Command::new("bash").arg("-c").arg(script).output().unwrap();
            format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
        };
        t.matches("bash completes a command word", "(?m)^new-window$", &reply("imux new-w", 1));
        t.matches("bash completes an option after set", "(?m)^status-position$", &reply("imux set -g status-p", 3));
        t.matches("bash completes a hook after set-hook", "(?m)^alert-bell$", &reply("imux set-hook alert-b", 2));
        t.matches("bash completes a sub-command", "(?m)^list$", &reply("imux agent l", 2));
    }
    t.finish();
}
