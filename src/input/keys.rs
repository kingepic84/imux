//! Decoding of terminal input into keys: legacy control bytes, ESC-prefixed Alt, CSI/SS3 cursor
//! and function keys, xterm `modifyOtherKeys` and CSI u ("extended keys").

pub const SHIFT: u8 = 1;
pub const ALT: u8 = 2;
pub const CTRL: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    Char(char),
    Enter,
    Tab,
    Escape,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    /// A mouse event as a binding sees it, never decoded from key bytes (see `server::mousekey`).
    Mouse(MouseKey),
    /// A complete sequence that is not a key we know (focus reports, replies); forwarded as is.
    Other,
}

/// What the pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseKind {
    Down,
    Up,
    /// Moved with a button held.
    Drag,
    /// The button of a drag was let go.
    DragEnd,
    WheelUp,
    WheelDown,
    /// A second press close to the first in time and place.
    Double,
    /// A third.
    Triple,
}

/// Where the pointer was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseLoc {
    Pane,
    /// A line between two panes.
    Border,
    /// A window's entry in the status line.
    Status,
    /// The session part at the left of the status line.
    StatusLeft,
    /// The blocks at the right of the status line.
    StatusRight,
    /// The rest of the status line.
    StatusDefault,
}

const LOCATIONS: [(&str, MouseLoc); 6] = [
    ("StatusDefault", MouseLoc::StatusDefault),
    ("StatusRight", MouseLoc::StatusRight),
    ("StatusLeft", MouseLoc::StatusLeft),
    ("Status", MouseLoc::Status),
    ("Border", MouseLoc::Border),
    ("Pane", MouseLoc::Pane),
];

/// A mouse event as a key name: `MouseDown1Pane`, `WheelUpStatus`, `DoubleClick1Pane`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseKey {
    pub kind: MouseKind,
    /// 1 to 3; 0 for the wheel.
    pub button: u8,
    pub loc: MouseLoc,
}

impl MouseKey {
    pub fn name(&self) -> String {
        let (stem, buttoned) = match self.kind {
            MouseKind::Down => ("MouseDown", true),
            MouseKind::Up => ("MouseUp", true),
            MouseKind::Drag => ("MouseDrag", true),
            MouseKind::DragEnd => ("MouseDragEnd", true),
            MouseKind::WheelUp => ("WheelUp", false),
            MouseKind::WheelDown => ("WheelDown", false),
            MouseKind::Double => ("DoubleClick", true),
            MouseKind::Triple => ("TripleClick", true),
        };
        let loc = LOCATIONS.iter().find(|(_, l)| *l == self.loc).map_or("", |(n, _)| *n);
        if buttoned { format!("{stem}{}{loc}", self.button) } else { format!("{stem}{loc}") }
    }

    /// The inverse of `name`; a button is 1, 2 or 3 and a location is required.
    pub fn parse(text: &str) -> Option<MouseKey> {
        let (loc, head) = LOCATIONS.iter().find_map(|(n, l)| text.strip_suffix(n).map(|h| (*l, h)))?;
        let stems = [
            ("MouseDragEnd", MouseKind::DragEnd),
            ("MouseDown", MouseKind::Down),
            ("MouseUp", MouseKind::Up),
            ("MouseDrag", MouseKind::Drag),
            ("DoubleClick", MouseKind::Double),
            ("TripleClick", MouseKind::Triple),
        ];
        for (stem, kind) in stems {
            if let Some(digit) = head.strip_prefix(stem) {
                return match digit.parse::<u8>() {
                    Ok(button @ 1..=3) if digit.len() == 1 => Some(MouseKey { kind, button, loc }),
                    _ => None,
                };
            }
        }
        match head {
            "WheelUp" => Some(MouseKey { kind: MouseKind::WheelUp, button: 0, loc }),
            "WheelDown" => Some(MouseKey { kind: MouseKind::WheelDown, button: 0, loc }),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: Code,
    pub mods: u8,
}

/// A decoded key and the exact bytes it came from, so unbound keys can be passed on untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub key: Key,
    pub raw: Vec<u8>,
}

impl Key {
    pub fn new(code: Code, mods: u8) -> Key {
        Key { code, mods }.normalized()
    }

