//! The status line: left segment, window tabs and right blocks laid out from runs of styled
//! text that the format options produce (see `server::conf::status_for`). The constants are
//! the built-in colors, which the option defaults repeat.

use unicode_width::UnicodeWidthChar;

#[cfg(test)]
use crate::term::cell::BOLD;
use crate::term::cell::{Cell, Color, Style};

pub const BAR: Color = Color::Rgb(24, 24, 37);
pub const TEXT: Color = Color::Rgb(205, 214, 244);
#[cfg(test)]
pub const SUBTEXT: Color = Color::Rgb(166, 173, 200);
pub const DARK: Color = Color::Rgb(0, 0, 0);
#[cfg(test)]
const TAB: Color = Color::Rgb(30, 30, 46);
#[cfg(test)]
const SURFACE: Color = Color::Rgb(49, 50, 68);
#[cfg(test)]
const OVERLAY: Color = Color::Rgb(69, 71, 90);
#[cfg(test)]
const MUTED: Color = Color::Rgb(88, 91, 112);
pub const BLUE: Color = Color::Rgb(137, 180, 250);
#[cfg(test)]
const LEAF: char = '\u{f032a}';

#[cfg(test)]
pub struct Tab<'a> {
    pub name: &'a str,
    pub active: bool,
}

/// The bar grows downwards as tabs wrap, up to this many rows; tabs past that are cut off.
pub const MAX_ROWS: usize = 5;

pub struct Status {
    /// `rows` rows of `cols` cells, top row first.
    pub cells: Vec<Cell>,
    pub rows: usize,
    /// Width of the session segment at the left of the top row, for click handling.
    pub left_w: usize,
    /// Columns `[start, end)` of each tab, in window order, for click handling.
    pub tab_spans: Vec<(usize, usize)>,
    /// Row each tab sits on, parallel to `tab_spans`.
    pub tab_rows: Vec<usize>,
    /// Column the right blocks start at on the top row, and the runs drawn there.
    pub right_start: usize,
    pub right: Vec<Run>,
    /// Columns `[start, end)` of agent glyphs in the right blocks of the top row, for clicks.
    pub agent_spans: Vec<(usize, usize)>,
}

impl Status {
    /// Draw row `index` from `runs` alone, left to right and cut at the edge (`status-format[N]`).
    /// What the standard layout put there goes: tabs on that row cannot be clicked, and the right
    /// blocks and agent glyphs are gone when it is the top row.
    pub fn replace_row(&mut self, cols: usize, index: usize, bar: Style, runs: &[Run]) {
        if index >= self.rows || cols == 0 {
            return;
        }
        let mut row = Row::new(cols, bar);
        for (text, style) in runs {
            row.put(text, *style);
        }
        self.finish_row(cols, index, row);
    }

    /// `replace_row` for a row of three parts (`#[align=left]`, `#[align=centre]`, `#[align=right]`):
    /// the left part from the first cell, the right part against the last one, the center part
    /// in the middle of the row; a part never covers the one before it.
    pub fn replace_row_aligned(&mut self, cols: usize, index: usize, bar: Style, parts: &[Vec<Run>; 3]) {
        if index >= self.rows || cols == 0 {
            return;
        }
        let mut row = Row::new(cols, bar);
        for (text, style) in &parts[0] {
            row.put(text, *style);
        }
        let left_end = row.x;
        let right_start = (cols - runs_width(&parts[2]).min(cols)).max(left_end);
        row.x = right_start;
        for (text, style) in &parts[2] {
            row.put(text, *style);
        }
        row.x = (cols.saturating_sub(runs_width(&parts[1])) / 2).max(left_end);
        row.limit = right_start;
        for (text, style) in &parts[1] {
            row.put(text, *style);
        }
        self.finish_row(cols, index, row);
    }

