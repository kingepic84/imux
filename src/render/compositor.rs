use std::collections::HashMap;
use std::sync::Arc;

#[cfg(test)]
use unicode_width::UnicodeWidthChar;

use super::quad;
use crate::copymode::View;
use crate::image::Bitmap;
use crate::proto::ImageMode;
use crate::session::layout::{Rect, Separator};
#[cfg(test)]
use crate::term::cell::REVERSE;
use crate::term::cell::{Cell, Color, DIM, Style};
use crate::term::grid::{Grid, MouseMode};

/// Pane modes the outer terminal must mirror so its input encoding matches what the pane's
/// program expects. Temporary bridge: M7 replaces raw forwarding with a real key/mouse parser.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PaneModes {
    pub cursor_keys: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub app_keypad: bool,
    pub mouse: MouseMode,
    pub mouse_sgr: bool,
}

/// One cell's worth of an image that a client draws itself (`ImageMode::Iterm2`). A screen cell
/// with `tile != 0` shows `pieces[tile - 1]`; tile ids of different panes never clash this way.
#[derive(Clone)]
pub struct Piece {
    pub bitmap: Arc<Bitmap>,
    /// Footprint of the whole image in cells, and which cell of it this is.
    pub cols: usize,
    pub rows: usize,
    pub tx: u16,
    pub ty: u16,
}

impl Piece {
    /// Pieces of the same image share the bitmap.
    pub fn same_image(&self, other: &Piece) -> bool {
        Arc::ptr_eq(&self.bitmap, &other.bitmap) && self.cols == other.cols && self.rows == other.rows
    }

    pub fn same_cell(&self, other: &Piece) -> bool {
        self.same_image(other) && (self.tx, self.ty) == (other.tx, other.ty)
    }
}

#[derive(Clone)]
pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Option<(usize, usize)>,
    pub cursor_style: u16,
    pub modes: PaneModes,
    pub pieces: Vec<Piece>,
    /// How the pieces are to be drawn.
    pub images: ImageMode,
    /// Rows `(first, last)` of each pane that spans the whole width of the screen: the places where
    /// the terminal itself can scroll the cells (pictures move with them) instead of every picture
    /// being sent again.
    pub scroll_areas: Vec<(usize, usize)>,
}

impl Screen {
    /// The image piece shown at a cell, `None` for text (also where an overlay covers an image).
    pub fn piece(&self, x: usize, y: usize) -> Option<&Piece> {
        let tile = self.cells.get(y * self.cols + x)?.tile;
        self.pieces.get((tile as usize).checked_sub(1)?)
    }
}

/// Stands in for a cell whose image data was dropped to stay inside the memory budget.
fn placeholder() -> Cell {
    Cell { ch: '░', style: Style { fg: Color::Indexed(8), bg: Color::Default, attrs: DIM }, ..Cell::BLANK }
}

/// What to put on the screen for a cell holding an image tile.
fn image_cell(grid: &Grid, cell: Cell, mode: ImageMode, pane: usize, pieces: &mut Vec<Piece>, seen: &mut HashMap<(usize, u32), u32>) -> Cell {
    let Some(tile) = grid.images.lookup(cell.tile) else { return placeholder() };
    let Some(placed) = grid.images.image(tile.image) else { return placeholder() };
    match mode {
        ImageMode::Blocks => {
            let mut out = quad::tile_cell(&placed.bitmap, placed.cols, placed.rows, usize::from(tile.tx), usize::from(tile.ty));
            out.style.attrs |= cell.style.attrs;
            out
        }
        ImageMode::Iterm2 | ImageMode::Kitty | ImageMode::Sixel => {
            let id = *seen.entry((pane, cell.tile)).or_insert_with(|| {
                pieces.push(Piece { bitmap: placed.bitmap.clone(), cols: placed.cols, rows: placed.rows, tx: tile.tx, ty: tile.ty });
                pieces.len() as u32
            });
            Cell { tile: id, ..Cell::BLANK }
        }
    }
}

fn filler() -> Cell {
    Cell { ch: '.', style: Style { fg: Color::Indexed(8), bg: Color::Default, attrs: DIM }, ..Cell::BLANK }
}

/// One pane as the compositor sees it: where it sits and what it holds.
pub struct PaneView<'a> {
    pub rect: Rect,
    pub grid: &'a Grid,
    pub active: bool,
    /// Copy mode's picture of the pane, shown instead of the live screen.
    pub copy: Option<&'a View>,
}

#[cfg(test)]
pub fn compose(grid: &Grid, cols: usize, rows: usize, status: &str) -> Screen {
    let rect = Rect { x: 0, y: 0, w: grid.cols as u16, h: grid.rows as u16 };
    compose_window(&[PaneView { rect, grid, active: true, copy: None }], &[], cols, rows, &plain_status(status, cols), 1)
}

/// The default `pane-border-style` and `pane-active-border-style` colors, for the tests.
#[cfg(test)]
const BORDER: Color = Color::Rgb(88, 91, 112);
#[cfg(test)]
const BORDER_ACTIVE: Color = Color::Rgb(137, 180, 250);

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// The characters pane dividers are drawn with (`pane-border-lines`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BorderLines {
    #[default]
    Single,
    Double,
    Heavy,
    Simple,
    Spaces,
}

impl BorderLines {
    /// The line set an option value names; anything else is the single line.
    pub fn from_name(name: &str) -> Self {
        match name {
            "double" => Self::Double,
            "heavy" => Self::Heavy,
            "simple" => Self::Simple,
            "spaces" => Self::Spaces,
            _ => Self::Single,
        }
    }

    /// The character of a horizontal divider.
    pub fn across(self) -> char {
        self.set()[0]
    }

