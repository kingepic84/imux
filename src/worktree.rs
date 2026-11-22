//! `worktree`: a git worktree for a name (a ticket key, say) cut from the up-to-date remote base
//! branch, without touching the checkout the command was run from. This module is the part that
//! needs no server: checking names, choosing the base, the pane layout and the git commands.
//! `server/worktree.rs` runs it on a thread and opens the window.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use regex_lite::Regex;

use crate::config::format;
use crate::log;
use crate::session::layout::{Shape, Split};

const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const GIT_TIMEOUT: Duration = Duration::from_secs(20);
/// Remote branches tried, in order, when `origin/HEAD` is not known.
const COMMON_BASES: &[&str] = &["main", "master", "develop", "trunk"];

/// What a pane of the new window runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneKind {
    Claude,
    Shell,
}

/// The options that shape a worktree, copied when the command is given.
#[derive(Clone, Debug)]
pub struct Settings {
    pub base: String,
    pub detach: bool,
    pub pattern: String,
    pub path: String,
    pub branch_format: String,
    pub layout: String,
    pub command: String,
    pub start: String,
    /// Typed before `command` (and only if it worked); see `server::worktree::compose_run`.
    pub setup: String,
    /// `--setup` was given: run the setup also in a worktree that already existed.
    pub setup_forced: bool,
}

pub struct Request {
    /// Repository given by the user; else the one `from` is in.
    pub repo: Option<PathBuf>,
    pub from: PathBuf,
    pub name: String,
    pub base: Option<String>,
    pub detach: bool,
    pub pattern: String,
    pub path_format: String,
    pub branch_format: String,
    pub slug: Option<String>,
    pub base_option: String,
}

#[derive(Debug)]
pub struct Prepared {
    pub root: PathBuf,
    pub path: PathBuf,
    pub name: String,
    /// The branch checked out, `None` when detached or when an existing worktree was reused.
    pub branch: Option<String>,
    pub base: String,
    pub reused: bool,
    pub warnings: Vec<String>,
}

/// Rules git itself applies to ref names, plus no spaces or leading dashes so the name is safe as
/// an argument and as a directory name.
pub fn check_ref(name: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("bad worktree name {name:?}: {why}"));
    if name.is_empty() || name.len() > 100 {
        return bad("must be 1 to 100 characters");
    }
    if name.starts_with(['-', '.', '/']) || name.ends_with(['/', '.']) || name.ends_with(".lock") {
        return bad("cannot start with - . / or end with / . .lock");
    }
    if name == "@" || name.contains("..") || name.contains("//") || name.contains("@{") || name.contains("/.") {
        return bad("not a valid git ref name");
    }
    if name.chars().any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c)) {
        return bad("contains a character git refs cannot have");
    }
    Ok(())
}

/// `name` must be a valid ref name and match `pattern` as a whole.
pub fn check_name(name: &str, pattern: &str) -> Result<(), String> {
    check_ref(name)?;
    let re = compile(pattern)?;
    if re.is_match(name) { Ok(()) } else { Err(format!("{name:?} does not match worktree-name-pattern {pattern}")) }
}

/// A pattern that has to match the whole name.
fn compile(pattern: &str) -> Result<Regex, String> {
    Regex::new(&format!("^(?:{pattern})$")).map_err(|e| format!("bad regular expression {pattern:?}: {e}"))
}

/// For `set worktree-name-pattern`.
pub fn check_pattern(pattern: &str) -> Result<(), String> {
    compile(pattern).map(|_| ())
}

/// Directory of the worktree: `fmt` with `#{repo}` and `#{name}` (slashes become dashes), `~`
/// expanded, a relative result placed beside the repository.
pub fn worktree_path(fmt: &str, root: &Path, name: &str) -> PathBuf {
    let repo = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir_name = name.replace('/', "-");
    let text = format::expand(fmt, &|v| match v {
        "repo" => Some(repo.clone()),
        "name" => Some(dir_name.clone()),
        _ => None,
    });
    let path = match text.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map_or_else(|| PathBuf::from(&text), |h| PathBuf::from(h).join(rest)),
        None => PathBuf::from(&text),
    };
    if path.is_absolute() { path } else { root.parent().unwrap_or(root).join(path) }
}

