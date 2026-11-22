//! The mouse as keys of the root table. A press, a release, a drag, the wheel or a second and third
//! press becomes a key such as `MouseDown1Pane` or `WheelUpStatus` (see `input::keys::MouseKey`);
//! when `bind -n` gave that key a command it runs and the event is used up, otherwise the event goes
//! on to what imux does with it by itself (focus, divider drag, copy mode, the program's own mouse
//! mode). A binding on a press also moves the focus to what is under the pointer first, as that
//! built-in behavior does; the other events leave the focus alone. Whatever the binding runs sees
//! `#{mouse_x}`, `#{mouse_y}`, `#{mouse_any_flag}`, `#{mouse_word}` and `#{mouse_line}`, and a menu
//! or popup it opens may use the pointer for `-x M`, `-y M`, `-x W` and `-y W`.

use std::time::{Duration, Instant};

use super::{MOUSE_MOTION, MOUSE_WHEEL, Mouse, Server};
use crate::input::bindings::Command;
use crate::input::keys::{ALT, CTRL, Code, Key, MouseKey, MouseKind, MouseLoc, SHIFT};
use crate::sanitize;
use crate::session::layout::{PaneId, Rect};
use crate::term::grid::MouseMode;

/// The button that is down, and where its press was (a drag belongs to the place it started in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Held {
    pub button: u8,
    pub loc: MouseLoc,
    pub dragged: bool,
}

/// The last press, for counting second and third presses.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pressed {
    at: Instant,
    x: u16,
    y: u16,
    button: u8,
    count: u8,
}

/// What a binding that is running knows about the pointer.
#[derive(Clone, Debug, Default)]
pub(super) struct MouseCtx {
    pub vars: Vec<(&'static str, String)>,
    /// Column and row on the screen.
    pub at: (usize, usize),
}

/// What is under the pointer.
struct Spot {
    loc: MouseLoc,
    pane: Option<(PaneId, Rect)>,
    /// Position of the window whose entry in the status line it is.
    tab: Option<usize>,
}

/// The button of a report: 1 to 3, none for motion without a button, the wheel and the extra ones.
pub(super) fn button(code: u16) -> Option<u8> {
    if code & (MOUSE_WHEEL | 128) != 0 {
        return None;
    }
    match code & 3 {
        0 => Some(1),
        1 => Some(2),
        2 => Some(3),
        _ => None,
    }
}

/// The modifier keys held, as key modifiers.
pub(super) fn modifiers(code: u16) -> u8 {
    (if code & 4 != 0 { SHIFT } else { 0 }) | (if code & 8 != 0 { ALT } else { 0 }) | (if code & 16 != 0 { CTRL } else { 0 })
}

/// What a report is, given the button held and how many presses in a row this one is.
pub(super) fn kind(m: &Mouse, held: Option<Held>, count: u8) -> Option<MouseKind> {
    if m.code & 128 != 0 {
        return None;
    }
    if m.code & MOUSE_WHEEL != 0 {
        if m.release {
            return None;
        }
        return Some(if m.code & 1 == 0 { MouseKind::WheelUp } else { MouseKind::WheelDown });
    }
    if m.release {
        return Some(if held.is_some_and(|h| h.dragged) { MouseKind::DragEnd } else { MouseKind::Up });
    }
    if m.code & MOUSE_MOTION != 0 {
        return (held.is_some() && button(m.code).is_some()).then_some(MouseKind::Drag);
    }
    button(m.code)?;
    Some(match count {
        0 | 1 => MouseKind::Down,
        2 => MouseKind::Double,
        _ => MouseKind::Triple,
    })
}

/// The word of `line` that the column `x` is in; empty on a blank.
fn word_at(line: &str, x: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let Some(&here) = chars.get(x) else { return String::new() };
    if here.is_whitespace() {
        return String::new();
    }
    let start = chars[..x].iter().rposition(|c| c.is_whitespace()).map_or(0, |i| i + 1);
    let end = chars[x..].iter().position(|c| c.is_whitespace()).map_or(chars.len(), |i| x + i);
    chars[start..end].iter().collect()
}

impl Server {
    /// Count a press: the number of presses in a row on the same cell with the same button, each
    /// within `double-click-time` of the one before.
    fn count_press(&mut self, m: &Mouse) -> u8 {
        let now = Instant::now();
        let Some(button) = button(m.code) else { return 0 };
        let within = Duration::from_millis(self.options.num("double-click-time"));
        let count = match self.mouse_pressed {
            Some(p) if p.button == button && p.x == m.x && p.y == m.y && now.duration_since(p.at) <= within => (p.count % 3) + 1,
            _ => 1,
        };
        self.mouse_pressed = Some(Pressed { at: now, x: m.x, y: m.y, button, count });
        count
    }

