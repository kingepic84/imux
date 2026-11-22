//! The snapshot file format: line oriented text, one record per line, tokens split by single
//! spaces. Strings are percent-encoded (the empty string is `~`), so names, paths and arguments
//! may hold anything. Counts in the header lines let the reader notice a file cut short; the last
//! line is `end`. Decoding never panics: whatever is wrong comes back as a `FormatError`.
//!
//! ```text
//! imux-snapshot 1
//! saved 1767225600
//! recent 0
//! sessions 1
//! session main 0 1
//! window editor 1 0 0
//! split side 0.5
//! pane /Users/me/src 0 2 vim main.rs
//! pane /Users/me/src 1 0
//! end
//! ```

use std::fmt;

use super::{SPane, SSession, STree, SWindow, Snapshot};
use crate::session::layout::{Shape, Split};

pub const VERSION: u32 = 1;

const MAX_BYTES: usize = 8 << 20;
const MAX_SESSIONS: usize = 256;
const MAX_WINDOWS: usize = 256;
const MAX_PANES: usize = 256;
const MAX_ALL_PANES: usize = 1024;
const MAX_DEPTH: usize = 64;
const MAX_ARGC: usize = 1024;
/// Programs a pane's program may run inside, as far as a snapshot goes.
const MAX_OUTER: usize = 4;
const MAX_META: usize = 64;

#[derive(Debug, PartialEq)]
pub struct FormatError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

pub fn encode(snap: &Snapshot) -> String {
    let mut out = String::new();
    out.push_str(&format!("imux-snapshot {VERSION}\n"));
    out.push_str(&format!("saved {}\n", snap.saved_at));
    out.push_str(&format!("recent {}\n", snap.recent));
    out.push_str(&format!("sessions {}\n", snap.sessions.len()));
    for s in &snap.sessions {
        out.push_str(&format!("session {} {} {}\n", esc(&s.name), s.current, s.windows.len()));
        for w in &s.windows {
            out.push_str(&format!(
                "window {} {} {} {}\n",
                esc(&w.name),
                u8::from(w.auto_rename),
                w.active_leaf,
                w.meta.len()
            ));
            for (k, v) in &w.meta {
                out.push_str(&format!("meta {} {}\n", esc(k), esc(v)));
            }
            tree(&w.tree, &mut out);
        }
    }
    out.push_str("end\n");
    out
}

fn tree(t: &STree, out: &mut String) {
    match t {
        Shape::Leaf(p) => {
            out.push_str(&format!("pane {} {} {}", esc(&p.cwd), u8::from(p.claude), p.argv.len()));
            for a in &p.argv {
                out.push(' ');
                out.push_str(&esc(a));
            }
            if let Some(resume) = &p.resume {
                out.push_str(&format!(" +resume {}", resume.len()));
                for a in resume {
                    out.push(' ');
                    out.push_str(&esc(a));
                }
            }
            if !p.outer.is_empty() {
                out.push_str(&format!(" +outer {}", p.outer.len()));
                for level in &p.outer {
                    out.push_str(&format!(" {}", level.len()));
                    for a in level {
                        out.push(' ');
                        out.push_str(&esc(a));
                    }
                }
            }
            if let Some(name) = &p.name {
                out.push_str(" +name ");
                out.push_str(&esc(name));
            }
            out.push('\n');
        }
        Shape::Branch { kind, ratio, a, b } => {
            let side = match kind {
                Split::SideBySide => "side",
                Split::Stacked => "stack",
            };
            out.push_str(&format!("split {side} {ratio}\n"));
            tree(a, out);
            tree(b, out);
        }
    }
}

fn esc(s: &str) -> String {
    if s.is_empty() {
        return "~".to_string();
    }
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_./+=:@,-".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unesc(s: &str) -> Result<String, String> {
    if s == "~" {
        return Ok(String::new());
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3).ok_or("cut short escape")?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "bad escape".to_string())?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "not UTF-8".to_string())
}

struct Reader<'a> {
    lines: std::str::Lines<'a>,
    n: usize,
    /// Panes in the whole file: the limits per level multiply (256 x 256 x 256), and restoring
    /// forks one process for each.
    all_panes: usize,
}

