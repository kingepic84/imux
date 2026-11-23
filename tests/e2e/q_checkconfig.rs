//! `imux check-config`: what a configuration holds that imux does not take, by file and line, with
//! counts; a sourced file is read; the exit status; --summary; the default file; and that it
//! starts no server, writes no socket directory and changes no file.
//! (The Rust version of tests/q_checkconfig.sh.)
use crate::support::*;

#[test]
fn q_checkconfig() {
    let e = Env::new("qcc");
    let mut t = Check::new("q_checkconfig");
    let d = p(&e.dir);
    let other = e.write("other.conf", "set -g nosuchoption 1\n");
    let main = e.write(
        "main.conf",
        &format!("set -g status-left \"#{{N:session_name}} #{{nope}}\"\nbind x frobnicate\nsplit-window -Q\nbind -T copy-mode-vi y send -X frobnicate\nset -g @plugin y\nsource-file {d}/other.conf\nsource-file {d}/gone.conf\nset -g mouse on\n"),
    );
    let clean = e.write("clean.conf", "set -g mouse on\nbind r source-file ~/.imux.conf\n");
    let before = (std::fs::read(&main).unwrap(), std::fs::read(&other).unwrap());
    let r = e.run(&["check-config", &p(&main)]);
    let out = r.all();
    t.ok("a configuration with problems exits 1", r.code == 1);
    t.ok(
        "every problem is named with its file and line",
        out.contains("main.conf:2: unknown-command: frobnicate")
            && out.contains("main.conf:3: unknown-flag: split-window -Q")
            && out.contains("main.conf:1: unknown-variable: nope")
            && out.contains("main.conf:1: unknown-modifier: N:"),
    );
    t.ok("a sourced file is read, with its own file and line", out.contains("other.conf:1: unknown-option: nosuchoption"));
    t.ok(
        "what is understood and does nothing is listed as information",
        out.contains("ignored-command: bind") && out.contains("user-option: @plugin") && regex_lite::Regex::new("missing-file: .*gone.conf").unwrap().is_match(&out),
    );
    t.ok(
        "the counts by kind and the totals follow",
        out.contains("\nfiles: 2, lines: 9, problems: ") || out.starts_with("files: 2, lines: 9, problems: "),
    );
    t.matches("(and the count of unknown flags)", "(?m)^unknown-flag: 1$", &out);
    let sum = e.all(&["check-config", "--summary", &p(&main)]);
    t.ok("--summary leaves out the line per problem", !sum.contains("main.conf:2:") && sum.lines().any(|l| l == "unknown-command: 1"));
    let r = e.run(&["check-config", &p(&clean)]);
    t.ok("a clean file exits 0", r.code == 0 && r.all().contains("problems: 0"));
    let r = e.run_env(&["check-config"], &[("IMUX_CONFIG", &p(&clean))]);
    t.ok("without a file it reads the configuration imux would read", r.all().contains("files: 1"));
    let r = e.run(&["check-config", &p(&e.path("missing.conf"))]);
    t.ok("a file that cannot be read is a problem", r.code == 1 && r.all().contains("unreadable"));
    t.ok("no file was changed", (std::fs::read(&main).unwrap(), std::fs::read(&other).unwrap()) == before);
    let any_sockdir = std::fs::read_dir(&e.dir).unwrap().filter_map(|f| f.ok()).any(|f| f.file_name().to_string_lossy().starts_with("imux-"));
    t.ok("no server was started and no socket directory made", !any_sockdir && e.run(&["ls"]).code != 0);
    t.finish();
}