    /// Across, down, cross, tee right, tee left, tee down, tee up, and the corners up-right,
    /// up-left, down-right, down-left.
    fn set(self) -> [char; 11] {
        match self {
            Self::Single => ['─', '│', '┼', '├', '┤', '┬', '┴', '└', '┘', '┌', '┐'],
            Self::Double => ['═', '║', '╬', '╠', '╣', '╦', '╩', '╚', '╝', '╔', '╗'],
            Self::Heavy => ['━', '┃', '╋', '┣', '┫', '┳', '┻', '┗', '┛', '┏', '┓'],
            Self::Simple => ['-', '|', '+', '+', '+', '+', '+', '+', '+', '+', '+'],
            Self::Spaces => [' '; 11],
        }
    }
}

/// How panes look beyond the dividers' colors: the base styles `(not active, active)` their default
/// cell colors come from (`window-style`, `window-active-style`) and the divider characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub bases: (Style, Style),
    pub lines: BorderLines,
    pub status: PaneStatus,
}

/// Where a pane has its label row (`pane-border-status`): the first or the last row of its rect,
/// which its program does not get.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneStatus {
    #[default]
    Off,
    Top,
    Bottom,
}

impl PaneStatus {
    pub fn from_name(name: &str) -> Self {
        match name {
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            _ => Self::Off,
        }
    }

    /// The rect a program gets out of `rect`; a rect of one row has no label row.
    pub fn content(self, rect: Rect) -> Rect {
        match self {
            Self::Top if rect.h >= 2 => Rect { y: rect.y + 1, h: rect.h - 1, ..rect },
            Self::Bottom if rect.h >= 2 => Rect { h: rect.h - 1, ..rect },
            _ => rect,
        }
    }

    /// The screen row of the label of a pane whose rect is `rect`, if it has one.
    pub fn label_row(self, rect: Rect) -> Option<usize> {
        match self {
            Self::Top if rect.h >= 2 => Some(usize::from(rect.y)),
            Self::Bottom if rect.h >= 2 => Some(usize::from(rect.y) + usize::from(rect.h) - 1),
            _ => None,
        }
    }
}

fn border_glyph(mask: u8, lines: BorderLines) -> char {
    let [across, down, cross, right, left, tee_down, tee_up, up_right, up_left, down_right, down_left] = lines.set();
    match mask {
        m if m == UP | DOWN | LEFT | RIGHT => cross,
        m if m == UP | DOWN | RIGHT => right,
        m if m == UP | DOWN | LEFT => left,
        m if m == LEFT | RIGHT | DOWN => tee_down,
        m if m == LEFT | RIGHT | UP => tee_up,
        m if m == UP | RIGHT => up_right,
        m if m == UP | LEFT => up_left,
        m if m == DOWN | RIGHT => down_right,
        m if m == DOWN | LEFT => down_left,
        m if m & (LEFT | RIGHT) != 0 => across,
        _ => down,
    }
}

/// Junction bits for every separator cell inside `cols` x `rows`.
fn border_masks(seps: &[Separator], cols: usize, rows: usize) -> Vec<u8> {
    let mut mask = vec![0u8; cols * rows];
    let mut set = |x: usize, y: usize, bits: u8, only_borders: bool| {
        if x < cols && y < rows && (!only_borders || mask[y * cols + x] != 0) {
            mask[y * cols + x] |= bits;
        }
    };
    for s in seps {
        for i in 0..usize::from(s.len) {
            if s.vertical {
                set(usize::from(s.x), usize::from(s.y) + i, UP | DOWN, false);
            } else {
                set(usize::from(s.x) + i, usize::from(s.y), LEFT | RIGHT, false);
            }
        }
    }
    // Where a divider ends against another one, the cell it ends on gains a branch toward it.
    for s in seps {
        let (x, y, len) = (usize::from(s.x), usize::from(s.y), usize::from(s.len));
        if s.vertical {
            if y > 0 {
                set(x, y - 1, DOWN, true);
            }
            set(x, y + len, UP, true);
        } else {
            if x > 0 {
                set(x - 1, y, RIGHT, true);
            }
            set(x + len, y, LEFT, true);
        }
    }
    mask
}

fn touches(r: Rect, x: usize, y: usize) -> bool {
    let (rx, ry, rw, rh) = (usize::from(r.x), usize::from(r.y), usize::from(r.w), usize::from(r.h));
    let in_cols = x + 1 >= rx && x <= rx + rw;
    let in_rows = y + 1 >= ry && y <= ry + rh;
    let beside = (y >= ry && y < ry + rh) && (x + 1 == rx || x == rx + rw);
    let above = (x >= rx && x < rx + rw) && (y + 1 == ry || y == ry + rh);
    in_cols && in_rows && (beside || above)
}

/// Build one client's screen: panes at their rects, dividers between them, filler where the
/// client is larger than the window, and `status_rows` rows of status cells (row-major, `cols`
/// wide) at the bottom.
#[cfg(test)]
pub fn compose_window(
    panes: &[PaneView],
    seps: &[Separator],
    cols: usize,
    rows: usize,
    status: &[Cell],
    status_rows: usize,
) -> Screen {
    let borders = (Style { fg: BORDER, ..Style::default() }, Style { fg: BORDER_ACTIVE, ..Style::default() });
    compose_window_with(panes, seps, cols, rows, status, status_rows, borders, ImageMode::Blocks)
}

/// `compose_window` with the divider styles (`pane-border-style`, `pane-active-border-style`) and
/// the way the client shows images.
#[allow(clippy::too_many_arguments)]
pub fn compose_window_with(
    panes: &[PaneView],
    seps: &[Separator],
    cols: usize,
    rows: usize,
    status: &[Cell],
    status_rows: usize,
    borders: (Style, Style),
    images: ImageMode,
) -> Screen {
    compose_window_placed(panes, seps, cols, rows, status, status_rows, false, borders, images)
}

