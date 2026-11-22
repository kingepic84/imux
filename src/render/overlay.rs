//! A centered list box drawn over the panes: the session and window pickers. The look is a
//! placeholder for `mode-style` and `message-style` (M9).

use unicode_width::UnicodeWidthChar;

use super::compositor::Screen;
use super::status::{BAR, BLUE, DARK, TEXT};
use crate::term::cell::{BOLD, Cell, Color, DIM, Style};

const MIN_WIDTH: usize = 30;
const MAX_WIDTH: usize = 72;

/// Where the box sits and which items it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    /// First item shown; items scroll a page at a time so clicks and drawing agree.
    pub top: usize,
    pub visible: usize,
}

pub enum Hit {
    Item(usize),
    Outside,
    Border,
}

fn width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(1).max(1)).sum()
}

/// Box geometry for `items` inside the `rows` rows above the status bar of a `cols` wide screen.
pub fn geometry(cols: usize, rows: usize, items: &[String], selected: usize) -> Geometry {
    let widest = items.iter().map(|t| width(t)).max().unwrap_or(0);
    let w = (widest + 4).clamp(MIN_WIDTH, MAX_WIDTH).min(cols);
    let visible = items.len().min(rows.saturating_sub(2)).max(1);
    let h = (visible + 2).min(rows);
    let top = (selected / visible) * visible;
    Geometry { x: (cols - w) / 2, y: (rows - h) / 2, w, h, top, visible }
}

pub fn hit(g: &Geometry, count: usize, x: usize, y: usize) -> Hit {
    let inside = x >= g.x && x < g.x + g.w && y >= g.y && y < g.y + g.h;
    if !inside {
        return Hit::Outside;
    }
    let row = y - g.y;
    if row == 0 || row == g.h - 1 || x == g.x || x == g.x + g.w - 1 {
        return Hit::Border;
    }
    let index = g.top + row - 1;
    if index < count { Hit::Item(index) } else { Hit::Border }
}

fn put(screen: &mut Screen, x: usize, y: usize, ch: char, style: Style) {
    if x < screen.cols && y < screen.rows {
        screen.cells[y * screen.cols + x] = Cell { ch, width: 1, style, tile: 0, mark: 0 };
    }
}

/// The label row of a pane (`pane-border-status`): `line` across `w` cells from `x`, and the styled
/// `runs` of its text from the third cell (the first when the row is short), cut at the end of
/// the row. Characters that are not one cell wide are left out.
pub fn draw_pane_label(screen: &mut Screen, x: usize, y: usize, w: usize, line: char, border: Style, runs: &[(String, Style)]) {
    for i in 0..w {
        put(screen, x + i, y, line, border);
    }
    let (start, room) = if w >= 8 { (2, w - 4) } else { (0, w) };
    let mut at = 0;
    for (text, style) in runs {
        for ch in text.chars().filter(|c| c.width() == Some(1)) {
            if at >= room {
                return;
            }
            put(screen, x + start + at, y, ch, *style);
            at += 1;
        }
    }
}

/// Draw each pane's number as a block centered in the pane (`display-panes`); the active pane's is
/// in the accent color. Panes too small for the label get none. The cursor is hidden. `backs` are
/// the backgrounds of the labels of the other panes and of the active one (`display-panes-color`,
/// `display-panes-active-color`); the default color keeps the built-in one.
pub fn draw_pane_numbers(screen: &mut Screen, panes: &[(usize, usize, usize, usize, usize, bool)], backs: (Color, Color)) {
    for &(number, x, y, w, h, active) in panes {
        let text = format!(" {number} ");
        let width = text.chars().count();
        if w < width || h == 0 {
            continue;
        }
        let pick = |own: Color, built_in: Color| if own == Color::Default { built_in } else { own };
        let style = if active {
            Style { fg: DARK, bg: pick(backs.1, BLUE), attrs: BOLD }
        } else {
            Style { fg: TEXT, bg: pick(backs.0, BAR), attrs: BOLD }
        };
        let (x0, y0) = (x + (w - width) / 2, y + h / 2);
        for (i, ch) in text.chars().enumerate() {
            put(screen, x0 + i, y0, ch, style);
        }
    }
    screen.cursor = None;
}

