//! Ordered dither (`image-dither ordered`) for the two paths that cut a picture down to a small
//! palette: the 256-color block fallback and the Sixel registers. A Bayer threshold is added to a
//! color before it is mapped to the palette, so a smooth gradient turns into a fine pattern of
//! the two nearest entries instead of bands.
//!
//! The threshold is looked up by the position inside the PICTURE (a tile for blocks, a pixel of
//! the picture scaled to its footprint for Sixel), never by where it sits on the screen: a
//! picture that scrolls keeps the same pattern, nothing shimmers, and a cached crop stays valid.

/// The 8x8 Bayer matrix, every value 0..=63 once.
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9,
    41, 51, 19, 59, 27, 49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

/// How far a threshold moves a color (the whole span, so plus or minus half of it) before the
/// 256-color cube: its steps are 40 to 55 apart.
pub const BLOCK_SPAN: i32 = 40;

/// The same for Sixel registers, which a median cut puts much closer together than the cube.
pub const SIXEL_SPAN: i32 = 24;

/// The threshold at a position of the picture, in `-span/2 .. span/2` and zero on average.
pub fn offset(x: usize, y: usize, span: i32) -> i32 {
    let t = i32::from(BAYER[(y % 8) * 8 + x % 8]);
    ((2 * t + 1 - 64) * span) / 128
}

/// A color moved by the threshold at a position of the picture.
pub fn nudge(c: [u8; 3], x: usize, y: usize, span: i32) -> [u8; 3] {
    let d = offset(x, y, span);
    c.map(|v| (i32::from(v) + d).clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matrix_holds_every_threshold_once() {
        let mut seen = [false; 64];
        for v in BAYER {
            assert!(!seen[usize::from(v)], "{v} twice");
            seen[usize::from(v)] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn the_thresholds_average_to_nothing_and_stay_inside_the_span() {
        for span in [SIXEL_SPAN, BLOCK_SPAN] {
            let all: Vec<i32> = (0..64).map(|i| offset(i % 8, i / 8, span)).collect();
            assert!(all.iter().all(|d| d.abs() <= span / 2), "{span}");
            let sum: i32 = all.iter().sum();
            assert!(sum.abs() <= 64, "mean off by {} for span {span}", sum / 64);
        }
    }

    #[test]
    fn the_pattern_repeats_every_eight_pixels_so_it_depends_on_the_position_and_nothing_else() {
        for (x, y) in [(0, 0), (3, 5), (7, 7)] {
            assert_eq!(offset(x, y, 40), offset(x + 8, y, 40));
            assert_eq!(offset(x, y, 40), offset(x, y + 8 * 5, 40));
            assert_eq!(nudge([10, 100, 250], x, y, 24), nudge([10, 100, 250], x + 64, y + 16, 24));
        }
    }

    #[test]
    fn nudging_never_leaves_the_color_range() {
        for y in 0..8 {
            for x in 0..8 {
                let low = nudge([0, 0, 0], x, y, 40);
                let high = nudge([255, 255, 255], x, y, 40);
                assert!(low.iter().all(|&v| v <= 20) && high.iter().all(|&v| v >= 235), "{x},{y}");
            }
        }
    }

    #[test]
    fn neighboring_positions_get_different_thresholds() {
        assert_ne!(offset(0, 0, 40), offset(1, 0, 40));
        assert_ne!(offset(0, 0, 40), offset(0, 1, 40));
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;
    use crate::render::xterm::to_256;

    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

    fn color_of(index: u8) -> [i32; 3] {
        match index {
            16..=231 => {
                let i = usize::from(index) - 16;
                [i / 36, (i / 6) % 6, i % 6].map(|l| i32::from(LEVELS[l]))
            }
            232..=255 => [8 + 10 * (i32::from(index) - 232); 3],
            _ => panic!("not a color of the cube or the ramp: {index}"),
        }
    }

    fn error_of_the_mean(c: [u8; 3], dither: bool) -> i32 {
        let mut sum = [0i32; 3];
        for i in 0..64 {
            let shown = if dither { nudge(c, i % 8, i / 8, BLOCK_SPAN) } else { c };
            for (s, v) in sum.iter_mut().zip(color_of(to_256(shown[0], shown[1], shown[2]))) {
                *s += v;
            }
        }
        sum.iter().zip(c).map(|(s, v)| (s / 64 - i32::from(v)).abs()).sum()
    }

    #[test]
    fn dithering_brings_the_average_of_a_patch_closer_to_the_color_than_one_palette_entry() {
        let mut worse = 0;
        for c in [[60, 120, 200], [20, 70, 160], [115, 150, 230], [100, 100, 180], [210, 40, 90]] {
            let (plain, dithered) = (error_of_the_mean(c, false), error_of_the_mean(c, true));
            assert!(dithered <= plain, "{c:?}: {dithered} against {plain}");
            if dithered * 2 > plain {
                worse += 1;
            }
        }
        assert!(worse <= 1, "the average should be at least twice as close for nearly every color");
    }
}