    /// One spelling per key: Shift on a letter is the capital letter, Ctrl on a letter is lowercase.
    fn normalized(mut self) -> Key {
        if let Code::Char(c) = self.code {
            if self.mods & CTRL != 0 && c.is_ascii_alphabetic() {
                self.code = Code::Char(c.to_ascii_lowercase());
                self.mods &= !SHIFT;
            } else if self.mods & SHIFT != 0 && c.is_ascii_lowercase() && self.mods & CTRL == 0 {
                self.code = Code::Char(c.to_ascii_uppercase());
                self.mods &= !SHIFT;
            } else if c.is_ascii_uppercase() {
                self.mods &= !SHIFT;
            }
        }
        self
    }

    /// The name `bind` takes and `list-keys` shows: `C-b`, `M-Up`, `PageUp`, `Space`, `F5`.
    pub fn name(&self) -> String {
        let base = match self.code {
            Code::Char(' ') => "Space".to_string(),
            Code::Char(c) => c.to_string(),
            Code::Enter => "Enter".to_string(),
            Code::Tab => "Tab".to_string(),
            Code::Escape => "Escape".to_string(),
            Code::Backspace => "BSpace".to_string(),
            Code::Up => "Up".to_string(),
            Code::Down => "Down".to_string(),
            Code::Left => "Left".to_string(),
            Code::Right => "Right".to_string(),
            Code::Home => "Home".to_string(),
            Code::End => "End".to_string(),
            Code::PageUp => "PageUp".to_string(),
            Code::PageDown => "PageDown".to_string(),
            Code::Insert => "IC".to_string(),
            Code::Delete => "DC".to_string(),
            Code::F(n) => format!("F{n}"),
            Code::Mouse(m) => m.name(),
            Code::Other => "Other".to_string(),
        };
        let mut out = String::new();
        for (bit, prefix) in [(CTRL, "C-"), (ALT, "M-"), (SHIFT, "S-")] {
            if self.mods & bit != 0 {
                out.push_str(prefix);
            }
        }
        out + &base
    }

