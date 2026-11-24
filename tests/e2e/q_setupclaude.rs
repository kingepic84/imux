//! `imux setup` puts the Claude Code hook (a script and a settings.json that calls it) next to the
//! skill, adds the hook to a settings.json that is already there (keeping the rest, with a copy of the old file), and leaves Claude Code alone with
//! `--no-claude-hooks` or `--no-skill`; `--claude-dir` names another directory.
use crate::support::*;

#[test]
fn q_setupclaude() {
    let mut e = Env::new("qsc");
    e.set("IMUX_CONFIG", "/dev/null");
    let mut t = Check::new("q_setupclaude");
    let home = e.path("home");
    std::fs::create_dir_all(&home).unwrap();
    e.set("HOME", &p(&home));
    for v in ["XDG_DATA_HOME", "ZDOTDIR", "XDG_CONFIG_HOME"] {
        e.unset(v);
    }
    let script = home.join(".claude/imux-claude-hook.sh");
    let settings = home.join(".claude/settings.json");

    t.section("a fresh home");
    let r = e.run(&["setup", "--no-man", "--no-shell"]);
    t.ok("setup succeeds", r.code == 0);
    t.matches("it says the script is installed", "claude hook: installed .*imux-claude-hook.sh", &r.all());
    t.ok("the script is there and is the shipped one", std::fs::read_to_string(&script).map(|s| s.starts_with("#!/bin/sh") && s.contains("claude --resume")).unwrap_or(false));
    let text = std::fs::read_to_string(&settings).unwrap_or_default();
    t.ok("settings.json calls it for the name and the way back", text.contains("sh ~/.claude/imux-claude-hook.sh session") && text.contains("sh ~/.claude/imux-claude-hook.sh resume"));
    t.ok("the skill is installed beside it", home.join(".claude/skills/imux/SKILL.md").exists());
    let r = e.run(&["setup", "--no-man", "--no-shell"]);
    t.matches("a second run changes nothing", "claude hook: already installed", &r.all());
    t.matches("and says settings.json already calls the script", "already calls the script", &r.all());

    t.section("a settings.json of the person's own");
    std::fs::remove_file(&script).unwrap();
    let mine = "{\"permissions\":{\"allow\":[\"Bash(ls)\"]}}\n";
    std::fs::write(&settings, mine).unwrap();
    let r = e.run(&["setup", "--no-man", "--no-shell"]);
    let now = std::fs::read_to_string(&settings).unwrap();
    t.ok("the hook is added to it", now.contains("sh ~/.claude/imux-claude-hook.sh session") && now.contains("sh ~/.claude/imux-claude-hook.sh resume"));
    t.ok("and what was in it stays", now.contains("\"permissions\":{\"allow\":[\"Bash(ls)\"]}"));
    t.matches("it says so", "added the hook to .*settings.json \\(SessionStart and UserPromptSubmit\\)", &r.all());
    t.ok("the file as it was is kept", std::fs::read_to_string(home.join(".claude/settings.json.imux-backup")).unwrap() == mine);
    t.ok("the script is installed", script.exists());
    let r = e.run(&["setup", "--no-man", "--no-shell"]);
    t.matches("a second run changes nothing", "already calls the script", &r.all());
    t.ok("the file is the same", std::fs::read_to_string(&settings).unwrap() == now);
    std::fs::write(&settings, "{ // not strict JSON\n}").unwrap();
    let r = e.run(&["setup", "--no-man", "--no-shell"]);
    t.ok("a file that cannot be read safely is left as it is", std::fs::read_to_string(&settings).unwrap() == "{ // not strict JSON\n}");
    t.matches("and the entries are printed", "could not be changed safely.*\\n(.*\\n)*.*imux-claude-hook.sh resume", &r.all());

    t.section("the ways to leave it alone");
    let other = e.path("other");
    std::fs::create_dir_all(&other).unwrap();
    e.set("HOME", &p(&other));
    let r = e.run(&["setup", "--no-man", "--no-shell", "--no-skill"]);
    t.ok("--no-skill leaves Claude Code alone", r.code == 0 && !other.join(".claude").exists() && !r.all().contains("claude hook"));
    let r = e.run(&["setup", "--no-man", "--no-shell", "--no-claude-hooks"]);
    t.ok("--no-claude-hooks installs the skill only", r.code == 0 && other.join(".claude/skills/imux/SKILL.md").exists() && !other.join(".claude/imux-claude-hook.sh").exists() && !r.all().contains("claude hook"));
    let elsewhere = e.path("elsewhere");
    let r = e.run(&["setup", "--no-man", "--no-shell", "--claude-dir", &p(&elsewhere)]);
    t.ok("--claude-dir names the directory", r.code == 0 && elsewhere.join("imux-claude-hook.sh").exists() && elsewhere.join("settings.json").exists());
    t.matches("and the settings point at the whole path", "sh .*elsewhere/imux-claude-hook.sh session", &std::fs::read_to_string(elsewhere.join("settings.json")).unwrap_or_default());
    t.finish();
}
