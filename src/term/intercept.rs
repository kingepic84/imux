//! Pulls inline-image sequences out of the byte stream before it reaches `vte`, which has no
//! size limit on OSC strings and so cannot be asked to refuse a hostile 2 GB payload.
//!
//! Two things are taken out: `OSC 1337 ; File=...` (iTerm2 images) and `DCS tmux ; ... ST`
//! (what `imgcat` wraps images in when `TERM` looks like tmux), whose body is returned with its
//! doubled ESCs undone. Everything else, including sequences cut by the end of a read, passes
//! through unchanged and in order.

use crate::log;

const OSC_PREFIX: &[u8] = b"\x1b]1337;File=";
const START_PREFIX: &[u8] = b"\x1b]1337;MultipartFile=";
const PART_PREFIX: &[u8] = b"\x1b]1337;FilePart=";
const END_PREFIX: &[u8] = b"\x1b]1337;FileEnd";
const DCS_PREFIX: &[u8] = b"\x1bPtmux;";
const APC_PREFIX: &[u8] = b"\x1b_G";
const STATUS_PREFIX: &[u8] = b"\x1b]7501;";

/// Largest image payload taken in, as sent (base64 for iTerm2).
pub const MAX_PAYLOAD: usize = 64 * 1024 * 1024;

/// Largest program status report body (OSC 7501); the protocol caps a whole sequence at 4096.
pub const MAX_STATUS: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Bytes for the terminal parser.
    Pass(&'a [u8]),
    /// Held-back bytes that turned out not to start an image sequence.
    Owned(Vec<u8>),
    /// What followed `File=` in an iTerm2 image sequence.
    File(Vec<u8>),
    /// The unwrapped body of a `DCS tmux ;` passthrough.
    Wrapped(Vec<u8>),
    /// What followed `ESC _ G` in a Kitty graphics command: `control;payload`.
    Kitty(Vec<u8>),
    /// What followed `MultipartFile=`: the arguments of an image sent in parts.
    MultipartStart(Vec<u8>),
    /// What followed `FilePart=`: the next piece of the base64 data.
    MultipartPart(Vec<u8>),
    /// `FileEnd`: the image sent in parts is complete.
    MultipartEnd,
    /// What followed `OSC 7501 ;`: a program status report or the `?` query.
    Status(Vec<u8>),
}

impl Piece<'_> {
    /// The same piece owning its bytes, for keeping it past the read that brought it.
    pub fn into_owned(self) -> Piece<'static> {
        match self {
            Piece::Pass(b) => Piece::Owned(b.to_vec()),
            Piece::Owned(b) => Piece::Owned(b),
            Piece::File(b) => Piece::File(b),
            Piece::Wrapped(b) => Piece::Wrapped(b),
            Piece::Kitty(b) => Piece::Kitty(b),
            Piece::MultipartStart(b) => Piece::MultipartStart(b),
            Piece::MultipartPart(b) => Piece::MultipartPart(b),
            Piece::MultipartEnd => Piece::MultipartEnd,
            Piece::Status(b) => Piece::Status(b),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Osc,
    Start,
    Part,
    End,
    Dcs,
    Apc,
    Status,
}

impl Kind {
    /// OSC-style strings, which BEL ends (APC and DCS end at ST only).
    fn is_osc(self) -> bool {
        !matches!(self, Kind::Dcs | Kind::Apc)
    }
}

const PREFIXES: [(Kind, &[u8]); 7] = [
    (Kind::Osc, OSC_PREFIX),
    (Kind::Start, START_PREFIX),
    (Kind::Part, PART_PREFIX),
    (Kind::End, END_PREFIX),
    (Kind::Dcs, DCS_PREFIX),
    (Kind::Apc, APC_PREFIX),
    (Kind::Status, STATUS_PREFIX),
];

struct Capture {
    kind: Kind,
    buf: Vec<u8>,
    esc: bool,
    overflow: bool,
}