    /// What is at the pointer, or none when it is somewhere a binding cannot name (the docked
    /// agent panel, outside every pane).
    fn mouse_spot(&mut self, client: usize, si: usize, m: &Mouse) -> Option<Spot> {
        let c = self.clients.get(&client)?;
        let (size, client_size) = (self.size_for(c.session)?, c.size?);
        let status_rows = self.sessions[si].status_rows;
        let bar_rows = status_rows.min(client_size.rows);
        let bar_first = if self.status_top() { 0 } else { client_size.rows - bar_rows };
        if bar_rows > 0 && m.y >= bar_first && m.y < bar_first + bar_rows {
            let (loc, tab) = self.status_spot(client, si, usize::from(m.y - bar_first), m.x);
            return Some(Spot { loc, pane: None, tab });
        }
        if self.dock_rect(size, status_rows).is_some_and(|d| m.x >= d.x && m.x < d.x + d.w && m.y >= d.y && m.y < d.y + d.h) {
            return None;
        }
        let area = self.pane_area(size, status_rows);
        let win = self.sessions[si].window();
        let on_border = win.layout.separators(area).into_iter().any(|s| {
            if s.vertical { m.x == s.x && m.y >= s.y && m.y < s.y + s.len } else { m.y == s.y && m.x >= s.x && m.x < s.x + s.len }
        });
        if on_border {
            return Some(Spot { loc: MouseLoc::Border, pane: None, tab: None });
        }
        let pane = win.layout.rects(area).into_iter().find(|(_, r)| m.x >= r.x && m.x < r.x + r.w && m.y >= r.y && m.y < r.y + r.h);
        pane.map(|p| Spot { loc: MouseLoc::Pane, pane: Some(p), tab: None })
    }