    /// Parse a tmux key name: `a`, `H`, `C-a`, `M-x`, `Tab`, `Escape`, `Up`, `C-Up`, `F5`, `Space`.
    pub fn parse(name: &str) -> Option<Key> {
        let mut mods = 0;
        let mut rest = name;
        while rest.len() > 2 && rest.as_bytes()[1] == b'-' {
            mods |= match rest.as_bytes()[0] {
                b'C' => CTRL,
                b'M' => ALT,
                b'S' => SHIFT,
                _ => return None,
            };
            rest = &rest[2..];
        }
        // `BTab` is tmux's name for Shift-Tab.
        if rest == "BTab" {
            mods |= SHIFT;
            rest = "Tab";
        }
        let code = match rest {
            "Enter" => Code::Enter,
            "Tab" => Code::Tab,
            "Escape" => Code::Escape,
            "BSpace" => Code::Backspace,
            "Space" => Code::Char(' '),
            "Up" => Code::Up,
            "Down" => Code::Down,
            "Left" => Code::Left,
            "Right" => Code::Right,
            "Home" => Code::Home,
            "End" => Code::End,
            "PageUp" | "PgUp" => Code::PageUp,
            "PageDown" | "PgDn" => Code::PageDown,
            "IC" | "Insert" => Code::Insert,
            "DC" | "Delete" => Code::Delete,
            f if f.len() > 1 && f.starts_with('F') && f[1..].bytes().all(|b| b.is_ascii_digit()) => {
                match f[1..].parse() {
                    Ok(n @ 1..=12) => Code::F(n),
                    _ => return None,
                }
            }
            s if MouseKey::parse(s).is_some() => Code::Mouse(MouseKey::parse(s)?),
            s if s.chars().count() == 1 => Code::Char(s.chars().next()?),
            _ => return None,
        };
        Some(Key::new(code, mods))
    }
}

fn modifiers(param: Option<u32>) -> u8 {
    param.map_or(0, |m| (m.saturating_sub(1) & 7) as u8)
}

/// The bytes a program reads when `key` is pressed (what `send-keys` types). `app_cursor` is the
/// program's cursor key mode: arrows, Home and End come as `ESC O x` instead of `ESC [ x`.
/// Keys with no usual spelling give `None`.
pub fn encode(key: &Key, app_cursor: bool) -> Option<Vec<u8>> {
    let modifier = u32::from(key.mods) + 1;
    let tilde = |n: u32| if key.mods == 0 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{modifier}~") };
    let letter = |c: char| match (key.mods, app_cursor) {
        (0, true) => format!("\x1bO{c}"),
        (0, false) => format!("\x1b[{c}"),
        _ => format!("\x1b[1;{modifier}{c}"),
    };
    let text = match key.code {
        Code::Char(c) => return encode_char(c, key.mods),
        Code::Enter => return Some(with_alt(b"\r", key.mods)),
        Code::Tab if key.mods & SHIFT != 0 => "\x1b[Z".to_string(),
        Code::Tab => return Some(with_alt(b"\t", key.mods)),
        Code::Escape => return Some(with_alt(b"\x1b", key.mods)),
        Code::Backspace => return Some(with_alt(b"\x7f", key.mods)),
        Code::Up => letter('A'),
        Code::Down => letter('B'),
        Code::Right => letter('C'),
        Code::Left => letter('D'),
        Code::Home => letter('H'),
        Code::End => letter('F'),
        Code::PageUp => tilde(5),
        Code::PageDown => tilde(6),
        Code::Insert => tilde(2),
        Code::Delete => tilde(3),
        Code::F(n @ 1..=4) => {
            let c = char::from(b'P' + n - 1);
            if key.mods == 0 { format!("\x1bO{c}") } else { format!("\x1b[1;{modifier}{c}") }
        }
        Code::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n) - 5]),
        Code::F(_) | Code::Mouse(_) | Code::Other => return None,
    };
    Some(text.into_bytes())
}

