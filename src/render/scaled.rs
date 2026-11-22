//! A picture scaled once to the pixel size of its footprint on the client, then cut by cell.
//!
//! Scrolling over a big picture used to crop and resample the visible part of the source bitmap at
//! every step, so each step cost a resample and the edges of neighboring crops differed (seams).
//! Here the whole picture is scaled once per cell size; a crop is then only a copy of whole pixel
//! rows, exactly `cell` pixels per cell, and two crops of one picture always agree at their edge.
//! Kitty clients get the same scaled picture transmitted once and show parts of it by source
//! rectangle, so a scroll step sends no pixels at all.

use ::image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use ::image::imageops::{self, FilterType};
use ::image::{ExtendedColorType, ImageEncoder, RgbaImage};

use crate::image::Bitmap;

/// Most pixels of a scaled picture (24 Mpx, 96 MB of RGBA). A bigger footprint is drawn the old
/// way, one crop at a time.
pub const MAX_PIXELS: usize = 24 * 1024 * 1024;

/// Pixel size of the picture's footprint, `cols` x `rows` cells of `cell` pixels; `None` when it is
/// empty or too big to keep.
pub fn footprint(cols: usize, rows: usize, cell: (usize, usize)) -> Option<(usize, usize)> {
    let (w, h) = (cols.checked_mul(cell.0)?, rows.checked_mul(cell.1)?);
    (w > 0 && h > 0 && w.checked_mul(h)? <= MAX_PIXELS).then_some((w, h))
}

/// The picture at the size of its footprint (resampled once).
pub fn scale(b: &Bitmap, cols: usize, rows: usize, cell: (usize, usize)) -> Result<RgbaImage, String> {
    let (w, h) = footprint(cols, rows, cell).ok_or("picture footprint empty or too big to scale")?;
    let src = source(b)?;
    Ok(if (src.width() as usize, src.height() as usize) == (w, h) { src } else { imageops::resize(&src, w as u32, h as u32, FilterType::CatmullRom) })
}

fn source(b: &Bitmap) -> Result<RgbaImage, String> {
    if b.w == 0 || b.h == 0 || b.rgba.len() != b.w * b.h * 4 {
        return Err("bitmap size does not match its pixels".into());
    }
    RgbaImage::from_raw(b.w as u32, b.h as u32, b.rgba.clone()).ok_or_else(|| "bitmap size does not match its pixels".to_string())
}

/// What a Kitty terminal is sent for the whole picture and its size in pixels: the original when it
/// has no more pixels than the footprint (the terminal enlarges it), else the picture scaled to the
/// footprint, which is then shown one source pixel per screen pixel.
pub fn for_terminal(b: &Bitmap, cols: usize, rows: usize, cell: (usize, usize)) -> Result<(Vec<u8>, (usize, usize)), String> {
    let fit = footprint(cols, rows, cell);
    let img = match fit {
        Some((fw, fh)) if b.w > fw || b.h > fh => scale(b, cols, rows, cell)?,
        _ => source(b)?,
    };
    let size = (img.width() as usize, img.height() as usize);
    Ok((png(&img)?, size))
}

/// Whether `for_terminal` can handle this picture at all.
pub fn fits(b: &Bitmap, cols: usize, rows: usize, cell: (usize, usize)) -> bool {
    b.w > 0 && b.h > 0 && cols > 0 && rows > 0 && (footprint(cols, rows, cell).is_some() || b.w * b.h <= MAX_PIXELS)
}

