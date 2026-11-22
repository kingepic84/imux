//! Copy mode: browsing a pane's scrollback with a cursor, selecting text and copying it.
//!
//! Positions are `(line, column)` into one virtual buffer, the scrollback followed by the screen,
//! so they stay valid while the pane keeps producing output. On the alternate screen the buffer
//! is just that screen. Everything here is pure; the server owns the clipboard and the input.

use std::collections::HashMap;
use std::rc::Rc;

use crate::input::keys::{CTRL, Code, Key};
#[cfg(test)]
use crate::term::cell::Color;
use crate::term::cell::{Cell, Style};
use crate::term::grid::Grid;

/// The default `mode-style`, for the tests.
#[cfg(test)]
const MODE_STYLE: Style = Style { fg: Color::Rgb(30, 30, 46), bg: Color::Rgb(249, 226, 175), attrs: 0 };
/// Lines moved by one mouse wheel notch.
pub const WHEEL_LINES: usize = 5;

pub type Pos = (usize, usize);

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Leave copy mode.
    Cancel,
    /// Put this text in the paste buffer and clipboard, then leave copy mode.
    Copy(String),
}

/// What a key does in copy mode. The names are the ones `send-keys -X` takes in `bind -T
/// copy-mode[-vi]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Cancel,
    /// Copy the selection into a paste buffer and leave.
    CopyAndCancel,
    /// Select from the cursor to the end of the line, copy it and leave.
    CopyEndOfLine,
    BeginSelection,
    /// imux's `v`: start a selection, or drop the one there is.
    ToggleSelection,
    ClearSelection,
    SelectLine,
    /// Switch the selection between running text and a block of columns.
    RectangleToggle,
    /// Move the cursor to the other end of the selection.
    OtherEnd,
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    StartOfLine,
    EndOfLine,
    BackToIndentation,
    NextWord,
    NextSpace,
    PreviousWord,
    PreviousSpace,
    NextWordEnd,
    NextSpaceEnd,
    HistoryTop,
    HistoryBottom,
    TopLine,
    MiddleLine,
    BottomLine,
    HalfpageUp,
    HalfpageDown,
    PageUp,
    PageDown,
    ScrollUp,
    ScrollDown,
    SearchForward,
    SearchBackward,
    SearchAgain,
    SearchReverse,
}

/// Every action with its name; the first name of an action is the one `list-keys` prints, later
/// ones are accepted spellings.
const OPS: &[(&str, Op)] = &[
    ("cancel", Op::Cancel),
    ("copy-selection-and-cancel", Op::CopyAndCancel),
    ("copy-pipe-and-cancel", Op::CopyAndCancel),
    ("copy-selection", Op::CopyAndCancel),
    ("copy-pipe", Op::CopyAndCancel),
    ("copy-end-of-line", Op::CopyEndOfLine),
    ("copy-end-of-line-and-cancel", Op::CopyEndOfLine),
    ("begin-selection", Op::BeginSelection),
    ("toggle-selection", Op::ToggleSelection),
    ("clear-selection", Op::ClearSelection),
    ("select-line", Op::SelectLine),
    ("rectangle-toggle", Op::RectangleToggle),
    ("other-end", Op::OtherEnd),
    ("cursor-left", Op::CursorLeft),
    ("cursor-right", Op::CursorRight),
    ("cursor-up", Op::CursorUp),
    ("cursor-down", Op::CursorDown),
    ("start-of-line", Op::StartOfLine),
    ("end-of-line", Op::EndOfLine),
    ("back-to-indentation", Op::BackToIndentation),
    ("next-word", Op::NextWord),
    ("next-space", Op::NextSpace),
    ("previous-word", Op::PreviousWord),
    ("previous-space", Op::PreviousSpace),
    ("next-word-end", Op::NextWordEnd),
    ("next-space-end", Op::NextSpaceEnd),
    ("history-top", Op::HistoryTop),
    ("history-bottom", Op::HistoryBottom),
    ("top-line", Op::TopLine),
    ("middle-line", Op::MiddleLine),
    ("bottom-line", Op::BottomLine),
    ("halfpage-up", Op::HalfpageUp),
    ("halfpage-down", Op::HalfpageDown),
    ("page-up", Op::PageUp),
    ("page-down", Op::PageDown),
    ("scroll-up", Op::ScrollUp),
    ("scroll-down", Op::ScrollDown),
    ("search-forward", Op::SearchForward),
    ("search-backward", Op::SearchBackward),
    ("search-again", Op::SearchAgain),
    ("search-reverse", Op::SearchReverse),
];

impl Op {
    pub fn parse(name: &str) -> Option<Op> {
        OPS.iter().find(|(n, _)| *n == name).map(|(_, op)| *op)
    }

    pub fn name(self) -> &'static str {
        OPS.iter().find(|(_, op)| *op == self).map_or("cancel", |(n, _)| n)
    }
}

/// The keys of one copy mode table (`copy-mode` for emacs keys, `copy-mode-vi`).
#[derive(Clone, Default)]
pub struct CopyKeys {
    map: HashMap<Key, Op>,
}