/// Name of the branch: `fmt` with `#{name}` and `#{slug}`. Without a slug, separators left dangling
/// at the end (`feature/ABC-1-`) are dropped. The result has to be a valid ref name.
pub fn branch_name(fmt: &str, name: &str, slug: Option<&str>) -> Result<String, String> {
    let slug = slug.unwrap_or_default();
    let text = format::expand(fmt, &|v| match v {
        "name" => Some(name.to_string()),
        "slug" => Some(slug.to_string()),
        _ => None,
    });
    let text = if slug.is_empty() { text.trim_end_matches(['-', '_', '.']).to_string() } else { text };
    check_ref(&text).map_err(|e| format!("worktree-branch-format {fmt:?}: {e}"))?;
    Ok(text)
}

/// For `set worktree-branch-format`: it must use `#{name}` and give a valid branch for a sample.
pub fn check_branch_format(fmt: &str) -> Result<(), String> {
    if !fmt.contains("#{name}") {
        return Err("must contain #{name}, or every worktree would get the same branch".to_string());
    }
    branch_name(fmt, "ABC-1", None)?;
    branch_name(fmt, "ABC-1", Some("a-slug"))?;
    Ok(())
}

/// The remote ref to cut from: the argument, else the option, else `origin/HEAD`, else the first
/// of main, master, develop, trunk that exists.
pub fn base_ref(
    arg: Option<&str>,
    option: &str,
    head: Option<String>,
    exists: &dyn Fn(&str) -> bool,
) -> Result<String, String> {
    let wanted = arg.filter(|s| !s.is_empty()).or(Some(option).filter(|s| !s.is_empty()));
    if let Some(branch) = wanted {
        let ref_name = format!("origin/{}", branch.strip_prefix("origin/").unwrap_or(branch));
        return if exists(&ref_name) {
            Ok(ref_name)
        } else {
            Err(format!("no {ref_name}: is there an origin remote with that branch?"))
        };
    }
    if let Some(head) = head.filter(|h| exists(h)) {
        return Ok(head);
    }
    COMMON_BASES
        .iter()
        .map(|b| format!("origin/{b}"))
        .find(|r| exists(r))
        .ok_or_else(|| "cannot tell the base branch: set worktree-base or give one".to_string())
}

/// `claude|shell`, `claude|shell/shell`: `|` puts panes side by side, `/` stacks them (`/` binds
/// tighter), several of one kind share the space evenly.
pub fn parse_layout(text: &str) -> Result<Shape<PaneKind>, String> {
    let side: Result<Vec<_>, String> = text.split('|').map(stacked).collect();
    Ok(nest(side?, Split::SideBySide))
}

fn stacked(text: &str) -> Result<Shape<PaneKind>, String> {
    let items: Result<Vec<_>, String> = text.split('/').map(atom).collect();
    Ok(nest(items?, Split::Stacked))
}

fn atom(text: &str) -> Result<Shape<PaneKind>, String> {
    match text.trim() {
        "claude" => Ok(Shape::Leaf(PaneKind::Claude)),
        "shell" => Ok(Shape::Leaf(PaneKind::Shell)),
        other => Err(format!("worktree-layout: unknown pane {other:?}, use claude or shell")),
    }
}

fn nest(mut items: Vec<Shape<PaneKind>>, kind: Split) -> Shape<PaneKind> {
    let first = items.remove(0);
    if items.is_empty() {
        return first;
    }
    let ratio = 1.0 / (items.len() + 1) as f32;
    Shape::Branch { kind, ratio, a: Box::new(first), b: Box::new(nest(items, kind)) }
}

/// Repositories one level below `dir`: visible subdirectories holding `.git` (a directory, or a
/// file for a worktree), sorted by name without regard to case.
pub fn scan_repos(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut repos = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log::warn(&format!("scanning {}: {e}", dir.display()));
                continue;
            }
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() && path.join(".git").exists() {
            repos.push(path);
        }
    }
    repos.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    Ok(repos)
}

/// Variables that point git at a repository other than the one in `dir`. The server keeps the
/// environment of whatever terminal started it, and a `GIT_DIR` left over from there would send
/// every worktree operation to the wrong repository.
const GIT_REPOSITORY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// `git args` to run in `dir`, with the repository variables removed and password prompts off.
fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    // `core.fsmonitor` is a command that `git status` runs; a repository that was not cloned by
    // git (a copied directory, an archive) can carry one in its config.
    cmd.args(["-c", "core.fsmonitor=false"]).args(args).current_dir(dir).env("GIT_TERMINAL_PROMPT", "0");
    for name in GIT_REPOSITORY_ENV {
        cmd.env_remove(name);
    }
    cmd
}

