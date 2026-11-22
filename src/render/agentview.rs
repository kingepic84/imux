//! The agent panel: one row per agent pane (glyph, name, message, age) and, optionally, the last
//! lines of the selected pane. Drawn as a box over the panes (overlay) or over the whole screen
//! (full). Geometry is pure so drawing and mouse hits agree.

use unicode_width::UnicodeWidthChar;

use super::compositor::Screen;
use super::status::{BAR, BLUE, DARK, TEXT};
use crate::term::cell::{BOLD, Cell, Style};

const MIN_OVERLAY_W: usize = 40;
const MIN_OVERLAY_H: usize = 8;
/// A full view this wide puts the preview beside the list instead of under it.
const SIDE_BY_SIDE_FROM: usize = 110;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A box over the panes, `w` and `h` percent of the area.
    Overlay { w: usize, h: usize },
    /// The whole screen.
    Full,
}

/// One list row, already reduced to text.
#[derive(Clone, Debug)]
pub struct Row {
    pub glyph: String,
    pub glyph_style: Style,
    pub name: String,
    pub msg: String,
    pub age: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub frame: Area,
    /// Where list rows go (inside the frame).
    pub list: Area,
    pub preview: Option<Area>,
    /// First row shown; the list scrolls a page at a time so clicks and drawing agree.
    pub top: usize,
    pub visible: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Hit {
    Row(usize),
    Outside,
    Border,
}

fn width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(1).max(1)).sum()
}

/// Lay the panel out on a `cols` x `rows` screen for `count` rows with `selected` chosen;
/// `preview_lines` is 0 for no preview.
pub fn geometry(shape: Shape, cols: usize, rows: usize, count: usize, selected: usize, preview_lines: usize) -> Geometry {
    let frame = match shape {
        Shape::Overlay { w, h } => {
            let fw = (cols * w / 100).max(MIN_OVERLAY_W).min(cols);
            let fh = (rows * h / 100).max(MIN_OVERLAY_H).min(rows);
            Area { x: (cols - fw) / 2, y: (rows - fh) / 2, w: fw, h: fh }
        }
        Shape::Full => Area { x: 0, y: 0, w: cols, h: rows },
    };
    let inner = Area { x: frame.x + 1, y: frame.y + 1, w: frame.w.saturating_sub(2), h: frame.h.saturating_sub(2) };
    let side = shape == Shape::Full && cols >= SIDE_BY_SIDE_FROM && preview_lines > 0;
    let (list, preview) = if preview_lines == 0 || inner.h < 6 {
        (inner, None)
    } else if side {
        let lw = inner.w * 6 / 10;
        (
            Area { w: lw, ..inner },
            Some(Area { x: inner.x + lw + 1, y: inner.y, w: inner.w - lw - 1, h: inner.h }),
        )
    } else {
        let ph = preview_lines.min(inner.h / 2);
        let lh = inner.h - ph - 1;
        (Area { h: lh, ..inner }, Some(Area { x: inner.x, y: inner.y + lh + 1, w: inner.w, h: ph }))
    };
    let visible = list.h.max(1);
    let top = (selected.min(count.saturating_sub(1)) / visible) * visible;
    Geometry { frame, list, preview, top, visible }
}

pub fn hit(g: &Geometry, count: usize, x: usize, y: usize) -> Hit {
    let f = g.frame;
    if x < f.x || x >= f.x + f.w || y < f.y || y >= f.y + f.h {
        return Hit::Outside;
    }
    let l = g.list;
    if x >= l.x && x < l.x + l.w && y >= l.y && y < l.y + l.h {
        let index = g.top + (y - l.y);
        if index < count {
            return Hit::Row(index);
        }
    }
    Hit::Border
}

fn put(screen: &mut Screen, x: usize, y: usize, ch: char, style: Style) {
    if x < screen.cols && y < screen.rows {
        screen.cells[y * screen.cols + x] = Cell { ch, width: 1, style, tile: 0, mark: 0 };
    }
}