/// `compose_window_with` with the status rows at the top of the screen when `status_top` is set
/// (`status-position top`). Pane rects are screen rows either way.
#[allow(clippy::too_many_arguments)]
pub fn compose_window_placed(
    panes: &[PaneView],
    seps: &[Separator],
    cols: usize,
    rows: usize,
    status: &[Cell],
    status_rows: usize,
    status_top: bool,
    borders: (Style, Style),
    images: ImageMode,
) -> Screen {
    compose_window_popup(panes, seps, cols, rows, status, status_rows, status_top, borders, images, None)
}

/// A popup box drawn over the panes: its border, its title and the program's screen inside.
pub struct PopupView<'a> {
    /// Left, top, width and height of the whole box, in screen cells (rows counted from the top of
    /// the screen, not of the part the panes use).
    pub rect: (usize, usize, usize, usize),
    /// The program's screen, which fills the inside of the border.
    pub grid: &'a Grid,
    pub lines: &'a str,
    pub title: &'a str,
    /// A short text drawn by the server in the top border, at its right (who opened the popup).
    pub marker: &'a str,
    pub border: Style,
    /// Style of the cells under the program's own (the popup's `-s`), used where it printed nothing.
    pub base: Style,
}

/// `compose_window_placed` with a popup over the panes. The popup takes the cursor and the
/// terminal modes of the active pane while it is open, and nothing scrolls the screen.
#[allow(clippy::too_many_arguments)]
pub fn compose_window_popup(
    panes: &[PaneView],
    seps: &[Separator],
    cols: usize,
    rows: usize,
    status: &[Cell],
    status_rows: usize,
    status_top: bool,
    borders: (Style, Style),
    images: ImageMode,
    popup: Option<&PopupView>,
) -> Screen {
    compose_window_full(panes, seps, cols, rows, status, status_rows, status_top, borders, Look::default(), images, popup)
}

/// A pane cell seen through the pane's base style (`window-style`, `window-active-style`): a color
/// the program did not choose (the terminal default) is the base's, and the base's attributes are
/// added.
fn under(cell: Style, base: Style) -> Style {
    Style {
        fg: if cell.fg == Color::Default { base.fg } else { cell.fg },
        bg: if cell.bg == Color::Default { base.bg } else { cell.bg },
        attrs: cell.attrs | base.attrs,
    }
}

/// `compose_window_popup` with the look of the panes: `look.bases.0` is the base style of a pane
/// that is not the active one, `look.bases.1` of the active one, `look.lines` the divider characters.
#[allow(clippy::too_many_arguments)]
pub fn compose_window_full(
    panes: &[PaneView],
    seps: &[Separator],
    cols: usize,
    rows: usize,
    status: &[Cell],
    status_rows: usize,
    status_top: bool,
    borders: (Style, Style),
    look: Look,
    images: ImageMode,
    popup: Option<&PopupView>,
) -> Screen {
    let mut cells = vec![filler(); cols * rows];
    let mut pieces = Vec::new();
    let mut seen = HashMap::new();
    let status_rows = status_rows.min(rows);
    // The rows the panes may use are `first_row..pane_end`.
    let first_row = if status_top { status_rows } else { 0 };
    let pane_end = first_row + (rows - status_rows);
    for (pane_index, pane) in panes.iter().enumerate() {
        let r = pane.rect;
        let base = if pane.active { look.bases.1 } else { look.bases.0 };
        let based = base != Style::default();
        let content = look.status.content(r);
        let shift = usize::from(content.y - r.y);
        for y in 0..usize::from(content.h).min(pane.grid.rows) {
            for x in 0..usize::from(r.w).min(pane.grid.cols) {
                let (px, py) = (usize::from(r.x) + x, usize::from(r.y) + shift + y);
                if px >= cols || py < first_row || py >= pane_end {
                    continue;
                }
                let mut cell = match pane.copy {
                    Some(view) => view.cells[y * pane.grid.cols + x],
                    None => pane.grid.screen[y].cells[x],
                };
                // A wide char clipped by the pane or client edge must not leave a half cell behind.
                if cell.width == 2 && (px + 1 >= cols || x + 1 >= usize::from(r.w)) {
                    cell = Cell::BLANK;
                }
                if cell.tile != 0 {
                    cell = image_cell(pane.grid, cell, images, pane_index, &mut pieces, &mut seen);
                } else if based {
                    cell.style = under(cell.style, base);
                }
                cells[py * cols + px] = cell;
            }
        }
    }

    let active = panes.iter().find(|p| p.active);
    let masks = border_masks(seps, cols, pane_end);
    for y in first_row..pane_end {
        for x in 0..cols {
            let mask = masks[y * cols + x];
            if mask == 0 {
                continue;
            }
            let hot = active.is_some_and(|p| touches(p.rect, x, y));
            let style = if hot { borders.1 } else { borders.0 };
            cells[y * cols + x] = Cell { ch: border_glyph(mask, look.lines), style, ..Cell::BLANK };
        }
    }
    let bar = if status_top { 0 } else { (rows - status_rows) * cols };
    let bar_end = bar + status_rows * cols;
    let n = status.len().min(bar_end - bar);
    cells[bar..bar + n].copy_from_slice(&status[..n]);
    cells[bar + n..bar_end].fill(Cell::BLANK);

    let mut cursor = None;
    let mut cursor_style = 0;
    let mut modes = PaneModes::default();
    if let Some(pv) = popup {
        let area = (first_row, pane_end);
        let drawn = draw_popup(&mut cells, cols, area, pv, images, panes.len(), &mut pieces, &mut seen);
        (cursor, cursor_style, modes) = drawn;
    } else if let Some(p) = active {
        let g = p.grid;
        let (cx, cy, visible) = match p.copy.and_then(|v| v.cursor) {
            Some((x, y)) => (x, y, true),
            None => (g.cx, g.cy, g.modes.cursor_visible && p.copy.is_none()),
        };
        let content = look.status.content(p.rect);
        let (px, py) = (usize::from(content.x) + cx, usize::from(content.y) + cy);
        let inside = cx < usize::from(content.w) && cy < usize::from(content.h) && px < cols && py >= first_row && py < pane_end;
        cursor = (visible && inside).then_some((px, py));
        cursor_style = g.cursor_style;
        let m = &g.modes;
        modes = PaneModes {
            cursor_keys: m.cursor_keys,
            bracketed_paste: m.bracketed_paste,
            focus_events: m.focus_events,
            app_keypad: m.app_keypad,
            mouse: m.mouse,
            mouse_sgr: m.mouse_sgr,
        };
    }
    let scroll_areas = panes
        .iter()
        .filter(|_| popup.is_none() && look.status == PaneStatus::Off)
        .filter(|p| p.rect.x == 0 && usize::from(p.rect.w) == cols && p.rect.h > 0 && usize::from(p.rect.y) >= first_row && usize::from(p.rect.y) < pane_end)
        .map(|p| (usize::from(p.rect.y), (usize::from(p.rect.y) + usize::from(p.rect.h)).min(pane_end) - 1))
        .collect();
    Screen { cols, rows, cells, cursor, cursor_style, modes, pieces, images, scroll_areas }
}