const VI_KEYS: &[(&str, Op)] = &[
    ("Escape", Op::Cancel),
    ("q", Op::Cancel),
    ("c", Op::Cancel),
    ("C-q", Op::Cancel),
    ("C-c", Op::Cancel),
    ("y", Op::CopyAndCancel),
    ("Enter", Op::CopyAndCancel),
    ("D", Op::CopyEndOfLine),
    ("v", Op::ToggleSelection),
    ("Space", Op::ToggleSelection),
    ("V", Op::SelectLine),
    ("C-v", Op::RectangleToggle),
    ("o", Op::OtherEnd),
    ("h", Op::CursorLeft),
    ("Left", Op::CursorLeft),
    ("l", Op::CursorRight),
    ("Right", Op::CursorRight),
    ("k", Op::CursorUp),
    ("Up", Op::CursorUp),
    ("j", Op::CursorDown),
    ("Down", Op::CursorDown),
    ("0", Op::StartOfLine),
    ("Home", Op::StartOfLine),
    ("$", Op::EndOfLine),
    ("End", Op::EndOfLine),
    ("^", Op::BackToIndentation),
    ("w", Op::NextWord),
    ("W", Op::NextSpace),
    ("b", Op::PreviousWord),
    ("B", Op::PreviousSpace),
    ("e", Op::NextWordEnd),
    ("E", Op::NextSpaceEnd),
    ("g", Op::HistoryTop),
    ("G", Op::HistoryBottom),
    ("H", Op::TopLine),
    ("M", Op::MiddleLine),
    ("L", Op::BottomLine),
    ("C-u", Op::HalfpageUp),
    ("C-d", Op::HalfpageDown),
    ("C-b", Op::PageUp),
    ("PageUp", Op::PageUp),
    ("C-f", Op::PageDown),
    ("PageDown", Op::PageDown),
    ("C-y", Op::ScrollUp),
    ("C-e", Op::ScrollDown),
    ("/", Op::SearchForward),
    ("?", Op::SearchBackward),
    ("n", Op::SearchAgain),
    ("N", Op::SearchReverse),
];

const EMACS_KEYS: &[(&str, Op)] = &[
    ("Escape", Op::Cancel),
    ("q", Op::Cancel),
    ("C-g", Op::Cancel),
    ("C-Space", Op::BeginSelection),
    ("C-w", Op::CopyAndCancel),
    ("M-w", Op::CopyAndCancel),
    ("C-k", Op::CopyEndOfLine),
    ("R", Op::RectangleToggle),
    ("C-b", Op::CursorLeft),
    ("Left", Op::CursorLeft),
    ("C-f", Op::CursorRight),
    ("Right", Op::CursorRight),
    ("C-p", Op::CursorUp),
    ("Up", Op::CursorUp),
    ("C-n", Op::CursorDown),
    ("Down", Op::CursorDown),
    ("C-a", Op::StartOfLine),
    ("Home", Op::StartOfLine),
    ("C-e", Op::EndOfLine),
    ("End", Op::EndOfLine),
    ("M-m", Op::BackToIndentation),
    ("M-b", Op::PreviousWord),
    ("M-f", Op::NextWordEnd),
    ("M-<", Op::HistoryTop),
    ("M->", Op::HistoryBottom),
    ("M-r", Op::MiddleLine),
    ("M-v", Op::PageUp),
    ("PageUp", Op::PageUp),
    ("C-v", Op::PageDown),
    ("PageDown", Op::PageDown),
    ("M-Up", Op::HalfpageUp),
    ("M-Down", Op::HalfpageDown),
    ("C-Up", Op::ScrollUp),
    ("C-Down", Op::ScrollDown),
    ("C-s", Op::SearchForward),
    ("C-r", Op::SearchBackward),
    ("n", Op::SearchAgain),
    ("N", Op::SearchReverse),
];

/// The `mode-keys` value when the configuration does not say: `vi` when the editor in `$VISUAL`
/// (or else `$EDITOR`) has `vi` in its name, otherwise `emacs`.
pub fn default_mode_keys(visual: Option<&str>, editor: Option<&str>) -> &'static str {
    let name = visual.or(editor).map_or("", |path| path.rsplit('/').next().unwrap_or(path));
    if name.contains("vi") { "vi" } else { "emacs" }
}

impl CopyKeys {
    fn build(list: &[(&str, Op)]) -> CopyKeys {
        let map = list.iter().map(|(k, op)| (Key::parse(k).unwrap_or_else(|| panic!("bad key {k} in the copy mode keys")), *op)).collect();
        CopyKeys { map }
    }

    /// The vi table.
    pub fn vi() -> CopyKeys {
        CopyKeys::build(VI_KEYS)
    }

    /// The emacs table.
    pub fn emacs() -> CopyKeys {
        CopyKeys::build(EMACS_KEYS)
    }

    pub fn get(&self, key: &Key) -> Option<Op> {
        self.map.get(key).copied()
    }

    pub fn bind(&mut self, key: Key, op: Op) {
        self.map.insert(key, op);
    }

    /// False when the key was not bound.
    pub fn unbind(&mut self, key: &Key) -> bool {
        self.map.remove(key).is_some()
    }