    fn finish_row(&mut self, cols: usize, index: usize, row: Row) {
        self.cells.splice(index * cols..(index + 1) * cols, row.cells);
        for (span, &on) in self.tab_spans.iter_mut().zip(&self.tab_rows) {
            if on == index {
                *span = (0, 0);
            }
        }
        if index == 0 {
            self.left_w = 0;
            self.right_start = cols;
            self.right.clear();
            self.agent_spans.clear();
        }
    }
}

struct Row {
    cells: Vec<Cell>,
    x: usize,
    limit: usize,
}

impl Row {
    fn new(cols: usize, style: Style) -> Row {
        Row { cells: vec![Cell { style, ..Cell::BLANK }; cols], x: 0, limit: cols }
    }

    fn put(&mut self, text: &str, style: Style) {
        for ch in text.chars() {
            // The last stop before the outer terminal: a control, C1 or bidirectional character
            // from any source is shown as `?` (security review P-08).
            let ch = if crate::sanitize::is_unsafe(ch) { '?' } else { ch };
            let w = ch.width().unwrap_or(1).max(1);
            if self.x + w > self.limit {
                self.x = self.limit;
                return;
            }
            self.cells[self.x] = Cell { ch, width: w as u8, style, tile: 0, mark: 0 };
            if w == 2 {
                self.cells[self.x + 1] = Cell { ch: ' ', width: 0, style, tile: 0, mark: 0 };
            }
            self.x += w;
        }
    }
}

pub fn width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(1).max(1)).sum()
}

#[cfg(test)]
pub fn build(cols: usize, session: &str, tabs: &[Tab], right: &[(String, Style)]) -> Status {
    build_rows(cols, session, tabs, right, 1)
}

/// `right` is a list of blocks drawn right-aligned and flush against each other on the top row;
/// they are reserved before any tab is placed, so growing tabs never push them out. Tabs that
/// do not fit wrap onto further rows. The bar has at least `min_rows` rows.
#[cfg(test)]
pub fn build_rows(cols: usize, session: &str, tabs: &[Tab], right: &[(String, Style)], min_rows: usize) -> Status {
    build_rows_from(cols, session, tabs, right, min_rows, 1)
}

/// A piece of text in one style.
pub type Run = (String, Style);

/// What the bar shows, already expanded from the format options.
pub struct Parts<'a> {
    /// Style of cells nothing is drawn on.
    pub bar: Style,
    pub left: &'a [Run],
    /// Runs of each window's tab, in window order.
    pub tabs: &'a [Vec<Run>],
    /// Drawn between tabs (`window-status-separator`).
    pub sep: &'a [Run],
    pub right: &'a [Run],
}

fn runs_width(runs: &[Run]) -> usize {
    runs.iter().map(|(t, _)| width(t)).sum()
}

/// The first `max` columns of `runs`.
pub fn clip(runs: &[Run], max: usize) -> Vec<Run> {
    let mut out = Vec::new();
    let mut used = 0;
    for (text, style) in runs {
        let mut piece = String::new();
        for ch in text.chars() {
            let w = ch.width().unwrap_or(1).max(1);
            if used + w > max {
                break;
            }
            used += w;
            piece.push(ch);
        }
        let whole = piece.len() == text.len();
        out.push((piece, *style));
        if !whole {
            break;
        }
    }
    out
}

/// `build_rows` with the first tab numbered `base` (`base-index`), in the built-in look.
#[cfg(test)]
pub fn build_rows_from(
    cols: usize,
    session: &str,
    tabs: &[Tab],
    right: &[(String, Style)],
    min_rows: usize,
    base: usize,
) -> Status {
    let bar = Style { fg: TEXT, bg: BAR, attrs: 0 };
    let on_blue = Style { fg: DARK, bg: BLUE, attrs: BOLD };
    let idle_tab = Style { fg: SUBTEXT, bg: TAB, attrs: 0 };
    let left = vec![(format!(" {LEAF} {session} "), on_blue)];
    let room = cols.saturating_sub(runs_width(&left));
    let mut shown: Vec<&(String, Style)> = right.iter().collect();
    // On a bar too narrow for both, shed the date first, then the host; the clock goes last.
    while !shown.is_empty() && shown.iter().map(|(t, _)| width(t)).sum::<usize>() > room {
        shown.remove(if shown.len() == 2 { 0 } else { shown.len() - 1 });
    }
    let right: Vec<Run> = shown.into_iter().cloned().collect();
    let tab_runs: Vec<Vec<Run>> = tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| vec![(format!(" {}  {} ", i + base, tab.name), if tab.active { on_blue } else { idle_tab })])
        .collect();
    layout(cols, &Parts { bar, left: &left, tabs: &tab_runs, sep: &[], right: &right }, min_rows)
}

