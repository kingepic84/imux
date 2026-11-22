//! Format strings: `#{name}` and `#{s/from/to/:name}` (substitute), `##` for a literal `#`.
//! Conditionals and comparisons nest up to `MAX_DEPTH`: `#{?cond,then,else}` (also a chain
//! `cond,then,cond,then,else`), `#{==:a,b}`, `!=`, `<`, `>`, `<=`, `>=` (compared as text),
//! `#{&&:a,b,...}`, `#{||:a,b,...}` and `#{!:a}`; true means neither empty nor `0`. A literal comma
//! or closing brace inside an operand is `#,` and `#}`; a `#[style]` may hold commas as it is.
//! `#{q:name}` is the value as one word for a shell. `expand_for_shell` refuses every variable
//! that a program in a pane could have set unless it is written that way.
//! `expand_with_commands` also resolves `#(command)`: the command is a format that goes through the
//! same shell check, its output is shown with every `#` in it turned into `LITERAL_HASH` (so it can
//! be neither a variable nor a style), and `restore_hash` puts the `#` back after `runs`.
//! `#X` is a one-letter variable when the lookup knows it. `strftime` fills in `%` times and
//! `runs` splits `#[fg=red,bold]` style directives into styled runs. Anything else is copied
//! through unchanged.

use std::cell::RefCell;

use super::options::parse_style;
use crate::term::cell::Style;

/// How deep conditionals and comparisons may nest; anything deeper expands to nothing.
const MAX_DEPTH: usize = 8;

type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// What one expansion works with. With `safe` set the text is going to a shell: a variable that
/// `safe` does not name is refused (its name goes to `refused`, its place stays empty) unless it is
/// written `#{q:name}`, which quotes the value for the shell.
struct Env<'a> {
    lookup: Lookup<'a>,
    safe: Option<&'a dyn Fn(&str) -> bool>,
    refused: &'a RefCell<Vec<String>>,
    /// For `#(command)`: what is safe in the command text and what the command's output is. Without
    /// it `#(` stays text.
    commands: Option<Commands<'a>>,
    /// For `#{W:...}`, `#{P:...}` and `#{S:...}`: what to loop over. A loop that is not given stays
    /// text, and inside a loop there is none, so loops do not nest.
    loops: Loops<'a>,
    /// For `#{C:...}`: the search of the pane's text. Without it the result is empty.
    search: Option<Search<'a>>,
}

/// What a loop goes over: `count` items, the `current` one, and `lookup(index, name)`, the
/// variables of one item. A name it does not know falls back to the lookup outside the loop.
#[derive(Clone, Copy)]
pub struct Windows<'a> {
    pub count: usize,
    pub current: usize,
    pub lookup: &'a dyn Fn(usize, &str) -> Option<String>,
}

/// The loops a format may have: `W:` over windows, `P:` over the panes of a window, `S:` over
/// sessions.
#[derive(Clone, Copy, Default)]
pub struct Loops<'a> {
    pub windows: Option<Windows<'a>>,
    pub panes: Option<Windows<'a>>,
    pub sessions: Option<Windows<'a>>,
}

/// The search behind `#{C:text}`: `(text, regex, ignore_case)` gives the number of the first line of
/// the pane's screen that holds it, counted from 1, or 0 when none does.
pub type Search<'a> = &'a dyn Fn(&str, bool, bool) -> usize;

/// How `#(command)` is resolved: `safe` is the shell check of the text of the command, and
/// `output` gets the expanded command (or the variables that were refused in it) and returns the
/// text to show.
#[derive(Clone, Copy)]
struct Commands<'a> {
    safe: &'a dyn Fn(&str) -> bool,
    output: &'a dyn Fn(Result<String, Vec<String>>) -> String,
}

impl Env<'_> {
    /// The value of `name` for text that is printed.
    fn value(&self, name: &str) -> Option<String> {
        let value = (self.lookup)(name)?;
        if self.safe.is_some_and(|safe| !safe(name)) {
            let mut refused = self.refused.borrow_mut();
            if !refused.iter().any(|r| r == name) {
                refused.push(name.to_string());
            }
            return Some(String::new());
        }
        Some(value)
    }

    /// The same lookup with no shell check, for operands whose text is only tested and not printed.
    fn quiet(&self) -> Env<'_> {
        Env { lookup: self.lookup, safe: None, refused: self.refused, commands: self.commands, loops: self.loops, search: self.search }
    }
}

/// Stands for a `#` that came out of a command: `runs` must not take `#[` in it for a style, and
/// nothing expands it again. It is not a character that can be in text, so `restore_hash` can put
/// the `#` back once the runs are cut.
pub const LITERAL_HASH: char = '\u{FDD0}';

/// Turn the stand-ins for `#` in `text` back into `#`.
pub fn restore_hash(text: &str) -> String {
    text.replace(LITERAL_HASH, "#")
}

