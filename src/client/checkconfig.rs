//! `imux check-config [FILE ...]`: read configuration files without a server and say what imux does
//! not take, by file and line, with counts by kind. The files are read the way the server reads its
//! own (every line parsed, `set` lines applied to a fresh set of options, `source-file` followed),
//! and the format strings in them are checked against the variables and operators imux has. It
//! changes nothing and starts nothing. A line that is understood and does nothing (`run`, an
//! option accepted and ignored) is listed too, as information.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use super::doctor;
use crate::config::cond;
use crate::config::lex::{self, Vars};
use crate::config::options::Options;
use crate::config::stmt::{self, Stmt};
use crate::input::bindings::Command;
use crate::paths;

/// How deep `source-file` is followed and how many files are read in all.
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 64;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    UnknownCommand,
    UnknownFlag,
    UnknownOption,
    UnknownVariable,
    UnknownModifier,
    /// Any other line imux refuses (wrong arguments, a bad key name, an unclosed block).
    Refused,
    /// A file that cannot be read.
    Unreadable,
    /// Understood and without effect.
    IgnoredCommand,
    IgnoredOption,
    /// `#{@name}` or `set -g @name`: accepted, and nothing is stored.
    UserOption,
    /// `source-file` of a file that is not there.
    MissingFile,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::UnknownCommand => "unknown-command",
            Kind::UnknownFlag => "unknown-flag",
            Kind::UnknownOption => "unknown-option",
            Kind::UnknownVariable => "unknown-variable",
            Kind::UnknownModifier => "unknown-modifier",
            Kind::Refused => "refused",
            Kind::Unreadable => "unreadable",
            Kind::IgnoredCommand => "ignored-command",
            Kind::IgnoredOption => "ignored-option",
            Kind::UserOption => "user-option",
            Kind::MissingFile => "missing-file",
        }
    }

    /// Whether the configuration would not work as written (the others only inform).
    pub fn is_error(self) -> bool {
        !matches!(self, Kind::IgnoredCommand | Kind::IgnoredOption | Kind::UserOption | Kind::MissingFile)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: Kind,
    /// What is not taken: a command, an option, `split-window -Q`, `m:`, `window_layout`.
    pub item: String,
    pub file: String,
    /// 0 when the line is not known (an option listed without its line).
    pub line: usize,
}

#[derive(Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub files: usize,
    pub lines: usize,
}

impl Report {
    fn add(&mut self, kind: Kind, item: impl Into<String>, file: &str, line: usize) {
        self.findings.push(Finding { kind, item: item.into(), file: file.to_string(), line });
    }

    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.kind.is_error()).count()
    }

    /// The text `imux check-config` prints: one line per finding (unless `summary`), then the
    /// counts by kind and the items most often met.
    pub fn render(&self, summary: bool) -> String {
        let mut out = String::new();
        if !summary {
            for f in &self.findings {
                let at = if f.line == 0 { f.file.clone() } else { format!("{}:{}", f.file, f.line) };
                out.push_str(&format!("{at}: {}: {}\n", f.kind.word(), f.item));
            }
        }
        out.push_str(&format!("files: {}, lines: {}, problems: {}\n", self.files, self.lines, self.errors()));
        let mut counts: Vec<(Kind, usize)> = Vec::new();
        for f in &self.findings {
            match counts.iter_mut().find(|(k, _)| *k == f.kind) {
                Some((_, n)) => *n += 1,
                None => counts.push((f.kind, 1)),
            }
        }
        counts.sort();
        for (kind, n) in counts {
            out.push_str(&format!("{}: {n}\n", kind.word()));
        }
        let mut items: HashMap<(Kind, &str), usize> = HashMap::new();
        for f in &self.findings {
            *items.entry((f.kind, f.item.as_str())).or_default() += 1;
        }
        let mut ranked: Vec<_> = items.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for ((kind, item), n) in ranked.into_iter().take(40) {
            out.push_str(&format!("{n:>5}  {}  {item}\n", kind.word()));
        }
        out
    }
}