/// Big digits for `clock-mode`: five rows of five columns, `#` is a lit cell.
const DIGITS: [[&str; 5]; 10] = [
    ["#####", "#   #", "#   #", "#   #", "#####"],
    ["  #  ", " ##  ", "  #  ", "  #  ", " ### "],
    ["#####", "    #", "#####", "#    ", "#####"],
    ["#####", "    #", "#####", "    #", "#####"],
    ["#   #", "#   #", "#####", "    #", "    #"],
    ["#####", "#    ", "#####", "    #", "#####"],
    ["#####", "#    ", "#####", "#   #", "#####"],
    ["#####", "    #", "    #", "    #", "    #"],
    ["#####", "#   #", "#####", "#   #", "#####"],
    ["#####", "#   #", "#####", "    #", "#####"],
];
const COLON: [&str; 5] = [" ", "#", " ", "#", " "];

/// The five rows of `text` (digits and colons) in big letters; characters that are neither are
/// left out. Glyphs are one column apart.
pub fn clock_rows(text: &str) -> [String; 5] {
    let mut rows: [String; 5] = Default::default();
    let mut first = true;
    for c in text.chars() {
        let glyph: [&str; 5] = match c {
            '0'..='9' => DIGITS[c as usize - '0' as usize],
            ':' => COLON,
            _ => continue,
        };
        for (row, part) in rows.iter_mut().zip(glyph) {
            if !first {
                row.push(' ');
            }
            row.push_str(part);
        }
        first = false;
    }
    rows
}

/// `clock-mode`: blank the pane at (`x`, `y`, `w`, `h`) and draw `text` (such as `14:05`) in big
/// digits in `color`, centered, with `suffix` (`AM`) in small letters under them. A pane too
/// small for the digits gets the time in plain text, one too small for that stays blank. The
/// cursor is hidden.
pub fn draw_clock(screen: &mut Screen, (x, y, w, h): (usize, usize, usize, usize), text: &str, suffix: &str, color: Color) {
    for dy in 0..h {
        for dx in 0..w {
            put(screen, x + dx, y + dy, ' ', Style::default());
        }
    }
    screen.cursor = None;
    let rows = clock_rows(text);
    let big = rows[0].chars().count();
    let extra = usize::from(!suffix.is_empty());
    if big > 0 && w >= big && h >= 5 + extra {
        let (x0, y0) = (x + (w - big) / 2, y + (h - 5 - extra) / 2);
        for (dy, line) in rows.iter().enumerate() {
            for (dx, _) in line.chars().enumerate().filter(|&(_, c)| c == '#') {
                put(screen, x0 + dx, y0 + dy, ' ', Style { fg: Color::Default, bg: color, attrs: 0 });
            }
        }
        for (i, c) in suffix.chars().enumerate().take(w) {
            put(screen, x + (w - suffix.chars().count().min(w)) / 2 + i, y0 + 5, c, Style { fg: color, bg: Color::Default, attrs: BOLD });
        }
        return;
    }
    let width = text.chars().count();
    if w >= width && h >= 1 {
        for (i, c) in text.chars().enumerate() {
            put(screen, x + (w - width) / 2 + i, y + h / 2, c, Style { fg: color, bg: Color::Default, attrs: BOLD });
        }
    }
}

/// Draw the box with a title in its top border; `selected` is highlighted. The cursor is hidden.
pub fn draw(screen: &mut Screen, pane_rows: usize, title: &str, items: &[String], selected: usize) {
    draw_at(screen, 0, pane_rows, title, "", items, selected);
}

