use std::collections::{HashMap, VecDeque};

use unicode_width::UnicodeWidthChar;

use crate::image::store::{DEFAULT_BUDGET, ImageId, Store};

use super::cell::{Cell, Color, Row, Style};
use super::placeholder::PLACEHOLDER;
use super::cell::{BLINK, BOLD, DIM, HIDDEN, ITALIC, REVERSE, STRIKE, UNDERLINE};

/// Most bytes of answers kept for a program that has not read the earlier ones.
const MAX_REPLIES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MouseMode {
    #[default]
    None,
    Click,
    Drag,
    Any,
}

#[derive(Clone, Copy, Debug)]
pub struct Modes {
    pub cursor_keys: bool,
    pub autowrap: bool,
    pub cursor_visible: bool,
    pub origin: bool,
    pub insert: bool,
    pub newline: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub mouse: MouseMode,
    pub mouse_sgr: bool,
    pub sync_output: bool,
    pub app_keypad: bool,
    pub reverse_video: bool,
}

impl Default for Modes {
    fn default() -> Modes {
        Modes {
            cursor_keys: false,
            autowrap: true,
            cursor_visible: true,
            origin: false,
            insert: false,
            newline: false,
            bracketed_paste: false,
            focus_events: false,
            mouse: MouseMode::None,
            mouse_sgr: false,
            sync_output: false,
            app_keypad: false,
            reverse_video: false,
        }
    }
}

/// Things the server must act on that are not screen contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Title(String),
    Cwd(String),
    Clipboard { target: String, data: String },
    Bell,
    /// A valid OSC 7501 program status report.
    Agent(super::status::Report),
    /// `OSC 133 ; A`: the shell is about to show a prompt.
    PromptStart,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Charset {
    Ascii,
    DecGraphics,
}

#[derive(Clone, Copy, Debug)]
struct SavedCursor {
    cx: usize,
    cy: usize,
    pen: Style,
    wrap_pending: bool,
    origin: bool,
    charsets: [Charset; 2],
    gl: usize,
}

pub(super) struct MainScreen {
    pub(super) screen: Vec<Row>,
    pub(super) cx: usize,
    pub(super) cy: usize,
    pub(super) wrap_pending: bool,
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    pub screen: Vec<Row>,
    pub scrollback: VecDeque<Row>,
    pub history_limit: usize,
    pub cx: usize,
    pub cy: usize,
    pub(super) wrap_pending: bool,
    pub pen: Style,
    pub(super) top: usize,
    pub(super) bottom: usize,
    saved: SavedCursor,
    pub(super) main: Option<MainScreen>,
    pub modes: Modes,
    pub(super) tabs: Vec<bool>,
    charsets: [Charset; 2],
    gl: usize,
    pub(super) last_char: Option<char>,
    pub title: String,
    pub cwd: Option<String>,
    pub cursor_style: u16,
    pub cell_px: (u16, u16),
    pub(super) replies: Vec<u8>,
    pub(super) events: Vec<Event>,
    /// Decoded images that cells' `tile` ids refer to.
    pub images: Store,
    /// Data of a Sixel string being received, while its DCS is open.
    pub(super) sixel: Option<Vec<u8>>,
    pub(super) sixel_overflow: bool,
    /// A DCS query being received (`DCS $ q` setting, `DCS + q` capability) and its data.
    pub(super) dcs: Option<(DcsQuery, Vec<u8>)>,
    /// Pictures shown by Kitty unicode placeholders, by the id the program gave: the image in the
    /// store and its size in cells.
    pub(super) virtual_images: HashMap<u32, (ImageId, usize, usize)>,
}

/// The two DCS requests that get an answer (see `parser.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DcsQuery {
    Setting,
    Capability,
}

thread_local! {
    static CLUSTERS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `emoji-width cluster` (the server sets it when the option changes): the width model of a
/// terminal that draws joined emoji, flags and skin tones as one two-cell glyph.
pub fn set_emoji_clusters(on: bool) {
    CLUSTERS.with(|c| c.set(on));
}

fn clusters() -> bool {
    CLUSTERS.with(std::cell::Cell::get)
}

fn is_regional(c: char) -> bool {
    ('\u{1f1e6}'..='\u{1f1ff}').contains(&c)
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|x| x % 8 == 0 && x != 0).collect()
}