/// The variables a format may use: the pane, window, session and client ones, the hook's, the
/// worktree and agent ones, and the one-letter forms.
const VARIABLES: &[&str] = &[
    "agent_count", "agent_glyph", "agent_name", "agent_state", "client_height", "client_name", "client_readonly", "client_session", "client_width", "cursor_x",
    "cursor_y", "hook", "hook_client", "hook_command", "hook_exit_status", "hook_message", "hook_pane", "hook_session",
    "hook_session_name", "hook_state", "hook_window", "hook_window_name", "host", "host_short", "mouse_any_flag", "mouse_line",
    "mouse_word", "mouse_x", "mouse_y", "pane_active", "pane_bottom", "pane_current_command", "pane_current_path", "pane_dead",
    "pane_dead_status", "pane_height", "pane_id", "pane_in_mode", "pane_index", "pane_input_off", "pane_left", "pane_restore_missing", "pane_spawned_by", "pane_pid", "pane_right",
    "pane_synchronized", "pane_title", "pane_top", "pane_width", "prefix", "session_attached", "session_id", "session_name",
    "session_windows", "window_active", "window_activity_flag", "window_agent_glyph", "window_agent_state", "window_bell_flag",
    "window_flags", "window_height", "window_layout", "window_visible_layout", "window_id", "window_index", "window_marked_flag", "window_name", "window_panes",
    "loop_first_flag", "loop_last_flag", "window_silence_flag", "window_synchronized", "window_width", "window_zoomed_flag", "worktree_branch", "worktree_name", "worktree_path",
    "worktree_repo", "S", "I", "W", "P", "D", "F", "H", "T", "h", "pane_tty", "pid", "start_time", "socket_path", "version", "window_last_flag",
    "session_many_attached", "pane_mode", "pane_at_left", "pane_at_right", "pane_at_top", "pane_at_bottom", "client_prefix", "pane_start_command",
];

/// The placeholders the worktree options fill in when a worktree is made; they are variables of that
/// option only, so a `#{name}` anywhere else is still reported.
const WORKTREE_PLACEHOLDERS: &[(&str, &[&str])] = &[
    ("worktree-start", &["repo", "name", "branch", "base", "path"]),
    ("worktree-setup", &["repo", "name", "branch", "base", "path"]),
    ("worktree-branch-format", &["name", "slug"]),
    ("worktree-path", &["repo", "name"]),
];

/// The extra variable names a line may use because of the option it sets.
fn option_placeholders(line: &str) -> &'static [&'static str] {
    WORKTREE_PLACEHOLDERS
        .iter()
        .find(|(option, _)| line.split_whitespace().any(|word| word == *option))
        .map_or(&[], |(_, names)| *names)
}

/// One-letter forms other multiplexers have that imux has not (none now: `S I W F T P D H h`).
const LETTERS_NOT_HERE: &[char] = &[];

/// A name in capitals is read from the environment (`#{TERM_PROGRAM}`), which a dry read cannot
/// know, so it is not reported.
fn environment_name(name: &str) -> bool {
    name.len() > 1 && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
}

fn known_variable(name: &str, options: &Options) -> bool {
    VARIABLES.contains(&name) || options.knows(name) || options.user(name).is_some() || environment_name(name)
}

/// The operators of `#{...}` imux has: a head that ends in `:` (or `?`, or the `s/` form).
fn supported_operator(head: &str) -> bool {
    matches!(
        head,
        "==" | "!=" | "<=" | ">=" | "<" | ">" | "&&" | "||" | "!" | "q" | "c" | "b" | "d" | "l" | "n" | "E" | "T" | "m" | "=N" | "=-N" | "e|op|"
            | "w" | "t" | "t/p" | "pN" | "p-N" | "W" | "P" | "S" | "C"
    ) || head.starts_with("s/")
        || head.strip_prefix("C/").is_some_and(|f| !f.is_empty() && f.chars().all(|c| matches!(c, 'r' | 'i')))
        || head.starts_with("=/N/")
        || head.starts_with("=/-N/")
        || head.strip_prefix("t/f/").is_some_and(|f| !f.is_empty())
        || head.strip_prefix("m/").is_some_and(|f| !f.is_empty() && f.chars().all(|c| matches!(c, 'r' | 'i')))
}

