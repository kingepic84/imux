//! Paste buffers: the stack that copy mode fills and `paste-buffer` empties, plus named
//! buffers made with `set-buffer`. Pure data; the server decides when to use it.

use std::collections::VecDeque;

/// Largest text one buffer holds. Copies from copy mode are cut here, `set-buffer` and
/// `load-buffer` refuse more.
pub const MAX_BYTES: usize = 8 << 20;
/// Named buffers kept at most (the automatic ones are limited by `buffer-limit`).
const MAX_NAMED: usize = 100;
const MAX_NAME: usize = 64;
/// How much of a buffer `list-buffers` shows.
const SAMPLE: usize = 50;

pub struct Buffer {
    pub name: String,
    pub text: String,
    automatic: bool,
}

#[derive(Default)]
pub struct Buffers {
    /// Newest first.
    list: VecDeque<Buffer>,
    serial: u64,
}

fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(format!("bad buffer name {name:?}"));
    }
    Ok(())
}

/// `text` cut to at most `MAX_BYTES`, at a character boundary.
fn clip(mut text: String) -> String {
    if text.len() > MAX_BYTES {
        let mut at = MAX_BYTES;
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        text.truncate(at);
    }
    text
}

impl Buffers {
    /// A copy: becomes the newest buffer, named `buffer0`, `buffer1`, ... The oldest automatic
    /// buffers go once there are more than `limit`. Returns the name.
    pub fn push(&mut self, text: String, limit: usize) -> String {
        let name = format!("buffer{}", self.serial);
        self.serial += 1;
        self.list.push_front(Buffer { name: name.clone(), text: clip(text), automatic: true });
        let mut seen = 0;
        self.list.retain(|b| {
            seen += usize::from(b.automatic);
            !b.automatic || seen <= limit.max(1)
        });
        name
    }

    /// `set-buffer -b name [-a] text`: a named buffer, replaced or (with `append`) extended.
    pub fn set(&mut self, name: &str, text: &str, append: bool) -> Result<(), String> {
        valid_name(name)?;
        let at = self.list.iter().position(|b| b.name == name);
        if at.is_none() && self.list.iter().filter(|b| !b.automatic).count() >= MAX_NAMED {
            return Err(format!("no more than {MAX_NAMED} named buffers"));
        }
        let kept = at.filter(|_| append).map_or(0, |i| self.list[i].text.len());
        if kept + text.len() > MAX_BYTES {
            return Err(format!("a buffer holds at most {MAX_BYTES} bytes"));
        }
        let mut full = at.and_then(|i| self.list.remove(i)).filter(|_| append).map_or_else(String::new, |b| b.text);
        full.push_str(text);
        self.list.push_front(Buffer { name: name.to_string(), text: full, automatic: false });
        Ok(())
    }

    /// The buffer called `name`, or the newest one.
    pub fn get(&self, name: Option<&str>) -> Option<&Buffer> {
        match name {
            Some(n) => self.list.iter().find(|b| b.name == n),
            None => self.list.front(),
        }
    }

    /// The `index`th buffer, newest first.
    pub fn at(&self, index: usize) -> Option<&Buffer> {
        self.list.get(index)
    }

    /// Delete the buffer called `name`, or the newest one; false when there is none.
    pub fn remove(&mut self, name: Option<&str>) -> bool {
        let at = match name {
            Some(n) => self.list.iter().position(|b| b.name == n),
            None => (!self.list.is_empty()).then_some(0),
        };
        at.and_then(|i| self.list.remove(i)).is_some()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Buffer> {
        self.list.iter()
    }
}

impl Buffer {
    /// `buffer0: 12 bytes: "hello world"`
    pub fn summary(&self) -> String {
        let mut sample: String = self.text.chars().take(SAMPLE).map(|c| if c.is_control() { '.' } else { c }).collect();
        if self.text.chars().count() > SAMPLE {
            sample.push_str("...");
        }
        format!("{}: {} bytes: \"{sample}\"", self.name, self.text.len())
    }
}

/// The bytes typed into a pane for a paste. Nothing that could end a bracketed paste or act as a
/// control sequence gets through: ESC, the other control characters except tab, line feed and
/// carriage return, DEL and the C1 range are dropped. A line break becomes `sep` (a carriage
/// return, as when typing Enter) unless `raw` keeps the line feeds.
pub fn paste_bytes(text: &str, sep: &str, raw: bool) -> Vec<u8> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' if !raw => out.push_str(sep),
            '\n' | '\r' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(b: &Buffers) -> Vec<&str> {
        b.iter().map(|b| b.name.as_str()).collect()
    }

    #[test]
    fn copies_stack_up_newest_first() {
        let mut b = Buffers::default();
        assert_eq!(b.push("one".into(), 50), "buffer0");
        assert_eq!(b.push("two".into(), 50), "buffer1");
        assert_eq!(names(&b), ["buffer1", "buffer0"]);
        assert_eq!(b.get(None).unwrap().text, "two");
        assert_eq!(b.get(Some("buffer0")).unwrap().text, "one");
        assert_eq!(b.at(1).unwrap().name, "buffer0");
        assert!(b.get(Some("nope")).is_none());
    }

