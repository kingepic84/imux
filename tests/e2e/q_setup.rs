//! imux setup puts the manual and the agent skill where they are read, and leaves them alone when
//! they are already right. HOME is a private directory, so nothing outside the test is touched.
//! (The Rust version of tests/q_setup.sh.)
use crate::support::*;

fn count(text: &str, needle: &str) -> usize {
    text.lines().filter(|l| l.contains(needle)).count()
}

#[test]
fn q_setup() {
    let mut e = Env::new("qsetup");
    let mut t = Check::new("q_setup");
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    e.set("HOME", &p(&home));
    for v in ["XDG_DATA_HOME", "ZDOTDIR", "XDG_CONFIG_HOME"] {
        e.unset(v);
    }
    e.set("SHELL", "/bin/zsh");
    let d = p(&e.dir);
    let man = home.join(".local/share/man/man1/imux.1");
    let skill_path = e.out(&["skill", "path"]);
    let skill = std::path::PathBuf::from(&skill_path);
    let nonempty = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false);
    let first_line = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default().lines().next().unwrap_or("").to_string();
    let zshrc = home.join(".zshrc");
    let rz = || std::fs::read_to_string(&zshrc).unwrap_or_default();

    t.section("first run");
    let r = e.run(&["setup"]);
    t.ok("setup succeeds", r.code == 0);
    t.ok("the manual is installed", nonempty(&man));
    t.ok("the manual is a man page", std::fs::read_to_string(&man).unwrap_or_default().lines().take(5).any(|l| l.starts_with(".TH")));
    t.ok("the skill is installed where skill path says", nonempty(&skill) && skill_path == p(&home.join(".claude/skills/imux/SKILL.md")));
    t.ok("the skill is what the binary prints", e.out(&["skill"]) == std::fs::read_to_string(&skill).unwrap_or_default().trim_end_matches('\n'));
    t.ok("it says what it did", r.all().contains("manual: installed") && r.all().contains("skill: installed"));
    t.matches("doctor sees the skill as installed", "skill.*the same as this imux", &e.all(&["doctor"]));

    t.section("second run");
    let r = e.run(&["setup"]);
    t.ok("nothing is rewritten", r.all().contains("manual: already installed") && r.all().contains("skill: already installed"));

    t.section("an older copy is brought up to date");
    std::fs::write(&skill, "old\n").unwrap();
    let r = e.run(&["setup"]);
    t.ok("the skill is updated", r.all().contains("skill: updated") && e.out(&["skill"]) == std::fs::read_to_string(&skill).unwrap_or_default().trim_end_matches('\n'));

    t.section("choosing");
    let _ = std::fs::remove_file(&man);
    let _ = std::fs::remove_file(&skill);
    e.run(&["setup", "--no-man"]);
    t.ok("--no-man leaves the manual out", !man.exists() && nonempty(&skill));
    let _ = std::fs::remove_file(&skill);
    e.run(&["setup", "--no-skill", "--man-dir", &p(&e.path("elsewhere"))]);
    t.ok("--no-skill and --man-dir", nonempty(&e.path("elsewhere/man1/imux.1")) && !skill.exists());
    e.run(&["setup", "--no-man", "--skill-path", &p(&e.path("my/SKILL.md"))]);
    t.ok("--skill-path", nonempty(&e.path("my/SKILL.md")));

    t.section("the shell startup file");
    let _ = std::fs::remove_file(&zshrc);
    let _ = std::fs::remove_dir_all(home.join(".config"));
    let r = e.run(&["setup", "--no-man", "--no-skill"]);
    t.ok("the zsh line is added to .zshrc", count(&rz(), "imux init zsh") == 1 && rz().lines().any(|l| l == "eval \"$(imux init zsh)\""));
    t.ok("and the report shows the line", r.all().contains("shell: added to"));
    let r = e.run(&["setup", "--no-man", "--no-skill"]);
    t.ok("a second run adds nothing", count(&rz(), "imux init") == 1 && r.all().contains("shell: already set up"));
    e.run(&["setup", "--no-man", "--no-skill", "--shell", "fish"]);
    t.ok("--shell fish uses config.fish and fish syntax", std::fs::read_to_string(home.join(".config/fish/config.fish")).unwrap_or_default().lines().any(|l| l == "imux init fish | source"));
    e.run_env(&["setup", "--no-man", "--no-skill"], &[("SHELL", "/bin/bash")]);
    t.ok("bash goes to .bashrc", std::fs::read_to_string(home.join(".bashrc")).unwrap_or_default().lines().any(|l| l == "eval \"$(imux init bash)\""));
    std::fs::write(home.join(".keep"), "export KEEP=1\n").unwrap();
    std::fs::copy(home.join(".keep"), &zshrc).unwrap();
    e.run(&["setup", "--no-man", "--no-skill"]);
    t.ok("an existing .zshrc keeps its text, first", first_line(&zshrc) == "export KEEP=1" && rz().contains("imux init zsh"));
    let _ = std::fs::remove_file(&zshrc);
    e.run(&["setup", "--no-man", "--no-skill", "--no-shell"]);
    t.ok("--no-shell touches no startup file", !zshrc.exists());
    let r = e.run_env(&["setup", "--no-man", "--no-skill"], &[("SHELL", "/usr/bin/nu")]);
    t.ok("a shell imux init does not know is said, not guessed", r.all().contains("not for /usr/bin/nu") && !zshrc.exists());
    e.run_env(&["setup", "--no-man", "--no-skill"], &[("ZDOTDIR", &format!("{d}/zd"))]);
    t.ok("ZDOTDIR moves the zsh file", std::fs::read_to_string(e.path("zd/.zshrc")).unwrap_or_default().contains("imux init zsh"));
    let _ = std::fs::remove_file(&zshrc);

    t.section("a symlink is not written through");
    std::fs::create_dir_all(e.path("dots")).unwrap();
    std::fs::write(e.path("dots/skill"), "mine\n").unwrap();
    let _ = std::fs::remove_file(&skill);
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(e.path("dots/skill"), &skill).unwrap();
    let r = e.run(&["setup", "--no-man"]);
    t.ok("the link is left alone and says so", r.all().contains("symlink") && e.read("dots/skill") == "mine\n");

    t.section("it needs a home");
    let out = e.command(BIN).env_remove("HOME").arg("setup").output().unwrap();
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    t.ok("no HOME is an error that says so", !out.status.success() || all.contains("HOME"));
    t.ok("and exits non-zero", !out.status.success());
    t.finish();
}