/// Draw a popup into `cells`; `area` is the first and the end row the panes may use. Returns the
/// cursor, its style and the modes of the popup's terminal.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn draw_popup(
    cells: &mut [Cell],
    cols: usize,
    area: (usize, usize),
    pv: &PopupView,
    images: ImageMode,
    pane_index: usize,
    pieces: &mut Vec<Piece>,
    seen: &mut HashMap<(usize, u32), u32>,
) -> (Option<(usize, usize)>, u16, PaneModes) {
    use unicode_width::UnicodeWidthChar;
    let (first_row, pane_end) = area;
    let (x0, y0, w, h) = pv.rect;
    let glyphs = super::overlay::line_glyphs(pv.lines);
    let pad = usize::from(glyphs.is_some());
    if w < 1 + 2 * pad || h < 1 + 2 * pad {
        return (None, 0, PaneModes::default());
    }
    let mut put = |x: usize, y: usize, cell: Cell| {
        if x < cols && y >= first_row && y < pane_end {
            cells[y * cols + x] = cell;
        }
    };
    let blank = Cell { style: pv.base, ..Cell::BLANK };
    for dy in 0..h {
        for dx in 0..w {
            put(x0 + dx, y0 + dy, blank);
        }
    }
    let edge = |ch: char| Cell { ch, style: pv.border, ..Cell::BLANK };
    if let Some([across, down, tl, tr, br, bl]) = glyphs {
        for dx in 0..w {
            put(x0 + dx, y0, edge(across));
            put(x0 + dx, y0 + h - 1, edge(across));
        }
        for dy in 0..h {
            put(x0, y0 + dy, edge(down));
            put(x0 + w - 1, y0 + dy, edge(down));
        }
        put(x0, y0, edge(tl));
        put(x0 + w - 1, y0, edge(tr));
        put(x0 + w - 1, y0 + h - 1, edge(br));
        put(x0, y0 + h - 1, edge(bl));
        let room = w.saturating_sub(4);
        for (i, ch) in pv.title.chars().filter(|c| c.width() == Some(1)).take(room).enumerate() {
            put(x0 + 2 + i, y0, edge(ch));
        }
        let marker: Vec<char> = pv.marker.chars().filter(|c| c.width() == Some(1)).collect();
        let used = pv.title.chars().count().min(room);
        if !marker.is_empty() && marker.len() + 2 <= room.saturating_sub(used) {
            let start = x0 + w - 2 - marker.len();
            for (i, ch) in marker.into_iter().enumerate() {
                put(start + i, y0, edge(ch));
            }
        }
    }
    let (ix, iy) = (x0 + pad, y0 + pad);
    let (iw, ih) = (w.saturating_sub(2 * pad), h.saturating_sub(2 * pad));
    let g = pv.grid;
    for y in 0..ih.min(g.rows) {
        for x in 0..iw.min(g.cols) {
            let mut cell = g.screen[y].cells[x];
            if cell.width == 2 && (x + 1 >= iw || ix + x + 1 >= cols) {
                cell = Cell::BLANK;
            }
            if cell.tile != 0 {
                cell = image_cell(g, cell, images, pane_index, pieces, seen);
            }
            if cell.style.bg == Color::Default && cell.ch == ' ' && cell.tile == 0 {
                cell.style = Style { fg: cell.style.fg, ..pv.base };
            }
            put(ix + x, iy + y, cell);
        }
    }
    let m = &g.modes;
    let modes = PaneModes {
        cursor_keys: m.cursor_keys,
        bracketed_paste: m.bracketed_paste,
        focus_events: m.focus_events,
        app_keypad: m.app_keypad,
        mouse: m.mouse,
        mouse_sgr: m.mouse_sgr,
    };
    let (cx, cy) = (ix + g.cx, iy + g.cy);
    let inside = g.cx < iw && g.cy < ih && cx < cols && cy >= first_row && cy < pane_end;
    let cursor = (m.cursor_visible && inside).then_some((cx, cy));
    (cursor, g.cursor_style, modes)
}

