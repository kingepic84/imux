use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Weak};

use crate::image::Bitmap;
use crate::log;
use crate::proto::{Caps, ImageMode};
use crate::term::cell::{BLINK, BOLD, Cell, Color, DIM, HIDDEN, ITALIC, REVERSE, STRIKE, Style, UNDERLINE};
use crate::term::grid::MouseMode;

use super::compositor::{PaneModes, Piece, Screen};
use super::{iterm, kitty, quad, scaled, sixel, xterm};
use ::image::RgbaImage;

/// Unchanged cells up to this many are printed through rather than paying for a cursor jump.
const GAP: usize = 4;

/// Per-client frame differ: remembers the last screen it emitted and outputs only the changes.
pub struct Renderer {
    last: Option<Screen>,
    caps: Caps,
    /// Pixels per cell of the client's terminal; images are drawn at this size.
    cell_px: (usize, usize),
    cache: ImageCache,
    /// Kitty clients: crops already stored on the terminal, by cache key, with the id they were
    /// sent under. A weak reference to the bitmap is held: the allocation, and so the address that
    /// is part of the key, cannot be reused while the entry lives, but the pixels are freed when
    /// the pane's store drops them (a strong reference pinned evicted pictures, security review P-04).
    sent: HashMap<[usize; 10], (u32, Weak<Bitmap>)>,
    sent_bytes: usize,
    next_id: u32,
    /// Regions the encoder could not make a picture of; they are drawn with quadrant blocks and
    /// not tried again. The bitmap is held for the same reason as in `sent`.
    failed: HashMap<[usize; 10], Weak<Bitmap>>,
    /// Picture regions are cut into bands this many cell rows high, on the picture's own row
    /// boundaries, so that scrolling reuses the encoded bands instead of encoding the whole visible
    /// part of the picture again at every step. 0 draws each region in one piece.
    band: usize,
    /// Kitty clients: send each picture once, whole and at the size of its footprint, and show
    /// parts of it by source rectangle (`image-kitty-placement source`), instead of storing every
    /// crop. A scroll step then sends a few bytes per picture and nothing is resampled again.
    kitty_source: bool,
    /// Inline-image clients: let the terminal scroll the cells (`image-scroll`).
    scroll: bool,
    /// Sixel clients: dither the pictures that need more than 256 colors (`image-dither`).
    dither: bool,
    /// Whole pictures stored on the terminal, by (bitmap address, footprint, cell size).
    whole: HashMap<[usize; 5], Whole>,
    frames: u64,
    /// Inline-image clients: pictures scaled once to the size of their footprint, so that a crop is
    /// a copy of pixel rows and not a resample.
    scaled: HashMap<[usize; 5], (Weak<Bitmap>, Arc<RgbaImage>)>,
    scaled_bytes: usize,
}

/// A picture stored on a Kitty terminal as a whole.
struct Whole {
    id: u32,
    /// Pixel size of what was sent, which the source rectangles are cut from.
    size: (usize, usize),
    bytes: usize,
    /// Frame number that last placed a part of it.
    used: u64,
    /// Placements of it on the screen, numbered 1 to `slots`. Each is placed again, with the same
    /// ids, in the frame that moves it: that replaces it in one step, so there is never a moment
    /// with the picture unplaced (iTerm2 drops the texture of a picture nothing places and then
    /// draws it again from nothing, top to bottom).
    slots: u32,
    bitmap: Weak<Bitmap>,
}

/// Whole pictures a Kitty terminal is asked to keep, and their bytes sent; past these the ones the
/// frame does not show are freed (never the ones it does: that would flash).
const MAX_WHOLE: usize = 32;
const MAX_WHOLE_BYTES: usize = 192 * 1024 * 1024;

/// Scaled pictures kept for inline-image clients.
const MAX_SCALED_BYTES: usize = 256 * 1024 * 1024;

/// Whole screen rows `(first, last)` the terminal is asked to scroll, and by how much: new row `y`
/// was row `y + by` before (positive: the content moves up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scroll {
    first: usize,
    last: usize,
    by: isize,
}

/// Longest scroll looked for, in rows.
const MAX_SCROLL: usize = 64;

fn row_has_piece(s: &Screen, y: usize) -> bool {
    (0..s.cols).any(|x| s.piece(x, y).is_some())
}

/// Row `y` of `new` is what row `y2` of `old` shows.
fn same_row(new: &Screen, y: usize, old: &Screen, y2: usize) -> bool {
    (0..new.cols).all(|x| match (new.piece(x, y), old.piece(x, y2)) {
        (None, None) => new.cells[y * new.cols + x] == old.cells[y2 * old.cols + x],
        (Some(a), Some(b)) => a.same_cell(b),
        _ => false,
    })
}

/// A scroll of one of the full-width panes that makes the picture rows of `new` the picture rows
/// `old` already shows. An inline picture belongs to the cells it is drawn in, so when the terminal
/// scrolls those cells the picture goes with them: nothing is encoded or sent for the rows that
/// stay, only the rows scrolled in are drawn.
fn find_scroll(new: &Screen, old: &Screen) -> Option<Scroll> {
    if (new.cols, new.rows) != (old.cols, old.rows) {
        return None;
    }
    let mut best: Option<(usize, Scroll)> = None;
    for &(first, last) in &new.scroll_areas {
        let picture_rows: Vec<usize> = (first..=last).filter(|&y| row_has_piece(new, y)).collect();
        if picture_rows.is_empty() {
            continue;
        }
        let kept = picture_rows.iter().filter(|&&y| same_row(new, y, old, y)).count();
        for k in 1..=(last - first).min(MAX_SCROLL) as isize {
            for by in [k, -k] {
                let hits = picture_rows
                    .iter()
                    .filter(|&&y| {
                        let y2 = y as isize + by;
                        y2 >= first as isize && y2 <= last as isize && same_row(new, y, old, y2 as usize)
                    })
                    .count();
                if hits > kept && best.is_none_or(|(n, _)| hits > n) {
                    best = Some((hits, Scroll { first, last, by }));
                }
            }
        }
    }
    best.map(|(_, s)| s)
}

/// What the terminal shows after it scrolled `s`: the rows moved, the ones scrolled in blank.
fn scrolled(old: &Screen, s: Scroll) -> Screen {
    let mut out = old.clone();
    for y in s.first..=s.last {
        let y2 = y as isize + s.by;
        let row = &mut out.cells[y * old.cols..(y + 1) * old.cols];
        if y2 >= s.first as isize && y2 <= s.last as isize {
            row.copy_from_slice(&old.cells[y2 as usize * old.cols..(y2 as usize + 1) * old.cols]);
        } else {
            row.fill(Cell::BLANK);
        }
    }
    out
}

/// Failed regions remembered before the list starts over.
const MAX_FAILED: usize = 256;

/// A region that could not be encoded: it is drawn with blocks from now on.
fn remember_failure(failed: &mut HashMap<[usize; 10], Weak<Bitmap>>, key: [usize; 10], p: &Piece) {
    if failed.len() >= MAX_FAILED {
        failed.clear();
    }
    failed.insert(key, Arc::downgrade(&p.bitmap));
}

/// Stored crops a Kitty terminal is asked to keep before everything is deleted and sent anew.
const MAX_SENT: usize = 256;
const MAX_SENT_BYTES: usize = 128 * 1024 * 1024;

/// Used when the terminal does not report its size in pixels.
const DEFAULT_CELL_PX: (usize, usize) = (8, 16);

impl Default for Renderer {
    fn default() -> Renderer {
        Renderer {
            last: None,
            caps: Caps { truecolor: true, ..Caps::BASIC },
            cell_px: DEFAULT_CELL_PX,
            cache: ImageCache::default(),
            sent: HashMap::new(),
            sent_bytes: 0,
            next_id: 0,
            failed: HashMap::new(),
            band: 0,
            kitty_source: false,
            scroll: false,
            dither: false,
            whole: HashMap::new(),
            frames: 0,
            scaled: HashMap::new(),
            scaled_bytes: 0,
        }
    }
}

/// Free whole pictures the terminal holds for nothing: those whose bitmap the pane has dropped, and,
/// while there are too many or too many bytes, the oldest ones this frame did not place.
fn trim_whole(whole: &mut HashMap<[usize; 5], Whole>, out: &mut String, frame: u64) {
    let mut gone: Vec<[usize; 5]> = whole.iter().filter(|(_, w)| w.bitmap.strong_count() == 0).map(|(k, _)| *k).collect();
    let mut bytes: usize = whole.values().map(|w| w.bytes).sum::<usize>() - gone.iter().filter_map(|k| whole.get(k)).map(|w| w.bytes).sum::<usize>();
    let mut count = whole.len() - gone.len();
    if count > MAX_WHOLE || bytes > MAX_WHOLE_BYTES {
        let mut idle: Vec<([usize; 5], u32, usize)> =
            whole.iter().filter(|(k, w)| w.used != frame && !gone.contains(k)).map(|(k, w)| (*k, w.id, w.bytes)).collect();
        idle.sort_by_key(|(_, id, _)| *id);
        for (k, _, b) in idle {
            if count <= MAX_WHOLE && bytes <= MAX_WHOLE_BYTES {
                break;
            }
            gone.push(k);
            count -= 1;
            bytes -= b;
        }
    }
    for k in gone {
        if let Some(w) = whole.remove(&k) {
            out.push_str(&kitty::delete_image_apc(w.id));
        }
    }
}

