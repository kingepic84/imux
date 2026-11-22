//! Splitting configuration and prompt text into statements of words, tmux style: `#` comments,
//! single and double quotes, `${name}` / `$name` expansion of variables set with `name=value`,
//! a leading `~`, and `;` (written `\;` in files) between commands on one line.
//!
//! A word that is a lone `{` starts a command block that ends at the matching lone `}`; the block
//! may span lines and nest, its commands are separated by newlines or `;`, and it becomes ONE word:
//! the commands joined by ` ; `, with comments dropped and quoted text kept as written. The commands
//! that take a command (`bind`, `if-shell`, `run-shell`, `set-hook`, aliases, the prompt) read such
//! a word as a command line, so `bind x { new-window; split-window }` is `bind x new-window \;
//! split-window`.

use std::collections::HashMap;

pub type Vars = HashMap<String, String>;

/// Deepest nesting of blocks inside blocks.
const MAX_BLOCK_DEPTH: usize = 8;
/// Longest block, in bytes of its text.
const MAX_BLOCK: usize = 64 * 1024;

/// Whether what follows (`None` is the end of the line) ends a word: a brace is a block brace only
/// when it stands alone.
fn is_break(next: Option<&char>) -> bool {
    next.is_none_or(|c| c.is_whitespace() || *c == ';')
}

/// Whether a lone brace that follows `before` (the words of the statement so far) names the key of
/// `bind` or `unbind` and so is not a block brace: `unbind {`, `bind } swap-pane -D`,
/// `bind -T prefix { swap-pane -U`. A brace after the key (`bind x { ... }`) is a block.
fn brace_is_key(before: &[String]) -> bool {
    let Some((name, rest)) = before.split_first() else { return false };
    if !matches!(name.as_str(), "bind" | "bind-key" | "unbind" | "unbind-key") {
        return false;
    }
    let mut it = rest.iter();
    while let Some(word) = it.next() {
        match word.as_str() {
            "-T" | "-N" => {
                it.next();
            }
            flag if flag.len() > 1 && flag.starts_with('-') => {}
            _ => return false,
        }
    }
    true
}

/// The words of the statement being read: those after the last `;`.
fn statement_so_far(out: &[String]) -> &[String] {
    let start = out.iter().rposition(|w| w == ";").map_or(0, |i| i + 1);
    &out[start..]
}

/// Follows block braces over lines so that a block can be joined into one logical line: how deep
/// the open blocks are, and whether a quote is open (quotes may span lines inside a block).
#[derive(Default)]
struct Scan {
    depth: usize,
    quote: Option<char>,
    escape: bool,
    fresh: bool,
    /// The words of the statement on this line before the one being read, and that word so far, while
    /// no block is open: they tell a brace that is the key of `bind` or `unbind` from a block brace.
    head: Vec<String>,
    word: String,
}

impl Scan {
    fn new() -> Scan {
        Scan { fresh: true, ..Scan::default() }
    }

    fn end_word(&mut self) {
        if self.depth == 0 && !self.word.is_empty() && self.head.len() < 16 {
            self.head.push(std::mem::take(&mut self.word));
        } else {
            self.word.clear();
        }
    }

    fn feed(&mut self, line: &str) {
        if self.depth == 0 && self.quote.is_none() {
            self.head.clear();
            self.word.clear();
        }
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match self.quote {
                Some('\'') => {
                    if c == '\'' {
                        self.quote = None;
                    }
                }
                Some(_) => {
                    if self.escape {
                        self.escape = false;
                    } else if c == '\\' {
                        self.escape = true;
                    } else if c == '"' {
                        self.quote = None;
                    }
                }
                None => match c {
                    '#' if self.fresh => break,
                    c if c.is_whitespace() || c == ';' => {
                        self.end_word();
                        if c == ';' {
                            self.head.clear();
                        }
                        self.fresh = true;
                    }
                    '\\' => {
                        let semicolon = chars.next() == Some(';');
                        if semicolon {
                            self.end_word();
                            self.head.clear();
                        } else if self.depth == 0 && self.word.len() < 32 {
                            self.word.push('\\');
                        }
                        self.fresh = semicolon;
                    }
                    '\'' | '"' => {
                        self.quote = Some(c);
                        self.fresh = false;
                        if self.depth == 0 && self.word.len() < 32 {
                            self.word.push(c);
                        }
                    }
                    '{' if self.fresh && is_break(chars.peek()) && !brace_is_key(&self.head) => {
                        self.depth += 1;
                        self.fresh = false;
                        self.head.clear();
                        self.word.clear();
                    }
                    '}' if self.fresh && is_break(chars.peek()) && !brace_is_key(&self.head) => {
                        self.depth = self.depth.saturating_sub(1);
                        self.fresh = false;
                    }
                    _ => {
                        self.fresh = false;
                        if self.depth == 0 && self.word.len() < 32 {
                            self.word.push(c);
                        }
                    }
                },
            }
        }
        if self.quote.is_none() {
            self.fresh = true;
        }
    }

    /// A block is open, or a quote inside one.
    fn open(&self) -> bool {
        self.depth > 0
    }
}