/// `draw` for panes that start at screen row `first_row` (a status bar on top), with `footer`
/// (the filter query) written on the bottom border.
pub fn draw_at(screen: &mut Screen, first_row: usize, pane_rows: usize, title: &str, footer: &str, items: &[String], selected: usize) {
    let mut g = geometry(screen.cols, pane_rows, items, selected);
    g.y += first_row;
    let border = Style { fg: BLUE, bg: BAR, attrs: 0 };
    let plain = Style { fg: TEXT, bg: BAR, attrs: 0 };
    let chosen = Style { fg: DARK, bg: BLUE, attrs: BOLD };
    for dy in 0..g.h {
        for dx in 0..g.w {
            let (x, y) = (g.x + dx, g.y + dy);
            let (top, bottom, left, right) = (dy == 0, dy == g.h - 1, dx == 0, dx == g.w - 1);
            let ch = match (top, bottom, left, right) {
                (true, _, true, _) => '┌',
                (true, _, _, true) => '┐',
                (_, true, true, _) => '└',
                (_, true, _, true) => '┘',
                (true, ..) | (_, true, ..) => '─',
                (_, _, true, _) | (_, _, _, true) => '│',
                _ => ' ',
            };
            let style = if ch == ' ' { plain } else { border };
            put(screen, x, y, ch, style);
        }
    }
    for (i, ch) in title.chars().take(g.w.saturating_sub(4)).enumerate() {
        put(screen, g.x + 2 + i, g.y, ch, border);
    }
    for (i, ch) in footer.chars().take(g.w.saturating_sub(4)).enumerate() {
        put(screen, g.x + 2 + i, g.y + g.h - 1, if ch.is_control() { '?' } else { ch }, border);
    }
    for row in 0..g.visible {
        let index = g.top + row;
        let Some(text) = items.get(index) else { break };
        let style = if index == selected { chosen } else { plain };
        let y = g.y + 1 + row;
        for dx in 1..g.w - 1 {
            put(screen, g.x + dx, y, ' ', style);
        }
        let mut x = g.x + 2;
        for ch in text.chars() {
            let w = ch.width().unwrap_or(1).max(1);
            if w != 1 || x + 2 > g.x + g.w {
                continue;
            }
            put(screen, x, y, ch, style);
            x += 1;
        }
    }
    screen.cursor = None;
}

/// The six glyphs of a line set, in the order across, down, and the four corners from the top left
/// clockwise. `None` for `none` (no border at all).
pub fn line_glyphs(lines: &str) -> Option<[char; 6]> {
    match lines {
        "single" => Some(['─', '│', '┌', '┐', '┘', '└']),
        "rounded" => Some(['─', '│', '╭', '╮', '╯', '╰']),
        "double" => Some(['═', '║', '╔', '╗', '╝', '╚']),
        "heavy" => Some(['━', '┃', '┏', '┓', '┛', '┗']),
        "simple" => Some(['-', '|', '+', '+', '+', '+']),
        "padded" => Some([' '; 6]),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Item,
    Disabled,
    Separator,
}

/// One row of a menu: its text, the key shown on its right (`(k)`), what it is, and whether it is
/// the chosen one.
pub struct MenuRow<'a> {
    pub label: &'a str,
    pub key: &'a str,
    pub kind: RowKind,
    pub selected: bool,
}

/// Where a menu sits and how it looks.
pub struct MenuLook<'a> {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub title: &'a str,
    pub lines: &'a str,
    pub base: Style,
    pub chosen: Style,
    pub border: Style,
}

