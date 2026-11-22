//! How the screen is divided: the status rows at the bottom, an optional dock (the agent panel
//! kept open) on one edge, and the rest for the panes. One function so that drawing, mouse
//! hit-testing and pty sizes all use the same rectangles.

use crate::proto::Size;
use crate::session::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockPos {
    Left,
    Right,
    Top,
    Bottom,
}

/// The dock's edge and thickness: columns for left and right, rows for top and bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dock {
    pub pos: DockPos,
    pub size: u16,
}

/// The dock needs this much, and leaves the panes at least this much, or it is not shown.
const MIN_DOCK_COLS: u16 = 8;
const MIN_DOCK_ROWS: u16 = 3;
const MIN_PANE_COLS: u16 = 20;
const MIN_PANE_ROWS: u16 = 5;

/// `(panes, dock)`: the pane area is everything above the status rows minus the dock; the dock is
/// `None` when there is none or the screen is too small for it.
#[cfg(test)]
pub fn layout_regions(size: Size, status_rows: u16, dock: Option<Dock>) -> (Rect, Option<Rect>) {
    layout_regions_at(size, status_rows, false, dock)
}

/// `layout_regions` with the status rows at the top of the screen when `status_top` is set: the
/// panes and the dock then start below them.
pub fn layout_regions_at(size: Size, status_rows: u16, status_top: bool, dock: Option<Dock>) -> (Rect, Option<Rect>) {
    let (panes, dock) = bottom_regions(size, status_rows, dock);
    if !status_top || status_rows == 0 {
        return (panes, dock);
    }
    let shift = |r: Rect| Rect { y: r.y + status_rows.min(size.rows.saturating_sub(1)), ..r };
    (shift(panes), dock.map(shift))
}