/// End a command inside a block text: one ` ; `, not after another separator, not first, not right
/// after an opening brace.
fn separate(out: &mut String) {
    out.truncate(out.trim_end().len());
    let after_brace = out == "{" || out.ends_with(" {");
    if !out.is_empty() && !out.ends_with(';') && !after_brace {
        out.push_str(" ; ");
    }
}

/// The block after its opening `{` (already read) up to the matching `}`, as one string: commands
/// separated by ` ; `, comments gone, quoted text as it was written (it is read again when the
/// block is used).
fn block(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<String, String> {
    let mut out = String::new();
    let mut depth = 1usize;
    let mut fresh = true;
    loop {
        if out.len() > MAX_BLOCK {
            return Err(format!("a {{ }} block is over {MAX_BLOCK} bytes"));
        }
        let Some(c) = chars.next() else { return Err("a { block has no matching }".to_string()) };
        match c {
            '\n' | ';' => {
                separate(&mut out);
                fresh = true;
            }
            c if c.is_whitespace() => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                fresh = true;
            }
            '#' if fresh => {
                while chars.peek().is_some_and(|&n| n != '\n') {
                    chars.next();
                }
            }
            '\\' => {
                if chars.peek() == Some(&';') {
                    chars.next();
                    separate(&mut out);
                    fresh = true;
                } else {
                    out.push('\\');
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                    fresh = false;
                }
            }
            '\'' => {
                out.push('\'');
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(q) => out.push(q),
                        None => return Err("unterminated quote".to_string()),
                    }
                }
                out.push('\'');
                fresh = false;
            }
            '"' => {
                out.push('"');
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => {
                            out.push('\\');
                            out.push(chars.next().ok_or_else(|| "unterminated quote".to_string())?);
                        }
                        Some(q) => out.push(q),
                        None => return Err("unterminated quote".to_string()),
                    }
                }
                out.push('"');
                fresh = false;
            }
            '{' if fresh && is_break(chars.peek()) => {
                depth += 1;
                if depth > MAX_BLOCK_DEPTH {
                    return Err(format!("{{ blocks are nested deeper than {MAX_BLOCK_DEPTH}"));
                }
                out.push('{');
                fresh = false;
            }
            '}' if fresh && is_break(chars.peek()) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(out.trim_matches(|c: char| c.is_whitespace() || c == ';').to_string());
                }
                out.truncate(out.trim_end_matches(|c: char| c.is_whitespace() || c == ';').len());
                out.push_str(" }");
                fresh = false;
            }
            c => {
                out.push(c);
                fresh = false;
            }
        }
    }
}

