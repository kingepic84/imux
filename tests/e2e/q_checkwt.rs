//! check-config knows the placeholders the worktree options fill in (#{name}, #{slug}, #{repo},
//! #{branch}, #{base}, #{path}) and only in those options: the same name elsewhere is still reported.
//! (The Rust version of tests/q_checkwt.sh.)
use crate::support::*;

#[test]
fn q_checkwt() {
    let e = Env::new("qcw");
    let mut t = Check::new("q_checkwt");
    let wt = e.write("wt.conf", "set -g worktree-start \"/start #{name} #{repo} #{branch} #{base} #{path}\"\nset -g worktree-setup \"echo #{name}\"\nset -g worktree-branch-format \"feature/#{name}-#{slug}\"\nset -g worktree-path \"#{repo}-worktrees/#{name}\"\n");
    let out = e.all(&["check-config", &p(&wt)]);
    t.ok("the worktree placeholders are not reported", !out.contains("unknown-variable"));
    t.ok("the file is clean", out.contains("problems: 0"));
    let other = e.write("other.conf", "set -g status-left \"#{name}\"\nset -g worktree-branch-format \"#{name}-#{repo}\"\n");
    let out = e.all(&["check-config", &p(&other)]);
    t.ok("the same name in another option is still reported", out.contains("other.conf:1: unknown-variable: name"));
    t.ok("a placeholder that belongs to another worktree option is still reported", out.contains("other.conf:2: unknown-variable: repo"));
    t.finish();
}
