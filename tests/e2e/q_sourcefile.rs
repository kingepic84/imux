//! source-file -q (a missing file is no error), -n (read and check only), -v (log each line), and
//! a pattern in the file name. (The Rust version of tests/q_sourcefile.sh.)
use crate::support::*;

#[test]
fn q_sourcefile() {
    let mut e = Env::new("qsf2");
    let mut t = Check::new("q_sourcefile");
    let log = e.path("log");
    e.set("IMUX_LOG_FILE", &p(&log));
    let d = e.path("conf");
    std::fs::create_dir_all(&d).unwrap();
    let w = |n: &str, text: &str| std::fs::write(d.join(n), text).unwrap();
    w("a1.conf", "set -g @one 1\n");
    w("a2.conf", "set -g @two 2\n");
    w("b.conf", "set -g @other 9\n");
    w(".a3.conf", "set -g @hidden 7\n");
    w("bad.conf", "set -g @three 3\nfrobnicate now\n");
    w("ok.conf", "set -g @four 4\n");
    let dp = p(&d);
    e.session("sf", "w");
    let opt = |n: &str| e.out(&["show-options", "-gqv", n]);
    let r = e.run(&["source-file", &format!("{dp}/nothing.conf")]);
    t.ok("a missing file is an error", r.code != 0 || r.all().contains("No such file"));
    let r = e.run(&["source-file", "-q", &format!("{dp}/nothing.conf")]);
    t.ok("-q makes a missing file no error", r.code == 0 && r.all().is_empty());
    e.run(&["source-file", &format!("{dp}/a*.conf")]);
    t.ok("a pattern reads every match", opt("@one") == "1" && opt("@two") == "2");
    t.ok("and no other file", opt("@other").is_empty());
    t.ok("a hidden file is not matched by a plain pattern", opt("@hidden").is_empty());
    e.run(&["source-file", ".a*.conf"]);
    e.run(&["source-file", &format!("{dp}/.a*.conf")]);
    t.eq("a pattern that starts with a dot matches hidden files", opt("@hidden"), "7".to_string());
    let r = e.run(&["source-file", &format!("{dp}/z*.conf")]);
    t.ok("a pattern without a match is an error", r.all().contains("no file matches"));
    let r = e.run(&["source-file", "-q", &format!("{dp}/z*.conf")]);
    t.ok("-q makes it quiet", r.code == 0 && r.all().is_empty());
    let r = e.run(&["source-file", &format!("{}/co*/a1.conf", p(&e.dir))]);
    t.ok("a pattern in a directory part matches nothing", r.all().contains("no file matches"));
    let r = e.run(&["source-file", "-n", &format!("{dp}/ok.conf")]);
    t.ok("-n of a good file is quiet", r.code == 0 && r.all().is_empty());
    t.ok("and changes nothing", opt("@four").is_empty());
    let r = e.run(&["source-file", "-n", &format!("{dp}/bad.conf")]);
    t.ok("-n names the file and line of a bad one", r.all().contains("bad.conf:2"));
    t.ok("and changes nothing there either", opt("@three").is_empty());
    let r = e.run(&["source-file", "-nq", &format!("{dp}/ok.conf")]);
    t.ok("flags can be together", r.code == 0 && opt("@four").is_empty());
    e.run(&["source-file", "-v", &format!("{dp}/ok.conf")]);
    t.ok("-v reads and applies", opt("@four") == "4");
    t.matches("and logs the line", "source-file: .*ok.conf:1: set -g @four 4", &std::fs::read_to_string(&log).unwrap_or_default());
    let r = e.run(&["source-file", "-z", &format!("{dp}/ok.conf")]);
    t.ok("an unknown flag is a usage error", r.all().contains("usage: source-file"));
    let main = e.write("main.conf", &format!("source-file -q {dp}/absent.conf\nsource-file {dp}/ghost.conf\n"));
    let r = e.all(&["check-config", &p(&main)]);
    t.ok("check-config skips a quiet missing file", !r.contains("absent.conf"));
    t.ok("and reports a loud one", r.contains("ghost.conf"));
    let glob = e.write("glob.conf", &format!("source-file {dp}/a*.conf\n"));
    w("a9.conf", "set -g nonsense-option 1\n");
    t.ok("check-config reads every file a pattern names", e.all(&["check-config", &p(&glob)]).contains("nonsense-option"));
    e.kill_server();
    t.finish();
}