/// The picture scaled to its footprint, made once and kept (inline-image clients).
fn scaled_picture(
    map: &mut HashMap<[usize; 5], (Weak<Bitmap>, Arc<RgbaImage>)>,
    bytes: &mut usize,
    p: &Piece,
    cell: (usize, usize),
) -> Result<Arc<RgbaImage>, String> {
    let key = [Arc::as_ptr(&p.bitmap) as usize, p.cols, p.rows, cell.0, cell.1];
    if let Some((_, img)) = map.get(&key) {
        return Ok(Arc::clone(img));
    }
    let img = Arc::new(scaled::scale(&p.bitmap, p.cols, p.rows, cell)?);
    let size = img.as_raw().len();
    if *bytes + size > MAX_SCALED_BYTES {
        map.clear();
        *bytes = 0;
    }
    *bytes += size;
    map.insert(key, (Arc::downgrade(&p.bitmap), Arc::clone(&img)));
    Ok(img)
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer::default()
    }

    /// Height in cell rows of the bands pictures are drawn in (`image-band-rows`, 0 = one piece).
    pub fn set_band(&mut self, rows: usize) {
        self.band = rows;
    }

    /// Kitty clients: show pictures by source rectangle from one stored copy of each
    /// (`image-kitty-placement source`) or store every crop (`crop`). A change repaints.
    pub fn set_kitty_source(&mut self, on: bool) {
        if self.kitty_source != on {
            self.kitty_source = on;
            self.last = None;
        }
    }

    /// Inline-image clients: when the picture rows of a full-width pane are the rows of the last
    /// frame moved up or down, have the terminal scroll them (`image-scroll`) and draw only the rows
    /// that came into view.
    pub fn set_scroll(&mut self, on: bool) {
        self.scroll = on;
    }

    /// Sixel clients: dither pictures that need more than 256 colors (`image-dither ordered`). A
    /// change drops the encoded regions and repaints.
    pub fn set_dither(&mut self, on: bool) {
        if self.dither != on {
            self.dither = on;
            self.cache.map.clear();
            self.cache.bytes = 0;
            self.last = None;
        }
    }

    pub fn caps(&self) -> Caps {
        self.caps
    }

    /// Learn the cell size from the window size in pixels and cells; a change repaints images.
    pub fn set_cell_px(&mut self, xpix: usize, ypix: usize, cols: usize, rows: usize) {
        let px = if xpix >= cols && ypix >= rows && cols > 0 && rows > 0 { (xpix / cols, ypix / rows) } else { DEFAULT_CELL_PX };
        if self.cell_px != px {
            self.cell_px = px;
            self.last = None;
        }
    }

    /// Adopt what the client's terminal can show; a change forces a full repaint.
    pub fn set_caps(&mut self, caps: Caps) {
        if self.caps != caps {
            self.caps = caps;
            self.last = None;
        }
    }

    /// Forget what the client shows, forcing a full repaint on the next frame.
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    /// Returns the bytes to send, empty when nothing changed.
    pub fn frame(&mut self, screen: Screen) -> Vec<u8> {
        let full = self.last.as_ref().is_none_or(|l| l.cols != screen.cols || l.rows != screen.rows);
        let mut last = if full { None } else { self.last.as_ref() };

        let mut body = String::new();
        let mut emitter = Emitter { out: &mut body, style: Style::default(), pos: None, truecolor: self.caps.truecolor };
        if full {
            emitter.out.push_str("\x1b[0m\x1b[2J");
        }
        let moved;
        let mut scroll_done = false;
        if self.scroll
            && matches!(screen.images, ImageMode::Iterm2 | ImageMode::Sixel)
            && let Some(l) = last
            && let Some(s) = find_scroll(&screen, l)
        {
            // Colors reset so the rows scrolled in are blank in the default colors; the region
            // is set only for the scroll.
            let _ = write!(emitter.out, "\x1b[0m\x1b[{};{}r\x1b[{}{}\x1b[r", s.first + 1, s.last + 1, s.by.unsigned_abs(), if s.by > 0 { 'S' } else { 'T' });
            emitter.pos = None;
            moved = scrolled(l, s);
            last = Some(&moved);
            scroll_done = true;
        }
        // An inline picture replaces the cells it covers when it is drawn. Printing blanks there
        // first would erase the picture that is on the screen before the new one has arrived, and
        // the terminal can show the empty cells in between (the black flash while scrolling).
        let inline = screen.images == ImageMode::Iterm2;
        let covered = |s: &Screen, x: usize, y: usize| inline && s.piece(x, y).is_some();
        for y in 0..screen.rows {
            let mut x = 0;
            while x < screen.cols {
                if !differs(&screen, last, x, y) || covered(&screen, x, y) {
                    x += 1;
                    continue;
                }
                let mut start = x;
                if screen.cells[y * screen.cols + start].width == 0 && start > 0 {
                    start -= 1;
                }
                let mut end = x + 1;
                let mut z = end;
                while z < screen.cols && z <= end + GAP {
                    let differing = differs(&screen, last, z, y);
                    if covered(&screen, z, y) {
                        break;
                    } else if differing {
                        end = z + 1;
                    } else if screen.piece(z, y).is_some() {
                        // Printing through would blank an image cell that is fine as it is.
                        break;
                    }
                    z += 1;
                }
                if screen.cells[y * screen.cols + end - 1].width == 2 && end < screen.cols {
                    end += 1;
                }
                emitter.run(&screen, y, start, end);
                x = end;
            }
        }
        // Kitty placements float over the text and nothing overwrites them: when any picture
        // cell changes, all placements are deleted and every region is placed again. The pixels
        // stay on the terminal under their ids, so only a first sight of a crop sends them.
        // A full repaint, or too much stored, starts over: delete the data too and forget the ids.
        let floating = screen.images == ImageMode::Kitty;
        let any_piece = |s: &Screen, x, y| s.piece(x, y).is_some();
        let overfull = floating && (self.sent.len() >= MAX_SENT || self.sent_bytes > MAX_SENT_BYTES);
        let floating_changed = floating
            && (full
                || overfull
                || (0..screen.rows).any(|y| {
                    (0..screen.cols).any(|x| (any_piece(&screen, x, y) || last.is_some_and(|l| any_piece(l, x, y))) && differs(&screen, last, x, y))
                }));
        self.frames += 1;
        if floating_changed {
            if full || overfull {
                emitter.out.push_str(kitty::DELETE_ALL);
                self.sent.clear();
                self.sent_bytes = 0;
                self.whole.clear();
            } else if !self.kitty_source {
                emitter.out.push_str(kitty::DELETE_PLACEMENTS);
            }
        } else if !floating && (!self.sent.is_empty() || !self.whole.is_empty()) {
            // The client left Kitty graphics (its terminal changed under it): forget the ids.
            self.sent.clear();
            self.sent_bytes = 0;
            self.whole.clear();
        }
        // Parts of a picture held whole on the terminal need no bands: a placement is a rectangle.
        let band = if floating && self.kitty_source { 0 } else { self.band };
        let mut placed: HashMap<[usize; 5], u32> = HashMap::new();
        // After a scroll the picture cells that moved are right already: only the ones that came
        // into view are drawn, not the whole rectangle they happen to be part of.
        let regions = if scroll_done { regions_where(&screen, band, &|x, y| !differs(&screen, last, x, y)) } else { regions_banded(&screen, band) };
        for r in regions {
            let changed = if floating { floating_changed } else { (r.y..r.y + r.h).any(|y| (r.x..r.x + r.w).any(|x| differs(&screen, last, x, y))) };
            let Some(p) = screen.piece(r.x, r.y).filter(|_| changed) else { continue };
            let mode = screen.images;
            let key = [Arc::as_ptr(&p.bitmap) as usize, p.cols, p.rows, usize::from(p.tx), usize::from(p.ty), r.w, r.h, self.cell_px.0, self.cell_px.1, mode as usize];
            let (cell_px, tile, size) = (self.cell_px, (usize::from(p.tx), usize::from(p.ty)), (r.w, r.h));
            if self.failed.contains_key(&key) {
                emitter.blocks(r.x, r.y, r.w, r.h, p);
                continue;
            }
            if mode == ImageMode::Kitty && self.kitty_source && scaled::fits(&p.bitmap, p.cols, p.rows, cell_px) {
                let wkey = [Arc::as_ptr(&p.bitmap) as usize, p.cols, p.rows, cell_px.0, cell_px.1];
                if !self.whole.contains_key(&wkey) {
                    let id = self.next_id + 1;
                    match scaled::for_terminal(&p.bitmap, p.cols, p.rows, cell_px) {
                        Ok((png, size)) => {
                            self.next_id = id;
                            let seq = kitty::transmit_png_apc(&png, id);
                            emitter.out.push_str(&seq);
                            self.whole.insert(wkey, Whole { id, size, bytes: seq.len(), used: 0, slots: 0, bitmap: Arc::downgrade(&p.bitmap) });
                        }
                        Err(e) => {
                            log::warn(&format!("image region drawn with blocks: {e}"));
                            remember_failure(&mut self.failed, key, p);
                            emitter.blocks(r.x, r.y, r.w, r.h, p);
                            continue;
                        }
                    }
                }
                if let Some(w) = self.whole.get_mut(&wkey) {
                    w.used = self.frames;
                    let slot = placed.entry(wkey).or_insert(0);
                    *slot += 1;
                    let src = scaled::source_rect(w.size, p.cols, p.rows, tile, size);
                    emitter.image(r.x, r.y, &kitty::place_source_apc(w.id, *slot, src, size));
                }
                continue;
            }
            if mode == ImageMode::Kitty {
                let id = match self.sent.get(&key) {
                    Some((id, _)) => *id,
                    None => {
                        let id = self.next_id + 1;
                        match kitty::transmit_apc(&p.bitmap, p.cols, p.rows, tile, size, cell_px, id) {
                            Ok(seq) => {
                                self.next_id = id;
                                self.sent_bytes += seq.len();
                                emitter.out.push_str(&seq);
                                self.sent.insert(key, (id, Arc::downgrade(&p.bitmap)));
                                id
                            }
                            Err(e) => {
                                log::warn(&format!("image region drawn with blocks: {e}"));
                                remember_failure(&mut self.failed, key, p);
                                emitter.blocks(r.x, r.y, r.w, r.h, p);
                                continue;
                            }
                        }
                    }
                };
                emitter.image(r.x, r.y, &kitty::place_apc(id, r.w, r.h));
                continue;
            }
            let (scaled_map, scaled_bytes, dither) = (&mut self.scaled, &mut self.scaled_bytes, self.dither);
            let make = || match mode {
                // A part of a picture is cut from the picture scaled once; the whole picture in one
                // piece is simply resampled, which is the same single resample.
                ImageMode::Iterm2 if (r.w < p.cols || r.h < p.rows) && scaled::footprint(p.cols, p.rows, cell_px).is_some() => {
                    let img = scaled_picture(scaled_map, scaled_bytes, p, cell_px)?;
                    iterm::region_osc_scaled(&img, p.cols, p.rows, tile, size)
                }
                ImageMode::Iterm2 => iterm::region_osc(&p.bitmap, p.cols, p.rows, tile, size, cell_px),
                ImageMode::Sixel if (r.w < p.cols || r.h < p.rows) && scaled::footprint(p.cols, p.rows, cell_px).is_some() => {
                    let img = scaled_picture(scaled_map, scaled_bytes, p, cell_px)?;
                    sixel::region_dcs_scaled_dithered(&img, p.cols, p.rows, tile, size, dither)
                }
                ImageMode::Sixel => sixel::region_dcs_dithered(&p.bitmap, p.cols, p.rows, tile, size, cell_px, dither),
                ImageMode::Kitty | ImageMode::Blocks => Err("no escape sequence for this mode".to_string()),
            };
            match self.cache.get(key, &p.bitmap, make) {
                Some(seq) => emitter.image(r.x, r.y, seq),
                None => {
                    remember_failure(&mut self.failed, key, p);
                    emitter.blocks(r.x, r.y, r.w, r.h, p);
                }
            }
        }
        emitter.reset_style();
        if floating && floating_changed {
            // Placements the picture no longer has (a region it was split into, or all of them
            // when it scrolled away) go; the others were replaced above.
            for (key, w) in &mut self.whole {
                let now = placed.get(key).copied().unwrap_or(0);
                for slot in now + 1..=w.slots {
                    emitter.out.push_str(&kitty::delete_placement_apc(w.id, slot));
                }
                w.slots = now;
            }
        }
        if floating {
            trim_whole(&mut self.whole, emitter.out, self.frames);
        }

        let modes_changed = full || last.is_none_or(|l| l.modes != screen.modes);
        let cursor_changed = full || last.is_none_or(|l| l.cursor != screen.cursor || l.cursor_style != screen.cursor_style);
        if body.is_empty() && !modes_changed && !cursor_changed {
            self.last = Some(screen);
            return Vec::new();
        }

        if modes_changed {
            mirror_modes(&mut body, if full { None } else { last.map(|l| &l.modes) }, &screen.modes);
        }
        match screen.cursor {
            Some((x, y)) => {
                if full || last.is_none_or(|l| l.cursor_style != screen.cursor_style) {
                    let _ = write!(body, "\x1b[{} q", screen.cursor_style);
                }
                let _ = write!(body, "\x1b[{};{}H\x1b[?25h", y + 1, x + 1);
            }
            None => body.push_str("\x1b[?25l"),
        }

        let mut out = Vec::with_capacity(body.len() + 24);
        out.extend_from_slice(b"\x1b[?2026h\x1b[?25l");
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[?2026l");
        self.last = Some(screen);
        out
    }
}

