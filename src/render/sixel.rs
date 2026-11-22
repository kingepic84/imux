//! Drawing image regions on a Sixel client. Sixel is stamped into the cells like text, so it
//! behaves like the iTerm2 path: drawing over a cell replaces what was there.

use std::collections::HashMap;

use ::image::RgbaImage;

use crate::image::Bitmap;

use super::iterm::region_pixels;

/// Color registers used; every Sixel terminal has at least this many.
const MAX_COLORS: usize = 256;

/// The colors of one picture. With 256 or fewer distinct colors each gets its own register and
/// is exact; otherwise the picture is reduced to 256 by median cut over a 5-bits-per-channel
/// histogram (32768 bins), which keeps the cost independent of the picture size.
struct Palette {
    colors: Vec<[u8; 3]>,
    lookup: Lookup,
}

enum Lookup {
    Exact(HashMap<[u8; 3], u8>),
    /// Register of each histogram bin.
    Binned(Vec<u8>),
}

fn bin_of(p: [u8; 3]) -> usize {
    (usize::from(p[0] >> 3) << 10) | (usize::from(p[1] >> 3) << 5) | usize::from(p[2] >> 3)
}

/// A histogram bin that has pixels in it: its key, how many, and the sum of their colors.
struct Bin {
    key: usize,
    n: u64,
    sum: [u64; 3],
}

impl Bin {
    fn channel(&self, c: usize) -> usize {
        (self.key >> (10 - 5 * c)) & 31
    }
}

impl Palette {
    fn of(img: &RgbaImage) -> Palette {
        let mut exact: HashMap<[u8; 3], u8> = HashMap::new();
        for p in img.pixels().map(|p| p.0).filter(|p| p[3] >= 128) {
            let c = [p[0], p[1], p[2]];
            if exact.contains_key(&c) {
                continue;
            }
            if exact.len() >= MAX_COLORS {
                return Palette::median_cut(img);
            }
            exact.insert(c, exact.len() as u8);
        }
        let mut colors = vec![[0; 3]; exact.len()];
        for (c, &i) in &exact {
            colors[usize::from(i)] = *c;
        }
        Palette { colors, lookup: Lookup::Exact(exact) }
    }

    fn median_cut(img: &RgbaImage) -> Palette {
        let mut table = vec![(0u64, [0u64; 3]); 1 << 15];
        for p in img.pixels().map(|p| p.0).filter(|p| p[3] >= 128) {
            let slot = &mut table[bin_of([p[0], p[1], p[2]])];
            slot.0 += 1;
            for (sum, &v) in slot.1.iter_mut().zip(&p) {
                *sum += u64::from(v);
            }
        }
        let bins: Vec<Bin> = table.iter().enumerate().filter(|(_, t)| t.0 > 0).map(|(key, t)| Bin { key, n: t.0, sum: t.1 }).collect();

        let mut boxes = vec![bins];
        while boxes.len() < MAX_COLORS {
            let weight = |b: &Vec<Bin>| b.iter().map(|x| x.n).sum::<u64>();
            let Some(at) = (0..boxes.len()).filter(|&i| boxes[i].len() > 1).max_by_key(|&i| weight(&boxes[i])) else { break };
            let mut b = boxes.swap_remove(at);
            let spread = |c: usize| {
                let (lo, hi) = b.iter().fold((31, 0), |(lo, hi), x| (lo.min(x.channel(c)), hi.max(x.channel(c))));
                hi - lo
            };
            let widest = (0..3).max_by_key(|&c| spread(c)).unwrap_or(0);
            b.sort_by_key(|x| x.channel(widest));
            let (total, mut seen) = (weight(&b), 0);
            let mut cut = b.len() - 1;
            for (i, x) in b.iter().enumerate() {
                seen += x.n;
                if seen * 2 >= total {
                    cut = (i + 1).clamp(1, b.len() - 1);
                    break;
                }
            }
            let upper = b.split_off(cut);
            boxes.push(b);
            boxes.push(upper);
        }

        let mut colors = Vec::with_capacity(boxes.len());
        let mut lookup = vec![0u8; 1 << 15];
        for (i, b) in boxes.iter().enumerate() {
            let n: u64 = b.iter().map(|x| x.n).sum();
            let mean = |c: usize| (b.iter().map(|x| x.sum[c]).sum::<u64>() + n / 2) / n.max(1);
            colors.push([mean(0) as u8, mean(1) as u8, mean(2) as u8]);
            for x in b {
                lookup[x.key] = i as u8;
            }
        }
        Palette { colors, lookup: Lookup::Binned(lookup) }
    }