    /// Key name and action name of every binding, by key name.
    pub fn list(&self) -> Vec<(String, &'static str)> {
        let mut rows: Vec<(String, &'static str)> = self.map.iter().map(|(k, op)| (k.name(), op.name())).collect();
        rows.sort();
        rows
    }
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: Pos,
    lines: bool,
    /// A block of columns instead of running text.
    rect: bool,
}

struct Search {
    forward: bool,
    text: String,
}

pub struct CopyMode {
    keys: Rc<CopyKeys>,
    /// First line of the buffer shown.
    top: usize,
    cur: Pos,
    sel: Option<Selection>,
    /// Leave on scrolling back to the bottom (entered with the wheel).
    pub exit_at_bottom: bool,
    /// Leave out the `[n/m]` position at the top right (`copy-mode -H`).
    pub hide_indicator: bool,
    /// Where the mouse went down; a drag selects from here.
    press: Option<Pos>,
    search: Option<Search>,
    last_search: Option<(String, bool)>,
    /// `word-separators`: the characters that end a word; empty keeps the built-in rule (letters,
    /// digits and `_` make a word, other characters are punctuation). Set by the server before it
    /// hands the mode a key or a click.
    pub separators: String,
}

/// What the pane shows while copy mode is on.
pub struct View {
    pub cells: Vec<Cell>,
    pub cursor: Option<(usize, usize)>,
}

fn base(g: &Grid) -> usize {
    if g.is_alt() { 0 } else { g.scrollback.len() }
}

fn count(g: &Grid) -> usize {
    base(g) + g.rows
}

fn row(g: &Grid, line: usize) -> Option<&crate::term::cell::Row> {
    let b = base(g);
    if line < b { g.scrollback.get(line) } else { g.screen.get(line - b) }
}

fn cell(g: &Grid, line: usize, col: usize) -> Cell {
    row(g, line).and_then(|r| r.cells.get(col).copied()).unwrap_or(Cell::BLANK)
}

fn wrapped(g: &Grid, line: usize) -> bool {
    row(g, line).is_some_and(|r| r.wrapped)
}

fn ch(g: &Grid, line: usize, col: usize) -> char {
    let c = cell(g, line, col);
    if c.width == 0 && col > 0 { cell(g, line, col - 1).ch } else { c.ch }
}

/// Column after the last non-blank cell.
fn text_len(g: &Grid, line: usize) -> usize {
    (0..g.cols).rev().find(|&c| !cell(g, line, c).is_blank() && cell(g, line, c).ch != ' ').map_or(0, |c| c + 1)
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

/// The kind of a character for word motions. With `separators` set, the characters in it are
/// punctuation and every other character that is not a blank belongs to a word (as `word-separators`
/// does elsewhere); a capital-letter motion (`big`) only stops at blanks.
fn word_class(c: char, big: bool, separators: &str) -> Class {
    if c == ' ' || c == '\0' {
        Class::Space
    } else if big {
        Class::Word
    } else if separators.is_empty() {
        if c.is_alphanumeric() || c == '_' { Class::Word } else { Class::Punct }
    } else if separators.contains(c) {
        Class::Punct
    } else {
        Class::Word
    }
}

fn next(g: &Grid, p: Pos) -> Option<Pos> {
    if p.1 + 1 < g.cols {
        Some((p.0, p.1 + 1))
    } else if p.0 + 1 < count(g) {
        Some((p.0 + 1, 0))
    } else {
        None
    }
}

fn prev(g: &Grid, p: Pos) -> Option<Pos> {
    if p.1 > 0 {
        Some((p.0, p.1 - 1))
    } else if p.0 > 0 {
        Some((p.0 - 1, g.cols - 1))
    } else {
        None
    }
}

fn ordered(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b { (a, b) } else { (b, a) }
}

impl CopyMode {
    /// Copy mode with the vi keys.
    #[cfg(test)]
    pub fn enter(g: &Grid) -> CopyMode {
        CopyMode::enter_with(g, Rc::new(CopyKeys::vi()))
    }

    /// Copy mode reading its keys from `keys`.
    pub fn enter_with(g: &Grid, keys: Rc<CopyKeys>) -> CopyMode {
        CopyMode {
            keys,
            top: count(g).saturating_sub(g.rows),
            cur: (base(g) + g.cy, g.cx),
            sel: None,
            exit_at_bottom: false,
            hide_indicator: false,
            press: None,
            search: None,
            last_search: None,
            separators: String::new(),
        }
    }

    fn max_top(g: &Grid) -> usize {
        count(g).saturating_sub(g.rows)
    }

    pub fn at_bottom(&self, g: &Grid) -> bool {
        self.top >= Self::max_top(g)
    }

    /// Keep the view and the cursor inside the buffer and the cursor inside the view.
    fn fit(&mut self, g: &Grid) {
        let last = count(g) - 1;
        self.cur.0 = self.cur.0.min(last);
        self.cur.1 = self.cur.1.min(g.cols - 1);
        self.top = self.top.min(Self::max_top(g));
        if self.cur.0 < self.top {
            self.top = self.cur.0;
        } else if self.cur.0 >= self.top + g.rows {
            self.top = self.cur.0 + 1 - g.rows;
        }
    }

    /// Scroll the view; the cursor stays in view.
    pub fn scroll(&mut self, g: &Grid, delta: isize) {
        let top = self.top.saturating_add_signed(delta).min(Self::max_top(g));
        self.top = top;
        self.cur.0 = self.cur.0.clamp(top, top + g.rows - 1);
        self.fit(g);
    }

    /// One page back, as the page-up key does (`copy-mode -u`).
    pub fn page_up(&mut self, g: &Grid) {
        self.scroll(g, -(g.rows.saturating_sub(1).max(1) as isize));
    }

    /// Position under a pane-relative cell.
    pub fn pos_at(&self, g: &Grid, x: usize, y: usize) -> Pos {
        ((self.top + y).min(count(g) - 1), x.min(g.cols - 1))
    }

    fn goto(&mut self, g: &Grid, p: Pos) {
        self.cur = p;
        if cell(g, p.0, p.1).width == 0 && p.1 > 0 {
            self.cur.1 -= 1;
        }
        self.fit(g);
    }

    fn word_forward(&self, g: &Grid, big: bool) -> Pos {
        let class = |c: char, big: bool| word_class(c, big, &self.separators);
        let mut p = self.cur;
        let start = class(ch(g, p.0, p.1), big);
        if start != Class::Space {
            while class(ch(g, p.0, p.1), big) == start {
                match next(g, p) {
                    Some(n) => p = n,
                    None => return p,
                }
            }
        }
        while class(ch(g, p.0, p.1), big) == Class::Space {
            match next(g, p) {
                Some(n) => p = n,
                None => return p,
            }
        }
        p
    }

    fn word_back(&self, g: &Grid, big: bool) -> Pos {
        let class = |c: char, big: bool| word_class(c, big, &self.separators);
        let Some(mut p) = prev(g, self.cur) else { return self.cur };
        while class(ch(g, p.0, p.1), big) == Class::Space {
            match prev(g, p) {
                Some(n) => p = n,
                None => return p,
            }
        }
        let cls = class(ch(g, p.0, p.1), big);
        while let Some(n) = prev(g, p) {
            if class(ch(g, n.0, n.1), big) != cls {
                break;
            }
            p = n;
        }
        p
    }

    fn word_end(&self, g: &Grid, big: bool) -> Pos {
        let class = |c: char, big: bool| word_class(c, big, &self.separators);
        let Some(mut p) = next(g, self.cur) else { return self.cur };
        while class(ch(g, p.0, p.1), big) == Class::Space {
            match next(g, p) {
                Some(n) => p = n,
                None => return p,
            }
        }
        let cls = class(ch(g, p.0, p.1), big);
        while let Some(n) = next(g, p) {
            if class(ch(g, n.0, n.1), big) != cls {
                break;
            }
            p = n;
        }
        p
    }

    fn selected(&self, line: usize, col: usize) -> bool {
        let Some(sel) = self.sel else { return false };
        let (a, b) = ordered(sel.anchor, self.cur);
        if line < a.0 || line > b.0 {
            return false;
        }
        if sel.rect && !sel.lines {
            return col >= sel.anchor.1.min(self.cur.1) && col <= sel.anchor.1.max(self.cur.1);
        }
        sel.lines || ((line > a.0 || col >= a.1) && (line < b.0 || col <= b.1))
    }

    /// The text of a block selection: the same columns of every line, without trailing blanks.
    fn block_text(&self, g: &Grid, sel: Selection) -> String {
        let (a, b) = ordered(sel.anchor, self.cur);
        let (from, to) = (sel.anchor.1.min(self.cur.1), sel.anchor.1.max(self.cur.1));
        let lines: Vec<String> = (a.0..=b.0)
            .map(|line| {
                let s = Cell::text_for_copy((from..=to).map(|c| cell(g, line, c)).collect::<Vec<_>>().iter());
                s.trim_end().to_string()
            })
            .collect();
        lines.join("\n")
    }

    fn text(&self, g: &Grid) -> Option<String> {
        let sel = self.sel?;
        if sel.rect && !sel.lines {
            return Some(self.block_text(g, sel));
        }
        let (a, b) = ordered(sel.anchor, self.cur);
        let mut out = String::new();
        for line in a.0..=b.0 {
            let from = if sel.lines || line > a.0 { 0 } else { a.1 };
            let to = if sel.lines || line < b.0 { g.cols - 1 } else { b.1 };
            let mut s = Cell::text_for_copy((from..=to).map(|c| cell(g, line, c)).collect::<Vec<_>>().iter());
            let joined = wrapped(g, line) && line < b.0;
            if !joined {
                s.truncate(s.trim_end().len());
            }
            out.push_str(&s);
            if line < b.0 && !joined {
                out.push('\n');
            }
        }
        Some(out)
    }

    /// Copy the selection, or leave without copying when there is none.
    fn finish(&self, g: &Grid) -> Action {
        match self.text(g) {
            Some(t) if !t.is_empty() => Action::Copy(t),
            _ => Action::Cancel,
        }
    }

    pub fn select_word(&mut self, g: &Grid, p: Pos) {
        let class = |c: char, big: bool| word_class(c, big, &self.separators);
        let cls = class(ch(g, p.0, p.1), false);
        let (mut a, mut b) = (p.1, p.1);
        while a > 0 && class(ch(g, p.0, a - 1), false) == cls {
            a -= 1;
        }
        while b + 1 < g.cols && class(ch(g, p.0, b + 1), false) == cls {
            b += 1;
        }
        self.sel = Some(Selection { anchor: (p.0, a), lines: false, rect: false });
        self.cur = (p.0, b);
        self.fit(g);
    }

    pub fn select_line(&mut self, g: &Grid, p: Pos) {
        self.sel = Some(Selection { anchor: p, lines: true, rect: false });
        self.cur = p;
        self.fit(g);
    }

    /// Text of a word or line selection made with a double or triple click, for copying.
    pub fn copy_selection(&self, g: &Grid) -> Action {
        self.finish(g)
    }

    pub fn mouse_down(&mut self, g: &Grid, p: Pos) {
        self.press = Some(p);
        self.sel = None;
        self.goto(g, p);
    }

    pub fn mouse_drag(&mut self, g: &Grid, p: Pos) {
        if self.sel.is_none() {
            let anchor = self.press.unwrap_or(self.cur);
            self.sel = Some(Selection { anchor, lines: false, rect: false });
        }
        self.goto(g, p);
    }

    /// Button released: a drag copies what it selected.
    pub fn mouse_up(&mut self, g: &Grid) -> Action {
        self.press = None;
        if self.sel.is_some() { self.finish(g) } else { Action::None }
    }

    fn find(&mut self, g: &Grid, text: &str, forward: bool) -> bool {
        let needle: Vec<char> = text.chars().collect();
        if needle.is_empty() {
            return false;
        }
        let fold = text.chars().all(|c| !c.is_uppercase());
        let lower = |c: char| if fold { c.to_lowercase().next().unwrap_or(c) } else { c };
        let total = count(g);
        for step in 0..=total {
            let line = if forward { (self.cur.0 + step) % total } else { (self.cur.0 + total - step % total) % total };
            let hay: Vec<char> = (0..g.cols).map(|c| lower(ch(g, line, c))).collect();
            let want: Vec<char> = needle.iter().map(|&c| lower(c)).collect();
            let hits: Vec<usize> = (0..hay.len().saturating_sub(want.len() - 1))
                .filter(|&i| hay[i..i + want.len()] == want[..])
                .collect();
            let pick = if forward {
                hits.into_iter().find(|&i| step > 0 || i > self.cur.1)
            } else {
                hits.into_iter().rev().find(|&i| step > 0 || i < self.cur.1)
            };
            if let Some(i) = pick {
                self.goto(g, (line, i));
                return true;
            }
        }
        false
    }

    fn repeat_search(&mut self, g: &Grid, reverse: bool) {
        if let Some((text, forward)) = self.last_search.clone() {
            self.find(g, &text, forward != reverse);
        }
    }

    fn search_key(&mut self, g: &Grid, key: Key) -> Action {
        let Some(s) = self.search.as_mut() else { return Action::None };
        match (key.code, key.mods) {
            (Code::Escape, _) | (Code::Char('c'), CTRL) => self.search = None,
            (Code::Enter, _) => {
                let (text, forward) = (std::mem::take(&mut s.text), s.forward);
                self.search = None;
                self.find(g, &text, forward);
                self.last_search = Some((text, forward));
            }
            (Code::Backspace, _) => {
                s.text.pop();
            }
            (Code::Char('u'), CTRL) => s.text.clear(),
            (Code::Char(c), 0) => s.text.push(c),
            _ => {}
        }
        Action::None
    }

    pub fn key(&mut self, g: &Grid, key: Key) -> Action {
        if self.search.is_some() {
            return self.search_key(g, key);
        }
        match self.keys.get(&key) {
            Some(op) => self.apply(g, op),
            None => Action::None,
        }
    }

    /// Do what `op` says.
    pub fn apply(&mut self, g: &Grid, op: Op) -> Action {
        let half = (g.rows / 2).max(1) as isize;
        let page = g.rows.saturating_sub(1).max(1) as isize;
        let (line, col) = self.cur;
        let here = self.cur;
        match op {
            Op::Cancel => return Action::Cancel,
            Op::CopyAndCancel => return self.finish(g),
            Op::CopyEndOfLine => {
                self.sel = Some(Selection { anchor: here, lines: false, rect: false });
                self.cur = (line, text_len(g, line).saturating_sub(1).max(col));
                return self.finish(g);
            }
            Op::BeginSelection => self.sel = Some(Selection { anchor: here, lines: false, rect: false }),
            Op::ToggleSelection => {
                self.sel = if self.sel.is_some() { None } else { Some(Selection { anchor: here, lines: false, rect: false }) };
            }
            Op::ClearSelection => self.sel = None,
            Op::SelectLine => self.sel = Some(Selection { anchor: here, lines: true, rect: false }),
            Op::RectangleToggle => match self.sel.as_mut() {
                Some(sel) => {
                    sel.rect = !sel.rect;
                    sel.lines = false;
                }
                None => self.sel = Some(Selection { anchor: here, lines: false, rect: true }),
            },
            Op::OtherEnd => {
                if let Some(sel) = self.sel.as_mut() {
                    std::mem::swap(&mut sel.anchor, &mut self.cur);
                    self.fit(g);
                }
            }
            Op::CursorLeft => self.goto(g, (line, col.saturating_sub(1))),
            Op::CursorRight => self.goto(g, (line, col + 1)),
            Op::CursorUp => self.goto(g, (line.saturating_sub(1), col)),
            Op::CursorDown => self.goto(g, (line + 1, col)),
            Op::StartOfLine => self.goto(g, (line, 0)),
            Op::EndOfLine => self.goto(g, (line, text_len(g, line).saturating_sub(1))),
            Op::BackToIndentation => {
                let first = (0..g.cols).find(|&c| ch(g, line, c) != ' ').unwrap_or(0);
                self.goto(g, (line, first));
            }
            Op::NextWord => self.goto(g, self.word_forward(g, false)),
            Op::NextSpace => self.goto(g, self.word_forward(g, true)),
            Op::PreviousWord => self.goto(g, self.word_back(g, false)),
            Op::PreviousSpace => self.goto(g, self.word_back(g, true)),
            Op::NextWordEnd => self.goto(g, self.word_end(g, false)),
            Op::NextSpaceEnd => self.goto(g, self.word_end(g, true)),
            Op::HistoryTop => self.goto(g, (0, 0)),
            Op::HistoryBottom => self.goto(g, (count(g) - 1, 0)),
            Op::TopLine => self.goto(g, (self.top, col)),
            Op::MiddleLine => self.goto(g, (self.top + g.rows / 2, col)),
            Op::BottomLine => self.goto(g, (self.top + g.rows - 1, col)),
            Op::HalfpageUp => self.scroll(g, -half),
            Op::HalfpageDown => self.scroll(g, half),
            Op::PageUp => self.scroll(g, -page),
            Op::PageDown => self.scroll(g, page),
            Op::ScrollUp => self.scroll(g, -1),
            Op::ScrollDown => self.scroll(g, 1),
            Op::SearchForward => self.search = Some(Search { forward: true, text: String::new() }),
            Op::SearchBackward => self.search = Some(Search { forward: false, text: String::new() }),
            Op::SearchAgain => self.repeat_search(g, false),
            Op::SearchReverse => self.repeat_search(g, true),
        }
        if self.exit_at_bottom && self.at_bottom(g) && self.sel.is_none() && op == Op::CursorDown {
            return Action::Cancel;
        }
        Action::None
    }

    /// The pane as copy mode shows it: the viewed lines, selection, position indicator, search prompt.
    #[cfg(test)]
    pub fn view(&self, g: &Grid) -> View {
        self.view_with(g, MODE_STYLE)
    }

    /// Like `view`, with the selection and indicator drawn in `mode_style` (the `mode-style` option).
    pub fn view_with(&self, g: &Grid, mode_style: Style) -> View {
        let mut cells = Vec::with_capacity(g.rows * g.cols);
        for y in 0..g.rows {
            let line = self.top + y;
            for x in 0..g.cols {
                let mut c = if line < count(g) { cell(g, line, x) } else { Cell::BLANK };
                if self.selected(line, x) {
                    c.style = mode_style;
                }
                cells.push(c);
            }
        }
        let indicator = format!("[{}/{}]", count(g) - g.rows - self.top.min(count(g) - g.rows), base(g));
        let mut put = |y: usize, x0: usize, text: &str| {
            for (i, c) in text.chars().enumerate() {
                if x0 + i < g.cols && y < g.rows {
                    cells[y * g.cols + x0 + i] = Cell { ch: c, width: 1, style: mode_style, tile: 0, mark: 0 };
                }
            }
        };
        if !self.hide_indicator {
            put(0, g.cols.saturating_sub(indicator.len()), &indicator);
        }
        if let Some(s) = &self.search {
            let prompt = format!("{}{}", if s.forward { '/' } else { '?' }, s.text);
            put(g.rows - 1, 0, &prompt);
        }
        let cursor = match &self.search {
            Some(s) => Some((s.text.chars().count() + 1, g.rows - 1)),
            None => (self.cur.0 >= self.top && self.cur.0 < self.top + g.rows).then(|| (self.cur.1, self.cur.0 - self.top)),
        };
        View { cells, cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Terminal;

    fn term(lines: &[&str], rows: usize) -> Terminal {
        let mut t = Terminal::new(20, rows, 100);
        t.feed(lines.join("\r\n").as_bytes());
        t
    }

    fn k(name: &str) -> Key {
        Key::parse(name).unwrap()
    }

    fn press(m: &mut CopyMode, g: &Grid, keys: &[&str]) -> Action {
        let mut last = Action::None;
        for name in keys {
            last = m.key(g, k(name));
        }
        last
    }

    fn eight_lines() -> Terminal {
        term(&["one", "two words", "three", "four", "five", "six", "seven", "eight"], 4)
    }

    #[test]
    fn enters_at_the_bottom_with_the_cursor_where_it_was() {
        let t = eight_lines();
        let m = CopyMode::enter(&t.grid);
        assert!(m.at_bottom(&t.grid));
        assert_eq!(m.cur.0, base(&t.grid) + t.grid.cy);
    }

    #[test]
    fn scrolling_up_leaves_the_bottom_and_the_indicator_counts_lines() {
        let t = eight_lines();
        let mut m = CopyMode::enter(&t.grid);
        m.scroll(&t.grid, -3);
        assert!(!m.at_bottom(&t.grid));
        let v = m.view(&t.grid);
        let top: String = v.cells[..20].iter().map(|c| c.ch).collect();
        assert!(top.trim_end().ends_with("[3/4]"), "{top:?}");
    }

    #[test]
    fn vi_motions() {
        let t = term(&["alpha beta", "gamma"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "w"]);
        assert_eq!(m.cur, (0, 6));
        press(&mut m, g, &["e"]);
        assert_eq!(m.cur, (0, 9));
        press(&mut m, g, &["b"]);
        assert_eq!(m.cur, (0, 6));
        press(&mut m, g, &["0", "$"]);
        assert_eq!(m.cur, (0, 9));
        press(&mut m, g, &["j", "0"]);
        assert_eq!(m.cur, (1, 0));
    }

    #[test]
    fn yank_copies_the_selection_across_lines_without_trailing_blanks() {
        let t = term(&["alpha beta", "gamma"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "w", "v", "j", "0", "e"]);
        assert_eq!(m.key(g, k("y")), Action::Copy("beta\ngamma".to_string()));
    }

    #[test]
    fn line_selection_and_wrapped_lines_join() {
        let mut t = Terminal::new(10, 4, 100);
        t.feed(b"0123456789abcde\r\nnext");
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "V", "j"]);
        assert_eq!(m.key(g, k("y")), Action::Copy("0123456789abcde".to_string()));
    }

    #[test]
    fn yank_without_a_selection_just_leaves() {
        let t = eight_lines();
        let mut m = CopyMode::enter(&t.grid);
        assert_eq!(m.key(&t.grid, k("y")), Action::Cancel);
        assert_eq!(m.key(&t.grid, k("Escape")), Action::Cancel);
    }

    #[test]
    fn search_finds_forward_backward_and_wraps() {
        let t = term(&["alpha", "beta", "gamma", "delta"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "/", "a", "m", "Enter"]);
        assert_eq!(m.cur, (2, 1));
        press(&mut m, g, &["n"]);
        assert_eq!(m.cur.0, 2, "only one match wraps to itself");
        press(&mut m, g, &["g", "?", "t", "Enter"]);
        assert_eq!(m.cur, (3, 3));
    }

    #[test]
    fn lowercase_search_ignores_case_and_capitals_do_not() {
        let t = term(&["Alpha", "alpha"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "/", "a", "l", "Enter"]);
        assert_eq!(m.cur, (1, 0), "the search starts after the cursor and ignores case");
        press(&mut m, g, &["g", "/", "A", "l", "Enter"]);
        assert_eq!(m.cur, (0, 0));
        press(&mut m, g, &["j", "0", "/", "A", "l", "Enter"]);
        assert_eq!(m.cur, (0, 0), "wrapped around to the only capitalised match");
    }

    #[test]
    fn mouse_drag_selects_and_release_copies() {
        let t = term(&["hello world"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        let (a, b) = (m.pos_at(g, 6, 0), m.pos_at(g, 10, 0));
        m.mouse_down(g, a);
        m.mouse_drag(g, b);
        assert_eq!(m.mouse_up(g), Action::Copy("world".to_string()));
    }

    #[test]
    fn a_click_without_a_drag_copies_nothing() {
        let t = term(&["hello world"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        let p = m.pos_at(g, 2, 0);
        m.mouse_down(g, p);
        assert_eq!(m.mouse_up(g), Action::None);
    }

    #[test]
    fn double_click_selects_a_word_and_on_blank_copies_nothing() {
        let t = term(&["hello world"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        m.select_word(g, (0, 7));
        assert_eq!(m.copy_selection(g), Action::Copy("world".to_string()));
        let mut blank = CopyMode::enter(g);
        blank.select_word(g, (0, 15));
        assert_eq!(blank.copy_selection(g), Action::Cancel);
    }

    #[test]
    fn word_separators_decide_where_a_word_ends() {
        let t = term(&["a /usr/lo-b:x_y z"], 4);
        let g = &t.grid;
        let mut built_in = CopyMode::enter(g);
        built_in.select_word(g, (0, 4));
        assert_eq!(built_in.copy_selection(g), Action::Copy("usr".to_string()), "letters, digits and _ make a word");
        let mut x_y = CopyMode::enter(g);
        x_y.select_word(g, (0, 14));
        assert_eq!(x_y.copy_selection(g), Action::Copy("x_y".to_string()));
        let mut blanks_only = CopyMode::enter(g);
        blanks_only.separators = " ".to_string();
        blanks_only.select_word(g, (0, 4));
        assert_eq!(blanks_only.copy_selection(g), Action::Copy("/usr/lo-b:x_y".to_string()), "only a blank ends a word");
        let mut slash = CopyMode::enter(g);
        slash.separators = " /:".to_string();
        slash.select_word(g, (0, 8));
        assert_eq!(slash.copy_selection(g), Action::Copy("lo-b".to_string()), "a character not listed is part of the word");
        let mut m = CopyMode::enter(g);
        m.separators = " ".to_string();
        m.goto(g, (0, 2));
        assert_eq!(m.word_forward(g, false), (0, 16), "w crosses punctuation up to the next blank");
    }

    #[test]
    fn exit_at_bottom_leaves_when_scrolled_down_to_the_end() {
        let t = eight_lines();
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        m.exit_at_bottom = true;
        m.scroll(g, -2);
        assert!(!m.at_bottom(g));
        m.scroll(g, 5);
        assert!(m.at_bottom(g));
    }

    fn emacs(g: &Grid) -> CopyMode {
        CopyMode::enter_with(g, Rc::new(CopyKeys::emacs()))
    }

    #[test]
    fn every_action_name_parses_back_and_every_default_key_is_a_real_key() {
        for (name, op) in OPS {
            assert_eq!(Op::parse(name), Some(*op), "{name}");
        }
        for op in OPS.iter().map(|(_, op)| *op) {
            assert_eq!(Op::parse(op.name()), Some(op));
        }
        assert_eq!(CopyKeys::vi().list().len(), VI_KEYS.len());
        assert_eq!(CopyKeys::emacs().list().len(), EMACS_KEYS.len());
        assert_eq!(Op::parse("no-such-action"), None);
    }

    #[test]
    fn every_action_is_reachable_from_a_default_key_of_one_table() {
        let bound: Vec<Op> = [CopyKeys::vi(), CopyKeys::emacs()].iter().flat_map(|t| t.map.values().copied()).collect();
        for (name, op) in OPS {
            assert!(*op == Op::ClearSelection || bound.contains(op), "{name} has no default key");
        }
    }

    #[test]
    fn the_editor_decides_the_default_keys() {
        assert_eq!(default_mode_keys(None, None), "emacs");
        assert_eq!(default_mode_keys(None, Some("vim")), "vi");
        assert_eq!(default_mode_keys(None, Some("/usr/bin/nvim")), "vi");
        assert_eq!(default_mode_keys(None, Some("emacs")), "emacs");
        assert_eq!(default_mode_keys(None, Some("/opt/vim-things/nano")), "emacs", "only the file name counts");
        assert_eq!(default_mode_keys(Some("nano"), Some("vim")), "emacs", "VISUAL wins when it is set");
        assert_eq!(default_mode_keys(Some("vi"), Some("nano")), "vi");
    }

    #[test]
    fn emacs_keys_move_select_and_copy() {
        let t = term(&["alpha beta", "gamma"], 4);
        let g = &t.grid;
        let mut m = emacs(g);
        press(&mut m, g, &["M-<", "M-f"]);
        assert_eq!(m.cur, (0, 4), "M-f goes to the end of the word");
        press(&mut m, g, &["C-a", "C-Space", "C-f", "C-f", "C-f"]);
        assert_eq!(m.key(g, k("M-w")), Action::Copy("alph".to_string()));
        let mut m = emacs(g);
        press(&mut m, g, &["M-<", "C-n", "C-e"]);
        assert_eq!(m.cur, (1, 4));
        press(&mut m, g, &["C-p", "C-b"]);
        assert_eq!(m.cur, (0, 3));
        assert_eq!(m.key(g, k("C-g")), Action::Cancel);
        assert_eq!(emacs(g).key(g, k("Escape")), Action::Cancel);
        assert_eq!(emacs(g).key(g, k("q")), Action::Cancel);
    }

    #[test]
    fn emacs_does_not_answer_to_vi_keys_and_back() {
        let t = term(&["alpha beta", "gamma"], 4);
        let g = &t.grid;
        let mut m = emacs(g);
        press(&mut m, g, &["M-<", "j", "l", "w"]);
        assert_eq!(m.cur, (0, 0));
        let mut vi = CopyMode::enter(g);
        press(&mut vi, g, &["M-<", "C-n"]);
        assert_ne!(vi.cur, (1, 0));
    }

    #[test]
    fn emacs_search_and_copy_to_the_end_of_the_line() {
        let t = term(&["alpha", "beta", "gamma", "delta"], 4);
        let g = &t.grid;
        let mut m = emacs(g);
        press(&mut m, g, &["M-<", "C-s", "g", "a", "Enter"]);
        assert_eq!(m.cur, (2, 0));
        press(&mut m, g, &["C-f"]);
        assert_eq!(m.key(g, k("C-k")), Action::Copy("amma".to_string()));
    }

    #[test]
    fn rectangle_selection_copies_the_same_columns_of_every_line() {
        let t = term(&["abcdef", "ghijkl", "mnopqr"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "l", "C-v", "j", "j", "l"]);
        assert_eq!(m.key(g, k("y")), Action::Copy("bc\nhi\nno".to_string()));
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "v", "j", "l", "l", "C-v"]);
        assert_eq!(m.key(g, k("y")), Action::Copy("abc\nghi".to_string()), "C-v turns a running selection into a block");
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "C-v", "j", "l", "C-v"]);
        assert_eq!(m.key(g, k("y")), Action::Copy("abcdef\ngh".to_string()), "and back to running text");
    }

    #[test]
    fn a_block_selection_marks_only_its_columns_in_the_view() {
        let t = term(&["abcdef", "ghijkl", "mnopqr"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "l", "C-v", "j", "l"]);
        let v = m.view(g);
        let marked = |x: usize, y: usize| v.cells[y * g.cols + x].style == MODE_STYLE;
        assert!(marked(1, 0) && marked(2, 0) && marked(1, 1) && marked(2, 1));
        assert!(!marked(0, 0) && !marked(3, 0) && !marked(0, 1) && !marked(3, 1));
        assert!(!marked(1, 2), "the third line is outside the block");
    }

    #[test]
    fn other_end_swaps_the_cursor_and_the_start_of_the_selection() {
        let t = term(&["alpha beta"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "v", "w", "o"]);
        assert_eq!(m.cur, (0, 0));
        assert_eq!(m.key(g, k("y")), Action::Copy("alpha b".to_string()));
        let mut none = CopyMode::enter(g);
        press(&mut none, g, &["g", "o"]);
        assert_eq!(none.cur, (0, 0), "without a selection there is no other end");
    }

    #[test]
    fn vi_d_copies_to_the_end_of_the_line() {
        let t = term(&["alpha beta"], 4);
        let g = &t.grid;
        let mut m = CopyMode::enter(g);
        press(&mut m, g, &["g", "w"]);
        assert_eq!(m.key(g, k("D")), Action::Copy("beta".to_string()));
    }

    #[test]
    fn a_user_binding_changes_the_table_a_mode_started_with() {
        let t = term(&["alpha beta"], 4);
        let g = &t.grid;
        let mut keys = CopyKeys::vi();
        keys.bind(k("x"), Op::Cancel);
        keys.bind(k("v"), Op::BeginSelection);
        assert!(keys.unbind(&k("q")));
        assert!(!keys.unbind(&k("q")));
        let keys = Rc::new(keys);
        let mut m = CopyMode::enter_with(g, Rc::clone(&keys));
        assert_eq!(m.key(g, k("x")), Action::Cancel);
        assert_eq!(m.key(g, k("q")), Action::None, "an unbound key does nothing");
        press(&mut m, g, &["g", "v", "v", "w"]);
        assert_eq!(m.key(g, k("y")), Action::Copy("alpha b".to_string()), "begin-selection restarts instead of toggling");
        assert_eq!(CopyMode::enter(g).key(g, k("x")), Action::None, "the default table is untouched");
    }

    #[test]
    fn the_listing_has_one_row_per_key_by_name() {
        let rows = CopyKeys::vi().list();
        assert!(rows.contains(&("y".to_string(), "copy-selection-and-cancel")));
        assert!(rows.contains(&("C-v".to_string(), "rectangle-toggle")));
        let mut sorted = rows.clone();
        sorted.sort();
        assert_eq!(rows, sorted);
    }
}
