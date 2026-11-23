//! P10.10, the agent skill: `imux skill` prints the file built into the binary, `imux skill path`
//! says where an agent looks for it, `imux doctor` reports whether the installed copy is there and
//! the same, and imux writes nothing outside its own state directory.
//! (The Rust version of tests/q_skill.sh.)
use crate::support::*;

#[test]
fn q_skill() {
    let mut e = Env::new("qskill");
    let mut t = Check::new("q_skill");
    for v in ["IMUX_ENV", "IMUX_PANE_ID", "IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME"] {
        e.unset(v);
    }
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    e.set("HOME", &p(&home));
    let src = std::fs::read(format!("{}/skills/imux/SKILL.md", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let out = e.command(BIN).arg("skill").output().unwrap();
    t.ok("imux skill prints the shipped file, byte for byte", out.status.success() && out.stdout == src);
    let r = e.run(&["skill", "path"]);
    t.ok("skill path says where an agent looks", r.code == 0 && r.text() == p(&home.join(".claude/skills/imux/SKILL.md")));
    t.ok("a word the tree does not know is a usage error", e.run(&["skill", "bogus"]).code == 2);
    let r = e.run(&["skill", "--help"]);
    t.ok("skill has its help", r.code == 0 && r.all().contains("path"));
    let r = e.run(&["doctor"]);
    t.matches("doctor says the skill is not installed", "(?m)^skill: not installed", &r.out);
    t.ok("and that nothing was written for it", !home.join(".claude").exists());
    std::fs::create_dir_all(home.join(".claude/skills/imux")).unwrap();
    std::fs::write(home.join(".claude/skills/imux/SKILL.md"), "old\n").unwrap();
    t.matches("doctor says an installed copy differs", "(?m)^skill: installed at .* but different", &e.run(&["doctor"]).out);
    std::fs::write(home.join(".claude/skills/imux/SKILL.md"), &src).unwrap();
    t.matches("and when it is the same", "(?m)^skill: installed at .*, the same as this imux", &e.run(&["doctor"]).out);
    t.ok("completions know skill", e.run(&["completions", "zsh"]).out.contains("skill"));
    t.finish();
}
