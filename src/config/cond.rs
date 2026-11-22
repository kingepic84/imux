//! Conditional lines of a configuration: `%if`, `%elif`, `%else`, `%endif` and `%hidden`.
//!
//! `%if FORMAT` keeps the lines up to the next `%elif`, `%else` or `%endif` when the format
//! expands to something that is neither empty nor `0` (quotes around it are dropped). The format
//! sees the same names as any other format with no pane: `version`, `pid`, `host`, `socket_path`,
//! options, user options and environment variables. Conditions nest to `MAX_NEST`. `%hidden
//! NAME=value` is a variable of the file, as the line `NAME=value` would be.

use super::format;
use super::options::Options;

/// How deep `%if` may nest.
const MAX_NEST: usize = 16;

struct Frame {
    /// The lines around this `%if` are being run.
    outer: bool,
    /// A branch of it has run already.
    taken: bool,
    /// The current branch runs.
    active: bool,
}

/// What to do with one line.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Run this line (a `%hidden` line comes back as the assignment).
    Run(String),
    /// The line is a directive or is in a branch that does not run.
    Skip,
    /// A directive that does not make sense, with the reason.
    Error(String),
}

#[derive(Default)]
pub struct Conditions {
    frames: Vec<Frame>,
}

/// The value of a name in a condition: `version`, `pid`, `socket_path`, `host`, `host_short`, the
/// options and the user options; `None` for anything else (the caller may try the environment).
pub fn lookup(options: &Options, name: &str) -> Option<String> {
    let host = || crate::paths::hostname();
    match name {
        "version" => Some(format::COMPAT_VERSION.to_string()),
        "pid" => Some(std::process::id().to_string()),
        "socket_path" => Some(crate::paths::socket_path().to_string_lossy().into_owned()),
        "host" | "H" => Some(host()),
        "host_short" | "h" => Some(host().split('.').next().unwrap_or_default().to_string()),
        _ => options.user(name).or_else(|| options.value_text(name)).map(str::to_string),
    }
}

/// Whether the condition `text` holds: it expands (with `lookup`, then `env` for names it does
/// not know) to something that is neither empty nor `0`.
pub fn holds(options: &Options, text: &str, env: &dyn Fn(&str) -> Option<String>) -> bool {
    let look = |name: &str| lookup(options, name).or_else(|| env(name));
    format::truthy(format::expand(text, &look).trim())
}

/// `text` without one layer of matching quotes.
fn unquote(text: &str) -> &str {
    let text = text.trim();
    for q in ['"', '\''] {
        if let Some(inner) = text.strip_prefix(q).and_then(|t| t.strip_suffix(q)) {
            return inner;
        }
    }
    text
}

impl Conditions {
    fn active(&self) -> bool {
        self.frames.last().is_none_or(|f| f.active)
    }

