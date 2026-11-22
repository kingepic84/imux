//! Asking the outer terminal what it can do, instead of guessing from the environment (which
//! ssh and `sudo` lose). Three questions go out in one write, the last being Primary Device
//! Attributes, which every terminal answers: when its reply is in, the others will not come.
//!
//! - `CSI > 0 q` (XTVERSION): the terminal's name, e.g. `kitty(0.35.2)`, `foot(1.17.2)`.
//! - A Kitty graphics query with a 1x1 pixel: `OK` means the protocol works.
//! - `CSI c`: its attribute list contains 4 when Sixel is supported.

use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use crate::proto::{Caps, ImageMode};
use crate::tty::{read_some, write_all};

pub const QUERY: &[u8] = b"\x1b[>0q\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub sixel: bool,
    pub kitty: bool,
    /// What XTVERSION said, empty when it did not answer.
    pub name: String,
    /// The attribute reply (the last of the three) has arrived.
    pub done: bool,
}

/// Split terminal replies out of `bytes`. Everything else, such as keys typed meanwhile, is
/// returned in order so it can still be delivered.
pub fn parse(bytes: &[u8]) -> (Probe, Vec<u8>) {
    let mut probe = Probe::default();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let tail = &bytes[i..];
        if let Some((len, kind)) = reply_at(tail) {
            let body = &tail[..len];
            match kind {
                Reply::Attributes => {
                    probe.sixel |= body[3..body.len() - 1].split(|&b| b == b';').any(|p| p == b"4");
                    probe.done = true;
                }
                Reply::Version => probe.name = String::from_utf8_lossy(&body[4..body.len() - 2]).into_owned(),
                Reply::Kitty => probe.kitty |= body.windows(3).any(|w| w == b";OK"),
            }
            i += len;
        } else {
            rest.push(bytes[i]);
            i += 1;
        }
    }
    (probe, rest)
}

enum Reply {
    Attributes,
    Version,
    Kitty,
}

/// A complete reply at the start of `s`: its length and kind.
fn reply_at(s: &[u8]) -> Option<(usize, Reply)> {
    let st = |from: usize| s[from..].windows(2).position(|w| w == b"\x1b\\").map(|p| from + p + 2);
    if s.starts_with(b"\x1b[?") {
        let end = s.iter().position(|b| (0x40..=0x7e).contains(b) && *b != b'?' && !b.is_ascii_digit() && *b != b';' && *b != b'[')?;
        return (s[end] == b'c').then_some((end + 1, Reply::Attributes));
    }
    if s.starts_with(b"\x1bP>|") {
        return st(4).map(|n| (n, Reply::Version));
    }
    if s.starts_with(b"\x1b_G") {
        return st(3).map(|n| (n, Reply::Kitty));
    }
    None
}

/// Takes the answers of a Kitty-graphics terminal (`ESC _ G ... ESC \`) out of what the user types.
/// imux asks for none after the probe, but a terminal can still answer: a reply arriving late would
/// otherwise be typed into the pane's shell as `Gi=0;OK`. A reply cut in two reads is held until it
/// is complete.
#[derive(Default)]
pub struct ReplyFilter {
    held: Vec<u8>,
}

/// Most bytes of one reply held back; longer is not a reply.
const MAX_REPLY: usize = 4096;

impl ReplyFilter {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            let tail = &data[i..];
            if tail.starts_with(b"\x1b_G") {
                match tail[3..].windows(2).position(|w| w == b"\x1b\\") {
                    Some(p) => {
                        i += 3 + p + 2;
                        continue;
                    }
                    None if tail.len() <= MAX_REPLY => {
                        self.held = tail.to_vec();
                        break;
                    }
                    None => {}
                }
            }
            out.push(data[i]);
            i += 1;
        }
        out
    }
}

/// Whether to ask the terminal at all (`image-probe`). `auto` leaves out the terminals that cannot
/// answer anything better than the blocks the environment gives already, so they are never shown
/// query strings they do not understand: Terminal.app (no image protocol, no Sixel) and `dumb`.
pub fn should_ask(mode: &str, env: impl Fn(&str) -> Option<String>) -> bool {
    match mode {
        "always" => true,
        "never" => false,
        _ => env("TERM_PROGRAM").as_deref() != Some("Apple_Terminal") && env("TERM").as_deref() != Some("dumb"),
    }
}

/// Ask the terminal and wait up to `patience` for its answer (`image-probe-timeout`; ssh round
/// trips are the slow case). Bytes that are not answers come back too.
pub fn run(tty_in: &File, tty_out: &File, patience: Duration) -> io::Result<(Probe, Vec<u8>)> {
    write_all(tty_out.as_fd(), QUERY)?;
    let deadline = Instant::now() + patience;
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match read_some(tty_in.as_fd(), &mut buf)? {
            Some(0) => break,
            Some(n) => got.extend_from_slice(&buf[..n]),
            None => std::thread::sleep(Duration::from_millis(5)),
        }
        if parse(&got).0.done || Instant::now() >= deadline {
            break;
        }
    }
    Ok(parse(&got))
}