pub struct Intercept {
    /// Start of a prefix cut by the end of the previous read.
    matching: Vec<u8>,
    capture: Option<Capture>,
    limit: usize,
}

impl Default for Intercept {
    fn default() -> Intercept {
        Intercept::with_limit(MAX_PAYLOAD)
    }
}

impl Intercept {
    pub fn with_limit(limit: usize) -> Intercept {
        Intercept { matching: Vec::new(), capture: None, limit }
    }

    pub fn split<'a>(&mut self, bytes: &'a [u8]) -> Vec<Piece<'a>> {
        let mut out = Vec::new();
        let mut i = 0;
        let mut run = 0;
        while i < bytes.len() {
            if self.capture.is_some() {
                i = self.capture_from(bytes, i, &mut out);
                run = i;
                continue;
            }
            if !self.matching.is_empty() {
                let held = std::mem::take(&mut self.matching);
                match self.continue_prefix(held, bytes, i) {
                    Ok(next) => {
                        i = next;
                        run = i;
                    }
                    Err(held) => out.push(Piece::Owned(held)),
                }
                continue;
            }
            let Some(j) = bytes[i..].iter().position(|&b| b == 0x1b).map(|p| p + i) else { break };
            let tail = &bytes[j..];
            let mut found = false;
            for (kind, prefix) in PREFIXES {
                if tail.starts_with(prefix) {
                    if run < j {
                        out.push(Piece::Pass(&bytes[run..j]));
                    }
                    self.capture = Some(Capture { kind, buf: Vec::new(), esc: false, overflow: false });
                    i = j + prefix.len();
                    run = i;
                    found = true;
                } else if prefix.starts_with(tail) {
                    if run < j {
                        out.push(Piece::Pass(&bytes[run..j]));
                    }
                    self.matching = tail.to_vec();
                    i = bytes.len();
                    run = i;
                    found = true;
                }
                if found {
                    break;
                }
            }
            if !found {
                i = j + 1;
            }
        }
        if run < bytes.len() {
            out.push(Piece::Pass(&bytes[run..]));
        }
        out
    }

    /// `held` is the start of a prefix from the previous read. Returns the index after the bytes
    /// of this read that it used, or gives `held` back when the stream turned out to be something
    /// else.
    fn continue_prefix(&mut self, held: Vec<u8>, bytes: &[u8], i: usize) -> Result<usize, Vec<u8>> {
        for (kind, prefix) in PREFIXES {
            if !prefix.starts_with(&held) {
                continue;
            }
            let rest = &prefix[held.len()..];
            let n = rest.len().min(bytes.len() - i);
            if bytes[i..i + n] != rest[..n] {
                continue;
            }
            if n == rest.len() {
                self.capture = Some(Capture { kind, buf: Vec::new(), esc: false, overflow: false });
            } else {
                self.matching = [&held[..], &bytes[i..i + n]].concat();
            }
            return Ok(i + n);
        }
        Err(held)
    }

    /// Consume bytes of the open capture from `i`; returns where it stopped.
    fn capture_from<'a>(&mut self, bytes: &[u8], i: usize, out: &mut Vec<Piece<'a>>) -> usize {
        let Some(cap) = self.capture.as_mut() else { return i };
        let limit = if cap.kind == Kind::Status { MAX_STATUS } else { self.limit };
        let mut k = i;
        while k < bytes.len() {
            let b = bytes[k];
            k += 1;
            let push = |cap: &mut Capture, b: u8| {
                if cap.buf.len() >= limit {
                    cap.overflow = true;
                } else {
                    cap.buf.push(b);
                }
            };
            let finished = match (cap.kind, cap.esc, b) {
                (kind, false, 0x07) if kind.is_osc() => true,
                (_, false, 0x1b) => {
                    cap.esc = true;
                    false
                }
                (_, true, b'\\') => true,
                (Kind::Dcs, true, 0x1b) => {
                    cap.esc = false;
                    push(cap, 0x1b);
                    false
                }
                (_, true, _) => {
                    // ESC then something else cancels the string; the bytes go to the parser.
                    self.capture = None;
                    out.push(Piece::Owned(vec![0x1b, b]));
                    return k;
                }
                (_, false, _) => {
                    push(cap, b);
                    false
                }
            };
            if finished {
                let Some(cap) = self.capture.take() else { return k };
                if cap.overflow {
                    log::warn(&format!("inline image or status report dropped: payload over {limit} bytes"));
                } else {
                    out.push(match cap.kind {
                        Kind::Status => Piece::Status(cap.buf),
                        Kind::Osc => Piece::File(cap.buf),
                        Kind::Dcs => Piece::Wrapped(cap.buf),
                        Kind::Apc => Piece::Kitty(cap.buf),
                        Kind::Start => Piece::MultipartStart(cap.buf),
                        Kind::Part => Piece::MultipartPart(cap.buf),
                        Kind::End => Piece::MultipartEnd,
                    });
                }
                return k;
            }
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes that reach the parser, and the images/wrapped bodies found, in order.
    fn run(chunks: &[&[u8]]) -> (Vec<u8>, Vec<String>) {
        let mut ic = Intercept::default();
        run_with(&mut ic, chunks)
    }

    fn run_with(ic: &mut Intercept, chunks: &[&[u8]]) -> (Vec<u8>, Vec<String>) {
        let (mut pass, mut found) = (Vec::new(), Vec::new());
        for chunk in chunks {
            for piece in ic.split(chunk) {
                match piece {
                    Piece::Pass(b) => pass.extend_from_slice(b),
                    Piece::Owned(b) => pass.extend(b),
                    Piece::File(p) => found.push(format!("file:{}", String::from_utf8_lossy(&p))),
                    Piece::Wrapped(p) => found.push(format!("wrapped:{}", String::from_utf8_lossy(&p))),
                    Piece::Kitty(p) => found.push(format!("kitty:{}", String::from_utf8_lossy(&p))),
                    Piece::MultipartStart(p) => found.push(format!("start:{}", String::from_utf8_lossy(&p))),
                    Piece::MultipartPart(p) => found.push(format!("part:{}", String::from_utf8_lossy(&p))),
                    Piece::MultipartEnd => found.push("end".to_string()),
                    Piece::Status(p) => found.push(format!("status:{}", String::from_utf8_lossy(&p))),
                }
            }
        }
        (pass, found)
    }

    #[test]
    fn plain_text_is_one_borrowed_piece() {
        let mut ic = Intercept::default();
        let pieces = ic.split(b"hello \x1b[31mred");
        assert_eq!(pieces, vec![Piece::Pass(b"hello \x1b[31mred")]);
    }

    #[test]
    fn an_image_ends_at_bel_or_st() {
        let (pass, found) = run(&[b"a\x1b]1337;File=inline=1:Zm9v\x07b"]);
        assert_eq!(pass, b"ab");
        assert_eq!(found, ["file:inline=1:Zm9v"]);
        let (pass, found) = run(&[b"a\x1b]1337;File=x:Zm9v\x1b\\b"]);
        assert_eq!(pass, b"ab");
        assert_eq!(found, ["file:x:Zm9v"]);
    }

    #[test]
    fn the_result_is_the_same_wherever_reads_are_cut() {
        let stream: &[u8] = b"pre\x1b[1m\x1b]0;t\x07\x1b]1337;File=a=b:QUJD\x07mid\x1bPtmux;\x1b\x1b]1337;File=c:REVG\x07\x1b\\post";
        let whole = run(&[stream]);
        assert_eq!(whole.1, ["file:a=b:QUJD", "wrapped:\x1b]1337;File=c:REVG\x07"]);
        for cut in 0..=stream.len() {
            assert_eq!(run(&[&stream[..cut], &stream[cut..]]), whole, "cut at {cut}");
        }
        let singles: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(run(&singles), whole);
    }

    #[test]
    fn other_sequences_pass_through_untouched() {
        let stream: &[u8] = b"\x1b]0;title\x07\x1b[31m\x1b]1337;SetMark\x07\x1bPqdata\x1b\\";
        let (pass, found) = run(&[stream]);
        assert_eq!(pass, stream);
        assert!(found.is_empty());
        let singles: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(run(&singles).0, stream);
    }

    #[test]
    fn a_prefix_cut_by_the_read_is_released_in_order_or_completed() {
        assert_eq!(run(&[b"ab\x1b]13", b"x"]), (b"ab\x1b]13x".to_vec(), vec![]));
        assert_eq!(run(&[b"\x1b]13", b"37;File=:AA\x07z"]), (b"z".to_vec(), vec!["file::AA".to_string()]));
        assert_eq!(run(&[b"\x1b", b"[0m"]).0, b"\x1b[0m");
    }

    #[test]
    fn an_escape_inside_an_image_cancels_it() {
        let (pass, found) = run(&[b"x\x1b]1337;File=a:Zm\x1b[0my"]);
        assert!(found.is_empty());
        assert_eq!(pass, b"x\x1b[0my");
    }

    #[test]
    fn a_kitty_command_ends_at_st_only_and_is_cut_out_whole() {
        let (pass, found) = run(&[b"a\x1b_Ga=T,f=100;Zm9v\x07still\x1b\\b"]);
        assert_eq!(pass, b"ab");
        assert_eq!(found, ["kitty:a=T,f=100;Zm9v\x07still"]);
    }

    #[test]
    fn kitty_commands_survive_being_cut_anywhere() {
        let stream: &[u8] = b"x\x1b_Gi=1,m=1;AAAA\x1b\\y\x1b_Gm=0;BBBB\x1b\\z\x1b[0m";
        let whole = run(&[stream]);
        assert_eq!(whole.0, b"xyz\x1b[0m");
        assert_eq!(whole.1, ["kitty:i=1,m=1;AAAA", "kitty:m=0;BBBB"]);
        for cut in 1..stream.len() {
            assert_eq!(run(&[&stream[..cut], &stream[cut..]]), whole, "cut at {cut}");
        }
    }

    #[test]
    fn a_multipart_image_comes_out_as_start_parts_end_wherever_reads_are_cut() {
        let stream: &[u8] = b"a\x1b]1337;MultipartFile=inline=1;size=3\x07\x1b]1337;FilePart=Zm9v\x07\x1b]1337;FilePart=YmFy\x07\x1b]1337;FileEnd\x07b\x1b]1337;SetMark\x07";
        let whole = run(&[stream]);
        assert_eq!(whole.0, b"ab\x1b]1337;SetMark\x07");
        assert_eq!(whole.1, ["start:inline=1;size=3", "part:Zm9v", "part:YmFy", "end"]);
        for cut in 1..stream.len() {
            assert_eq!(run(&[&stream[..cut], &stream[cut..]]), whole, "cut at {cut}");
        }
        let singles: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(run(&singles), whole);
    }

    #[test]
    fn other_apc_strings_are_left_to_the_parser() {
        let (pass, found) = run(&[b"\x1b_Xnot kitty\x1b\\"]);
        assert_eq!(pass, b"\x1b_Xnot kitty\x1b\\");
        assert!(found.is_empty());
    }

    #[test]
    fn a_payload_over_the_limit_is_dropped_but_the_stream_recovers() {
        let mut ic = Intercept::with_limit(4);
        let (pass, found) = run_with(&mut ic, &[b"a\x1b]1337;File=:AAAAAAAAAA\x07b\x1b]1337;File=:AA\x07c"]);
        assert_eq!(pass, b"abc");
        assert_eq!(found, ["file::AA"]);
    }
}
