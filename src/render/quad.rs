//! Drawing an image with text: each cell shows a 2x2 block of samples as one quadrant glyph with a
//! foreground and a background color. Works on any terminal with block characters.

use crate::image::Bitmap;
use crate::term::cell::{Cell, Color, Style};

/// Quadrant glyphs indexed by which samples are in the foreground: bit 0 top-left, 1 top-right,
/// 2 bottom-left, 3 bottom-right.
const GLYPHS: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

type Rgb = [f32; 3];

fn luma(c: Rgb) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// Mean color of the pixels in `[x0, x1) x [y0, y1)`, weighted by alpha; `None` when the box is
/// mostly transparent. Boxes are widened to at least one pixel.
fn sample(b: &Bitmap, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Rgb> {
    let px0 = (x0.floor() as usize).min(b.w - 1);
    let py0 = (y0.floor() as usize).min(b.h - 1);
    let px1 = (x1.ceil() as usize).clamp(px0 + 1, b.w);
    let py1 = (y1.ceil() as usize).clamp(py0 + 1, b.h);
    let (mut sum, mut alpha, mut n) = ([0f32; 3], 0f32, 0f32);
    for y in py0..py1 {
        for x in px0..px1 {
            let p = b.pixel(x, y);
            let a = f32::from(p[3]);
            for (s, &c) in sum.iter_mut().zip(&p[..3]) {
                *s += f32::from(c) * a;
            }
            alpha += a;
            n += 1.0;
        }
    }
    (alpha / n >= 128.0).then(|| sum.map(|s| s / alpha))
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c[0].round() as u8, c[1].round() as u8, c[2].round() as u8)
}

fn mean(colors: &[Rgb]) -> Rgb {
    let n = colors.len() as f32;
    let mut m = [0f32; 3];
    for c in colors {
        for (s, v) in m.iter_mut().zip(c) {
            *s += v / n;
        }
    }
    m
}

/// Which block characters a cell is drawn with: two samples across and two (quadrants), three
/// (sextants) or four (octants) down. More samples are finer, and need a font that has the
/// characters (`image-glyphs`); a terminal cannot be asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Glyphs {
    #[default]
    Quadrant,
    Sextant,
    Octant,
}

impl Glyphs {
    pub fn parse(name: &str) -> Glyphs {
        match name {
            "sextant" => Glyphs::Sextant,
            "octant" => Glyphs::Octant,
            _ => Glyphs::Quadrant,
        }
    }

    /// Samples down a cell.
    fn rows(self) -> usize {
        match self {
            Glyphs::Quadrant => 2,
            Glyphs::Sextant => 3,
            Glyphs::Octant => 4,
        }
    }

    /// The character for a set of samples: bit `row * 2 + column` is set where the sample is in the
    /// foreground, counted from the top left.
    fn glyph(self, mask: usize) -> char {
        match self {
            Glyphs::Quadrant => GLYPHS[mask & 15],
            Glyphs::Sextant => sextant(mask as u8 & 63),
            Glyphs::Octant => octant(mask as u8),
        }
    }
}

/// Sextants are U+1FB00 on in the order of their patterns, without the four patterns that were
/// characters already: blank, the left half, the right half and the full block.
fn sextant(mask: u8) -> char {
    match mask {
        0 => ' ',
        21 => '▌',
        42 => '▐',
        63 => '█',
        m => char::from_u32(0x1FB00 + u32::from(m) - 1 - u32::from(m > 21) - u32::from(m > 42)).unwrap_or(' '),
    }
}

/// The 24 octant patterns that already had a character (blank and full excepted), and the
/// character: Unicode leaves them out of the octant block, which starts at U+1CD00 and runs through
/// the other 230 patterns in order.
const OCTANT_OLD: [(u8, char); 24] = [
    (1, '\u{1CEA8}'),
    (2, '\u{1CEAB}'),
    (3, '\u{1FB82}'),
    (5, '▘'),
    (10, '▝'),
    (15, '▀'),
    (20, '\u{1FBE6}'),
    (40, '\u{1FBE7}'),
    (63, '\u{1FB85}'),
    (64, '\u{1CEA3}'),
    (80, '▖'),
    (85, '▌'),
    (90, '▞'),
    (95, '▛'),
    (128, '\u{1CEA0}'),
    (160, '▗'),
    (165, '▚'),
    (170, '▐'),
    (175, '▜'),
    (192, '▂'),
    (240, '▄'),
    (245, '▙'),
    (250, '▟'),
    (252, '▆'),
];