/// Write `text` from `(x, y)` up to `limit` columns, one cell per character (wide characters
/// become `?`); returns the next column.
fn text_at(screen: &mut Screen, x: usize, y: usize, limit: usize, text: &str, style: Style) -> usize {
    let mut x = x;
    for ch in text.chars() {
        if x >= limit {
            break;
        }
        let ch = if ch.is_control() || ch.width().unwrap_or(1) != 1 { '?' } else { ch };
        put(screen, x, y, ch, style);
        x += 1;
    }
    x
}

/// Draw the panel; `rows` is the whole list, `selected` an index into it. `preview_lines` rows are
/// set aside for the selected pane's text (0: none), of which `preview` is the last part. The
/// cursor is hidden.
pub fn draw(screen: &mut Screen, shape: Shape, title: &str, rows: &[Row], selected: usize, preview_lines: usize, preview: &[String]) {
    let g = geometry(shape, screen.cols, screen.rows, rows.len(), selected, preview_lines);
    let border = Style { fg: BLUE, bg: BAR, attrs: 0 };
    let plain = Style { fg: TEXT, bg: BAR, attrs: 0 };
    let dim = Style { fg: TEXT, bg: BAR, attrs: crate::term::cell::DIM };
    let f = g.frame;
    for dy in 0..f.h {
        for dx in 0..f.w {
            let (top, bottom, left, right) = (dy == 0, dy == f.h - 1, dx == 0, dx == f.w - 1);
            let ch = match (top, bottom, left, right) {
                (true, _, true, _) => '┌',
                (true, _, _, true) => '┐',
                (_, true, true, _) => '└',
                (_, true, _, true) => '┘',
                (true, ..) | (_, true, ..) => '─',
                (_, _, true, _) | (_, _, _, true) => '│',
                _ => ' ',
            };
            put(screen, f.x + dx, f.y + dy, ch, if ch == ' ' { plain } else { border });
        }
    }
    text_at(screen, f.x + 2, f.y, f.x + f.w.saturating_sub(2), title, border);
    if let Some(p) = g.preview {
        let divider = if g.list.x == p.x { None } else { Some(p.x - 1) };
        match divider {
            Some(x) => (p.y..p.y + p.h).for_each(|y| put(screen, x, y, '│', border)),
            None => (p.x..p.x + p.w).for_each(|x| put(screen, x, p.y - 1, '─', border)),
        }
        let skip = preview.len().saturating_sub(p.h);
        for (i, line) in preview.iter().skip(skip).enumerate() {
            text_at(screen, p.x + 1, p.y + i, p.x + p.w, line, dim);
        }
    }
    draw_rows(screen, g.list, rows, Some(selected), g.top);
    screen.cursor = None;
}

/// The list rows from index `top` into `list`, one per screen row; `selected` is highlighted.
fn draw_rows(screen: &mut Screen, list: Area, rows: &[Row], selected: Option<usize>, top: usize) {
    let plain = Style { fg: TEXT, bg: BAR, attrs: 0 };
    let dim = Style { fg: TEXT, bg: BAR, attrs: crate::term::cell::DIM };
    let chosen = Style { fg: DARK, bg: BLUE, attrs: BOLD };
    let name_w = rows.iter().map(|r| width(&r.name)).max().unwrap_or(0).min(list.w / 3).max(4);
    let age_w = rows.iter().map(|r| width(&r.age)).max().unwrap_or(0);
    for line in 0..list.h {
        let index = top + line;
        let Some(row) = rows.get(index) else { break };
        let selected_row = selected == Some(index);
        let base = if selected_row { chosen } else { plain };
        let y = list.y + line;
        let (left, right) = (list.x, list.x + list.w);
        for x in left..right {
            put(screen, x, y, ' ', base);
        }
        let glyph_style = if selected_row { chosen } else { row.glyph_style };
        text_at(screen, left + 1, y, right, &row.glyph, glyph_style);
        let x = left + 3;
        let name_end = (x + name_w).min(right);
        text_at(screen, x, y, name_end, &row.name, base);
        let age_start = right.saturating_sub(age_w + 1);
        let msg_start = name_end + 2;
        if msg_start < age_start {
            text_at(screen, msg_start, y, age_start.saturating_sub(1), &row.msg, if selected_row { chosen } else { dim });
        }
        text_at(screen, age_start, y, right, &row.age, base);
    }
}