    fn index(&self, p: [u8; 3]) -> usize {
        match &self.lookup {
            Lookup::Exact(m) => m.get(&p).copied().map_or(0, usize::from),
            Lookup::Binned(t) => usize::from(t[bin_of(p)]),
        }
    }
}

/// Sixel color levels run 0 to 100 percent.
fn percent(v: u8) -> usize {
    (usize::from(v) * 100 + 127) / 255
}

/// Sixel data for an image, DCS to ST. Pixels with less than half opacity stay unset so what is
/// under them shows.
pub fn encode(img: &RgbaImage) -> String {
    encode_at(img, (0, 0), false)
}

/// `encode` for the part of a picture whose top-left pixel is `origin` in the picture, with an
/// ordered dither when `dither` is set. The dither applies when the picture has more colors than
/// registers (fewer are exact already), and its pattern follows the position in the picture, so a
/// part cut elsewhere, or the same part after a scroll, agrees with its neighbors.
pub fn encode_at(img: &RgbaImage, origin: (usize, usize), dither: bool) -> String {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let palette = Palette::of(img);
    let dither = dither && matches!(palette.lookup, Lookup::Binned(_));
    let index = |x: usize, y: usize| -> Option<usize> {
        let p = img.get_pixel(x as u32, y as u32).0;
        let color = [p[0], p[1], p[2]];
        let color = if dither { super::dither::nudge(color, origin.0 + x, origin.1 + y, super::dither::SIXEL_SPAN) } else { color };
        (p[3] >= 128).then(|| palette.index(color))
    };

    let mut out = format!("\x1bP0;1;0q\"1;1;{w};{h}");
    for (i, c) in palette.colors.iter().enumerate() {
        out.push_str(&format!("#{i};2;{};{};{}", percent(c[0]), percent(c[1]), percent(c[2])));
    }

    let mut rows: Vec<Vec<u8>> = vec![vec![0; w]; palette.colors.len()];
    let mut seen = vec![false; palette.colors.len()];
    for band in (0..h).step_by(6) {
        let mut colors: Vec<usize> = Vec::new();
        #[allow(clippy::needless_range_loop)] // x is a column in the image as well as in the bit rows
        for x in 0..w {
            for dy in 0..6.min(h - band) {
                if let Some(i) = index(x, band + dy) {
                    if !seen[i] {
                        seen[i] = true;
                        colors.push(i);
                    }
                    rows[i][x] |= 1 << dy;
                }
            }
        }
        for (n, &c) in colors.iter().enumerate() {
            out.push_str(&format!("#{c}"));
            push_run_length(&mut out, &rows[c]);
            out.push(if n + 1 < colors.len() { '$' } else { '-' });
        }
        if colors.is_empty() {
            out.push('-');
        }
        for &c in &colors {
            rows[c].fill(0);
            seen[c] = false;
        }
    }
    out.push_str("\x1b\\");
    out
}

fn push_run_length(out: &mut String, bits: &[u8]) {
    let mut i = 0;
    while i < bits.len() {
        let mut n = 1;
        while i + n < bits.len() && bits[i + n] == bits[i] {
            n += 1;
        }
        let ch = (bits[i] + 63) as char;
        if n > 3 {
            out.push_str(&format!("!{n}{ch}"));
        } else {
            out.extend(std::iter::repeat_n(ch, n));
        }
        i += n;
    }
}

/// Cells `[tx, tx + w) x [ty, ty + h)` of an image spanning `cols` x `rows` cells, as Sixel
/// exactly `w * cell.0` x `h * cell.1` pixels big.
pub fn region_dcs(b: &Bitmap, cols: usize, rows: usize, tile: (usize, usize), size: (usize, usize), cell: (usize, usize)) -> Result<String, String> {
    Ok(encode(&region_pixels(b, cols, rows, tile, size, cell)?))
}

/// The same as `region_dcs`, cut out of a picture `scaled::scale` already made at the size of its
/// footprint: no resampling, and neighboring regions agree at their edge.
pub fn region_dcs_scaled(scaled: &RgbaImage, cols: usize, rows: usize, tile: (usize, usize), size: (usize, usize)) -> Result<String, String> {
    Ok(encode(&super::scaled::crop(scaled, cols, rows, tile, size)?))
}