fn octant(mask: u8) -> char {
    match mask {
        0 => ' ',
        255 => '█',
        m => match OCTANT_OLD.iter().find(|(p, _)| *p == m) {
            Some((_, c)) => *c,
            None => {
                let before = OCTANT_OLD.iter().filter(|(p, _)| *p < m).count() as u32;
                char::from_u32(0x1CD00 + u32::from(m) - 1 - before).unwrap_or(' ')
            }
        },
    }
}

static GLYPHS_IN_USE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The block characters `tile_cell` uses (`image-glyphs`).
pub fn set_glyphs(g: Glyphs) {
    GLYPHS_IN_USE.store(g as u8, std::sync::atomic::Ordering::Relaxed);
}

static DITHER_IN_USE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether `tile_cell` dithers the colors it makes (`image-dither ordered`). Set for the client
/// about to be drawn, and only when that client cannot show 24-bit color: the colors are then
/// mapped to the 256-color palette, which is what the pattern is for.
pub fn set_dither(on: bool) {
    DITHER_IN_USE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// A color of tile (`tx`, `ty`), moved by the ordered-dither threshold of that tile when dithering
/// is on. The tile is the position, so the pattern is the picture's and does not move on screen.
fn dithered(c: Rgb, tx: usize, ty: usize) -> Rgb {
    let d = super::dither::offset(tx, ty, super::dither::BLOCK_SPAN) as f32;
    c.map(|v| (v + d).clamp(0.0, 255.0))
}

fn glyphs_in_use() -> Glyphs {
    match GLYPHS_IN_USE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Glyphs::Sextant,
        2 => Glyphs::Octant,
        _ => Glyphs::Quadrant,
    }
}