/// Run git in `dir`; stdout on success, git's own message on failure. Never waits longer than
/// `timeout` and never asks for a password.
fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let started = Instant::now();
    let shown = args.join(" ");
    log::info(&format!("git {shown} in {} (service git, local repository and its origin remote)", dir.display()));
    let mut child = git_command(dir, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut p) = pipe
                && let Err(e) = p.read_to_string(&mut text)
            {
                log::warn(&format!("reading git output: {e}"));
            }
            text
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= timeout => {
                if let Err(e) = child.kill() {
                    log::warn(&format!("stop git: {e}"));
                }
                if let Err(e) = child.wait() {
                    log::warn(&format!("reap git: {e}"));
                }
                break Err(format!("git {shown} timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => break Err(format!("waiting for git: {e}")),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let ms = started.elapsed().as_millis();
    match status {
        Ok(s) if s.success() => {
            log::info(&format!("git {shown}: ok duration_ms={ms}"));
            Ok(stdout)
        }
        Ok(s) => {
            let why = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string();
            log::warn(&format!("git {shown}: {s} duration_ms={ms}"));
            Err(format!("git {shown}: {why}"))
        }
        Err(e) => {
            log::warn(&format!("{e} duration_ms={ms}"));
            Err(e)
        }
    }
}

fn exists(dir: &Path, rev: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")], GIT_TIMEOUT).is_ok()
}

/// The main work tree of the repository `dir` is in (also from inside one of its worktrees).
fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let common = git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"], GIT_TIMEOUT)
        .map_err(|_| format!("{} is not in a git repository", dir.display()))?;
    let common = PathBuf::from(common.trim());
    match common.file_name() {
        Some(n) if n == ".git" => common.parent().map(Path::to_path_buf).ok_or_else(|| "odd git directory".to_string()),
        _ => Err(format!("{} is a bare or unusual repository, which worktree does not handle", common.display())),
    }
}

/// Worktree paths of the repository, from `git worktree list --porcelain`.
fn listed(root: &Path) -> Result<Vec<PathBuf>, String> {
    let text = git(root, &["worktree", "list", "--porcelain"], GIT_TIMEOUT)?;
    Ok(text.lines().filter_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from).collect())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Everything up to and including the worktree on disk. Blocking: run it off the server thread.
pub fn prepare(req: &Request) -> Result<Prepared, String> {
    check_name(&req.name, &req.pattern)?;
    let start = req.repo.as_deref().unwrap_or(&req.from);
    let root = repo_root(start)?;
    let path = worktree_path(&req.path_format, &root, &req.name);
    let mut warnings = Vec::new();

    if let Err(e) = git(&root, &["fetch", "--quiet", "origin"], FETCH_TIMEOUT) {
        warnings.push(format!("fetch failed, using the last known origin ({e})"));
    }

    if path.exists() {
        let known = listed(&root)?.iter().any(|w| same_dir(w, &path));
        if !known {
            return Err(format!("{} exists and is not a worktree of {}", path.display(), root.display()));
        }
        return Ok(Prepared { root, path, name: req.name.clone(), branch: None, base: String::new(), reused: true, warnings });
    }

    let wanted = if req.detach { None } else { Some(branch_name(&req.branch_format, &req.name, req.slug.as_deref())?) };
    let local_branch = wanted.as_ref().is_some_and(|b| exists(&root, &format!("refs/heads/{b}")));
    let path_text = path.to_string_lossy().into_owned();
    let (branch, base) = if let (true, Some(b)) = (local_branch, &wanted) {
        git(&root, &["worktree", "add", &path_text, b], GIT_TIMEOUT)?;
        (Some(b.clone()), String::new())
    } else {
        let head = git(&root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], GIT_TIMEOUT)
            .ok()
            .map(|s| s.trim().to_string());
        let base = base_ref(req.base.as_deref(), &req.base_option, head, &|r| exists(&root, r))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        match wanted {
            None => {
                git(&root, &["worktree", "add", "--detach", &path_text, &base], GIT_TIMEOUT)?;
                (None, base)
            }
            Some(b) => {
                git(&root, &["worktree", "add", "--no-track", "-b", &b, &path_text, &base], GIT_TIMEOUT)?;
                (Some(b), base)
            }
        }
    };
    Ok(Prepared { root, path, name: req.name.clone(), branch, base, reused: false, warnings })
}

