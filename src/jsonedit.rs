//! Putting imux's hook entries into a JSON settings file (Claude Code's `settings.json`) without
//! rewriting the file: the text is read once, with the place of every value, and the changes are
//! made as splices of the original text, so the person's own entries, their order, their spacing and
//! their other settings stay exactly as they were. Nothing here writes a file; the caller writes
//! the result, after it was read back once more.
//!
//! The reader takes strict JSON only (no comments, no trailing commas) and gives up, with a
//! reason, on anything else, on a file larger than `MAX_BYTES`, and on nesting deeper than
//! `MAX_DEPTH`. A file it cannot read is never changed.

const MAX_BYTES: usize = 4 << 20;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Str(String),
    /// A number, `true`, `false`, or `null`: nothing here needs more than where it is.
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub value: Value,
    /// Byte range of the value in the text: `start` is its first byte and `end` the one after the last.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub key: String,
    pub node: Node,
}

struct Reader<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
}

/// Read `text` as one JSON value.
pub fn parse(text: &str) -> Result<Node, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("the file is larger than {MAX_BYTES} bytes"));
    }
    let mut r = Reader { text, bytes: text.as_bytes(), at: 0 };
    // a byte order mark is part of the file, not of the value
    if text.starts_with('\u{feff}') {
        r.at = '\u{feff}'.len_utf8();
    }
    r.skip_space();
    let node = r.value(0)?;
    r.skip_space();
    if r.at != r.bytes.len() {
        return Err(format!("unexpected text after the value at byte {}", r.at));
    }
    Ok(node)
}

impl Reader<'_> {
    fn skip_space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn fail<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.at))
    }

    fn value(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return self.fail("nesting is too deep");
        }
        let start = self.at;
        let value = match self.peek() {
            Some(b'{') => self.object(depth)?,
            Some(b'[') => self.array(depth)?,
            Some(b'"') => Value::Str(self.string()?),
            Some(b'-' | b'0'..=b'9') => {
                self.number()?;
                Value::Other
            }
            Some(b't') => self.literal("true")?,
            Some(b'f') => self.literal("false")?,
            Some(b'n') => self.literal("null")?,
            _ => return self.fail("a value was expected"),
        };
        Ok(Node { value, start, end: self.at })
    }

    fn literal(&mut self, word: &str) -> Result<Value, String> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(Value::Other)
        } else {
            self.fail("a value was expected")
        }
    }

    fn number(&mut self) -> Result<(), String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        let digits = |r: &mut Self| {
            let from = r.at;
            while matches!(r.peek(), Some(b'0'..=b'9')) {
                r.at += 1;
            }
            r.at > from
        };
        if self.peek() == Some(b'0') {
            self.at += 1;
        } else if !digits(self) {
            return self.fail("a number was expected");
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if !digits(self) {
                return self.fail("digits were expected after the point");
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !digits(self) {
                return self.fail("digits were expected in the exponent");
            }
        }
        debug_assert!(self.at > start);
        Ok(())
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1; // the opening quote
        let mut out = String::new();
        loop {
            let Some(c) = self.text[self.at..].chars().next() else { return self.fail("the string does not end") };
            match c {
                '"' => {
                    self.at += 1;
                    return Ok(out);
                }
                '\\' => {
                    self.at += 1;
                    let Some(e) = self.peek() else { return self.fail("the string does not end") };
                    self.at += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let ch = if (0xd800..0xdc00).contains(&first) {
                                // a surrogate pair is one character
                                if self.text[self.at..].starts_with("\\u") {
                                    self.at += 2;
                                    let second = self.hex4()?;
                                    if !(0xdc00..0xe000).contains(&second) {
                                        return self.fail("a surrogate pair was expected");
                                    }
                                    char::from_u32(0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00))
                                } else {
                                    return self.fail("a surrogate pair was expected");
                                }
                            } else {
                                char::from_u32(first)
                            };
                            match ch {
                                Some(ch) => out.push(ch),
                                None => return self.fail("not a character"),
                            }
                        }
                        _ => return self.fail("an unknown escape"),
                    }
                }
                c if (c as u32) < 0x20 => return self.fail("a control character in a string"),
                c => {
                    out.push(c);
                    self.at += c.len_utf8();
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.text.get(self.at..self.at + 4).filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()));
        let Some(digits) = digits else { return self.fail("four hexadecimal digits were expected") };
        self.at += 4;
        Ok(u32::from_str_radix(digits, 16).unwrap_or(0))
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_space();
            items.push(self.value(depth + 1)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return self.fail("a comma or the end of the array was expected"),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.at += 1;
        let mut members = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return self.fail("a key was expected");
            }
            let key = self.string()?;
            self.skip_space();
            if self.peek() != Some(b':') {
                return self.fail("a colon was expected");
            }
            self.at += 1;
            self.skip_space();
            let node = self.value(depth + 1)?;
            members.push(Member { key, node });
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(members));
                }
                _ => return self.fail("a comma or the end of the object was expected"),
            }
        }
    }
}