    /// Whether a `%if` is still open (a file that ends there is wrong).
    pub fn open(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Take the next line. `eval` expands a condition and says whether it is true.
    pub fn step(&mut self, line: &str, eval: &dyn Fn(&str) -> bool) -> Step {
        let trimmed = line.trim_start();
        let Some(directive) = trimmed.strip_prefix('%') else {
            return if self.active() { Step::Run(line.to_string()) } else { Step::Skip };
        };
        let (word, rest) = directive.split_once(char::is_whitespace).map_or((directive, ""), |(w, r)| (w, r.trim()));
        match word {
            "if" => {
                if self.frames.len() >= MAX_NEST {
                    return Step::Error(format!("%if nested more than {MAX_NEST} deep"));
                }
                let outer = self.active();
                let cond = outer && eval(unquote(rest));
                self.frames.push(Frame { outer, taken: cond, active: cond });
                Step::Skip
            }
            "elif" => {
                let Some(frame) = self.frames.last_mut() else { return Step::Error("%elif without %if".to_string()) };
                let cond = frame.outer && !frame.taken && eval(unquote(rest));
                frame.active = cond;
                frame.taken |= cond;
                Step::Skip
            }
            "else" => {
                let Some(frame) = self.frames.last_mut() else { return Step::Error("%else without %if".to_string()) };
                frame.active = frame.outer && !frame.taken;
                frame.taken = true;
                Step::Skip
            }
            "endif" => {
                if self.frames.pop().is_none() {
                    return Step::Error("%endif without %if".to_string());
                }
                Step::Skip
            }
            "hidden" if self.active() => Step::Run(rest.to_string()),
            _ if self.active() => Step::Run(line.to_string()),
            _ => Step::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, eval: &dyn Fn(&str) -> bool) -> (Vec<String>, Vec<String>) {
        let mut c = Conditions::default();
        let (mut lines, mut errors) = (Vec::new(), Vec::new());
        for line in text.lines() {
            match c.step(line, eval) {
                Step::Run(l) => lines.push(l),
                Step::Skip => {}
                Step::Error(e) => errors.push(e),
            }
        }
        if c.open() {
            errors.push("open".to_string());
        }
        (lines, errors)
    }

    fn yes(cond: &str) -> bool {
        cond == "yes" || cond == "1"
    }

    #[test]
    fn a_true_branch_runs_and_a_false_one_does_not() {
        assert_eq!(run("a\n%if yes\nb\n%endif\nc\n", &yes).0, ["a", "b", "c"]);
        assert_eq!(run("a\n%if no\nb\n%endif\nc\n", &yes).0, ["a", "c"]);
    }

    #[test]
    fn elif_and_else_take_the_first_true_branch_only() {
        let text = "%if no\n1\n%elif yes\n2\n%elif yes\n3\n%else\n4\n%endif\n";
        assert_eq!(run(text, &yes).0, ["2"]);
        assert_eq!(run("%if no\n1\n%elif no\n2\n%else\n3\n%endif\n", &yes).0, ["3"]);
        assert_eq!(run("%if yes\n1\n%else\n2\n%endif\n", &yes).0, ["1"]);
    }

    #[test]
    fn conditions_nest_and_an_inactive_outer_keeps_everything_inside_off() {
        let text = "%if no\n%if yes\nx\n%else\ny\n%endif\nz\n%else\n%if yes\nw\n%endif\n%endif\n";
        assert_eq!(run(text, &yes).0, ["w"]);
    }

    #[test]
    fn the_condition_is_not_evaluated_when_the_branch_cannot_run() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let eval = |_: &str| {
            calls.set(calls.get() + 1);
            true
        };
        run("%if no_such\n%if x\n%endif\n%endif\n", &|c| {
            calls.set(calls.get() + 1);
            c == "never"
        });
        assert_eq!(calls.get(), 1);
        run("%if yes\n%elif x\n%endif\n", &eval);
        assert_eq!(calls.get(), 2, "an %elif after a taken branch is not evaluated");
    }

    #[test]
    fn quotes_around_the_condition_are_dropped() {
        assert_eq!(run("%if \"yes\"\na\n%endif\n", &yes).0, ["a"]);
        assert_eq!(run("%if 'yes'\na\n%endif\n", &yes).0, ["a"]);
        assert_eq!(run("%if \"#{==:a,a}\"\na\n%endif\n", &|c| c == "#{==:a,a}").0, ["a"]);
    }

    #[test]
    fn hidden_is_an_assignment_only_where_it_runs() {
        assert_eq!(run("%hidden A=1\n%if no\n%hidden B=2\n%endif\n", &yes).0, ["A=1"]);
    }

    #[test]
    fn mistakes_are_reported() {
        assert_eq!(run("%endif\n", &yes).1.len(), 1);
        assert_eq!(run("%else\n", &yes).1.len(), 1);
        assert_eq!(run("%elif x\n", &yes).1.len(), 1);
        assert_eq!(run("%if yes\na\n", &yes).1, ["open"]);
        let deep = "%if yes\n".repeat(MAX_NEST + 1);
        assert!(run(&deep, &yes).1.iter().any(|e| e.contains("nested")));
    }

    #[test]
    fn other_percent_lines_are_left_to_the_parser_when_they_run() {
        assert_eq!(run("%wat x\n", &yes).0, ["%wat x"]);
        assert!(run("%if no\n%wat x\n%endif\n", &yes).0.is_empty());
    }

    #[test]
    fn the_condition_names_are_version_host_options_and_user_options() {
        let mut o = Options::new();
        o.set_user("@x", Some("v"), false, false).unwrap();
        assert_eq!(lookup(&o, "version").as_deref(), Some(format::COMPAT_VERSION));
        assert_eq!(lookup(&o, "@x").as_deref(), Some("v"));
        assert_eq!(lookup(&o, "mouse").as_deref(), Some("on"));
        assert_eq!(lookup(&o, "nonsense"), None);
        assert!(lookup(&o, "pid").is_some_and(|p| p.parse::<u32>().is_ok()));
    }
}