impl<'a> Reader<'a> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, FormatError> {
        Err(FormatError { line: self.n, msg: msg.into() })
    }

    /// The next line split into tokens, which must start with `kind` and have at least `min`
    /// tokens after it. Extra tokens are left for the caller to ignore.
    fn record(&mut self, kind: &str, min: usize) -> Result<Vec<&'a str>, FormatError> {
        let Some(line) = self.lines.next() else {
            return self.err(format!("expected `{kind}`, file ends"));
        };
        self.n += 1;
        let mut toks = line.split(' ');
        if toks.next() != Some(kind) {
            return self.err(format!("expected `{kind}`"));
        }
        let rest: Vec<&str> = toks.collect();
        if rest.len() < min {
            return self.err(format!("`{kind}` has too few fields"));
        }
        Ok(rest)
    }

    fn num<T: std::str::FromStr>(&self, tok: &str) -> Result<T, FormatError> {
        match tok.parse() {
            Ok(v) => Ok(v),
            Err(_) => self.err(format!("not a number: {tok}")),
        }
    }

    fn text(&self, tok: &str) -> Result<String, FormatError> {
        match unesc(tok) {
            Ok(s) => Ok(s),
            Err(e) => self.err(e),
        }
    }

    fn flag(&self, tok: &str) -> Result<bool, FormatError> {
        match tok {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => self.err(format!("not 0 or 1: {tok}")),
        }
    }

    fn limited(&self, n: usize, max: usize, what: &str) -> Result<usize, FormatError> {
        if n > max {
            return self.err(format!("too many {what}: {n}"));
        }
        Ok(n)
    }
}

pub fn decode(text: &str) -> Result<Snapshot, FormatError> {
    if text.len() > MAX_BYTES {
        return Err(FormatError { line: 0, msg: "file too large".into() });
    }
    let mut r = Reader { lines: text.lines(), n: 0, all_panes: 0 };
    let head = r.record("imux-snapshot", 1)?;
    let version: u32 = r.num(head[0])?;
    if version != VERSION {
        return r.err(format!("unsupported version {version}"));
    }
    let saved_at = {
        let f = r.record("saved", 1)?;
        r.num(f[0])?
    };
    let recent: usize = {
        let f = r.record("recent", 1)?;
        r.num(f[0])?
    };
    let nsessions = {
        let f = r.record("sessions", 1)?;
        let n = r.num(f[0])?;
        r.limited(n, MAX_SESSIONS, "sessions")?
    };
    let mut sessions = Vec::with_capacity(nsessions);
    for _ in 0..nsessions {
        sessions.push(session(&mut r)?);
    }
    r.record("end", 0)?;
    if recent > 0 && recent >= sessions.len() {
        return r.err("recent session out of range");
    }
    Ok(Snapshot { saved_at, recent, sessions })
}

fn session(r: &mut Reader) -> Result<SSession, FormatError> {
    let f = r.record("session", 3)?;
    let name = r.text(f[0])?;
    let current: usize = r.num(f[1])?;
    let n: usize = r.num(f[2])?;
    let n = r.limited(n, MAX_WINDOWS, "windows")?;
    if n > 0 && current >= n {
        return r.err("current window out of range");
    }
    let mut windows = Vec::with_capacity(n);
    for _ in 0..n {
        windows.push(window(r)?);
    }
    Ok(SSession { name, current, windows })
}

fn window(r: &mut Reader) -> Result<SWindow, FormatError> {
    let f = r.record("window", 4)?;
    let name = r.text(f[0])?;
    let auto_rename = r.flag(f[1])?;
    let active_leaf: usize = r.num(f[2])?;
    let nmeta: usize = r.num(f[3])?;
    let nmeta = r.limited(nmeta, MAX_META, "meta lines")?;
    let mut meta = Vec::with_capacity(nmeta);
    for _ in 0..nmeta {
        let f = r.record("meta", 2)?;
        meta.push((r.text(f[0])?, r.text(f[1])?));
    }
    let mut panes = 0;
    let tree = node(r, 0, &mut panes)?;
    if active_leaf >= panes {
        return r.err("active pane out of range");
    }
    Ok(SWindow { name, auto_rename, active_leaf, tree, meta })
}

