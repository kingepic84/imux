//! Drawing image regions on an iTerm2 client: crop the part of the bitmap a rectangle of cells
//! shows, resample it to the exact pixel size of those cells, and wrap it as an inline image.

use ::image::imageops::{self, FilterType};
use ::image::{ImageFormat, RgbaImage};

use super::scaled;
use crate::image::{Bitmap, base64};

/// `OSC 1337 ; File=...` showing cells `[tx, tx + w) x [ty, ty + h)` of an image that spans
/// `cols` x `rows` cells, in a rectangle of `w` x `h` cells of `cell` pixels each. The size is
/// given in cells and the picture already has the right pixel size, so the terminal neither
/// scales it nor adds margins for the aspect ratio.
pub fn region_osc(b: &Bitmap, cols: usize, rows: usize, tile: (usize, usize), size: (usize, usize), cell: (usize, usize)) -> Result<String, String> {
    let png = region_png(b, cols, rows, tile, size, cell)?;
    let (w, h) = size;
    Ok(format!("\x1b]1337;File=inline=1;width={w};height={h};preserveAspectRatio=0:{}\x07", base64::encode(&png)))
}

/// The same inline image as `region_osc`, cut out of a picture that `scaled::scale` already made at
/// the size of its footprint: no resampling, and two neighboring regions agree at their edge.
pub fn region_osc_scaled(scaled: &RgbaImage, cols: usize, rows: usize, tile: (usize, usize), size: (usize, usize)) -> Result<String, String> {
    let png = scaled::png(&scaled::crop(scaled, cols, rows, tile, size)?)?;
    let (w, h) = size;
    Ok(format!("\x1b]1337;File=inline=1;width={w};height={h};preserveAspectRatio=0:{}\x07", base64::encode(&png)))
}

/// The pixels of a rectangle of cells, `(tx, ty)` and `(w, h)` in cells, as a PNG exactly
/// `w * cell.0` x `h * cell.1` pixels big.
pub fn region_png(b: &Bitmap, cols: usize, rows: usize, tile: (usize, usize), size: (usize, usize), cell: (usize, usize)) -> Result<Vec<u8>, String> {
    let sized = region_pixels(b, cols, rows, tile, size, cell)?;
    let mut png = Vec::new();
    sized.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png).map_err(|e| format!("png encode: {e}"))?;
    Ok(png)
}

