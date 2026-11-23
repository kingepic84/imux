//! imux setup says nothing about the manpage, and writes nothing, when the manpage came with the
//! binary (Homebrew puts it in the keg next to bin/imux and links the binary). An explicit
//! --man-dir still installs there, and without such a page it installs as always. HOME is private.
//! (The Rust version of tests/q_setup_brew.sh.)
use crate::support::*;
use std::path::Path;

fn imux(e: &Env, exe: &Path, args: &[&str]) -> Res {
    let out = e.command(&p(exe)).args(args).output().expect("imux runs");
    Res { code: out.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&out.stdout).into_owned(), err: String::from_utf8_lossy(&out.stderr).into_owned() }
}

#[test]
fn q_setup_brew() {
    let mut e = Env::new("qsb");
    let mut t = Check::new("q_setup_brew");
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    e.set("HOME", &p(&home));
    for v in ["XDG_DATA_HOME", "ZDOTDIR", "XDG_CONFIG_HOME"] {
        e.unset(v);
    }
    e.set("SHELL", "/bin/zsh");
    let man = home.join(".local/share/man/man1/imux.1");
    let keg = e.path("Cellar/imux/0.1.0");
    std::fs::create_dir_all(keg.join("bin")).unwrap();
    std::fs::create_dir_all(e.path("prefix/bin")).unwrap();
    std::fs::copy(BIN, keg.join("bin/imux")).unwrap();
    std::os::unix::fs::symlink(keg.join("bin/imux"), e.path("prefix/bin/imux")).unwrap();
    let exe = e.path("prefix/bin/imux");

    t.section("no page next to the binary: installed as always");
    let r = imux(&e, &exe, &["setup", "--no-skill", "--no-shell"]);
    t.ok("the manpage is installed", std::fs::metadata(&man).map(|m| m.len() > 0).unwrap_or(false));
    t.ok("it says so", r.all().contains("manual: installed"));
    let _ = std::fs::remove_dir_all(home.join(".local"));

    t.section("a page in the keg, run through the symlink");
    std::fs::create_dir_all(keg.join("share/man/man1")).unwrap();
    std::fs::write(keg.join("share/man/man1/imux.1"), ".TH IMUX 1\n").unwrap();
    let r = imux(&e, &exe, &["setup", "--no-skill", "--no-shell"]);
    t.ok("setup succeeds", r.code == 0);
    t.ok("it prints nothing", r.all().is_empty());
    t.ok("nothing is written under HOME", !man.exists());

    t.section("the other steps still run");
    let skill = e.path("skill/SKILL.md");
    let r = imux(&e, &exe, &["setup", "--no-shell", "--skill-path", &p(&skill)]);
    t.ok("the skill is installed", std::fs::metadata(&skill).map(|m| m.len() > 0).unwrap_or(false) && r.all().contains("skill: installed"));
    t.ok("and the manpage is still not mentioned", !r.all().to_lowercase().contains("manual"));

    t.section("an explicit --man-dir installs there anyway");
    let r = imux(&e, &exe, &["setup", "--no-skill", "--no-shell", "--man-dir", &p(&e.path("chosen"))]);
    t.ok("the page is written where it was asked", e.path("chosen/man1/imux.1").metadata().map(|m| m.len() > 0).unwrap_or(false) && r.all().contains("manual: installed"));

    t.section("our own copy under HOME is still kept up to date");
    std::fs::create_dir_all(man.parent().unwrap()).unwrap();
    std::fs::write(&man, "old\n").unwrap();
    let r = imux(&e, &exe, &["setup", "--no-skill", "--no-shell"]);
    let head: String = std::fs::read_to_string(&man).unwrap_or_default().chars().take(200).collect();
    t.ok("it is updated", r.all().contains("manual: updated") && head.lines().any(|l| l.starts_with(".TH")));
    t.finish();
}
