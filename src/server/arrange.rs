//! Arranging the panes of a window: zoom, break a pane out, swap and rotate panes, the layout
//! presets. All of them change the layout only; `relayout` then gives every pty its new size.

use super::remote::{Place, flags};
use super::{Server, default_window_name};
use crate::session::Window;
use crate::session::layout::{Direction, Layout, PaneId, Preset, Rect, Split};
use crate::target::Kind;

/// Give a window one of the named arrangements and remember it for `next-layout`.
fn apply_preset(win: &mut Window, preset: Preset, area: Rect) {
    match preset {
        Preset::EvenHorizontal => win.layout.make_even(Split::SideBySide),
        Preset::EvenVertical => win.layout.make_even(Split::Stacked),
        other => win.layout.apply(other, area),
    }
    win.preset = Some(preset.index());
}

/// Move dividers until `pane` is `want` cells wide (or high), as near as the layout allows: the
/// divider is pushed one way, and when that made the pane move the wrong way it is pushed back
/// and the other way.
fn resize_to(layout: &mut Layout, pane: PaneId, want: u16, ways: (Direction, Direction), wide: bool, area: Rect) {
    let measure = |layout: &Layout| layout.rects(area).into_iter().find(|(id, _)| *id == pane).map(|(_, r)| if wide { r.w } else { r.h });
    let Some(now) = measure(layout) else { return };
    if now == want {
        return;
    }
    let step = now.abs_diff(want);
    layout.resize(pane, if want > now { ways.0 } else { ways.1 }, step, area);
    let Some(after) = measure(layout) else { return };
    if after.abs_diff(want) > now.abs_diff(want) {
        layout.resize(pane, if want > now { ways.1 } else { ways.0 }, step.saturating_mul(2), area);
    }
}

/// What `select-layout` and `next-layout` ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    /// A named arrangement.
    Preset(Preset),
    /// The one after the last chosen.
    Next,
    /// Keep the structure, share the room out again (`select-layout -E`).
    Spread,
}

/// The preset `next-layout` picks after `current`: the first one when none was chosen yet.
pub fn after(current: Option<usize>) -> Preset {
    Preset::ALL[current.map_or(0, |i| (i + 1) % Preset::ALL.len())]
}

impl Server {
    /// `resize-pane -Z`: show the active pane alone at full size, or bring the others back.
    pub(super) fn toggle_zoom(&mut self, client: usize, si: usize) {
        let win = self.sessions[si].window_mut();
        let active = win.active;
        if win.tiled() < 2 {
            return self.show_message(client, "can't zoom: only one pane".to_string());
        }
        win.layout.toggle_zoom(active);
        self.relayout();
        self.dirty = true;
    }

    /// `break-pane`: the active pane becomes a window of its own, which is selected.
    pub(super) fn break_pane(&mut self, client: usize, si: usize) {
        self.break_out(client, si, false);
    }

    /// `break-pane -d`: the new window is made and the current one stays selected.
    pub(super) fn break_pane_keep(&mut self, client: usize, si: usize) {
        self.break_out(client, si, true);
    }

    fn break_out(&mut self, client: usize, si: usize, keep: bool) {
        let name = default_window_name(&self.shell());
        let session = &mut self.sessions[si];
        let before = session.current;
        let win = session.window_mut();
        if win.tiled() < 2 {
            return self.show_message(client, "can't break pane: only one pane in the window".to_string());
        }
        win.layout.unzoom();
        let id = win.active;
        let Some(pane) = win.remove(id) else { return };
        session.add(Window::new(name, id, pane));
        if keep {
            session.select(before);
        }
        self.relayout();
        self.dirty = true;
    }

    /// `swap-pane -U|-D`: the active pane trades places with the previous or the next one and
    /// stays the active pane.
    pub(super) fn swap_pane(&mut self, client: usize, si: usize, down: bool) {
        let win = self.sessions[si].window_mut();
        let order = win.layout.panes();
        let Some(at) = order.iter().position(|&p| p == win.active) else { return };
        if order.len() < 2 {
            return self.show_message(client, "can't swap: only one pane".to_string());
        }
        let other = order[if down { (at + 1) % order.len() } else { (at + order.len() - 1) % order.len() }];
        win.layout.unzoom();
        let active = win.active;
        win.layout.swap(active, other);
        self.relayout();
        self.dirty = true;
    }

    /// `rotate-window [-D]`: every pane moves to the next place (or, by default, the previous).
    pub(super) fn rotate_window(&mut self, client: usize, si: usize, down: bool) {
        let win = self.sessions[si].window_mut();
        if win.tiled() < 2 {
            return self.show_message(client, "can't rotate: only one pane".to_string());
        }
        win.layout.unzoom();
        win.layout.rotate(down);
        self.relayout();
        self.dirty = true;
    }

    /// The area the panes of session `si` share, for a command that works without a client.
    fn area_of(&self, si: usize) -> Rect {
        let size = self.size_of(si).unwrap_or_else(|| self.default_size());
        self.pane_area(size, self.sessions[si].status_rows)
    }

