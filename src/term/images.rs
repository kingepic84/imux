//! Putting images into the grid: stamping tiles into cells, and collecting images whose cells
//! have scrolled out of history.

use std::collections::HashSet;

use super::cell::{Cell, Row};
use super::grid::Grid;
use crate::image::Bitmap;
use crate::image::iterm::Metrics;
use crate::image::store::ImageId;

/// Tiles kept before a collection is forced, whatever the memory in use.
const MAX_TILES: usize = 200_000;

/// Most cells one picture may cover (a 4000x3000 picture in a 200-column pane is about 26 000).
const MAX_IMAGE_CELLS: usize = 32_768;

/// Pixels per cell when the outer terminal does not say.
const FALLBACK_CELL: (usize, usize) = (8, 16);

impl Grid {
    pub fn image_metrics(&self) -> Metrics {
        let (w, h) = (usize::from(self.cell_px.0), usize::from(self.cell_px.1));
        let (cell_w, cell_h) = if w == 0 || h == 0 { FALLBACK_CELL } else { (w, h) };
        Metrics { cell_w, cell_h, cols: self.cols, rows: self.rows }
    }

    /// Stamp an image of `cols` x `rows` cells with its top-left at the cursor, scrolling as
    /// needed, and leave the cursor at the start of the line below it. Cells past the right edge
    /// are clipped; the image keeps its footprint.
    pub fn place_bitmap(&mut self, bitmap: Bitmap, cols: usize, rows: usize) -> ImageId {
        self.place_bitmap_at(bitmap, cols, rows, false)
    }

    /// Like `place_bitmap`; with `stay` the cursor ends where it started (moved up with the text
    /// if the picture scrolled the screen) instead of below the picture.
    pub fn place_bitmap_at(&mut self, bitmap: Bitmap, cols: usize, rows: usize, stay: bool) -> ImageId {
        // Every cell of the footprint gets a tile, however small the picture: a one-pixel picture
        // asked to cover 200 x 4096 cells would cost a million tiles (security review P-02).
        let rows = rows.min(MAX_IMAGE_CELLS / cols.min(self.cols).max(1));
        let id = self.images.add(bitmap, cols, rows);
        let (cx0, cy0, wrap0) = (self.cx, self.cy, self.wrap_pending);
        let mut scrolled = 0;
        self.wrap_pending = false;
        let x0 = self.cx.min(self.cols - 1);
        let visible = cols.min(self.cols - x0);
        for ty in 0..rows {
            let y = self.cy;
            for tx in 0..visible {
                self.clear_wide_overlap(y, x0 + tx);
                let tile = self.images.tile(id, tx as u16, ty as u16);
                self.screen[y].cells[x0 + tx] = Cell { tile, ..Cell::erased(self.pen) };
            }
            if ty + 1 < rows {
                self.linefeed();
                if self.cy == y {
                    scrolled += 1;
                }
            }
        }
        if stay {
            (self.cx, self.cy, self.wrap_pending) = (cx0, cy0.saturating_sub(scrolled), wrap0);
        } else {
            self.cx = 0;
            self.linefeed();
        }
        self.limit_images();
        id
    }

    /// Remove the pictures `hit` accepts from the visible screen (a Kitty delete); the cells
    /// become blanks and pictures nothing shows any more are freed.
    pub fn erase_images(&mut self, hit: impl Fn(ImageId) -> bool) {
        let shown: HashSet<u32> =
            self.screen.iter().flat_map(|r| r.cells.iter().chain(&r.clipped)).map(|c| c.tile).filter(|&t| t != 0).collect();
        let doomed: HashSet<u32> = shown.into_iter().filter(|&t| self.images.lookup(t).is_some_and(|tile| hit(tile.image))).collect();
        if doomed.is_empty() {
            return;
        }
        let blank = Cell::erased(self.pen);
        for row in &mut self.screen {
            for cell in row.cells.iter_mut().chain(row.clipped.iter_mut()).filter(|c| doomed.contains(&c.tile)) {
                *cell = blank;
            }
        }
        self.collect_images();
    }

    /// The pictures with a tile on a visible cell (column, row; both from 0) that `hit` accepts.
    pub fn images_where(&self, hit: impl Fn(usize, usize) -> bool) -> HashSet<ImageId> {
        let mut seen = HashSet::new();
        for (y, row) in self.screen.iter().enumerate() {
            for (x, cell) in row.cells.iter().enumerate() {
                if cell.tile != 0
                    && hit(x, y)
                    && let Some(tile) = self.images.lookup(cell.tile)
                {
                    seen.insert(tile.image);
                }
            }
        }
        seen
    }

    pub(super) fn limit_images(&mut self) {
        if self.images.over_budget() || self.images.tiles_over(MAX_TILES) {
            self.collect_images();
        }
        while self.images.over_budget() && self.images.evict_oldest() {}
    }