fn differs(screen: &Screen, last: Option<&Screen>, x: usize, y: usize) -> bool {
    let Some(l) = last else { return true };
    let i = y * screen.cols + x;
    match (screen.piece(x, y), l.piece(x, y)) {
        (None, None) => l.cells[i] != screen.cells[i],
        (Some(a), Some(b)) => !a.same_cell(b),
        _ => true,
    }
}

/// A rectangle of cells showing one contiguous part of one image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

/// Group image cells into the fewest rectangles: each is drawn with one escape sequence, and cells
/// an overlay has taken back (no piece) split them.
#[cfg(test)]
fn regions(screen: &Screen) -> Vec<Region> {
    regions_banded(screen, 0)
}

/// `regions`, but no region is taller than `band` rows or crosses a row of the picture that is a
/// multiple of `band` (0: no such limit). A band then shows the same crop wherever it sits on the
/// screen, so its encoding is found again when the view scrolls, and something small laid over the
/// picture (copy mode's `[n/m]`) changes only the band it touches, not all of them.
fn regions_banded(screen: &Screen, band: usize) -> Vec<Region> {
    regions_where(screen, band, &|_, _| false)
}

/// `regions_banded` leaving out the cells `skip` says are already right on the terminal.
fn regions_where(screen: &Screen, band: usize, skip: &dyn Fn(usize, usize) -> bool) -> Vec<Region> {
    let mut done: Vec<bool> = (0..screen.cols * screen.rows).map(|i| skip(i % screen.cols, i / screen.cols)).collect();
    let mut out = Vec::new();
    for y in 0..screen.rows {
        for x in 0..screen.cols {
            let Some(p) = screen.piece(x, y) else { continue };
            if done[y * screen.cols + x] {
                continue;
            }
            let continues = |cx: usize, cy: usize| -> bool {
                cx < screen.cols
                    && cy < screen.rows
                    && !done[cy * screen.cols + cx]
                    && screen
                        .piece(cx, cy)
                        .is_some_and(|q| q.same_image(p) && usize::from(q.tx) == usize::from(p.tx) + (cx - x) && usize::from(q.ty) == usize::from(p.ty) + (cy - y))
            };
            let mut w = 1;
            while continues(x + w, y) {
                w += 1;
            }
            let mut h = 1;
            while (band == 0 || (usize::from(p.ty) + h) % band != 0) && (0..w).all(|dx| continues(x + dx, y + h)) {
                h += 1;
            }
            for cy in y..y + h {
                done[cy * screen.cols + x..cy * screen.cols + x + w].fill(true);
            }
            out.push(Region { x, y, w, h });
        }
    }
    out
}

/// Encoded images are kept for the next frames that show the same crop.
const CACHE_LIMIT: usize = 64 * 1024 * 1024;

/// A weak reference to the bitmap is held with its encoding, so its address, which is part of the
/// key, cannot be reused by another image while the entry lives; the pixels themselves are not
/// kept alive (strong references pinned pictures the pane had already evicted).
#[derive(Default)]
struct ImageCache {
    map: HashMap<[usize; 10], (Weak<Bitmap>, String)>,
    bytes: usize,
}

impl ImageCache {
    fn get(&mut self, key: [usize; 10], bitmap: &Arc<Bitmap>, make: impl FnOnce() -> Result<String, String>) -> Option<&str> {
        if !self.map.contains_key(&key) {
            match make() {
                Ok(osc) => {
                    if self.bytes + osc.len() > CACHE_LIMIT {
                        self.map.clear();
                        self.bytes = 0;
                    }
                    self.bytes += osc.len();
                    self.map.insert(key, (Arc::downgrade(bitmap), osc));
                }
                Err(e) => {
                    log::warn(&format!("image region drawn with blocks: {e}"));
                    return None;
                }
            }
        }
        self.map.get(&key).map(|(_, osc)| osc.as_str())
    }
}

struct Emitter<'a> {
    out: &'a mut String,
    style: Style,
    pos: Option<(usize, usize)>,
    truecolor: bool,
}

impl Emitter<'_> {
    fn run(&mut self, screen: &Screen, y: usize, start: usize, end: usize) {
        if self.pos != Some((start, y)) {
            let _ = write!(self.out, "\x1b[{};{}H", y + 1, start + 1);
        }
        for cell in &screen.cells[y * screen.cols + start..y * screen.cols + end] {
            if cell.width == 0 {
                continue;
            }
            if cell.style != self.style {
                sgr(self.out, &cell.style, self.truecolor);
                self.style = cell.style;
            }
            cell.push_to(self.out);
        }
        // Reaching the last column leaves the cursor in a terminal-defined spot: always re-address.
        self.pos = if end >= screen.cols { None } else { Some((end, y)) };
    }

    /// Draw the `w` x `h` cells of a picture region at (`x`, `y`) as quadrant blocks, the way a
    /// terminal that shows no pictures gets them. Used when the encoder failed for the region.
    fn blocks(&mut self, x: usize, y: usize, w: usize, h: usize, p: &Piece) {
        for dy in 0..h {
            let _ = write!(self.out, "\x1b[{};{}H", y + dy + 1, x + 1);
            for dx in 0..w {
                let cell = quad::tile_cell(&p.bitmap, p.cols, p.rows, usize::from(p.tx) + dx, usize::from(p.ty) + dy);
                if cell.style != self.style {
                    sgr(self.out, &cell.style, self.truecolor);
                    self.style = cell.style;
                }
                self.out.push(cell.ch);
            }
        }
        self.pos = None;
    }

    /// Draw an inline image with its top-left cell at (`x`, `y`). The cursor is saved around it:
    /// where the terminal leaves it afterwards is not something to rely on. The colors are reset
    /// first: a terminal gives the cells of a picture the colors in force while it is drawn, so a
    /// highlighted cell printed just before (copy mode's `[n/m]`) colored the picture's cells.
    fn image(&mut self, x: usize, y: usize, osc: &str) {
        self.reset_style();
        let _ = write!(self.out, "\x1b7\x1b[{};{}H{osc}\x1b8", y + 1, x + 1);
        self.pos = None;
    }

    fn reset_style(&mut self) {
        if self.style != Style::default() {
            self.out.push_str("\x1b[0m");
            self.style = Style::default();
        }
    }
}

