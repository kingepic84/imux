//! Mapping 24-bit colors onto the xterm 256-color palette, for terminals without truecolor.

const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn cube_index(v: u8) -> usize {
    match v {
        0..=47 => 0,
        48..=114 => 1,
        _ => (usize::from(v) - 35) / 40,
    }
}

fn dist(a: [u8; 3], b: [u8; 3]) -> i32 {
    a.iter().zip(&b).map(|(&x, &y)| (i32::from(x) - i32::from(y)).pow(2)).sum()
}

/// Nearest palette entry in the 6x6x6 color cube or the 24-step gray ramp.
pub fn to_256(r: u8, g: u8, b: u8) -> u8 {
    let (ri, gi, bi) = (cube_index(r), cube_index(g), cube_index(b));
    let cube = [LEVELS[ri], LEVELS[gi], LEVELS[bi]];
    let cube_code = 16 + 36 * ri + 6 * gi + bi;

    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let step = ((avg.saturating_sub(8) + 5) / 10).min(23) as u8;
    let level = 8 + 10 * step;
    let gray = [level; 3];

    let want = [r, g, b];
    if dist(want, gray) < dist(want, cube) { 232 + step } else { cube_code as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_of_the_cube() {
        assert_eq!(to_256(0, 0, 0), 16);
        assert_eq!(to_256(255, 255, 255), 231);
        assert_eq!(to_256(255, 0, 0), 196);
        assert_eq!(to_256(0, 255, 0), 46);
        assert_eq!(to_256(0, 0, 255), 21);
        assert_eq!(to_256(255, 255, 0), 226);
    }

    #[test]
    fn grays_use_the_ramp_when_it_is_closer() {
        assert_eq!(to_256(128, 128, 128), 244);
        assert_eq!(to_256(8, 8, 8), 232);
        assert_eq!(to_256(238, 238, 238), 255);
        assert_eq!(to_256(88, 91, 112), 241);
    }

    #[test]
    fn cube_steps_use_the_uneven_xterm_levels() {
        assert_eq!(to_256(95, 0, 0), 52);
        assert_eq!(to_256(135, 175, 215), 110);
        assert_eq!(cube_index(47), 0);
        assert_eq!(cube_index(48), 1);
        assert_eq!(cube_index(114), 1);
        assert_eq!(cube_index(115), 2);
    }

    #[test]
    fn every_color_maps_into_the_palette_range() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    assert!(to_256(r as u8, g as u8, b as u8) >= 16);
                }
            }
        }
    }
}