/// Draw a menu box at its own place: a border from the line set (none for `none`), the title in
/// the top border, one row per item with its key at the right edge, a rule for a separator, dim
/// text for a disabled item. Cells outside the screen are left alone; the cursor is hidden.
pub fn draw_menu(screen: &mut Screen, look: &MenuLook, rows: &[MenuRow]) {
    let glyphs = line_glyphs(look.lines);
    let pad = usize::from(glyphs.is_some());
    for dy in 0..look.h {
        for dx in 0..look.w {
            put(screen, look.x + dx, look.y + dy, ' ', look.base);
        }
    }
    if let Some([across, down, tl, tr, br, bl]) = glyphs {
        let (right, bottom) = (look.w - 1, look.h - 1);
        for dx in 0..look.w {
            put(screen, look.x + dx, look.y, across, look.border);
            put(screen, look.x + dx, look.y + bottom, across, look.border);
        }
        for dy in 0..look.h {
            put(screen, look.x, look.y + dy, down, look.border);
            put(screen, look.x + right, look.y + dy, down, look.border);
        }
        put(screen, look.x, look.y, tl, look.border);
        put(screen, look.x + right, look.y, tr, look.border);
        put(screen, look.x + right, look.y + bottom, br, look.border);
        put(screen, look.x, look.y + bottom, bl, look.border);
        for (i, ch) in look.title.chars().filter(|c| c.width() == Some(1)).take(look.w.saturating_sub(4)).enumerate() {
            put(screen, look.x + 2 + i, look.y, ch, look.border);
        }
    }
    let inner = look.w.saturating_sub(2 * pad);
    for (row, item) in rows.iter().enumerate() {
        let y = look.y + pad + row;
        if y >= look.y + look.h - pad {
            break;
        }
        let style = match (item.selected, item.kind) {
            (true, _) => look.chosen,
            (_, RowKind::Disabled) => Style { attrs: look.base.attrs | DIM, ..look.base },
            _ => look.base,
        };
        if item.kind == RowKind::Separator {
            let rule = glyphs.map_or('-', |g| if g[0] == ' ' { ' ' } else { g[0] });
            for dx in 0..inner {
                put(screen, look.x + pad + dx, y, rule, look.border);
            }
            continue;
        }
        for dx in 0..inner {
            put(screen, look.x + pad + dx, y, ' ', style);
        }
        let mut x = look.x + pad + 1;
        for ch in item.label.chars().filter(|c| c.width() == Some(1)) {
            if x + 1 > look.x + pad + inner {
                break;
            }
            put(screen, x, y, ch, style);
            x += 1;
        }
        if !item.key.is_empty() {
            let shown = format!("({})", item.key);
            let at = (look.x + pad + inner).saturating_sub(1 + shown.chars().count());
            for (i, ch) in shown.chars().enumerate() {
                if at + i > x {
                    put(screen, at + i, y, ch, style);
                }
            }
        }
    }
    screen.cursor = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::compose;
    use crate::term::Terminal;

    fn items(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("item {i}")).collect()
    }

    fn screen(cols: usize, rows: usize) -> Screen {
        let t = Terminal::new(cols, rows - 1, 0);
        compose(&t.grid, cols, rows, "")
    }

    fn row(s: &Screen, y: usize) -> String {
        s.cells[y * s.cols..(y + 1) * s.cols].iter().filter(|c| c.width != 0).map(|c| c.ch).collect()
    }

    #[test]
    fn a_box_drawn_below_a_top_bar_sits_in_the_rows_under_it() {
        let mut top = screen(40, 12);
        draw_at(&mut top, 1, 11, "t", "", &items(3), 0);
        let mut plain = screen(40, 12);
        draw(&mut plain, 11, "t", &items(3), 0);
        assert!(row(&top, 0).trim().is_empty(), "the bar row is left alone");
        let rows_of = |s: &Screen| (0..12).filter(|&y| row(s, y).contains('┌') || row(s, y).contains('└')).collect::<Vec<_>>();
        let (a, b) = (rows_of(&top), rows_of(&plain));
        assert_eq!(a.iter().map(|y| y - 1).collect::<Vec<_>>(), b, "the same box, one row lower");
    }

    #[test]
    fn box_is_centered_and_sized_to_the_items() {
        let g = geometry(80, 23, &items(3), 0);
        assert_eq!((g.w, g.h), (MIN_WIDTH, 5));
        assert_eq!(g.x, (80 - MIN_WIDTH) / 2);
        assert_eq!(g.y, (23 - 5) / 2);
    }

    #[test]
    fn long_lists_are_capped_to_the_pane_area_and_scroll_by_page() {
        let g = geometry(80, 12, &items(40), 0);
        assert_eq!(g.visible, 10);
        assert_eq!(g.h, 12);
        assert_eq!(geometry(80, 12, &items(40), 9).top, 0);
        assert_eq!(geometry(80, 12, &items(40), 10).top, 10);
        assert_eq!(geometry(80, 12, &items(40), 39).top, 30);
    }

    #[test]
    fn clicks_map_to_items_borders_and_outside() {
        let list = items(3);
        let g = geometry(80, 23, &list, 0);
        assert!(matches!(hit(&g, 3, g.x + 3, g.y + 1), Hit::Item(0)));
        assert!(matches!(hit(&g, 3, g.x + 3, g.y + 3), Hit::Item(2)));
        assert!(matches!(hit(&g, 3, g.x + 3, g.y), Hit::Border));
        assert!(matches!(hit(&g, 3, g.x, g.y + 1), Hit::Border));
        assert!(matches!(hit(&g, 3, 0, 0), Hit::Outside));
    }

    #[test]
    fn drawing_puts_the_title_items_and_selection_in_the_box() {
        let mut s = screen(60, 12);
        let list = items(2);
        draw(&mut s, 11, "choose", &list, 1);
        let g = geometry(60, 11, &list, 1);
        assert!(row(&s, g.y).contains("choose"));
        assert!(row(&s, g.y + 1).contains("item 0"));
        assert!(row(&s, g.y + 2).contains("item 1"));
        assert_eq!(s.cells[(g.y + 2) * 60 + g.x + 2].style.bg, BLUE);
        assert_eq!(s.cells[(g.y + 1) * 60 + g.x + 2].style.bg, BAR);
        assert!(s.cursor.is_none());
    }

    #[test]
    fn big_clock_rows_are_five_high_and_as_wide_as_their_glyphs() {
        let rows = clock_rows("14:05");
        assert!(rows.iter().all(|r| r.chars().count() == 25), "{rows:?}");
        assert_eq!(clock_rows("0")[0], "#####");
        assert_eq!(clock_rows(":").map(|r| r.len()), [1; 5]);
        assert_eq!(clock_rows("x"), <[String; 5]>::default());
        for d in 0..10 {
            let r = clock_rows(&d.to_string());
            assert!(r.iter().all(|l| l.chars().count() == 5) && r.iter().any(|l| l.contains('#')), "{d}");
        }
    }

    #[test]
    fn the_clock_fills_the_pane_with_big_digits_and_clears_it_first() {
        let mut s = screen(60, 12);
        put(&mut s, 12, 3, 'x', Style::default());
        draw_clock(&mut s, (10, 2, 40, 8), "12:34", "PM", BLUE);
        assert_eq!(s.cells[3 * 60 + 12].ch, ' ', "the old text is gone");
        let lit = s.cells.iter().filter(|c| c.style.bg == BLUE).count();
        assert!(lit > 40, "{lit}");
        assert!(row(&s, 8).contains("PM"), "{}", row(&s, 8));
        assert!(s.cursor.is_none());
    }

    #[test]
    fn a_small_pane_gets_plain_text_and_a_tiny_one_nothing() {
        let mut s = screen(60, 12);
        draw_clock(&mut s, (0, 0, 10, 3), "09:15", "", BLUE);
        assert!(row(&s, 1).contains("09:15"));
        assert_eq!(s.cells.iter().filter(|c| c.style.bg == BLUE).count(), 0);
        let mut s = screen(60, 12);
        draw_clock(&mut s, (0, 0, 4, 1), "09:15", "", BLUE);
        assert!(!row(&s, 0).contains("09"));
    }
}