    #[test]
    fn the_oldest_automatic_buffers_go_over_the_limit() {
        let mut b = Buffers::default();
        b.set("keep", "x", false).unwrap();
        for i in 0..5 {
            b.push(format!("t{i}"), 3);
        }
        assert_eq!(names(&b), ["buffer4", "buffer3", "buffer2", "keep"]);
        let mut b = Buffers::default();
        b.push("a".into(), 0);
        b.push("b".into(), 0);
        assert_eq!(b.len(), 1, "a limit of 0 still keeps the latest copy");
    }

    #[test]
    fn set_replaces_or_appends_and_checks_names() {
        let mut b = Buffers::default();
        b.set("n", "ab", false).unwrap();
        b.set("n", "cd", true).unwrap();
        assert_eq!(b.get(Some("n")).unwrap().text, "abcd");
        b.set("n", "z", false).unwrap();
        assert_eq!(b.get(Some("n")).unwrap().text, "z");
        assert_eq!(b.len(), 1);
        b.set("fresh", "q", true).unwrap();
        assert_eq!(b.get(Some("fresh")).unwrap().text, "q");
        assert_eq!(b.get(None).unwrap().name, "fresh", "a set buffer is the newest");
        for bad in ["", "a\nb", &"x".repeat(65)] {
            assert!(b.set(bad, "x", false).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn named_buffers_and_sizes_are_limited() {
        let mut b = Buffers::default();
        for i in 0..MAX_NAMED {
            b.set(&format!("n{i}"), "x", false).unwrap();
        }
        assert!(b.set("one-too-many", "x", false).is_err());
        assert!(b.set("n0", "y", false).is_ok(), "replacing an existing one is fine");
        let mut b = Buffers::default();
        assert!(b.set("big", &"x".repeat(MAX_BYTES + 1), false).is_err());
        b.set("big", &"x".repeat(MAX_BYTES), false).unwrap();
        assert!(b.set("big", "y", true).is_err());
        assert_eq!(b.get(Some("big")).unwrap().text.len(), MAX_BYTES, "a refused append leaves the buffer as it was");
    }

    #[test]
    fn a_copy_bigger_than_the_limit_is_cut_at_a_character() {
        let mut b = Buffers::default();
        let text = "é".repeat(MAX_BYTES / 2 + 10);
        b.push(text, 50);
        let kept = &b.get(None).unwrap().text;
        assert!(kept.len() <= MAX_BYTES && kept.chars().all(|c| c == 'é'));
    }

    #[test]
    fn remove_takes_the_newest_or_the_named_one() {
        let mut b = Buffers::default();
        assert!(!b.remove(None));
        b.push("a".into(), 50);
        b.push("b".into(), 50);
        b.set("n", "c", false).unwrap();
        assert!(b.remove(Some("buffer0")));
        assert!(!b.remove(Some("buffer0")));
        assert!(b.remove(None));
        assert_eq!(names(&b), ["buffer1"]);
    }

    #[test]
    fn the_summary_shows_name_size_and_a_clean_sample() {
        let mut b = Buffers::default();
        b.push("hello\nworld".into(), 50);
        assert_eq!(b.get(None).unwrap().summary(), "buffer0: 11 bytes: \"hello.world\"");
        b.push("y".repeat(80), 50);
        assert!(b.get(None).unwrap().summary().ends_with(&format!("\"{}...\"", "y".repeat(50))));
    }

    #[test]
    fn paste_turns_line_breaks_into_returns_and_keeps_text() {
        assert_eq!(paste_bytes("a\nb\tc", "\r", false), b"a\rb\tc");
        assert_eq!(paste_bytes("a\r\nb", "\r", false), b"a\rb");
        assert_eq!(paste_bytes("a\nb", "\r", true), b"a\nb");
        assert_eq!(paste_bytes("a\nb", ";", false), b"a;b");
        assert_eq!(paste_bytes("héllo ✓", "\r", false), "héllo ✓".as_bytes());
    }

    #[test]
    fn paste_drops_escapes_and_other_controls() {
        assert_eq!(paste_bytes("x\x1b[201~rm -rf /\x1b[200~", "\r", false), b"x[201~rm -rf /[200~");
        assert_eq!(paste_bytes("a\x00b\x07c\x7fd\u{9b}31m\u{9d}e", "\r", false), b"abcd31me");
        for c in (0u32..0x20).chain(0x7f..0xa0).filter_map(char::from_u32) {
            let out = paste_bytes(&c.to_string(), "\r", false);
            assert!(out.iter().all(|b| matches!(b, b'\t' | b'\n' | b'\r')), "{c:?}");
        }
    }
}