/// Which edge a dock is on, for where its divider goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// Where a dock's list rows go inside its rectangle: the divider takes the edge next to the
/// panes, and a title row heads a vertical dock.
pub fn dock_list(rect: Area, edge: Edge) -> Area {
    match edge {
        Edge::Left => Area { x: rect.x, y: rect.y + 1, w: rect.w.saturating_sub(1), h: rect.h.saturating_sub(1) },
        Edge::Right => Area { x: rect.x + 1, y: rect.y + 1, w: rect.w.saturating_sub(1), h: rect.h.saturating_sub(1) },
        Edge::Top => Area { x: rect.x, y: rect.y, w: rect.w, h: rect.h.saturating_sub(1) },
        Edge::Bottom => Area { x: rect.x, y: rect.y + 1, w: rect.w, h: rect.h.saturating_sub(1) },
    }
}

/// First row index shown when `selected` is chosen and `visible` rows fit.
fn page_top(selected: usize, count: usize, visible: usize) -> usize {
    (selected.min(count.saturating_sub(1)) / visible.max(1)) * visible.max(1)
}

/// The row under `(x, y)` in a dock whose selection is `selected`.
pub fn dock_hit(rect: Area, edge: Edge, count: usize, selected: usize, x: usize, y: usize) -> Option<usize> {
    let list = dock_list(rect, edge);
    if x < list.x || x >= list.x + list.w || y < list.y || y >= list.y + list.h {
        return None;
    }
    let index = page_top(selected, count, list.h) + (y - list.y);
    (index < count).then_some(index)
}