fn rank(m: ImageMode) -> u8 {
    match m {
        ImageMode::Iterm2 => 3,
        ImageMode::Kitty => 2,
        ImageMode::Sixel => 1,
        ImageMode::Blocks => 0,
    }
}

/// The picture protocols this terminal accepts, as the bits of `ClientMsg::Modes`. Without an answer
/// to the probe nothing can be ruled out and everything is allowed (`image-mode` is then trusted);
/// with one, a protocol counts when the terminal said so or the environment named it.
pub fn accepted_modes(env: Caps, probe: &Probe) -> u8 {
    use crate::proto::mode_bit;
    if !probe.done {
        return 7;
    }
    let named = |p: &str| probe.name.starts_with(p);
    let mut mask = mode_bit(env.images);
    if named("iTerm2") || named("WezTerm") {
        mask |= mode_bit(ImageMode::Iterm2);
    }
    if probe.kitty {
        mask |= mode_bit(ImageMode::Kitty);
    }
    if probe.sixel {
        mask |= mode_bit(ImageMode::Sixel);
    }
    mask
}

/// Combine what the environment suggested with what the terminal said: the better protocol wins.
#[cfg(test)]
pub fn merge(env: Caps, probe: &Probe) -> Caps {
    merge_preferring(env, probe, "inline")
}