/// One worktree of `git worktree list --porcelain`.
#[derive(Debug, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    /// The checked out branch without `refs/heads/`; `None` when detached or bare.
    pub branch: Option<String>,
    pub locked: bool,
    pub bare: bool,
}

/// The worktrees in the text of `git worktree list --porcelain`: blocks separated by blank lines,
/// each starting with `worktree <path>`; the first one is the main checkout.
pub fn parse_list(text: &str) -> Vec<Listed> {
    let mut out: Vec<Listed> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push(Listed { path: PathBuf::from(path), branch: None, locked: false, bare: false });
        } else if let Some(current) = out.last_mut() {
            if let Some(branch) = line.strip_prefix("branch ") {
                current.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).to_string());
            } else if line == "bare" {
                current.bare = true;
            } else if line == "locked" || line.starts_with("locked ") {
                current.locked = true;
            }
        }
    }
    out
}

/// One worktree for `imux worktree list`.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub locked: bool,
    /// The main checkout (git lists it first).
    pub main: bool,
}

/// The worktrees of the repository `repo`, or the one `from` is in, main checkout first. Blocking
/// (it runs git): not for the server loop.
pub fn list(repo: Option<&Path>, from: &Path) -> Result<Vec<Entry>, String> {
    let start = match repo {
        Some(r) => match r.strip_prefix("~") {
            Ok(rest) => std::env::var_os("HOME").map(PathBuf::from).ok_or("HOME is not set")?.join(rest),
            Err(_) => r.to_path_buf(),
        },
        None => from.to_path_buf(),
    };
    let root = repo_root(&start)?;
    let text = git(&root, &["worktree", "list", "--porcelain"], GIT_TIMEOUT)?;
    Ok(parse_list(&text)
        .into_iter()
        .enumerate()
        .map(|(i, l)| Entry { path: l.path, branch: l.branch, locked: l.locked, main: i == 0 })
        .collect())
}

/// Whether two paths are the same directory (links resolved when both exist).
pub fn same_path(a: &Path, b: &Path) -> bool {
    same_dir(a, b)
}

/// Why a worktree is not removed.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotAWorktree,
    MainCheckout,
    Locked,
    Dirty(usize),
    Unpushed(usize),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotAWorktree => write!(f, "that is not a worktree of the repository"),
            Refusal::MainCheckout => write!(f, "that is the main checkout, not a linked worktree"),
            Refusal::Locked => write!(f, "the worktree is locked (git worktree unlock first)"),
            Refusal::Dirty(n) => write!(f, "{n} changed or untracked files; commit or stash them, or use --force"),
            Refusal::Unpushed(n) => write!(f, "{n} commits are on no remote; push them, or use --force"),
        }
    }
}

/// Whether the worktree `entry` may be removed. Only a listed, linked, unlocked worktree ever
/// can; `force` waives the checks on its content (changes, commits that no remote has).
pub fn check_removal(entry: Option<&Listed>, is_main: bool, dirty: usize, unpushed: usize, force: bool) -> Result<(), Refusal> {
    let Some(entry) = entry else { return Err(Refusal::NotAWorktree) };
    if is_main {
        return Err(Refusal::MainCheckout);
    }
    if entry.bare {
        return Err(Refusal::NotAWorktree);
    }
    if entry.locked {
        return Err(Refusal::Locked);
    }
    if force {
        return Ok(());
    }
    if dirty > 0 {
        return Err(Refusal::Dirty(dirty));
    }
    if unpushed > 0 {
        return Err(Refusal::Unpushed(unpushed));
    }
    Ok(())
}

pub struct RemoveRequest {
    /// Repository given by the user; else the one `from` (or `path`) is in.
    pub repo: Option<PathBuf>,
    pub from: PathBuf,
    /// The worktree by name (placed with `path_format`), or by path when it is known, for example
    /// from the window it was opened in; with neither, the one `from` is in.
    pub name: Option<String>,
    pub path: Option<PathBuf>,
    pub path_format: String,
    pub force: bool,
}

/// A worktree that was removed. Its branch is never touched.
pub struct Removed {
    pub path: PathBuf,
    /// The path as the system resolves it, taken before the directory was gone.
    pub canonical: PathBuf,
    pub branch: Option<String>,
    pub forced: bool,
}