/// Draw the dock into `rect`: a divider on the side of the panes, a title row on vertical docks
/// and the list. With `selected` the row is highlighted (the dock has the keyboard).
pub fn draw_dock(screen: &mut Screen, rect: Area, edge: Edge, title: &str, rows: &[Row], selected: Option<usize>) {
    let border = Style { fg: BLUE, bg: BAR, attrs: 0 };
    let plain = Style { fg: TEXT, bg: BAR, attrs: 0 };
    for y in rect.y..rect.y + rect.h {
        for x in rect.x..rect.x + rect.w {
            put(screen, x, y, ' ', plain);
        }
    }
    match edge {
        Edge::Left => (rect.y..rect.y + rect.h).for_each(|y| put(screen, rect.x + rect.w - 1, y, '│', border)),
        Edge::Right => (rect.y..rect.y + rect.h).for_each(|y| put(screen, rect.x, y, '│', border)),
        Edge::Top => (rect.x..rect.x + rect.w).for_each(|x| put(screen, x, rect.y + rect.h - 1, '─', border)),
        Edge::Bottom => (rect.x..rect.x + rect.w).for_each(|x| put(screen, x, rect.y, '─', border)),
    }
    let list = dock_list(rect, edge);
    if matches!(edge, Edge::Left | Edge::Right | Edge::Bottom) {
        text_at(screen, list.x + 1, list.y - 1, list.x + list.w, title, border);
    }
    draw_rows(screen, list, rows, selected, page_top(selected.unwrap_or(0), rows.len(), list.h));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::compose;
    use crate::term::Terminal;

    fn screen(cols: usize, rows: usize) -> Screen {
        let t = Terminal::new(cols, rows - 1, 0);
        compose(&t.grid, cols, rows, "")
    }

    fn line(s: &Screen, y: usize) -> String {
        s.cells[y * s.cols..(y + 1) * s.cols].iter().filter(|c| c.width != 0).map(|c| c.ch).collect()
    }

    fn row(name: &str, msg: &str) -> Row {
        Row { glyph: "*".into(), glyph_style: Style::default(), name: name.into(), msg: msg.into(), age: "3s".into() }
    }

    const OVERLAY: Shape = Shape::Overlay { w: 70, h: 60 };

    #[test]
    fn an_overlay_is_centered_and_sized_by_percent_with_a_floor() {
        let g = geometry(OVERLAY, 100, 40, 3, 0, 0);
        assert_eq!((g.frame.w, g.frame.h), (70, 24));
        assert_eq!((g.frame.x, g.frame.y), (15, 8));
        let small = geometry(OVERLAY, 30, 10, 3, 0, 0);
        assert_eq!((small.frame.w, small.frame.h), (30, 8));
    }

    #[test]
    fn the_full_shape_covers_the_screen() {
        let g = geometry(Shape::Full, 100, 30, 3, 0, 0);
        assert_eq!(g.frame, Area { x: 0, y: 0, w: 100, h: 30 });
        assert!(g.preview.is_none());
    }

    #[test]
    fn a_preview_goes_below_the_list_or_beside_it_on_a_wide_full_view() {
        let g = geometry(OVERLAY, 100, 40, 3, 0, 8);
        let p = g.preview.unwrap();
        assert_eq!(p.h, 8);
        assert!(p.y > g.list.y + g.list.h - 1);
        assert_eq!(p.x, g.list.x);
        let wide = geometry(Shape::Full, 140, 40, 3, 0, 8);
        let p = wide.preview.unwrap();
        assert!(p.x > wide.list.x + wide.list.w - 1);
        assert_eq!(p.h, wide.list.h);
        let narrow = geometry(Shape::Full, 100, 40, 3, 0, 8);
        assert!(narrow.preview.unwrap().x == narrow.list.x);
    }

    #[test]
    fn the_list_scrolls_by_page_and_clicks_agree_with_drawing() {
        let g = geometry(OVERLAY, 100, 20, 50, 0, 0);
        assert_eq!(g.visible, g.list.h);
        let second = geometry(OVERLAY, 100, 20, 50, g.visible, 0);
        assert_eq!(second.top, g.visible);
        assert_eq!(hit(&g, 50, g.list.x + 2, g.list.y), Hit::Row(0));
        assert_eq!(hit(&second, 50, g.list.x + 2, g.list.y + 1), Hit::Row(g.visible + 1));
        assert_eq!(hit(&g, 2, g.list.x + 2, g.list.y + 5), Hit::Border);
        assert_eq!(hit(&g, 50, 0, 0), Hit::Outside);
        assert_eq!(hit(&g, 50, g.frame.x, g.frame.y), Hit::Border);
    }

    #[test]
    fn drawing_puts_title_rows_selection_and_preview_in_the_box() {
        let mut s = screen(100, 30);
        let rows = [row("ABC-1", "approve Bash"), row("ABC-2", "working")];
        draw(&mut s, OVERLAY, "agents", &rows, 1, 2, &["$ ls".to_string(), "file".to_string()]);
        let g = geometry(OVERLAY, 100, 30, 2, 1, 2);
        assert!(line(&s, g.frame.y).contains("agents"));
        assert!(line(&s, g.list.y).contains("ABC-1"));
        assert!(line(&s, g.list.y).contains("approve Bash"));
        assert!(line(&s, g.list.y + 1).contains("ABC-2"));
        assert!(line(&s, g.list.y + 1).contains("3s"));
        let sel = s.cells[(g.list.y + 1) * 100 + g.list.x + 2];
        assert_eq!(sel.style.bg, BLUE);
        assert_eq!(s.cells[g.list.y * 100 + g.list.x + 2].style.bg, BAR);
        let p = g.preview.unwrap();
        assert!(line(&s, p.y).contains("$ ls"));
        assert!(line(&s, p.y + 1).contains("file"));
        assert!(s.cursor.is_none());
    }

    const DOCK: Area = Area { x: 70, y: 0, w: 30, h: 20 };

    #[test]
    fn a_vertical_dock_keeps_a_divider_and_a_title_row() {
        assert_eq!(dock_list(DOCK, Edge::Right), Area { x: 71, y: 1, w: 29, h: 19 });
        let left = Area { x: 0, ..DOCK };
        assert_eq!(dock_list(left, Edge::Left), Area { x: 0, y: 1, w: 29, h: 19 });
        let bottom = Area { x: 0, y: 12, w: 100, h: 8 };
        assert_eq!(dock_list(bottom, Edge::Bottom), Area { x: 0, y: 13, w: 100, h: 7 });
        let top = Area { x: 0, y: 0, w: 100, h: 8 };
        assert_eq!(dock_list(top, Edge::Top), Area { x: 0, y: 0, w: 100, h: 7 });
    }

    #[test]
    fn dock_clicks_map_to_rows_and_scroll_with_the_selection() {
        assert_eq!(dock_hit(DOCK, Edge::Right, 3, 0, 75, 1), Some(0));
        assert_eq!(dock_hit(DOCK, Edge::Right, 3, 0, 75, 3), Some(2));
        assert_eq!(dock_hit(DOCK, Edge::Right, 3, 0, 75, 4), None);
        assert_eq!(dock_hit(DOCK, Edge::Right, 3, 0, 70, 1), None);
        assert_eq!(dock_hit(DOCK, Edge::Right, 3, 0, 75, 0), None);
        assert_eq!(dock_hit(DOCK, Edge::Right, 3, 0, 10, 1), None);
        assert_eq!(dock_hit(DOCK, Edge::Right, 50, 25, 75, 1), Some(19));
    }

    #[test]
    fn drawing_a_dock_fills_its_rectangle_and_leaves_the_rest_alone() {
        let mut s = screen(100, 30);
        let before = s.cells[5 * 100 + 10];
        draw_dock(&mut s, DOCK, Edge::Right, "agents", &[row("ABC-1", "approve Bash"), row("ABC-2", "working")], Some(1));
        assert_eq!(s.cells[5 * 100 + 10], before);
        assert_eq!(s.cells[5 * 100 + 70].ch, '│');
        assert!(line(&s, 0).contains("agents"));
        assert!(line(&s, 1).contains("ABC-1"));
        assert!(line(&s, 2).contains("ABC-2"));
        assert_eq!(s.cells[2 * 100 + 73].style.bg, BLUE);
        assert_eq!(s.cells[100 + 73].style.bg, BAR);
        draw_dock(&mut s, DOCK, Edge::Right, "agents", &[row("ABC-1", "x")], None);
        assert_eq!(s.cells[100 + 73].style.bg, BAR);
    }

    #[test]
    fn a_horizontal_dock_draws_its_divider_on_the_side_of_the_panes() {
        let mut s = screen(100, 30);
        let top = Area { x: 0, y: 0, w: 100, h: 6 };
        draw_dock(&mut s, top, Edge::Top, "agents", &[row("a", "b")], None);
        assert_eq!(s.cells[5 * 100 + 4].ch, '─');
        let bottom = Area { x: 0, y: 20, w: 100, h: 6 };
        draw_dock(&mut s, bottom, Edge::Bottom, "agents", &[row("a", "b")], None);
        assert_eq!(s.cells[20 * 100 + 50].ch, '─');
        assert!(line(&s, 20).contains("agents"));
        assert!(line(&s, 21).contains("a"));
    }

    #[test]
    fn an_empty_list_and_odd_text_do_not_panic() {
        let mut s = screen(60, 12);
        draw(&mut s, OVERLAY, "agents", &[], 0, 0, &[]);
        let weird = Row { glyph: "\u{4e2d}".into(), glyph_style: Style::default(), name: "a\nb".into(), msg: "\u{1b}[31m".into(), age: String::new() };
        draw(&mut s, Shape::Full, "agents", &[weird], 5, 8, &["x".repeat(500)]);
        let mut tiny = screen(10, 4);
        draw(&mut tiny, OVERLAY, "agents", &[row("n", "m")], 0, 3, &["p".to_string()]);
    }
}
