//! The command line of imux itself (src/cli.rs): every form runs without a panic, with and without
//! a server; usage errors say why and show the usage and exit 2; --help works; the words that
//! belong to the server (new -d, a lone agent, tmux-style commands) still reach it.
//! (The Rust version of tests/q_cli.sh.)
use crate::support::*;
use std::time::Duration;

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn q_cli() {
    let mut e = Env::new("qcli");
    let mut t = Check::new("q_cli");
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    let run = |args: &[&str]| e.run_for(args, Duration::from_secs(20));
    let quiet = |s: &str| !(s.contains("panicked") || s.contains("panic:") || s.contains("Mismatch between definition"));
    let forms_ok = |t: &mut Check, forms: &[&str]| {
        for form in forms {
            let words: Vec<&str> = form.split_whitespace().collect();
            t.ok(&format!("no panic: {form}"), quiet(&run(&words).all()));
        }
    };

    t.section("no server");
    forms_ok(
        &mut t,
        &[
            "--version", "-V", "version", "api version", "ls", "list-keys", "snapshots", "snapshots --history", "doctor", "completions zsh", "init zsh",
            "check-config --summary", "agent list", "agent get 1", "agent wait 1 --timeout 1", "agent prompt 1 hello", "plugin list", "worktree list",
            "save x", "restore x", "delete-snapshot x", "events --types a,b", "pane release-agent 1 --source s", "pane report-agent 1 --source s --agent a --state idle -- cmd arg",
        ],
    );
    let r = run(&["--version"]);
    t.ok("--version prints the version", r.code == 0 && regex_lite::Regex::new(r"(?m)^imux [0-9.]*$").unwrap().is_match(&r.all()));
    let r = run(&["api", "version"]);
    t.ok("api version prints api=N", r.code == 0 && regex_lite::Regex::new(r"(?m)^api=[0-9]*$").unwrap().is_match(&r.all()));

    t.section("help");
    let r = run(&["--help"]);
    let o = r.all();
    t.ok("--help lists the commands and the server's ones", r.code == 0 && o.contains("Commands:") && o.contains("split-window") && o.contains("agent"));
    t.ok("and never names another multiplexer", !o.to_lowercase().contains("tmux"));
    let r = run(&["agent", "wait", "--help"]);
    t.ok("agent wait --help shows its flags", r.code == 0 && r.all().contains("--until") && r.all().contains("--timeout"));
    let r = run(&["attach", "--help"]);
    t.ok("attach --help shows -r", r.code == 0 && r.all().contains("-r"));
    let r = run(&["-h"]);
    t.ok("-h is --help", r.code == 0 && r.all().contains("Commands:"));

    t.section("usage errors");
    let r = run(&["events", "--bogus"]);
    t.ok("events --bogus: reason, usage, exit 2", r.code == 2 && r.all().contains("unexpected argument") && r.all().contains("usage: imux events"));
    let r = run(&["agent", "prompt", "3"]);
    t.ok("agent prompt without text: usage, exit 2", r.code == 2 && r.all().contains("usage: imux agent prompt"));
    let r = run(&["agent", "wait"]);
    t.ok("agent wait without a pane: usage, exit 2", r.code == 2 && r.all().contains("usage: imux agent wait"));
    let r = run(&["attach", "-t", "w", "--bogus"]);
    t.ok("attach with an unknown flag: exit 2", r.code == 2 && r.all().contains("unexpected argument"));
    let r = run(&["worktree"]);
    t.ok("worktree without a name: usage, exit 2", r.code == 2 && r.all().contains("usage: worktree"));
    let r = run(&["worktree", "list", "a", "b"]);
    t.ok("worktree list with two words: usage, exit 2", r.code == 2 && r.all().contains("usage: worktree list"));
    let r = run(&["pane", "report-agent", "--source", "s"]);
    t.ok("pane report-agent missing flags: exit 2", r.code == 2 && r.all().contains("usage: imux pane report-agent"));
    t.ok("completions without a shell: exit 2", run(&["completions"]).code == 2);
    for (label, word) in [("a bare pane prints its help, exit 2", "pane"), ("a bare plugin prints its help, exit 2", "plugin"), ("a bare api prints its help, exit 2", "api")] {
        let r = run(&[word]);
        t.ok(label, r.code == 2 && quiet(&r.all()));
    }
    let r = run(&["nonsense"]);
    t.ok("an unknown command prints the usage, exit 2", r.code == 2 && r.all().contains("usage: imux [new"));
    let r = run(&["completions", "nosuchshell"]);
    t.ok("a bad shell name is an error, not a panic", r.code != 0 && quiet(&r.all()));
    let r = run(&["pane", "report-agent", "--source", "s", "--agent", "a", "--state", "idle"]);
    t.ok("report-agent outside a pane says there is no pane", r.code != 0 && r.all().contains("IMUX_PANE_ID"));

    t.section("with a server");
    e.session("w", "w");
    e.ready("w:", Duration::from_millis(300));
    forms_ok(
        &mut t,
        &[
            "ls", "list-keys", "snapshots", "snapshots --history", "agent list", "agent get 1", "plugin list", "worktree list", "doctor", "events --types nothing",
            "check-config --summary", "save cli1", "snapshots", "delete-snapshot cli1", "agent wait 1 --timeout 1", "agent prompt 1 hello",
        ],
    );
    let r = run(&["ls"]);
    t.ok("ls shows the session", r.code == 0 && r.all().lines().any(|l| l.starts_with('w')));
    run(&["save", "cli2"]);
    t.ok("save then snapshots lists it", run(&["snapshots"]).all().contains("cli2"));
    run(&["delete-snapshot", "cli2"]);
    t.ok("delete-snapshot removes it", !run(&["snapshots"]).all().contains("cli2"));
    let r = run(&["agent", "get", "99"]);
    t.ok("agent get of a pane that is not there is an error", r.code != 0 && quiet(&r.all()));

    t.section("the words that are the server's");
    let r = run(&["new", "-d", "-s", "viaserver"]);
    t.ok("new -d is the new-session command", r.code == 0 && run(&["has-session", "-t", "=viaserver"]).code == 0);
    let o = run(&["list-sessions", "-F", "#{session_name}"]).all();
    t.ok("a command the server runs works as a word", has_line(&o, "w") && has_line(&o, "viaserver"));
    t.ok("a lone agent is the server's command, not a panic", quiet(&run(&["agent"]).all()));
    let o = run(&["agent", "-t", "nothing"]).all();
    t.ok("agent -t x is the server's too", quiet(&o) && !o.contains("usage: imux agent"));
    let r = run(&["split-window", "-h", "-d", "-t", "w:", "-P", "-F", "#{pane_id}"]);
    t.ok("tmux-style flags such as -h reach the server untouched", r.code == 0 && r.all().lines().any(|l| l.starts_with('%')));
    let r = run(&["list-keys", "-T", "prefix"]);
    t.ok("list-keys -T is the server's command, not a flag error", r.code == 0 && r.all().contains("-T prefix") && quiet(&r.all()));
    let r = run(&["list-keys", "-T", "nonsense"]);
    t.ok("and its own errors come back", r.code != 0 && r.all().contains("unknown key table"));
    t.ok("-L is taken before the tree", run(&["-L", "other", "ls"]).all().contains("no server running"));
    e.kill_server();
    t.finish();
}