/// `merge`, with `image-prefer`: `auto` and `inline` try the iTerm2 protocol first (then Kitty,
/// Sixel, blocks); `kitty` shows pictures on an iTerm2 that answered the Kitty query through that
/// protocol instead, which sends each picture once and only places parts of it afterwards.
pub fn merge_preferring(env: Caps, probe: &Probe, prefer: &str) -> Caps {
    if probe.kitty && probe.name.starts_with("iTerm2") && prefer == "kitty" {
        return Caps { images: ImageMode::Kitty, truecolor: true };
    }
    let named = |p: &str| probe.name.starts_with(p);
    let from_probe = if named("iTerm2") || named("WezTerm") {
        ImageMode::Iterm2
    } else if probe.kitty {
        ImageMode::Kitty
    } else if probe.sixel {
        ImageMode::Sixel
    } else {
        ImageMode::Blocks
    };
    let images = if rank(from_probe) > rank(env.images) { from_probe } else { env.images };
    Caps { images, truecolor: env.truecolor || images != ImageMode::Blocks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kitty_terminal_answers_all_three() {
        let (p, rest) = parse(b"\x1bP>|kitty(0.35.2)\x1b\\\x1b_Gi=31;OK\x1b\\\x1b[?62;c");
        assert_eq!((p.name.as_str(), p.kitty, p.sixel, p.done), ("kitty(0.35.2)", true, false, true));
        assert!(rest.is_empty());
    }

    #[test]
    fn sixel_is_attribute_4() {
        assert!(parse(b"\x1b[?65;4;6;22c").0.sixel);
        assert!(!parse(b"\x1b[?65;14;6c").0.sixel, "14 is not 4");
        assert!(!parse(b"\x1b[?62;22c").0.sixel);
    }

    #[test]
    fn auto_does_not_ask_a_terminal_that_cannot_help() {
        let env = |vars: &'static [(&'static str, &'static str)]| move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert!(!should_ask("auto", env(&[("TERM_PROGRAM", "Apple_Terminal")])));
        assert!(!should_ask("auto", env(&[("TERM", "dumb")])));
        assert!(should_ask("auto", env(&[("TERM_PROGRAM", "iTerm.app")])));
        assert!(should_ask("auto", env(&[])), "over ssh nothing is known: ask");
        assert!(should_ask("always", env(&[("TERM_PROGRAM", "Apple_Terminal")])));
        assert!(!should_ask("never", env(&[])));
    }

    #[test]
    fn a_late_kitty_reply_is_not_typed_into_the_pane() {
        let mut f = ReplyFilter::default();
        assert_eq!(f.feed(b"ab\x1b_Gi=0;OK\x1b\\cd"), b"abcd");
        assert_eq!(f.feed(b"\x1b_Gi=0;OK\x1b\\\x1b_Gi=3;OK\x1b\\"), b"");
        assert_eq!(f.feed(b"\x1b[A"), b"\x1b[A", "other sequences pass");
        assert_eq!(f.feed(b"\x1b"), b"\x1b", "a lone escape key is not held");
    }

    #[test]
    fn a_reply_cut_between_two_reads_is_held_and_dropped_whole() {
        let mut f = ReplyFilter::default();
        assert_eq!(f.feed(b"x\x1b_Gi=0;"), b"x");
        assert_eq!(f.feed(b"OK\x1b"), b"");
        assert_eq!(f.feed(b"\\y"), b"y");
        let long = [b"\x1b_G".as_slice(), &[b'a'; 5000]].concat();
        assert_eq!(f.feed(&long).len(), long.len(), "something that never ends is not a reply");
    }

    #[test]
    fn a_failed_kitty_query_is_not_support() {
        let (p, _) = parse(b"\x1b_Gi=31;EINVAL:nope\x1b\\\x1b[?1;2c");
        assert!(!p.kitty);
        assert!(p.done);
    }

    #[test]
    fn keys_typed_meanwhile_are_given_back_in_order() {
        let (p, rest) = parse(b"ab\x1b[?62;4c\x1b[Acd");
        assert!(p.done && p.sixel);
        assert_eq!(rest, b"ab\x1b[Acd");
    }

    #[test]
    fn a_reply_cut_short_is_not_taken_for_complete() {
        let (p, rest) = parse(b"\x1b_Gi=31;OK");
        assert!(!p.kitty && !p.done);
        assert_eq!(rest, b"\x1b_Gi=31;OK");
        assert!(!parse(b"\x1b[?62;4").0.done);
    }

    #[test]
    fn silence_changes_nothing() {
        let env = Caps { images: ImageMode::Sixel, truecolor: false };
        assert_eq!(merge(env, &Probe::default()), Caps { images: ImageMode::Sixel, truecolor: true });
        assert_eq!(merge(Caps::BASIC, &Probe::default()), Caps::BASIC);
    }

    #[test]
    fn a_terminal_that_answered_is_taken_at_its_word_and_one_that_did_not_is_trusted() {
        let env = Caps { images: ImageMode::Blocks, truecolor: false };
        assert_eq!(accepted_modes(env, &Probe::default()), 7, "no answer: nothing is ruled out");
        let apple = Probe { done: true, ..Probe::default() };
        assert_eq!(accepted_modes(env, &apple), 0, "answered, offers nothing");
        let iterm = Probe { done: true, name: "iTerm2 3.7.7".into(), kitty: true, sixel: true };
        assert_eq!(accepted_modes(env, &iterm), 7);
        let foot = Probe { done: true, sixel: true, ..Probe::default() };
        assert_eq!(accepted_modes(env, &foot), 4);
        let by_env = Caps { images: ImageMode::Kitty, truecolor: true };
        assert_eq!(accepted_modes(by_env, &apple), 2, "named by the environment, silent in the probe");
    }

    #[test]
    fn the_iterm2_protocol_is_tried_first_and_kitty_on_an_iterm2_only_when_asked_for() {
        let iterm = Probe { name: "iTerm2 3.7.7".into(), kitty: true, sixel: true, ..Probe::default() };
        let env = Caps { images: ImageMode::Iterm2, truecolor: true };
        assert_eq!(merge_preferring(env, &iterm, "auto").images, ImageMode::Iterm2);
        assert_eq!(merge_preferring(env, &iterm, "inline").images, ImageMode::Iterm2);
        assert_eq!(merge_preferring(env, &iterm, "kitty").images, ImageMode::Kitty);
        assert_eq!(merge_preferring(Caps::BASIC, &iterm, "auto").images, ImageMode::Iterm2, "found by its name alone");
        let silent = Probe { name: "iTerm2 3.7.7".into(), ..Probe::default() };
        assert_eq!(merge_preferring(env, &silent, "kitty").images, ImageMode::Iterm2, "no answer, no Kitty");
        let wez = Probe { name: "WezTerm 2024".into(), kitty: true, ..Probe::default() };
        assert_eq!(merge_preferring(Caps::BASIC, &wez, "kitty").images, ImageMode::Iterm2, "only iTerm2 is moved");
        let kitty_only = Probe { kitty: true, sixel: true, ..Probe::default() };
        assert_eq!(merge_preferring(Caps::BASIC, &kitty_only, "auto").images, ImageMode::Kitty, "then Kitty");
        let sixel_only = Probe { sixel: true, ..Probe::default() };
        assert_eq!(merge_preferring(Caps::BASIC, &sixel_only, "auto").images, ImageMode::Sixel, "then Sixel");
        assert_eq!(merge_preferring(Caps::BASIC, &Probe::default(), "auto").images, ImageMode::Blocks, "then blocks");
    }

    #[test]
    fn the_better_protocol_wins_whoever_found_it() {
        let kitty = Probe { kitty: true, sixel: true, ..Probe::default() };
        assert_eq!(merge(Caps::BASIC, &kitty).images, ImageMode::Kitty);
        let sixel = Probe { sixel: true, ..Probe::default() };
        assert_eq!(merge(Caps { images: ImageMode::Kitty, truecolor: true }, &sixel).images, ImageMode::Kitty);
        let wez = Probe { name: "WezTerm 2024".into(), kitty: true, ..Probe::default() };
        assert_eq!(merge(Caps::BASIC, &wez).images, ImageMode::Iterm2);
        assert_eq!(merge(Caps { images: ImageMode::Iterm2, truecolor: true }, &kitty).images, ImageMode::Iterm2);
    }
}