    fn all_rows_mut(&mut self) -> impl Iterator<Item = &mut Row> {
        self.screen.iter_mut().chain(self.scrollback.iter_mut()).chain(self.main.iter_mut().flat_map(|m| m.screen.iter_mut()))
    }

    /// Free images and tiles that no cell on the screen, in the history or on the saved main
    /// screen refers to any more.
    pub fn collect_images(&mut self) {
        let mut live = HashSet::new();
        for row in self.all_rows_mut() {
            live.extend(row.cells.iter().chain(&row.clipped).map(|c| c.tile).filter(|&t| t != 0));
        }
        let map = self.images.retain(&live);
        for row in self.all_rows_mut() {
            for cell in row.cells.iter_mut().chain(row.clipped.iter_mut()).filter(|c| c.tile != 0) {
                cell.tile = map[cell.tile as usize];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Terminal;

    fn term(cols: usize, rows: usize) -> Terminal {
        let mut t = Terminal::new(cols, rows, 100);
        t.grid.cell_px = (10, 20);
        t
    }

    fn bmp(w: usize, h: usize) -> Bitmap {
        Bitmap::new(w, h)
    }

    fn tile_at(t: &Terminal, x: usize, y: usize) -> Option<(u32, u16, u16)> {
        let c = t.grid.screen[y].cells[x];
        t.grid.images.lookup(c.tile).map(|tile| (tile.image, tile.tx, tile.ty))
    }

    #[test]
    fn an_image_stamps_its_footprint_at_the_cursor() {
        let mut t = term(10, 6);
        t.feed(b"ab\r\ncd");
        t.grid.place_bitmap(bmp(30, 40), 3, 2);
        assert_eq!(tile_at(&t, 2, 1), Some((1, 0, 0)));
        assert_eq!(tile_at(&t, 4, 1), Some((1, 2, 0)));
        assert_eq!(tile_at(&t, 2, 2), Some((1, 0, 1)));
        assert_eq!(tile_at(&t, 4, 2), Some((1, 2, 1)));
        assert_eq!(tile_at(&t, 5, 1), None);
        assert_eq!(tile_at(&t, 1, 1), None);
        assert_eq!((t.grid.cx, t.grid.cy), (0, 3));
    }

    #[test]
    fn a_tiny_picture_cannot_ask_for_a_huge_footprint_or_make_every_later_picture_slow() {
        let mut t = Terminal::new(200, 50, 50_000);
        t.grid.cell_px = (10, 20);
        for _ in 0..40 {
            t.grid.place_bitmap(bmp(1, 1), 200, 4096);
        }
        assert!(t.grid.images.tile_count() <= 40 * MAX_IMAGE_CELLS, "{}", t.grid.images.tile_count());
        let started = std::time::Instant::now();
        for _ in 0..20 {
            t.grid.place_bitmap(bmp(1, 1), 1, 1);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "20 small pictures took {:?}", started.elapsed());
    }

    #[test]
    fn an_image_at_the_bottom_scrolls_and_keeps_its_tiles() {
        let mut t = term(10, 4);
        t.feed(b"1\r\n2\r\n3\r\n4");
        t.grid.place_bitmap(bmp(20, 40), 2, 2);
        assert_eq!(t.grid.cy, 3);
        assert_eq!(tile_at(&t, 1, 1), Some((1, 0, 0)));
        assert_eq!(tile_at(&t, 1, 2), Some((1, 0, 1)));
        t.feed(b"\r\n\r\n\r\n");
        assert!(t.grid.scrollback.iter().any(|r| r.cells.iter().any(|c| c.tile != 0)));
    }

    #[test]
    fn text_and_erasing_replace_tiles() {
        let mut t = term(10, 6);
        t.grid.place_bitmap(bmp(20, 20), 2, 1);
        t.feed(b"\x1b[1;1Hx");
        assert_eq!(tile_at(&t, 0, 0), None);
        assert_eq!(tile_at(&t, 1, 0), Some((1, 1, 0)));
        t.feed(b"\x1b[2J");
        assert_eq!(tile_at(&t, 1, 0), None);
    }

    #[test]
    fn a_wide_image_is_clipped_at_the_right_edge() {
        let mut t = term(6, 4);
        t.feed(b"\x1b[1;5H");
        t.grid.place_bitmap(bmp(50, 20), 5, 1);
        assert_eq!(tile_at(&t, 5, 0), Some((1, 1, 0)));
        assert_eq!(t.grid.images.image(1).map(|p| p.cols), Some(5));
    }

    #[test]
    fn collecting_frees_images_that_scrolled_out_of_history() {
        let mut t = Terminal::new(10, 3, 2);
        t.grid.cell_px = (10, 20);
        t.grid.place_bitmap(bmp(10, 20), 1, 1);
        t.grid.place_bitmap(bmp(10, 20), 1, 1);
        assert!(t.grid.images.image(1).is_some());
        t.feed(b"\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n");
        t.grid.collect_images();
        assert!(t.grid.images.image(1).is_none());
        assert!(t.grid.images.image(2).is_none());
        assert_eq!(t.grid.images.tile_count(), 0);
    }

    #[test]
    fn collecting_renumbers_the_tiles_cells_still_use() {
        let mut t = term(10, 6);
        t.grid.place_bitmap(bmp(10, 20), 1, 1);
        t.grid.place_bitmap(bmp(10, 20), 1, 1);
        t.feed(b"\x1b[1;1H ");
        t.grid.collect_images();
        assert_eq!(t.grid.images.tile_count(), 1);
        assert_eq!(tile_at(&t, 0, 1), Some((2, 0, 0)));
        assert!(t.grid.images.image(1).is_none());
    }

    #[test]
    fn the_budget_evicts_old_images_and_cells_survive_as_placeholders() {
        let mut t = term(10, 6);
        t.grid.images = crate::image::store::Store::new(100);
        t.grid.place_bitmap(bmp(4, 4), 1, 1);
        t.grid.place_bitmap(bmp(4, 4), 1, 1);
        t.grid.place_bitmap(bmp(4, 4), 1, 1);
        assert!(t.grid.images.bytes() <= 100);
        assert!(t.grid.images.image(3).is_some());
        assert!(t.grid.screen[0].cells[0].tile != 0);
    }

    fn tiles_in_row(t: &Terminal, y: usize) -> Vec<u16> {
        t.grid.screen[y].cells.iter().filter_map(|c| t.grid.images.lookup(c.tile)).map(|tile| tile.tx).collect()
    }

    #[test]
    fn narrowing_clips_an_image_row_instead_of_wrapping_it() {
        let mut t = term(10, 6);
        t.feed(b"top\r\n");
        t.grid.place_bitmap(bmp(60, 40), 6, 2);
        t.feed(b"below");
        t.grid.resize(4, 6, (10, 20));
        assert_eq!(t.grid.screen[0].text(), "top");
        assert_eq!(tiles_in_row(&t, 1), vec![0, 1, 2, 3]);
        assert_eq!(tiles_in_row(&t, 2), vec![0, 1, 2, 3]);
        assert_eq!(t.grid.screen[3].text(), "belo");
        assert!(!t.grid.screen[1].wrapped);
    }

    #[test]
    fn widening_again_brings_the_whole_image_back() {
        let mut t = term(10, 6);
        t.grid.place_bitmap(bmp(60, 40), 6, 2);
        t.grid.resize(3, 6, (10, 20));
        t.grid.resize(10, 6, (10, 20));
        assert_eq!(tiles_in_row(&t, 0), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(tiles_in_row(&t, 1), vec![0, 1, 2, 3, 4, 5]);
        assert!(t.grid.screen[0].clipped.is_empty());
    }

    #[test]
    fn plain_lines_next_to_an_image_still_reflow() {
        let mut t = term(10, 6);
        t.feed(b"abcdefgh\r\n");
        t.grid.place_bitmap(bmp(20, 20), 2, 1);
        t.grid.resize(4, 6, (10, 20));
        assert_eq!(t.grid.screen[0].text(), "abcd");
        assert_eq!(t.grid.screen[1].text(), "efgh");
        assert_eq!(tiles_in_row(&t, 2), vec![0, 1]);
    }

    #[test]
    fn clipped_tiles_keep_their_image_alive_through_collection() {
        let mut t = term(10, 6);
        t.grid.place_bitmap(bmp(60, 20), 6, 1);
        t.grid.place_bitmap(bmp(10, 20), 1, 1);
        t.grid.resize(2, 6, (10, 20));
        t.feed(b"\x1b[2;1H\x1b[2K");
        t.grid.collect_images();
        t.grid.resize(10, 6, (10, 20));
        assert_eq!(tiles_in_row(&t, 0), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(t.grid.images.image(1).map(|p| p.cols), Some(6));
    }

    #[test]
    fn writing_on_a_clipped_row_forgets_what_was_cut_off() {
        let mut t = term(10, 6);
        t.grid.place_bitmap(bmp(60, 20), 6, 1);
        t.grid.place_bitmap(bmp(60, 20), 6, 1);
        t.grid.resize(3, 6, (10, 20));
        t.feed(b"\x1b[1;1Hx\x1b[2;1H\x1b[K");
        t.grid.resize(10, 6, (10, 20));
        assert_eq!(tiles_in_row(&t, 0), vec![1, 2]);
        assert_eq!(tiles_in_row(&t, 1), Vec::<u16>::new());
    }

    #[test]
    fn the_cursor_stays_on_its_row_when_an_image_row_is_clipped() {
        let mut t = term(10, 6);
        t.grid.place_bitmap(bmp(60, 20), 6, 1);
        t.feed(b"ab");
        t.grid.resize(4, 6, (10, 20));
        assert_eq!((t.grid.cx, t.grid.cy), (2, 1));
        t.feed(b"c");
        assert_eq!(t.grid.screen[1].text(), "abc");
    }
}