/// Longest value a variable may be given, in bytes.
const MAX_VAR: usize = 64 * 1024;
/// Most variables set at once.
const MAX_VARS: usize = 1024;
/// Longest word after variables are put in. Without a limit, `A=$A$A` on every line of a file
/// doubles a variable each time and the server runs out of memory while loading it (found by
/// fuzzing).
const MAX_EXPANDED: usize = 256 * 1024;

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replace `${name}` and `$name`; unknown names expand to nothing, like the shell.
fn expand(s: &str, vars: &Vars, out: &mut String) {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if out.len() > MAX_EXPANDED {
            return;
        }
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let (name, next) = if chars.get(i + 1) == Some(&'{') {
            match chars[i + 2..].iter().position(|&c| c == '}') {
                Some(end) => (chars[i + 2..i + 2 + end].iter().collect::<String>(), i + 3 + end),
                None => {
                    out.push('$');
                    i += 1;
                    continue;
                }
            }
        } else {
            let end = chars[i + 1..].iter().position(|c| !(c.is_ascii_alphanumeric() || *c == '_')).unwrap_or(chars.len() - i - 1);
            (chars[i + 1..i + 1 + end].iter().collect::<String>(), i + 1 + end)
        };
        if name.is_empty() || !is_name(&name) {
            out.push('$');
            i += 1;
            continue;
        }
        if let Some(v) = vars.get(&name) {
            out.push_str(v);
        }
        i = next;
    }
}

/// Words of one logical line, with `;` words kept as separate `";"` entries.
fn words(line: &str, vars: &Vars) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '#' if !started => break,
            c if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\\' if chars.peek() == Some(&';') => {
                chars.next();
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
                out.push(";".to_string());
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unterminated quote".to_string()),
                    }
                }
            }
            '"' => {
                started = true;
                let mut raw = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$')) => {
                                if c == '$' {
                                    raw.push('\u{1}');
                                } else {
                                    raw.push(c);
                                }
                            }
                            Some(c) => {
                                raw.push('\\');
                                raw.push(c);
                            }
                            None => return Err("unterminated quote".to_string()),
                        },
                        Some(c) => raw.push(c),
                        None => return Err("unterminated quote".to_string()),
                    }
                }
                let mut expanded = String::new();
                expand(&raw, vars, &mut expanded);
                cur.push_str(&expanded.replace('\u{1}', "$"));
                if cur.len() > MAX_EXPANDED {
                    return Err(format!("a word is over {MAX_EXPANDED} bytes after variables are put in"));
                }
            }
            '{' if !started && is_break(chars.peek()) && !brace_is_key(statement_so_far(&out)) => {
                started = true;
                cur.push_str(&block(&mut chars)?);
            }
            '}' if !started && is_break(chars.peek()) && !brace_is_key(statement_so_far(&out)) => {
                return Err("unexpected } with no { block open".to_string());
            }
            '~' if !started && matches!(chars.peek(), None | Some('/')) => {
                started = true;
                cur.push_str(&std::env::var("HOME").unwrap_or_else(|_| "~".to_string()));
            }
            c => {
                started = true;
                let mut piece = String::new();
                piece.push(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || matches!(n, '\'' | '"' | '\\') {
                        break;
                    }
                    piece.push(n);
                    chars.next();
                }
                expand(&piece, vars, &mut cur);
                if cur.len() > MAX_EXPANDED {
                    return Err(format!("a word is over {MAX_EXPANDED} bytes after variables are put in"));
                }
            }
        }
    }
    if started {
        out.push(cur);
    }
    Ok(out)
}

