use vte::{Params, Perform};

use super::grid::{Charset, DcsQuery, Event, Grid, MouseMode};
use super::intercept::MAX_PAYLOAD;
use crate::image::iterm::{self, Dim};
use crate::image::sixel;
use crate::log;

const MAX_TITLE: usize = 1024;

/// Longest DCS query that is looked at; the rest of a longer one is dropped.
const MAX_DCS_QUERY: usize = 256;

/// At most this many capability names are answered from one request.
const MAX_CAPABILITY_NAMES: usize = 8;

/// The terminfo capabilities imux states (XTGETTCAP): name and value. A reply is built from this
/// table only, never from the bytes of the request, so a pane cannot make imux say anything else.
const CAPABILITIES: &[(&str, &str)] = &[("TN", "imux"), ("Co", "256"), ("colors", "256"), ("RGB", "8/8/8")];

/// `DCS $ q Pt ST` (DECRQSS): the value of a setting imux keeps, or "invalid request".
fn answer_setting(g: &mut Grid, id: &[u8]) {
    let value = match id {
        b"m" => {
            let mut s = String::new();
            crate::render::sgr(&mut s, &g.pen, true);
            Some(format!("{}m", &s[2..s.len() - 1]))
        }
        b"r" => Some(format!("{};{}r", g.top + 1, g.bottom + 1)),
        b" q" => Some(format!("{} q", g.cursor_style)),
        _ => None,
    };
    match value {
        Some(v) => g.reply(format!("\x1bP1$r{v}\x1b\\").as_bytes()),
        None => g.reply(b"\x1bP0$r\x1b\\"),
    }
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02X}")).collect()
}

/// `DCS + q Pt ST` (XTGETTCAP): `Pt` is hex-encoded names separated by `;`. Each gets its own
/// reply: the value when imux has one, "not found" otherwise.
fn answer_capabilities(g: &mut Grid, names: &[u8]) {
    for name in names.split(|&b| b == b';').take(MAX_CAPABILITY_NAMES) {
        let known = CAPABILITIES.iter().find(|(n, _)| hex(n).as_bytes() == name.to_ascii_uppercase().as_slice());
        match known {
            Some((n, v)) => g.reply(format!("\x1bP1+r{}={}\x1b\\", hex(n), hex(v)).as_bytes()),
            None => g.reply(b"\x1bP0+r\x1b\\"),
        }
    }
}

pub struct Performer<'a> {
    pub grid: &'a mut Grid,
}

fn num(ps: &[usize], i: usize, default: usize) -> usize {
    match ps.get(i) {
        Some(&v) if v != 0 => v,
        _ => default,
    }
}