/// The operator of a body such as `=5:name` or `m:*x*,y`, when the text before the first colon is
/// not a variable name; digits are written `N` so every width counts as one thing.
fn operator_head(body: &str) -> Option<String> {
    let colon = body.find(':')?;
    let head = &body[..colon];
    if head.is_empty() {
        return None;
    }
    let plain = head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '@' || c == '-');
    // A name with a colon after it that is a word such as `window_name`: nothing before the colon
    // is an operator when it has an underscore or is longer than the longest operator.
    let counted = head.strip_prefix(['p', '=']).map(|n| n.trim_start_matches('-')).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if plain && !counted && (head.contains('_') || head.len() > 3) {
        return None;
    }
    let squashed: String = head.chars().map(|c| if c.is_ascii_digit() { 'N' } else { c }).collect();
    let mut squashed = squashed;
    while squashed.contains("NN") {
        squashed = squashed.replace("NN", "N");
    }
    if let Some(bar) = squashed.find('|').filter(|&b| b > 0) {
        squashed.truncate(bar + 1);
        squashed.push_str("op|");
    }
    Some(squashed)
}

/// Split at the commas that are not inside a nested `#{...}`.
fn split_top(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let (mut parts, mut cur, mut depth, mut i) = (Vec::new(), String::new(), 0usize, 0usize);
    while i < chars.len() {
        let c = chars[i];
        if c == '#' && chars.get(i + 1) == Some(&'{') {
            depth += 1;
            cur.push_str("#{");
            i += 2;
            continue;
        }
        match c {
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut cur));
                i += 1;
                continue;
            }
            _ => {}
        }
        cur.push(c);
        i += 1;
    }
    parts.push(cur);
    parts
}

/// What a format string uses that imux does not have, as (kind, item) pairs.
pub fn check_format(text: &str, options: &Options) -> Vec<(Kind, String)> {
    let mut out = Vec::new();
    scan(text, options, &mut out);
    out
}