/// Statements of one logical line: a variable assignment (returned through `vars`) yields none.
/// A word `;` ends a statement.
pub fn statements(line: &str, vars: &mut Vars) -> Result<Vec<Vec<String>>, String> {
    let words = words(line, vars)?;
    if let Some(first) = words.first()
        && let Some((name, value)) = first.split_once('=')
        && words.len() == 1
        && is_name(name)
    {
        if value.len() > MAX_VAR {
            return Err(format!("variable {name} is over {MAX_VAR} bytes"));
        }
        if vars.len() >= MAX_VARS && !vars.contains_key(name) {
            return Err(format!("more than {MAX_VARS} variables"));
        }
        vars.insert(name.to_string(), value.to_string());
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for w in words {
        if w == ";" {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(w);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Join physical lines that end in a backslash (but not `\;`) into logical lines, with line numbers;
/// then the lines of a `{ }` block into the one logical line the block starts on (newlines kept).
pub fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut open: Option<Scan> = None;
    for (n, line) in joined_lines(text) {
        if let (Some(scan), Some((_, acc))) = (open.as_mut(), out.last_mut()) {
            if acc.len() > MAX_BLOCK + 1024 {
                open = None;
            } else {
                acc.push('\n');
                acc.push_str(&line);
                scan.feed(&line);
                if !scan.open() {
                    open = None;
                }
                continue;
            }
        }
        let mut scan = Scan::new();
        scan.feed(&line);
        if scan.open() {
            open = Some(scan);
        }
        out.push((n, line));
    }
    out
}

/// The lines of `text` with the ones that end in a backslash joined to the next.
fn joined_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (i, line) in text.lines().enumerate() {
        let joined = match pending.take() {
            Some((n, mut s)) => {
                s.push_str(line);
                (n, s)
            }
            None => (i + 1, line.to_string()),
        };
        if joined.1.ends_with('\\') && !joined.1.ends_with("\\;") && !joined.1.ends_with("\\\\") {
            let (n, mut s) = joined;
            s.pop();
            pending = Some((n, s));
        } else {
            out.push(joined);
        }
    }
    if let Some(p) = pending {
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(line: &str) -> Vec<Vec<String>> {
        statements(line, &mut Vars::new()).unwrap()
    }

    fn w(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn comments_blank_lines_and_plain_words() {
        assert!(st("# a comment").is_empty());
        assert!(st("   ").is_empty());
        assert_eq!(st("set -g mouse on  # trailing"), [w(&["set", "-g", "mouse", "on"])]);
    }

    #[test]
    fn quotes_group_and_hash_inside_quotes_is_text() {
        assert_eq!(st(r##"set -g status-left "#[fg=red] #S ""##), [w(&["set", "-g", "status-left", "#[fg=red] #S "])]);
        assert_eq!(st("display 'a b'"), [w(&["display", "a b"])]);
        assert_eq!(st(r#"x "" y"#), [w(&["x", "", "y"])]);
        assert!(statements("a \"open", &mut Vars::new()).is_err());
    }

    #[test]
    fn backslash_semicolon_separates_commands() {
        assert_eq!(
            st(r#"bind r source-file ~/.tmux.conf \; display "Config reloaded""#),
            [w(&["bind", "r", "source-file", &format!("{}/.tmux.conf", std::env::var("HOME").unwrap_or("~".into()))]), w(&["display", "Config reloaded"])]
        );
    }

    #[test]
    fn variables_are_assigned_and_expanded() {
        let mut vars = Vars::new();
        assert!(statements(r##"blue="#89b4fa""##, &mut vars).unwrap().is_empty());
        assert_eq!(vars["blue"], "#89b4fa");
        assert_eq!(
            statements(r#"set -g pane-active-border-style "fg=${blue}""#, &mut vars).unwrap(),
            [w(&["set", "-g", "pane-active-border-style", "fg=#89b4fa"])]
        );
        assert_eq!(statements("echo $blue", &mut vars).unwrap(), [w(&["echo", "#89b4fa"])]);
    }

    #[test]
    fn format_strings_survive() {
        assert_eq!(
            st(r##"bind | split-window -h -c "#{pane_current_path}""##),
            [w(&["bind", "|", "split-window", "-h", "-c", "#{pane_current_path}"])]
        );
    }

    #[test]
    fn backslash_newline_joins_lines() {
        assert_eq!(logical_lines("a \\\nb\nc"), [(1, "a b".to_string()), (3, "c".to_string())]);
        assert_eq!(logical_lines("x \\;\ny").len(), 2);
    }

    #[test]
    fn a_variable_that_doubles_on_every_line_is_stopped_before_it_eats_the_memory() {
        let mut vars = Vars::new();
        statements("A=aaaaaaaaaaaaaaaa", &mut vars).unwrap();
        let mut refused = None;
        for n in 0..40 {
            if let Err(e) = statements("A=$A$A", &mut vars) {
                refused = Some((n, e));
                break;
            }
        }
        let (n, e) = refused.expect("the doubling stops");
        assert!(n < 20, "stopped after {n} lines");
        assert!(e.contains("bytes"), "{e}");
        assert!(vars["A"].len() <= MAX_VAR);
    }

    #[test]
    fn many_references_to_a_large_variable_make_an_error_not_a_huge_word() {
        let mut vars = Vars::new();
        vars.insert("B".to_string(), "b".repeat(MAX_VAR));
        let line = "echo ".to_string() + &"$B".repeat(1000);
        assert!(statements(&line, &mut vars).is_err());
        let quoted = "echo \"".to_string() + &"$B".repeat(1000) + "\"";
        assert!(statements(&quoted, &mut vars).is_err());
    }

    #[test]
    fn the_number_of_variables_is_limited_but_a_known_one_can_change() {
        let mut vars = Vars::new();
        for i in 0..MAX_VARS {
            statements(&format!("V{i}=x"), &mut vars).unwrap();
        }
        assert!(statements("EXTRA=x", &mut vars).is_err());
        statements("V0=y", &mut vars).unwrap();
        assert_eq!(vars["V0"], "y");
    }
}

#[cfg(test)]
mod block_tests {
    use super::*;

    fn st(line: &str) -> Vec<Vec<String>> {
        statements(line, &mut Vars::new()).unwrap()
    }

    fn w(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn lines(text: &str) -> Vec<(usize, String)> {
        logical_lines(text)
    }

    #[test]
    fn a_block_on_one_line_is_one_word_with_its_commands_joined() {
        assert_eq!(st("bind x { new-window }"), [w(&["bind", "x", "new-window"])]);
        assert_eq!(st("bind x { new-window; split-window -h }"), [w(&["bind", "x", "new-window ; split-window -h"])]);
        assert_eq!(st("bind x { new-window \\; kill-pane }"), [w(&["bind", "x", "new-window ; kill-pane"])]);
        assert_eq!(st("bind x {new-window}"), [w(&["bind", "x", "{new-window}"])], "a brace that does not stand alone is text");
    }

    #[test]
    fn a_block_over_several_lines_is_one_logical_line_and_newlines_separate_commands() {
        let text = "set -g mouse on\nbind x {\n  new-window\n  split-window -h\n}\nset -g status on\n";
        let all = lines(text);
        assert_eq!(all.len(), 3);
        assert_eq!((all[0].0, all[1].0, all[2].0), (1, 2, 6), "line numbers are those of the first line");
        assert_eq!(st(&all[1].1), [w(&["bind", "x", "new-window ; split-window -h"])]);
    }

    #[test]
    fn comments_inside_a_block_are_dropped_and_quotes_are_kept_as_written() {
        assert_eq!(st("bind x { new-window # open one\n kill-pane }"), [w(&["bind", "x", "new-window ; kill-pane"])]);
        assert_eq!(st("bind x { display \"a; b }\" 'c\nd' }"), [w(&["bind", "x", "display \"a; b }\" 'c\nd'"])]);
        assert_eq!(st("bind x { display \"say \\\"}\\\"\" }"), [w(&["bind", "x", "display \"say \\\"}\\\"\""])]);
    }

    #[test]
    fn blocks_nest() {
        let got = st("bind x { if-shell true { new-window; kill-pane } { split-window } }");
        assert_eq!(got, [w(&["bind", "x", "if-shell true { new-window ; kill-pane } { split-window }"])]);
        let inner = st(&got[0][2]);
        assert_eq!(inner, [w(&["if-shell", "true", "new-window ; kill-pane", "split-window"])]);
    }

    #[test]
    fn a_block_can_be_an_argument_among_others() {
        assert_eq!(st("if-shell -F x { a } { b }"), [w(&["if-shell", "-F", "x", "a", "b"])]);
        assert_eq!(st("set-hook -g after-new-window { a; b } ; set -g mouse on"), [w(&["set-hook", "-g", "after-new-window", "a ; b"]), w(&["set", "-g", "mouse", "on"])]);
    }

    #[test]
    fn a_brace_that_is_the_key_of_bind_or_unbind_is_a_key_not_a_block() {
        assert_eq!(st("unbind {"), [w(&["unbind", "{"])]);
        assert_eq!(st("unbind }"), [w(&["unbind", "}"])]);
        assert_eq!(st("unbind-key -n {"), [w(&["unbind-key", "-n", "{"])]);
        assert_eq!(st("bind { swap-pane -U"), [w(&["bind", "{", "swap-pane", "-U"])]);
        assert_eq!(st("bind -T prefix } swap-pane -D"), [w(&["bind", "-T", "prefix", "}", "swap-pane", "-D"])]);
        assert_eq!(st("unbind } ; unbind {"), [w(&["unbind", "}"]), w(&["unbind", "{"])]);
        assert_eq!(st("bind x { new-window }"), [w(&["bind", "x", "new-window"])], "after the key a brace opens a block");
        assert!(statements("bind x new-window }", &mut Vars::new()).unwrap_err().contains("unexpected }"));
        assert!(statements("display {", &mut Vars::new()).unwrap_err().contains("no matching"), "only bind and unbind take a key");
    }

    #[test]
    fn a_brace_key_does_not_swallow_the_lines_after_it() {
        let text = "unbind }\nunbind {\nset -g mouse on\nbind x {\n  new-window\n}\nset -g status on\n";
        let all = lines(text);
        assert_eq!(all.len(), 5, "{all:?}");
        assert_eq!(all[2].1, "set -g mouse on");
        assert_eq!(st(&all[3].1), [w(&["bind", "x", "new-window"])]);
        assert_eq!(all[4].1, "set -g status on");
    }

    #[test]
    fn braces_that_are_not_alone_are_ordinary_text() {
        assert_eq!(st("display ${HOME_X}x"), [w(&["display", "x"])]);
        assert_eq!(st("display a{b}c"), [w(&["display", "a{b}c"])]);
        assert_eq!(st("display \"#{pane_id}\""), [w(&["display", "#{pane_id}"])]);
        assert_eq!(st("display {x"), [w(&["display", "{x"])]);
        assert_eq!(st("display x}"), [w(&["display", "x}"])]);
    }

    #[test]
    fn a_stray_closing_brace_or_an_open_block_is_an_error() {
        assert!(statements("}", &mut Vars::new()).unwrap_err().contains("unexpected }"));
        assert!(statements("bind x new-window }", &mut Vars::new()).unwrap_err().contains("unexpected }"));
        assert!(statements("bind x { new-window", &mut Vars::new()).unwrap_err().contains("no matching }"));
        assert!(statements("bind x { display 'a }", &mut Vars::new()).unwrap_err().contains("unterminated"));
    }

    #[test]
    fn the_depth_and_the_size_of_a_block_are_limited() {
        let deep = format!("a {} x {}", "{ b".repeat(MAX_BLOCK_DEPTH + 1).replace("b", ""), "}".repeat(MAX_BLOCK_DEPTH + 1));
        assert!(statements(&deep, &mut Vars::new()).unwrap_err().contains("nested deeper"));
        let ok = format!("a {} x {}", "{".repeat(MAX_BLOCK_DEPTH), "}".repeat(MAX_BLOCK_DEPTH));
        assert!(statements(&ok, &mut Vars::new()).is_ok());
        let big = format!("a {{ {} }}", "x ".repeat(MAX_BLOCK));
        assert!(statements(&big, &mut Vars::new()).unwrap_err().contains("over"));
    }

    #[test]
    fn braces_in_comments_and_quotes_do_not_open_or_close_blocks_when_lines_are_joined() {
        assert_eq!(lines("# { not a block\nset -g a b\n").len(), 2);
        assert_eq!(lines("display \"{\"\nset -g a b\n").len(), 2);
        assert_eq!(lines("bind x { new-window # }\n}\nset -g a b\n").len(), 2);
        assert_eq!(lines("display '{' ; display x\nset a b\n").len(), 2);
    }

    #[test]
    fn a_block_that_never_closes_ends_up_in_one_line_and_is_reported_there() {
        let all = lines("set a b\nbind x {\n new-window\nset c d\n");
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].0, 2);
        assert!(statements(&all[1].1, &mut Vars::new()).unwrap_err().contains("no matching }"));
    }

    #[test]
    fn a_backslash_at_the_end_of_a_line_still_joins_inside_and_outside_blocks() {
        assert_eq!(lines("set -g a \\\nb\n").len(), 1);
        let all = lines("bind x { new-window \\\n kill-pane }\n");
        assert_eq!(all.len(), 1);
        assert_eq!(st(&all[0].1), [w(&["bind", "x", "new-window kill-pane"])]);
    }

    #[test]
    fn a_huge_open_block_is_not_joined_forever() {
        let text = format!("bind x {{\n{}", "set a b\n".repeat(20_000));
        let all = lines(&text);
        assert!(all.len() > 1, "joining stops once the block is over the size limit");
    }
}