fn bottom_regions(size: Size, status_rows: u16, dock: Option<Dock>) -> (Rect, Option<Rect>) {
    let rows = size.rows.saturating_sub(status_rows).max(1);
    let cols = size.cols;
    let full = Rect { x: 0, y: 0, w: cols, h: rows };
    let Some(Dock { pos, size: d }) = dock else { return (full, None) };
    match pos {
        DockPos::Left | DockPos::Right => {
            if d < MIN_DOCK_COLS || cols < d + MIN_PANE_COLS {
                return (full, None);
            }
            let (px, dx) = if pos == DockPos::Left { (d, 0) } else { (0, cols - d) };
            (Rect { x: px, y: 0, w: cols - d, h: rows }, Some(Rect { x: dx, y: 0, w: d, h: rows }))
        }
        DockPos::Top | DockPos::Bottom => {
            if d < MIN_DOCK_ROWS || rows < d + MIN_PANE_ROWS {
                return (full, None);
            }
            let (py, dy) = if pos == DockPos::Top { (d, 0) } else { (0, rows - d) };
            (Rect { x: 0, y: py, w: cols, h: rows - d }, Some(Rect { x: 0, y: dy, w: cols, h: d }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cols: u16, rows: u16) -> Size {
        Size { cols, rows, xpix: 0, ypix: 0 }
    }

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn without_a_dock_the_panes_get_everything_above_the_status_rows() {
        assert_eq!(layout_regions(size(100, 30), 1, None), (rect(0, 0, 100, 29), None));
        assert_eq!(layout_regions(size(100, 30), 3, None), (rect(0, 0, 100, 27), None));
        assert_eq!(layout_regions(size(10, 1), 1, None).0.h, 1);
    }

    #[test]
    fn each_edge_takes_its_share_and_the_panes_get_the_rest() {
        let d = |pos, size| Some(Dock { pos, size });
        assert_eq!(layout_regions(size(100, 30), 1, d(DockPos::Left, 30)), (rect(30, 0, 70, 29), Some(rect(0, 0, 30, 29))));
        assert_eq!(layout_regions(size(100, 30), 1, d(DockPos::Right, 30)), (rect(0, 0, 70, 29), Some(rect(70, 0, 30, 29))));
        assert_eq!(layout_regions(size(100, 30), 1, d(DockPos::Top, 8)), (rect(0, 8, 100, 21), Some(rect(0, 0, 100, 8))));
        assert_eq!(layout_regions(size(100, 30), 1, d(DockPos::Bottom, 8)), (rect(0, 0, 100, 21), Some(rect(0, 21, 100, 8))));
    }

    #[test]
    fn the_regions_never_overlap_and_fill_the_area() {
        for pos in [DockPos::Left, DockPos::Right, DockPos::Top, DockPos::Bottom] {
            let (panes, dock) = layout_regions(size(120, 40), 2, Some(Dock { pos, size: 10 }));
            let dock = dock.unwrap();
            assert_eq!(u32::from(panes.w) * u32::from(panes.h) + u32::from(dock.w) * u32::from(dock.h), 120 * 38, "{pos:?}");
            let apart = panes.x + panes.w <= dock.x || dock.x + dock.w <= panes.x || panes.y + panes.h <= dock.y || dock.y + dock.h <= panes.y;
            assert!(apart, "{pos:?}");
        }
    }

    #[test]
    fn a_status_bar_on_top_pushes_the_panes_and_the_dock_down() {
        let d = |pos, size| Some(Dock { pos, size });
        assert_eq!(layout_regions_at(size(100, 30), 1, true, None), (rect(0, 1, 100, 29), None));
        assert_eq!(layout_regions_at(size(100, 30), 3, true, None), (rect(0, 3, 100, 27), None));
        assert_eq!(layout_regions_at(size(100, 30), 1, false, None), layout_regions(size(100, 30), 1, None));
        assert_eq!(layout_regions_at(size(100, 30), 1, true, d(DockPos::Left, 30)), (rect(30, 1, 70, 29), Some(rect(0, 1, 30, 29))));
        assert_eq!(layout_regions_at(size(100, 30), 1, true, d(DockPos::Top, 8)), (rect(0, 9, 100, 21), Some(rect(0, 1, 100, 8))));
        assert_eq!(layout_regions_at(size(100, 30), 1, true, d(DockPos::Bottom, 8)), (rect(0, 1, 100, 21), Some(rect(0, 22, 100, 8))));
    }

    #[test]
    fn no_status_rows_give_the_panes_the_whole_screen_on_either_side() {
        for top in [false, true] {
            assert_eq!(layout_regions_at(size(100, 30), 0, top, None), (rect(0, 0, 100, 30), None), "top={top}");
        }
        let d = Some(Dock { pos: DockPos::Top, size: 8 });
        assert_eq!(layout_regions_at(size(100, 30), 0, true, d), (rect(0, 8, 100, 22), Some(rect(0, 0, 100, 8))));
    }

    #[test]
    fn a_top_bar_on_a_tiny_screen_keeps_the_panes_on_the_screen() {
        let (panes, _) = layout_regions_at(size(10, 2), 3, true, None);
        assert_eq!((panes.y, panes.h), (1, 1));
        let (panes, _) = layout_regions_at(size(10, 1), 1, true, None);
        assert_eq!((panes.y, panes.h), (0, 1));
    }

    #[test]
    fn the_top_regions_never_overlap_and_fill_the_area() {
        for pos in [DockPos::Left, DockPos::Right, DockPos::Top, DockPos::Bottom] {
            let (panes, dock) = layout_regions_at(size(120, 40), 2, true, Some(Dock { pos, size: 10 }));
            let dock = dock.unwrap();
            assert_eq!(u32::from(panes.w) * u32::from(panes.h) + u32::from(dock.w) * u32::from(dock.h), 120 * 38, "{pos:?}");
            assert!(panes.y >= 2 && dock.y >= 2, "{pos:?}: both start under the bar");
            assert!(panes.y + panes.h <= 40 && dock.y + dock.h <= 40, "{pos:?}: both end on the screen");
        }
    }

    #[test]
    fn a_dock_that_would_squeeze_the_panes_or_is_too_thin_is_not_shown() {
        let d = |pos, size| Some(Dock { pos, size });
        assert_eq!(layout_regions(size(40, 30), 1, d(DockPos::Left, 30)).1, None);
        assert_eq!(layout_regions(size(100, 30), 1, d(DockPos::Left, 4)).1, None);
        assert_eq!(layout_regions(size(100, 12), 1, d(DockPos::Bottom, 8)).1, None);
        assert_eq!(layout_regions(size(100, 30), 1, d(DockPos::Top, 1)).1, None);
        assert_eq!(layout_regions(size(40, 30), 1, d(DockPos::Left, 30)).0, rect(0, 0, 40, 29));
    }
}