pub fn sgr(out: &mut String, s: &Style, truecolor: bool) {
    out.push_str("\x1b[0");
    for (bit, code) in [
        (BOLD, 1),
        (DIM, 2),
        (ITALIC, 3),
        (UNDERLINE, 4),
        (BLINK, 5),
        (REVERSE, 7),
        (HIDDEN, 8),
        (STRIKE, 9),
    ] {
        if s.attrs & bit != 0 {
            let _ = write!(out, ";{code}");
        }
    }
    color(out, s.fg, 30, 90, 38, truecolor);
    color(out, s.bg, 40, 100, 48, truecolor);
    out.push('m');
}

fn color(out: &mut String, c: Color, base: u8, bright: u8, ext: u8, truecolor: bool) {
    let c = match c {
        Color::Rgb(r, g, b) if !truecolor => Color::Indexed(xterm::to_256(r, g, b)),
        other => other,
    };
    match c {
        Color::Default => {}
        Color::Indexed(n) if n < 8 => {
            let _ = write!(out, ";{}", base + n);
        }
        Color::Indexed(n) if n < 16 => {
            let _ = write!(out, ";{}", bright + n - 8);
        }
        Color::Indexed(n) => {
            let _ = write!(out, ";{ext};5;{n}");
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(out, ";{ext};2;{r};{g};{b}");
        }
    }
}

fn mirror_modes(out: &mut String, old: Option<&PaneModes>, new: &PaneModes) {
    let flag = |out: &mut String, old: Option<bool>, new: bool, code: u16| {
        if old != Some(new) {
            let _ = write!(out, "\x1b[?{code}{}", if new { 'h' } else { 'l' });
        }
    };
    flag(out, old.map(|o| o.cursor_keys), new.cursor_keys, 1);
    flag(out, old.map(|o| o.bracketed_paste), new.bracketed_paste, 2004);
    flag(out, old.map(|o| o.focus_events), new.focus_events, 1004);
    if old.map(|o| o.app_keypad) != Some(new.app_keypad) {
        out.push_str(if new.app_keypad { "\x1b=" } else { "\x1b>" });
    }
    if old.map(|o| o.mouse) != Some(new.mouse) {
        out.push_str("\x1b[?1000l\x1b[?1002l\x1b[?1003l");
        match new.mouse {
            MouseMode::None => {}
            MouseMode::Click => out.push_str("\x1b[?1000h"),
            MouseMode::Drag => out.push_str("\x1b[?1002h"),
            MouseMode::Any => out.push_str("\x1b[?1003h"),
        }
    }
    flag(out, old.map(|o| o.mouse_sgr), new.mouse_sgr, 1006);
}

#[cfg(test)]
mod tests {
    use super::super::compose;
    use super::*;
    use crate::term::Terminal;
    use crate::term::cell::Cell;

    /// The renderer's contract: replaying its frames into a plain terminal reproduces the screen.
    struct Harness {
        pane: Terminal,
        renderer: Renderer,
        client: Terminal,
        cols: usize,
        rows: usize,
    }

    impl Harness {
        fn new(pane_cols: usize, pane_rows: usize, cols: usize, rows: usize) -> Harness {
            Harness {
                pane: Terminal::new(pane_cols, pane_rows, 100),
                renderer: Renderer::new(),
                client: Terminal::new(cols, rows, 0),
                cols,
                rows,
            }
        }

        fn feed(&mut self, s: &str) -> usize {
            self.pane.feed(s.as_bytes());
            self.sync()
        }

        fn sync(&mut self) -> usize {
            let screen = compose(&self.pane.grid, self.cols, self.rows, " status ");
            let want: Vec<Cell> = screen.cells.clone();
            let cursor = screen.cursor;
            let bytes = self.renderer.frame(screen);
            self.client.feed(&bytes);
            let got: Vec<Cell> = self.client.grid.screen.iter().flat_map(|r| r.cells.iter().copied()).collect();
            assert_eq!(got.len(), want.len());
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                assert_eq!(g, w, "cell ({}, {}) differs", i % self.cols, i / self.cols);
            }
            if let Some((x, y)) = cursor {
                assert_eq!((self.client.grid.cx, self.client.grid.cy), (x, y), "cursor");
                assert!(self.client.grid.modes.cursor_visible);
            }
            bytes.len()
        }