fn node(r: &mut Reader, depth: usize, panes: &mut usize) -> Result<STree, FormatError> {
    if depth > MAX_DEPTH {
        return r.err("layout nested too deeply");
    }
    let Some(line) = r.lines.clone().next() else {
        return r.err("expected a pane or split, file ends");
    };
    if line.starts_with("split ") {
        let f = r.record("split", 2)?;
        let kind = match f[0] {
            "side" => Split::SideBySide,
            "stack" => Split::Stacked,
            other => return r.err(format!("unknown split kind: {other}")),
        };
        let ratio: f32 = r.num(f[1])?;
        if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
            return r.err("split ratio out of range");
        }
        let a = Box::new(node(r, depth + 1, panes)?);
        let b = Box::new(node(r, depth + 1, panes)?);
        return Ok(Shape::Branch { kind, ratio, a, b });
    }
    let f = r.record("pane", 3)?;
    *panes += 1;
    r.limited(*panes, MAX_PANES, "panes in a window")?;
    r.all_panes += 1;
    r.limited(r.all_panes, MAX_ALL_PANES, "panes in the file")?;
    let cwd = r.text(f[0])?;
    let claude = r.flag(f[1])?;
    let argc: usize = r.num(f[2])?;
    let argc = r.limited(argc, MAX_ARGC, "arguments")?;
    if f.len() < 3 + argc {
        return r.err("pane has fewer arguments than it says");
    }
    let mut argv = Vec::with_capacity(argc);
    for tok in &f[3..3 + argc] {
        argv.push(r.text(tok)?);
    }
    let mut rest = &f[3 + argc..];
    let resume = match rest.first() {
        Some(&"+resume") => {
            let n: usize = r.num(rest.get(1).copied().unwrap_or("x"))?;
            let n = r.limited(n, MAX_ARGC, "resume arguments")?;
            if rest.len() < 2 + n {
                return r.err("pane has fewer resume arguments than it says");
            }
            let mut resume = Vec::with_capacity(n);
            for tok in &rest[2..2 + n] {
                resume.push(r.text(tok)?);
            }
            rest = &rest[2 + n..];
            Some(resume)
        }
        _ => None,
    };
    let mut outer = Vec::new();
    if rest.first() == Some(&"+outer") {
        let levels: usize = r.num(rest.get(1).copied().unwrap_or("x"))?;
        let levels = r.limited(levels, MAX_OUTER, "enclosing programs")?;
        let mut at = 2;
        for _ in 0..levels {
            let n: usize = r.num(rest.get(at).copied().unwrap_or("x"))?;
            let n = r.limited(n, MAX_ARGC, "enclosing program arguments")?;
            if rest.len() < at + 1 + n {
                return r.err("pane has fewer enclosing program arguments than it says");
            }
            let mut level = Vec::with_capacity(n);
            for tok in &rest[at + 1..at + 1 + n] {
                level.push(r.text(tok)?);
            }
            outer.push(level);
            at += 1 + n;
        }
        rest = &rest[at..];
    }
    let name = match rest {
        ["+name", token, ..] => Some(r.text(token)?).filter(|n| crate::session::agent::valid_name(n)),
        _ => None,
    };
    Ok(Shape::Leaf(SPane { cwd, argv, claude, resume, outer, name }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(cwd: &str, argv: &[&str], claude: bool) -> STree {
        Shape::Leaf(SPane { cwd: cwd.into(), argv: argv.iter().map(|s| s.to_string()).collect(), claude, resume: None, outer: Vec::new(), name: None })
    }

    fn sample() -> Snapshot {
        let tree = Shape::Branch {
            kind: Split::SideBySide,
            ratio: 0.37,
            a: Box::new(pane("/Users/me/my src", &["vim", "a b.rs", "100%", ""], false)),
            b: Box::new(Shape::Branch {
                kind: Split::Stacked,
                ratio: 0.5,
                a: Box::new(pane("/tmp", &[], true)),
                b: Box::new(pane("", &[], false)),
            }),
        };
        Snapshot {
            saved_at: 1_767_225_600,
            recent: 1,
            sessions: vec![
                SSession { name: "zero".into(), current: 0, windows: vec![] },
                SSession {
                    name: "main \u{e9}\n~".into(),
                    current: 1,
                    windows: vec![
                        SWindow {
                            name: "w one".into(),
                            auto_rename: true,
                            active_leaf: 2,
                            tree,
                            meta: vec![("worktree.repo".into(), "/r p".into()), ("k".into(), String::new())],
                        },
                        SWindow {
                            name: String::new(),
                            auto_rename: false,
                            active_leaf: 0,
                            tree: pane("/", &[], false),
                            meta: vec![],
                        },
                    ],
                },
            ],
        }
    }

    #[test]
    fn a_snapshot_survives_encode_and_decode_unchanged() {
        let s = sample();
        assert_eq!(decode(&encode(&s)), Ok(s));
    }

    #[test]
    fn strings_with_spaces_newlines_percent_and_tilde_round_trip() {
        for s in ["", "~", "a b", "100%", "line\nbreak", "\u{1F600}", "%41", "x~y"] {
            assert_eq!(unesc(&esc(s)).as_deref(), Ok(s), "{s:?}");
            assert!(!esc(s).contains([' ', '\n']));
        }
    }

    #[test]
    fn a_file_cut_short_anywhere_is_an_error_never_a_panic() {
        let text = encode(&sample());
        for cut in 0..text.len() - 1 {
            if !text.is_char_boundary(cut) {
                continue;
            }
            assert!(decode(&text[..cut]).is_err(), "cut at {cut} was accepted");
        }
    }

    #[test]
    fn an_unknown_version_is_refused() {
        let text = encode(&sample()).replacen("imux-snapshot 1", "imux-snapshot 2", 1);
        let err = decode(&text).unwrap_err();
        assert!(err.msg.contains("version"), "{err}");
    }

    #[test]
    fn unknown_meta_keys_and_extra_trailing_tokens_are_tolerated() {
        let text = encode(&sample())
            .replace("window w%20one 1 2 2", "window w%20one 1 2 2 future")
            .replace("meta k ~", "meta k ~ more");
        assert_eq!(decode(&text), Ok(sample()));
    }

    #[test]
    fn corrupt_content_is_refused() {
        let good = encode(&sample());
        let cases = [
            good.replace("split side 0.37", "split side 1.5"),
            good.replace("split side 0.37", "split side NaN"),
            good.replace("split side 0.37", "split diag 0.5"),
            good.replace("window w%20one 1 2 2", "window w%20one 1 9 2"),
            good.replace("session main", "session main%ZZ"),
            good.replace("session zero 0 0", "session zero 5 1"),
            good.replace("pane /tmp 1 0", "pane /tmp 7 0"),
            good.replace("vim a%20b.rs 100%25 ~", "vim a%20b.rs"),
            good.replace("sessions 2", "sessions 99999"),
            good.replace("recent 1", "recent 5"),
            good.replace("end\n", ""),
            good.replace("%C3%A9", "%C3"),
            String::new(),
            "garbage\n".into(),
        ];
        for (i, c) in cases.iter().enumerate() {
            assert!(decode(c).is_err(), "case {i} accepted:\n{c}");
        }
    }

    #[test]
    fn a_deeply_nested_layout_is_refused_without_overflowing_the_stack() {
        let mut text = String::from("imux-snapshot 1\nsaved 0\nrecent 0\nsessions 1\nsession s 0 1\nwindow w 0 0 0\n");
        for _ in 0..10_000 {
            text.push_str("split side 0.5\n");
        }
        assert!(decode(&text).is_err());
    }

    #[test]
    fn too_many_panes_in_one_window_are_refused() {
        let mut text = String::from("imux-snapshot 1\nsaved 0\nrecent 0\nsessions 1\nsession s 0 1\nwindow w 0 0 0\n");
        for _ in 0..(MAX_PANES + 1) {
            text.push_str("split side 0.5\npane / 0 0\n");
        }
        text.push_str("pane / 0 0\nend\n");
        assert!(decode(&text).is_err());
    }

    #[test]
    fn too_many_panes_in_the_whole_file_are_refused_though_each_window_is_within_its_limit() {
        let mut text = String::from("imux-snapshot 1\nsaved 0\nrecent 0\nsessions 1\nsession s 0 256\n");
        for _ in 0..256 {
            text.push_str("window w 0 0 0\n");
            for _ in 0..4 {
                text.push_str("split side 0.5\npane / 0 0\n");
            }
            text.push_str("pane / 0 0\n");
        }
        text.push_str("end\n");
        let err = decode(&text).unwrap_err();
        assert!(err.msg.contains("panes in the file"), "{err:?}");
    }

    fn with_resume(resume: Option<&[&str]>) -> Snapshot {
        let mut snap = sample();
        let Shape::Branch { a, .. } = &mut snap.sessions[1].windows[0].tree else { panic!("a split") };
        let Shape::Leaf(p) = &mut **a else { panic!("a leaf") };
        p.resume = resume.map(|r| r.iter().map(|s| s.to_string()).collect());
        snap
    }

    #[test]
    fn a_resume_command_round_trips_and_files_without_one_still_load() {
        let snap = with_resume(Some(&["claude", "--resume", "a b", ""]));
        assert_eq!(decode(&encode(&snap)).unwrap(), snap);
        let plain = with_resume(None);
        assert!(!encode(&plain).contains("+resume"));
        assert_eq!(decode(&encode(&plain)).unwrap(), plain);
    }

    #[test]
    fn a_resume_command_cut_short_or_with_a_wrong_count_is_refused() {
        let text = encode(&with_resume(Some(&["claude", "--resume", "x"])));
        assert!(decode(&text.replace("+resume 3", "+resume 9")).is_err());
        assert!(decode(&text.replace("+resume 3", "+resume x")).is_err());
        assert!(decode(&text.replace("+resume 3", "+resume 99999")).is_err());
        for cut in 0..text.len() - 1 {
            assert!(decode(&text[..cut]).is_err(), "cut at {cut}");
        }
    }

    fn named(name: &str, outer: &[&[&str]], resume: Option<&[&str]>) -> Snapshot {
        let mut snap = with_resume(resume);
        let Shape::Branch { a, .. } = &mut snap.sessions[1].windows[0].tree else { panic!("a split") };
        let Shape::Leaf(p) = &mut **a else { panic!("a leaf") };
        p.outer = outer.iter().map(|l| l.iter().map(|s| s.to_string()).collect()).collect();
        p.name = Some(name.to_string());
        snap
    }

    #[test]
    fn an_agent_name_round_trips_with_or_without_the_other_extras() {
        for snap in [named("rev", &[], None), named("rev", &[], Some(&["claude", "-c"])), named("rev", &[&["nix", "develop"]], None), named("rev", &[&["nix", "develop"]], Some(&["claude"]))] {
            let text = encode(&snap);
            assert!(text.contains(" +name rev"), "{text}");
            assert_eq!(decode(&text).unwrap(), snap);
        }
        assert!(!encode(&with_resume(None)).contains("+name"), "no name, no token");
    }

    #[test]
    fn a_name_that_could_not_be_given_is_dropped_when_a_file_is_read() {
        let text = encode(&named("rev", &[], None));
        let bad = decode(&text.replace("+name rev", "+name Bad:Name")).unwrap();
        let Shape::Branch { a, .. } = &bad.sessions[1].windows[0].tree else { panic!("a split") };
        let Shape::Leaf(p) = &**a else { panic!("a leaf") };
        assert_eq!(p.name, None);
    }

    fn with_outer(outer: &[&[&str]], resume: Option<&[&str]>) -> Snapshot {
        let mut snap = with_resume(resume);
        let Shape::Branch { a, .. } = &mut snap.sessions[1].windows[0].tree else { panic!("a split") };
        let Shape::Leaf(p) = &mut **a else { panic!("a leaf") };
        p.outer = outer.iter().map(|l| l.iter().map(|s| s.to_string()).collect()).collect();
        snap
    }

    #[test]
    fn enclosing_programs_round_trip_with_or_without_a_resume_command() {
        let snap = with_outer(&[&["nix", "develop", "a b"], &["bash"]], None);
        assert!(encode(&snap).contains("+outer 2 3 nix develop"), "{}", encode(&snap));
        assert_eq!(decode(&encode(&snap)).unwrap(), snap);
        let both = with_outer(&[&["bash"]], Some(&["claude", "--resume", "x"]));
        assert_eq!(decode(&encode(&both)).unwrap(), both);
        assert!(!encode(&with_outer(&[], None)).contains("+outer"));
    }

    #[test]
    fn enclosing_programs_cut_short_or_with_a_wrong_count_are_refused() {
        let text = encode(&with_outer(&[&["nix", "develop"], &["bash"]], None));
        assert!(decode(&text.replace("+outer 2 2", "+outer 9 2")).is_err());
        assert!(decode(&text.replace("+outer 2 2", "+outer 2 9")).is_err());
        assert!(decode(&text.replace("+outer 2", "+outer 99")).is_err());
        assert!(decode(&text.replace("+outer 2", "+outer x")).is_err());
        for cut in 0..text.len() - 1 {
            assert!(decode(&text[..cut]).is_err(), "cut at {cut}");
        }
    }
}
