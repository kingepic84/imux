//! The popup of a window (`display-popup`): a box over the panes that holds one more pane, a pane
//! that is in no layout. The pure parts are here: when a popup closes, how big it is, and where
//! the program's screen sits inside the border. The pane itself lives in `Window::panes` like any
//! other, so output, replies and the poll treat it the same; `Window::popup` says it is the popup.

use super::layout::PaneId;
use crate::term::cell::Style;

/// What the end of the popup's program does: `-E` once closes, `-E -E` closes only on success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnExit {
    Close,
    CloseIfSuccess,
    Stay,
}

/// When a popup closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosePolicy {
    pub on_exit: OnExit,
    /// `-k`: a popup that stayed after its program ended closes on any key, not only Escape.
    pub any_key: bool,
    /// Escape and `C-c` close a popup whose program is still running (without `-E`; with it they
    /// reach the program).
    pub cancel: bool,
}

impl ClosePolicy {
    /// From the number of `-E` flags and whether `-k` was given.
    pub fn from_flags(e: u8, k: bool) -> ClosePolicy {
        let on_exit = match e {
            0 => OnExit::Stay,
            1 => OnExit::Close,
            _ => OnExit::CloseIfSuccess,
        };
        ClosePolicy { on_exit, any_key: k, cancel: e == 0 }
    }

    /// Whether a program that ended with `status` takes its popup with it.
    pub fn closes_on_exit(&self, status: i32) -> bool {
        match self.on_exit {
            OnExit::Close => true,
            OnExit::CloseIfSuccess => status == 0,
            OnExit::Stay => false,
        }
    }
}

/// How the keys of a client go to a popup, decided from the key and the state of the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyFate {
    /// To the program.
    Program,
    /// The popup closes.
    Close,
    /// Nobody wants it (a popup that stayed, a key that does not close it).
    Drop,
}

/// What a key does to a popup. `cancel_key` is Escape or `C-c`, `stayed` says the program has
/// ended.
pub fn key_fate(policy: &ClosePolicy, stayed: bool, cancel_key: bool) -> KeyFate {
    match (stayed, cancel_key) {
        (true, true) => KeyFate::Close,
        (true, false) if policy.any_key => KeyFate::Close,
        (true, false) => KeyFate::Drop,
        (false, true) if policy.cancel => KeyFate::Close,
        (false, _) => KeyFate::Program,
    }
}

/// The start of the error a waiting `display-popup` fails with when its program ended with a
/// status other than 0; the status follows. The `tmux` shim reads it back.
pub const ENDED_WITH: &str = "display-popup: the program ended with status ";

/// Presses of the prefix key in a row that close a popup whatever its program does.
pub const CHORD: u8 = 3;

#[derive(Clone, Debug)]
pub struct Popup {
    /// The pane that runs the program (in `Window::panes`, in no layout).
    pub pane: PaneId,
    /// Left, top, width and height of the box including its border, in cells of the part of the
    /// screen the panes use.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    /// A line set of `config::options::BORDER_LINES`.
    pub lines: String,
    pub title: String,
    /// The popup's own style and its border's.
    pub styles: (Style, Style),
    pub policy: ClosePolicy,
    /// The pane whose program asked for the popup, when it was not the user's key or command.
    pub origin: Option<PaneId>,
    /// The socket connection waiting for the popup to close.
    pub waiter: Option<usize>,
    /// How the program ended, once it has and the popup stayed.
    pub exit: Option<i32>,
    /// Presses of the prefix key in a row (see `CHORD`).
    pub chord: u8,
}

impl Popup {
    pub fn bordered(&self) -> bool {
        crate::render::overlay::line_glyphs(&self.lines).is_some()
    }

    /// Left, top, width and height of the program's screen inside the border.
    pub fn inner(&self) -> (usize, usize, usize, usize) {
        if self.bordered() {
            (self.x + 1, self.y + 1, self.w.saturating_sub(2).max(1), self.h.saturating_sub(2).max(1))
        } else {
            (self.x, self.y, self.w.max(1), self.h.max(1))
        }
    }

    /// Keep the box inside a screen part of `sx` by `sy` cells; true when it moved or changed size.
    pub fn fit(&mut self, sx: usize, sy: usize) -> bool {
        let before = (self.x, self.y, self.w, self.h);
        self.w = self.w.min(sx);
        self.h = self.h.min(sy);
        self.x = self.x.min(sx - self.w);
        self.y = self.y.min(sy - self.h);
        before != (self.x, self.y, self.w, self.h)
    }
}

/// The smallest box that can be shown: three cells each way with a border, one without.
pub fn smallest(bordered: bool) -> usize {
    if bordered { 3 } else { 1 }
}