/// Same as `region_png` before encoding.
pub fn region_pixels(b: &Bitmap, cols: usize, rows: usize, (tx, ty): (usize, usize), (w, h): (usize, usize), cell: (usize, usize)) -> Result<RgbaImage, String> {
    if b.w == 0 || b.h == 0 || cols == 0 || rows == 0 || w == 0 || h == 0 || cell.0 == 0 || cell.1 == 0 {
        return Err("empty image region".into());
    }
    let edge = |i: usize, extent: usize, cells: usize| (i * extent / cells).min(extent - 1);
    let (x0, y0) = (edge(tx, b.w, cols), edge(ty, b.h, rows));
    let x1 = ((tx + w) * b.w / cols).clamp(x0 + 1, b.w);
    let y1 = ((ty + h) * b.h / rows).clamp(y0 + 1, b.h);

    // Only the rows of the crop are copied: cloning the whole picture (tens of MB) for every crop
    // that is encoded made scrolling past a large one slow.
    if b.rgba.len() != b.w * b.h * 4 {
        return Err("bitmap size does not match its pixels".into());
    }
    let mut pixels = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for row in y0..y1 {
        pixels.extend_from_slice(&b.rgba[(row * b.w + x0) * 4..(row * b.w + x1) * 4]);
    }
    let crop = RgbaImage::from_raw((x1 - x0) as u32, (y1 - y0) as u32, pixels).ok_or("crop size does not match its pixels")?;
    let (pw, ph) = ((w * cell.0) as u32, (h * cell.1) as u32);
    Ok(imageops::resize(&crop, pw, ph, FilterType::Triangle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::iterm::{self, Dim};

    fn halves() -> Bitmap {
        let mut b = Bitmap::new(8, 4);
        for y in 0..4 {
            for x in 0..8 {
                b.set(x, y, if x < 4 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
            }
        }
        b
    }

    fn decode(osc: &str) -> (iterm::File, Bitmap) {
        let body = osc.strip_prefix("\x1b]1337;File=").and_then(|s| s.strip_suffix('\x07')).expect("osc framing");
        let file = iterm::parse(body.as_bytes()).expect("parses as an iTerm2 file");
        let bitmap = crate::image::decode_image(&file.data).expect("png decodes");
        (file, bitmap)
    }

    /// Where the time of one region goes. Not a check: `cargo test --release stages -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stages_of_one_large_region() {
        use std::io::Write;
        use std::time::Instant;
        let mut out = std::io::stdout();
        let (w, h) = (4000usize, 3000usize);
        let mut b = Bitmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let n = (x * 7 + y * 3) as u32;
                b.set(x, y, [(n >> 2) as u8, (x / 16) as u8 ^ (y / 9) as u8, (n >> 4) as u8, 255]);
            }
        }
        let (cols, rows, cell, size) = (200usize, 100usize, (14usize, 28usize), (120usize, 40usize));
        let t = Instant::now();
        let full = RgbaImage::from_raw(w as u32, h as u32, b.rgba.clone()).unwrap();
        let _ = writeln!(out, "clone of the whole bitmap ({} MB): {:?}", b.rgba.len() >> 20, t.elapsed());
        let t = Instant::now();
        let crop = imageops::crop_imm(&full, 0, 0, (w * size.0 / cols) as u32, (h * size.1 / rows) as u32).to_image();
        let _ = writeln!(out, "crop {}x{}: {:?}", crop.width(), crop.height(), t.elapsed());
        let t = Instant::now();
        let sized = imageops::resize(&crop, (size.0 * cell.0) as u32, (size.1 * cell.1) as u32, FilterType::Triangle);
        let _ = writeln!(out, "resize to {}x{}: {:?}", sized.width(), sized.height(), t.elapsed());
        let t = Instant::now();
        let mut png = Vec::new();
        sized.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let _ = writeln!(out, "png {} KB: {:?}", png.len() >> 10, t.elapsed());
        for (name, comp, filt) in [
            ("fast+sub", ::image::codecs::png::CompressionType::Fast, ::image::codecs::png::FilterType::Sub),
            ("fast+nofilter", ::image::codecs::png::CompressionType::Fast, ::image::codecs::png::FilterType::NoFilter),
            ("default+adaptive", ::image::codecs::png::CompressionType::Default, ::image::codecs::png::FilterType::Adaptive),
        ] {
            use ::image::ImageEncoder;
            let t = Instant::now();
            let mut encoded = Vec::new();
            ::image::codecs::png::PngEncoder::new_with_quality(&mut encoded, comp, filt)
                .write_image(sized.as_raw(), sized.width(), sized.height(), ::image::ExtendedColorType::Rgba8)
                .unwrap();
            let _ = writeln!(out, "png {name}: {} KB in {:?}", encoded.len() >> 10, t.elapsed());
        }
        let t = Instant::now();
        let osc = base64::encode(&png);
        let _ = writeln!(out, "base64 {} KB: {:?}", osc.len() >> 10, t.elapsed());
        let t = Instant::now();
        let _ = region_osc(&b, cols, rows, (0, 0), size, cell).unwrap();
        let _ = writeln!(out, "all of region_osc: {:?}", t.elapsed());
    }

    #[test]
    fn the_picture_has_the_exact_pixel_size_of_its_cells() {
        let osc = region_osc(&halves(), 4, 2, (0, 0), (4, 2), (10, 20)).unwrap();
        let (file, img) = decode(&osc);
        assert_eq!((img.w, img.h), (40, 40));
        assert_eq!((file.width, file.height, file.preserve_aspect), (Dim::Cells(4), Dim::Cells(2), false));
        assert!(file.inline);
    }

    #[test]
    fn a_region_cut_from_the_scaled_picture_has_the_cells_size_and_the_same_pixels_as_its_neighbor_expects() {
        let s = scaled::scale(&halves(), 4, 2, (10, 20)).unwrap();
        let osc = region_osc_scaled(&s, 4, 2, (2, 0), (2, 2)).unwrap();
        let (file, img) = decode(&osc);
        assert_eq!((img.w, img.h), (20, 40));
        assert_eq!((file.width, file.height), (Dim::Cells(2), Dim::Cells(2)));
        assert_eq!(img.pixel(10, 20), [0, 0, 255, 255]);
        let left = decode(&region_osc_scaled(&s, 4, 2, (0, 0), (2, 2)).unwrap()).1;
        assert_eq!(left.pixel(5, 20), [255, 0, 0, 255]);
        assert!(region_osc_scaled(&s, 4, 2, (0, 0), (0, 1)).is_err());
    }

    #[test]
    fn a_region_shows_only_its_own_part_of_the_image() {
        let right = region_osc(&halves(), 4, 1, (2, 0), (2, 1), (4, 4)).unwrap();
        let (_, img) = decode(&right);
        assert_eq!((img.w, img.h), (8, 4));
        assert_eq!(img.pixel(0, 0), [0, 0, 255, 255]);
        assert_eq!(img.pixel(7, 3), [0, 0, 255, 255]);
        let left = region_osc(&halves(), 4, 1, (0, 0), (2, 1), (4, 4)).unwrap();
        assert_eq!(decode(&left).1.pixel(7, 3), [255, 0, 0, 255]);
    }

    #[test]
    fn a_tiny_bitmap_over_many_cells_still_gives_every_cell_a_pixel() {
        let one = Bitmap { w: 1, h: 1, rgba: vec![9, 9, 9, 255] };
        for tx in 0..5 {
            let osc = region_osc(&one, 5, 5, (tx, 4), (1, 1), (3, 3)).unwrap();
            assert_eq!(decode(&osc).1.pixel(1, 1), [9, 9, 9, 255]);
        }
    }

    #[test]
    fn empty_regions_are_refused() {
        assert!(region_osc(&halves(), 4, 2, (0, 0), (0, 1), (10, 20)).is_err());
        assert!(region_osc(&halves(), 4, 2, (0, 0), (1, 1), (0, 20)).is_err());
        assert!(region_osc(&Bitmap::new(0, 0), 4, 2, (0, 0), (1, 1), (10, 20)).is_err());
    }
}