/// Length of the command in `#(command)` whose text starts at `from`: the distance to the `)` that
/// closes it, with parentheses inside counted. `##` is a literal `#` and so never opens one.
fn command_end(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = from;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('#', Some('#')) => i += 2,
            ('#', Some('(')) => {
                depth += 1;
                i += 2;
            }
            ('(', _) => {
                depth += 1;
                i += 1;
            }
            (')', _) if depth == 0 => return Some(i - from),
            (')', _) => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// The text a `#(command)` stands for. The command is a format first, and a variable that a
/// program in a pane could have set is refused in it like in any shell text (`expand_for_shell`).
/// Its output is never expanded again: every `#` in it is a stand-in.
fn command_text(command: &str, env: &Env, commands: Commands, depth: usize) -> String {
    let refused = RefCell::new(Vec::new());
    let shell = Env { lookup: env.lookup, safe: Some(commands.safe), refused: &refused, commands: None, loops: Loops::default(), search: None };
    let text = expand_at(command, &shell, depth + 1);
    let refused = refused.into_inner();
    let output = (commands.output)(if refused.is_empty() { Ok(text) } else { Err(refused) });
    output.replace('#', &LITERAL_HASH.to_string())
}

/// `value` as one word for `sh`: in single quotes, a quote inside it as `'\''`.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `value` as one word of an imux command line: in double quotes, with `\`, `"` and `$` escaped
/// (what `run-shell -C` needs; `#{c:name}`).
pub fn command_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if matches!(c, '\\' | '"' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Expand `fmt`, asking `lookup` for the value of each variable name (unknown names expand to "").
pub fn expand(fmt: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let refused = RefCell::new(Vec::new());
    expand_at(fmt, &Env { lookup, safe: None, refused: &refused, commands: None, loops: Loops::default(), search: None }, 0)
}

/// `expand` that also loops and searches: `#{W:fmt}`, `#{P:fmt}` and `#{S:fmt}` (each also as
/// `fmt,current_fmt`) expand the format once per item of that loop in `loops`, and `#{C:text}` asks
/// `search`.
pub fn expand_with(fmt: &str, lookup: &dyn Fn(&str) -> Option<String>, loops: Loops, search: Option<Search>) -> String {
    let refused = RefCell::new(Vec::new());
    expand_at(fmt, &Env { lookup, safe: None, refused: &refused, commands: None, loops, search }, 0)
}

/// Expand `fmt` and resolve each `#(command)` in it. `safe` is the shell check of the command text
/// and `output` turns an expanded command (or the names refused in it) into what is shown.
pub fn expand_with_commands(
    fmt: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    safe: &dyn Fn(&str) -> bool,
    output: &dyn Fn(Result<String, Vec<String>>) -> String,
) -> String {
    expand_with_windows(fmt, lookup, safe, output, None)
}

/// `expand_with_commands` that also loops: `#{W:fmt}` or `#{W:fmt,current_fmt}` expands the format
/// once per window of `windows` (the second one for the current window), with that window's
/// variables, `loop_first_flag` and `loop_last_flag` set, and joins the results.
pub fn expand_with_windows(
    fmt: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    safe: &dyn Fn(&str) -> bool,
    output: &dyn Fn(Result<String, Vec<String>>) -> String,
    windows: Option<Windows>,
) -> String {
    let refused = RefCell::new(Vec::new());
    let loops = Loops { windows, ..Loops::default() };
    expand_at(fmt, &Env { lookup, safe: None, refused: &refused, commands: Some(Commands { safe, output }), loops, search: None }, 0)
}

/// Expand `fmt` into text for a shell. `safe` says which variables hold nothing a program in a
/// pane could have chosen; any other variable must be written `#{q:name}` or the call fails with
/// the names that were refused.
pub fn expand_for_shell(fmt: &str, lookup: &dyn Fn(&str) -> Option<String>, safe: &dyn Fn(&str) -> bool) -> Result<String, Vec<String>> {
    let refused = RefCell::new(Vec::new());
    let text = expand_at(fmt, &Env { lookup, safe: Some(safe), refused: &refused, commands: None, loops: Loops::default(), search: None }, 0);
    let refused = refused.into_inner();
    if refused.is_empty() { Ok(text) } else { Err(refused) }
}

fn expand_at(fmt: &str, env: &Env, depth: usize) -> String {
    if depth > MAX_DEPTH {
        return String::new();
    }
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' && matches!(chars.get(i + 1), Some('#' | ',' | '}')) {
            out.push(chars[i + 1]);
            i += 2;
        } else if chars[i] == '#'
            && chars.get(i + 1) == Some(&'(')
            && let Some(commands) = env.commands
            && let Some(len) = command_end(&chars, i + 2)
        {
            let command: String = chars[i + 2..i + 2 + len].iter().collect();
            out.push_str(&command_text(&command, env, commands, depth));
            i += len + 3;
        } else if chars[i] == '#' && chars.get(i + 1) == Some(&'{') {
            let Some(len) = closing(&chars, i + 2) else {
                out.push('#');
                i += 1;
                continue;
            };
            let body: String = chars[i + 2..i + 2 + len].iter().collect();
            match variable(&body, env, depth) {
                Some(value) => out.push_str(&value),
                None => out.extend(&chars[i..i + len + 3]),
            }
            i += len + 3;
        } else if chars[i] == '#'
            && let Some(letter) = chars.get(i + 1).filter(|c| c.is_ascii_alphabetic())
            && let Some(value) = env.value(&letter.to_string())
        {
            out.push_str(&value);
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Replace `%` conversions (`%I:%M %p`) with the current local time.
pub fn strftime(fmt: &str) -> String {
    if !fmt.contains('%') {
        return fmt.to_string();
    }
    // SAFETY: time(NULL) is always valid.
    strftime_at(fmt, unsafe { libc::time(std::ptr::null_mut()) } as i64)
}

/// `localtime_r` of `t` (seconds since 1970), zeroed when the time is out of range.
fn local_time(t: i64) -> libc::tm {
    let secs = t as libc::time_t;
    // SAFETY: localtime_r only writes into the zeroed tm we pass and reads the time we point at.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    }
}

/// Replace `%` conversions with the local time `t` (seconds since 1970).
fn strftime_at(fmt: &str, t: i64) -> String {
    if !fmt.contains('%') {
        return fmt.to_string();
    }
    let Ok(c_fmt) = std::ffi::CString::new(fmt) else { return fmt.to_string() };
    let tm = local_time(t);
    let mut buf = vec![0u8; fmt.len() * 4 + 64];
    // SAFETY: buf is valid for its length, c_fmt is NUL-terminated, tm is initialized.
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c_fmt.as_ptr(), &tm) };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// The `strftime` text `#{t/p:...}` uses for a time that is `age` seconds old: the clock time within
/// the last day, else the weekday and day of the month in this month, the day and month in this
/// year, and the month and year before that.
fn pretty_time_format(age: i64, same_month: bool, same_year: bool) -> &'static str {
    if age < 24 * 3600 {
        "%H:%M"
    } else if same_month {
        "%a%d"
    } else if same_year {
        "%d%b"
    } else {
        "%b%y"
    }
}

/// `#{t/p:...}` for the time `t`, seen at `now`.
fn pretty_time(t: i64, now: i64) -> String {
    let (then, here) = (local_time(t), local_time(now));
    let same_year = then.tm_year == here.tm_year;
    strftime_at(pretty_time_format(now - t, same_year && then.tm_mon == here.tm_mon, same_year), t)
}

/// Split expanded text into runs at its `#[style]` directives. `base` is where `#[default]`
/// returns to; a directive that does not parse is dropped.
pub fn runs(text: &str, base: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    let mut style = base;
    let mut cur = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("#[") {
        let Some(end) = rest[at..].find(']') else { break };
        cur.push_str(&rest[..at]);
        push_run(&mut out, std::mem::take(&mut cur), style);
        for word in rest[at + 2..at + end].split([',', ' ']).filter(|w| !w.is_empty()) {
            match word {
                "default" => style = base,
                "none" => style.attrs = 0,
                _ => {
                    if let Ok(spec) = parse_style(word) {
                        style = spec.apply(style);
                    }
                }
            }
        }
        rest = &rest[at + end + 1..];
    }
    cur.push_str(rest);
    push_run(&mut out, cur, style);
    out
}

/// `runs` for a row with `#[align=left]`, `#[align=centre]` and `#[align=right]` in it: the runs of
/// the left, the center and the right part. Text before any `align=` is the left part; a style
/// carries over from one part to the next, as it does inside one.
pub fn aligned_runs(text: &str, base: Style) -> [Vec<(String, Style)>; 3] {
    let mut out: [Vec<(String, Style)>; 3] = Default::default();
    let mut part = 0;
    let mut style = base;
    let mut cur = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("#[") {
        let Some(end) = rest[at..].find(']') else { break };
        cur.push_str(&rest[..at]);
        push_run(&mut out[part], std::mem::take(&mut cur), style);
        for word in rest[at + 2..at + end].split([',', ' ']).filter(|w| !w.is_empty()) {
            match word {
                "default" => style = base,
                "none" => style.attrs = 0,
                "align=left" => part = 0,
                "align=centre" | "align=center" | "align=absolute-centre" | "align=absolute-center" => part = 1,
                "align=right" => part = 2,
                _ => {
                    if let Ok(spec) = parse_style(word) {
                        style = spec.apply(style);
                    }
                }
            }
        }
        rest = &rest[at + end + 1..];
    }
    cur.push_str(rest);
    push_run(&mut out[part], cur, style);
    out
}

/// Append text to the last run when the style is the same, else start a new run.
fn push_run(out: &mut Vec<(String, Style)>, text: String, style: Style) {
    if text.is_empty() {
        return;
    }
    match out.last_mut() {
        Some((prev, s)) if *s == style => prev.push_str(&text),
        _ => out.push((text, style)),
    }
}

/// Length of the body of a `#{...}` whose body starts at `from`: the distance to the `}` that
/// closes it. Nested `#{...}` are skipped, as are the escapes `##`, `#,`, `#}` and `#[style]`
/// (a style may hold commas).
fn closing(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = from;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('#', Some('#' | ',' | '}')) => i += 2,
            ('#', Some('{')) => {
                depth += 1;
                i += 2;
            }
            ('#', Some('[')) => i += chars[i..].iter().position(|&c| c == ']').map_or(1, |n| n + 1),
            ('}', _) if depth == 0 => return Some(i - from),
            ('}', _) => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Split the operands of a conditional or comparison at its top-level commas. The pieces keep
/// their escapes, so they expand later exactly as written.
fn split_commas(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let step = match (chars[i], chars.get(i + 1)) {
            ('#', Some('#' | ',' | '}')) => 2,
            ('#', Some('{')) => {
                depth += 1;
                2
            }
            ('#', Some('[')) => chars[i..].iter().position(|&c| c == ']').map_or(1, |n| n + 1),
            ('}', _) => {
                depth = depth.saturating_sub(1);
                1
            }
            (',', _) if depth == 0 => {
                parts.push(std::mem::take(&mut cur));
                i += 1;
                continue;
            }
            _ => 1,
        };
        cur.extend(&chars[i..i + step]);
        i += step;
    }
    parts.push(cur);
    parts
}

/// A value is true when it is neither empty nor `0`.
pub fn truthy(value: &str) -> bool {
    !value.is_empty() && value != "0"
}

fn flag(on: bool) -> String {
    if on { "1" } else { "0" }.to_string()
}

type Compare = fn(&str, &str) -> bool;

const COMPARES: [(&str, Compare); 6] = [
    ("==:", |a, b| a == b),
    ("!=:", |a, b| a != b),
    ("<=:", |a, b| a <= b),
    (">=:", |a, b| a >= b),
    ("<:", |a, b| a < b),
    (">:", |a, b| a > b),
];

/// The value of the body of `#{...}`, or `None` when it is an operator that is malformed (the
/// text is then left as it was written).
fn variable(body: &str, env: &Env, depth: usize) -> Option<String> {
    // Text that is printed keeps the shell check; text that is only tested does not need it.
    let operand = |text: &str| expand_at(text, env, depth + 1);
    let tested = |text: &str| expand_at(text, &env.quiet(), depth + 1);
    if let Some(rest) = body.strip_prefix('?') {
        let parts = split_commas(rest);
        if parts.len() < 2 {
            return None;
        }
        let mut pairs = parts.chunks_exact(2);
        for pair in pairs.by_ref() {
            // A bare variable name is looked up; any other condition is expanded.
            let cond = (env.lookup)(&pair[0]).unwrap_or_else(|| tested(&pair[0]));
            if truthy(&cond) {
                return Some(operand(&pair[1]));
            }
        }
        return Some(pairs.remainder().first().map_or_else(String::new, |last| operand(last)));
    }
    for (op, test) in COMPARES {
        if let Some(rest) = body.strip_prefix(op) {
            let parts = split_commas(rest);
            return (parts.len() == 2).then(|| flag(test(&tested(&parts[0]), &tested(&parts[1]))));
        }
    }
    if let Some(rest) = body.strip_prefix("&&:").or_else(|| body.strip_prefix("||:")) {
        let parts = split_commas(rest);
        if parts.len() < 2 {
            return None;
        }
        return Some(flag(if body.starts_with("&&") {
            parts.iter().all(|p| truthy(&tested(p)))
        } else {
            parts.iter().any(|p| truthy(&tested(p)))
        }));
    }
    if let Some(rest) = body.strip_prefix("!:") {
        let parts = split_commas(rest);
        return (parts.len() == 1).then(|| flag(!truthy(&tested(&parts[0]))));
    }
    if let Some((rest, looping)) = [("W:", env.loops.windows), ("P:", env.loops.panes), ("S:", env.loops.sessions)]
        .into_iter()
        .find_map(|(prefix, looping)| body.strip_prefix(prefix).map(|rest| (rest, looping)))
    {
        let windows = looping?;
        let parts = split_commas(rest);
        let (each, current) = match parts.as_slice() {
            [one] => (one, one),
            [each, current] => (each, current),
            _ => return None,
        };
        let mut out = String::new();
        for n in 0..windows.count {
            let inner = |name: &str| match name {
                "loop_first_flag" => Some(flag(n == 0)),
                "loop_last_flag" => Some(flag(n + 1 == windows.count)),
                _ => (windows.lookup)(n, name).or_else(|| (env.lookup)(name)),
            };
            let env = Env { lookup: &inner, safe: env.safe, refused: env.refused, commands: env.commands, loops: Loops::default(), search: env.search };
            out.push_str(&expand_at(if n == windows.current { current } else { each }, &env, depth + 1));
        }
        return Some(out);
    }
    if let Some(name) = body.strip_prefix("q:") {
        return Some(shell_quote(&(env.lookup)(name).unwrap_or_default()));
    }
    if let Some(name) = body.strip_prefix("c:") {
        return Some(command_quote(&(env.lookup)(name).unwrap_or_default()));
    }
    if let Some(rest) = body.strip_prefix("s/") {
        let mut parts = rest.splitn(3, '/');
        if let (Some(from), Some(to), Some(tail)) = (parts.next(), parts.next(), parts.next())
            && let Some(name) = tail.strip_prefix(':')
        {
            let value = env.value(name).unwrap_or_default();
            return Some(if from.is_empty() { value } else { value.replace(from, to) });
        }
    }
    if let Some(value) = modified(body, env, depth) {
        return Some(value);
    }
    Some(env.value(body).unwrap_or_default())
}

/// The version a configuration may test with `#{version}`: the one the `tmux` shim reports.
pub const COMPAT_VERSION: &str = "3.4";

/// Longest regular expression `m/r:` takes.
const MAX_PATTERN: usize = 512;

/// Whether `text` reads as one variable name (so it is looked up and not expanded).
fn is_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '-'))
}