        fn client_text(&self, y: usize) -> String {
            self.client.grid.screen[y].text()
        }
    }

    #[test]
    fn first_frame_paints_everything() {
        let mut h = Harness::new(10, 3, 10, 4);
        h.feed("hello\r\n\x1b[31mred\x1b[0m");
        assert_eq!(h.client_text(0), "hello");
        assert_eq!(h.client_text(1), "red");
        assert_eq!(h.client_text(3), " status");
    }

    #[test]
    fn unchanged_screen_emits_nothing() {
        let mut h = Harness::new(10, 3, 10, 4);
        h.feed("hi");
        assert_eq!(h.sync(), 0);
    }

    #[test]
    fn small_change_is_a_small_frame() {
        let mut h = Harness::new(40, 10, 40, 11);
        h.feed("the quick brown fox\r\njumps over\r\nthe lazy dog");
        let n = h.feed("\x1b[2;1HX");
        assert!(n < 60, "frame was {n} bytes");
        assert_eq!(h.client_text(1), "Xumps over");
    }

    #[test]
    fn scrolling_stays_in_sync() {
        let mut h = Harness::new(12, 4, 12, 5);
        for i in 0..20 {
            h.feed(&format!("line {i}\r\n"));
        }
        assert_eq!(h.client_text(2), "line 19");
    }

    #[test]
    fn colors_and_attributes_roundtrip() {
        let mut h = Harness::new(20, 2, 20, 3);
        h.feed("\x1b[1;4;38;2;10;20;30;48;5;200mabc\x1b[0m \x1b[92;104mdef\x1b[7mg");
    }

    #[test]
    fn wide_chars_roundtrip_and_partial_updates() {
        let mut h = Harness::new(10, 2, 10, 3);
        h.feed("日本語 ok");
        h.feed("\x1b[1;2Hx");
        h.feed("\x1b[1;1H日");
    }

    #[test]
    fn client_larger_than_pane_gets_filler() {
        let mut h = Harness::new(5, 2, 8, 4);
        h.feed("ab");
        assert_eq!(h.client_text(0), "ab   ...");
        assert_eq!(h.client_text(2), "........");
    }

    #[test]
    fn client_smaller_than_pane_is_clipped() {
        let mut h = Harness::new(10, 4, 4, 3);
        h.feed("abcdefghij\r\nsecond");
        assert_eq!(h.client_text(0), "abcd");
    }

    #[test]
    fn clipped_wide_char_leaves_no_half_cell() {
        let mut h = Harness::new(6, 2, 3, 3);
        h.feed("ab日");
        assert_eq!(h.client_text(0), "ab");
    }

    #[test]
    fn resize_forces_full_repaint() {
        let mut h = Harness::new(10, 3, 10, 4);
        h.feed("hello");
        h.cols = 12;
        h.rows = 5;
        h.client = Terminal::new(12, 5, 0);
        h.pane.grid.resize(12, 4, (0, 0));
        h.sync();
        assert_eq!(h.client_text(0), "hello");
    }

    #[test]
    fn cursor_hidden_and_shown() {
        let mut h = Harness::new(10, 2, 10, 3);
        h.feed("a\x1b[?25l");
        assert!(!h.client.grid.modes.cursor_visible);
        h.feed("\x1b[?25h");
        assert!(h.client.grid.modes.cursor_visible);
    }

    #[test]
    fn pane_modes_are_mirrored_to_the_client() {
        let mut h = Harness::new(10, 2, 10, 3);
        h.feed("\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?1h");
        let m = h.client.grid.modes;
        assert!(m.bracketed_paste && m.mouse_sgr && m.cursor_keys);
        assert_eq!(m.mouse, MouseMode::Drag);
        h.feed("\x1b[?2004l\x1b[?1002l");
        let m = h.client.grid.modes;
        assert!(!m.bracketed_paste);
        assert_eq!(m.mouse, MouseMode::None);
    }

    #[test]
    fn alt_screen_program_paints_and_returns() {
        let mut h = Harness::new(10, 3, 10, 4);
        h.feed("shell$ ");
        h.feed("\x1b[?1049h\x1b[2J\x1b[1;1Hfull screen\x1b[3;1Hbottom");
        assert_eq!(h.client_text(0), "full scree");
        h.feed("\x1b[?1049l");
        assert_eq!(h.client_text(0), "shell$");
    }

    #[test]
    fn last_column_cells_are_painted() {
        let mut h = Harness::new(5, 2, 5, 3);
        h.feed("abcde\r\nfghij");
        assert_eq!(h.client_text(0), "abcde");
        assert_eq!(h.client_text(1), "fghij");
    }

    #[test]
    fn combining_marks_reach_the_client_with_their_letter() {
        let mut h = Harness::new(6, 1, 6, 2);
        h.feed("cafe\u{301} \u{2764}\u{fe0f}");
        assert_eq!(h.client_text(0), "cafe\u{301} \u{2764}\u{fe0f}");
    }

    const ITERM: Caps = Caps { images: crate::proto::ImageMode::Iterm2, truecolor: true };

    fn image_pane(cols: usize, rows: usize, img: (usize, usize), at: &str) -> Terminal {
        let mut t = Terminal::new(cols, rows, 0);
        let mut b = Bitmap::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                b.set(x, y, [200, 100, 50, 255]);
            }
        }
        t.feed(at.as_bytes());
        t.grid.place_bitmap(b, img.0, img.1);
        t
    }

    fn iterm_screen(t: &Terminal) -> Screen {
        let g = &t.grid;
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let view = [super::super::PaneView { rect, grid: g, active: true, copy: None }];
        let st = Style::default();
        super::super::compose_window_with(&view, &[], g.cols, g.rows + 1, &[], 1, (st, st), crate::proto::ImageMode::Iterm2)
    }

    /// Time and size of frames while a large picture is browsed in copy mode. Not a check:
    /// `cargo test --release browsing_a_large -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn browsing_a_large_picture_in_copy_mode() {
        use crate::copymode::{CopyKeys, CopyMode};
        use std::io::Write;
        use std::time::Instant;
        let mut out = std::io::stdout();
        let (pw, ph) = (4000usize, 3000usize);
        let mut b = Bitmap::new(pw, ph);
        for y in 0..ph {
            for x in 0..pw {
                let n = (x * 7 + y * 3) as u32;
                b.set(x, y, [(n >> 2) as u8, (x / 16) as u8 ^ (y / 9) as u8, (n >> 4) as u8, 255]);
            }
        }
        let mut t = Terminal::new(200, 50, 5000);
        t.feed(b"some text\r\n");
        t.grid.place_bitmap(b, 200, 90);
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.cell_px = (14, 28);
        r.set_band(std::env::var("BAND").ok().and_then(|v| v.parse().ok()).unwrap_or(4));
        let g = &t.grid;
        let st = Style::default();
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let started = Instant::now();
        let live = [super::super::PaneView { rect, grid: g, active: true, copy: None }];
        let screen = super::super::compose_window_with(&live, &[], g.cols, g.rows + 1, &[], 1, (st, st), crate::proto::ImageMode::Iterm2);
        let regions_live = regions(&screen).len();
        let bytes = r.frame(screen);
        let _ = writeln!(out, "live frame: {:?}, {} KB, {regions_live} regions", started.elapsed(), bytes.len() >> 10);
        let mut m = CopyMode::enter_with(g, std::rc::Rc::new(CopyKeys::vi()));
        for step in 0..8 {
            let started = Instant::now();
            let view = m.view(g);
            let pane = [super::super::PaneView { rect, grid: g, active: true, copy: Some(&view) }];
            let screen = super::super::compose_window_with(&pane, &[], g.cols, g.rows + 1, &[], 1, (st, st), crate::proto::ImageMode::Iterm2);
            let n = regions_banded(&screen, 4).len();
            let bytes = r.frame(screen);
            let _ = writeln!(out, "copy mode step {step}: {:?}, {} KB, {n} regions", started.elapsed(), bytes.len() >> 10);
            m.scroll(g, -1);
        }
    }

    /// Same walk for an inline-image client when the terminal scrolls the cells (`BAND=n` for the
    /// bands). `cargo test --release browsing_a_large_picture_with_the_terminal -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn browsing_a_large_picture_with_the_terminal_scrolling() {
        use crate::copymode::{CopyKeys, CopyMode};
        use std::io::Write;
        use std::time::Instant;
        let mut out = std::io::stdout();
        let (pw, ph) = (4000usize, 3000usize);
        let mut b = Bitmap::new(pw, ph);
        for y in 0..ph {
            for x in 0..pw {
                let n = (x * 7 + y * 3) as u32;
                b.set(x, y, [(n >> 2) as u8, (x / 16) as u8 ^ (y / 9) as u8, (n >> 4) as u8, 255]);
            }
        }
        let mut t = Terminal::new(200, 50, 5000);
        t.feed(b"some text\r\n");
        t.grid.place_bitmap(b, 200, 90);
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.cell_px = (14, 28);
        r.set_scroll(true);
        r.set_band(std::env::var("BAND").ok().and_then(|v| v.parse().ok()).unwrap_or(4));
        let g = &t.grid;
        let st = Style::default();
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let live = [super::super::PaneView { rect, grid: g, active: true, copy: None }];
        let screen = super::super::compose_window_with(&live, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Iterm2);
        let started = Instant::now();
        let bytes = r.frame(screen);
        let _ = writeln!(out, "live frame: {:?}, {} KB", started.elapsed(), bytes.len() >> 10);
        let mut m = CopyMode::enter_with(g, std::rc::Rc::new(CopyKeys::vi()));
        for step in 0..10 {
            let started = Instant::now();
            let view = m.view(g);
            let pane = [super::super::PaneView { rect, grid: g, active: true, copy: Some(&view) }];
            let screen = super::super::compose_window_with(&pane, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Iterm2);
            let bytes = r.frame(screen);
            let _ = writeln!(out, "copy mode step {step}: {:?}, {} KB, {} pictures", started.elapsed(), bytes.len() >> 10, osc_count(&bytes));
            m.scroll(g, if step < 6 { -1 } else { 1 });
        }
    }

    /// Same walk as above for a Kitty client with source-rectangle placement: the picture goes
    /// once, a step is a placement. `cargo test --release browsing_a_large_picture_on -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn browsing_a_large_picture_on_a_kitty_client() {
        use crate::copymode::{CopyKeys, CopyMode};
        use std::io::Write;
        use std::time::Instant;
        let mut out = std::io::stdout();
        let (pw, ph) = (4000usize, 3000usize);
        let mut b = Bitmap::new(pw, ph);
        for y in 0..ph {
            for x in 0..pw {
                let n = (x * 7 + y * 3) as u32;
                b.set(x, y, [(n >> 2) as u8, (x / 16) as u8 ^ (y / 9) as u8, (n >> 4) as u8, 255]);
            }
        }
        let mut t = Terminal::new(200, 50, 5000);
        t.feed(b"some text\r\n");
        t.grid.place_bitmap(b, 200, 90);
        let mut r = Renderer::new();
        r.set_caps(Caps { images: ImageMode::Kitty, truecolor: true });
        r.cell_px = (14, 28);
        r.set_kitty_source(true);
        let g = &t.grid;
        let st = Style::default();
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let started = Instant::now();
        let live = [super::super::PaneView { rect, grid: g, active: true, copy: None }];
        let screen = super::super::compose_window_with(&live, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Kitty);
        let bytes = r.frame(screen);
        let _ = writeln!(out, "live frame: {:?}, {} KB", started.elapsed(), bytes.len() >> 10);
        let mut m = CopyMode::enter_with(g, std::rc::Rc::new(CopyKeys::vi()));
        for step in 0..8 {
            let started = Instant::now();
            let view = m.view(g);
            let pane = [super::super::PaneView { rect, grid: g, active: true, copy: Some(&view) }];
            let screen = super::super::compose_window_with(&pane, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Kitty);
            let bytes = r.frame(screen);
            let _ = writeln!(out, "copy mode step {step}: {:?}, {} bytes", started.elapsed(), bytes.len());
            m.scroll(g, -1);
        }
    }

    fn osc_count(bytes: &[u8]) -> usize {
        String::from_utf8_lossy(bytes).matches("\x1b]1337;File=").count()
    }

    #[test]
    fn a_region_the_encoder_fails_on_is_drawn_with_blocks_and_not_tried_again() {
        let t = image_pane(10, 5, (3, 2), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.cell_px = (0, 0);
        let mut client = Terminal::new(10, 6, 0);
        let bytes = r.frame(iterm_screen(&t));
        assert_eq!(osc_count(&bytes), 0, "nothing could be encoded");
        client.feed(&bytes);
        for (x, y) in [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)] {
            let c = client.grid.screen[y].cells[x];
            assert_eq!(c.tile, 0, "cell {x},{y} is text");
            assert!(c.ch != ' ' || c.style.bg != Color::Default, "cell {x},{y} shows the picture: {c:?}");
        }
        assert_eq!(r.failed.len(), 1);
        r.invalidate();
        let again = r.frame(iterm_screen(&t));
        assert_eq!(osc_count(&again), 0);
        let mut second = Terminal::new(10, 6, 0);
        second.feed(&again);
        let c = second.grid.screen[1].cells[2];
        assert!(c.tile == 0 && (c.ch != ' ' || c.style.bg != Color::Default), "a repaint draws the blocks again: {c:?}");
        assert_eq!(r.failed.len(), 1, "the region is remembered once");
    }

    #[test]
    fn an_image_is_one_region_and_is_only_sent_once() {
        let t = image_pane(10, 5, (3, 2), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        let s = iterm_screen(&t);
        assert_eq!(regions(&s), vec![Region { x: 0, y: 0, w: 3, h: 2 }]);
        let first = r.frame(s);
        assert_eq!(osc_count(&first), 1);
        assert!(r.frame(iterm_screen(&t)).is_empty(), "unchanged frame sends nothing");
    }

    #[test]
    fn a_picture_is_drawn_with_the_colors_reset_whatever_was_printed_before_it() {
        // A highlighted cell printed in the same frame (copy mode's `[n/m]`) left its background
        // in force, and the terminal gave it to the picture's cells: the orange background.
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.frame(iterm_screen(&image_pane(10, 6, (3, 2), "")));
        let mut t = image_pane(10, 6, (3, 2), "\r\n");
        t.feed(b"\x1b[6;1H\x1b[48;5;208mhi");
        let bytes = String::from_utf8_lossy(&r.frame(iterm_screen(&t))).into_owned();
        let at = bytes.find("\x1b]1337;File=").expect("the picture is drawn");
        let before = &bytes[..at];
        assert!(before.contains("48;5;208"), "the highlighted text is printed first: {before:?}");
        assert!(before.rfind("\x1b[0m") > before.rfind("\x1b[0;"), "colors are reset before the picture: {before:?}");
    }

    #[test]
    fn scrolling_a_picture_prints_no_blanks_over_the_cells_it_covers() {
        // Blanks printed first erase the picture on the screen before the new one has arrived, and
        // the terminal can show the empty cells in between (the black flash while scrolling).
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.frame(iterm_screen(&image_pane(10, 5, (3, 2), "")));
        let down = image_pane(10, 5, (3, 2), "\r\n");
        let bytes = String::from_utf8_lossy(&r.frame(iterm_screen(&down))).into_owned();
        assert_eq!(osc_count(bytes.as_bytes()), 1, "the picture is drawn at its new place");
        assert!(bytes.contains("\x1b[1;1H"), "the row the picture left is cleared: {bytes:?}");
        assert!(!bytes.contains("\x1b[3;1H"), "no blanks over the row the picture moved onto: {bytes:?}");
        assert!(r.frame(iterm_screen(&down)).is_empty(), "nothing more to send");
    }

    #[test]
    fn the_renderer_does_not_keep_the_pixels_of_a_picture_the_pane_dropped() {
        let t = image_pane(10, 5, (3, 2), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        assert_eq!(osc_count(&r.frame(iterm_screen(&t))), 1);
        let held: Vec<_> = r.cache.map.values().map(|(w, _)| w.clone()).collect();
        assert_eq!(held.len(), 1);
        assert!(held[0].upgrade().is_some(), "alive while the pane has it");
        let other = image_pane(10, 5, (3, 2), "");
        assert_eq!(osc_count(&r.frame(iterm_screen(&other))), 1, "another picture replaces it on the screen");
        drop(t);
        assert!(held[0].upgrade().is_none(), "freed once the pane drops it, the cache holds no strong reference");
    }

    #[test]
    fn the_client_ends_up_with_the_image_cells_and_the_cursor_is_left_alone() {
        let t = image_pane(10, 5, (3, 2), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        let mut client = Terminal::new(10, 6, 0);
        client.grid.cell_px = (8, 16);
        client.feed(&r.frame(iterm_screen(&t)));
        for (x, y) in [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)] {
            assert!(client.grid.screen[y].cells[x].tile != 0, "cell {x},{y}");
        }
        assert_eq!(client.grid.screen[0].cells[3].tile, 0);
        assert_eq!(client.grid.screen[2].cells[0].tile, 0);
    }

    #[test]
    fn an_overlay_over_an_image_splits_it_and_the_text_wins() {
        let t = image_pane(10, 5, (4, 1), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        let mut client = Terminal::new(10, 6, 0);
        client.grid.cell_px = (8, 16);
        client.feed(&r.frame(iterm_screen(&t)));

        let mut s = iterm_screen(&t);
        s.cells[1] = Cell { ch: 'X', ..Cell::BLANK };
        assert_eq!(regions(&s), vec![Region { x: 0, y: 0, w: 1, h: 1 }, Region { x: 2, y: 0, w: 2, h: 1 }]);
        let bytes = r.frame(s);
        assert_eq!(osc_count(&bytes), 0, "the pieces still on screen are not resent");
        client.feed(&bytes);
        assert_eq!(client.grid.screen[0].cells[1].ch, 'X');
        assert_eq!(client.grid.screen[0].cells[1].tile, 0);
        assert!(client.grid.screen[0].cells[0].tile != 0);
        assert!(client.grid.screen[0].cells[2].tile != 0);
    }

    #[test]
    fn bands_follow_the_pictures_own_rows_and_something_laid_over_one_band_leaves_the_others_alone() {
        let t = image_pane(10, 13, (6, 12), "");
        let plain = iterm_screen(&t);
        assert_eq!(regions_banded(&plain, 0), vec![Region { x: 0, y: 0, w: 6, h: 12 }]);
        let bands = regions_banded(&plain, 4);
        assert_eq!(bands, vec![Region { x: 0, y: 0, w: 6, h: 4 }, Region { x: 0, y: 4, w: 6, h: 4 }, Region { x: 0, y: 8, w: 6, h: 4 }]);

        let mut hole = iterm_screen(&t);
        hole.cells[5] = Cell { ch: '[', ..Cell::BLANK };
        let with_hole = regions_banded(&hole, 4);
        assert_eq!(with_hole[0], Region { x: 0, y: 0, w: 5, h: 4 }, "the band with the hole is cut around it");
        assert!(with_hole.contains(&Region { x: 5, y: 1, w: 1, h: 3 }));
        assert!(with_hole.contains(&Region { x: 0, y: 4, w: 6, h: 4 }) && with_hole.contains(&Region { x: 0, y: 8, w: 6, h: 4 }));
    }

    #[test]
    fn a_band_that_scrolled_is_found_in_the_cache_and_a_picture_in_bands_looks_the_same() {
        let mut t = image_pane(10, 13, (6, 12), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.set_band(4);
        let mut client = Terminal::new(10, 14, 0);
        client.grid.cell_px = (8, 16);
        let first = r.frame(iterm_screen(&t));
        assert_eq!(osc_count(&first), 3, "three bands");
        client.feed(&first);
        let cached = r.cache.map.len();
        r.invalidate();
        let again = r.frame(iterm_screen(&t));
        assert_eq!(osc_count(&again), 3);
        assert_eq!(r.cache.map.len(), cached, "a second sight of the same bands encodes nothing");
        for y in 0..12 {
            for x in 0..6 {
                assert!(client.grid.screen[y].cells[x].tile != 0, "cell {x},{y} shows the picture");
            }
        }
        t.feed(b"\x1b[1;8Hx");
        assert_eq!(osc_count(&r.frame(iterm_screen(&t))), 0, "text beside the picture resends nothing");
    }

    #[test]
    fn text_next_to_an_image_does_not_blank_it() {
        let mut t = image_pane(10, 5, (2, 1), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        let mut client = Terminal::new(10, 6, 0);
        client.grid.cell_px = (8, 16);
        client.feed(&r.frame(iterm_screen(&t)));

        t.feed(b"\x1b[1;5Hhi");
        let bytes = r.frame(iterm_screen(&t));
        client.feed(&bytes);
        assert_eq!(client.grid.screen[0].cells[4].ch, 'h');
        assert!(client.grid.screen[0].cells[0].tile != 0 && client.grid.screen[0].cells[1].tile != 0);
        assert_eq!(osc_count(&bytes), 0);
    }

    #[test]
    fn a_vanished_image_is_painted_over_with_what_is_under_it() {
        let mut t = image_pane(10, 5, (3, 2), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        let mut client = Terminal::new(10, 6, 0);
        client.grid.cell_px = (8, 16);
        client.feed(&r.frame(iterm_screen(&t)));

        t.feed(b"\x1b[2J");
        let bytes = r.frame(iterm_screen(&t));
        assert!(!bytes.is_empty());
        client.feed(&bytes);
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(client.grid.screen[y].cells[x].tile, 0, "cell {x},{y} still shows the image");
            }
        }
    }

    fn screen_in(t: &Terminal, mode: crate::proto::ImageMode) -> Screen {
        let g = &t.grid;
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let view = [super::super::PaneView { rect, grid: g, active: true, copy: None }];
        let st = Style::default();
        super::super::compose_window_with(&view, &[], g.cols, g.rows + 1, &[], 1, (st, st), mode)
    }

    #[test]
    fn kitty_placements_are_deleted_and_redrawn_when_a_picture_cell_changes() {
        let mode = ImageMode::Kitty;
        let mut r = Renderer::new();
        r.set_caps(Caps { images: mode, truecolor: true });
        let mut t = image_pane(10, 5, (3, 2), "");
        let first = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!((first.matches("\x1b_Ga=t,").count(), first.matches("\x1b_Ga=p,").count()), (1, 1));
        assert!(first.contains(kitty::DELETE_ALL));
        assert!(r.frame(screen_in(&t, mode)).is_empty());

        t.feed(b"\x1b[5;1Hnote");
        let text_only = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert!(!text_only.contains("\x1b_G"), "text elsewhere leaves placements alone");

        t.feed(b"\x1b[2J");
        let gone = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert!(gone.contains(kitty::DELETE_PLACEMENTS));
        assert_eq!((gone.matches("\x1b_Ga=t,").count(), gone.matches("\x1b_Ga=p,").count()), (0, 0));
    }

    #[test]
    fn kitty_redraws_what_is_left_after_a_scroll() {
        let mode = ImageMode::Kitty;
        let mut r = Renderer::new();
        r.set_caps(Caps { images: mode, truecolor: true });
        let mut t = image_pane(20, 8, (2, 1), "");
        t.feed(b"\x1b[5;5H");
        let mut b = Bitmap::new(8, 8);
        b.set(0, 0, [1, 2, 3, 255]);
        t.grid.place_bitmap(b, 2, 1);
        let first = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!(first.matches("\x1b_Ga=t,").count(), 2);
        t.feed(b"\x1b[3S");
        let again = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!(again.matches("\x1b_Ga=p,").count(), 1, "the first picture scrolled away, the second moved up");
        assert_eq!(again.matches("\x1b_Ga=t,").count(), 0, "its pixels are already on the terminal");
        assert_eq!(again.matches(kitty::DELETE_PLACEMENTS).count(), 1);
    }

    fn kitty_renderer() -> Renderer {
        let mut r = Renderer::new();
        r.set_caps(Caps { images: ImageMode::Kitty, truecolor: true });
        r
    }

    #[test]
    fn a_crop_shown_twice_is_stored_once_and_placed_twice() {
        let mode = ImageMode::Kitty;
        let mut r = kitty_renderer();
        let mut t = image_pane(20, 8, (2, 1), "");
        t.feed(b"\x1b[5;5H");
        let mut b = Bitmap::new(8, 8);
        b.set(0, 0, [200, 100, 50, 255]);
        t.grid.place_bitmap(b, 2, 1);
        let one = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!((one.matches("\x1b_Ga=t,").count(), one.matches("\x1b_Ga=p,").count()), (2, 2), "two bitmaps, two crops");

        let mut again = image_pane(20, 8, (2, 1), "");
        again.feed(b"\x1b[3;3H");
        let sent_before = r.sent.len();
        let moved = String::from_utf8(r.frame(screen_in(&again, mode))).unwrap();
        assert_eq!(moved.matches("\x1b_Ga=p,").count(), 1);
        assert_eq!(r.sent.len(), sent_before + usize::from(moved.contains("\x1b_Ga=t,")));
    }

    #[test]
    fn kitty_ids_are_never_reused_and_a_full_repaint_starts_over() {
        let mode = ImageMode::Kitty;
        let mut r = kitty_renderer();
        let t = image_pane(10, 5, (3, 2), "");
        let first = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert!(first.contains("\x1b_Ga=t,i=1,"), "{first:?}");
        r.invalidate();
        let second = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert!(second.contains(kitty::DELETE_ALL), "data is freed before it is sent again");
        assert!(second.contains("\x1b_Ga=t,i=2,") && second.contains("\x1b_Ga=p,i=2,"), "{second:?}");
        assert_eq!(r.sent.len(), 1, "the old id is forgotten");
    }

    #[test]
    fn too_much_stored_on_the_terminal_deletes_it_all_and_sends_again() {
        let mode = ImageMode::Kitty;
        let mut r = kitty_renderer();
        let mut t = image_pane(10, 5, (3, 2), "");
        r.frame(screen_in(&t, mode));
        r.sent_bytes = MAX_SENT_BYTES + 1;
        let bytes = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert!(bytes.contains(kitty::DELETE_ALL) && bytes.contains("\x1b_Ga=t,"), "{bytes:?}");
        assert!(r.sent_bytes <= MAX_SENT_BYTES);
        t.feed(b"\x1b[5;1Hx");
        assert!(!String::from_utf8(r.frame(screen_in(&t, mode))).unwrap().contains("\x1b_G"));
    }

    #[test]
    fn a_kitty_client_ends_up_with_the_picture_and_later_frames_resend_no_pixels() {
        let mode = ImageMode::Kitty;
        let mut r = kitty_renderer();
        let mut t = image_pane(10, 5, (3, 2), "");
        let mut client = Terminal::new(10, 6, 0);
        client.grid.cell_px = (8, 16);
        client.feed(&r.frame(screen_in(&t, mode)));
        for (x, y) in [(0, 0), (2, 0), (0, 1), (2, 1)] {
            assert!(client.grid.screen[y].cells[x].tile != 0, "cell {x},{y}");
        }
        t.feed(b"\x1b[1;5Hhi");
        let bytes = r.frame(screen_in(&t, mode));
        assert!(!String::from_utf8_lossy(&bytes).contains("a=t,"), "no pixels again");
        client.feed(&bytes);
        assert!(client.grid.screen[0].cells[0].tile != 0 && client.grid.screen[1].cells[2].tile != 0, "placed again by id");
        assert_eq!(client.grid.screen[0].cells[4].ch, 'h');
        assert_eq!(client.grid.screen[2].cells[0].tile, 0);
    }

    fn tall_picture_pane() -> Terminal {
        let mut t = Terminal::new(20, 8, 0);
        let mut b = Bitmap::new(160, 384);
        for y in 0..384 {
            for x in 0..160 {
                b.set(x, y, [(x * 255 / 160) as u8, (y * 255 / 384) as u8, 90, 255]);
            }
        }
        t.grid.place_bitmap(b, 10, 12);
        t
    }

    fn source_renderer() -> Renderer {
        let mut r = kitty_renderer();
        r.set_kitty_source(true);
        r
    }

    /// The number after `key=` in the `n`th placement of a frame.
    fn placement_value(frame: &str, n: usize, key: &str) -> usize {
        let at = frame.match_indices("\x1b_Ga=p,").nth(n).expect("placement").0;
        let seq = &frame[at..frame[at..].find("\x1b\\").map_or(frame.len(), |e| at + e)];
        let from = seq.find(&format!(",{key}=")).expect("key") + key.len() + 2;
        seq[from..].split(',').next().unwrap().parse().unwrap()
    }

    #[test]
    fn a_whole_picture_is_stored_once_and_scrolling_it_sends_a_placement_and_no_pixels() {
        let mode = ImageMode::Kitty;
        let mut r = source_renderer();
        let mut t = tall_picture_pane();
        let first = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!(first.matches("\x1b_Ga=t,").count(), 1, "the picture goes once");
        assert_eq!(first.matches("\x1b_Ga=p,").count(), 1, "one placement, no bands: {first:?}");
        assert!(first.contains("f=100"));
        let y1 = placement_value(&first, 0, "y");
        for _ in 0..4 {
            t.feed(b"\x1b[S");
            let step = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
            assert_eq!(step.matches("\x1b_Ga=t,").count(), 0, "no pixels on a scroll step");
            assert!(step.len() < 300, "{} bytes: {step:?}", step.len());
            assert!(!step.contains("a=d,"), "the placement is replaced, never deleted first: {step:?}");
            assert!(step.contains("a=p,i=1,p=1,"), "same ids every time: {step:?}");
        }
        t.feed(b"\x1b[S");
        let last = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!(placement_value(&last, 0, "y"), y1 + 5 * 16, "five cell rows of 16 px further down the picture");
    }

    #[test]
    fn a_picture_that_scrolled_away_loses_its_placements_and_one_that_split_loses_the_extra_one() {
        let mode = ImageMode::Kitty;
        let mut r = source_renderer();
        let mut t = tall_picture_pane();
        let _ = r.frame(screen_in(&t, mode));
        t.feed(b"\x1b[3;1Hhi");
        let split = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!(split.matches("a=p,i=1,").count(), 3, "text in the middle of the picture splits the placement: {split:?}");
        t.feed(b"\x1b[20S");
        let away = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!(away.matches("a=p,").count(), 0);
        for slot in 1..=3 {
            assert!(away.contains(&format!("a=d,d=i,i=1,p={slot},q=2")), "placement {slot} goes: {away:?}");
        }
        assert!(r.frame(screen_in(&t, mode)).is_empty());
    }

    #[test]
    fn a_whole_picture_is_sent_at_the_size_of_its_footprint_and_cut_on_cell_edges() {
        let mode = ImageMode::Kitty;
        let mut r = source_renderer();
        r.cell_px = (8, 16);
        let t = tall_picture_pane();
        let frame = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        let (w, h) = (placement_value(&frame, 0, "w"), placement_value(&frame, 0, "h"));
        assert_eq!((w, h), (80, 7 * 16), "10 x 7 cells at 8 x 16 px: the picture was scaled once to 80 x 192 and cut where the cells are");
        assert_eq!((placement_value(&frame, 0, "c"), placement_value(&frame, 0, "r")), (10, 7));
        assert_eq!(placement_value(&frame, 0, "y") % 16, 0);
    }

    #[test]
    fn crop_placement_stays_what_it_was_when_asked_for() {
        let mode = ImageMode::Kitty;
        let mut r = kitty_renderer();
        let t = tall_picture_pane();
        let frame = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert!(!frame.contains(",x=") && !frame.contains(",w="), "{frame:?}");
        assert_eq!(frame.matches("\x1b_Ga=t,").count(), 1);
    }

    #[test]
    fn a_picture_the_pane_dropped_is_freed_on_the_terminal() {
        let mode = ImageMode::Kitty;
        let mut r = source_renderer();
        let mut t = tall_picture_pane();
        let _ = r.frame(screen_in(&t, mode));
        assert!(t.grid.images.evict_oldest());
        let _ = r.frame(screen_in(&t, mode));
        let later = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert!(later.contains("\x1b_Ga=d,d=I,i=1,q=2"), "{later:?}");
        let after = r.frame(screen_in(&t, mode));
        assert!(after.is_empty(), "freed once");
    }

    #[test]
    fn too_many_pictures_on_the_terminal_free_the_idle_ones_and_never_the_one_in_use() {
        let mut whole = HashMap::new();
        let keep: Vec<Arc<Bitmap>> = (0..MAX_WHOLE + 3).map(|_| Arc::new(Bitmap::new(1, 1))).collect();
        for (i, b) in keep.iter().enumerate() {
            whole.insert([i, 1, 1, 8, 16], Whole { id: i as u32 + 1, size: (1, 1), bytes: 10, used: if i == 0 { 9 } else { 1 }, slots: 0, bitmap: Arc::downgrade(b) });
        }
        let mut out = String::new();
        trim_whole(&mut whole, &mut out, 9);
        assert_eq!(whole.len(), MAX_WHOLE);
        assert!(whole.contains_key(&[0, 1, 1, 8, 16]), "placed this frame, kept although it is the oldest");
        assert_eq!(out.matches("a=d,d=I").count(), 3);
        assert!(out.contains("i=2,") && out.contains("i=3,") && out.contains("i=4,"), "the oldest idle ones go first: {out:?}");
    }

    fn striped_history_pane() -> Terminal {
        let mut b = Bitmap::new(160, 384);
        for y in 0..384 {
            let row = (y / 32) as u8;
            for x in 0..160 {
                b.set(x, y, [row * 20 + 10, 255 - row * 20, 50, 255]);
            }
        }
        let mut t = Terminal::new(20, 8, 500);
        t.feed(b"text\r\n");
        t.grid.place_bitmap(b, 10, 12);
        t.feed(b"after\r\n");
        t
    }

    /// The color the client shows in the middle of the cell at (x, y), if it holds a picture tile.
    fn client_color(client: &Terminal, x: usize, y: usize) -> Option<[u8; 4]> {
        let tile = client.grid.images.lookup(client.grid.screen[y].cells[x].tile)?;
        let placed = client.grid.images.image(tile.image)?;
        let (cw, ch) = (placed.bitmap.w / placed.cols, placed.bitmap.h / placed.rows);
        Some(placed.bitmap.pixel(usize::from(tile.tx) * cw + cw / 2, usize::from(tile.ty) * ch + ch / 2))
    }

    #[test]
    fn scrolling_the_terminal_keeps_every_picture_row_right_and_sends_only_the_rows_that_came_into_view() {
        use crate::copymode::{CopyKeys, CopyMode};
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.set_scroll(true);
        r.set_band(0);
        let t = striped_history_pane();
        let g = &t.grid;
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let st = Style::default();
        let mut client = Terminal::new(g.cols, g.rows + 1, 0);
        client.grid.cell_px = (8, 16);
        let mut m = CopyMode::enter_with(g, std::rc::Rc::new(CopyKeys::vi()));
        let mut scrolls = 0;
        for (step, delta) in [0isize, -1, -1, -2, -1, 1, 1, 3, -4, 1].into_iter().enumerate() {
            m.scroll(g, delta);
            let view = m.view(g);
            let pane = [super::super::PaneView { rect, grid: g, active: true, copy: Some(&view) }];
            let screen = super::super::compose_window_with(&pane, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Iterm2);
            let want: Vec<Option<[u8; 4]>> = (0..screen.rows * screen.cols)
                .map(|i| screen.piece(i % screen.cols, i / screen.cols).map(|p| p.bitmap.pixel(0, p.bitmap.h / p.rows * usize::from(p.ty) + 16)))
                .collect();
            let bytes = r.frame(screen);
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if step > 0 && delta != 0 && text.contains("r\x1b[") {
                scrolls += 1;
                assert!(osc_count(&bytes) <= delta.unsigned_abs(), "step {step}: {} pictures for a scroll of {delta}", osc_count(&bytes));
            }
            client.feed(&bytes);
            for (i, w) in want.iter().enumerate() {
                let (x, y) = (i % g.cols.max(1), i / g.cols.max(1));
                if y >= g.rows {
                    continue;
                }
                assert_eq!(client_color(&client, x, y), *w, "step {step} (scroll {delta}) cell {x},{y}");
            }
        }
        assert!(scrolls >= 5, "the terminal did the scrolling in {scrolls} frames");
    }

    #[test]
    fn a_sixel_client_gets_the_same_scrolling_and_the_same_right_rows() {
        use crate::copymode::{CopyKeys, CopyMode};
        let mut r = Renderer::new();
        r.set_caps(Caps { images: ImageMode::Sixel, truecolor: true });
        r.set_scroll(true);
        r.set_band(0);
        let t = striped_history_pane();
        let g = &t.grid;
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let st = Style::default();
        let mut client = Terminal::new(g.cols, g.rows + 1, 0);
        client.grid.cell_px = (8, 16);
        let mut m = CopyMode::enter_with(g, std::rc::Rc::new(CopyKeys::vi()));
        let mut scrolls = 0;
        // Sixel colors are percentages: a value comes back within 3 of what was sent.
        let near = |a: Option<[u8; 4]>, b: Option<[u8; 4]>| match (a, b) {
            (Some(a), Some(b)) => a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 3),
            (a, b) => a == b,
        };
        for (step, delta) in [0isize, -1, -1, -2, -1, 1, 1, 3, -4, 1].into_iter().enumerate() {
            m.scroll(g, delta);
            let view = m.view(g);
            let pane = [super::super::PaneView { rect, grid: g, active: true, copy: Some(&view) }];
            let screen = super::super::compose_window_with(&pane, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Sixel);
            let want: Vec<Option<[u8; 4]>> = (0..screen.rows * screen.cols)
                .map(|i| screen.piece(i % screen.cols, i / screen.cols).map(|p| p.bitmap.pixel(0, p.bitmap.h / p.rows * usize::from(p.ty) + 16)))
                .collect();
            let bytes = r.frame(screen);
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if step > 0 && delta != 0 && text.contains("r\x1b[") {
                scrolls += 1;
                assert!(text.matches("\x1bP").count() <= delta.unsigned_abs(), "step {step}: {} pictures for a scroll of {delta}", text.matches("\x1bP").count());
            }
            client.feed(&bytes);
            for (i, w) in want.iter().enumerate() {
                let (x, y) = (i % g.cols.max(1), i / g.cols.max(1));
                if y < g.rows {
                    let got = client_color(&client, x, y);
                    assert!(near(got, *w), "step {step} (scroll {delta}) cell {x},{y}: {got:?} not {w:?}");
                }
            }
        }
        assert!(scrolls >= 5, "the terminal did the scrolling in {scrolls} frames");
    }

    #[test]
    fn without_the_option_every_step_draws_its_pictures_the_old_way() {
        use crate::copymode::{CopyKeys, CopyMode};
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        let t = striped_history_pane();
        let g = &t.grid;
        let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
        let st = Style::default();
        let mut m = CopyMode::enter_with(g, std::rc::Rc::new(CopyKeys::vi()));
        let mut frame = |m: &CopyMode| {
            let view = m.view(g);
            let pane = [super::super::PaneView { rect, grid: g, active: true, copy: Some(&view) }];
            r.frame(super::super::compose_window_with(&pane, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Iterm2))
        };
        let _ = frame(&m);
        m.scroll(g, -1);
        let step = String::from_utf8(frame(&m)).unwrap();
        assert!(!step.contains(";7r") && !step.contains("\x1b[1S") && !step.contains("\x1b[1T"), "{step:?}");
    }

    fn pictures_in(frame: &[u8]) -> Vec<Bitmap> {
        let text = String::from_utf8_lossy(frame).into_owned();
        text.split("\x1b]1337;File=")
            .skip(1)
            .map(|s| {
                let body = s.split('\x07').next().unwrap();
                let file = crate::image::iterm::parse(body.as_bytes()).expect("iterm2 file");
                crate::image::decode_image(&file.data).expect("png")
            })
            .collect()
    }

    fn stacked(pictures: &[Bitmap]) -> Bitmap {
        let w = pictures[0].w;
        let h: usize = pictures.iter().map(|p| p.h).sum();
        let mut out = Bitmap::new(w, h);
        let mut y0 = 0;
        for p in pictures {
            out.rgba[y0 * w * 4..(y0 + p.h) * w * 4].copy_from_slice(&p.rgba);
            y0 += p.h;
        }
        out
    }

    #[test]
    fn inline_bands_are_cut_from_one_scaled_picture_so_any_banding_shows_the_same_pixels() {
        let mut shown = Vec::new();
        for band in [4, 2, 3] {
            let mut r = Renderer::new();
            r.set_caps(ITERM);
            r.set_band(band);
            let t = tall_picture_pane();
            let g = &t.grid;
            let rect = crate::session::layout::Rect { x: 0, y: 0, w: g.cols as u16, h: g.rows as u16 };
            let view = [super::super::PaneView { rect, grid: g, active: true, copy: None }];
            let st = Style::default();
            let screen = super::super::compose_window_with(&view, &[], g.cols, g.rows + 1, &[], 1, (st, st), ImageMode::Iterm2);
            let pics = pictures_in(&r.frame(screen));
            assert_eq!(pics.len(), 8usize.div_ceil(band).max(2), "band {band}");
            shown.push(stacked(&pics));
        }
        assert_eq!(shown[0], shown[1], "no seam between bands, whatever their height");
        assert_eq!(shown[0], shown[2]);
    }

    #[test]
    fn sixel_clients_get_a_dcs_per_region_and_it_is_stamped_like_text() {
        let mode = ImageMode::Sixel;
        let mut r = Renderer::new();
        r.set_caps(Caps { images: mode, truecolor: true });
        let t = image_pane(10, 5, (3, 2), "");
        let bytes = String::from_utf8(r.frame(screen_in(&t, mode))).unwrap();
        assert_eq!(bytes.matches("\x1bP0;1;0q").count(), 1);
        assert!(!bytes.contains(kitty::DELETE_ALL));
        assert!(r.frame(screen_in(&t, mode)).is_empty());
    }

    #[test]
    fn a_moved_image_is_resent_where_it_now_is() {
        let t = image_pane(10, 5, (2, 1), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.frame(iterm_screen(&t));
        let moved = image_pane(10, 5, (2, 1), "\x1b[3;4H");
        let bytes = String::from_utf8(r.frame(iterm_screen(&moved))).unwrap();
        assert_eq!(bytes.matches("\x1b]1337;File=").count(), 1);
        assert!(bytes.contains("\x1b7\x1b[3;4H\x1b]1337;"), "{}", &bytes[..bytes.len().min(60)]);
    }

    #[test]
    fn a_cell_size_change_resends_images_at_the_new_size() {
        let t = image_pane(10, 5, (2, 1), "");
        let mut r = Renderer::new();
        r.set_caps(ITERM);
        r.set_cell_px(800, 480, 100, 30);
        let a = r.frame(iterm_screen(&t));
        r.set_cell_px(1000, 600, 100, 30);
        let b = r.frame(iterm_screen(&t));
        assert_eq!((osc_count(&a), osc_count(&b)), (1, 1));
        assert_ne!(a, b);
        r.set_cell_px(0, 0, 100, 30);
        assert_eq!(r.cell_px, DEFAULT_CELL_PX);
    }

    #[test]
    fn without_truecolor_rgb_goes_out_as_256_colors_and_a_caps_change_repaints() {
        let mut pane = Terminal::new(4, 1, 0);
        pane.feed(b"\x1b[38;2;255;0;0mx");
        let mut r = Renderer::new();
        r.set_caps(Caps::BASIC);
        let bytes = String::from_utf8(r.frame(compose(&pane.grid, 4, 2, "s"))).unwrap();
        assert!(bytes.contains("38;5;196"), "{bytes:?}");
        assert!(!bytes.contains("38;2;"), "{bytes:?}");

        assert!(r.frame(compose(&pane.grid, 4, 2, "s")).is_empty());
        r.set_caps(Caps { truecolor: true, ..Caps::BASIC });
        let bytes = String::from_utf8(r.frame(compose(&pane.grid, 4, 2, "s"))).unwrap();
        assert!(bytes.contains("38;2;255;0;0"), "{bytes:?}");
    }
}