/// Remove a worktree with `git worktree remove`, after the checks of `check_removal`. Never deletes
/// a directory itself and never deletes the branch. Blocking: run it off the server thread.
pub fn remove(req: &RemoveRequest) -> Result<Removed, String> {
    let start = req.repo.as_deref().or(req.path.as_deref()).unwrap_or(&req.from);
    let root = repo_root(start)?;
    let target = match (&req.path, &req.name) {
        (Some(path), _) => path.clone(),
        (None, Some(name)) => {
            check_ref(name)?;
            worktree_path(&req.path_format, &root, name)
        }
        (None, None) => PathBuf::from(git(&req.from, &["rev-parse", "--show-toplevel"], GIT_TIMEOUT)?.trim()),
    };
    let entries = parse_list(&git(&root, &["worktree", "list", "--porcelain"], GIT_TIMEOUT)?);
    let entry = entries.iter().find(|e| same_dir(&e.path, &target));
    let is_main = same_dir(&target, &root);
    let present = target.is_dir();
    let (dirty, unpushed) = if present && entry.is_some() && !is_main {
        let dirty = git(&target, &["status", "--porcelain"], GIT_TIMEOUT)?.lines().filter(|l| !l.is_empty()).count();
        let unpushed = git(&target, &["rev-list", "--count", "HEAD", "--not", "--remotes"], GIT_TIMEOUT)?
            .trim()
            .parse::<usize>()
            .map_err(|_| "cannot count the commits that no remote has".to_string())?;
        (dirty, unpushed)
    } else {
        (0, 0)
    };
    check_removal(entry, is_main, dirty, unpushed, req.force).map_err(|r| format!("{}: {r}", target.display()))?;
    let canonical = target.canonicalize().unwrap_or_else(|_| target.clone());
    let path_text = target.to_string_lossy().into_owned();
    if req.force {
        git(&root, &["worktree", "remove", "--force", &path_text], GIT_TIMEOUT)?;
    } else {
        git(&root, &["worktree", "remove", &path_text], GIT_TIMEOUT)?;
    }
    if let Err(e) = git(&root, &["worktree", "prune"], GIT_TIMEOUT) {
        log::warn(&format!("worktree prune after remove: {e}"));
    }
    Ok(Removed { path: target, canonical, branch: entry.and_then(|e| e.branch.clone()), forced: req.force })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "[A-Za-z0-9][A-Za-z0-9._/-]*";

    #[test]
    fn the_default_pattern_takes_any_sane_ref_name() {
        for ok in ["ABC-123", "feature/login", "fix_1.2", "x"] {
            assert!(check_name(ok, DEFAULT).is_ok(), "{ok}");
        }
        for bad in ["", "-x", ".x", "a b", "a..b", "x/", "a//b", "a.lock", "a@{b", "a:b", "a~1", "a\nb", "@"] {
            assert!(check_name(bad, DEFAULT).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_ticket_pattern_matches_the_whole_name_only() {
        let p = "[A-Z][A-Z0-9]+-[0-9]+";
        assert!(check_name("ABC-123", p).is_ok());
        assert!(check_name("A1-9", p).is_ok());
        for bad in ["abc-123", "ABC-123x", "xABC-123", "ABC-", "ABC-123/extra"] {
            assert!(check_name(bad, p).is_err(), "{bad}");
        }
        assert!(check_name("ABC-1", "ABC-1|ABC-2").is_ok());
        assert!(check_name("ABC-12", "ABC-1|ABC-2").is_err());
    }

    #[test]
    fn a_loose_pattern_cannot_allow_an_unsafe_name() {
        for bad in ["--upload-pack=x", "a b", "../x", "x/.."] {
            assert!(check_name(bad, ".*").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_broken_pattern_is_reported_not_a_panic() {
        assert!(check_pattern("(").is_err());
        assert!(check_pattern("[A-Z]+").is_ok());
        assert!(check_name("x", "(").is_err());
    }

    #[test]
    fn the_path_goes_beside_the_repository_unless_it_is_absolute() {
        let root = Path::new("/w/proj");
        assert_eq!(worktree_path("#{repo}-worktrees/#{name}", root, "ABC-1"), PathBuf::from("/w/proj-worktrees/ABC-1"));
        assert_eq!(worktree_path("#{repo}-worktrees/#{name}", root, "feat/x"), PathBuf::from("/w/proj-worktrees/feat-x"));
        assert_eq!(worktree_path("/tmp/wt/#{name}", root, "a"), PathBuf::from("/tmp/wt/a"));
        assert_eq!(worktree_path("../wt/#{name}", root, "a"), PathBuf::from("/w/../wt/a"));
    }

    fn has(refs: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |r| refs.contains(&r)
    }

    #[test]
    fn the_base_comes_from_the_argument_then_the_option_then_origin_head_then_the_usual_names() {
        let all = has(&["origin/develop", "origin/main", "origin/master", "origin/release"]);
        let pick = |a, o, h: Option<&str>| base_ref(a, o, h.map(String::from), &all);
        assert_eq!(pick(Some("release"), "develop", Some("origin/main")).unwrap(), "origin/release");
        assert_eq!(pick(Some("origin/release"), "", None).unwrap(), "origin/release");
        assert_eq!(pick(None, "develop", Some("origin/main")).unwrap(), "origin/develop");
        assert_eq!(pick(None, "", Some("origin/main")).unwrap(), "origin/main");
        assert_eq!(pick(None, "", Some("origin/gone")).unwrap(), "origin/main");
        assert_eq!(pick(None, "", None).unwrap(), "origin/main");
        assert_eq!(base_ref(None, "", None, &has(&["origin/master", "origin/develop"])).unwrap(), "origin/master");
        assert_eq!(base_ref(None, "", None, &has(&["origin/trunk"])).unwrap(), "origin/trunk");
    }

    #[test]
    fn a_missing_base_is_an_error_that_names_it() {
        let e = base_ref(Some("nope"), "", None, &has(&["origin/main"])).unwrap_err();
        assert!(e.contains("origin/nope"), "{e}");
        assert!(base_ref(None, "", None, &has(&[])).is_err());
    }

    #[test]
    fn layouts_are_trees_of_claude_and_shell_panes() {
        use PaneKind::{Claude, Shell};
        let leaf = |k| Box::new(Shape::Leaf(k));
        assert_eq!(parse_layout("claude").unwrap(), Shape::Leaf(Claude));
        assert_eq!(
            parse_layout("claude|shell").unwrap(),
            Shape::Branch { kind: Split::SideBySide, ratio: 0.5, a: leaf(Claude), b: leaf(Shell) }
        );
        assert_eq!(
            parse_layout(" claude | shell/shell ").unwrap(),
            Shape::Branch {
                kind: Split::SideBySide,
                ratio: 0.5,
                a: leaf(Claude),
                b: Box::new(Shape::Branch { kind: Split::Stacked, ratio: 0.5, a: leaf(Shell), b: leaf(Shell) }),
            }
        );
        let Shape::Branch { ratio, .. } = parse_layout("shell|shell|shell").unwrap() else { panic!("split") };
        assert!((ratio - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn scanning_lists_visible_repositories_one_level_down_in_name_order() {
        let root = std::env::temp_dir().join(format!("imux-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["zeta/.git", "Alpha/.git", ".hidden/.git", "plain", "deep/inner/.git"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("file"), "x").unwrap();
        std::fs::create_dir_all(root.join("linked")).unwrap();
        std::fs::write(root.join("linked/.git"), "gitdir: /elsewhere").unwrap();
        let names: Vec<String> = scan_repos(&root)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Alpha", "linked", "zeta"]);
        assert!(scan_repos(&root.join("missing")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_branch_follows_the_format_with_or_without_a_slug() {
        assert_eq!(branch_name("#{name}", "ABC-1", None).unwrap(), "ABC-1");
        assert_eq!(branch_name("feature/#{name}", "ABC-1", None).unwrap(), "feature/ABC-1");
        assert_eq!(branch_name("feature/#{name}-#{slug}", "ABC-1", Some("login-fix")).unwrap(), "feature/ABC-1-login-fix");
        assert_eq!(branch_name("feature/#{name}-#{slug}", "ABC-1", None).unwrap(), "feature/ABC-1");
        assert_eq!(branch_name("feature/#{name}-#{slug}", "ABC-1", Some("")).unwrap(), "feature/ABC-1");
        assert!(branch_name("feature/#{name}-#{slug}", "ABC-1", Some("a b")).is_err());
        assert!(branch_name("#{name}", "a b", None).is_err());
    }

    #[test]
    fn a_branch_format_has_to_use_the_name_and_make_valid_refs() {
        assert!(check_branch_format("#{name}").is_ok());
        assert!(check_branch_format("feature/#{name}-#{slug}").is_ok());
        for bad in ["feature/", "feature/#{slug}", "#{name} x", "-#{name}", "#{name}..x"] {
            assert!(check_branch_format(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_bad_layout_is_an_error() {
        for bad in ["", "vim", "claude|", "|shell", "claude//shell"] {
            assert!(parse_layout(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn git_runs_without_variables_that_point_at_another_repository_and_never_prompts() {
        let cmd = git_command(Path::new("/tmp"), &["status"]);
        let envs: Vec<_> = cmd.get_envs().collect();
        for name in GIT_REPOSITORY_ENV {
            assert!(envs.iter().any(|(k, v)| k.to_str() == Some(*name) && v.is_none()), "{name} is not removed");
        }
        assert!(envs.iter().any(|(k, v)| k.to_str() == Some("GIT_TERMINAL_PROMPT") && v.is_some_and(|v| v == "0")));
    }

    #[test]
    fn git_runs_with_the_repository_s_fsmonitor_command_switched_off() {
        let cmd = git_command(Path::new("/tmp"), &["status", "--porcelain"]);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["-c", "core.fsmonitor=false", "status", "--porcelain"]);
    }

    const LIST: &str = "worktree /w/proj\nHEAD aaaa\nbranch refs/heads/main\n\n\
        worktree /w/proj-worktrees/ABC-1\nHEAD bbbb\nbranch refs/heads/feature/ABC-1\n\n\
        worktree /w/proj-worktrees/ABC-2\nHEAD cccc\ndetached\n\n\
        worktree /w/proj-worktrees/ABC-3\nHEAD dddd\nbranch refs/heads/ABC-3\nlocked moved to a usb stick\n\n\
        worktree /w/bare.git\nbare\n\n\
        worktree /w/gone\nHEAD eeee\ndetached\nprunable gitdir file points to non-existent location\n";

    #[test]
    fn the_porcelain_list_gives_path_branch_lock_and_bare() {
        let l = parse_list(LIST);
        assert_eq!(l.len(), 6);
        assert_eq!(l[0], Listed { path: "/w/proj".into(), branch: Some("main".into()), locked: false, bare: false });
        assert_eq!(l[1].branch.as_deref(), Some("feature/ABC-1"));
        assert_eq!(l[2].branch, None);
        assert!(!l[2].locked);
        assert!(l[3].locked && l[3].branch.as_deref() == Some("ABC-3"));
        assert!(l[4].bare && l[4].branch.is_none());
        assert_eq!(l[5].path, PathBuf::from("/w/gone"));
        assert!(parse_list("").is_empty());
        assert!(parse_list("HEAD abc\nbranch refs/heads/x\n").is_empty());
    }

    #[test]
    fn only_a_clean_pushed_linked_worktree_is_removed_without_force() {
        let l = parse_list(LIST);
        assert_eq!(check_removal(Some(&l[1]), false, 0, 0, false), Ok(()));
        assert_eq!(check_removal(Some(&l[1]), false, 3, 0, false), Err(Refusal::Dirty(3)));
        assert_eq!(check_removal(Some(&l[1]), false, 0, 2, false), Err(Refusal::Unpushed(2)));
        assert_eq!(check_removal(Some(&l[1]), false, 3, 2, false), Err(Refusal::Dirty(3)));
        assert_eq!(check_removal(Some(&l[1]), false, 3, 2, true), Ok(()));
    }

    #[test]
    fn force_never_covers_the_main_checkout_a_stranger_a_bare_repository_or_a_lock() {
        let l = parse_list(LIST);
        assert_eq!(check_removal(Some(&l[0]), true, 0, 0, true), Err(Refusal::MainCheckout));
        assert_eq!(check_removal(None, false, 0, 0, true), Err(Refusal::NotAWorktree));
        assert_eq!(check_removal(Some(&l[4]), false, 0, 0, true), Err(Refusal::NotAWorktree));
        assert_eq!(check_removal(Some(&l[3]), false, 0, 0, true), Err(Refusal::Locked));
        assert_eq!(check_removal(Some(&l[3]), false, 0, 0, false), Err(Refusal::Locked));
    }

    #[test]
    fn refusals_say_what_to_do() {
        assert!(Refusal::Dirty(2).to_string().contains("--force"));
        assert!(Refusal::Unpushed(1).to_string().contains("push"));
        assert!(Refusal::Locked.to_string().contains("unlock"));
    }
}