/// A format's escapes (`##`, `#,`, `#}`) turned into the characters they stand for.
fn unescape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' && matches!(chars.get(i + 1), Some('#' | ',' | '}')) {
            out.push(chars[i + 1]);
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// The last component of a path, as `basename` prints it.
fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { String::new() } else { "/".to_string() };
    }
    trimmed.rsplit('/').next().unwrap_or_default().to_string()
}

/// A path without its last component, as `dirname` prints it.
fn dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { ".".to_string() } else { "/".to_string() };
    }
    match trimmed.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(at) => trimmed[..at].trim_end_matches('/').to_string(),
    }
}

/// Shell-style match with `*` and `?` (no brackets), without backtracking blow-up.
fn glob(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] != '*' && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// `a op b` for the operators of `#{e|op|:a,b}`; a division by zero is 0.
fn arithmetic(op: &str, a: i64, b: i64) -> Option<String> {
    let number = |n: i64| Some(n.to_string());
    match op {
        "+" => number(a.wrapping_add(b)),
        "-" => number(a.wrapping_sub(b)),
        "*" => number(a.wrapping_mul(b)),
        "/" => number(if b == 0 { 0 } else { a.wrapping_div(b) }),
        "m" => number(if b == 0 { 0 } else { a.wrapping_rem(b) }),
        "==" => number(i64::from(a == b)),
        "!=" => number(i64::from(a != b)),
        "<" => number(i64::from(a < b)),
        ">" => number(i64::from(a > b)),
        "<=" => number(i64::from(a <= b)),
        ">=" => number(i64::from(a >= b)),
        _ => None,
    }
}