#[cfg(test)]
fn plain_status(text: &str, cols: usize) -> Vec<Cell> {
    let style = Style { attrs: REVERSE, ..Style::default() };
    let mut row = vec![Cell { style, ..Cell::BLANK }; cols];
    let mut x = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if w == 0 {
            continue;
        }
        if x + w > row.len() {
            break;
        }
        row[x] = Cell { ch, width: w as u8, style, tile: 0, mark: 0 };
        if w == 2 {
            row[x + 1] = Cell { ch: ' ', width: 0, style, tile: 0, mark: 0 };
        }
        x += w;
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::layout::{Layout, Split};
    use crate::term::Terminal;

    fn row(s: &Screen, y: usize) -> String {
        s.cells[y * s.cols..(y + 1) * s.cols].iter().filter(|c| c.width != 0).map(|c| c.ch).collect()
    }

    fn compose_layout(layout: &Layout, terms: &[(u32, Terminal)], active: u32, cols: usize, rows: usize) -> Screen {
        let area = Rect { x: 0, y: 0, w: cols as u16, h: rows as u16 - 1 };
        let views: Vec<PaneView> = layout
            .rects(area)
            .into_iter()
            .map(|(id, rect)| PaneView {
                rect,
                grid: &terms.iter().find(|(t, _)| *t == id).unwrap().1.grid,
                active: id == active,
                copy: None,
            })
            .collect();
        compose_window(&views, &layout.separators(area), cols, rows, &[], 1)
    }

    fn terms(layout: &Layout, cols: u16, rows: u16) -> Vec<(u32, Terminal)> {
        let area = Rect { x: 0, y: 0, w: cols, h: rows - 1 };
        layout
            .rects(area)
            .into_iter()
            .map(|(id, r)| {
                let mut t = Terminal::new(r.w as usize, r.h as usize, 0);
                t.feed(format!("p{id}").as_bytes());
                (id, t)
            })
            .collect()
    }

    #[test]
    fn side_by_side_panes_are_drawn_with_a_divider() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, Rect { x: 0, y: 0, w: 9, h: 3 }).unwrap();
        let s = compose_layout(&l, &terms(&l, 9, 4), 1, 9, 4);
        assert_eq!(row(&s, 0), "p1  │p2  ");
        assert_eq!(row(&s, 2), "    │    ");
    }

    #[test]
    fn stacked_panes_are_drawn_with_a_divider() {
        let mut l = Layout::new(1);
        l.split(1, Split::Stacked, 2, Rect { x: 0, y: 0, w: 5, h: 5 }).unwrap();
        let s = compose_layout(&l, &terms(&l, 5, 6), 1, 5, 6);
        assert_eq!(row(&s, 0), "p1   ");
        assert_eq!(row(&s, 2), "─────");
        assert_eq!(row(&s, 3), "p2   ");
    }

    #[test]
    fn dividers_that_meet_get_junction_glyphs() {
        let area = Rect { x: 0, y: 0, w: 9, h: 5 };
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, area).unwrap();
        l.split(2, Split::Stacked, 3, area).unwrap();
        let s = compose_layout(&l, &terms(&l, 9, 6), 1, 9, 6);
        assert_eq!(row(&s, 2), "    ├────");
        l.split(1, Split::Stacked, 4, area).unwrap();
        let s = compose_layout(&l, &terms(&l, 9, 6), 1, 9, 6);
        assert_eq!(row(&s, 2), "────┼────");
    }

    #[test]
    fn only_dividers_touching_the_active_pane_are_highlighted() {
        let area = Rect { x: 0, y: 0, w: 9, h: 5 };
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, area).unwrap();
        l.split(2, Split::Stacked, 3, area).unwrap();
        let t = terms(&l, 9, 6);
        let fg = |s: &Screen, x: usize, y: usize| s.cells[y * s.cols + x].style.fg;
        let s = compose_layout(&l, &t, 1, 9, 6);
        assert_eq!(fg(&s, 4, 0), BORDER_ACTIVE);
        assert_eq!(fg(&s, 6, 2), BORDER);
        let s = compose_layout(&l, &t, 3, 9, 6);
        assert_eq!(fg(&s, 6, 2), BORDER_ACTIVE);
        assert_eq!(fg(&s, 4, 4), BORDER_ACTIVE);
    }

    fn with_image(cols: usize, rows: usize, img_cols: usize, img_rows: usize, px: [u8; 4]) -> Terminal {
        let mut t = Terminal::new(cols, rows, 0);
        let mut b = Bitmap::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                b.set(x, y, px);
            }
        }
        t.grid.place_bitmap(b, img_cols, img_rows);
        t
    }

    fn window(t: &Terminal, mode: ImageMode, panes: usize) -> Screen {
        let g = &t.grid;
        let rect = Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let views: Vec<PaneView> = (0..panes).map(|_| PaneView { rect, grid: g, active: true, copy: None }).collect();
        let b = Style::default();
        compose_window_with(&views, &[], g.cols, g.rows + 1, &[], 1, (b, b), mode)
    }

    #[test]
    fn block_clients_get_image_tiles_as_plain_colored_cells() {
        let t = with_image(6, 4, 2, 2, [10, 20, 30, 255]);
        let s = window(&t, ImageMode::Blocks, 1);
        assert!(s.pieces.is_empty());
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let c = s.cells[y * s.cols + x];
            assert_eq!((c.tile, c.ch, c.style.bg), (0, ' ', Color::Rgb(10, 20, 30)), "cell {x},{y}");
        }
        assert_eq!(s.cells[2].style.bg, Color::Default);
    }

    #[test]
    fn iterm2_clients_get_pieces_that_know_their_place_in_the_image() {
        let t = with_image(6, 4, 2, 2, [10, 20, 30, 255]);
        let s = window(&t, ImageMode::Iterm2, 1);
        assert_eq!(s.pieces.len(), 4);
        let at = |x, y| s.piece(x, y).map(|p| (p.tx, p.ty, p.cols, p.rows));
        assert_eq!(at(0, 0), Some((0, 0, 2, 2)));
        assert_eq!(at(1, 1), Some((1, 1, 2, 2)));
        assert_eq!(at(2, 0), None);
        assert!(s.piece(0, 0).unwrap().same_image(s.piece(1, 1).unwrap()));
        assert!(!s.piece(0, 0).unwrap().same_cell(s.piece(1, 1).unwrap()));
    }

    #[test]
    fn tile_ids_of_two_panes_do_not_clash() {
        let a = with_image(4, 3, 1, 1, [255, 0, 0, 255]);
        let b = with_image(4, 3, 1, 1, [0, 0, 255, 255]);
        let rect = Rect { x: 0, y: 0, w: 4, h: 3 };
        let right = Rect { x: 4, ..rect };
        let views = [
            PaneView { rect, grid: &a.grid, active: true, copy: None },
            PaneView { rect: right, grid: &b.grid, active: false, copy: None },
        ];
        let st = Style::default();
        let s = compose_window_with(&views, &[], 8, 4, &[], 1, (st, st), ImageMode::Iterm2);
        assert_eq!(s.pieces.len(), 2);
        let (l, r) = (s.piece(0, 0).unwrap(), s.piece(4, 0).unwrap());
        assert!(!l.same_image(r));
        assert_eq!(l.bitmap.pixel(0, 0)[0], 255);
        assert_eq!(r.bitmap.pixel(0, 0)[2], 255);
    }

    #[test]
    fn a_dropped_image_shows_a_placeholder() {
        let mut t = with_image(4, 3, 2, 1, [1, 2, 3, 255]);
        assert!(t.grid.images.evict_oldest());
        for mode in [ImageMode::Blocks, ImageMode::Iterm2] {
            let s = window(&t, mode, 1);
            assert_eq!((s.cells[0].ch, s.cells[0].tile), ('░', 0));
            assert!(s.pieces.is_empty());
        }
    }

    #[test]
    fn cursor_belongs_to_the_active_pane_at_its_offset() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, Rect { x: 0, y: 0, w: 9, h: 3 }).unwrap();
        let t = terms(&l, 9, 4);
        assert_eq!(compose_layout(&l, &t, 1, 9, 4).cursor, Some((2, 0)));
        assert_eq!(compose_layout(&l, &t, 2, 9, 4).cursor, Some((7, 0)));
    }

    fn bar(text: &str, cols: usize) -> Vec<Cell> {
        plain_status(text, cols)
    }

    #[test]
    fn a_top_bar_takes_the_first_row_and_the_panes_the_rest() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, Rect { x: 0, y: 0, w: 9, h: 3 }).unwrap();
        let t = terms(&l, 9, 4);
        let area = Rect { x: 0, y: 1, w: 9, h: 3 };
        let views: Vec<PaneView> = l
            .rects(area)
            .into_iter()
            .map(|(id, rect)| PaneView { rect, grid: &t.iter().find(|(i, _)| *i == id).unwrap().1.grid, active: id == 1, copy: None })
            .collect();
        let b = (Style::default(), Style::default());
        let s = compose_window_placed(&views, &l.separators(area), 9, 4, &bar("top bar", 9), 1, true, b, ImageMode::Blocks);
        assert_eq!(row(&s, 0), "top bar  ");
        assert_eq!(row(&s, 1), "p1  │p2  ");
        assert_eq!(row(&s, 3), "    │    ");
        assert_eq!(s.cursor, Some((2, 1)), "the cursor is in the pane, below the bar");
        assert_eq!(s.scroll_areas, Vec::<(usize, usize)>::new(), "side by side panes do not span the width");
    }

    #[test]
    fn a_top_bar_clips_a_pane_that_reaches_into_it() {
        let t = Terminal::new(5, 4, 0);
        let rect = Rect { x: 0, y: 0, w: 5, h: 4 };
        let views = [PaneView { rect, grid: &t.grid, active: true, copy: None }];
        let b = (Style::default(), Style::default());
        let s = compose_window_placed(&views, &[], 5, 4, &bar("bar", 5), 1, true, b, ImageMode::Blocks);
        assert_eq!(row(&s, 0), "bar  ", "nothing of the pane over the bar");
        assert_eq!(s.scroll_areas, vec![]);
    }

    #[test]
    fn a_full_width_pane_under_a_top_bar_is_a_scroll_area_from_its_own_first_row() {
        let t = Terminal::new(5, 3, 0);
        let rect = Rect { x: 0, y: 1, w: 5, h: 3 };
        let views = [PaneView { rect, grid: &t.grid, active: true, copy: None }];
        let b = (Style::default(), Style::default());
        let s = compose_window_placed(&views, &[], 5, 4, &bar("bar", 5), 1, true, b, ImageMode::Blocks);
        assert_eq!(s.scroll_areas, vec![(1, 3)]);
    }

    #[test]
    fn a_pane_takes_its_default_colors_from_its_base_style() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, Rect { x: 0, y: 0, w: 9, h: 3 }).unwrap();
        let mut t = terms(&l, 9, 4);
        t[0].1.feed(b"\x1b[31mR");
        let area = Rect { x: 0, y: 0, w: 9, h: 3 };
        let views: Vec<PaneView> = l
            .rects(area)
            .into_iter()
            .map(|(id, rect)| PaneView { rect, grid: &t.iter().find(|(i, _)| *i == id).unwrap().1.grid, active: id == 1, copy: None })
            .collect();
        let idle = Style { bg: Color::Indexed(4), ..Style::default() };
        let busy = Style { bg: Color::Indexed(2), fg: Color::Indexed(7), attrs: 0 };
        let b = (Style::default(), Style::default());
        let s = compose_window_full(&views, &l.separators(area), 9, 4, &[], 1, false, b, Look { bases: (idle, busy), ..Look::default() }, ImageMode::Blocks, None);
        let at = |x: usize, y: usize| s.cells[y * s.cols + x].style;
        assert_eq!(at(2, 0).fg, Color::Indexed(1), "a color the program chose stays");
        assert_eq!(at(2, 0).bg, Color::Indexed(2), "the active pane takes the active base");
        assert_eq!(at(1, 1).bg, Color::Indexed(2), "an empty cell too");
        assert_eq!(at(1, 1).fg, Color::Indexed(7));
        assert_eq!(at(6, 1).bg, Color::Indexed(4), "the other pane takes the plain base");
        assert_ne!(at(4, 1).bg, Color::Indexed(2), "a divider is not part of a pane");
        let plain = compose_window_popup(&views, &l.separators(area), 9, 4, &[], 1, false, b, ImageMode::Blocks, None);
        assert_eq!(plain.cells[1].style.bg, Color::Default, "no base, no change");
    }

    #[test]
    fn dividers_are_drawn_with_the_chosen_line_set() {
        let mut l = Layout::new(1);
        l.split(1, Split::SideBySide, 2, Rect { x: 0, y: 0, w: 9, h: 5 }).unwrap();
        l.split(2, Split::Stacked, 3, Rect { x: 0, y: 0, w: 9, h: 5 }).unwrap();
        let t = terms(&l, 9, 6);
        let area = Rect { x: 0, y: 0, w: 9, h: 5 };
        let views: Vec<PaneView> = l
            .rects(area)
            .into_iter()
            .map(|(id, rect)| PaneView { rect, grid: &t.iter().find(|(i, _)| *i == id).unwrap().1.grid, active: id == 1, copy: None })
            .collect();
        let b = (Style::default(), Style::default());
        let glyphs = |lines: BorderLines| {
            let look = Look { lines, ..Look::default() };
            let s = compose_window_full(&views, &l.separators(area), 9, 6, &[], 1, false, b, look, ImageMode::Blocks, None);
            let at = |x: usize, y: usize| s.cells[y * s.cols + x].ch;
            (at(4, 0), at(5, 2), at(4, 2))
        };
        assert_eq!(glyphs(BorderLines::Single), ('│', '─', '├'));
        assert_eq!(glyphs(BorderLines::Double), ('║', '═', '╠'));
        assert_eq!(glyphs(BorderLines::Heavy), ('┃', '━', '┣'));
        assert_eq!(glyphs(BorderLines::Simple), ('|', '-', '+'));
        assert_eq!(glyphs(BorderLines::Spaces), (' ', ' ', ' '));
        assert_eq!(BorderLines::from_name("double"), BorderLines::Double);
        assert_eq!(BorderLines::from_name("anything"), BorderLines::Single);
    }

    #[test]
    fn a_label_row_is_taken_from_the_first_or_last_row_of_the_pane() {
        let rect = Rect { x: 2, y: 3, w: 10, h: 5 };
        assert_eq!(PaneStatus::Off.content(rect), rect);
        assert_eq!(PaneStatus::Top.content(rect), Rect { x: 2, y: 4, w: 10, h: 4 });
        assert_eq!(PaneStatus::Bottom.content(rect), Rect { x: 2, y: 3, w: 10, h: 4 });
        assert_eq!((PaneStatus::Top.label_row(rect), PaneStatus::Bottom.label_row(rect), PaneStatus::Off.label_row(rect)), (Some(3), Some(7), None));
        let one = Rect { h: 1, ..rect };
        assert_eq!((PaneStatus::Top.content(one), PaneStatus::Top.label_row(one)), (one, None), "one row has no label");
        assert_eq!(PaneStatus::from_name("top"), PaneStatus::Top);
        assert_eq!(PaneStatus::from_name("nonsense"), PaneStatus::Off);
    }

    #[test]
    fn a_pane_with_a_top_label_row_is_drawn_and_has_its_cursor_one_row_lower() {
        let mut t = Terminal::new(5, 3, 0);
        t.feed(b"ab");
        let rect = Rect { x: 0, y: 0, w: 5, h: 4 };
        let views = [PaneView { rect, grid: &t.grid, active: true, copy: None }];
        let b = (Style::default(), Style::default());
        let top = Look { status: PaneStatus::Top, ..Look::default() };
        let s = compose_window_full(&views, &[], 5, 5, &[], 1, false, b, top, ImageMode::Blocks, None);
        assert_eq!(row(&s, 0), ".....", "the label row is left to the server");
        assert_eq!(row(&s, 1), "ab   ");
        assert_eq!(s.cursor, Some((2, 1)));
        assert_eq!(s.scroll_areas, vec![], "no terminal scrolling with a label row");
        let bottom = Look { status: PaneStatus::Bottom, ..Look::default() };
        let s = compose_window_full(&views, &[], 5, 5, &[], 1, false, b, bottom, ImageMode::Blocks, None);
        assert_eq!(row(&s, 0), "ab   ");
        assert_eq!(s.cursor, Some((2, 0)));
    }

    #[test]
    fn no_status_rows_leave_the_whole_screen_to_the_panes() {
        let t = Terminal::new(5, 4, 0);
        let rect = Rect { x: 0, y: 0, w: 5, h: 4 };
        let views = [PaneView { rect, grid: &t.grid, active: true, copy: None }];
        let b = (Style::default(), Style::default());
        for top in [false, true] {
            let s = compose_window_placed(&views, &[], 5, 4, &[], 0, top, b, ImageMode::Blocks);
            assert_eq!(s.scroll_areas, vec![(0, 3)], "top={top}");
            assert_eq!(s.cursor, Some((0, 0)), "top={top}");
        }
    }

    #[test]
    fn a_bottom_bar_is_what_compose_window_with_always_drew() {
        let t = Terminal::new(5, 3, 0);
        let rect = Rect { x: 0, y: 0, w: 5, h: 3 };
        let views = [PaneView { rect, grid: &t.grid, active: true, copy: None }];
        let b = (Style::default(), Style::default());
        let a = compose_window_with(&views, &[], 5, 4, &bar("bar", 5), 1, b, ImageMode::Blocks);
        let c = compose_window_placed(&views, &[], 5, 4, &bar("bar", 5), 1, false, b, ImageMode::Blocks);
        assert_eq!(a.cells, c.cells);
        assert_eq!(row(&c, 3), "bar  ");
    }
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use crate::term::Terminal;

    fn row(s: &Screen, y: usize) -> String {
        s.cells[y * s.cols..(y + 1) * s.cols].iter().filter(|c| c.width != 0).map(|c| c.ch).collect()
    }

    fn pane(text: &str) -> Terminal {
        let mut t = Terminal::new(20, 6, 0);
        t.feed(text.as_bytes());
        t
    }

    fn compose_with(panes: &Terminal, popup: Option<&PopupView>) -> Screen {
        let rect = Rect { x: 0, y: 0, w: 20, h: 6 };
        let views = [PaneView { rect, grid: &panes.grid, active: true, copy: None }];
        let b = (Style::default(), Style::default());
        compose_window_popup(&views, &[], 20, 7, &[], 1, false, b, ImageMode::Blocks, popup)
    }

    fn view<'a>(grid: &'a Grid, lines: &'a str) -> PopupView<'a> {
        PopupView { rect: (4, 1, 12, 4), grid, lines, title: "title", marker: "[from %3]", border: Style::default(), base: Style::default() }
    }

    #[test]
    fn a_popup_is_a_border_with_the_programs_screen_inside_over_the_panes() {
        let under = pane("0123456789abcdefghij\r\n0123456789abcdefghij\r\n0123456789abcdefghij\r\n0123456789abcdefghij");
        let mut inside = Terminal::new(10, 2, 0);
        inside.feed(b"hello\r\nworld");
        let s = compose_with(&under, Some(&view(&inside.grid, "single")));
        assert_eq!(row(&s, 0), "0123456789abcdefghij", "above the box nothing changed");
        assert_eq!(row(&s, 1), "0123┌─title────┐ghij");
        assert_eq!(row(&s, 2), "0123│hello     │ghij");
        assert_eq!(row(&s, 3), "0123│world     │ghij");
        assert_eq!(row(&s, 4), "    └──────────┘    ");
        assert_eq!(s.cursor, Some((10, 3)), "the popup's cursor, inside the border");
    }

    #[test]
    fn the_origin_marker_shows_in_the_top_border_when_it_fits() {
        let under = pane("");
        let inside = Terminal::new(16, 2, 0);
        let mut v = view(&inside.grid, "single");
        v.rect = (2, 1, 18, 4);
        v.title = "t";
        let s = compose_with(&under, Some(&v));
        assert!(row(&s, 1).contains("[from %3]"), "{}", row(&s, 1));
        assert!(row(&s, 1).contains("─t"), "the title is kept");
        let narrow = Terminal::new(10, 2, 0);
        let s = compose_with(&under, Some(&view(&narrow.grid, "single")));
        assert!(!row(&s, 1).contains("[from"), "a marker that does not fit is left out, never cut");
    }

    #[test]
    fn a_popup_without_a_border_is_all_program_and_a_padded_one_has_blank_edges() {
        let under = pane("0123456789abcdefghij");
        let mut inside = Terminal::new(12, 4, 0);
        inside.feed(b"abc");
        let mut v = view(&inside.grid, "none");
        let s = compose_with(&under, Some(&v));
        assert_eq!(row(&s, 1), "    abc             ", "the program's screen fills the whole box");
        v.lines = "padded";
        let small = Terminal::new(10, 2, 0);
        v.grid = &small.grid;
        let s = compose_with(&under, Some(&v));
        assert_eq!(row(&s, 1).chars().skip(4).take(12).collect::<String>(), "  title     ", "padded edges are spaces, the title is still written");
    }

    #[test]
    fn a_popup_takes_the_terminal_modes_and_stops_scroll_regions() {
        let under = pane("x");
        let mut inside = Terminal::new(10, 2, 0);
        inside.feed(b"\x1b[?1h\x1b[?25l");
        let plain = compose_with(&under, None);
        assert!(!plain.modes.cursor_keys && plain.cursor.is_some() && !plain.scroll_areas.is_empty());
        let s = compose_with(&under, Some(&view(&inside.grid, "single")));
        assert!(s.modes.cursor_keys, "the keys go to the popup's program, which asked for application keys");
        assert!(s.cursor.is_none(), "its cursor is hidden");
        assert!(s.scroll_areas.is_empty(), "the terminal must not scroll cells the box covers");
    }

    #[test]
    fn a_box_that_cannot_hold_its_border_is_not_drawn() {
        let under = pane("0123456789abcdefghij");
        let inside = Terminal::new(1, 1, 0);
        let mut v = view(&inside.grid, "single");
        v.rect = (3, 1, 2, 1);
        let s = compose_with(&under, Some(&v));
        assert_eq!(row(&s, 0), "0123456789abcdefghij");
        assert!(s.cursor.is_none());
    }

    #[test]
    fn nothing_changes_without_a_popup() {
        let under = pane("0123456789abcdefghij");
        let rect = Rect { x: 0, y: 0, w: 20, h: 6 };
        let views = [PaneView { rect, grid: &under.grid, active: true, copy: None }];
        let b = (Style::default(), Style::default());
        let a = compose_window_placed(&views, &[], 20, 7, &[], 1, false, b, ImageMode::Blocks);
        assert_eq!(a.cells, compose_with(&under, None).cells);
    }
}