fn with_alt(bytes: &[u8], mods: u8) -> Vec<u8> {
    let mut out = Vec::new();
    if mods & ALT != 0 {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

fn encode_char(c: char, mods: u8) -> Option<Vec<u8>> {
    let base = if mods & CTRL != 0 {
        match c {
            'a'..='z' => vec![c as u8 - b'a' + 1],
            ' ' | '@' => vec![0],
            '[' => vec![0x1b],
            '\\' => vec![0x1c],
            ']' => vec![0x1d],
            '^' => vec![0x1e],
            '_' => vec![0x1f],
            '?' => vec![0x7f],
            _ => return None,
        }
    } else {
        c.to_string().into_bytes()
    };
    Some(with_alt(&base, mods))
}

fn code_for_codepoint(cp: u32) -> Option<Code> {
    Some(match cp {
        9 => Code::Tab,
        13 => Code::Enter,
        27 => Code::Escape,
        127 => Code::Backspace,
        _ => Code::Char(char::from_u32(cp)?),
    })
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

fn csi_key(params: &[Option<u32>], fin: u8) -> Key {
    let p = |i: usize| params.get(i).copied().flatten();
    let other = Key { code: Code::Other, mods: 0 };
    let arrow = |code| Key::new(code, modifiers(p(1)));
    match fin {
        b'A' => arrow(Code::Up),
        b'B' => arrow(Code::Down),
        b'C' => arrow(Code::Right),
        b'D' => arrow(Code::Left),
        b'H' => arrow(Code::Home),
        b'F' => arrow(Code::End),
        b'P'..=b'S' => Key::new(Code::F(fin - b'P' + 1), modifiers(p(1))),
        b'Z' => Key::new(Code::Tab, SHIFT),
        b'u' => match p(0).and_then(code_for_codepoint) {
            Some(code) => Key::new(code, modifiers(p(1))),
            None => other,
        },
        b'~' => {
            let mods = modifiers(p(1));
            let code = match p(0) {
                Some(1 | 7) => Code::Home,
                Some(2) => Code::Insert,
                Some(3) => Code::Delete,
                Some(4 | 8) => Code::End,
                Some(5) => Code::PageUp,
                Some(6) => Code::PageDown,
                Some(n @ 11..=15) => Code::F((n - 10) as u8),
                Some(n @ 17..=21) => Code::F((n - 11) as u8),
                Some(n @ 23..=24) => Code::F((n - 12) as u8),
                Some(27) => {
                    return match p(2).and_then(code_for_codepoint) {
                        Some(code) => Key::new(code, modifiers(p(1))),
                        None => other,
                    };
                }
                _ => return other,
            };
            Key::new(code, mods)
        }
        _ => other,
    }
}

fn ss3_key(fin: u8) -> Key {
    let code = match fin {
        b'A' => Code::Up,
        b'B' => Code::Down,
        b'C' => Code::Right,
        b'D' => Code::Left,
        b'H' => Code::Home,
        b'F' => Code::End,
        b'P'..=b'S' => Code::F(fin - b'P' + 1),
        _ => Code::Other,
    };
    Key { code, mods: 0 }
}

fn control_key(b: u8) -> Key {
    match b {
        0x00 => Key::new(Code::Char(' '), CTRL),
        0x09 => Key::new(Code::Tab, 0),
        0x0d => Key::new(Code::Enter, 0),
        0x7f => Key::new(Code::Backspace, 0),
        0x01..=0x1a => Key::new(Code::Char((b + 0x60) as char), CTRL),
        0x1c..=0x1f => Key::new(Code::Char((b + 0x40) as char), CTRL),
        _ => Key { code: Code::Other, mods: 0 },
    }
}

/// Decode as many whole keys as `buf` holds. Returns them with the number of bytes consumed; an
/// incomplete sequence at the end is left unconsumed unless `flush` says no more bytes will come
/// (then a lone ESC is the Escape key and other fragments are passed on as unknown keys).
pub fn parse(buf: &[u8], flush: bool) -> (Vec<Parsed>, usize) {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let rest = &buf[i..];
        let (key, len) = match rest[0] {
            0x1b => match rest.get(1) {
                None if flush => (Key::new(Code::Escape, 0), 1),
                None => break,
                Some(b'[') => {
                    let Some(n) = rest[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
                        if flush {
                            out.push(Parsed { key: Key { code: Code::Other, mods: 0 }, raw: rest.to_vec() });
                            i = buf.len();
                        }
                        break;
                    };
                    let params: Vec<Option<u32>> = std::str::from_utf8(&rest[2..2 + n])
                        .unwrap_or("")
                        .split(';')
                        .map(|f| f.parse().ok())
                        .collect();
                    (csi_key(&params, rest[2 + n]), 3 + n)
                }
                Some(b'O') => match rest.get(2) {
                    Some(&fin) => (ss3_key(fin), 3),
                    None if flush => (Key::new(Code::Char('O'), ALT), 2),
                    None => break,
                },
                Some(0x1b) => (Key::new(Code::Escape, 0), 1),
                Some(&b) if b < 0x20 || b == 0x7f => {
                    let k = control_key(b);
                    (Key::new(k.code, k.mods | ALT), 2)
                }
                Some(&lead) => {
                    let need = utf8_len(lead);
                    if rest.len() < 1 + need {
                        if flush {
                            (Key { code: Code::Other, mods: 0 }, rest.len())
                        } else {
                            break;
                        }
                    } else {
                        match std::str::from_utf8(&rest[1..1 + need]).ok().and_then(|s| s.chars().next()) {
                            Some(c) => (Key::new(Code::Char(c), ALT), 1 + need),
                            None => (Key { code: Code::Other, mods: 0 }, 2),
                        }
                    }
                }
            },
            b if b < 0x20 || b == 0x7f => (control_key(b), 1),
            b if b < 0x80 => (Key::new(Code::Char(b as char), 0), 1),
            lead => {
                let need = utf8_len(lead);
                if rest.len() < need {
                    if flush {
                        (Key { code: Code::Other, mods: 0 }, rest.len())
                    } else {
                        break;
                    }
                } else {
                    match std::str::from_utf8(&rest[..need]).ok().and_then(|s| s.chars().next()) {
                        Some(c) => (Key::new(Code::Char(c), 0), need),
                        None => (Key { code: Code::Other, mods: 0 }, 1),
                    }
                }
            }
        };
        out.push(Parsed { key, raw: rest[..len].to_vec() });
        i += len;
    }
    (out, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bytes: &[u8]) -> Vec<Key> {
        parse(bytes, true).0.into_iter().map(|p| p.key).collect()
    }

    #[test]
    fn btab_is_the_tmux_name_of_shift_tab() {
        assert_eq!(Key::parse("BTab"), Key::parse("S-Tab"));
        assert_eq!(Key::parse("M-BTab"), Key::parse("M-S-Tab"));
        assert!(Key::parse("BTab").is_some());
    }

    fn k(name: &str) -> Key {
        Key::parse(name).unwrap()
    }

    #[test]
    fn plain_and_control_characters() {
        assert_eq!(keys(b"aH"), [k("a"), k("H")]);
        assert_eq!(keys(b"\x01"), [k("C-a")]);
        assert_eq!(keys(b"\x00"), [k("C-Space")]);
        assert_eq!(keys(b"\r\t\x7f"), [k("Enter"), k("Tab"), k("BSpace")]);
    }

    #[test]
    fn utf8_characters_are_one_key() {
        assert_eq!(keys("é".as_bytes()), [Key::new(Code::Char('é'), 0)]);
        let (out, used) = parse(&"é".as_bytes()[..1], false);
        assert!(out.is_empty());
        assert_eq!(used, 0);
    }

    #[test]
    fn escape_sequences_for_cursor_and_editing_keys() {
        assert_eq!(keys(b"\x1b[A\x1b[B\x1b[C\x1b[D"), [k("Up"), k("Down"), k("Right"), k("Left")]);
        assert_eq!(keys(b"\x1bOA\x1bOH"), [k("Up"), k("Home")]);
        assert_eq!(keys(b"\x1b[5~\x1b[6~\x1b[3~"), [k("PageUp"), k("PageDown"), k("DC")]);
        assert_eq!(keys(b"\x1b[15~\x1b[24~\x1bOP"), [k("F5"), k("F12"), k("F1")]);
    }

    #[test]
    fn modifiers_in_csi_sequences() {
        assert_eq!(keys(b"\x1b[1;5A"), [k("C-Up")]);
        assert_eq!(keys(b"\x1b[1;3D"), [k("M-Left")]);
        assert_eq!(keys(b"\x1b[Z"), [k("S-Tab")]);
    }

    #[test]
    fn extended_keys_csi_u_and_modify_other_keys() {
        assert_eq!(keys(b"\x1b[97;5u"), [k("C-a")]);
        assert_eq!(keys(b"\x1b[72;2u"), [k("H")]);
        assert_eq!(keys(b"\x1b[13;2u"), [k("S-Enter")]);
        assert_eq!(keys(b"\x1b[27u"), [k("Escape")]);
        assert_eq!(keys(b"\x1b[27;5;97~"), [k("C-a")]);
    }

    #[test]
    fn alt_is_an_escape_prefix() {
        assert_eq!(keys(b"\x1bx"), [k("M-x")]);
        assert_eq!(keys(b"\x1b\x01"), [k("M-C-a")]);
    }

    #[test]
    fn a_lone_escape_waits_for_more_unless_flushed() {
        let (out, used) = parse(b"\x1b", false);
        assert!(out.is_empty());
        assert_eq!(used, 0);
        assert_eq!(keys(b"\x1b"), [k("Escape")]);
        let (out, used) = parse(b"a\x1b[", false);
        assert_eq!(out.len(), 1);
        assert_eq!(used, 1);
    }

    #[test]
    fn raw_bytes_are_kept_for_forwarding() {
        let (out, _) = parse(b"a\x1b[1;5A\x1b[I", true);
        assert_eq!(out[0].raw, b"a");
        assert_eq!(out[1].raw, b"\x1b[1;5A");
        assert_eq!(out[2].key.code, Code::Other);
        assert_eq!(out[2].raw, b"\x1b[I");
    }

    #[test]
    fn key_names_parse_to_the_decoded_form() {
        assert_eq!(k("C-A"), k("C-a"));
        assert_eq!(k("S-h"), k("H"));
        assert_eq!(k("Space"), Key::new(Code::Char(' '), 0));
        assert!(Key::parse("C-").is_none());
        assert!(Key::parse("X-a").is_none());
        assert!(Key::parse("F13").is_none());
        assert!(Key::parse("Nonsense").is_none());
    }

    #[test]
    fn every_key_name_parses_back_to_the_same_key() {
        for name in [
            "a", "Z", "0", "%", "\"", "Space", "Enter", "Tab", "Escape", "BSpace", "Up", "C-Down", "M-Left", "C-M-Right", "S-Up", "Home",
            "End", "PageUp", "PageDown", "IC", "DC", "F1", "F12", "C-b", "M-x", "C-Space",
        ] {
            let key = Key::parse(name).unwrap_or_else(|| panic!("{name} does not parse"));
            assert_eq!(Key::parse(&key.name()), Some(key), "{name} -> {}", key.name());
        }
        assert_eq!(k("C-b").name(), "C-b");
        assert_eq!(k("M-Up").name(), "M-Up");
        assert_eq!(k(" ").name(), "Space");
    }

    fn typed(name: &str, app: bool) -> Vec<u8> {
        encode(&k(name), app).unwrap_or_else(|| panic!("{name} has no bytes"))
    }

    #[test]
    fn keys_encode_to_what_a_program_reads() {
        assert_eq!(typed("a", false), b"a");
        assert_eq!(typed("é", false), "é".as_bytes());
        assert_eq!(typed("C-c", false), [3]);
        assert_eq!(typed("C-Space", false), [0]);
        assert_eq!(typed("C-[", false), [0x1b]);
        assert_eq!(typed("M-x", false), b"\x1bx");
        assert_eq!(typed("C-M-a", false), [0x1b, 1]);
        assert_eq!(typed("Enter", false), b"\r");
        assert_eq!(typed("Tab", false), b"\t");
        assert_eq!(typed("S-Tab", false), b"\x1b[Z");
        assert_eq!(typed("Escape", false), b"\x1b");
        assert_eq!(typed("BSpace", false), [0x7f]);
        assert_eq!(typed("Space", false), b" ");
    }

    #[test]
    fn cursor_keys_follow_the_application_cursor_mode_and_carry_modifiers() {
        assert_eq!(typed("Up", false), b"\x1b[A");
        assert_eq!(typed("Up", true), b"\x1bOA");
        assert_eq!(typed("Home", false), b"\x1b[H");
        assert_eq!(typed("End", true), b"\x1bOF");
        assert_eq!(typed("C-Up", true), b"\x1b[1;5A");
        assert_eq!(typed("M-Left", false), b"\x1b[1;3D");
        assert_eq!(typed("S-Right", false), b"\x1b[1;2C");
        assert_eq!(typed("PageUp", false), b"\x1b[5~");
        assert_eq!(typed("C-PageDown", false), b"\x1b[6;5~");
        assert_eq!(typed("IC", false), b"\x1b[2~");
        assert_eq!(typed("DC", false), b"\x1b[3~");
    }

    #[test]
    fn function_keys_encode_and_unknown_ones_do_not() {
        assert_eq!(typed("F1", false), b"\x1bOP");
        assert_eq!(typed("F4", true), b"\x1bOS");
        assert_eq!(typed("S-F1", false), b"\x1b[1;2P");
        assert_eq!(typed("F5", false), b"\x1b[15~");
        assert_eq!(typed("F6", false), b"\x1b[17~");
        assert_eq!(typed("F10", false), b"\x1b[21~");
        assert_eq!(typed("F12", false), b"\x1b[24~");
        assert_eq!(encode(&Key { code: Code::Other, mods: 0 }, false), None);
        assert_eq!(encode(&k("C-1"), false), None, "Ctrl and a digit has no byte");
    }

    #[test]
    fn what_encode_writes_decodes_back_to_the_key() {
        for name in ["a", "C-c", "M-x", "Enter", "Tab", "Escape", "Up", "C-Up", "Home", "PageUp", "DC", "F1", "F5", "F12", "S-Tab"] {
            let key = k(name);
            let bytes = encode(&key, false).unwrap();
            let (parsed, _) = parse(&bytes, true);
            assert_eq!(parsed.len(), 1, "{name}: {bytes:?}");
            assert_eq!(parsed[0].key, key, "{name}");
        }
    }
}

#[cfg(test)]
mod mouse_key_tests {
    use super::*;

    #[test]
    fn every_mouse_key_name_round_trips() {
        let mut seen = 0;
        for loc in ["Pane", "Border", "Status", "StatusLeft", "StatusRight", "StatusDefault"] {
            for stem in ["MouseDown", "MouseUp", "MouseDrag", "MouseDragEnd", "DoubleClick", "TripleClick"] {
                for button in 1..=3 {
                    let name = format!("{stem}{button}{loc}");
                    let key = Key::parse(&name).unwrap_or_else(|| panic!("{name} does not parse"));
                    assert_eq!(key.name(), name);
                    seen += 1;
                }
            }
            for name in [format!("WheelUp{loc}"), format!("WheelDown{loc}")] {
                let key = Key::parse(&name).unwrap_or_else(|| panic!("{name} does not parse"));
                assert_eq!(key.name(), name);
                seen += 1;
            }
        }
        assert_eq!(seen, 6 * (18 + 2));
    }

    #[test]
    fn modifiers_go_in_front_and_in_the_usual_order() {
        let key = Key::parse("C-M-MouseDown3Pane").unwrap();
        assert_eq!(key.mods, CTRL | ALT);
        assert_eq!(key.name(), "C-M-MouseDown3Pane");
        assert_ne!(Key::parse("MouseDown3Pane"), Key::parse("C-MouseDown3Pane"));
        assert_eq!(Key::parse("S-WheelUpStatus").unwrap().mods, SHIFT);
    }

    #[test]
    fn names_that_are_not_mouse_keys_are_refused() {
        for name in ["MouseDown4Pane", "MouseDown0Pane", "MouseDown1", "MouseDown12Pane", "WheelUp1Pane", "Wheel", "MouseDownPane", "MouseDown1Pan", "Mouse"] {
            assert_eq!(Key::parse(name), None, "{name}");
        }
    }

    #[test]
    fn a_mouse_key_has_no_bytes_to_type() {
        assert_eq!(encode(&Key::parse("MouseDown1Pane").unwrap(), false), None);
    }
}