/// The modifiers that take one operand or a pair: `=N:`, `b:`, `d:`, `n:`, `E:`, `T:`, `l:`,
/// `m:`, `m/flags:` and `e|op|:`. `None` when the body is not one of them (it is then a variable).
fn modified(body: &str, env: &Env, depth: usize) -> Option<String> {
    let tested = |text: &str| expand_at(text, &env.quiet(), depth + 1);
    if let Some(rest) = body.strip_prefix("l:") {
        return Some(unescape(rest));
    }
    if let Some(after) = body.strip_prefix("e|") {
        let (op, rest) = after.split_once('|')?;
        let rest = rest.strip_prefix(':')?;
        let parts = split_commas(rest);
        let [a, b] = parts.as_slice() else { return None };
        let number = |text: &str| tested(text).trim().parse::<i64>().unwrap_or(0);
        return arithmetic(op, number(a), number(b));
    }
    let colon = body.find(':')?;
    let (head, tail) = (&body[..colon], &body[colon + 1..]);
    if head == "m" || head.strip_prefix("m/").is_some_and(|f| !f.is_empty() && f.chars().all(|c| matches!(c, 'r' | 'i'))) {
        let flags = head.strip_prefix("m/").unwrap_or("");
        let parts = split_commas(tail);
        let [pattern, text] = parts.as_slice() else { return None };
        let (pattern, text) = (tested(pattern), tested(text));
        let ci = flags.contains('i');
        let matched = if flags.contains('r') {
            pattern.len() <= MAX_PATTERN
                && regex_lite::RegexBuilder::new(&pattern).case_insensitive(ci).build().is_ok_and(|re| re.is_match(&text))
        } else {
            let fold = |s: &str| -> Vec<char> { if ci { s.to_lowercase().chars().collect() } else { s.chars().collect() } };
            glob(&fold(&pattern), &fold(&text))
        };
        return Some(flag(matched));
    }
    if head == "C" || head.strip_prefix("C/").is_some_and(|f| !f.is_empty() && f.chars().all(|c| matches!(c, 'r' | 'i'))) {
        let flags = head.strip_prefix("C/").unwrap_or("");
        // Shell text and the status line have no search: what a program in a pane printed does not
        // become part of a command.
        let Some(search) = env.search else { return Some(String::new()) };
        let pattern = tested(tail);
        let regex = flags.contains('r');
        if pattern.is_empty() || (regex && pattern.len() > MAX_PATTERN) {
            return Some("0".to_string());
        }
        return Some(search(&pattern, regex, flags.contains('i')).to_string());
    }
    let cut = head.strip_prefix('=').and_then(|n| n.parse::<i64>().ok());
    let marked = marked_cut(head);
    let pad = head.strip_prefix('p').and_then(|n| n.parse::<i64>().ok());
    let time = time_kind(head);
    if cut.is_none() && marked.is_none() && pad.is_none() && time.is_none() && !matches!(head, "b" | "d" | "n" | "E" | "T" | "w") {
        return None;
    }
    let value = if is_name(tail) { env.value(tail).unwrap_or_default() } else { expand_at(tail, env, depth + 1) };
    if let Some((n, marker)) = marked {
        return Some(truncate(&value, n, marker));
    }
    if let Some(width) = pad {
        return Some(pad_to(&value, width));
    }
    if let Some(kind) = time {
        return Some(time_text(&value, &kind));
    }
    Some(match head {
        "b" => basename(&value),
        "d" => dirname(&value),
        "n" => value.chars().count().to_string(),
        "w" => unicode_width::UnicodeWidthStr::width(value.as_str()).to_string(),
        "E" => expand_at(&value, env, depth + 1),
        "T" => expand_at(&strftime(&value), env, depth + 1),
        _ => truncate(&value, cut.unwrap_or(0), ""),
    })
}

/// How `#{t:...}` shows a time.
#[derive(Debug, PartialEq, Eq)]
enum TimeKind {
    /// `t:`: as `ctime` does.
    Plain,
    /// `t/p:`: short for the age (`pretty_time_format`).
    Pretty,
    /// `t/f/FORMAT:`: through `strftime`.
    Format(String),
}

fn time_kind(head: &str) -> Option<TimeKind> {
    match head {
        "t" => Some(TimeKind::Plain),
        "t/p" => Some(TimeKind::Pretty),
        _ => head.strip_prefix("t/f/").filter(|f| !f.is_empty()).map(|f| TimeKind::Format(f.to_string())),
    }
}

/// The text of the time `value` (seconds since 1970); empty when it is not one.
fn time_text(value: &str, kind: &TimeKind) -> String {
    // The calendar of a year before 1 or after 9999 is not one `localtime_r` is asked for.
    let Some(t) = value.trim().parse::<i64>().ok().filter(|t| (0..=253_402_300_799).contains(t)) else { return String::new() };
    match kind {
        TimeKind::Plain => strftime_at("%a %b %e %H:%M:%S %Y", t),
        // SAFETY: time(NULL) is always valid.
        TimeKind::Pretty => pretty_time(t, unsafe { libc::time(std::ptr::null_mut()) } as i64),
        TimeKind::Format(f) => strftime_at(f, t),
    }
}

/// `=/N/marker` of `#{=/N/marker:...}`: the count and the marker.
fn marked_cut(head: &str) -> Option<(i64, &str)> {
    let (n, marker) = head.strip_prefix("=/")?.split_once('/')?;
    Some((n.parse().ok()?, marker))
}

/// The first `n` characters of `value`, or the last `-n` for a negative `n`; `marker` is added where
/// the text was cut (after the first part, before the last).
fn truncate(value: &str, n: i64, marker: &str) -> String {
    let count = n.unsigned_abs() as usize;
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= count {
        return value.to_string();
    }
    if n >= 0 {
        let mut out: String = chars.iter().take(count).collect();
        out.push_str(marker);
        out
    } else {
        let mut out = marker.to_string();
        out.extend(&chars[chars.len() - count..]);
        out
    }
}