/// The cell for tile (`tx`, `ty`) of an image placed over `cols` x `rows` cells, with the block
/// characters in use. The bitmap is stretched over the footprint, so a tile is a fixed fraction of it.
pub fn tile_cell(b: &Bitmap, cols: usize, rows: usize, tx: usize, ty: usize) -> Cell {
    tile_cell_dithered(b, cols, rows, tx, ty, glyphs_in_use(), DITHER_IN_USE.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn tile_cell_with(b: &Bitmap, cols: usize, rows: usize, tx: usize, ty: usize, glyphs: Glyphs) -> Cell {
    tile_cell_dithered(b, cols, rows, tx, ty, glyphs, false)
}

/// `tile_cell_with`, with the colors moved by the ordered-dither threshold of the tile when
/// `dither` is set (see `set_dither`).
pub fn tile_cell_dithered(b: &Bitmap, cols: usize, rows: usize, tx: usize, ty: usize, glyphs: Glyphs, dither: bool) -> Cell {
    let tint = |c: Rgb| if dither { dithered(c, tx, ty) } else { c };
    let (cols, rows) = (cols.max(1) as f32, rows.max(1) as f32);
    let (cw, ch) = (b.w as f32 / cols, b.h as f32 / rows);
    let (left, top) = (tx as f32 * cw, ty as f32 * ch);
    let down = glyphs.rows();
    let (sw, sh) = (cw / 2.0, ch / down as f32);
    let samples: Vec<Option<Rgb>> =
        (0..down * 2).map(|i| sample(b, left + (i % 2) as f32 * sw, top + (i / 2) as f32 * sh, left + (i % 2 + 1) as f32 * sw, top + (i / 2 + 1) as f32 * sh)).collect();

    let solid: Vec<Rgb> = samples.iter().flatten().copied().collect();
    if solid.is_empty() {
        return Cell { tile: 0, ..Cell::BLANK };
    }
    let lumas: Vec<f32> = solid.iter().map(|&c| luma(c)).collect();
    let (lo, hi) = lumas.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &l| (lo.min(l), hi.max(l)));
    let mid = (lo + hi) / 2.0;
    let transparent = solid.len() < samples.len();

    let mut mask = 0usize;
    let (mut on, mut off) = (Vec::new(), Vec::new());
    for (i, s) in samples.iter().enumerate() {
        match s {
            Some(c) if transparent || hi - lo < 1.0 || luma(*c) > mid => {
                mask |= 1 << i;
                on.push(*c);
            }
            Some(c) => off.push(*c),
            None => {}
        }
    }
    let style = if transparent {
        Style { fg: rgb(tint(mean(&on))), ..Style::default() }
    } else if off.is_empty() {
        Style { bg: rgb(tint(mean(&on))), ..Style::default() }
    } else {
        Style { fg: rgb(tint(mean(&on))), bg: rgb(tint(mean(&off))), ..Style::default() }
    };
    let ch = if !transparent && off.is_empty() { ' ' } else { glyphs.glyph(mask) };
    Cell { ch, style, ..Cell::BLANK }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, px: [u8; 4]) -> Bitmap {
        let mut b = Bitmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                b.set(x, y, px);
            }
        }
        b
    }

    /// A 2x2 bitmap whose pixels are `[tl, tr, bl, br]`.
    fn quad(px: [[u8; 4]; 4]) -> Bitmap {
        let mut b = Bitmap::new(2, 2);
        for (i, p) in px.into_iter().enumerate() {
            b.set(i % 2, i / 2, p);
        }
        b
    }

    const W: [u8; 4] = [255, 255, 255, 255];
    const K: [u8; 4] = [0, 0, 0, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    #[test]
    fn each_of_the_sixteen_patterns_picks_its_glyph() {
        for (mask, &glyph) in GLYPHS.iter().enumerate() {
            let px: [[u8; 4]; 4] = std::array::from_fn(|i| if mask & (1 << i) != 0 { W } else { K });
            let cell = tile_cell(&quad(px), 1, 1, 0, 0);
            if mask == 0 || mask == 15 {
                assert_eq!(cell.ch, ' ', "uniform cells are plain background");
            } else {
                assert_eq!(cell.ch, glyph, "mask {mask:04b}");
                assert_eq!(cell.style.fg, Color::Rgb(255, 255, 255));
                assert_eq!(cell.style.bg, Color::Rgb(0, 0, 0));
            }
        }
    }

    #[test]
    fn sextant_and_octant_tables_agree_with_the_unicode_character_database() {
        for (mask, code) in [(1u8, 0x1FB00u32), (20, 0x1FB13), (22, 0x1FB14), (41, 0x1FB27), (43, 0x1FB28), (62, 0x1FB3B)] {
            assert_eq!(sextant(mask), char::from_u32(code).unwrap(), "sextant {mask}");
        }
        assert_eq!((sextant(0), sextant(21), sextant(42), sextant(63)), (' ', '▌', '▐', '█'));
        for (mask, code) in [(4u8, 0x1CD00u32), (6, 0x1CD01), (7, 0x1CD02), (100, 0x1CD55), (200, 0x1CDB3), (253, 0x1CDE4), (254, 0x1CDE5)] {
            assert_eq!(octant(mask), char::from_u32(code).unwrap(), "octant {mask}");
        }
        let all: std::collections::HashSet<char> = (0..=255u8).map(octant).collect();
        assert_eq!(all.len(), 256, "every pattern has its own character");
        let block = (0..=255u8).filter(|&m| ('\u{1CD00}'..='\u{1CDE5}').contains(&octant(m))).count();
        assert_eq!(block, 230, "230 patterns are in the octant block");
        let sextants: std::collections::HashSet<char> = (0..64u8).map(sextant).collect();
        assert_eq!(sextants.len(), 64);
    }

    #[test]
    fn finer_glyph_sets_sample_more_rows_of_the_cell() {
        // top half white, bottom half black, over a cell: the upper half block with two and four
        // rows; three rows cannot split the middle one, which is gray and counts as dark
        let mut b = Bitmap::new(2, 12);
        for y in 0..12 {
            for x in 0..2 {
                b.set(x, y, if y < 6 { W } else { K });
            }
        }
        for g in [Glyphs::Quadrant, Glyphs::Octant] {
            let cell = tile_cell_with(&b, 1, 1, 0, 0, g);
            assert_eq!(cell.ch, '▀', "{g:?}");
        }
        assert_eq!(tile_cell_with(&b, 1, 1, 0, 0, Glyphs::Sextant).ch, sextant(0b00_0011), "the top row only");
        // a white stripe across the second of four rows: only octants can show it alone
        let mut stripe = Bitmap::new(2, 8);
        for y in 0..8 {
            for x in 0..2 {
                stripe.set(x, y, if (2..4).contains(&y) { W } else { K });
            }
        }
        let cell = tile_cell_with(&stripe, 1, 1, 0, 0, Glyphs::Octant);
        assert_eq!(cell.ch, octant(0b0000_1100), "cells 3 and 4");
        assert_eq!(cell.style.fg, Color::Rgb(255, 255, 255));
        assert_eq!(cell.style.bg, Color::Rgb(0, 0, 0));
        assert_ne!(tile_cell_with(&stripe, 1, 1, 0, 0, Glyphs::Quadrant).ch, cell.ch);
    }

    #[test]
    fn glyph_names_parse_and_anything_else_is_quadrants() {
        assert_eq!(Glyphs::parse("octant"), Glyphs::Octant);
        assert_eq!(Glyphs::parse("sextant"), Glyphs::Sextant);
        assert_eq!(Glyphs::parse("quadrant"), Glyphs::Quadrant);
        assert_eq!(Glyphs::parse("x"), Glyphs::Quadrant);
    }

    #[test]
    fn the_glyph_table_matches_the_unicode_code_points() {
        let mut seen: Vec<char> = GLYPHS.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 16, "every pattern has its own glyph");
        assert_eq!(GLYPHS[1] as u32, 0x2598);
        assert_eq!(GLYPHS[2] as u32, 0x259d);
        assert_eq!(GLYPHS[4] as u32, 0x2596);
        assert_eq!(GLYPHS[8] as u32, 0x2597);
        assert_eq!(GLYPHS[3] as u32, 0x2580);
        assert_eq!(GLYPHS[12] as u32, 0x2584);
        assert_eq!(GLYPHS[5] as u32, 0x258c);
        assert_eq!(GLYPHS[10] as u32, 0x2590);
        assert_eq!(GLYPHS[6] as u32, 0x259e);
        assert_eq!(GLYPHS[9] as u32, 0x259a);
        assert_eq!(GLYPHS[7] as u32, 0x259b);
        assert_eq!(GLYPHS[11] as u32, 0x259c);
        assert_eq!(GLYPHS[13] as u32, 0x2599);
        assert_eq!(GLYPHS[14] as u32, 0x259f);
        assert_eq!(GLYPHS[15] as u32, 0x2588);
    }

    #[test]
    fn a_solid_color_becomes_a_background_only_cell() {
        let cell = tile_cell(&solid(8, 8, [10, 20, 30, 255]), 1, 1, 0, 0);
        assert_eq!(cell.ch, ' ');
        assert_eq!(cell.style.bg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn foreground_and_background_are_the_means_of_each_side() {
        let g1 = [200, 0, 0, 255];
        let g2 = [120, 0, 0, 255];
        let cell = tile_cell(&quad([g1, g2, K, K]), 1, 1, 0, 0);
        assert_eq!(cell.ch, '▀');
        assert_eq!(cell.style.fg, Color::Rgb(160, 0, 0));
        assert_eq!(cell.style.bg, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn tiles_sample_their_own_part_of_the_stretched_bitmap() {
        let mut b = Bitmap::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                b.set(x, y, if x < 2 { W } else { K });
            }
        }
        assert_eq!(tile_cell(&b, 2, 1, 0, 0).style.bg, Color::Rgb(255, 255, 255));
        assert_eq!(tile_cell(&b, 2, 1, 1, 0).style.bg, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn transparent_pixels_stay_see_through() {
        let cell = tile_cell(&solid(4, 4, CLEAR), 1, 1, 0, 0);
        assert_eq!(cell, Cell::BLANK);
        let half = tile_cell(&quad([W, CLEAR, W, CLEAR]), 1, 1, 0, 0);
        assert_eq!(half.ch, '▌');
        assert_eq!(half.style.fg, Color::Rgb(255, 255, 255));
        assert_eq!(half.style.bg, Color::Default);
    }

    #[test]
    fn a_tiny_bitmap_over_many_cells_does_not_panic() {
        let b = solid(1, 1, W);
        for ty in 0..5 {
            for tx in 0..5 {
                let _ = tile_cell(&b, 5, 5, tx, ty);
            }
        }
    }
}

#[cfg(test)]
mod dither_tests {
    use super::*;

    fn flat(w: usize, h: usize, px: [u8; 4]) -> Bitmap {
        let mut b = Bitmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                b.set(x, y, px);
            }
        }
        b
    }

    const BLUE: [u8; 4] = [60, 120, 200, 255];

    #[test]
    fn without_the_dither_the_cell_is_what_it_always_was() {
        let b = flat(16, 16, BLUE);
        for (tx, ty) in [(0, 0), (3, 5), (7, 1)] {
            assert_eq!(tile_cell_dithered(&b, 8, 8, tx, ty, Glyphs::Quadrant, false), tile_cell_with(&b, 8, 8, tx, ty, Glyphs::Quadrant));
        }
    }

    #[test]
    fn a_flat_picture_gets_a_pattern_that_follows_the_tile_and_repeats_every_eight() {
        let b = flat(64, 64, BLUE);
        let at = |tx, ty| tile_cell_dithered(&b, 16, 16, tx, ty, Glyphs::Quadrant, true).style.bg;
        assert_ne!(at(0, 0), at(1, 0));
        assert_ne!(at(0, 0), at(0, 1));
        for (tx, ty) in [(0, 0), (2, 3), (7, 7)] {
            assert_eq!(at(tx, ty), at(tx + 8, ty), "{tx},{ty}");
            assert_eq!(at(tx, ty), at(tx, ty + 8), "{tx},{ty}");
        }
    }

    #[test]
    fn the_same_tile_is_the_same_cell_whatever_else_is_drawn() {
        let b = flat(64, 64, BLUE);
        let first = tile_cell_dithered(&b, 16, 16, 5, 6, Glyphs::Quadrant, true);
        let _ = tile_cell_dithered(&b, 16, 16, 0, 0, Glyphs::Quadrant, true);
        assert_eq!(first, tile_cell_dithered(&b, 16, 16, 5, 6, Glyphs::Quadrant, true));
    }

    #[test]
    fn every_dithered_color_stays_in_the_color_range_and_near_the_picture() {
        let b = flat(64, 64, [250, 3, 128, 255]);
        for ty in 0..8 {
            for tx in 0..8 {
                let Color::Rgb(r, g, bl) = tile_cell_dithered(&b, 16, 16, tx, ty, Glyphs::Quadrant, true).style.bg else { panic!("not rgb") };
                assert!(r >= 230 && g <= 23 && (108..=148).contains(&bl), "{r},{g},{bl}");
            }
        }
    }

    #[test]
    fn a_transparent_tile_stays_blank() {
        let b = flat(16, 16, [0, 0, 0, 0]);
        assert_eq!(tile_cell_dithered(&b, 8, 8, 2, 2, Glyphs::Quadrant, true).ch, ' ');
        assert_eq!(tile_cell_dithered(&b, 8, 8, 2, 2, Glyphs::Quadrant, true).style, Style::default());
    }
}
