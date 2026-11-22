//! Where a box (a menu, a popup) goes: the `-x` and `-y` of `display-menu` and `display-popup`.
//! Every number is a cell of the part of the screen the panes use, counted from its top left
//! corner. `-x` is the left column; `-y` is the row of the box's BOTTOM edge, so a box with `-y 10`
//! and a height of 4 occupies rows 6 to 9 (and one that would start above the first row starts on
//! it). The words and the `popup_*` variables a format may use are those of the other multiplexers
//! that have these commands, so configurations written for them place boxes the same way.

use crate::config::format;

/// What the position depends on.
pub struct Where {
    /// Width and height of the part of the screen the panes use.
    pub sx: usize,
    pub sy: usize,
    /// Width and height of the box.
    pub w: usize,
    pub h: usize,
    /// Left, top, width and height of the pane the formats are about.
    pub pane: (usize, usize, usize, usize),
    /// Whether there is a status bar, and if so whether it is at the top.
    pub status: Option<bool>,
    /// Where the last box of this kind had its top left corner.
    pub last: Option<(usize, usize)>,
}

impl Where {
    fn max_x(&self) -> i64 {
        self.sx.saturating_sub(self.w) as i64
    }

    fn max_y(&self) -> i64 {
        self.sy.saturating_sub(self.h) as i64
    }

    fn center_x(&self) -> i64 {
        ((self.sx as i64 - 1) / 2 - (self.w / 2) as i64).max(0)
    }

    fn center_y(&self) -> i64 {
        let n = (self.sy as i64 - 1) / 2 + (self.h / 2) as i64;
        if n >= self.sy as i64 { self.max_y() } else { n }
    }

    /// The variables a position format may use.
    fn vars(&self, mouse: Option<(usize, usize)>) -> Vec<(&'static str, i64)> {
        let (left, top, pw, ph) = self.pane;
        let (cx, cy) = (self.center_x(), self.center_y());
        let sy = self.sy as i64;
        let h = self.h as i64;
        // Where the pointer is, as a command bound to a mouse key opened the box: the box centered
        // on it, below it, above it. Without a pointer these are the center.
        let (mx, my) = mouse.map_or((cx, cy), |(x, y)| (x as i64, y as i64));
        let (m_cx, m_cy, m_top, m_bottom) = match mouse {
            None => (cx, cy, cy, cy),
            Some(_) => {
                let n = my - h / 2;
                let m_cy = if n + h >= sy { self.max_y() } else { n };
                let below = my + h;
                (
                    (mx - (self.w / 2) as i64).max(0),
                    m_cy,
                    if below >= sy { sy - 1 } else { below },
                    (my - h).max(0),
                )
            }
        };
        let pane_top = if (top + self.h) as i64 >= self.sy as i64 { self.max_y() } else { (top + self.h) as i64 };
        let status_y = match self.status {
            Some(true) => self.h as i64,
            _ => self.sy as i64,
        };
        let last = self.last.unwrap_or((cx as usize, cy.saturating_sub(self.h as i64) as usize));
        vec![
            ("popup_center_x", cx),
            ("popup_center_y", cy),
            ("popup_centre_x", cx),
            ("popup_centre_y", cy),
            ("popup_width", self.w as i64),
            ("popup_height", self.h as i64),
            ("popup_pane_left", left as i64),
            ("popup_pane_right", ((left + pw) as i64 - self.w as i64).max(0)),
            ("popup_pane_top", pane_top),
            ("popup_pane_bottom", (top + ph) as i64),
            ("popup_mouse_x", mx),
            ("popup_mouse_y", my),
            ("popup_mouse_center_x", m_cx),
            ("popup_mouse_center_y", m_cy),
            ("popup_mouse_centre_x", m_cx),
            ("popup_mouse_centre_y", m_cy),
            ("popup_mouse_top", m_top),
            ("popup_mouse_bottom", m_bottom),
            ("popup_last_x", last.0 as i64),
            ("popup_last_y", (last.1 + self.h) as i64),
            ("popup_status_line_y", status_y),
            ("popup_window_status_line_x", if mouse.is_some() { mx } else { cx }),
            ("popup_window_status_line_y", status_y),
        ]
    }
}

/// The left column and top row of a box from `-x` and `-y`. For `-x`: a number, `C` (center, the
/// default), `R` (the box's right edge on the pane's), `P` (the pane's left edge), `L` (where the
/// last box was), `M` and `W` (nothing to follow, the mouse or a tab of the status line: the
/// center); for `-y`: a number (the bottom edge), `C`, `P` (the box's bottom edge on the pane's),
/// `L`, `S` (against the status line), `M`, `W` (the center); or a format over the `popup_*`
/// variables and, through `outer`, the pane's own. The result is kept inside the window.
#[cfg(test)]
pub fn place(x: Option<&str>, y: Option<&str>, at: &Where, outer: &dyn Fn(&str) -> Option<String>) -> Result<(usize, usize), String> {
    place_at(x, y, at, None, outer)
}