/// `value` filled with spaces to `width` cells: after it for a positive `width`, before it for a
/// negative one. A longer value is left alone.
fn pad_to(value: &str, width: i64) -> String {
    let have = unicode_width::UnicodeWidthStr::width(value) as u64;
    let fill = " ".repeat(width.unsigned_abs().saturating_sub(have).min(4096) as usize);
    if width >= 0 { format!("{value}{fill}") } else { format!("{fill}{value}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str) -> Option<String> {
        (name == "pane_current_command").then(|| "my-wrapper".to_string())
    }

    #[test]
    fn plain_variables_and_literals() {
        assert_eq!(expand("#{pane_current_command}", &cmd), "my-wrapper");
        assert_eq!(expand("[#{pane_current_command}] ##x", &cmd), "[my-wrapper] #x");
        assert_eq!(expand("#{nope}!", &cmd), "!");
    }

    #[test]
    fn substitution_rewrites_the_value() {
        assert_eq!(expand("#{s/my-wrapper/claude/:pane_current_command}", &cmd), "claude");
        assert_eq!(expand("#{s/zzz/q/:pane_current_command}", &cmd), "my-wrapper");
    }

    #[test]
    fn unterminated_references_stay_text() {
        assert_eq!(expand("#{oops", &cmd), "#{oops");
    }

    #[test]
    fn one_letter_variables_expand_only_when_known() {
        let lookup = |n: &str| (n == "S").then(|| "work".to_string());
        assert_eq!(expand("#S #W #[bold]", &lookup), "work #W #[bold]");
    }

    fn looped(fmt: &str, count: usize, current: usize) -> String {
        let names = ["zero", "one", "two"];
        let in_window = |i: usize, name: &str| match name {
            "n" => names.get(i).map(|n| n.to_string()),
            "i" => Some(i.to_string()),
            _ => None,
        };
        let outer = |name: &str| match name {
            "n" => Some("outer".to_string()),
            "session" => Some("s".to_string()),
            _ => None,
        };
        let windows = Windows { count, current, lookup: &in_window };
        expand_with_windows(fmt, &outer, &|_| true, &|_| String::new(), Some(windows))
    }

    #[test]
    fn a_window_loop_expands_once_per_window_with_that_windows_variables() {
        assert_eq!(looped("#{W:#{i}=#{n} }", 3, 9), "0=zero 1=one 2=two ");
        assert_eq!(looped("#{W:[#{n}],<#{n}>}", 3, 1), "[zero]<one>[two]");
        assert_eq!(looped("#{W:#{n}#{?loop_last_flag,,-}}", 3, 0), "zero-one-two");
        assert_eq!(looped("#{W:#{?loop_first_flag,>,}#{n}}", 3, 0), ">zeroonetwo");
        assert_eq!(looped("#{session}:#{W:#{session}}", 2, 0), "s:ss", "a name the window lacks comes from outside");
        assert_eq!(looped("#{W:x}", 0, 0), "");
    }

    #[test]
    fn a_window_loop_does_not_nest_and_needs_a_runner() {
        assert_eq!(looped("#{W:a#{W:b}}", 2, 0), "a#{W:b}a#{W:b}");
        assert_eq!(looped("#{W:a,b,c}", 2, 0), "#{W:a,b,c}", "three formats are not a loop");
        assert_eq!(expand("#{W:a}", &vars), "#{W:a}");
    }

    #[test]
    fn pane_and_session_loops_work_like_the_window_loop() {
        let names = ["a", "b"];
        let item = |i: usize, name: &str| (name == "n").then(|| names[i].to_string());
        let outer = |_: &str| None;
        let each = Windows { count: 2, current: 1, lookup: &item };
        let loops = Loops { windows: None, panes: Some(each), sessions: Some(each) };
        assert_eq!(expand_with("#{P:#{n}}", &outer, loops, None), "ab");
        assert_eq!(expand_with("#{S:[#{n}],<#{n}>}", &outer, loops, None), "[a]<b>");
        assert_eq!(expand_with("#{W:#{n}}", &outer, loops, None), "#{W:#{n}}", "a loop that was not given stays text");
        assert_eq!(expand_with("#{P:#{S:x}}", &outer, loops, None), "#{S:x}#{S:x}", "loops do not nest");
    }

    fn screen_search(text: &str, regex: bool, ignore_case: bool) -> usize {
        let lines = ["$ cargo build", "Finished dev", "error: boom"];
        let found = |line: &str| {
            if regex {
                regex_lite::RegexBuilder::new(text).case_insensitive(ignore_case).build().is_ok_and(|re| re.is_match(line))
            } else if ignore_case {
                line.to_lowercase().contains(&text.to_lowercase())
            } else {
                line.contains(text)
            }
        };
        lines.iter().position(|l| found(l)).map_or(0, |i| i + 1)
    }

    #[test]
    fn a_search_gives_the_first_line_that_holds_the_text_or_zero() {
        let none = |_: &str| None;
        let at = |fmt: &str| expand_with(fmt, &none, Loops::default(), Some(&screen_search));
        assert_eq!(at("#{C:cargo}"), "1");
        assert_eq!(at("#{C:boom}"), "3");
        assert_eq!(at("#{C:nothing}"), "0");
        assert_eq!(at("#{C:FINISHED}"), "0");
        assert_eq!(at("#{C/i:FINISHED}"), "2");
        assert_eq!(at("#{C/r:^err.r}"), "3");
        assert_eq!(at("#{C/ri:^ERR.R}"), "3");
        assert_eq!(at("#{C:}"), "0", "an empty pattern finds nothing");
        assert_eq!(at("#{C/r:[}"), "0", "a pattern that is not a regular expression finds nothing");
        assert_eq!(at(&format!("#{{C/r:{}}}", "a".repeat(513))), "0", "a long pattern is not searched");
        assert_eq!(expand_with("#{C:cargo}", &none, Loops::default(), None), "", "no search, no answer");
        assert_eq!(at("#{?#{C:boom},found,missing}"), "found");
    }

    #[test]
    fn shell_text_has_no_search() {
        let none = |_: &str| None;
        assert_eq!(expand_for_shell("echo #{C:boom}", &none, &|_| true), Ok("echo ".to_string()));
    }

    #[test]
    fn a_row_splits_into_left_center_and_right_at_its_align_directives() {
        use crate::term::cell::{BOLD, Color};
        let base = Style { fg: Color::Indexed(7), bg: Color::Indexed(0), attrs: 0 };
        let [l, c, r] = aligned_runs("a#[align=centre]b#[align=right]c", base);
        let text = |p: &Vec<(String, Style)>| p.iter().map(|(t, _)| t.as_str()).collect::<String>();
        assert_eq!((text(&l), text(&c), text(&r)), ("a".to_string(), "b".to_string(), "c".to_string()));
        let [l, c, r] = aligned_runs("#[align=right,bold]x#[align=left]y", base);
        assert_eq!((text(&l), text(&c), text(&r)), ("y".to_string(), String::new(), "x".to_string()));
        assert_eq!(r[0].1.attrs & BOLD, BOLD);
        assert_eq!(l[0].1.attrs & BOLD, BOLD, "a style carries over to the next part");
        let [l, c, r] = aligned_runs("plain", base);
        assert_eq!((text(&l), c.len(), r.len()), ("plain".to_string(), 0, 0), "no directive, all left");
        let [_, c, _] = aligned_runs("#[align=center]m#[align=absolute-centre]n", base);
        assert_eq!(text(&c), "mn");
    }

    #[test]
    fn strftime_fills_in_the_time() {
        assert_eq!(strftime("100%%"), "100%");
        assert_eq!(strftime("%Y").len(), 4);
        assert_eq!(strftime("none"), "none");
    }

    #[test]
    fn directives_split_text_into_styled_runs() {
        use crate::term::cell::{BOLD, Color};
        let base = Style { fg: Color::Indexed(7), bg: Color::Indexed(0), attrs: 0 };
        let r = runs("a#[bg=red,bold]b#[default]c#[bogus]d", base);
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].0.as_str(), r[0].1), ("a", base));
        assert_eq!(r[1].0, "b");
        assert_eq!((r[1].1.bg, r[1].1.attrs), (Color::Indexed(1), BOLD));
        assert_eq!((r[2].0.as_str(), r[2].1), ("cd", base));
    }

    fn vars(name: &str) -> Option<String> {
        match name {
            "on" => Some("1".into()),
            "off" => Some("0".into()),
            "empty" => Some(String::new()),
            "name" => Some("work".into()),
            "num" => Some("10".into()),
            _ => None,
        }
    }

    #[test]
    fn conditionals_pick_a_branch() {
        assert_eq!(expand("#{?on,yes,no}", &vars), "yes");
        assert_eq!(expand("#{?off,yes,no}", &vars), "no");
        assert_eq!(expand("#{?empty,yes,no}", &vars), "no");
        assert_eq!(expand("#{?unknown,yes,no}", &vars), "yes", "a word that is no variable is text, and text is true");
        assert_eq!(expand("#{?off,yes}", &vars), "");
        assert_eq!(expand("[#{?on,x,}]", &vars), "[x]");
        assert_eq!(expand("#{?off,a,empty,b,c}", &vars), "c", "pairs chain, the last odd one is the else");
        assert_eq!(expand("#{?off,a,on,b,c}", &vars), "b");
    }

    #[test]
    fn branches_expand_and_only_the_chosen_one_does() {
        assert_eq!(expand("#{?on,#{name}!,#{nope}}", &vars), "work!");
        assert_eq!(expand("#{?off,#{name},[#{name}]}", &vars), "[work]");
        assert_eq!(expand("#{?#{==:#{name},work},same,other}", &vars), "same");
        assert_eq!(expand("#{?#{==:#{name},play},same,other}", &vars), "other");
    }

    #[test]
    fn comparisons_compare_text() {
        assert_eq!(expand("#{==:#{name},work}", &vars), "1");
        assert_eq!(expand("#{==:#{name},play}", &vars), "0");
        assert_eq!(expand("#{!=:#{name},play}", &vars), "1");
        assert_eq!(expand("#{==:,}", &vars), "1");
        assert_eq!(expand("#{<:a,b}", &vars), "1");
        assert_eq!(expand("#{>:a,b}", &vars), "0");
        assert_eq!(expand("#{<=:b,b}", &vars), "1");
        assert_eq!(expand("#{>=:a,b}", &vars), "0");
        assert_eq!(expand("#{<:#{num},9}", &vars), "1", "text order, not numbers");
    }

    #[test]
    fn logic_operators() {
        assert_eq!(expand("#{&&:#{on},#{name}}", &vars), "1");
        assert_eq!(expand("#{&&:#{on},#{off}}", &vars), "0");
        assert_eq!(expand("#{&&:#{on},#{on},#{empty}}", &vars), "0");
        assert_eq!(expand("#{||:#{off},#{empty}}", &vars), "0");
        assert_eq!(expand("#{||:#{off},#{on}}", &vars), "1");
        assert_eq!(expand("#{!:#{off}}", &vars), "1");
        assert_eq!(expand("#{!:#{name}}", &vars), "0");
        assert_eq!(expand("#{?#{&&:#{on},#{!:#{off}}},both,not}", &vars), "both");
    }

    #[test]
    fn commas_braces_and_styles_inside_operands() {
        assert_eq!(expand("#{?on,a#,b,c}", &vars), "a,b");
        assert_eq!(expand("#{?on,{#},c}", &vars), "{}");
        assert_eq!(expand("#{==:a#,b,a#,b}", &vars), "1");
        assert_eq!(expand("#{?on,##,c}", &vars), "#");
        assert_eq!(expand("#{?on,#[fg=red,bold]hi,no}", &vars), "#[fg=red,bold]hi");
        assert_eq!(expand("#{?off,x,#[fg=red#,bold]hi}", &vars), "#[fg=red,bold]hi");
        assert_eq!(expand("a#,b#}c", &vars), "a,b}c", "the escapes work outside operators too");
    }

    #[test]
    fn nesting_has_a_depth_limit() {
        let mut deep = "x".to_string();
        for _ in 0..20 {
            deep = format!("#{{?on,{deep},}}");
        }
        assert_eq!(expand(&deep, &vars), "", "past the limit it expands to nothing");
        let mut ok = "x".to_string();
        for _ in 0..4 {
            ok = format!("#{{?on,{ok},}}");
        }
        assert_eq!(expand(&ok, &vars), "x");
    }

    #[test]
    fn malformed_operators_are_left_as_written() {
        assert_eq!(expand("#{?on}", &vars), "#{?on}");
        assert_eq!(expand("#{==:a}", &vars), "#{==:a}");
        assert_eq!(expand("#{==:a,b,c}", &vars), "#{==:a,b,c}");
        assert_eq!(expand("#{&&:a}", &vars), "#{&&:a}");
        assert_eq!(expand("#{!:a,b}", &vars), "#{!:a,b}");
        assert_eq!(expand("#{?on,a", &vars), "#{?on,a");
    }

    fn safe(name: &str) -> bool {
        matches!(name, "num" | "on" | "off")
    }

    #[test]
    fn text_for_a_shell_refuses_variables_a_pane_could_set() {
        assert_eq!(expand_for_shell("echo #{num} #{on}", &vars, &safe), Ok("echo 10 1".to_string()));
        assert_eq!(expand_for_shell("echo #{name}", &vars, &safe), Err(vec!["name".to_string()]));
        assert_eq!(expand_for_shell("echo #{name} #{name} #{num}", &vars, &safe), Err(vec!["name".to_string()]), "once each");
        assert_eq!(expand_for_shell("echo #{nope}", &vars, &safe), Ok("echo ".to_string()), "an unknown name is empty and harmless");
        assert_eq!(expand_for_shell("echo #{s/o/0/:name}", &vars, &safe), Err(vec!["name".to_string()]), "a substituted value is the same value");
    }

    #[test]
    fn the_one_letter_forms_are_checked_too() {
        let lookup = |n: &str| (n == "W").then(|| "$(boom)".to_string());
        assert_eq!(expand_for_shell("x #W", &lookup, &safe), Err(vec!["W".to_string()]));
    }

    #[test]
    fn q_quotes_a_value_for_the_shell() {
        let lookup = |n: &str| match n {
            "name" => Some("it's $(x); `y`".to_string()),
            "empty" => Some(String::new()),
            _ => None,
        };
        assert_eq!(expand("echo #{q:name}", &lookup), "echo 'it'\\''s $(x); `y`'");
        assert_eq!(expand("echo #{q:empty}#{q:nope}", &lookup), "echo ''''");
        assert_eq!(expand_for_shell("echo #{q:name}", &lookup, &safe), Ok("echo 'it'\\''s $(x); `y`'".to_string()));
        assert_eq!(shell_quote("a b"), "'a b'");
    }

    #[test]
    fn c_colon_quotes_a_value_as_one_word_of_an_imux_command() {
        assert_eq!(command_quote(r#"it's "x" $y \ ; z"#), r#""it's \"x\" \$y \\ ; z""#);
        let lookup = |n: &str| (n == "name").then(|| "a\"b; run-shell x".to_string());
        assert_eq!(expand_for_shell("set x #{c:name}", &lookup, &|_| false), Ok(r#"set x "a\"b; run-shell x""#.to_string()));
    }

    #[test]
    fn only_the_printed_branch_is_checked_not_a_tested_condition() {
        assert_eq!(expand_for_shell("#{?#{==:#{name},work},yes,no}", &vars, &safe), Ok("yes".to_string()));
        assert_eq!(expand_for_shell("#{?on,#{name},no}", &vars, &safe), Err(vec!["name".to_string()]));
        assert_eq!(expand_for_shell("#{?off,#{name},no}", &vars, &safe), Ok("no".to_string()), "a branch that is not taken is not printed");
        assert_eq!(expand_for_shell("#{?on,#{q:name},no}", &vars, &safe), Ok("'work'".to_string()));
    }

    fn ran(fmt: &str) -> String {
        let text = expand_with_commands(fmt, &vars, &safe, &|c| match c {
            Ok(text) => format!("<{text}>"),
            Err(names) => format!("refused {}", names.join(",")),
        });
        restore_hash(&text)
    }

    #[test]
    fn a_command_is_replaced_by_its_output() {
        assert_eq!(ran("a #(echo hi) b"), "a <echo hi> b");
        assert_eq!(ran("#(one)#(two)"), "<one><two>");
        assert_eq!(ran("#(date +%H)"), "<date +%H>");
    }

    #[test]
    fn parentheses_inside_a_command_are_counted() {
        assert_eq!(ran("#(echo $(date) | cut -c1)x"), "<echo $(date) | cut -c1>x");
        assert_eq!(ran("#(a (b (c)) d)"), "<a (b (c)) d>");
        assert_eq!(ran("#(a #(b) c)"), "<a #(b) c>", "an inner #( is part of the text, not a command");
    }

    #[test]
    fn a_hash_pair_never_opens_a_command() {
        assert_eq!(ran("##(echo hi)"), "#(echo hi)");
        assert_eq!(ran("#(echo ##(x))"), "<echo #(x)>");
    }

    #[test]
    fn an_unterminated_command_stays_text() {
        assert_eq!(ran("#(echo hi"), "#(echo hi");
        assert_eq!(ran("x #( y"), "x #( y");
    }

    #[test]
    fn without_a_command_runner_the_text_is_left_alone() {
        assert_eq!(expand("a #(echo hi) b", &vars), "a #(echo hi) b");
    }

    #[test]
    fn the_text_of_a_command_is_a_format() {
        assert_eq!(ran("#(echo #{num} #{on})"), "<echo 10 1>");
        assert_eq!(ran("#(echo ##x #,y #}z)"), "<echo #x ,y }z>");
        assert_eq!(ran("#(echo #{nope}.)"), "<echo .>");
    }

    #[test]
    fn a_command_cannot_take_what_a_pane_could_have_set() {
        assert_eq!(ran("#(echo #{name})"), "refused name");
        assert_eq!(ran("#(echo #{name} #{name} #{num})"), "refused name");
        assert_eq!(ran("#(echo #{q:name})"), "<echo 'work'>");
        assert_eq!(ran("#(echo #{s/o/0/:name})"), "refused name");
    }

    #[test]
    fn what_a_command_prints_is_never_a_format_or_a_style() {
        let text = expand_with_commands("[#(x)]", &vars, &safe, &|_| "#{name}#(y)#[bold]##".to_string());
        assert!(!text.contains("#{") && !text.contains("#[") && !text.contains("#("), "{text:?}");
        assert_eq!(runs(&text, Style::default()).len(), 1, "no style directive came out of it");
        assert_eq!(restore_hash(&text), "[#{name}#(y)#[bold]##]");
    }

    #[test]
    fn a_command_works_in_a_conditional() {
        assert_eq!(ran("#{?#(x),yes,no}"), "yes");
        assert_eq!(ran("#{?off,#(a),#(b)}"), "<b>");
        assert_eq!(ran("#{?on,#(a#,b),no}"), "<a,b>");
    }

    #[test]
    fn commands_nest_in_depth_like_everything_else() {
        let mut deep = "x".to_string();
        for _ in 0..20 {
            deep = format!("#{{?on,{deep},}}");
        }
        assert_eq!(ran(&format!("{deep}#(y)")), "<y>");
    }

    fn world(name: &str) -> Option<String> {
        Some(
            match name {
                "path" => "/home/me/work/imux/",
                "title" => "héllo wörld",
                "empty" => "",
                "fmt" => "[#{title}]",
                "n" => "7",
                "@theme" => "mocha-#{n}",
                "stamp" => "%Y",
                _ => return None,
            }
            .to_string(),
        )
    }

    #[test]
    fn a_value_is_cut_to_its_first_or_last_characters() {
        assert_eq!(expand("#{=5:title}", &world), "héllo");
        assert_eq!(expand("#{=-5:title}", &world), "wörld");
        assert_eq!(expand("#{=50:title}", &world), "héllo wörld");
        assert_eq!(expand("#{=-50:title}", &world), "héllo wörld");
        assert_eq!(expand("#{=0:title}", &world), "");
        assert_eq!(expand("#{=3:nope}", &world), "");
    }

    #[test]
    fn basename_dirname_and_length() {
        assert_eq!(expand("#{b:path}", &world), "imux");
        assert_eq!(expand("#{d:path}", &world), "/home/me/work");
        assert_eq!(expand("#{n:title}", &world), "11");
        assert_eq!(expand("#{b:empty}", &world), "");
        assert_eq!(basename("/"), "/");
        assert_eq!(basename("a"), "a");
        assert_eq!(basename("/a"), "a");
        assert_eq!(dirname("a"), ".");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("/"), "/");
        assert_eq!(dirname("a/b"), "a");
        assert_eq!(dirname(""), ".");
    }

    #[test]
    fn a_modifier_works_on_a_nested_format_and_composes_with_the_others() {
        assert_eq!(expand("#{b:#{path}}", &world), "imux");
        assert_eq!(expand("#{=3:#{b:path}}", &world), "imu");
        assert_eq!(expand("#{?#{==:#{b:path},imux},in,out}", &world), "in");
    }

    #[test]
    fn a_literal_is_not_looked_up() {
        assert_eq!(expand("#{l:title}", &world), "title");
        assert_eq!(expand("#{l:a#,b#}c}", &world), "a,b}c");
    }

    #[test]
    fn expand_modifiers_read_a_value_as_a_format() {
        assert_eq!(expand("#{fmt}", &world), "[#{title}]");
        assert_eq!(expand("#{E:fmt}", &world), "[héllo wörld]");
        assert_eq!(expand("#{E:@theme}", &world), "mocha-7");
        assert_eq!(expand("#{E:nope}", &world), "");
        assert!(expand("#{T:stamp}", &world).chars().all(|c| c.is_ascii_digit()));
        assert_eq!(expand("#{T:stamp}", &world).len(), 4);
    }

    #[test]
    fn match_by_glob_and_by_regular_expression() {
        assert_eq!(expand("#{m:*work*,#{path}}", &world), "1");
        assert_eq!(expand("#{m:work,#{path}}", &world), "0");
        assert_eq!(expand("#{m:h?llo*,#{title}}", &world), "1");
        assert_eq!(expand("#{m:WORK,work}", &world), "0");
        assert_eq!(expand("#{m/i:WORK,work}", &world), "1");
        assert_eq!(expand("#{m/r:^/home/.*/$,#{path}}", &world), "1");
        assert_eq!(expand("#{m/ri:WORK,#{path}}", &world), "1");
        assert_eq!(expand("#{m/r:(,x}", &world), "0", "a bad expression matches nothing");
        assert_eq!(expand("#{?#{m:*imux*,#{path}},yes,no}", &world), "yes");
        assert_eq!(expand("#{m:onlyone}", &world), "", "a malformed one is an unknown variable");
    }

    #[test]
    fn the_glob_does_not_blow_up() {
        let pattern: Vec<char> = "*a*a*a*a*a*a*a*a*a*b".chars().collect();
        let text: Vec<char> = "a".repeat(200).chars().collect();
        assert!(!glob(&pattern, &text));
        assert!(glob(&['*'], &[]));
        assert!(!glob(&['?'], &[]));
        assert!(glob(&"a*c".chars().collect::<Vec<_>>(), &"abbbc".chars().collect::<Vec<_>>()));
    }

    #[test]
    fn a_regular_expression_over_the_cap_matches_nothing() {
        let long = "a".repeat(MAX_PATTERN + 1);
        assert_eq!(expand(&format!("#{{m/r:{long},{long}}}"), &world), "0");
    }

    #[test]
    fn arithmetic_on_integers() {
        assert_eq!(expand("#{e|+|:#{n},3}", &world), "10");
        assert_eq!(expand("#{e|-|:3,#{n}}", &world), "-4");
        assert_eq!(expand("#{e|*|:#{n},6}", &world), "42");
        assert_eq!(expand("#{e|/|:#{n},2}", &world), "3");
        assert_eq!(expand("#{e|m|:#{n},4}", &world), "3");
        assert_eq!(expand("#{e|/|:1,0}", &world), "0");
        assert_eq!(expand("#{e|>|:#{n},3}", &world), "1");
        assert_eq!(expand("#{e|==|:#{n},3}", &world), "0");
        assert_eq!(expand("#{e|+|:x,2}", &world), "2", "text counts as 0");
        assert_eq!(expand("#{e|^|:1,2}", &world), "", "an unknown operator is an unknown variable");
    }

    #[test]
    fn a_modifier_under_the_shell_check_still_refuses_what_a_pane_could_set() {
        let safe = |n: &str| n == "n";
        assert_eq!(expand_for_shell("#{=3:title}", &world, &safe), Err(vec!["title".to_string()]));
        assert_eq!(expand_for_shell("#{b:path}", &world, &safe), Err(vec!["path".to_string()]));
        assert_eq!(expand_for_shell("#{=1:n}", &world, &safe), Ok("7".to_string()));
        assert!(expand_for_shell("#{E:fmt}", &world, &safe).is_err());
    }

    #[test]
    fn an_operator_value_is_not_expanded_again() {
        let lookup = |n: &str| (n == "title").then(|| "#{name}#(x)#[bold]".to_string());
        assert_eq!(expand("#{?title,#{title},}", &lookup), "#{name}#(x)#[bold]");
        assert_eq!(expand("#{==:#{title},q}", &lookup), "0");
    }

    #[test]
    fn a_marker_shows_where_a_value_was_cut() {
        assert_eq!(expand("#{=/5/...:title}", &world), "héllo...");
        assert_eq!(expand("#{=/-5/...:title}", &world), "...wörld");
        assert_eq!(expand("#{=/50/...:title}", &world), "héllo wörld", "nothing cut, no marker");
        assert_eq!(expand("#{=/5/:title}", &world), "héllo", "an empty marker is a plain cut");
        assert_eq!(expand("#{=/5/>>:#{b:path}}", &world), "imux");
        assert_eq!(expand("#{=/x/..:title}", &world), "", "a count that is no number is an unknown variable");
    }

    #[test]
    fn the_width_counts_cells() {
        assert_eq!(expand("#{w:title}", &world), "11");
        assert_eq!(expand("#{w:#{title}日本}", &world), "15");
        assert_eq!(expand("#{w:empty}", &world), "0");
    }

    #[test]
    fn padding_fills_to_a_width_on_the_right_or_the_left() {
        assert_eq!(expand("#{p12:title}|", &world), "héllo wörld |");
        assert_eq!(expand("#{p-12:title}|", &world), " héllo wörld|");
        assert_eq!(expand("#{p5:title}|", &world), "héllo wörld|", "a longer value is left alone");
        assert_eq!(expand("#{p4:empty}|", &world), "    |");
        assert_eq!(expand("#{p99999999:empty}|", &world).len(), 4097, "the fill is capped");
        assert_eq!(expand("#{pz:title}", &world), "", "a width that is no number is an unknown variable");
    }

    #[test]
    fn a_time_is_shown_plain_formatted_or_short() {
        let lookup = |n: &str| match n {
            "when" => Some("1700000000".to_string()),
            "never" => Some("soon".to_string()),
            _ => None,
        };
        assert_eq!(expand("#{t/f/%Y:when}", &lookup), "2023");
        assert!(expand("#{t:when}", &lookup).ends_with(" 2023"));
        assert!(expand("#{t:when}", &lookup).starts_with("Tue Nov 1") || expand("#{t:when}", &lookup).starts_with("Wed Nov 1"));
        assert_eq!(expand("#{t:never}", &lookup), "");
        assert_eq!(expand("#{t/f/%Y:nope}", &lookup), "");
        assert_eq!(expand("#{t/f/%Y:#{when}}", &lookup), "2023");
        assert_eq!(expand("#{t:99999999999999}", &lookup), "", "a year past 9999 is not a time");
    }

    #[test]
    fn a_short_time_depends_on_its_age() {
        assert_eq!(pretty_time_format(3600, true, true), "%H:%M");
        assert_eq!(pretty_time_format(24 * 3600 - 1, false, false), "%H:%M");
        assert_eq!(pretty_time_format(24 * 3600, true, true), "%a%d");
        assert_eq!(pretty_time_format(30 * 24 * 3600, false, true), "%d%b");
        assert_eq!(pretty_time_format(400 * 24 * 3600, false, false), "%b%y");
        let now = 1_700_000_000;
        let recent = pretty_time(now - 600, now);
        assert_eq!(recent.len(), 5);
        assert_eq!(recent.as_bytes()[2], b':');
        assert_eq!(pretty_time(now - 3 * 365 * 24 * 3600, now).len(), 5, "month and two-digit year, as in Nov20");
    }
}