/// `region_dcs` with the dither (`image-dither`) on or off; the pattern is indexed by the pixel's
/// place in the picture at its footprint size.
pub fn region_dcs_dithered(
    b: &Bitmap,
    cols: usize,
    rows: usize,
    tile: (usize, usize),
    size: (usize, usize),
    cell: (usize, usize),
    dither: bool,
) -> Result<String, String> {
    Ok(encode_at(&region_pixels(b, cols, rows, tile, size, cell)?, (tile.0 * cell.0, tile.1 * cell.1), dither))
}

/// `region_dcs_scaled` with the dither on or off.
pub fn region_dcs_scaled_dithered(
    scaled: &RgbaImage,
    cols: usize,
    rows: usize,
    tile: (usize, usize),
    size: (usize, usize),
    dither: bool,
) -> Result<String, String> {
    let origin = (tile.0 * (scaled.width() as usize / cols.max(1)), tile.1 * (scaled.height() as usize / rows.max(1)));
    Ok(encode_at(&super::scaled::crop(scaled, cols, rows, tile, size)?, origin, dither))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::sixel;

    fn decode(s: &str) -> Bitmap {
        let body = s.strip_prefix("\x1bP0;1;0q").and_then(|s| s.strip_suffix("\x1b\\")).expect("dcs framing");
        sixel::decode(body.as_bytes()).expect("our decoder reads what we write")
    }

    fn picture(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| ::image::Rgba(f(x, y)))
    }

    #[test]
    fn colors_survive_a_round_trip_to_the_nearest_palette_entry() {
        let img = picture(10, 13, |x, _| if x < 5 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        let back = decode(&encode(&img));
        assert_eq!((back.w, back.h), (10, 13));
        assert_eq!(back.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(back.pixel(9, 12), [0, 0, 255, 255]);
        assert_eq!(back.pixel(4, 6), [255, 0, 0, 255]);
        assert_eq!(back.pixel(5, 7), [0, 0, 255, 255]);
    }

    #[test]
    fn transparent_pixels_stay_unset() {
        let img = picture(8, 6, |x, _| if x < 4 { [0, 255, 0, 255] } else { [0, 0, 0, 0] });
        let back = decode(&encode(&img));
        assert_eq!(back.pixel(1, 1), [0, 255, 0, 255]);
        assert_eq!(back.pixel(6, 1)[3], 0);
    }

    #[test]
    fn a_blank_picture_still_has_its_size() {
        let back = decode(&encode(&picture(7, 12, |_, _| [0, 0, 0, 0])));
        assert_eq!((back.w, back.h), (7, 12));
    }

    #[test]
    fn long_runs_are_compressed() {
        let s = encode(&picture(200, 6, |_, _| [255, 255, 255, 255]));
        assert!(s.contains("!200"), "{s}");
        assert!(s.len() < 100);
    }

    #[test]
    fn a_region_is_the_exact_pixel_size_of_its_cells() {
        let mut b = Bitmap::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                b.set(x, y, [255, 255, 0, 255]);
            }
        }
        let back = decode(&region_dcs(&b, 2, 2, (0, 0), (2, 2), (5, 10)).unwrap());
        assert_eq!((back.w, back.h), (10, 20));
        assert_eq!(back.pixel(9, 19), [255, 255, 0, 255]);
    }

    fn registers(sixel: &str) -> usize {
        sixel.split('#').skip(1).filter(|p| p.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with(";2;")).count()
    }

    fn off_by(a: [u8; 4], b: [u8; 4]) -> u8 {
        (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0)
    }

    #[test]
    fn a_picture_with_few_colors_keeps_them_all_even_off_the_old_cube() {
        let colors = [[10, 20, 30], [201, 77, 140], [250, 250, 3], [64, 64, 64]];
        let img = picture(8, 6, |x, _| {
            let c = colors[(x / 2) as usize];
            [c[0], c[1], c[2], 255]
        });
        let s = encode(&img);
        assert_eq!(registers(&s), 4);
        let back = decode(&s);
        for (i, c) in colors.iter().enumerate() {
            assert!(off_by(back.pixel(i * 2, 3), [c[0], c[1], c[2], 255]) <= 2, "color {i}: {:?}", back.pixel(i * 2, 3));
        }
    }

    #[test]
    fn a_picture_with_many_colors_is_reduced_to_256_registers_and_stays_close() {
        let img = picture(300, 12, |x, y| {
            let r = (x * 255 / 299) as u8;
            [r, 255 - r, (y * 20) as u8, 255]
        });
        let s = encode(&img);
        assert!(registers(&s) <= 256 && registers(&s) > 100, "{} registers", registers(&s));
        let back = decode(&s);
        for (x, y) in [(0, 0), (77, 5), (150, 11), (299, 3)] {
            let want = img.get_pixel(x, y).0;
            assert!(off_by(back.pixel(x as usize, y as usize), want) <= 12, "({x},{y}) {:?} vs {want:?}", back.pixel(x as usize, y as usize));
        }
    }

    #[test]
    fn noise_still_encodes_into_at_most_256_registers() {
        let img = picture(64, 64, |x, y| {
            let n = x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503);
            [n as u8, (n >> 8) as u8, (n >> 16) as u8, 255]
        });
        let s = encode(&img);
        assert!(registers(&s) <= 256);
        let back = decode(&s);
        assert_eq!((back.w, back.h), (64, 64));
    }
}