    /// `select-layout [-En] [-t window] [layout]` as a script gives it: a preset name, `-E`, `-n`, or
    /// a layout string (`#{window_layout}`), whose panes are those of the window in order.
    pub(super) fn cmd_select_layout(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("select-layout", args, "t", "En")?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Window, caller)?;
        let area = self.area_of(place.si);
        let win = &mut self.sessions[place.si].windows[place.wi];
        win.layout.unzoom();
        match (f.has('E'), f.has('n'), f.words.as_slice()) {
            (true, false, []) => win.layout.spread(),
            (false, true, []) => {
                let preset = after(win.preset);
                apply_preset(win, preset, area);
            }
            (false, false, [one]) if crate::session::layout::looks_like_layout(one) => {
                let panes = win.layout.panes();
                win.layout = Layout::from_tmux(one, &panes).map_err(|e| format!("select-layout: {e}"))?;
                win.preset = None;
            }
            (false, false, [one]) => match Preset::parse(one) {
                Some(preset) => apply_preset(win, preset, area),
                None => return Err(format!("select-layout: unknown layout {one}")),
            },
            _ => return Err("usage: select-layout [-En] [-t window] [layout]".to_string()),
        }
        self.relayout();
        self.dirty = true;
        Ok(String::new())
    }

    /// `resize-pane [-t pane] [-LRUD [cells]] [-x width] [-y height] [-Z]`.
    pub(super) fn cmd_resize_pane(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("resize-pane", args, "txy", "LRUDZ")?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        let area = self.area_of(place.si);
        let cells = match f.words.as_slice() {
            [] => 1,
            [n] => n.parse::<u16>().ok().filter(|n| *n >= 1).ok_or_else(|| format!("resize-pane: bad number {n}"))?,
            _ => return Err("usage: resize-pane [-t pane] [-LRUD [cells]] [-x width] [-y height] [-Z]".to_string()),
        };
        let size = |name: char, along: u16| -> Result<Option<u16>, String> {
            let Some(text) = f.value(name) else { return Ok(None) };
            let bad = || format!("resize-pane: bad -{name} {text}");
            match text.strip_suffix('%') {
                Some(p) => Ok(Some(((u32::from(along) * p.parse::<u32>().map_err(|_| bad())?.min(100)) / 100).max(1) as u16)),
                None => text.parse::<u16>().ok().filter(|n| *n >= 1).map(Some).ok_or_else(bad),
            }
        };
        let (want_w, want_h) = (size('x', area.w)?, size('y', area.h)?);
        let win = &mut self.sessions[place.si].windows[place.wi];
        if f.has('Z') {
            win.layout.toggle_zoom(place.pane);
        } else {
            win.layout.unzoom();
            for (flag, dir) in [('L', Direction::Left), ('R', Direction::Right), ('U', Direction::Up), ('D', Direction::Down)] {
                if f.has(flag) {
                    win.layout.resize(place.pane, dir, cells, area);
                }
            }
            for (want, plus, minus, wide) in [(want_w, Direction::Right, Direction::Left, true), (want_h, Direction::Down, Direction::Up, false)] {
                if let Some(want) = want {
                    resize_to(&mut win.layout, place.pane, want, (plus, minus), wide, area);
                }
            }
        }
        self.relayout();
        self.dirty = true;
        Ok(String::new())
    }

    /// `swap-pane [-d] -s pane -t pane`: the two panes trade places, in one window or between two
    /// windows (also of different sessions). Each window keeps its focus on the same place, so the
    /// pane that moved in is the active one where the active pane moved out; `-d` is accepted for
    /// scripts and changes nothing.
    pub(super) fn cmd_swap_pane(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("swap-pane", args, "st", "dZ")?;
        if !f.words.is_empty() {
            return Err("usage: swap-pane [-d] [-s pane] [-t pane]".to_string());
        }
        let a = self.resolve(f.value('s').unwrap_or(""), Kind::Pane, caller)?;
        let b = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        if a.pane == b.pane {
            return Err("swap-pane: the source and the target are the same pane".to_string());
        }
        if (a.si, a.wi) == (b.si, b.wi) {
            let win = &mut self.sessions[a.si].windows[a.wi];
            win.layout.unzoom();
            if !win.layout.swap(a.pane, b.pane) {
                return Err("swap-pane: can't find pane".to_string());
            }
        } else {
            let (pa, pb) = {
                let wa = &mut self.sessions[a.si].windows[a.wi];
                let pa = wa.panes.remove(&a.pane).ok_or("swap-pane: can't find pane")?;
                let wb = &mut self.sessions[b.si].windows[b.wi];
                match wb.panes.remove(&b.pane) {
                    Some(pb) => (pa, pb),
                    None => {
                        self.sessions[a.si].windows[a.wi].panes.insert(a.pane, pa);
                        return Err("swap-pane: can't find pane".to_string());
                    }
                }
            };
            for (place, out, inn, pane) in [(a, a.pane, b.pane, pb), (b, b.pane, a.pane, pa)] {
                self.sessions[place.si].windows[place.wi].exchange(out, inn, pane);
            }
        }
        self.relayout();
        self.dirty = true;
        Ok(String::new())
    }

    /// `move-window` and `swap-window [-d] [-s window] -t window`, with targets as scripts write them
    /// (`session:N`). A move to a number that is free goes to the end or that place; windows
    /// move between sessions, a swap stays inside one.
    pub(super) fn cmd_move_window(&mut self, name: &str, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let swap = name.starts_with("swap");
        let f = flags(name, args, "st", if swap { "d" } else { "dakr" })?;
        if !f.words.is_empty() {
            return Err(format!("usage: {name} [-d] [-s window] -t window"));
        }
        if f.has('r') && f.value('s').is_none() {
            // Windows are positions and always contiguous: there is nothing to renumber.
            return Ok(String::new());
        }
        let (after, kill) = (f.has('a'), f.has('k'));
        let mut from = self.resolve(f.value('s').unwrap_or(""), Kind::Window, caller)?;
        let spec = f.value('t').ok_or_else(|| format!("usage: {name} [-d] [-s window] -t window"))?;
        let select = !f.has('d');
        if swap && let Some(by) = spec.parse::<i32>().ok().filter(|_| spec.starts_with(['-', '+'])) {
            let len = self.sessions[from.si].windows.len() as i32;
            let to = (from.wi as i32 + by).rem_euclid(len) as usize;
            self.sessions[from.si].swap(from.wi, to, select);
            self.relayout();
            self.dirty = true;
            return Ok(String::new());
        }
        let (mut to_si, mut to_pos) = match self.resolve(spec, Kind::Window, caller) {
            Ok(p) => (p.si, p.wi),
            Err(first) => {
                let (session, number) = spec.rsplit_once(':').filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).ok_or(first)?;
                if swap {
                    return Err(format!("{name}: can't find window: {spec}"));
                }
                let si = self.resolve(session, Kind::Session, caller)?.si;
                let n: usize = number.parse().map_err(|_| format!("{name}: bad window number {number}"))?;
                (si, n.saturating_sub(self.base_index()))
            }
        };
        if kill && !swap && !after && (from.si, from.wi) != (to_si, to_pos) && to_pos < self.sessions[to_si].windows.len() {
            // The window in the way goes first; the others are found again by their ids.
            let (from_sid, from_wid) = (self.sessions[from.si].id, self.sessions[from.si].windows[from.wi].id);
            let to_sid = self.sessions[to_si].id;
            let doomed: Vec<PaneId> = self.sessions[to_si].windows[to_pos].panes.keys().copied().collect();
            for id in doomed {
                self.close_pane(id, None);
            }
            let si = self.sessions.iter().position(|s| s.id == from_sid).ok_or_else(|| format!("{name}: the source went with the window"))?;
            let wi = self.sessions[si].windows.iter().position(|w| w.id == from_wid).ok_or_else(|| format!("{name}: the source went with the window"))?;
            from = Place { si, wi, pane: from.pane };
            to_si = self.sessions.iter().position(|s| s.id == to_sid).ok_or_else(|| format!("{name}: the target session went with the window"))?;
            to_pos = to_pos.min(self.sessions[to_si].windows.len());
        }
        if from.si == to_si {
            let session = &mut self.sessions[from.si];
            let to = if after && !swap { if from.wi < to_pos { to_pos } else { to_pos + 1 } } else { to_pos };
            let to = to.min(session.windows.len() - 1);
            let done = if swap { session.swap(from.wi, to, select) } else { session.move_to(from.wi, to, select) };
            if !done {
                return Err(format!("{name}: can't find window"));
            }
        } else {
            if swap {
                return Err(format!("{name}: windows are swapped inside one session"));
            }
            let window = self.sessions[from.si].remove(from.wi).ok_or_else(|| format!("{name}: it is the only window of its session"))?;
            let to_si = if to_si > from.si && self.sessions.get(from.si).is_none() { to_si - 1 } else { to_si };
            self.sessions[to_si].insert_at(if after { to_pos + 1 } else { to_pos }, window, select);
        }
        self.number_windows();
        self.relayout();
        self.dirty = true;
        Ok(String::new())
    }

    /// `select-layout`, `next-layout`.
    pub(super) fn arrange(&mut self, si: usize, how: Arrange) {
        let Some(size) = self.size_of(si) else { return };
        let area = self.pane_area(size, self.sessions[si].status_rows);
        let win = self.sessions[si].window_mut();
        win.layout.unzoom();
        let preset = match how {
            Arrange::Preset(p) => p,
            Arrange::Next => after(win.preset),
            Arrange::Spread => {
                win.layout.spread();
                return self.relayout();
            }
        };
        apply_preset(win, preset, area);
        self.relayout();
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_layout_goes_through_all_presets_and_wraps() {
        assert_eq!(after(None), Preset::EvenHorizontal);
        let mut seen = vec![after(None)];
        for _ in 1..Preset::ALL.len() {
            seen.push(after(seen.last().map(|p| p.index())));
        }
        assert_eq!(seen, Preset::ALL);
        assert_eq!(after(Some(Preset::Tiled.index())), Preset::EvenHorizontal);
    }
}