/// Lay the bar out: the left segment, the tabs (wrapping onto more rows), the right blocks
/// flush right on the top row. They are reserved before any tab is placed, so growing tabs never
/// push them out. The bar has at least `min_rows` rows.
pub fn layout(cols: usize, parts: &Parts, min_rows: usize) -> Status {
    layout_justified(cols, parts, min_rows, Justify::Left)
}

/// Where the tabs sit between the left and the right segment (`status-justify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Left,
    Center,
    Right,
    /// Centered on the whole row, not on the room between the segments.
    AbsoluteCenter,
}

impl Justify {
    pub fn named(name: &str) -> Justify {
        match name {
            "centre" | "center" => Justify::Center,
            "right" => Justify::Right,
            "absolute-centre" | "absolute-center" => Justify::AbsoluteCenter,
            _ => Justify::Left,
        }
    }
}

/// `layout` with the tabs placed by `justify`. Tabs that do not all fit on the top row wrap from
/// the left, whatever the setting.
pub fn layout_justified(cols: usize, parts: &Parts, min_rows: usize, justify: Justify) -> Status {
    let Parts { bar, left, tabs, sep, right } = *parts;
    let right = clip(right, cols.saturating_sub(runs_width(left)));
    let right_w = runs_width(&right);

    let tab_right = cols - right_w;
    let left_w = runs_width(left).min(tab_right);
    // Wrapped rows line up under the first tab and stop where the top row's tabs stop.
    let fresh = || {
        let mut row = Row::new(cols, bar);
        row.x = left_w;
        row.limit = tab_right;
        row
    };
    let mut rows = vec![Row::new(cols, bar)];
    rows[0].limit = tab_right;
    for (text, style) in left {
        rows[0].put(text, *style);
    }
    let mut tab_spans = Vec::with_capacity(tabs.len());
    let mut tab_rows = Vec::with_capacity(tabs.len());
    let mut cut = false;
    // Where the first tab starts: past the left segment, moved right when they all fit on the top
    // row and `justify` says so.
    let total = tabs.iter().map(|t| runs_width(t)).sum::<usize>() + runs_width(sep) * tabs.len().saturating_sub(1);
    let room = tab_right - left_w;
    let home = if tabs.is_empty() || total > room {
        left_w
    } else {
        left_w
            + match justify {
                Justify::Left => 0,
                Justify::Center => (room - total) / 2,
                Justify::Right => room - total,
                Justify::AbsoluteCenter => (cols.saturating_sub(total) / 2).clamp(left_w, left_w + (room - total)) - left_w,
            }
    };
    rows[0].x = home;
    for tab in tabs {
        let tw = runs_width(tab);
        let sw = runs_width(sep);
        let row = rows.last().expect("at least one row");
        let lead = if row.x > home { sw } else { 0 };
        if row.x + lead + tw > row.limit && tw <= tab_right - left_w && row.x > home {
            if rows.len() == MAX_ROWS {
                cut = true;
            } else {
                rows.push(fresh());
            }
        }
        if cut {
            tab_spans.push((0, 0));
            tab_rows.push(0);
            continue;
        }
        let row = rows.last_mut().expect("at least one row");
        if row.x > home {
            for (text, style) in sep {
                row.put(text, *style);
            }
        }
        let start = row.x;
        for (text, style) in tab {
            row.put(text, *style);
        }
        tab_spans.push((start, row.x));
        tab_rows.push(rows.len() - 1);
    }
    if cut && tab_right > 0 {
        let last = rows.last_mut().expect("at least one row");
        last.cells[tab_right - 1] = Cell { ch: '\u{2026}', width: 1, style: bar, tile: 0, mark: 0 };
    }
    while rows.len() < min_rows.min(MAX_ROWS) {
        rows.push(fresh());
    }

    rows[0].limit = cols;
    rows[0].x = cols - right_w;
    for (text, style) in &right {
        rows[0].put(text, *style);
    }
    let count = rows.len();
    Status {
        cells: rows.into_iter().flat_map(|r| r.cells).collect(),
        rows: count,
        left_w,
        tab_spans,
        tab_rows,
        right_start: cols - right_w,
        right,
        agent_spans: Vec::new(),
    }
}