    /// The variables a binding sees for a pointer in `spot`.
    fn mouse_context(&self, si: usize, spot: &Spot, m: &Mouse) -> MouseCtx {
        let mut vars: Vec<(&'static str, String)> = Vec::new();
        if let Some((id, rect)) = spot.pane
            && let Some(pane) = self.sessions[si].window().panes.get(&id)
        {
            let (x, y) = (usize::from(m.x - rect.x), usize::from(m.y - rect.y));
            let g = &pane.term.grid;
            vars.push(("mouse_x", x.to_string()));
            vars.push(("mouse_y", y.to_string()));
            vars.push(("mouse_any_flag", if g.modes.mouse != MouseMode::None || g.is_alt() { "1" } else { "0" }.to_string()));
            if let Some(row) = g.screen.get(y) {
                let line = crate::term::cell::Cell::text_for_copy(&row.cells);
                vars.push(("mouse_word", sanitize::clean(&word_at(line.trim_end(), x))));
                vars.push(("mouse_line", sanitize::clean(line.trim_end())));
            }
        }
        MouseCtx { vars, at: (usize::from(m.x), usize::from(m.y)) }
    }

    /// A mouse report, as a key of the root table. True when a binding took it.
    pub(super) fn mouse_binding(&mut self, client: usize, si: usize, m: Mouse) -> bool {
        let count = if !m.release && m.code & (MOUSE_MOTION | MOUSE_WHEEL) == 0 { self.count_press(&m) } else { 0 };
        let held = self.mouse_held;
        let Some(kind) = kind(&m, held, count) else { return false };
        let spot = self.mouse_spot(client, si, &m);
        let loc = match kind {
            MouseKind::Drag | MouseKind::DragEnd => held.map(|h| h.loc),
            _ => spot.as_ref().map(|s| s.loc),
        };
        let wheel = matches!(kind, MouseKind::WheelUp | MouseKind::WheelDown);
        let pressed = matches!(kind, MouseKind::Down | MouseKind::Double | MouseKind::Triple);
        let btn = if wheel { Some(0) } else { button(m.code).or(held.map(|h| h.button)) };
        match kind {
            MouseKind::Down | MouseKind::Double | MouseKind::Triple => {
                self.mouse_held = loc.zip(btn).map(|(loc, button)| Held { button, loc, dragged: false });
            }
            MouseKind::Drag => {
                if let Some(h) = self.mouse_held.as_mut() {
                    h.dragged = true;
                }
            }
            MouseKind::Up | MouseKind::DragEnd => self.mouse_held = None,
            _ => {}
        }
        let (Some(loc), Some(button)) = (loc, btn) else { return false };
        let key = Key::new(Code::Mouse(MouseKey { kind, button, loc }), modifiers(m.code));
        let Some(command) = self.bindings.root_lookup(&key).map(|b| b.command.clone()) else { return false };
        // `resize-pane -M` (the stock `bind -n MouseDrag1Border resize-pane -M`) says: do what a
        // border drag does, which the built-in handling is.
        if matches!(&command, Command::Server(w) if w.first().is_some_and(|n| matches!(n.as_str(), "resize-pane" | "resizep")) && w.iter().any(|a| a == "-M")) {
            return false;
        }
        let Some(spot) = spot else { return false };
        if pressed {
            self.mouse_select(si, &spot);
        }
        self.mouse_ctx = Some(self.mouse_context(si, &spot, &m));
        self.run_command(client, &command);
        self.mouse_ctx = None;
        true
    }

    /// The pointer in cells of the part of the screen the panes use (`area`), while a command bound
    /// to a mouse key runs; none otherwise.
    pub(super) fn pointer_in(&self, area: Rect) -> Option<(usize, usize)> {
        let ctx = self.mouse_ctx.as_ref()?;
        Some((ctx.at.0.saturating_sub(usize::from(area.x)), ctx.at.1.saturating_sub(usize::from(area.y))))
    }

    /// A press moves the focus to the pane under the pointer, or selects the window whose entry in
    /// the status line it is on.
    fn mouse_select(&mut self, si: usize, spot: &Spot) {
        if let Some((id, _)) = spot.pane {
            let win = self.sessions[si].window_mut();
            if win.active != id {
                win.focus(id);
                self.dirty = true;
            }
        }
        if let Some(index) = spot.tab {
            self.sessions[si].select(index);
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(code: u16, release: bool) -> Mouse {
        Mouse { code, x: 3, y: 4, release }
    }

    fn held(dragged: bool) -> Option<Held> {
        Some(Held { button: 1, loc: MouseLoc::Pane, dragged })
    }

    #[test]
    fn buttons_and_modifiers_come_from_the_code() {
        assert_eq!((button(0), button(1), button(2), button(3)), (Some(1), Some(2), Some(3), None));
        assert_eq!(button(64), None, "the wheel has no button");
        assert_eq!(button(128), None, "nor have the extra ones");
        assert_eq!(modifiers(0), 0);
        assert_eq!(modifiers(4 | 16), SHIFT | CTRL);
        assert_eq!(modifiers(8), ALT);
    }

    #[test]
    fn a_report_is_one_kind_of_event() {
        assert_eq!(kind(&rep(0, false), None, 1), Some(MouseKind::Down));
        assert_eq!(kind(&rep(0, false), None, 2), Some(MouseKind::Double));
        assert_eq!(kind(&rep(2, false), None, 3), Some(MouseKind::Triple));
        assert_eq!(kind(&rep(0, true), held(false), 0), Some(MouseKind::Up));
        assert_eq!(kind(&rep(0, true), held(true), 0), Some(MouseKind::DragEnd));
        assert_eq!(kind(&rep(32, false), held(false), 0), Some(MouseKind::Drag));
        assert_eq!(kind(&rep(64, false), None, 0), Some(MouseKind::WheelUp));
        assert_eq!(kind(&rep(65, false), None, 0), Some(MouseKind::WheelDown));
    }

    #[test]
    fn motion_with_no_button_and_odd_reports_are_nothing() {
        assert_eq!(kind(&rep(35, false), None, 0), None, "the pointer moved, no button");
        assert_eq!(kind(&rep(32, false), None, 0), None, "a drag of a button nobody pressed");
        assert_eq!(kind(&rep(64, true), None, 0), None, "a wheel release");
        assert_eq!(kind(&rep(128, false), None, 1), None, "a button past the third");
        assert_eq!(kind(&rep(3, false), None, 1), None, "a press without a button");
    }

    #[test]
    fn the_word_under_the_pointer() {
        let line = "cd /tmp/some-dir  now";
        assert_eq!(word_at(line, 0), "cd");
        assert_eq!(word_at(line, 4), "/tmp/some-dir");
        assert_eq!(word_at(line, 16), "");
        assert_eq!(word_at(line, 19), "now");
        assert_eq!(word_at(line, 99), "");
    }
}