/// `place` with the pointer, in cells of the same part of the screen, when the command was opened
/// by a mouse key: `M` and `W` then follow it.
pub fn place_at(
    x: Option<&str>,
    y: Option<&str>,
    at: &Where,
    mouse: Option<(usize, usize)>,
    outer: &dyn Fn(&str) -> Option<String>,
) -> Result<(usize, usize), String> {
    let own: Vec<(&'static str, String)> = at.vars(mouse).into_iter().map(|(k, v)| (k, v.to_string())).collect();
    let look = |name: &str| own.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone()).or_else(|| outer(name));
    let number = |spec: &str, which: &str| -> Result<i64, String> {
        let text = if spec.contains('#') { format::expand(spec, &look) } else { spec.to_string() };
        text.trim().parse::<i64>().map_err(|_| format!("-{which} is not a position: {spec}"))
    };
    let var = |name: &str| look(name).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let x = match x.unwrap_or("C") {
        "C" => var("popup_center_x"),
        "R" => var("popup_pane_right"),
        "P" => var("popup_pane_left"),
        "M" => var("popup_mouse_center_x"),
        "L" => var("popup_last_x"),
        "W" => var("popup_window_status_line_x"),
        spec => number(spec, "x")?,
    };
    let bottom = match y.unwrap_or("C") {
        "C" => var("popup_center_y"),
        "P" => var("popup_pane_bottom"),
        "M" => var("popup_mouse_top"),
        "L" => var("popup_last_y"),
        "S" => var("popup_status_line_y"),
        "W" => var("popup_window_status_line_y"),
        spec => number(spec, "y")?,
    };
    let top = (bottom - at.h as i64).max(0);
    Ok((x.clamp(0, at.max_x()) as usize, top.clamp(0, at.max_y()) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Where {
        Where { sx: 80, sy: 23, w: 20, h: 6, pane: (40, 2, 30, 10), status: Some(false), last: Some((7, 8)) }
    }

    fn pos(x: &str, y: &str) -> Result<(usize, usize), String> {
        place(Some(x), Some(y), &at(), &|_| None)
    }

    #[test]
    fn the_default_is_the_center() {
        assert_eq!(place(None, None, &at(), &|_| None), Ok((29, 8)));
        assert_eq!(pos("C", "C"), Ok((29, 8)));
    }

    #[test]
    fn a_number_for_y_is_the_bottom_edge() {
        assert_eq!(pos("5", "10"), Ok((5, 4)), "rows 4 to 9");
        assert_eq!(pos("5", "3"), Ok((5, 0)), "one that would start above the first row starts on it");
        assert_eq!(pos("5", "6"), Ok((5, 0)));
        assert_eq!(pos("5", "7"), Ok((5, 1)));
    }

    #[test]
    fn letters_name_places_around_the_pane_the_status_line_and_the_last_box() {
        assert_eq!(pos("P", "P"), Ok((40, 6)), "the pane's left edge, its bottom edge");
        assert_eq!(pos("R", "C").map(|p| p.0), Ok(50), "the box ends where the pane does");
        assert_eq!(pos("L", "L"), Ok((7, 8)));
        assert_eq!(pos("C", "S").map(|p| p.1), Ok(17), "above a bottom bar");
        let top = Where { status: Some(true), ..at() };
        assert_eq!(place(None, Some("S"), &top, &|_| None).map(|p| p.1), Ok(0), "under a top bar");
        assert_eq!(pos("M", "M"), Ok((29, 8)), "no mouse to follow: the center");
        assert_eq!(pos("W", "W").map(|p| p.0), Ok(29));
    }

    #[test]
    fn the_result_is_kept_inside_the_window() {
        assert_eq!(pos("-4", "-9"), Ok((0, 0)));
        assert_eq!(pos("500", "500"), Ok((60, 17)));
    }

    #[test]
    fn formats_see_the_popup_variables_and_the_panes() {
        assert_eq!(pos("#{popup_pane_left}", "#{popup_pane_bottom}"), Ok((40, 6)));
        assert!(pos("#{e}", "1").unwrap_err().contains("not a position"));
        let outer = |n: &str| (n == "mine").then(|| "12".to_string());
        assert_eq!(place(Some("#{mine}"), Some("9"), &at(), &outer), Ok((12, 3)), "the pane's variables are visible too");
        assert_eq!(pos("#{popup_width}", "#{popup_height}"), Ok((20, 0)));
    }

    #[test]
    fn a_box_as_big_as_the_window_goes_to_its_corner() {
        let full = Where { sx: 80, sy: 23, w: 80, h: 23, ..at() };
        assert_eq!(place(None, None, &full, &|_| None), Ok((0, 0)));
        assert_eq!(place(Some("9"), Some("40"), &full, &|_| None), Ok((0, 0)));
    }

    fn at_pointer(x: usize, y: usize) -> Result<(usize, usize), String> {
        place_at(Some("M"), Some("M"), &at(), Some((x, y)), &|_| None)
    }

    #[test]
    fn m_follows_the_pointer_when_a_mouse_key_opened_the_box() {
        assert_eq!(at_pointer(30, 10), Ok((20, 10)), "centered on it sideways, from it downwards");
        assert_eq!(at_pointer(5, 2), Ok((0, 2)), "kept inside on the left");
        assert_eq!(at_pointer(70, 22), Ok((60, 16)), "and at the bottom right");
        assert_eq!(place_at(Some("M"), Some("M"), &at(), None, &|_| None), place(None, None, &at(), &|_| None), "no pointer, the center");
    }

    #[test]
    fn the_pointer_variables_are_there_for_formats() {
        let r = place_at(Some("#{popup_mouse_x}"), Some("#{popup_mouse_bottom}"), &at(), Some((30, 10)), &|_| None);
        assert_eq!(r, Ok((30, 0)), "x is the pointer's column; y the bottom edge six rows above it");
    }

    #[test]
    fn the_old_centre_spelling_of_the_variables_still_works() {
        assert_eq!(pos("#{popup_center_x}", "#{popup_center_y}"), pos("#{popup_centre_x}", "#{popup_centre_y}"));
        assert_eq!(pos("#{popup_center_x}", "#{popup_center_y}").map(|p| p.0), Ok(29));
        assert_eq!(pos("#{popup_mouse_center_x}", "5"), pos("#{popup_mouse_centre_x}", "5"));
    }
}
