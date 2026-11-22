//! Caps the length of OSC strings before they reach `vte`.
//!
//! `vte` built with `std` keeps the text of an OSC string in a `Vec` that has no limit, so a pane
//! that prints `ESC ] 0 ;` and then bytes forever makes the server grow until an allocation
//! fails, and that aborts every session (plan item H2). The guard follows just enough of the
//! parser's states to know when a string is open, counts its bytes, and past the limit ends the
//! string early with one CAN (the parser then runs the string as far as it got, which every
//! consumer already refuses or cuts: titles at 1 KiB, the clipboard above 1 MiB) and swallows the
//! rest of the string up to its terminator. Strings that fit pass through byte for byte.
//!
//! Only `ESC ]` opens an OSC string in the parser (it does not read the 8-bit introducer), and a
//! string ends at BEL, CAN, SUB or any ESC, so these are the only things tracked.

use std::borrow::Cow;

/// Longest OSC string handed to the parser, in bytes. The clipboard sequence (OSC 52) is the
/// largest legitimate user: `term::clip::MAX_DATA` of base64 plus its small prefix.
pub const MAX_OSC: usize = 2 * 1024 * 1024;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum State {
    #[default]
    Ground,
    /// Just after an ESC.
    Esc,
    /// Inside an OSC string.
    Osc,
    /// Inside an OSC string that went over the limit; its bytes are swallowed.
    Drop,
}

pub struct OscGuard {
    state: State,
    len: usize,
    limit: usize,
}

impl Default for OscGuard {
    fn default() -> OscGuard {
        OscGuard::with_limit(MAX_OSC)
    }
}

impl OscGuard {
    pub fn with_limit(limit: usize) -> OscGuard {
        OscGuard { state: State::Ground, len: 0, limit }
    }

    /// The bytes the parser should see for `bytes`. Borrowed (no copy) in the common case of
    /// text with no escape sequence in flight.
    pub fn pass<'a>(&mut self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        if self.state == State::Ground && !bytes.contains(&ESC) {
            return Cow::Borrowed(bytes);
        }
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            self.step(b, &mut out);
        }
        Cow::Owned(out)
    }

    fn step(&mut self, b: u8, out: &mut Vec<u8>) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
                out.push(b);
            }
            State::Esc => {
                self.state = match b {
                    b']' => {
                        self.len = 0;
                        State::Osc
                    }
                    ESC => State::Esc,
                    _ => State::Ground,
                };
                out.push(b);
            }
            State::Osc => match b {
                BEL | CAN | SUB => {
                    self.state = State::Ground;
                    out.push(b);
                }
                ESC => {
                    self.state = State::Esc;
                    out.push(b);
                }
                _ => {
                    self.len += 1;
                    if self.len > self.limit {
                        crate::log::warn(&format!("OSC string over {} bytes dropped", self.limit));
                        out.push(CAN);
                        self.state = State::Drop;
                    } else {
                        out.push(b);
                    }
                }
            },
            State::Drop => match b {
                BEL | CAN | SUB => self.state = State::Ground,
                ESC => {
                    self.state = State::Esc;
                    out.push(b);
                }
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(guard: &mut OscGuard, chunks: &[&[u8]]) -> Vec<u8> {
        chunks.iter().flat_map(|c| guard.pass(c).into_owned()).collect()
    }

    #[test]
    fn text_without_escape_is_not_copied() {
        let mut g = OscGuard::default();
        assert!(matches!(g.pass(b"plain text\r\n"), Cow::Borrowed(_)));
    }

    #[test]
    fn strings_that_fit_pass_unchanged() {
        let stream: &[u8] = b"a\x1b]0;title\x07b\x1b]52;c;Zm9v\x1b\\c\x1b[31md\x1b]8;;http://x\x07";
        let mut g = OscGuard::default();
        assert_eq!(run(&mut g, &[stream]), stream);
        let singles: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(run(&mut OscGuard::default(), &singles), stream);
    }

    #[test]
    fn a_string_over_the_limit_is_cut_with_a_can_and_its_rest_swallowed() {
        let mut g = OscGuard::with_limit(8);
        let out = run(&mut g, &[b"x\x1b]0;", &[b'a'; 100], b"\x07y"]);
        assert_eq!(out, b"x\x1b]0;aaaaaa\x18y");
        assert_eq!(g.state, State::Ground);
    }

    #[test]
    fn the_limit_counts_across_reads_and_the_result_does_not_depend_on_the_cut() {
        let mut stream = b"p\x1b]2;".to_vec();
        stream.extend([b'z'; 50]);
        stream.extend(b"\x1b\\q\x1b]0;ok\x07r");
        let whole = run(&mut OscGuard::with_limit(10), &[&stream]);
        assert_eq!(whole, b"p\x1b]2;zzzzzzzz\x18\x1b\\q\x1b]0;ok\x07r");
        for cut in 0..=stream.len() {
            assert_eq!(run(&mut OscGuard::with_limit(10), &[&stream[..cut], &stream[cut..]]), whole, "cut {cut}");
        }
        let singles: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(run(&mut OscGuard::with_limit(10), &singles), whole);
    }

    #[test]
    fn can_sub_and_esc_end_a_swallowed_string_too() {
        for end in [&[CAN][..], &[SUB][..], &[ESC, b'\\'][..]] {
            let mut g = OscGuard::with_limit(2);
            let mut input = b"\x1b]0;abcdef".to_vec();
            input.extend(end);
            input.extend(b"ok");
            let out = run(&mut g, &[&input]);
            assert!(out.ends_with(b"ok"), "{out:?}");
            assert_eq!(g.state, State::Ground);
        }
    }

    #[test]
    fn an_escape_in_a_string_starts_a_new_one_that_is_counted_from_zero() {
        let mut g = OscGuard::with_limit(4);
        let out = run(&mut g, &[b"\x1b]0;ab\x1b]0;cd\x07"]);
        assert_eq!(out, b"\x1b]0;ab\x1b]0;cd\x07");
    }

    #[test]
    fn a_csi_or_other_escape_does_not_open_a_string() {
        let mut g = OscGuard::with_limit(1);
        let stream: &[u8] = b"\x1b[1;2]long text long text\x1bP1$rdata\x1b\\\x1b_Gx\x1b\\";
        assert_eq!(run(&mut g, &[stream]), stream);
    }
}