impl Perform for Performer<'_> {
    fn print(&mut self, c: char) {
        self.grid.put(c);
    }

    fn execute(&mut self, byte: u8) {
        let g = &mut *self.grid;
        match byte {
            0x07 => g.events.push(Event::Bell),
            0x08 => g.backspace(),
            0x09 => g.tab(1),
            0x0A..=0x0C => g.newline(),
            0x0D => g.carriage_return(),
            0x0E => g.shift(1),
            0x0F => g.shift(0),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        let g = &mut *self.grid;
        match (intermediates, byte) {
            ([], b'7') => g.save_cursor(),
            ([], b'8') => g.restore_cursor(),
            ([], b'D') => g.linefeed(),
            ([], b'E') => {
                g.carriage_return();
                g.linefeed();
            }
            ([], b'M') => g.reverse_index(),
            ([], b'H') => g.set_tab(),
            ([], b'c') => g.reset(),
            ([], b'=') => g.modes.app_keypad = true,
            ([], b'>') => g.modes.app_keypad = false,
            ([slot @ (b'(' | b')')], final_byte) => {
                let cs = if final_byte == b'0' { Charset::DecGraphics } else { Charset::Ascii };
                g.set_charset(usize::from(*slot == b')'), cs);
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let groups: Vec<&[u16]> = params.iter().collect();
        let ps: Vec<usize> = groups.iter().map(|g| usize::from(g.first().copied().unwrap_or(0))).collect();
        let g = &mut *self.grid;
        match (intermediates, action) {
            ([], 'A') => g.move_y_rel(-(num(&ps, 0, 1) as isize)),
            ([], 'B' | 'e') => g.move_y_rel(num(&ps, 0, 1) as isize),
            ([], 'C' | 'a') => g.move_x_rel(num(&ps, 0, 1) as isize),
            ([], 'D') => g.move_x_rel(-(num(&ps, 0, 1) as isize)),
            ([], 'E') => {
                g.move_y_rel(num(&ps, 0, 1) as isize);
                g.carriage_return();
            }
            ([], 'F') => {
                g.move_y_rel(-(num(&ps, 0, 1) as isize));
                g.carriage_return();
            }
            ([], 'G' | '`') => g.move_x(num(&ps, 0, 1) - 1),
            ([], 'H' | 'f') => g.cup(num(&ps, 0, 1), num(&ps, 1, 1)),
            ([], 'd') => g.move_y_abs(num(&ps, 0, 1)),
            ([], 'I') => g.tab(num(&ps, 0, 1)),
            ([], 'Z') => g.back_tab(num(&ps, 0, 1)),
            ([], 'J') => g.erase_display(ps.first().copied().unwrap_or(0)),
            ([], 'K') => g.erase_line(ps.first().copied().unwrap_or(0)),
            ([], 'L') => g.insert_lines(num(&ps, 0, 1)),
            ([], 'M') => g.delete_lines(num(&ps, 0, 1)),
            ([], '@') => g.insert_chars(num(&ps, 0, 1)),
            ([], 'P') => g.delete_chars(num(&ps, 0, 1)),
            ([], 'X') => g.erase_chars(num(&ps, 0, 1)),
            ([], 'S') => g.scroll_up(num(&ps, 0, 1)),
            ([], 'T') if ps.len() <= 1 => g.scroll_down(num(&ps, 0, 1)),
            ([], 'r') => g.set_scroll_region(num(&ps, 0, 1), ps.get(1).copied().unwrap_or(0)),
            ([], 'm') => g.sgr(&groups),
            ([], 'b') => g.repeat_last(num(&ps, 0, 1)),
            ([], 'g') => g.clear_tab(ps.first() == Some(&3)),
            ([], 's') => g.save_cursor(),
            ([], 'u') => g.restore_cursor(),
            ([], 'h') => set_ansi_mode(g, &ps, true),
            ([], 'l') => set_ansi_mode(g, &ps, false),
            ([b'?'], 'h') => ps.iter().for_each(|&m| set_private_mode(g, m, true)),
            ([b'?'], 'l') => ps.iter().for_each(|&m| set_private_mode(g, m, false)),
            ([], 'n') => device_status(g, ps.first().copied().unwrap_or(0), false),
            ([b'?'], 'n') => device_status(g, ps.first().copied().unwrap_or(0), true),
            ([], 'c') if ps.first().copied().unwrap_or(0) == 0 => g.reply(b"\x1b[?1;2;4c"),
            ([b'?'], 'S') => graphics_attributes(g, &ps),
            ([b'>'], 'c') => g.reply(b"\x1b[>0;0;0c"),
            ([], 't') => window_ops(g, &ps),
            ([b' '], 'q') => g.cursor_style = ps.first().copied().unwrap_or(0) as u16,
            ([b'!'], 'p') => soft_reset(g),
            ([b'?', b'$'], 'p') => report_private_mode(g, ps.first().copied().unwrap_or(0)),
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        match (intermediates, action) {
            ([], 'q') => {
                self.grid.sixel = Some(Vec::new());
                self.grid.sixel_overflow = false;
            }
            ([b'$'], 'q') => self.grid.dcs = Some((DcsQuery::Setting, Vec::new())),
            ([b'+'], 'q') => self.grid.dcs = Some((DcsQuery::Capability, Vec::new())),
            _ => {}
        }
    }

    fn put(&mut self, byte: u8) {
        let g = &mut *self.grid;
        if let Some((_, data)) = g.dcs.as_mut() {
            if data.len() < MAX_DCS_QUERY {
                data.push(byte);
            }
            return;
        }
        let Some(data) = g.sixel.as_mut() else { return };
        if data.len() >= MAX_PAYLOAD {
            g.sixel_overflow = true;
        } else {
            data.push(byte);
        }
    }

    fn unhook(&mut self) {
        let g = &mut *self.grid;
        if let Some((query, data)) = g.dcs.take() {
            return match query {
                DcsQuery::Setting => answer_setting(g, &data),
                DcsQuery::Capability => answer_capabilities(g, &data),
            };
        }
        let Some(data) = g.sixel.take() else { return };
        if std::mem::take(&mut g.sixel_overflow) {
            return log::warn(&format!("sixel image dropped: over {MAX_PAYLOAD} bytes"));
        }
        match sixel::decode(&data) {
            Ok(bitmap) => {
                let (cols, rows) = iterm::footprint(bitmap.w, bitmap.h, Dim::Auto, Dim::Auto, true, g.image_metrics());
                g.place_bitmap(bitmap, cols, rows);
            }
            Err(e) => log::warn(&format!("sixel image: {e}")),
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let g = &mut *self.grid;
        let Some(code) = params.first() else { return };
        let rest = |from: usize| -> String {
            let joined: Vec<&[u8]> = params.iter().skip(from).copied().collect();
            String::from_utf8_lossy(&joined.join(&b';')).into_owned()
        };
        match *code {
            b"0" | b"1" | b"2" => {
                // Titles are shown in status lines and handed to hooks, scripts and plugins: no
                // control, C1 or bidirectional characters (security review P-08, F-F).
                let mut title = crate::sanitize::clean(&rest(1));
                title.truncate(title.floor_char_boundary(MAX_TITLE));
                g.title.clone_from(&title);
                g.events.push(Event::Title(title));
            }
            b"7" => {
                let url = rest(1);
                let path = url.strip_prefix("file://").map(|r| r.find('/').map_or("/", |i| &r[i..]));
                if let Some(path) = path {
                    let path = percent_decode(path);
                    g.cwd = Some(path.clone());
                    g.events.push(Event::Cwd(path));
                }
            }
            b"133" => {
                if params.get(1).is_some_and(|p| p.first() == Some(&b'A')) {
                    g.events.push(Event::PromptStart);
                }
            }
            b"52" => {
                let target = params.get(1).map(|t| String::from_utf8_lossy(t).into_owned()).unwrap_or_default();
                let data = params.get(2).map(|d| String::from_utf8_lossy(d).into_owned()).unwrap_or_default();
                if let Some((target, data)) = super::clip::request(&target, &data) {
                    g.events.push(Event::Clipboard { target, data });
                }
            }
            _ => {}
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(hex) = std::str::from_utf8(&b[i + 1..i + 3])
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn set_ansi_mode(g: &mut Grid, ps: &[usize], on: bool) {
    for &m in ps {
        match m {
            4 => g.modes.insert = on,
            20 => g.modes.newline = on,
            _ => {}
        }
    }
}

fn set_private_mode(g: &mut Grid, mode: usize, on: bool) {
    match mode {
        1 => g.modes.cursor_keys = on,
        5 => g.modes.reverse_video = on,
        6 => {
            g.modes.origin = on;
            g.cup(1, 1);
        }
        7 => g.modes.autowrap = on,
        25 => g.modes.cursor_visible = on,
        47 | 1047 => {
            if on {
                g.enter_alt(false);
            } else {
                g.leave_alt(false);
            }
        }
        1048 => {
            if on {
                g.save_cursor();
            } else {
                g.restore_cursor();
            }
        }
        1049 => {
            if on {
                g.enter_alt(true);
            } else {
                g.leave_alt(true);
            }
        }
        1000 | 1002 | 1003 => {
            let want = match mode {
                1000 => MouseMode::Click,
                1002 => MouseMode::Drag,
                _ => MouseMode::Any,
            };
            if on {
                g.modes.mouse = want;
            } else if g.modes.mouse == want {
                g.modes.mouse = MouseMode::None;
            }
        }
        1004 => g.modes.focus_events = on,
        1006 => g.modes.mouse_sgr = on,
        2004 => g.modes.bracketed_paste = on,
        2026 => g.modes.sync_output = on,
        _ => {}
    }
}

fn report_private_mode(g: &mut Grid, mode: usize) {
    let state = match mode {
        1 => g.modes.cursor_keys,
        6 => g.modes.origin,
        7 => g.modes.autowrap,
        25 => g.modes.cursor_visible,
        47 | 1047 | 1049 => g.is_alt(),
        1000 => g.modes.mouse == MouseMode::Click,
        1002 => g.modes.mouse == MouseMode::Drag,
        1003 => g.modes.mouse == MouseMode::Any,
        1004 => g.modes.focus_events,
        1006 => g.modes.mouse_sgr,
        2004 => g.modes.bracketed_paste,
        2026 => g.modes.sync_output,
        _ => {
            g.reply(format!("\x1b[?{mode};0$y").as_bytes());
            return;
        }
    };
    g.reply(format!("\x1b[?{mode};{}$y", if state { 1 } else { 2 }).as_bytes());
}

fn device_status(g: &mut Grid, kind: usize, private: bool) {
    match kind {
        5 => g.reply(b"\x1b[0n"),
        6 => {
            let row = if g.modes.origin { g.cy.saturating_sub(g.top) } else { g.cy } + 1;
            let col = g.cx + 1;
            let q = if private { "?" } else { "" };
            g.reply(format!("\x1b[{q}{row};{col}R").as_bytes());
        }
        _ => {}
    }
}

fn window_ops(g: &mut Grid, ps: &[usize]) {
    let (cw, ch) = (usize::from(g.cell_px.0), usize::from(g.cell_px.1));
    match ps.first() {
        Some(14) if cw > 0 && ch > 0 => g.reply(format!("\x1b[4;{};{}t", ch * g.rows, cw * g.cols).as_bytes()),
        Some(16) if cw > 0 && ch > 0 => g.reply(format!("\x1b[6;{ch};{cw}t").as_bytes()),
        Some(18) => g.reply(format!("\x1b[8;{};{}t", g.rows, g.cols).as_bytes()),
        _ => {}
    }
}

/// Sixel color registers a program may use; the decoder has no limit, this is what the output
/// side keeps of a picture.
const SIXEL_REGISTERS: usize = 256;

/// XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`): what a program may ask before sending a Sixel. Item 1
/// is the color registers, item 2 the largest picture in pixels (the pane). The reply is
/// `CSI ? Pi ; Ps ; Pv S` with Ps 0 for success, 1 for an item not known, 3 for a refused set.
fn graphics_attributes(g: &mut Grid, ps: &[usize]) {
    let item = ps.first().copied().unwrap_or(0);
    let action = ps.get(1).copied().unwrap_or(0);
    let reply = match (item, action) {
        (1, 1 | 2 | 4) => format!("\x1b[?1;0;{SIXEL_REGISTERS}S"),
        (2, 1 | 2 | 4) => {
            let m = g.image_metrics();
            format!("\x1b[?2;0;{};{}S", m.cols * m.cell_w, m.rows * m.cell_h)
        }
        (1 | 2, _) => format!("\x1b[?{item};3S"),
        _ => format!("\x1b[?{item};1S"),
    };
    g.reply(reply.as_bytes());
}

fn soft_reset(g: &mut Grid) {
    g.modes = Default::default();
    g.pen = Default::default();
    g.set_charset(0, Charset::Ascii);
    g.set_charset(1, Charset::Ascii);
    g.shift(0);
    g.top = 0;
    g.bottom = g.rows - 1;
    g.cursor_style = 0;
}

#[cfg(test)]
mod tests {
    use super::super::Terminal;
    use super::super::cell::{BOLD, Color, UNDERLINE};
    use super::super::grid::{Event, MouseMode};

    fn term(cols: usize, rows: usize) -> Terminal {
        Terminal::new(cols, rows, 100)
    }

    fn feed(t: &mut Terminal, s: &str) {
        t.feed(s.as_bytes());
    }

    fn screen(t: &Terminal) -> Vec<String> {
        t.grid.screen.iter().map(|r| r.text()).collect()
    }

    fn history(t: &Terminal) -> Vec<String> {
        t.grid.scrollback.iter().map(|r| r.text()).collect()
    }

    #[test]
    fn prints_and_wraps_with_soft_wrap_flag() {
        let mut t = term(5, 3);
        feed(&mut t, "abcdefgh");
        assert_eq!(screen(&t), ["abcde", "fgh", ""]);
        assert!(t.grid.screen[0].wrapped);
        assert!(!t.grid.screen[1].wrapped);
    }

    #[test]
    fn last_column_wraps_only_on_next_char() {
        let mut t = term(5, 3);
        feed(&mut t, "abcde");
        assert_eq!((t.grid.cx, t.grid.cy), (4, 0));
        feed(&mut t, "f");
        assert_eq!((t.grid.cx, t.grid.cy), (1, 1));
        assert_eq!(screen(&t), ["abcde", "f", ""]);
    }

    #[test]
    fn crlf_and_scrollback() {
        let mut t = term(5, 3);
        feed(&mut t, "1\r\n2\r\n3\r\n4");
        assert_eq!(screen(&t), ["2", "3", "4"]);
        assert_eq!(history(&t), ["1"]);
    }

    #[test]
    fn lf_alone_keeps_column() {
        let mut t = term(5, 3);
        feed(&mut t, "ab\ncd");
        assert_eq!(screen(&t), ["ab", "  cd", ""]);
    }

    #[test]
    fn cursor_addressing_and_erase() {
        let mut t = term(6, 3);
        feed(&mut t, "\x1b[2;3Hx\x1b[1;1Habc");
        assert_eq!(screen(&t), ["abc", "  x", ""]);
        feed(&mut t, "\x1b[1;2H\x1b[K");
        assert_eq!(screen(&t)[0], "a");
        feed(&mut t, "\x1b[2J");
        assert_eq!(screen(&t), ["", "", ""]);
    }

    #[test]
    fn erase_line_modes() {
        let mut t = term(6, 1);
        feed(&mut t, "abcdef\x1b[1;4H\x1b[1K");
        assert_eq!(screen(&t)[0], "    ef");
        feed(&mut t, "\x1b[2K");
        assert_eq!(screen(&t)[0], "");
    }

    #[test]
    fn scroll_region_confines_scrolling() {
        let mut t = term(4, 4);
        feed(&mut t, "a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(screen(&t), ["a", "c", "", "d"]);
        assert!(history(&t).is_empty());
    }

    #[test]
    fn insert_and_delete_lines() {
        let mut t = term(3, 4);
        feed(&mut t, "a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[L");
        assert_eq!(screen(&t), ["a", "", "b", "c"]);
        feed(&mut t, "\x1b[M");
        assert_eq!(screen(&t), ["a", "b", "c", ""]);
    }

    #[test]
    fn char_editing() {
        let mut t = term(6, 1);
        feed(&mut t, "abcdef\x1b[1;2H\x1b[2@");
        assert_eq!(screen(&t)[0], "a  bcd");
        feed(&mut t, "\x1b[2P");
        assert_eq!(screen(&t)[0], "abcd");
        feed(&mut t, "\x1b[1;2H\x1b[2X");
        assert_eq!(screen(&t)[0], "a  d");
    }

    #[test]
    fn sgr_attributes_and_colors() {
        let mut t = term(10, 1);
        feed(&mut t, "\x1b[1;4;31ma\x1b[38;5;196mb\x1b[38;2;1;2;3mc\x1b[38:2::4:5:6md\x1b[0me");
        let c = &t.grid.screen[0].cells;
        assert_eq!(c[0].style.attrs, BOLD | UNDERLINE);
        assert_eq!(c[0].style.fg, Color::Indexed(1));
        assert_eq!(c[1].style.fg, Color::Indexed(196));
        assert_eq!(c[2].style.fg, Color::Rgb(1, 2, 3));
        assert_eq!(c[3].style.fg, Color::Rgb(4, 5, 6));
        assert_eq!(c[4].style.attrs, 0);
        assert_eq!(c[4].style.fg, Color::Default);
    }

    #[test]
    fn bright_and_background_colors() {
        let mut t = term(4, 1);
        feed(&mut t, "\x1b[92;104ma\x1b[39;49mb");
        let c = &t.grid.screen[0].cells;
        assert_eq!((c[0].style.fg, c[0].style.bg), (Color::Indexed(10), Color::Indexed(12)));
        assert_eq!((c[1].style.fg, c[1].style.bg), (Color::Default, Color::Default));
    }

    #[test]
    fn erase_keeps_background_color() {
        let mut t = term(4, 1);
        feed(&mut t, "\x1b[44m\x1b[K");
        assert!(t.grid.screen[0].cells.iter().all(|c| c.style.bg == Color::Indexed(4)));
    }

    #[test]
    fn wide_chars_take_two_cells() {
        let mut t = term(6, 2);
        feed(&mut t, "a日b");
        let c = &t.grid.screen[0].cells;
        assert_eq!((c[1].ch, c[1].width, c[2].width, c[3].ch), ('日', 2, 0, 'b'));
        assert_eq!(t.grid.cx, 4);
    }

    #[test]
    fn wide_char_wraps_instead_of_splitting() {
        let mut t = term(4, 2);
        feed(&mut t, "abc日");
        assert_eq!(screen(&t), ["abc", "日"]);
    }

    #[test]
    fn overwriting_half_of_wide_char_blanks_other_half() {
        let mut t = term(6, 1);
        feed(&mut t, "日\x1b[1;2Hx");
        assert_eq!(screen(&t)[0], " x");
        assert_eq!(t.grid.screen[0].cells[0].width, 1);
    }

    #[test]
    fn alt_screen_restores_main_and_cursor() {
        let mut t = term(8, 3);
        feed(&mut t, "main\x1b[?1049h\x1b[2;2Halt");
        assert!(t.grid.is_alt());
        assert_eq!(screen(&t), ["", " alt", ""]);
        feed(&mut t, "\x1b[?1049l");
        assert!(!t.grid.is_alt());
        assert_eq!(screen(&t), ["main", "", ""]);
        assert_eq!((t.grid.cx, t.grid.cy), (4, 0));
    }

    #[test]
    fn alt_screen_never_feeds_scrollback() {
        let mut t = term(4, 2);
        feed(&mut t, "\x1b[?1049h1\r\n2\r\n3\r\n4");
        assert!(history(&t).is_empty());
    }

    #[test]
    fn dec_graphics_charset() {
        let mut t = term(5, 1);
        feed(&mut t, "\x1b(0lqk\x1b(Bq");
        assert_eq!(screen(&t)[0], "┌─┐q");
    }

    #[test]
    fn tabs_default_every_eight() {
        let mut t = term(20, 1);
        feed(&mut t, "a\tb\tc");
        assert_eq!(screen(&t)[0], "a       b       c");
    }

    #[test]
    fn save_and_restore_cursor() {
        let mut t = term(6, 3);
        feed(&mut t, "\x1b[2;3H\x1b7\x1b[1;1H\x1b8x");
        assert_eq!(screen(&t), ["", "  x", ""]);
    }

    #[test]
    fn repeat_last_char() {
        let mut t = term(6, 1);
        feed(&mut t, "a\x1b[3b");
        assert_eq!(screen(&t)[0], "aaaa");
    }

    #[test]
    fn origin_mode_is_relative_to_region() {
        let mut t = term(5, 5);
        feed(&mut t, "\x1b[2;4r\x1b[?6h\x1b[1;1Hx");
        assert_eq!(screen(&t), ["", "x", "", "", ""]);
    }

    #[test]
    fn replies_for_status_queries() {
        let mut t = term(10, 5);
        t.grid.cell_px = (7, 14);
        feed(&mut t, "\x1b[3;4H\x1b[6n\x1b[5n\x1b[c\x1b[18t\x1b[16t");
        let r = String::from_utf8(t.grid.take_replies()).unwrap();
        assert_eq!(r, "\x1b[3;4R\x1b[0n\x1b[?1;2;4c\x1b[8;5;10t\x1b[6;14;7t");
    }

    fn replies(t: &mut Terminal) -> String {
        String::from_utf8(t.grid.take_replies()).unwrap()
    }

    #[test]
    fn setting_requests_get_imuxs_own_values() {
        let mut t = term(10, 5);
        feed(&mut t, "\x1b[2;4r\x1b[5 q\x1bP$qr\x1b\\\x1bP$q q\x1b\\\x1bP$qm\x1b\\");
        assert_eq!(replies(&mut t), "\x1bP1$r2;4r\x1b\\\x1bP1$r5 q\x1b\\\x1bP1$r0m\x1b\\");
        feed(&mut t, "\x1b[1;31m\x1bP$qm\x1b\\");
        assert_eq!(replies(&mut t), "\x1bP1$r0;1;31m\x1b\\");
    }

    #[test]
    fn a_setting_imux_does_not_keep_is_an_invalid_request_without_an_echo() {
        let mut t = term(10, 5);
        feed(&mut t, "\x1bP$qsecret\x1b\\\x1bP$q\x1b\\");
        assert_eq!(replies(&mut t), "\x1bP0$r\x1b\\\x1bP0$r\x1b\\");
    }

    #[test]
    fn capability_requests_name_only_what_imux_states() {
        let mut t = term(10, 5);
        feed(&mut t, "\x1bP+q544e;436f;Tc\x1b\\");
        assert_eq!(replies(&mut t), "\x1bP1+r544E=696D7578\x1b\\\x1bP1+r436F=323536\x1b\\\x1bP0+r\x1b\\");
        feed(&mut t, "\x1bP+q524742\x1b\\");
        assert_eq!(replies(&mut t), "\x1bP1+r524742=382F382F38\x1b\\");
    }

    #[test]
    fn capability_requests_are_bounded_and_do_not_echo() {
        let mut t = term(10, 5);
        let many = vec!["544e"; 40].join(";");
        feed(&mut t, &format!("\x1bP+q{many}\x1b\\"));
        let r = replies(&mut t);
        assert!(r.matches("\x1bP1+r").count() <= 8, "{r:?}");
        feed(&mut t, &format!("\x1bP+q{}\x1b\\\x1bP$q{}\x1b\\", "a".repeat(100_000), "b".repeat(100_000)));
        let r = replies(&mut t);
        assert!(r.len() < 100, "no echo of the request: {r:?}");
        feed(&mut t, "text");
        assert_eq!(t.grid.screen[0].text(), "text", "the stream went on");
    }

    #[test]
    fn other_dcs_data_is_dropped_and_the_stream_goes_on() {
        let mut t = term(10, 5);
        feed(&mut t, "a\x1bP1;2|garbage\x1b\\b\x1bP=1s\x1b\\c");
        assert_eq!(t.grid.screen[0].text(), "abc");
        assert_eq!(replies(&mut t), "");
    }

    struct Clusters;

    impl Clusters {
        fn on() -> Clusters {
            crate::term::grid::set_emoji_clusters(true);
            Clusters
        }
    }

    impl Drop for Clusters {
        fn drop(&mut self) {
            crate::term::grid::set_emoji_clusters(false);
        }
    }

    #[test]
    fn a_joined_emoji_is_one_wide_cell_in_cluster_mode_and_its_parts_otherwise() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}x";
        let mut parts = term(20, 3);
        feed(&mut parts, family);
        assert_eq!(parts.grid.cx, 7, "three wide people take six cells, then x");
        let _on = Clusters::on();
        let mut t = term(20, 3);
        feed(&mut t, family);
        assert_eq!(t.grid.cx, 3, "one two-cell glyph, then x");
        assert_eq!(t.grid.screen[0].cells[0].width, 2);
        assert_eq!(t.grid.screen[0].cells[2].ch, 'x');
        assert_eq!(t.grid.screen[0].text(), family, "nothing was lost");
    }

    #[test]
    fn a_long_family_fits_in_the_cluster_cap() {
        let _on = Clusters::on();
        let mut t = term(20, 3);
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}x";
        feed(&mut t, family);
        assert_eq!(t.grid.screen[0].text(), family);
        assert_eq!(t.grid.cx, 3);
    }

    #[test]
    fn flags_skin_tones_and_emoji_selectors_are_two_cells_in_cluster_mode() {
        let _on = Clusters::on();
        for (text, name) in [("\u{1f1fa}\u{1f1f8}x", "flag"), ("\u{1f44d}\u{1f3fd}x", "skin tone"), ("\u{2764}\u{fe0f}x", "selector"), ("\u{261d}\u{1f3fd}x", "narrow base with a tone")] {
            let mut t = term(20, 3);
            feed(&mut t, text);
            assert_eq!(t.grid.screen[0].cells[0].width, 2, "{name}");
            assert_eq!(t.grid.screen[0].cells[1].width, 0, "{name}");
            assert_eq!(t.grid.screen[0].cells[2].ch, 'x', "{name}");
            assert_eq!(t.grid.screen[0].text(), text, "{name}");
        }
    }

    #[test]
    fn a_joiner_between_letters_of_other_scripts_stays_with_its_cell() {
        let _on = Clusters::on();
        let mut t = term(20, 3);
        feed(&mut t, "\u{915}\u{94d}\u{200d}\u{937}x");
        assert_eq!(t.grid.screen[0].text(), "\u{915}\u{94d}\u{200d}\u{937}x");
        assert_eq!(t.grid.cx, 2, "the ZWJ joined the next letter into the first cell");
    }

    #[test]
    fn a_cluster_at_the_edge_of_the_line_does_not_break_the_cursor() {
        let _on = Clusters::on();
        let mut t = term(4, 3);
        feed(&mut t, "abc\u{2764}\u{fe0f}d");
        assert!(t.grid.cx < 4 && t.grid.cy < 3);
        let mut t = term(4, 3);
        feed(&mut t, "ab\u{2764}\u{fe0f}d");
        assert_eq!(t.grid.screen[0].cells[2].width, 2);
        assert_eq!(t.grid.screen[1].cells[0].ch, 'd', "wrapped after the wide cell");
    }

    #[test]
    fn graphics_queries_say_how_many_colors_and_how_big_a_picture_may_be() {
        let mut t = term(10, 5);
        t.grid.cell_px = (7, 14);
        feed(&mut t, "\x1b[?1;1;0S\x1b[?2;1;0S\x1b[?2;4S");
        let r = String::from_utf8(t.grid.take_replies()).unwrap();
        assert_eq!(r, "\x1b[?1;0;256S\x1b[?2;0;70;70S\x1b[?2;0;70;70S");
        feed(&mut t, "\x1b[?1;3;16S\x1b[?3;1S\x1b[?9;1S");
        let r = String::from_utf8(t.grid.take_replies()).unwrap();
        assert_eq!(r, "\x1b[?1;3S\x1b[?3;1S\x1b[?9;1S", "a set is refused, ReGIS and unknown items are not known");
        let mut t = term(10, 5);
        feed(&mut t, "\x1b[?2;1S");
        assert_eq!(String::from_utf8(t.grid.take_replies()).unwrap(), "\x1b[?2;0;80;80S", "8x16 pixels a cell when the terminal did not say");
    }

    #[test]
    fn device_attributes_list_sixel_and_scrolling_up_is_untouched() {
        let mut t = term(5, 3);
        feed(&mut t, "a\r\nb\r\nc\x1b[0c\x1b[S");
        assert_eq!(String::from_utf8(t.grid.take_replies()).unwrap(), "\x1b[?1;2;4c");
        assert_eq!(screen(&t), ["b", "c", ""]);
    }

    #[test]
    fn decrqm_reports_mode_state() {
        let mut t = term(10, 5);
        feed(&mut t, "\x1b[?2004h\x1b[?2004$p\x1b[?2026$p\x1b[?9999$p");
        let r = String::from_utf8(t.grid.take_replies()).unwrap();
        assert_eq!(r, "\x1b[?2004;1$y\x1b[?2026;2$y\x1b[?9999;0$y");
    }

    #[test]
    fn mode_flags() {
        let mut t = term(10, 5);
        feed(&mut t, "\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?25l\x1b[?1h");
        let m = t.grid.modes;
        assert!(m.bracketed_paste && m.mouse_sgr && m.cursor_keys && !m.cursor_visible);
        assert_eq!(m.mouse, MouseMode::Drag);
        feed(&mut t, "\x1b[?1002l");
        assert_eq!(t.grid.modes.mouse, MouseMode::None);
    }

    #[test]
    fn osc_title_cwd_clipboard_and_bell() {
        let mut t = term(10, 2);
        feed(&mut t, "\x1b]2;my;title\x07\x1b]7;file://host/Users/me/a%20b\x1b\\\x1b]52;c;aGk=\x07\x07");
        let ev = t.grid.take_events();
        assert_eq!(
            ev,
            [
                Event::Title("my;title".into()),
                Event::Cwd("/Users/me/a b".into()),
                Event::Clipboard { target: "c".into(), data: "aGk=".into() },
                Event::Bell,
            ]
        );
        assert_eq!(t.grid.title, "my;title");
    }

    #[test]
    fn utf8_split_across_feeds() {
        let mut t = term(5, 1);
        let bytes = "é".as_bytes();
        t.feed(&bytes[..1]);
        t.feed(&bytes[1..]);
        assert_eq!(screen(&t)[0], "é");
    }

    #[test]
    fn full_reset_clears_screen_and_modes() {
        let mut t = term(5, 2);
        feed(&mut t, "abc\x1b[?2004h\x1bc");
        assert_eq!(screen(&t), ["", ""]);
        assert!(!t.grid.modes.bracketed_paste);
    }

    #[test]
    fn resize_wider_joins_soft_wrapped_line_and_back() {
        let mut t = term(10, 4);
        feed(&mut t, "hello world foo");
        assert_eq!(screen(&t)[..2], ["hello worl", "d foo"]);
        t.grid.resize(20, 4, (0, 0));
        assert_eq!(screen(&t), ["hello world foo", "", "", ""]);
        assert_eq!((t.grid.cx, t.grid.cy), (15, 0));
        t.grid.resize(6, 4, (0, 0));
        assert_eq!(screen(&t), ["hello", "world", "foo", ""]);
        assert_eq!((t.grid.cx, t.grid.cy), (3, 2));
    }

    #[test]
    fn resize_height_moves_rows_through_scrollback() {
        let mut t = term(5, 3);
        feed(&mut t, "1\r\n2\r\n3");
        t.grid.resize(5, 2, (0, 0));
        assert_eq!(screen(&t), ["2", "3"]);
        assert_eq!(history(&t), ["1"]);
        assert_eq!((t.grid.cx, t.grid.cy), (1, 1));
        t.grid.resize(5, 3, (0, 0));
        assert_eq!(screen(&t), ["1", "2", "3"]);
        assert!(history(&t).is_empty());
    }

    #[test]
    fn a_cursor_report_after_restoring_a_cursor_above_the_region_does_not_underflow() {
        let mut t = term(10, 6);
        feed(&mut t, "\x1b[?6h\x1b7\x1b[3;5r\x1b8\x1b[6n");
        let r = String::from_utf8(t.grid.take_replies()).unwrap();
        assert!(r.starts_with("\x1b[") && r.ends_with('R'), "{r:?}");
    }

    #[test]
    fn a_title_loses_control_c1_and_bidirectional_characters() {
        let mut t = term(10, 3);
        feed(&mut t, "\x1b]0;x\u{9d}52;c;aGk=\u{9c}y\u{202e}z\x07");
        assert_eq!(t.grid.title, "x52;c;aGk=yz");
    }

    #[test]
    fn a_repeat_count_is_cut_at_one_screenful_and_the_reply_queue_has_a_ceiling() {
        let mut t = term(10, 3);
        feed(&mut t, "a\x1b[65535b");
        assert!(t.grid.screen.iter().map(|r| r.text()).collect::<String>().len() <= 30 + 3);
        for _ in 0..20_000 {
            feed(&mut t, "\x1b[c");
        }
        assert!(t.grid.take_replies().len() <= 64 * 1024);
    }

    #[test]
    fn a_cursor_above_the_last_line_survives_a_shrink() {
        let mut t = term(10, 12);
        feed(&mut t, "a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\nh\r\ni\r\nj\x1b[10A");
        assert_eq!(t.grid.cy, 0);
        t.grid.resize(10, 3, (0, 0));
        assert_eq!(t.grid.cy, 0);
        assert_eq!(screen(&t), ["a", "b", "c"]);
        t.grid.resize(10, 1, (0, 0));
        assert_eq!((t.grid.cy, screen(&t)), (0, vec!["a".to_string()]));
    }

    #[test]
    fn resize_keeps_prompt_at_bottom() {
        let mut t = term(10, 4);
        feed(&mut t, "\x1b[4;1H$ ");
        t.grid.resize(10, 6, (0, 0));
        assert_eq!(t.grid.cy, 3);
        assert_eq!(screen(&t)[3], "$");
    }

    #[test]
    fn resize_does_not_split_wide_chars() {
        let mut t = term(6, 2);
        feed(&mut t, "ab日cd");
        t.grid.resize(3, 3, (0, 0));
        assert_eq!(screen(&t), ["ab", "日c", "d"]);
    }

    #[test]
    fn resize_in_alt_screen_keeps_main_for_return() {
        let mut t = term(8, 3);
        feed(&mut t, "main\x1b[?1049halt");
        t.grid.resize(4, 2, (0, 0));
        assert!(t.grid.is_alt());
        assert_eq!(t.grid.screen.len(), 2);
        feed(&mut t, "\x1b[?1049l");
        assert_eq!(screen(&t), ["main", ""]);
    }

    #[test]
    fn history_limit_drops_oldest() {
        let mut t = Terminal::new(3, 2, 2);
        feed(&mut t, "1\r\n2\r\n3\r\n4\r\n5");
        assert_eq!(screen(&t), ["4", "5"]);
        assert_eq!(history(&t), ["2", "3"]);
    }

    #[test]
    fn erase_display_3_clears_scrollback() {
        let mut t = term(3, 2);
        feed(&mut t, "1\r\n2\r\n3\x1b[3J");
        assert!(history(&t).is_empty());
        assert_eq!(screen(&t), ["2", "3"]);
    }

    #[test]
    fn a_combining_accent_stays_with_its_letter_in_one_cell() {
        let mut t = term(10, 1);
        feed(&mut t, "cafe\u{301}!");
        let c = &t.grid.screen[0].cells;
        assert_eq!((c[3].ch, c[3].width), ('e', 1));
        assert_ne!(c[3].mark, 0);
        assert_eq!(c[4].ch, '!', "the accent took no column of its own");
        assert_eq!(t.grid.cx, 5);
        assert_eq!(screen(&t)[0], "cafe\u{301}!");
    }

    #[test]
    fn thai_and_devanagari_marks_stack_on_their_consonant() {
        let mut t = term(10, 1);
        feed(&mut t, "\u{e01}\u{e34}\u{e49}\u{915}\u{94d}\u{937}");
        let text = screen(&t)[0].clone();
        assert_eq!(text, "\u{e01}\u{e34}\u{e49}\u{915}\u{94d}\u{937}");
        assert_ne!(t.grid.screen[0].cells[0].mark, 0);
        assert_ne!(t.grid.screen[0].cells[1].mark, 0, "the virama goes with the consonant before it");
    }

    #[test]
    fn a_variation_selector_and_a_joiner_attach_to_the_emoji_before_them() {
        let mut t = term(10, 1);
        feed(&mut t, "\u{2764}\u{fe0f}x\u{1f468}\u{200d}\u{1f469}");
        assert_eq!(screen(&t)[0], "\u{2764}\u{fe0f}x\u{1f468}\u{200d}\u{1f469}");
        let c = &t.grid.screen[0].cells;
        assert_ne!(c[0].mark, 0);
        assert_eq!(c[1].ch, 'x');
        assert_eq!((c[2].ch, c[2].width, c[3].width), ('\u{1f468}', 2, 0));
        assert_ne!(c[2].mark, 0, "the joiner is on the lead half of the wide character");
        assert_eq!((c[4].ch, c[4].width), ('\u{1f469}', 2));
    }

    #[test]
    fn a_mark_with_nothing_before_it_is_dropped() {
        let mut t = term(5, 2);
        feed(&mut t, "\u{301}a");
        assert_eq!(t.grid.screen[0].cells[0].mark, 0);
        assert_eq!(screen(&t)[0], "a");
        feed(&mut t, "\r\n\u{301}");
        assert_eq!(t.grid.screen[1].cells[0].mark, 0);
    }

    #[test]
    fn a_mark_after_the_last_column_goes_to_the_last_cell() {
        let mut t = term(3, 2);
        feed(&mut t, "abe\u{301}");
        assert_ne!(t.grid.screen[0].cells[2].mark, 0);
        assert_eq!(screen(&t)[1], "", "nothing wrapped");
        feed(&mut t, "z");
        assert_eq!(screen(&t), ["abe\u{301}", "z"]);
    }

    #[test]
    fn overwriting_a_cell_removes_its_marks() {
        let mut t = term(5, 1);
        feed(&mut t, "e\u{301}\x1b[1Gx");
        assert_eq!(t.grid.screen[0].cells[0].mark, 0);
        assert_eq!(screen(&t)[0], "x");
    }

    #[test]
    fn at_most_three_marks_are_kept_for_a_cell() {
        let mut t = term(5, 1);
        feed(&mut t, "a\u{301}\u{302}\u{303}\u{304}\u{305}b");
        assert_eq!(screen(&t)[0], "a\u{301}\u{302}\u{303}b");
    }

    #[test]
    fn marks_survive_a_resize_and_travel_into_the_history() {
        let mut t = term(6, 2);
        feed(&mut t, "ab e\u{301}f\r\nline2\r\nline3");
        assert_eq!(history(&t), ["ab e\u{301}f"]);
        t.grid.resize(4, 2, (0, 0));
        t.grid.resize(8, 2, (0, 0));
        let all: Vec<String> = history(&t).into_iter().chain(screen(&t)).collect();
        assert!(all.iter().any(|l| l.contains("e\u{301}")), "{all:?}");
    }

    #[test]
    fn no_character_makes_the_emulator_panic_or_grow_a_cell_past_its_cap() {
        let mut t = term(8, 2);
        let mut line = String::from("a");
        for cp in (0u32..0x2_0000).step_by(7) {
            if let Some(c) = char::from_u32(cp) {
                line.push(c);
            }
            if line.chars().count() > 200 {
                feed(&mut t, &line);
                line = String::from("a");
            }
        }
        feed(&mut t, &line);
        for row in &t.grid.screen {
            for cell in &row.cells {
                let mut s = String::new();
                crate::term::marks::push_to(&mut s, cell.mark);
                assert!(s.chars().count() <= crate::term::marks::PER_CELL);
            }
        }
    }
}