#[cfg(test)]
mod dither_tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| ::image::Rgba([(x * 255 / w.max(1)) as u8, ((x / 2 + y * 7) % 256) as u8, (255 - x * 255 / w.max(1)) as u8, 255]))
    }

    fn registers(sixel: &str) -> usize {
        sixel.split('#').skip(1).filter(|p| p.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with(";2;")).count()
    }

    #[test]
    fn off_is_the_encoder_it_always_was() {
        let img = gradient(64, 32);
        assert_eq!(encode_at(&img, (0, 0), false), encode(&img));
        assert_eq!(encode_at(&img, (13, 29), false), encode(&img), "the origin matters only with the dither on");
    }

    #[test]
    fn a_picture_that_needs_more_than_256_colors_is_dithered_and_still_fits_the_registers() {
        let img = gradient(128, 32);
        let plain = encode(&img);
        let dithered = encode_at(&img, (0, 0), true);
        assert!(registers(&plain) > 200, "the picture should need the median cut");
        assert_ne!(plain, dithered);
        assert!(registers(&dithered) <= 256);
        assert_eq!(dithered, encode_at(&img, (0, 0), true), "the same input gives the same bytes");
    }

    #[test]
    fn a_picture_with_few_colors_is_exact_with_or_without_the_dither() {
        let img = RgbaImage::from_fn(32, 12, |x, _| if x < 16 { ::image::Rgba([200, 30, 30, 255]) } else { ::image::Rgba([30, 30, 200, 255]) });
        assert_eq!(encode_at(&img, (5, 9), true), encode(&img));
    }

    #[test]
    fn the_pattern_follows_the_position_in_the_picture_and_repeats_every_eight_pixels() {
        let img = gradient(128, 32);
        let here = encode_at(&img, (0, 0), true);
        assert_eq!(here, encode_at(&img, (8, 16), true), "a shift of whole periods changes nothing");
        assert_eq!(here, encode_at(&img, (64, 8), true));
        assert_ne!(here, encode_at(&img, (1, 0), true), "another phase is another pattern");
    }

    #[test]
    fn transparent_pixels_stay_unset_with_the_dither_on() {
        let img = RgbaImage::from_fn(16, 12, |x, y| if x < 8 { ::image::Rgba([0, 0, 0, 0]) } else { ::image::Rgba([x as u8 * 9, y as u8 * 20, 7, 255]) });
        let dithered = encode_at(&img, (0, 0), true);
        assert!(dithered.starts_with("\x1bP0;1;0q\"1;1;16;12") && dithered.ends_with("\x1b\\"));
        assert_eq!(dithered, encode(&img), "few colors: exact, nothing to dither");
    }

    #[test]
    fn the_dithered_region_calls_cut_the_same_pixels_the_plain_ones_do() {
        let mut b = Bitmap::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                b.set(x, y, [(x * 4) as u8, ((x / 2 + y * 7) % 256) as u8, (255 - x * 4) as u8, 255]);
            }
        }
        let plain = region_dcs(&b, 4, 2, (1, 0), (2, 2), (8, 16)).unwrap();
        assert_eq!(region_dcs_dithered(&b, 4, 2, (1, 0), (2, 2), (8, 16), false).unwrap(), plain);
        let on = region_dcs_dithered(&b, 4, 2, (1, 0), (2, 2), (8, 16), true).unwrap();
        assert!(on.starts_with("\x1bP0;1;0q\"1;1;16;32"), "the size of the region is unchanged");
        let scaled = RgbaImage::from_fn(64, 32, |x, y| ::image::Rgba([(x * 4) as u8, ((x / 2 + y * 7) % 256) as u8, (255 - x * 4) as u8, 255]));
        assert_eq!(region_dcs_scaled_dithered(&scaled, 4, 2, (1, 0), (2, 2), false).unwrap(), region_dcs_scaled(&scaled, 4, 2, (1, 0), (2, 2)).unwrap());
    }
}