/// `-w` or `-h`: a number of cells, or `N%` of `total`. `None` is half of `total`. The result is
/// between 1 and `total`.
pub fn size_spec(spec: Option<&str>, total: usize) -> Result<usize, String> {
    let Some(spec) = spec else { return Ok(total / 2) };
    let (digits, percent) = match spec.strip_suffix('%') {
        Some(d) => (d, true),
        None => (spec, false),
    };
    let n: usize = digits.parse().map_err(|_| format!("not a size: {spec}"))?;
    let cells = if percent { total * n.min(100) / 100 } else { n };
    if cells == 0 {
        return Err(format!("not a size: {spec}"));
    }
    Ok(cells.min(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_make_the_policy() {
        let p = ClosePolicy::from_flags(0, false);
        assert_eq!((p.on_exit, p.any_key, p.cancel), (OnExit::Stay, false, true));
        let p = ClosePolicy::from_flags(1, false);
        assert_eq!((p.on_exit, p.cancel), (OnExit::Close, false));
        let p = ClosePolicy::from_flags(2, true);
        assert_eq!((p.on_exit, p.any_key, p.cancel), (OnExit::CloseIfSuccess, true, false));
        assert_eq!(ClosePolicy::from_flags(5, false).on_exit, OnExit::CloseIfSuccess);
    }

    #[test]
    fn the_end_of_the_program_closes_by_the_policy_and_the_status() {
        for (e, status, closes) in [(0, 0, false), (0, 1, false), (1, 0, true), (1, 3, true), (2, 0, true), (2, 3, false)] {
            assert_eq!(ClosePolicy::from_flags(e, false).closes_on_exit(status), closes, "-E x{e}, status {status}");
        }
    }

    #[test]
    fn keys_go_to_the_program_unless_the_popup_may_be_canceled() {
        let plain = ClosePolicy::from_flags(0, false);
        let exit = ClosePolicy::from_flags(1, false);
        assert_eq!(key_fate(&plain, false, true), KeyFate::Close, "Escape cancels a popup without -E");
        assert_eq!(key_fate(&plain, false, false), KeyFate::Program);
        assert_eq!(key_fate(&exit, false, true), KeyFate::Program, "with -E Escape is the program's");
    }

    #[test]
    fn a_popup_that_stayed_closes_on_escape_or_with_k_on_any_key() {
        let plain = ClosePolicy::from_flags(2, false);
        let any = ClosePolicy::from_flags(2, true);
        assert_eq!(key_fate(&plain, true, true), KeyFate::Close);
        assert_eq!(key_fate(&plain, true, false), KeyFate::Drop);
        assert_eq!(key_fate(&any, true, false), KeyFate::Close);
    }

    fn popup(x: usize, y: usize, w: usize, h: usize, lines: &str) -> Popup {
        Popup {
            pane: 1,
            x,
            y,
            w,
            h,
            lines: lines.to_string(),
            title: String::new(),
            styles: Default::default(),
            policy: ClosePolicy::from_flags(0, false),
            origin: None,
            waiter: None,
            exit: None,
            chord: 0,
        }
    }

    #[test]
    fn the_program_screen_is_inside_the_border() {
        assert_eq!(popup(5, 4, 20, 10, "single").inner(), (6, 5, 18, 8));
        assert_eq!(popup(5, 4, 20, 10, "none").inner(), (5, 4, 20, 10));
        assert_eq!(popup(0, 0, 3, 3, "rounded").inner(), (1, 1, 1, 1));
    }

    #[test]
    fn fitting_keeps_the_box_inside_a_smaller_screen() {
        let mut p = popup(60, 15, 20, 8, "single");
        assert!(p.fit(50, 12));
        assert_eq!((p.x, p.y, p.w, p.h), (30, 4, 20, 8));
        assert!(!p.fit(50, 12), "nothing to change the second time");
        assert!(p.fit(10, 5));
        assert_eq!((p.x, p.y, p.w, p.h), (0, 0, 10, 5));
    }

    #[test]
    fn sizes_are_cells_or_percent_and_default_to_half() {
        assert_eq!(size_spec(None, 80), Ok(40));
        assert_eq!(size_spec(Some("30"), 80), Ok(30));
        assert_eq!(size_spec(Some("50%"), 80), Ok(40));
        assert_eq!(size_spec(Some("100%"), 80), Ok(80));
        assert_eq!(size_spec(Some("200"), 80), Ok(80), "more than the window is the window");
        assert!(size_spec(Some("0"), 80).is_err());
        assert!(size_spec(Some("0%"), 80).is_err());
        assert!(size_spec(Some("wide"), 80).is_err());
        assert!(size_spec(Some("-3"), 80).is_err());
    }

    #[test]
    fn the_smallest_box_depends_on_the_border() {
        assert_eq!((smallest(true), smallest(false)), (3, 1));
    }
}