/// Source rectangle `(x, y, w, h)` in pixels of cells `[tx, tx + w) x [ty, ty + h)` of a picture
/// that spans `cols` x `rows` cells and is `size` pixels big. Neighboring rectangles share their
/// edge exactly.
pub fn source_rect(size: (usize, usize), cols: usize, rows: usize, tile: (usize, usize), cells: (usize, usize)) -> (usize, usize, usize, usize) {
    let edge = |i: usize, extent: usize, n: usize| (i * extent / n).min(extent.saturating_sub(1));
    let (x0, y0) = (edge(tile.0, size.0, cols), edge(tile.1, size.1, rows));
    let x1 = ((tile.0 + cells.0) * size.0 / cols).clamp(x0 + 1, size.0);
    let y1 = ((tile.1 + cells.1) * size.1 / rows).clamp(y0 + 1, size.1);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Cells `[tx, tx + w) x [ty, ty + h)` of a scaled picture, exactly `w * cell.0` x `h * cell.1`
/// pixels; a part that lies outside the picture is left transparent.
pub fn crop(scaled: &RgbaImage, cols: usize, rows: usize, tile: (usize, usize), cells: (usize, usize)) -> Result<RgbaImage, String> {
    let (cw, ch) = (scaled.width() as usize / cols.max(1), scaled.height() as usize / rows.max(1));
    if cw == 0 || ch == 0 || cells.0 == 0 || cells.1 == 0 {
        return Err("empty image region".into());
    }
    let (pw, ph) = (cells.0 * cw, cells.1 * ch);
    let (x0, y0) = (tile.0 * cw, tile.1 * ch);
    let mut out = vec![0u8; pw * ph * 4];
    let have = (scaled.width() as usize).saturating_sub(x0).min(pw);
    let raw = scaled.as_raw();
    let stride = scaled.width() as usize * 4;
    for row in 0..ph {
        let sy = y0 + row;
        if sy >= scaled.height() as usize || have == 0 {
            break;
        }
        out[row * pw * 4..row * pw * 4 + have * 4].copy_from_slice(&raw[sy * stride + x0 * 4..sy * stride + (x0 + have) * 4]);
    }
    RgbaImage::from_raw(pw as u32, ph as u32, out).ok_or_else(|| "crop size does not match its pixels".to_string())
}

/// PNG with the fastest settings: these are encoded while a frame waits, and the terminal does not
/// care how well they are compressed.
pub fn png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilter::Sub)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Bitmap {
        let mut b = Bitmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                b.set(x, y, [(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 7, 255]);
            }
        }
        b
    }

    #[test]
    fn the_footprint_is_cells_times_cell_pixels_and_has_a_ceiling() {
        assert_eq!(footprint(4, 3, (10, 20)), Some((40, 60)));
        assert_eq!(footprint(0, 3, (10, 20)), None);
        assert_eq!(footprint(4, 3, (0, 20)), None);
        assert_eq!(footprint(10_000, 10_000, (10, 20)), None);
    }

    #[test]
    fn two_crops_of_one_picture_agree_at_their_edge() {
        let b = gradient(300, 200);
        let s = scale(&b, 6, 4, (10, 20)).unwrap();
        assert_eq!((s.width(), s.height()), (60, 80));
        let top = crop(&s, 6, 4, (0, 0), (6, 2)).unwrap();
        let bottom = crop(&s, 6, 4, (0, 2), (6, 2)).unwrap();
        let whole = crop(&s, 6, 4, (0, 0), (6, 4)).unwrap();
        assert_eq!((top.width(), top.height()), (60, 40));
        assert_eq!(top.as_raw()[..], whole.as_raw()[..60 * 40 * 4]);
        assert_eq!(bottom.as_raw()[..], whole.as_raw()[60 * 40 * 4..]);
    }

    #[test]
    fn a_crop_is_exactly_its_cells_wide_and_tall_and_shows_its_own_part() {
        let b = gradient(80, 40);
        let s = scale(&b, 8, 2, (10, 20)).unwrap();
        let right = crop(&s, 8, 2, (6, 0), (2, 2)).unwrap();
        assert_eq!((right.width(), right.height()), (20, 40));
        assert_eq!(right.get_pixel(0, 0).0, s.get_pixel(60, 0).0);
        assert_eq!(right.get_pixel(19, 39).0, s.get_pixel(79, 39).0);
    }

    #[test]
    fn a_crop_past_the_picture_is_transparent_not_a_panic() {
        let s = scale(&gradient(20, 20), 2, 2, (10, 10)).unwrap();
        let c = crop(&s, 2, 2, (1, 1), (3, 3)).unwrap();
        assert_eq!((c.width(), c.height()), (30, 30));
        assert_eq!(c.get_pixel(29, 29).0, [0, 0, 0, 0]);
        assert_eq!(c.get_pixel(0, 0).0, s.get_pixel(10, 10).0);
        assert!(crop(&s, 2, 2, (0, 0), (0, 1)).is_err());
    }

    #[test]
    fn a_big_picture_goes_to_the_terminal_at_footprint_size_and_a_small_one_as_it_is() {
        let (png_big, size) = for_terminal(&gradient(400, 300), 10, 5, (8, 16)).unwrap();
        assert_eq!(size, (80, 80));
        let img = crate::image::decode_image(&png_big).unwrap();
        assert_eq!((img.w, img.h), (80, 80));
        let (_, size) = for_terminal(&gradient(30, 20), 10, 5, (8, 16)).unwrap();
        assert_eq!(size, (30, 20));
    }

    #[test]
    fn source_rectangles_tile_the_picture_without_gaps_or_overlap() {
        let size = (1003, 777);
        let (cols, rows) = (7, 5);
        let mut covered = 0;
        for ty in 0..rows {
            let (x, y, w, h) = source_rect(size, cols, rows, (0, ty), (cols, 1));
            assert_eq!((x, w), (0, size.0));
            covered += h;
            if ty + 1 < rows {
                let (_, ny, _, _) = source_rect(size, cols, rows, (0, ty + 1), (cols, 1));
                assert_eq!(y + h, ny);
            }
        }
        assert_eq!(covered, size.1);
        let (x, _, w, _) = source_rect(size, cols, rows, (3, 0), (4, 1));
        assert_eq!(x + w, size.0);
    }

    #[test]
    fn fits_refuses_what_cannot_be_kept() {
        assert!(fits(&gradient(10, 10), 4, 4, (8, 16)));
        assert!(!fits(&gradient(10, 10), 0, 4, (8, 16)));
        assert!(fits(&gradient(10, 10), 20_000, 20_000, (8, 16)), "a footprint too big to scale to: the original goes as it is");
    }
}