impl Node {
    pub fn members(&self) -> Option<&[Member]> {
        match &self.value {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[Node]> {
        match &self.value {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match &self.value {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn member(&self, key: &str) -> Option<&Node> {
        self.members()?.iter().find(|m| m.key == key).map(|m| &m.node)
    }
}

/// A change to the text: the bytes `from..to` become `with`.
#[derive(Debug, Clone, PartialEq)]
pub struct Splice {
    pub from: usize,
    pub to: usize,
    pub with: String,
}

pub fn apply(text: &str, splice: &Splice) -> String {
    let mut out = String::with_capacity(text.len() + splice.with.len());
    out.push_str(&text[..splice.from]);
    out.push_str(&splice.with);
    out.push_str(&text[splice.to..]);
    out
}

/// The white space the line that holds byte `at` starts with.
pub fn indent_at(text: &str, at: usize) -> String {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// The text a person's file already uses for one more level of nesting: a tab when the file indents
/// with tabs, else two spaces.
fn step_of(text: &str) -> &'static str {
    if text.lines().any(|l| l.starts_with('\t')) { "\t" } else { "  " }
}

/// The splice that removes element `index` of the array or the member `index` of the object that
/// holds `spans` (the byte ranges of its elements), with the comma that belongs to it.
pub fn remove_element(spans: &[(usize, usize)], index: usize) -> Splice {
    let (start, end) = spans[index];
    if let Some(&(next, _)) = spans.get(index + 1) {
        // everything up to the next element: this one, its comma, and the space before the next
        Splice { from: start, to: next, with: String::new() }
    } else if index > 0 {
        // the last of several: the comma before it goes too
        Splice { from: spans[index - 1].1, to: end, with: String::new() }
    } else {
        Splice { from: start, to: end, with: String::new() }
    }
}

/// The splice that adds `item` (already written out, its first line not indented) as the last element
/// of the array or the last member of the object `container` (given its byte range and the byte
/// ranges of what it holds now).
pub fn append_element(text: &str, container: (usize, usize), spans: &[(usize, usize)], item: &str) -> Splice {
    let base = indent_at(text, container.0);
    let step = step_of(text);
    match spans.last() {
        Some(&(first_start, _)) if spans.len() > 1 || true => {
            let (_, last_end) = *spans.last().unwrap();
            let _ = first_start;
            let multiline = text[container.0..spans[0].0].contains('\n');
            if multiline {
                let child = indent_at(text, spans[0].0);
                let child = if child.is_empty() { format!("{base}{step}") } else { child };
                Splice { from: last_end, to: last_end, with: format!(",\n{child}{}", reindent(item, &child)) }
            } else {
                Splice { from: last_end, to: last_end, with: format!(", {}", compact(item)) }
            }
        }
        _ => {
            // nothing in it yet: the inside is replaced by the item on a line of its own
            let child = format!("{base}{step}");
            Splice { from: container.0 + 1, to: container.1 - 1, with: format!("\n{child}{}\n{base}", reindent(item, &child)) }
        }
    }
}

/// `item` written on several lines relative to column 0, moved to `indent`.
fn reindent(item: &str, indent: &str) -> String {
    let mut lines = item.lines();
    let mut out = lines.next().unwrap_or("").to_string();
    for line in lines {
        out.push('\n');
        if !line.is_empty() {
            out.push_str(indent);
        }
        out.push_str(line);
    }
    out
}

/// `item` on one line, for an array that is written on one line.
fn compact(item: &str) -> String {
    item.lines().map(str::trim).collect::<Vec<_>>().join(" ")
}

pub fn spans_of_items(items: &[Node]) -> Vec<(usize, usize)> {
    items.iter().map(|n| (n.start, n.end)).collect()
}

/// The start of a member is where its key begins, found going back from its value: the key, a
/// colon, and the white space between. A member's range is that start up to the end of its value.
pub fn spans_of_members(text: &str, members: &[Member]) -> Vec<(usize, usize)> {
    members
        .iter()
        .map(|m| {
            let before = &text[..m.node.start];
            let colon = before.trim_end().len().saturating_sub(1); // the colon
            let key_end = before[..colon].trim_end().len(); // just after the closing quote
            // back over the key's string to its opening quote
            let mut at = key_end.saturating_sub(1);
            let b = text.as_bytes();
            while at > 0 {
                at -= 1;
                if b[at] == b'"' {
                    // an escaped quote has an odd number of backslashes before it
                    let slashes = b[..at].iter().rev().take_while(|c| **c == b'\\').count();
                    if slashes % 2 == 0 {
                        break;
                    }
                }
            }
            (at, m.node.end)
        })
        .collect()
}

/// What `merge_hooks` is asked to make true.
pub struct Wanted<'a> {
    /// The events (`SessionStart`) and, for each, the group to put in its list: written out in the
    /// shape `{ "hooks": [ ... ] }`, the first line not indented.
    pub events: &'a [(&'a str, String)],
    /// Whether a group's text already is the one this is about (it names the script).
    pub is_ours: &'a dyn Fn(&str) -> bool,
    /// Whether one command is a hook of imux's own from before (to be taken out).
    pub is_old: &'a dyn Fn(&str) -> bool,
}

/// What `merge_hooks` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub text: String,
    /// The events a group was added to.
    pub added: Vec<String>,
    /// How many hooks of imux's own from before were taken out.
    pub removed: usize,
}

/// Make `text` (a Claude Code `settings.json`) call the hook for each wanted event: add the group
/// where it is not, take out the old inline hooks of imux's own, and leave everything else as it was.
/// An error says why the file could not be changed, and nothing is changed then.
pub fn merge_hooks(text: &str, wanted: &Wanted) -> Result<Merged, String> {
    let mut text = text.to_string();
    let mut merged = Merged::default();
    parse_root(&text)?;
    // 1. the old hooks of imux's own, one at a time (each change is read back before the next)
    for _ in 0..200 {
        let root = parse_root(&text)?;
        let Some((splice, hooks)) = next_old_hook(&text, &root, wanted) else { break };
        text = apply(&text, &splice);
        merged.removed += hooks;
    }
    // 2. the group for each event
    for (event, group) in wanted.events {
        let root = parse_root(&text)?;
        let hooks = root.member("hooks");
        let splice = match hooks {
            None => {
                let step = step_of(&text);
                let object = format!("{{\n{step}{}\n}}", reindent(&member_text(event, group, step), step));
                let (from, to) = (root.start, root.end);
                let members = root.members().unwrap_or(&[]);
                let spans = spans_of_members(&text, members);
                Some(append_element(&text, (from, to), &spans, &format!("\"hooks\": {object}")))
            }
            Some(h) => {
                let Some(members) = h.members() else { return Err("\"hooks\" is not an object".to_string()) };
                match members.iter().find(|m| m.key == *event) {
                    None => {
                        let spans = spans_of_members(&text, members);
                        Some(append_element(&text, (h.start, h.end), &spans, &member_text(event, group, step_of(&text))))
                    }
                    Some(m) => {
                        let Some(items) = m.node.items() else { return Err(format!("\"{event}\" is not a list")) };
                        if items.iter().any(|i| (wanted.is_ours)(&text[i.start..i.end])) {
                            None
                        } else {
                            Some(append_element(&text, (m.node.start, m.node.end), &spans_of_items(items), group))
                        }
                    }
                }
            }
        };
        if let Some(splice) = splice {
            text = apply(&text, &splice);
            merged.added.push((*event).to_string());
        }
    }
    // read the result back before anyone writes it
    let again = parse_root(&text).map_err(|e| format!("the result would not read back ({e})"))?;
    for (event, _) in wanted.events {
        let has = again.member("hooks").and_then(|h| h.member(event)).and_then(Node::items).is_some_and(|items| items.iter().any(|i| (wanted.is_ours)(&text[i.start..i.end])));
        if !has {
            return Err(format!("the result would not hold the hook for {event}"));
        }
    }
    merged.text = text;
    Ok(merged)
}

fn parse_root(text: &str) -> Result<Node, String> {
    let root = parse(text)?;
    if root.members().is_none() {
        return Err("the file is not a JSON object".to_string());
    }
    Ok(root)
}

/// `"event": [ group ]` as a member of an object that is written over several lines.
fn member_text(event: &str, group: &str, step: &str) -> String {
    format!("\"{event}\": [\n{step}{}\n]", reindent(group, step))
}

/// The splice that takes out the first hook of imux's own from before in the wanted events, if any, and
/// how many hooks that is: the whole group when that is all it holds, else just the hook.
fn next_old_hook(text: &str, root: &Node, wanted: &Wanted) -> Option<(Splice, usize)> {
    let hooks = root.member("hooks")?;
    for (event, _) in wanted.events {
        let Some(items) = hooks.member(event).and_then(Node::items) else { continue };
        for (gi, group) in items.iter().enumerate() {
            if (wanted.is_ours)(&text[group.start..group.end]) {
                continue;
            }
            let Some(commands) = group.member("hooks").and_then(Node::items) else { continue };
            let old: Vec<usize> = commands
                .iter()
                .enumerate()
                .filter(|(_, c)| c.member("command").and_then(Node::str).is_some_and(|s| (wanted.is_old)(s)))
                .map(|(i, _)| i)
                .collect();
            let Some(&first) = old.first() else { continue };
            return Some(if old.len() == commands.len() {
                (remove_element(&spans_of_items(items), gi), old.len())
            } else {
                (remove_element(&spans_of_items(commands), first), 1)
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::merge_claude_settings;

    const REF: &str = "~/.claude/imux-claude-hook.sh";

    fn merged(text: &str) -> Merged {
        merge_claude_settings(text, REF).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    /// A text that holds the hook for both events once, and reads back.
    fn assert_has_hook(text: &str) {
        let root = parse(text).unwrap_or_else(|e| panic!("does not read back: {e}\n{text}"));
        for event in ["SessionStart", "UserPromptSubmit"] {
            let items = root.member("hooks").and_then(|h| h.member(event)).and_then(Node::items).unwrap_or_else(|| panic!("no {event} list in\n{text}"));
            let ours = items.iter().filter(|i| text[i.start..i.end].contains("imux-claude-hook.sh")).count();
            assert_eq!(ours, 1, "{event} holds the hook {ours} times in\n{text}");
        }
    }

    #[test]
    fn the_reader_takes_json_and_says_where_every_value_is() {
        let text = " {\"a\": [1, -2.5e+3, true, null, \"x\\ny\\u00e9\\ud83d\\ude00\"], \"b\": {\"c\": {}}} ";
        let root = parse(text).unwrap();
        assert_eq!((root.start, root.end), (1, text.len() - 1));
        let a = root.member("a").unwrap();
        assert_eq!(&text[a.start..a.end], "[1, -2.5e+3, true, null, \"x\\ny\\u00e9\\ud83d\\ude00\"]");
        assert_eq!(a.items().unwrap()[4].str(), Some("x\ny\u{e9}\u{1f600}"));
        assert!(root.member("b").and_then(|b| b.member("c")).is_some());
        assert!(parse("\u{feff}{}").is_ok(), "a byte order mark is allowed");
    }

    #[test]
    fn the_reader_refuses_what_is_not_strict_json_and_says_why() {
        for bad in [
            "", "{", "{\"a\":}", "{\"a\" 1}", "{\"a\":1,}", "[1,]", "[1 2]", "{'a':1}", "// c\n{}", "{} {}", "{\"a\":01}", "{\"a\":1.}", "{\"a\":\"\\q\"}",
            "{\"a\":\"\\ud800\"}", "{\"a\":\"line\nbreak\"}", "nul", "-", "[", "\"open", "{\"a\":tru}",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should not read");
        }
        assert!(parse(&"[".repeat(MAX_DEPTH + 2)).is_err(), "too deep");
        assert!(parse(&format!("[{}]", "0,".repeat(10) + "0")).is_ok());
        assert!(parse(&" ".repeat(MAX_BYTES + 1)).is_err(), "too large");
    }

    #[test]
    fn a_file_with_no_hooks_gets_them_and_keeps_every_other_setting_byte_for_byte() {
        let text = "{\n  \"permissions\": { \"allow\": [\"Bash(ls)\"] },\n  \"env\": {\"A\": \"1\"}\n}\n";
        let m = merged(text);
        assert_has_hook(&m.text);
        assert_eq!(m.added, ["SessionStart", "UserPromptSubmit"]);
        assert!(m.text.contains("\"permissions\": { \"allow\": [\"Bash(ls)\"] },\n  \"env\": {\"A\": \"1\"}"), "{}", m.text);
        assert!(m.text.starts_with("{\n  \"permissions\""));
        assert_eq!(merged("{}").added.len(), 2);
    }

    #[test]
    fn hooks_of_the_person_are_kept_and_the_group_comes_after_them() {
        let mine = "{ \"matcher\": \"Bash\", \"hooks\": [{ \"type\": \"command\", \"command\": \"echo mine\" }] }";
        let text = format!("{{\n  \"hooks\": {{\n    \"PreToolUse\": [\n      {mine}\n    ],\n    \"SessionStart\": [\n      {mine}\n    ]\n  }}\n}}\n");
        let m = merged(&text);
        assert_has_hook(&m.text);
        assert_eq!(m.text.matches(mine).count(), 2, "both of the person's groups are as they were");
        assert!(m.text.find(mine).unwrap() < m.text.find("imux-claude-hook.sh").unwrap());
        assert_eq!(m.removed, 0);
    }

    #[test]
    fn a_second_run_changes_nothing() {
        for text in ["{}", "{\"hooks\":{}}", "{\n\t\"hooks\": {\n\t\t\"SessionStart\": []\n\t}\n}\n", "{\"a\":1}"] {
            let once = merged(text);
            let twice = merged(&once.text);
            assert_eq!(twice.text, once.text, "{text}");
            assert!(twice.added.is_empty() && twice.removed == 0);
        }
    }

    #[test]
    fn the_old_hooks_of_imux_are_taken_out_and_nothing_else() {
        let old_name = "{ \"type\": \"command\", \"command\": \"imux agent name \\\"${IMUX_PANE_ID#%}\\\" x\" }";
        let old_report = "{ \"type\": \"command\", \"command\": \"imux pane report-agent --source claude-hook --agent claude-code --state idle -- claude --resume x\" }";
        let other = "{ \"type\": \"command\", \"command\": \"echo keep\" }";
        // a group of only old hooks goes whole; one that also holds another hook loses only the old one
        let text = format!(
            "{{\n  \"hooks\": {{\n    \"SessionStart\": [\n      {{ \"hooks\": [{old_name}, {old_report}] }},\n      {{ \"hooks\": [{other}, {old_name}] }}\n    ],\n    \"UserPromptSubmit\": [\n      {{ \"hooks\": [{old_name}] }}\n    ]\n  }}\n}}\n"
        );
        let m = merged(&text);
        assert_has_hook(&m.text);
        assert_eq!(m.removed, 4);
        assert!(!m.text.contains("imux agent name") && !m.text.contains("claude-hook ") && !m.text.contains("report-agent"), "{}", m.text);
        assert!(m.text.contains("echo keep"), "{}", m.text);
        // also the script of the first instructions
        let m = merged("{\"hooks\":{\"SessionStart\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"sh ~/.claude/imux-agent-name.sh\"}]}]}}");
        assert_eq!(m.removed, 1);
        assert_has_hook(&m.text);
        assert!(!m.text.contains("imux-agent-name.sh"));
    }

    #[test]
    fn a_hook_that_only_looks_like_one_of_imux_is_not_taken_out() {
        let text = "{\"hooks\":{\"SessionStart\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo imux is nice\"},{\"type\":\"command\",\"command\":\"my-imux-notes\"}]}]}}";
        let m = merged(text);
        assert_eq!(m.removed, 0);
        assert!(m.text.contains("echo imux is nice") && m.text.contains("my-imux-notes"));
    }

    #[test]
    fn spacing_styles_are_followed_or_at_least_valid() {
        // tabs
        let m = merged("{\n\t\"a\": 1\n}\n");
        assert_has_hook(&m.text);
        assert!(m.text.contains("\n\t\"hooks\": {\n\t\t\"SessionStart\""), "{}", m.text);
        // one line
        assert_has_hook(&merged("{\"a\":1,\"hooks\":{\"SessionStart\":[{\"hooks\":[]}]}}").text);
        // windows line endings and a byte order mark
        assert_has_hook(&merged("\u{feff}{\r\n  \"a\": 1\r\n}\r\n").text);
        // empty containers
        assert_has_hook(&merged("{\"hooks\": {\"SessionStart\": [], \"UserPromptSubmit\": []}}").text);
        // a key with an escaped quote in it before the member we add after
        assert_has_hook(&merged("{\"we\\\"ird\": {\"x\": [1,2]}, \"hooks\": {\"Stop\": []}}").text);
    }

    #[test]
    fn a_file_that_cannot_be_changed_safely_is_refused_with_a_reason_and_left_as_it_is() {
        for (text, why) in [
            ("[]", "not a JSON object"),
            ("{\"hooks\": []}", "\"hooks\" is not an object"),
            ("{\"hooks\": {\"SessionStart\": {}}}", "is not a list"),
            ("{\"a\": 1,}", "byte"),
            ("// comment\n{}", "byte"),
            ("{\"hooks\": ", "byte"),
        ] {
            let e = merge_claude_settings(text, REF).unwrap_err();
            assert!(e.contains(why), "{text:?}: {e}");
        }
    }

    #[test]
    fn whatever_is_in_the_file_the_result_reads_back_with_the_same_other_members() {
        let samples = [
            "{}",
            "{\n}",
            "{\"x\": [1, {\"y\": null}], \"hooks\": {\"A\": [{\"hooks\": []}], \"SessionStart\": [1, 2]}}",
            "{\n  \"hooks\": {\n    \"UserPromptSubmit\": [\n      {\"hooks\": [{\"command\": \"imux agent name 1 x\"}]}\n    ]\n  },\n  \"model\": \"sonnet\"\n}\n",
            "{\"a\":\"b\",\"hooks\":{\"SessionStart\":[{\"hooks\":[{\"command\":\"imux agent name 1\"},{\"command\":\"imux agent name 2\"},{\"command\":\"imux agent name 3\"}]}]},\"c\":[true]}",
        ];
        for text in samples {
            let before = parse(text).unwrap();
            let m = merged(text);
            assert_has_hook(&m.text);
            let after = parse(&m.text).unwrap();
            for member in before.members().unwrap().iter().filter(|m| m.key != "hooks") {
                let again = after.member(&member.key).unwrap_or_else(|| panic!("{} is gone from\n{}", member.key, m.text));
                assert_eq!(&text[member.node.start..member.node.end], &m.text[again.start..again.end], "{}", member.key);
            }
        }
    }
}