impl Grid {
    pub fn new(cols: usize, rows: usize, history_limit: usize) -> Grid {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Grid {
            cols,
            rows,
            screen: (0..rows).map(|_| Row::new(cols, Cell::BLANK)).collect(),
            scrollback: VecDeque::new(),
            history_limit,
            cx: 0,
            cy: 0,
            wrap_pending: false,
            pen: Style::default(),
            top: 0,
            bottom: rows - 1,
            saved: SavedCursor {
                cx: 0,
                cy: 0,
                pen: Style::default(),
                wrap_pending: false,
                origin: false,
                charsets: [Charset::Ascii; 2],
                gl: 0,
            },
            main: None,
            modes: Modes::default(),
            tabs: default_tabs(cols),
            charsets: [Charset::Ascii; 2],
            gl: 0,
            last_char: None,
            title: String::new(),
            cwd: None,
            cursor_style: 0,
            cell_px: (0, 0),
            replies: Vec::new(),
            events: Vec::new(),
            images: Store::new(DEFAULT_BUDGET),
            sixel: None,
            sixel_overflow: false,
            dcs: None,
            virtual_images: HashMap::new(),
        }
    }

    pub fn is_alt(&self) -> bool {
        self.main.is_some()
    }

    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub(super) fn reply(&mut self, bytes: &[u8]) {
        // A program that asks and never reads would otherwise grow this without end.
        if self.replies.len() + bytes.len() <= MAX_REPLIES {
            self.replies.extend_from_slice(bytes);
        }
    }

    fn erased_row(&self) -> Row {
        Row::new(self.cols, Cell::erased(self.pen))
    }

    // ---- printing ----

    fn translate(&self, c: char) -> char {
        if self.charsets[self.gl] != Charset::DecGraphics {
            return c;
        }
        match c {
            '`' => '◆',
            'a' => '▒',
            'f' => '°',
            'g' => '±',
            'j' => '┘',
            'k' => '┐',
            'l' => '┌',
            'm' => '└',
            'n' => '┼',
            'o' => '⎺',
            'p' => '⎻',
            'q' => '─',
            'r' => '⎼',
            's' => '⎽',
            't' => '├',
            'u' => '┤',
            'v' => '┴',
            'w' => '┬',
            'x' => '│',
            'y' => '≤',
            'z' => '≥',
            '{' => 'π',
            '|' => '≠',
            '}' => '£',
            '~' => '·',
            other => other,
        }
    }

    pub fn put(&mut self, c: char) {
        let c = self.translate(c);
        if clusters() && self.join_cluster(c) {
            return;
        }
        let width = match c.width() {
            Some(w) if w > 0 => w,
            Some(_) => return self.attach_mark(c),
            None => return,
        };
        if self.wrap_pending {
            if self.modes.autowrap {
                self.wrap();
            } else {
                self.wrap_pending = false;
            }
        }
        if width == 2 && self.cx + 1 >= self.cols {
            if self.cols < 2 {
                return;
            }
            if self.modes.autowrap {
                let (cx, cy) = (self.cx, self.cy);
                self.screen[cy].cells[cx] = Cell::erased(self.pen);
                self.wrap();
            } else {
                self.cx = self.cols - 2;
            }
        }
        if self.modes.insert {
            self.insert_chars(width);
        }
        let (cx, cy, pen) = (self.cx, self.cy, self.pen);
        self.clear_wide_overlap(cy, cx);
        if width == 2 {
            self.clear_wide_overlap(cy, cx + 1);
        }
        let row = &mut self.screen[cy];
        row.clipped.clear();
        row.cells[cx] = Cell { ch: c, width: width as u8, style: pen, tile: 0, mark: 0 };
        if width == 2 {
            row.cells[cx + 1] = Cell { ch: ' ', width: 0, style: pen, tile: 0, mark: 0 };
        }
        self.last_char = Some(c);
        if c == PLACEHOLDER {
            self.resolve_placeholder(cy, cx);
        }
        if cx + width >= self.cols {
            self.cx = self.cols - 1;
            self.wrap_pending = true;
        } else {
            self.cx = cx + width;
        }
    }