/// The status row while a prompt is open: a label on blue, then the text typed so far. Returns
/// the cells and the column the cursor belongs in.
#[cfg(test)]
pub fn prompt_row(cols: usize, label: &str, text: &str) -> (Vec<Cell>, usize) {
    let bar = Style { fg: TEXT, bg: BAR, attrs: 0 };
    prompt_row_styled(cols, label, text, Style { fg: DARK, bg: BLUE, attrs: BOLD }, bar)
}

/// `prompt_row` with the label and the text in the given styles (`message-style`).
pub fn prompt_row_styled(cols: usize, label: &str, text: &str, label_style: Style, bar: Style) -> (Vec<Cell>, usize) {
    let mut row = Row::new(cols, bar);
    row.put(&format!(" {label}"), label_style);
    row.put(" ", bar);
    row.put(text, bar);
    let col = row.x.min(cols.saturating_sub(1));
    (row.cells, col)
}

/// Host, clock and date blocks in local time (tmux `#h`, `%I:%M %p`, `%m/%d/%Y`).
#[cfg(test)]
pub fn right_blocks() -> Vec<(String, Style)> {
    let mut host = [0u8; 256];
    // SAFETY: the buffer is valid for its length and gethostname NUL-terminates within it.
    let host = if unsafe { libc::gethostname(host.as_mut_ptr().cast(), host.len()) } == 0 {
        let end = host.iter().position(|&b| b == 0).unwrap_or(host.len());
        let full = String::from_utf8_lossy(&host[..end]).into_owned();
        full.split('.').next().unwrap_or("").to_string()
    } else {
        String::new()
    };
    // SAFETY: time(NULL) is always valid; localtime_r only writes into the zeroed tm we pass.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    let hour = if tm.tm_hour % 12 == 0 { 12 } else { tm.tm_hour % 12 };
    let ampm = if tm.tm_hour < 12 { "AM" } else { "PM" };
    vec![
        (format!(" {host} "), Style { fg: DARK, bg: MUTED, attrs: 0 }),
        (format!(" {hour:02}:{:02} {ampm} ", tm.tm_min), Style { fg: SUBTEXT, bg: SURFACE, attrs: 0 }),
        (
            format!(" {:02}/{:02}/{} ", tm.tm_mon + 1, tm.tm_mday, tm.tm_year + 1900),
            Style { fg: TEXT, bg: OVERLAY, attrs: 0 },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &Status) -> String {
        s.cells.iter().filter(|c| c.width != 0).map(|c| c.ch).collect()
    }

    fn block(text: &str) -> Vec<(String, Style)> {
        vec![(text.to_string(), Style { fg: TEXT, bg: OVERLAY, attrs: 0 })]
    }

    #[test]
    fn segment_and_tabs_are_laid_out_left_to_right() {
        let tabs = [Tab { name: "claude", active: false }, Tab { name: "zsh", active: true }];
        let s = build(40, "1", &tabs, &[]);
        assert_eq!(text(&s).trim_end(), " \u{f032a} 1  1  claude  2  zsh");
        assert_eq!(s.tab_spans, vec![(5, 16), (16, 24)]);
    }

    #[test]
    fn the_session_segment_starts_at_the_left_edge() {
        let s = build(40, "1", &[], &[]);
        assert_eq!(s.cells[0].style.bg, BLUE);
        assert_eq!(s.cells[4].style.bg, BLUE);
        assert_eq!(s.cells[5].style.bg, BAR);
    }

    #[test]
    fn active_tab_is_bold_on_blue_and_idle_tabs_are_on_the_tab_color() {
        let tabs = [Tab { name: "a", active: false }, Tab { name: "b", active: true }];
        let s = build(30, "0", &tabs, &[]);
        let (start, _) = s.tab_spans[1];
        assert_eq!(s.cells[start + 1].style.bg, BLUE);
        assert_eq!(s.cells[start + 1].style.attrs, BOLD);
        let (start, _) = s.tab_spans[0];
        assert_eq!(s.cells[start + 1].style.bg, TAB);
    }

    #[test]
    fn right_blocks_are_right_aligned_when_they_fit() {
        let s = build(40, "0", &[Tab { name: "zsh", active: true }], &block(" right "));
        assert!(text(&s).ends_with(" right "));
        assert_eq!(s.cells.len(), 40);
        assert_eq!(s.cells[39].style.bg, OVERLAY);
    }

    #[test]
    fn right_blocks_yield_to_the_tabs_on_a_narrow_bar() {
        let s = build(20, "0", &[Tab { name: "zsh", active: true }], &block("a very long right side"));
        assert!(!text(&s).contains("right"));
    }

    #[test]
    fn tabs_are_clipped_at_the_edge() {
        let tabs = [Tab { name: "abcdefghijkl", active: true }];
        let s = build(12, "0", &tabs, &[]);
        assert_eq!(s.cells.len(), 12);
        assert_eq!(s.tab_spans[0].1, 12);
    }

    fn many_tabs(n: usize) -> Vec<Tab<'static>> {
        (0..n).map(|i| Tab { name: "zsh", active: i == 0 }).collect()
    }

    fn row_text(s: &Status, cols: usize, row: usize) -> String {
        s.cells[row * cols..(row + 1) * cols].iter().filter(|c| c.width != 0).map(|c| c.ch).collect()
    }

    #[test]
    fn an_aligned_row_puts_its_three_parts_left_center_and_right_without_overlap() {
        let mut s = build(20, "0", &[Tab { name: "z", active: true }], &[]);
        let st = Style::default();
        let part = |t: &str| vec![(t.to_string(), st)];
        s.replace_row_aligned(20, 0, st, &[part("LEFT"), part("MID"), part("RIGHT")]);
        assert_eq!(row_text(&s, 20, 0), "LEFT    MID    RIGHT");
        s.replace_row_aligned(20, 0, st, &[part("LEFT"), vec![], part("RIGHT")]);
        assert_eq!(row_text(&s, 20, 0), "LEFT           RIGHT");
        s.replace_row_aligned(20, 0, st, &[part("0123456789"), part("MIDDLE"), part("ABCDEFGHIJKL")]);
        assert_eq!(row_text(&s, 20, 0), "0123456789ABCDEFGHIJ", "the left part is never covered, the right one is cut, the center has no room");
    }

    #[test]
    fn overflowing_tabs_wrap_onto_more_rows() {
        let s = build(30, "0", &many_tabs(6), &[]);
        assert!(s.rows > 1);
        assert_eq!(s.cells.len(), s.rows * 30);
        assert_eq!(s.tab_rows[0], 0);
        assert_eq!(*s.tab_rows.last().unwrap(), s.rows - 1);
        for (&(start, end), &row) in s.tab_spans.iter().zip(&s.tab_rows) {
            assert!(start < end && end <= 30 && row < s.rows);
        }
    }

    #[test]
    fn wrapped_rows_line_up_under_the_first_tab_and_stop_at_the_top_rows_edge() {
        let s = build(60, "0", &many_tabs(14), &block(" 10:15 AM "));
        assert!(s.rows > 2);
        let first = s.tab_spans[0].0;
        let edge = 60 - 10;
        for (&(start, end), &row) in s.tab_spans.iter().zip(&s.tab_rows) {
            assert!(start >= first && end <= edge, "row {row}: {start}..{end}");
        }
        for row in 1..s.rows {
            assert!(s.tab_spans.iter().zip(&s.tab_rows).any(|(&(start, _), &r)| r == row && start == first));
            assert_eq!(s.cells[row * 60 + first - 1].style.bg, BAR);
            assert_eq!(s.cells[row * 60 + edge].style.bg, BAR);
        }
    }

    #[test]
    fn tabs_stay_in_order_and_never_overlap_on_a_row() {
        let s = build(30, "0", &many_tabs(9), &[]);
        let placed: Vec<_> = s.tab_rows.iter().zip(&s.tab_spans).collect();
        for pair in placed.windows(2) {
            let ((r1, (_, e1)), (r2, (s2, _))) = (pair[0], pair[1]);
            assert!(r2 > r1 || (r2 == r1 && s2 >= e1));
        }
    }

    #[test]
    fn right_blocks_stay_on_the_top_row_at_every_tab_count() {
        for n in 0..40 {
            let s = build(60, "0", &many_tabs(n), &block(" 10:15 AM "));
            assert!(row_text(&s, 60, 0).ends_with(" 10:15 AM "), "{n} tabs");
            assert_eq!(s.cells[59].style.bg, OVERLAY, "{n} tabs");
        }
    }

    #[test]
    fn top_row_tabs_stop_before_the_right_blocks() {
        let s = build(40, "0", &many_tabs(6), &block(" 10:15 AM "));
        for (&(_, end), &row) in s.tab_spans.iter().zip(&s.tab_rows) {
            if row == 0 {
                assert!(end <= 30);
            }
        }
    }

    #[test]
    fn the_bar_never_grows_past_the_row_cap_and_marks_the_cut() {
        let s = build(40, "0", &many_tabs(60), &block(" 10:15 AM "));
        assert_eq!(s.rows, MAX_ROWS);
        assert!(row_text(&s, 40, MAX_ROWS - 1).contains('\u{2026}'));
        assert!(row_text(&s, 40, 0).ends_with(" 10:15 AM "));
    }

    #[test]
    fn min_rows_pads_the_bar_with_blank_rows() {
        let s = build_rows(40, "0", &many_tabs(1), &[], 3);
        assert_eq!(s.rows, 3);
        assert_eq!(s.cells.len(), 120);
        assert_eq!(row_text(&s, 40, 2).trim(), "");
    }

    fn justified(justify: Justify, tabs: &[Vec<Run>]) -> Status {
        let bar = Style { fg: TEXT, bg: BAR, attrs: 0 };
        let left = vec![("[s]".to_string(), bar)];
        let right = vec![("RRRRRR".to_string(), bar)];
        layout_justified(40, &Parts { bar, left: &left, tabs, sep: &[], right: &right }, 1, justify)
    }

    #[test]
    fn status_justify_moves_the_tabs_between_the_segments() {
        let bar = Style { fg: TEXT, bg: BAR, attrs: 0 };
        let tabs = vec![vec![("ab".to_string(), bar)], vec![("cd".to_string(), bar)]];
        assert_eq!(justified(Justify::Left, &tabs).tab_spans, vec![(3, 5), (5, 7)]);
        assert_eq!(justified(Justify::Center, &tabs).tab_spans, vec![(16, 18), (18, 20)]);
        assert_eq!(justified(Justify::Right, &tabs).tab_spans, vec![(30, 32), (32, 34)]);
        assert_eq!(justified(Justify::AbsoluteCenter, &tabs).tab_spans, vec![(18, 20), (20, 22)]);
    }

    #[test]
    fn a_replaced_row_shows_only_its_own_runs_and_drops_what_was_there() {
        let bar = Style { fg: TEXT, bg: BAR, attrs: 0 };
        let tabs = vec![vec![("ab".to_string(), bar)], vec![("cd".to_string(), bar)]];
        let mut s = justified(Justify::Left, &tabs);
        s.replace_row(40, 0, bar, &[("custom".to_string(), bar)]);
        assert_eq!(row_text(&s, 40, 0).trim_end(), "custom");
        assert_eq!(s.tab_spans, vec![(0, 0), (0, 0)]);
        assert!(s.right.is_empty() && s.agent_spans.is_empty());
        assert_eq!(s.right_start, 40);
        let mut s = justified(Justify::Left, &tabs);
        s.replace_row(40, 3, bar, &[("nope".to_string(), bar)]);
        assert!(row_text(&s, 40, 0).starts_with("[s]abcd") && row_text(&s, 40, 0).ends_with("RRRRRR"), "a row past the bar is ignored");
    }

    #[test]
    fn status_justify_leaves_tabs_that_do_not_fit_to_wrap_from_the_left() {
        let bar = Style { fg: TEXT, bg: BAR, attrs: 0 };
        let tabs: Vec<Vec<Run>> = (0..8).map(|_| vec![("abcdef".to_string(), bar)]).collect();
        let s = justified(Justify::Right, &tabs);
        assert_eq!(s.tab_spans[0].0, 3);
        assert!(s.rows > 1);
    }

    #[test]
    fn narrow_bars_shed_the_date_then_the_host_but_keep_the_clock() {
        let style = Style { fg: TEXT, bg: OVERLAY, attrs: 0 };
        let blocks = vec![(" host ".to_string(), style), (" 10:15 AM ".to_string(), style), (" 11/03/2026 ".to_string(), style)];
        let s = build(27, "0", &[], &blocks);
        let t = row_text(&s, 27, 0);
        assert!(t.ends_with(" host  10:15 AM "));
        assert!(!t.contains("2026"));
        let s = build(20, "0", &[], &blocks);
        let t = row_text(&s, 20, 0);
        assert!(t.ends_with(" 10:15 AM ") && !t.contains("host"));
    }

    #[test]
    fn prompt_row_shows_label_and_text_with_the_cursor_after_them() {
        let (cells, col) = prompt_row(40, "rename-window:", "abc");
        let text: String = cells.iter().filter(|c| c.width != 0).map(|c| c.ch).collect();
        assert!(text.trim_end().ends_with("rename-window: abc"));
        assert_eq!(cells.len(), 40);
        assert_eq!(col, 19);
        assert_eq!(cells[1].style.bg, BLUE);
    }

    #[test]
    fn clock_blocks_have_the_expected_shape() {
        let blocks = right_blocks();
        assert_eq!(blocks.len(), 3);
        assert!(blocks.iter().all(|(t, _)| t.starts_with(' ') && t.ends_with(' ')));
        assert!(blocks[1].0.contains("AM") || blocks[1].0.contains("PM"));
        assert_eq!(blocks[2].0.matches('/').count(), 2);
    }

    fn run(text: &str) -> Run {
        (text.to_string(), Style::default())
    }

    #[test]
    fn clip_cuts_runs_at_a_column() {
        let runs = [run("abc"), run("def")];
        assert_eq!(clip(&runs, 4).iter().map(|r| r.0.as_str()).collect::<String>(), "abcd");
        assert_eq!(clip(&runs, 0).iter().map(|r| r.0.as_str()).collect::<String>(), "");
        assert_eq!(clip(&runs, 99).iter().map(|r| r.0.as_str()).collect::<String>(), "abcdef");
    }

    #[test]
    fn the_separator_goes_between_tabs_only() {
        let tabs = [vec![run("[a]")], vec![run("[b]")], vec![run("[c]")]];
        let parts = Parts { bar: Style::default(), left: &[run("L")], tabs: &tabs, sep: &[run("|")], right: &[] };
        let s = layout(30, &parts, 1);
        assert_eq!(text(&s).trim_end(), "L[a]|[b]|[c]");
        assert_eq!(s.tab_spans, vec![(1, 4), (5, 8), (9, 12)]);
    }
}
