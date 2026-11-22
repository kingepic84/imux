use std::collections::VecDeque;

use super::cell::{Cell, Row};
use super::grid::{Grid, MainScreen};

impl Grid {
    /// Resize with reflow on the main screen. The alt screen is truncated or padded (apps repaint).
    pub fn resize(&mut self, cols: usize, rows: usize, cell_px: (u16, u16)) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        self.cell_px = cell_px;
        if cols == self.cols && rows == self.rows {
            return;
        }

        let alt = self.main.take().map(|main| {
            let alt_screen = std::mem::replace(&mut self.screen, main.screen);
            let alt_cursor = (self.cx, self.cy);
            self.cx = main.cx;
            self.cy = main.cy;
            (alt_screen, alt_cursor)
        });

        let (cx, cy) = reflow(self, cols, rows);
        self.cols = cols;
        self.rows = rows;
        self.cx = cx;
        self.cy = cy;
        self.wrap_pending = false;
        self.top = 0;
        self.bottom = rows - 1;
        self.tabs = (0..cols).map(|x| x % 8 == 0 && x != 0).collect();

        if let Some((mut alt_screen, alt_cursor)) = alt {
            alt_screen.truncate(rows);
            while alt_screen.len() < rows {
                alt_screen.push(Row::new(cols, Cell::BLANK));
            }
            for r in &mut alt_screen {
                r.cells.resize(cols, Cell::BLANK);
                if r.cells.last().is_some_and(|c| c.width == 2) {
                    r.cells[cols - 1] = Cell::BLANK;
                }
            }
            let main_screen = std::mem::replace(&mut self.screen, alt_screen);
            self.main = Some(MainScreen { screen: main_screen, cx, cy, wrap_pending: false });
            self.cx = alt_cursor.0.min(cols - 1);
            self.cy = alt_cursor.1.min(rows - 1);
        }
    }
}

fn is_blank_row(row: &Row) -> bool {
    row.clipped.is_empty() && row.cells.iter().all(Cell::is_blank)
}

/// Rebuild scrollback + screen at the new size. Returns the new cursor position.
fn reflow(g: &mut Grid, cols: usize, rows: usize) -> (usize, usize) {
    let cursor = (g.cx, g.cy);
    let old_sb = std::mem::take(&mut g.scrollback);
    let sb_len = old_sb.len();
    let mut lines: Vec<Row> = old_sb.into_iter().collect();
    lines.extend(std::mem::take(&mut g.screen));
    let cursor_abs = sb_len + cursor.1;

    let mut used = lines.len();
    while used > cursor_abs + 1 && is_blank_row(&lines[used - 1]) && !lines[used - 2].wrapped {
        used -= 1;
    }
    lines.truncate(used);

    let mut out: Vec<Row> = Vec::new();
    let mut new_cursor = (0usize, 0usize);
    let mut i = 0;
    while i < lines.len() {
        let mut cells: Vec<Cell> = Vec::new();
        let mut cursor_off: Option<usize> = None;
        loop {
            let row = &lines[i];
            if i == cursor_abs {
                cursor_off = Some(cells.len() + cursor.0);
            }
            let continues = row.wrapped && i + 1 < lines.len();
            let mut part = row.cells.clone();
            part.extend_from_slice(&row.clipped);
            if !continues {
                while part.last().is_some_and(Cell::is_blank) {
                    part.pop();
                }
            }
            cells.extend(part);
            i += 1;
            if !continues {
                break;
            }
        }
        if let Some(off) = cursor_off
            && cells.len() < off
        {
            cells.resize(off, Cell::BLANK);
        }
        let first_row = out.len();
        let (pos_row, pos_col) = if cells.iter().any(|c| c.tile != 0) {
            clip_line(&cells, cols, cursor_off, &mut out)
        } else {
            wrap_line(&cells, cols, cursor_off, &mut out)
        };
        if cursor_off.is_some() {
            new_cursor = (pos_col, first_row + pos_row);
        }
    }

    let cursor_row = new_cursor.1;
    let total = out.len().max(cursor_row + 1);
    while out.len() < total {
        out.push(Row::new(cols, Cell::BLANK));
    }
    // The cursor line stays on the screen: when lines below the cursor do not fit, the bottom ones
    // are cut rather than the cursor going up into the history (a program can leave the cursor
    // above its last line, and the number would underflow).
    let split = total.saturating_sub(rows).min(cursor_row);
    let mut screen: Vec<Row> = out.split_off(split);
    screen.truncate(rows);
    let mut history: VecDeque<Row> = out.into();
    while screen.len() < rows {
        screen.push(Row::new(cols, Cell::BLANK));
    }
    for r in &mut screen {
        r.pad_to(cols);
    }
    for r in &mut history {
        r.trim();
    }
    while history.len() > g.history_limit {
        history.pop_front();
    }
    g.screen = screen;
    g.scrollback = history;
    (new_cursor.0.min(cols - 1), (cursor_row - split).min(rows - 1))
}

/// Lay out a line that holds image tiles: it stays one row, whatever the width. What does not fit
/// is kept on the row and comes back when the pane is wide enough again. Returns the cursor as
/// `wrap_line` does.
fn clip_line(cells: &[Cell], cols: usize, cursor_off: Option<usize>, out: &mut Vec<Row>) -> (usize, usize) {
    let mut row = Row::new(cols, Cell::BLANK);
    let shown = cells.len().min(cols);
    row.cells[..shown].copy_from_slice(&cells[..shown]);
    if cells.len() > cols {
        // A wide char cut in two would leave half of it on screen.
        if row.cells[cols - 1].width == 2 {
            row.cells[cols - 1] = Cell::BLANK;
        }
        row.clipped = cells[cols..].to_vec();
    }
    out.push(row);
    (0, cursor_off.map_or(0, |off| off.min(cols - 1)))
}

/// Lay one logical line out into `out`. Returns the (row, col) of `cursor_off` relative to the
/// first row written. Wide chars are never split across rows.
fn wrap_line(cells: &[Cell], cols: usize, cursor_off: Option<usize>, out: &mut Vec<Row>) -> (usize, usize) {
    let first = out.len();
    let mut row = Row::new(cols, Cell::BLANK);
    let (mut x, mut cur_row, mut cur_col) = (0usize, 0usize, 0usize);
    let mut placed_cursor = false;
    for (i, &c) in cells.iter().enumerate() {
        if c.width == 0 {
            continue;
        }
        let w = usize::from(c.width);
        if x + w > cols {
            row.wrapped = true;
            out.push(std::mem::replace(&mut row, Row::new(cols, Cell::BLANK)));
            x = 0;
        }
        if cursor_off == Some(i) {
            cur_row = out.len() - first;
            cur_col = x;
            placed_cursor = true;
        }
        row.cells[x] = c;
        if w == 2 && x + 1 < cols {
            row.cells[x + 1] = Cell { ch: ' ', width: 0, style: c.style, tile: 0, mark: 0 };
        }
        x += w;
    }
    if let Some(off) = cursor_off
        && !placed_cursor
    {
        let extra = off.saturating_sub(cells.len());
        cur_col = (x + extra).min(cols - 1);
        cur_row = out.len() - first;
    }
    out.push(row);
    (cur_row, cur_col)
}