fn scan(text: &str, options: &Options, out: &mut Vec<(Kind, String)>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '#' {
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('#') | Some(',') | Some('}') => i += 2,
            Some('{') => {
                let mut depth = 1usize;
                let mut j = i + 2;
                while j < chars.len() && depth > 0 {
                    match (chars[j], chars.get(j + 1)) {
                        ('#', Some('{')) => {
                            depth += 1;
                            j += 2;
                            continue;
                        }
                        ('}', _) => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                if depth > 0 {
                    return;
                }
                let body: String = chars[i + 2..j - 1].iter().collect();
                body_check(&body, options, out);
                i = j;
            }
            Some(&c) if LETTERS_NOT_HERE.contains(&c) => {
                out.push((Kind::UnknownVariable, format!("#{c}")));
                i += 2;
            }
            _ => i += 1,
        }
    }
}

fn name_check(name: &str, options: &Options, out: &mut Vec<(Kind, String)>) {
    let name = name.trim();
    if name.is_empty() || name.contains('#') || name.contains(' ') {
        return scan(name, options, out);
    }
    if name.parse::<f64>().is_ok() {
        return;
    }
    if name.starts_with('@') {
        out.push((Kind::UserOption, name.to_string()));
    } else if !known_variable(name, options) {
        out.push((Kind::UnknownVariable, name.to_string()));
    }
}

fn body_check(body: &str, options: &Options, out: &mut Vec<(Kind, String)>) {
    if let Some(rest) = body.strip_prefix('?') {
        let parts = split_top(rest);
        let last = parts.len().saturating_sub(1);
        for (n, part) in parts.iter().enumerate() {
            // The condition of each pair is a name or an expression; the branches are text.
            let is_condition = n % 2 == 0 && n != last;
            if is_condition && !part.contains("#{") && !part.contains(':') {
                name_check(part, options, out);
            } else {
                scan(part, options, out);
            }
        }
        return;
    }
    let Some(head) = operator_head(body) else {
        return name_check(body, options, out);
    };
    let colon = body.find(':').unwrap_or(0);
    let rest = &body[colon + 1..];
    let bare = head.trim_end_matches(':');
    if supported_operator(bare) {
        match bare {
            "q" | "c" | "b" | "d" | "n" | "E" | "T" | "=N" | "=-N" => name_check(rest, options, out),
            "l" => {}
            h if h.starts_with("s/") => name_check(rest, options, out),
            _ => {
                for part in split_top(rest) {
                    scan(&part, options, out);
                }
            }
        }
        return;
    }
    out.push((Kind::UnknownModifier, format!("{head}:")));
    for part in split_top(rest) {
        if part.contains("#{") {
            scan(&part, options, out);
        } else if !part.contains(' ') && !part.contains('*') && !part.contains('/') && !part.contains('.') {
            // Loops and `E:`/`T:` name a variable or an option.
            name_check(&part, options, out);
        }
    }
}

/// `unknown option: x` and the like, from the loader's message.
fn classify(message: &str) -> (Kind, String) {
    if let Some(name) = message.strip_prefix("unknown option: ") {
        return (Kind::UnknownOption, name.to_string());
    }
    if let Some(name) = message.strip_prefix("unknown command: ") {
        return (Kind::UnknownCommand, name.to_string());
    }
    if let Some((command, flag)) = message.split_once(": unknown flag ") {
        return (Kind::UnknownFlag, format!("{command} {flag}"));
    }
    (Kind::Refused, message.to_string())
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

struct Walk {
    seen: HashSet<PathBuf>,
    options: Options,
}

fn check_file(path: &Path, depth: usize, walk: &mut Walk, report: &mut Report) {
    let label = path.to_string_lossy().into_owned();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !walk.seen.insert(canonical) || report.files >= MAX_FILES {
        return;
    }
    let text = paths::read_regular(path, MAX_BYTES).and_then(|b| String::from_utf8(b).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not valid UTF-8")));
    let text = match text {
        Ok(t) => t,
        Err(e) => {
            report.add(Kind::Unreadable, e.to_string(), &label, 0);
            return;
        }
    };
    report.files += 1;
    let lines = lex::logical_lines(&text);
    report.lines += lines.len();

    let check = doctor::check_text(&label, &text);
    let prefix = format!("{label}:");
    for problem in &check.problems {
        let Some(rest) = problem.strip_prefix(&prefix) else { continue };
        let Some((n, message)) = rest.split_once(": ") else { continue };
        let (kind, item) = classify(message);
        report.add(kind, item, &label, n.parse().unwrap_or(0));
    }
    for name in &check.ignored {
        report.add(Kind::IgnoredOption, name.clone(), &label, 0);
    }

    let mut vars = Vars::new();
    let aliases = HashMap::new();
    let mut conditions = cond::Conditions::default();
    for (line_no, original) in &lines {
        if let Some(rest) = original.trim_start().strip_prefix("%if ").or_else(|| original.trim_start().strip_prefix("%elif ")) {
            for (kind, item) in check_format(rest, &walk.options) {
                report.add(kind, item, &label, *line_no);
            }
        }
        let line = match conditions.step(original, &|c| cond::holds(&walk.options, c, &|n| std::env::var(n).ok())) {
            cond::Step::Run(l) => l,
            cond::Step::Skip | cond::Step::Error(_) => continue,
        };
        let line = &line;
        let placeholders = option_placeholders(line);
        for (kind, item) in check_format(line, &walk.options) {
            if kind == Kind::UnknownVariable && placeholders.contains(&item.as_str()) {
                continue;
            }
            report.add(kind, item, &label, *line_no);
        }
        let Ok(stmts) = stmt::parse_line_aliased(line, &mut vars, &aliases) else { continue };
        for s in stmts {
            match s {
                Stmt::Ignored(why) if !why.is_empty() => {
                    let word = line.split_whitespace().next().unwrap_or("").to_string();
                    report.add(Kind::IgnoredCommand, format!("{word} ({why})"), &label, *line_no);
                }
                Stmt::Set { name, value, append } | Stmt::SetGuarded { name, value, append, .. } if name.starts_with('@') => {
                    let _ = walk.options.set_user(&name, value.as_deref(), append, false);
                    report.add(Kind::UserOption, name, &label, *line_no);
                }
                Stmt::Run(Command::SourceFile(target)) => {
                    let next = expand_home(&target);
                    let next = if next.is_relative() { path.parent().unwrap_or(Path::new(".")).join(next) } else { next };
                    if !next.exists() {
                        report.add(Kind::MissingFile, next.to_string_lossy().into_owned(), &label, *line_no);
                    } else if depth < MAX_DEPTH {
                        check_file(&next, depth + 1, walk, report);
                    }
                }
                Stmt::Run(Command::SourceWith(spec)) => {
                    let files = crate::paths::expand_source_path(&spec.path);
                    if files.is_empty() && !spec.quiet {
                        report.add(Kind::MissingFile, spec.path.clone(), &label, *line_no);
                    }
                    for next in files {
                        let next = if next.is_relative() { path.parent().unwrap_or(Path::new(".")).join(next) } else { next };
                        if !next.exists() {
                            if !spec.quiet {
                                report.add(Kind::MissingFile, next.to_string_lossy().into_owned(), &label, *line_no);
                            }
                        } else if depth < MAX_DEPTH {
                            check_file(&next, depth + 1, walk, report);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Check these files, or the configuration imux would read when there are none.
pub fn check(files: &[PathBuf]) -> Report {
    let mut report = Report::default();
    let mut walk = Walk { seen: HashSet::new(), options: Options::new() };
    let files: Vec<PathBuf> = if files.is_empty() { crate::config::default_path().into_iter().collect() } else { files.to_vec() };
    if files.is_empty() {
        report.add(Kind::Unreadable, "no configuration file found (give one)", "check-config", 0);
    }
    for file in &files {
        check_file(file, 0, &mut walk, &mut report);
    }
    report
}

/// `imux check-config [--summary] [FILE ...]`. Exit status 1 when anything would not work.
pub fn run(args: &[String]) -> io::Result<i32> {
    let summary = args.iter().any(|a| a == "--summary");
    let files: Vec<PathBuf> = args.iter().filter(|a| *a != "--summary").map(PathBuf::from).collect();
    let report = check(&files);
    io::Write::write_all(&mut io::stdout(), report.render(summary).as_bytes())?;
    Ok(i32::from(report.errors() > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<(Kind, String)> {
        check_format(text, &Options::new())
    }

    #[test]
    fn the_worktree_options_have_their_own_placeholders() {
        assert_eq!(option_placeholders("set -g worktree-start \"/start #{name}\""), &["repo", "name", "branch", "base", "path"]);
        assert_eq!(option_placeholders("set -g worktree-branch-format \"feature/#{name}-#{slug}\""), &["name", "slug"]);
        assert_eq!(option_placeholders("set -g worktree-path '#{repo}-worktrees/#{name}'"), &["repo", "name"]);
        assert!(option_placeholders("set -g status-left \"#{name}\"").is_empty(), "other options have none");
        assert!(option_placeholders("set -g status-left \"worktree-start\"").is_empty(), "the name inside a value is not the option");
    }

    #[test]
    fn known_variables_and_operators_are_quiet() {
        assert!(found("#{session_name} #{pane_width} #S #W #{?window_zoomed_flag,Z,} #{==:#{window_index},1}").is_empty());
        assert!(found("#{q:pane_title} #{s/a/b/:window_name} #{&&:#{pane_active},#{window_active}}").is_empty());
        assert!(found("##{not_a_variable} #[fg=red] #(echo hi)").is_empty(), "an escaped # and a style are not variables");
    }

    #[test]
    fn an_unknown_variable_is_named_wherever_it_sits() {
        assert_eq!(found("#{nope}"), vec![(Kind::UnknownVariable, "nope".to_string())]);
        assert_eq!(found("#{?nope,a,b}"), vec![(Kind::UnknownVariable, "nope".to_string())]);
        assert_eq!(found("#{?#{window_active},#{nope2},x}"), vec![(Kind::UnknownVariable, "nope2".to_string())]);
        assert_eq!(found("#{q:nope3}"), vec![(Kind::UnknownVariable, "nope3".to_string())]);
        assert!(found("#H").is_empty(), "every one-letter form is known now");
    }

    #[test]
    fn the_loops_and_the_search_are_known() {
        assert!(found("#{W:#{window_name}}").is_empty());
        assert!(found("#{W:#{window_name},*#{window_name}}").is_empty());
        assert!(found("#{P:#{pane_index}}").is_empty());
        assert!(found("#{S:#{session_name}}").is_empty());
        assert!(found("#{W:#{?loop_first_flag,>,}#{?loop_last_flag,<,}}").is_empty());
        assert!(found("#{C:error}").is_empty());
        assert!(found("#{C/r:^err}").is_empty());
        assert!(found("#{C/ri:^err}").is_empty());
        assert_eq!(found("#{C/x:#{pane_title}}"), vec![(Kind::UnknownModifier, "C/x:".to_string())]);
        assert_eq!(found("#{W:#{nope}}"), vec![(Kind::UnknownVariable, "nope".to_string())]);
    }

    #[test]
    fn operators_imux_lacks_are_one_item_whatever_their_number() {
        assert_eq!(found("#{N:session_name}"), vec![(Kind::UnknownModifier, "N:".to_string())]);
        assert_eq!(found("#{L:pane_title}"), vec![(Kind::UnknownModifier, "L:".to_string())]);
        assert_eq!(found("#{O:#{window_name}}"), vec![(Kind::UnknownModifier, "O:".to_string())]);
        assert_eq!(found("#{X:status-left}"), vec![(Kind::UnknownModifier, "X:".to_string())]);
        assert_eq!(found("#{a:#{pane_width}}"), vec![(Kind::UnknownModifier, "a:".to_string())]);
        assert_eq!(found("#{R:session_name,3}"), vec![(Kind::UnknownModifier, "R:".to_string())]);
    }

    #[test]
    fn the_modifiers_the_formats_have_are_quiet() {
        assert!(found("#{=5:session_name} #{=-3:pane_title} #{b:pane_current_path} #{d:pane_current_path} #{n:pane_title}").is_empty());
        assert!(found("#{m:*x*,#{window_name}} #{m/ri:a,#{pane_title}} #{E:status-left} #{T:status-right} #{l:anything at all}").is_empty());
        assert!(found("#{e|+|:#{pane_width},1} #{pane_tty} #{pid} #{socket_path} #{version} #{window_last_flag} #D #F #T #h #H").is_empty());
        assert!(found("#{TERM_PROGRAM} #{?#{==:#{TMUX_CONF},x},a,b}").is_empty(), "a name in capitals is the environment");
    }

    #[test]
    fn the_time_width_pad_and_marker_modifiers_are_quiet() {
        assert!(found("#{t:start_time} #{t/p:start_time} #{t/f/%Y:start_time} #{w:pane_title} #{p10:session_name} #{p-12:session_name}").is_empty());
        assert!(found("#{=/20/...:pane_title} #{=/-8/..:pane_current_path} #{p100:window_name}").is_empty());
    }

    #[test]
    fn a_conditional_branch_that_does_not_run_is_not_read_and_its_condition_is_checked() {
        let dir = std::env::temp_dir().join(format!("imux-cc3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cond.conf");
        std::fs::write(
            &file,
            "%if \"#{==:#{version},0.0}\"\nbind x frobnicate1\n%elif \"#{>=:#{version},3.0}\"\nbind y frobnicate2\n%else\nbind z frobnicate3\n%endif\nset -g status-left \"#{nonesuch}\"\n%if #{nope4}\n%endif\n",
        )
        .unwrap();
        let report = check(std::slice::from_ref(&file));
        let items: Vec<&str> = report.findings.iter().map(|f| f.item.as_str()).collect();
        assert!(items.contains(&"frobnicate2"), "{items:?}");
        assert!(!items.contains(&"frobnicate1") && !items.contains(&"frobnicate3"), "{items:?}");
        assert!(items.contains(&"nonesuch") && items.contains(&"nope4"), "{items:?}");
        assert!(!items.iter().any(|i| i.contains("%if") || i.contains("%endif")), "{items:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_or_and_and_operators_are_not_taken_for_arithmetic() {
        assert!(found("#{||:#{pane_active},#{window_active}}").is_empty());
        assert!(found("#{&&:#{pane_active},#{window_active}}").is_empty());
    }

    #[test]
    fn user_options_are_information_and_option_names_are_variables() {
        assert_eq!(found("#{@theme_color}"), vec![(Kind::UserOption, "@theme_color".to_string())]);
        assert!(found("#{status-position} #{mouse}").is_empty());
    }

    #[test]
    fn the_variable_table_has_what_the_server_builds() {
        for source in [include_str!("../server/remote.rs"), include_str!("../server/mousekey.rs")] {
            for part in source.split("v.insert(\"").skip(1).chain(source.split("vars.push((\"").skip(1)) {
                let name: String = part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if name.is_empty() {
                    continue;
                }
                assert!(VARIABLES.contains(&name.as_str()), "{name} is built by the server but missing from VARIABLES");
            }
        }
    }

    #[test]
    fn messages_of_the_loader_are_sorted_into_kinds() {
        assert_eq!(classify("unknown option: x"), (Kind::UnknownOption, "x".to_string()));
        assert_eq!(classify("unknown command: frob"), (Kind::UnknownCommand, "frob".to_string()));
        assert_eq!(classify("split-window: unknown flag -Q"), (Kind::UnknownFlag, "split-window -Q".to_string()));
        assert_eq!(classify("usage: bind key command").0, Kind::Refused);
    }

    #[test]
    fn a_file_is_read_with_its_sourced_files_and_counted_by_kind() {
        let dir = std::env::temp_dir().join(format!("imux-cc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (main, other) = (dir.join("main.conf"), dir.join("other.conf"));
        std::fs::write(&other, "set -g nosuchoption 1\n").unwrap();
        std::fs::write(
            &main,
            format!(
                "set -g status-left \"#{{N:session_name}} #{{nope}}\"\nbind x frobnicate\nbind -T copy-mode-vi y send -X frobnicate\nsource-file {}\nsource-file {}/gone.conf\nset -g @plugin 'x'\n",
                other.display(),
                dir.display()
            ),
        )
        .unwrap();
        let report = check(std::slice::from_ref(&main));
        let kinds: Vec<Kind> = report.findings.iter().map(|f| f.kind).collect();
        for want in [
            Kind::UnknownModifier,
            Kind::UnknownVariable,
            Kind::UnknownCommand,
            Kind::IgnoredCommand,
            Kind::UnknownOption,
            Kind::MissingFile,
            Kind::UserOption,
        ] {
            assert!(kinds.contains(&want), "{want:?} missing from {:?}", report.findings);
        }
        assert_eq!(report.files, 2, "the sourced file is read too");
        let other_hit = report.findings.iter().find(|f| f.kind == Kind::UnknownOption).unwrap();
        assert!(other_hit.file.ends_with("other.conf") && other_hit.line == 1);
        assert!(report.errors() >= 4);
        let text = report.render(false);
        assert!(text.contains("unknown-command: frobnicate") && text.contains("files: 2"), "{text}");
        assert!(report.render(true).lines().count() < text.lines().count());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_file_has_no_problems_and_a_source_loop_ends() {
        let dir = std::env::temp_dir().join(format!("imux-cc2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("loop.conf");
        std::fs::write(&file, format!("set -g mouse on\nsource-file {}\n", file.display())).unwrap();
        let report = check(&[file]);
        assert_eq!(report.errors(), 0, "{:?}", report.findings);
        assert_eq!(report.files, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
