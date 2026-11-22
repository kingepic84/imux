//! Sixel decoder: the data of a DCS `q` string (everything after the `q`) to a bitmap. Pixels
//! that nothing draws stay transparent, so the pane background shows through.

use super::{Bitmap, MAX_PIXELS};

/// Widest or tallest image accepted, in pixels.
const MAX_SIDE: usize = 16384;

/// The VT340 sixteen-color palette as percentages, used until the image defines its own.
const DEFAULT_PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0],
    [20, 20, 80],
    [80, 13, 13],
    [20, 80, 20],
    [80, 20, 80],
    [20, 80, 80],
    [80, 80, 20],
    [53, 53, 53],
    [26, 26, 26],
    [33, 33, 60],
    [60, 26, 26],
    [33, 60, 33],
    [60, 33, 60],
    [33, 60, 60],
    [60, 60, 33],
    [80, 80, 80],
];

fn percent(v: usize) -> u8 {
    (v.min(100) * 255 / 100) as u8
}

/// DEC hue 0 is blue, 120 red, 240 green; shift to the usual red-at-zero before converting.
fn hls(h: usize, l: usize, s: usize) -> [u8; 3] {
    let h = ((h % 360 + 240) % 360) as f32 / 60.0;
    let l = l.min(100) as f32 / 100.0;
    let s = s.min(100) as f32 / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as usize {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r, g, b].map(|v| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Numbers separated by `;` starting at `i`; returns them and the index of the next byte.
fn numbers(data: &[u8], mut i: usize) -> (Vec<usize>, usize) {
    let mut out = Vec::new();
    let mut cur: Option<usize> = None;
    while i < data.len() {
        match data[i] {
            d @ b'0'..=b'9' => cur = Some(cur.unwrap_or(0).saturating_mul(10).saturating_add(usize::from(d - b'0'))),
            b';' => out.push(cur.take().unwrap_or(0)),
            _ => break,
        }
        i += 1;
    }
    if let Some(v) = cur {
        out.push(v);
    }
    (out, i)
}

struct Canvas {
    rows: Vec<Vec<[u8; 4]>>,
    width: usize,
    /// Pixels painted so far, over and over included: a few bytes such as `!16384~$` repeated
    /// paint a whole row each time.
    work: usize,
}

/// Most pixels one sixel image may paint in total.
const MAX_WORK: usize = 8 * MAX_PIXELS;

impl Canvas {
    fn paint(&mut self, x: usize, row: usize, count: usize, px: [u8; 4]) -> Result<(), String> {
        let end = x.saturating_add(count);
        if end > MAX_SIDE || row >= MAX_SIDE {
            return Err("sixel image too large".to_string());
        }
        // The size is known before the canvas grows: a 21 KB image could otherwise allocate a
        // gigabyte and only then be refused (found by the security review).
        if self.width.max(end).saturating_mul(self.rows.len().max(row + 1)) > MAX_PIXELS {
            return Err("sixel image too large".to_string());
        }
        self.work = self.work.saturating_add(count);
        if self.work > MAX_WORK {
            return Err("sixel image needs too much work".to_string());
        }
        if self.rows.len() <= row {
            self.rows.resize(row + 1, Vec::new());
        }
        let r = &mut self.rows[row];
        if r.len() < end {
            r.resize(end, [0; 4]);
        }
        r[x..end].fill(px);
        self.width = self.width.max(end);
        Ok(())
    }
}

pub fn decode(data: &[u8]) -> Result<Bitmap, String> {
    let mut palette = [[0u8; 3]; 256];
    for (slot, p) in palette.iter_mut().zip(DEFAULT_PALETTE) {
        *slot = p.map(|v| percent(usize::from(v)));
    }
    let mut color = 0usize;
    let (mut x, mut band) = (0usize, 0usize);
    let mut canvas = Canvas { rows: Vec::new(), width: 0, work: 0 };
    let (mut declared_w, mut declared_h) = (0usize, 0usize);
    let mut repeat = 1usize;

    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            b'"' => {
                let (n, next) = numbers(data, i + 1);
                declared_w = n.get(2).copied().unwrap_or(0);
                declared_h = n.get(3).copied().unwrap_or(0);
                i = next;
            }
            b'#' => {
                let (n, next) = numbers(data, i + 1);
                if let Some(&pc) = n.first() {
                    let pc = pc.min(255);
                    if n.len() >= 5 {
                        let (a, b, c) = (n[2], n[3], n[4]);
                        palette[pc] = match n[1] {
                            1 => hls(a, b, c),
                            2 => [percent(a), percent(b), percent(c)],
                            _ => palette[pc],
                        };
                    }
                    color = pc;
                }
                i = next;
            }
            b'!' => {
                let (n, next) = numbers(data, i + 1);
                repeat = n.first().copied().unwrap_or(1).max(1);
                i = next;
            }
            b'$' => {
                x = 0;
                i += 1;
            }
            b'-' => {
                x = 0;
                band += 1;
                i += 1;
            }
            0x3f..=0x7e => {
                let bits = b - 0x3f;
                let [r, g, bl] = palette[color];
                for k in 0..6 {
                    if bits & (1 << k) != 0 {
                        canvas.paint(x, band * 6 + k, repeat, [r, g, bl, 255])?;
                    }
                }
                x = x.saturating_add(repeat);
                canvas.width = canvas.width.max(x.min(MAX_SIDE));
                repeat = 1;
                i += 1;
            }
            _ => i += 1,
        }
    }

    let w = canvas.width.max(declared_w.min(MAX_SIDE));
    let h = canvas.rows.len().max(declared_h).min(MAX_SIDE);
    if w == 0 || h == 0 {
        return Err("empty sixel image".to_string());
    }
    if w.saturating_mul(h) > MAX_PIXELS {
        return Err(format!("sixel image {w}x{h} refused"));
    }
    let mut bitmap = Bitmap::new(w, h);
    for (y, row) in canvas.rows.iter().enumerate().take(h) {
        for (x, &px) in row.iter().enumerate().take(w) {
            bitmap.set(x, y, px);
        }
    }
    Ok(bitmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_picture_is_refused_before_the_canvas_grows_not_after() {
        let mut data = b"#0;2;100;0;0".to_vec();
        data.extend_from_slice(b"#0!16384~-");
        for _ in 0..2730 {
            data.extend_from_slice(b"!16384~-");
        }
        let started = std::time::Instant::now();
        let err = decode(&data).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    #[test]
    fn painting_the_same_row_again_and_again_runs_out_of_work() {
        let mut data = b"#0;2;100;0;0#0".to_vec();
        for _ in 0..40_000 {
            data.extend_from_slice(b"!16384~$");
        }
        assert!(decode(&data).unwrap_err().contains("work"));
    }

    #[test]
    fn a_hue_that_is_a_huge_number_does_not_overflow() {
        let b = decode(b"#0;1;99999999999999999999;50;50#0~").unwrap();
        assert_eq!((b.w, b.h), (1, 6));
    }

    #[test]
    fn one_sixel_column_sets_the_bits_it_names() {
        let b = decode(b"#1;2;100;0;0#1~").unwrap();
        assert_eq!((b.w, b.h), (1, 6));
        for y in 0..6 {
            assert_eq!(b.pixel(0, y), [255, 0, 0, 255]);
        }
    }

    #[test]
    fn partial_bits_leave_other_rows_transparent() {
        let b = decode(b"#0;2;0;100;0#0A").unwrap();
        assert_eq!(b.pixel(0, 1), [0, 255, 0, 255]);
        assert_eq!(b.pixel(0, 0)[3], 0);
    }

    #[test]
    fn repeat_and_carriage_return_overlay_colors() {
        let b = decode(b"#1;2;100;0;0#2;2;0;0;100#1!4~$#2!2~").unwrap();
        assert_eq!(b.w, 4);
        assert_eq!(b.pixel(0, 0), [0, 0, 255, 255]);
        assert_eq!(b.pixel(1, 3), [0, 0, 255, 255]);
        assert_eq!(b.pixel(3, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn dash_starts_the_next_band() {
        let b = decode(b"#1;2;0;100;0#1~-~").unwrap();
        assert_eq!((b.w, b.h), (1, 12));
        assert_eq!(b.pixel(0, 11), [0, 255, 0, 255]);
    }

    #[test]
    fn raster_attributes_set_a_minimum_size() {
        let b = decode(b"\"1;1;8;12#1;2;100;100;100#1~").unwrap();
        assert_eq!((b.w, b.h), (8, 12));
        assert_eq!(b.pixel(7, 11)[3], 0);
    }

    #[test]
    fn hls_colors_use_the_dec_hue_origin() {
        assert_eq!(hls(0, 50, 100), [0, 0, 255]);
        assert_eq!(hls(120, 50, 100), [255, 0, 0]);
        assert_eq!(hls(240, 50, 100), [0, 255, 0]);
        assert_eq!(hls(0, 100, 0), [255, 255, 255]);
    }

    #[test]
    fn default_palette_is_used_before_any_definition() {
        let b = decode(b"#15~").unwrap();
        assert_eq!(b.pixel(0, 0), [204, 204, 204, 255]);
    }

    #[test]
    fn empty_and_oversized_images_are_errors() {
        assert!(decode(b"").is_err());
        assert!(decode(b"#1").is_err());
        assert!(decode(b"!99999999~").is_err());
    }

    #[test]
    fn junk_does_not_panic() {
        let _ = decode(b"\x00\xff;;;###!!!\"\"\"-$-$~~~");
        let _ = decode(b"#999;9;999;999;999~");
    }
}