    /// A zero-width character (a combining accent, a variation selector, a joiner) goes with the
    /// cell printed before it: the one under the cursor while a wrap is pending, else the one to
    /// its left, and for a wide character its lead half. At the start of a line, or after an image
    /// cell, there is nothing to attach it to and it is dropped.
    fn attach_mark(&mut self, c: char) {
        // Overrides, embeddings, isolates and tag characters are width 0 and would ride along with
        // a cell to the outer terminal, reordering or hiding text around them (review P-08).
        if matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{E0000}'..='\u{E007F}') {
            return;
        }
        let Some(x) = self.prev_cell() else { return };
        let cluster = clusters();
        let cell = &mut self.screen[self.cy].cells[x];
        let placeholder = cell.ch == PLACEHOLDER;
        if (cell.tile != 0 && !placeholder) || cell.width == 0 {
            return;
        }
        let cap = if cluster { super::marks::PER_CLUSTER } else { super::marks::PER_CELL };
        cell.mark = super::marks::add_up_to(cell.mark, c, cap);
        if placeholder {
            return self.resolve_placeholder(self.cy, x);
        }
        // An emoji presentation selector makes a narrow character as wide as an emoji.
        if cluster && c == '\u{fe0f}' && cell.width == 1 {
            self.widen(x);
        }
    }

    /// The cell a zero-width character goes with: the one under the cursor while a wrap is
    /// pending, else the one to its left, for a wide character its lead half.
    fn prev_cell(&self) -> Option<usize> {
        let mut x = if self.wrap_pending { self.cx } else { self.cx.checked_sub(1)? };
        if self.screen[self.cy].cells[x].width == 0 && x > 0 {
            x -= 1;
        }
        Some(x)
    }

    /// `emoji-width cluster`: a character that a terminal draws as part of the cell before it
    /// joins that cell instead of taking one of its own: the character after a joiner, the second
    /// letter of a flag, a skin tone. Returns whether `c` was taken.
    fn join_cluster(&mut self, c: char) -> bool {
        let Some(x) = self.prev_cell() else { return false };
        let cell = self.screen[self.cy].cells[x];
        if cell.tile != 0 || cell.width == 0 {
            return false;
        }
        let after_joiner = super::marks::last(cell.mark) == Some('\u{200d}') && !c.is_ascii() && c.width().is_some_and(|w| w > 0);
        let flag = is_regional(c) && is_regional(cell.ch) && cell.mark == 0;
        let tone = ('\u{1f3fb}'..='\u{1f3ff}').contains(&c) && !cell.ch.is_ascii();
        if !(after_joiner || flag || tone) {
            return false;
        }
        self.screen[self.cy].cells[x].mark = super::marks::add_up_to(cell.mark, c, super::marks::PER_CLUSTER);
        if (flag || tone) && cell.width == 1 {
            self.widen(x);
        }
        true
    }

    /// Make the narrow cell `x`, which the cursor is just past, two cells wide.
    fn widen(&mut self, x: usize) {
        if self.wrap_pending || self.cx != x + 1 {
            return;
        }
        let (cy, pen) = (self.cy, self.pen);
        self.clear_wide_overlap(cy, x + 1);
        let row = &mut self.screen[cy];
        row.cells[x].width = 2;
        row.cells[x + 1] = Cell { ch: ' ', width: 0, style: pen, tile: 0, mark: 0 };
        if x + 2 >= self.cols {
            self.cx = self.cols - 1;
            self.wrap_pending = true;
        } else {
            self.cx = x + 2;
        }
    }

    /// Overwriting half of a wide char must blank the other half.
    pub(super) fn clear_wide_overlap(&mut self, y: usize, x: usize) {
        let pen = self.pen;
        let row = &mut self.screen[y];
        match row.cells[x].width {
            0 if x > 0 => row.cells[x - 1] = Cell::erased(pen),
            2 if x + 1 < row.cells.len() => row.cells[x + 1] = Cell::erased(pen),
            _ => {}
        }
    }

    fn wrap(&mut self) {
        let cy = self.cy;
        self.screen[cy].wrapped = true;
        self.cx = 0;
        self.wrap_pending = false;
        self.linefeed();
    }

    // ---- C0 / cursor movement ----

    pub fn linefeed(&mut self) {
        self.wrap_pending = false;
        if self.cy == self.bottom {
            self.scroll_up(1);
        } else if self.cy + 1 < self.rows {
            self.cy += 1;
        }
    }

    pub fn newline(&mut self) {
        self.linefeed();
        if self.modes.newline {
            self.cx = 0;
        }
    }

    pub fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.cy == self.top {
            self.scroll_down(1);
        } else if self.cy > 0 {
            self.cy -= 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cx = 0;
        self.wrap_pending = false;
    }

    pub fn backspace(&mut self) {
        self.cx = self.cx.saturating_sub(1);
        self.wrap_pending = false;
    }

    pub fn tab(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cx + 1;
            while x < self.cols - 1 && !self.tabs[x] {
                x += 1;
            }
            self.cx = x.min(self.cols - 1);
        }
        self.wrap_pending = false;
    }

    pub fn back_tab(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cx.saturating_sub(1);
            while x > 0 && !self.tabs[x] {
                x -= 1;
            }
            self.cx = x;
        }
        self.wrap_pending = false;
    }

    pub fn set_tab(&mut self) {
        self.tabs[self.cx] = true;
    }

    pub fn clear_tab(&mut self, all: bool) {
        if all {
            self.tabs.iter_mut().for_each(|t| *t = false);
        } else {
            self.tabs[self.cx] = false;
        }
    }

    /// 1-based CUP, honoring origin mode.
    pub fn cup(&mut self, row1: usize, col1: usize) {
        let (row, col) = (row1.max(1) - 1, col1.max(1) - 1);
        let (lo, hi) = if self.modes.origin { (self.top, self.bottom) } else { (0, self.rows - 1) };
        self.cy = (lo + row).min(hi);
        self.cx = col.min(self.cols - 1);
        self.wrap_pending = false;
    }

    pub fn move_x(&mut self, col0: usize) {
        self.cx = col0.min(self.cols - 1);
        self.wrap_pending = false;
    }

    pub fn move_y_abs(&mut self, row1: usize) {
        let (lo, hi) = if self.modes.origin { (self.top, self.bottom) } else { (0, self.rows - 1) };
        self.cy = (lo + row1.max(1) - 1).min(hi);
        self.wrap_pending = false;
    }

    /// Relative vertical move; stops at the scroll-region edge when starting inside it.
    pub fn move_y_rel(&mut self, delta: isize) {
        let (lo, hi) = if delta < 0 && self.cy >= self.top {
            (self.top, self.rows - 1)
        } else if delta > 0 && self.cy <= self.bottom {
            (0, self.bottom)
        } else {
            (0, self.rows - 1)
        };
        let target = self.cy as isize + delta;
        self.cy = target.clamp(lo as isize, hi as isize) as usize;
        self.wrap_pending = false;
    }

    pub fn move_x_rel(&mut self, delta: isize) {
        let target = self.cx as isize + delta;
        self.cx = target.clamp(0, self.cols as isize - 1) as usize;
        self.wrap_pending = false;
    }

    // ---- scrolling ----

    pub fn set_scroll_region(&mut self, top1: usize, bottom1: usize) {
        let top = top1.max(1) - 1;
        let bottom = if bottom1 == 0 { self.rows - 1 } else { (bottom1 - 1).min(self.rows - 1) };
        if top >= bottom {
            return;
        }
        self.top = top;
        self.bottom = bottom;
        self.cup(1, 1);
    }

    pub fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        let full = self.top == 0 && self.bottom == self.rows - 1;
        for _ in 0..n {
            let blank = self.erased_row();
            let mut gone = self.screen.remove(self.top);
            self.screen.insert(self.bottom, blank);
            if full && !self.is_alt() && self.history_limit > 0 {
                gone.trim();
                self.scrollback.push_back(gone);
                if self.scrollback.len() > self.history_limit {
                    self.scrollback.pop_front();
                }
            }
        }
    }

    pub fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        for _ in 0..n {
            let blank = self.erased_row();
            self.screen.remove(self.bottom);
            self.screen.insert(self.top, blank);
        }
    }

    pub fn insert_lines(&mut self, n: usize) {
        if self.cy < self.top || self.cy > self.bottom {
            return;
        }
        let n = n.min(self.bottom - self.cy + 1);
        for _ in 0..n {
            let blank = self.erased_row();
            self.screen.remove(self.bottom);
            self.screen.insert(self.cy, blank);
        }
        self.cx = 0;
        self.wrap_pending = false;
    }

    pub fn delete_lines(&mut self, n: usize) {
        if self.cy < self.top || self.cy > self.bottom {
            return;
        }
        let n = n.min(self.bottom - self.cy + 1);
        for _ in 0..n {
            let blank = self.erased_row();
            self.screen.remove(self.cy);
            self.screen.insert(self.bottom, blank);
        }
        self.cx = 0;
        self.wrap_pending = false;
    }

    // ---- character editing ----

    pub fn insert_chars(&mut self, n: usize) {
        let (cx, cy, cols, pen) = (self.cx, self.cy, self.cols, self.pen);
        let n = n.min(cols - cx);
        self.clear_wide_overlap(cy, cx);
        let row = &mut self.screen[cy];
        row.cells.truncate(cols - n);
        for _ in 0..n {
            row.cells.insert(cx, Cell::erased(pen));
        }
        self.fix_row_end(cy);
        self.wrap_pending = false;
    }

    pub fn delete_chars(&mut self, n: usize) {
        let (cx, cy, cols, pen) = (self.cx, self.cy, self.cols, self.pen);
        let n = n.min(cols - cx);
        self.clear_wide_overlap(cy, cx);
        let row = &mut self.screen[cy];
        row.cells.drain(cx..cx + n);
        row.cells.resize(cols, Cell::erased(pen));
        if row.cells[cx].width == 0 && cx > 0 {
            row.cells[cx] = Cell::erased(pen);
        }
        self.wrap_pending = false;
    }

    pub fn erase_chars(&mut self, n: usize) {
        let (cx, cy, cols, pen) = (self.cx, self.cy, self.cols, self.pen);
        let end = (cx + n).min(cols);
        self.clear_wide_overlap(cy, cx);
        if end < cols {
            self.clear_wide_overlap(cy, end);
        }
        for cell in &mut self.screen[cy].cells[cx..end] {
            *cell = Cell::erased(pen);
        }
        self.wrap_pending = false;
    }

    /// A wide lead pushed onto the last column has lost its trailing half.
    fn fix_row_end(&mut self, y: usize) {
        let pen = self.pen;
        if let Some(last) = self.screen[y].cells.last_mut()
            && last.width == 2
        {
            *last = Cell::erased(pen);
        }
    }

    pub fn erase_line(&mut self, mode: usize) {
        let (cx, cy, cols, pen) = (self.cx, self.cy, self.cols, self.pen);
        let (from, to) = match mode {
            0 => (cx, cols),
            1 => (0, (cx + 1).min(cols)),
            _ => (0, cols),
        };
        self.clear_wide_overlap(cy, from);
        if to < cols {
            self.clear_wide_overlap(cy, to);
        }
        let row = &mut self.screen[cy];
        for cell in &mut row.cells[from..to] {
            *cell = Cell::erased(pen);
        }
        if to == cols {
            row.wrapped = false;
            row.clipped.clear();
        }
        self.wrap_pending = false;
    }

    pub fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.erase_line(0);
                for y in self.cy + 1..self.rows {
                    self.screen[y] = self.erased_row();
                }
            }
            1 => {
                for y in 0..self.cy {
                    self.screen[y] = self.erased_row();
                }
                self.erase_line(1);
            }
            2 => {
                for y in 0..self.rows {
                    self.screen[y] = self.erased_row();
                }
            }
            3 => self.scrollback.clear(),
            _ => {}
        }
        self.wrap_pending = false;
    }

    pub fn repeat_last(&mut self, n: usize) {
        if let Some(c) = self.last_char {
            // One screenful at most: 8 bytes of `CSI 65535 b` otherwise cost 65 535 puts each, and
            // a read turn of such sequences held the server for seconds (security review P-07).
            for _ in 0..n.min(65_535).min(self.cols.saturating_mul(self.rows)) {
                self.put(c);
            }
        }
    }

    // ---- cursor save/restore, alt screen, reset ----

    pub fn save_cursor(&mut self) {
        self.saved = SavedCursor {
            cx: self.cx,
            cy: self.cy,
            pen: self.pen,
            wrap_pending: self.wrap_pending,
            origin: self.modes.origin,
            charsets: self.charsets,
            gl: self.gl,
        };
    }

    pub fn restore_cursor(&mut self) {
        let s = self.saved;
        self.cx = s.cx.min(self.cols - 1);
        self.cy = s.cy.min(self.rows - 1);
        self.pen = s.pen;
        self.wrap_pending = s.wrap_pending;
        self.modes.origin = s.origin;
        self.charsets = s.charsets;
        self.gl = s.gl;
    }

    pub fn enter_alt(&mut self, save_cursor: bool) {
        if self.is_alt() {
            return;
        }
        if save_cursor {
            self.save_cursor();
        }
        let blank: Vec<Row> = (0..self.rows).map(|_| Row::new(self.cols, Cell::BLANK)).collect();
        let screen = std::mem::replace(&mut self.screen, blank);
        self.main = Some(MainScreen { screen, cx: self.cx, cy: self.cy, wrap_pending: self.wrap_pending });
        self.top = 0;
        self.bottom = self.rows - 1;
    }

    pub fn leave_alt(&mut self, restore_cursor: bool) {
        let Some(main) = self.main.take() else { return };
        self.screen = main.screen;
        self.cx = main.cx.min(self.cols - 1);
        self.cy = main.cy.min(self.rows - 1);
        self.wrap_pending = main.wrap_pending;
        self.top = 0;
        self.bottom = self.rows - 1;
        if restore_cursor {
            self.restore_cursor();
        }
    }

    pub fn set_charset(&mut self, slot: usize, cs: Charset) {
        self.charsets[slot & 1] = cs;
    }

    pub fn shift(&mut self, gl: usize) {
        self.gl = gl & 1;
    }

    /// RIS: full reset, keeping the size and history limit.
    pub fn reset(&mut self) {
        let px = self.cell_px;
        let mut fresh = Grid::new(self.cols, self.rows, self.history_limit);
        fresh.cell_px = px;
        fresh.replies = std::mem::take(&mut self.replies);
        fresh.events = std::mem::take(&mut self.events);
        *self = fresh;
    }

    // ---- SGR ----

    pub fn sgr(&mut self, groups: &[&[u16]]) {
        let mut i = 0;
        if groups.is_empty() {
            self.pen = Style::default();
            return;
        }
        while i < groups.len() {
            let g = groups[i];
            let code = g.first().copied().unwrap_or(0);
            match code {
                0 => self.pen = Style::default(),
                1 => self.pen.attrs |= BOLD,
                2 => self.pen.attrs |= DIM,
                3 => self.pen.attrs |= ITALIC,
                4 => {
                    if g.get(1) == Some(&0) {
                        self.pen.attrs &= !UNDERLINE;
                    } else {
                        self.pen.attrs |= UNDERLINE;
                    }
                }
                5 | 6 => self.pen.attrs |= BLINK,
                7 => self.pen.attrs |= REVERSE,
                8 => self.pen.attrs |= HIDDEN,
                9 => self.pen.attrs |= STRIKE,
                21 => self.pen.attrs |= UNDERLINE,
                22 => self.pen.attrs &= !(BOLD | DIM),
                23 => self.pen.attrs &= !ITALIC,
                24 => self.pen.attrs &= !UNDERLINE,
                25 => self.pen.attrs &= !BLINK,
                27 => self.pen.attrs &= !REVERSE,
                28 => self.pen.attrs &= !HIDDEN,
                29 => self.pen.attrs &= !STRIKE,
                30..=37 => self.pen.fg = Color::Indexed((code - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((code - 40) as u8),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 => {
                    let (color, used) = parse_extended_color(groups, i);
                    if let Some(c) = color {
                        if code == 38 {
                            self.pen.fg = c;
                        } else {
                            self.pen.bg = c;
                        }
                    }
                    i += used;
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Returns the color and how many extra groups (semicolon form) were consumed.
fn parse_extended_color(groups: &[&[u16]], i: usize) -> (Option<Color>, usize) {
    let g = groups[i];
    if g.len() > 1 {
        let rest = &g[1..];
        return (colon_color(rest), 0);
    }
    let next = |k: usize| groups.get(i + k).and_then(|g| g.first().copied());
    match next(1) {
        Some(5) => (next(2).map(|n| Color::Indexed(n as u8)), 2),
        Some(2) => match (next(2), next(3), next(4)) {
            (Some(r), Some(g), Some(b)) => (Some(Color::Rgb(r as u8, g as u8, b as u8)), 4),
            _ => (None, groups.len()),
        },
        _ => (None, 0),
    }
}

/// `38:5:n`, `38:2:r:g:b` and `38:2::r:g:b` (empty color-space id).
fn colon_color(rest: &[u16]) -> Option<Color> {
    match rest.first()? {
        5 => rest.get(1).map(|&n| Color::Indexed(n as u8)),
        2 => {
            let rgb = if rest.len() >= 5 { &rest[2..5] } else { rest.get(1..4)? };
            Some(Color::Rgb(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8))
        }
        _ => None,
    }
}
