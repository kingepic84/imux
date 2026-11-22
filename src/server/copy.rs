//! Server side of copy mode: entering and leaving it, the mouse (wheel, drag, double and triple
//! click, edge autoscroll), the paste buffer and clipboard delivery.

use std::io::Write;
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use super::{MOUSE_MOTION, MOUSE_WHEEL, Mouse, Server};
use crate::copymode::{Action, CopyMode, WHEEL_LINES};
use crate::input::keys::Key;
use crate::log;
use crate::session::layout::{PaneId, Rect};

/// Least time between two clipboard writes from the same pane (OSC 52).
const CLIPBOARD_INTERVAL: Duration = Duration::from_secs(1);
/// How often the view scrolls while a drag is held at the edge of the pane.
pub const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(40);
/// Lines per tick when the pointer is far outside the pane.
const MAX_AUTOSCROLL: usize = 5;

/// A drag held at the top or bottom edge of a pane keeps scrolling.
#[derive(Clone, Copy)]
pub struct AutoScroll {
    client: usize,
    pane: PaneId,
    /// -1 up, 1 down.
    dir: isize,
    lines: usize,
    x: usize,
    y: usize,
}

#[derive(Clone, Copy)]
pub struct Click {
    at: Instant,
    pane: PaneId,
    x: u16,
    y: u16,
    count: u8,
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, &b)| acc | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Server {
    pub(super) fn active_in_copy_mode(&self, client: usize) -> bool {
        let Some(si) = self.client_session(client) else { return false };
        let win = self.sessions[si].window();
        win.panes.get(&win.active).is_some_and(|p| p.copy.is_some())
    }

    /// `[`: browse the active pane's history.
    pub(super) fn enter_copy_mode(&mut self, client: usize) {
        let Some(si) = self.client_session(client) else { return };
        let win = self.sessions[si].window_mut();
        let active = win.active;
        self.start_copy(si, active, false);
    }

    fn start_copy(&mut self, si: usize, pane: PaneId, exit_at_bottom: bool) {
        let wi = self.sessions[si].current;
        self.start_copy_in(si, wi, pane, exit_at_bottom, false, false);
    }

    /// Put a pane of any window in copy mode (one already in it is left as it is): `exit_at_bottom`
    /// leaves it on scrolling back to the end, `page_up` starts one page back, `hide_indicator`
    /// leaves out the position at the top right.
    fn start_copy_in(&mut self, si: usize, wi: usize, pane: PaneId, exit_at_bottom: bool, page_up: bool, hide_indicator: bool) {
        let keys = self.bindings.copy_keys(self.options.text("mode-keys") == "vi");
        let separators = self.options.text("word-separators").to_string();
        let Some(p) = self.sessions[si].windows[wi].panes.get_mut(&pane) else { return };
        if p.copy.is_none() {
            let mut m = CopyMode::enter_with(&p.term.grid, keys);
            m.exit_at_bottom = exit_at_bottom;
            m.hide_indicator = hide_indicator;
            m.separators = separators;
            if page_up {
                m.page_up(&p.term.grid);
            }
            p.copy = Some(m);
            self.dirty = true;
        }
    }

    /// `copy-mode [-eHu] [-t pane]` from the socket or a key: copy mode on any pane, which is not
    /// made the active one. `-e` leaves when scrolled back to the end, `-u` starts a page back, `-H`
    /// hides the position indicator.
    pub(super) fn cmd_copy_mode(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = super::remote::flags("copy-mode", args, "t", "eHuq")?;
        if !f.words.is_empty() {
            return Err("usage: copy-mode [-eHu] [-t pane]".to_string());
        }
        let place = self.resolve(f.value('t').unwrap_or(""), crate::target::Kind::Pane, caller)?;
        if self.sessions[place.si].windows[place.wi].panes.get(&place.pane).is_none_or(|p| p.dead.is_some()) {
            return Err("copy-mode: that pane is not running".to_string());
        }
        self.start_copy_in(place.si, place.wi, place.pane, f.has('e'), f.has('u'), f.has('H'));
        Ok(String::new())
    }

    /// A key for the active pane while it is in copy mode.
    pub(super) fn copy_key(&mut self, client: usize, key: Key) {
        let Some(si) = self.client_session(client) else { return };
        let separators = self.options.text("word-separators").to_string();
        let win = self.sessions[si].window_mut();
        let active = win.active;
        let Some(pane) = win.panes.get_mut(&active) else { return };
        let Some(mut m) = pane.copy.take() else { return };
        m.separators = separators;
        let action = m.key(&pane.term.grid, key);
        self.finish_copy_step(si, active, m, action);
    }

    /// Put the mode back unless the action ends it; deliver copied text.
    pub(super) fn finish_copy_step(&mut self, si: usize, pane: PaneId, m: CopyMode, action: Action) {
        self.dirty = true;
        match action {
            Action::None => {
                if let Some(p) = self.sessions[si].window_mut().panes.get_mut(&pane) {
                    p.copy = Some(m);
                }
            }
            Action::Cancel => {}
            Action::Copy(text) => self.set_buffer(text),
        }
    }

    /// Fill the paste buffer and the system clipboard.
    pub(super) fn set_buffer(&mut self, text: String) {
        let command = self.options.text("copy-command").to_string();
        if !command.is_empty() {
            match Process::new("sh").arg("-c").arg(&command).stdin(Stdio::piped()).spawn() {
                Ok(mut child) => {
                    if let Some(Err(e)) = child.stdin.take().map(|mut stdin| stdin.write_all(text.as_bytes())) {
                        log::warn(&format!("write to {command}: {e}"));
                    }
                    self.copiers.push(child);
                }
                Err(e) => log::warn(&format!("start {command}: {e}")),
            }
        }
        if self.options.text("set-clipboard") != "off" {
            let osc = format!("\x1b]52;c;{}\x07", base64(text.as_bytes())).into_bytes();
            let ids: Vec<usize> = self.clients.keys().copied().collect();
            for id in ids {
                self.queue(id, &crate::proto::ServerMsg::Output(osc.clone()));
            }
        }
        let limit = self.buffer_limit();
        self.buffers.push(text, limit);
    }

    /// A program in a pane asked for the clipboard (OSC 52; the request is already checked for
    /// form). Only `set-clipboard on` accepts that, and then only for the terminals that show
    /// the pane and at most once a second per pane, so a pane cannot flood or hijack the
    /// clipboard of a user looking elsewhere.
    pub(super) fn pane_clipboard(&mut self, pane: PaneId, target: &str, data: &str) {
        if self.options.text("set-clipboard") != "on" {
            return;
        }
        let now = Instant::now();
        if self.clip_at.get(&pane).is_some_and(|t| now.duration_since(*t) < CLIPBOARD_INTERVAL) {
            return;
        }
        self.clip_at.insert(pane, now);
        let osc = format!("\x1b]52;{target};{data}\x07").into_bytes();
        let ids: Vec<usize> = self.clients.keys().copied().filter(|&c| self.client_shows(c, pane)).collect();
        log::info(&format!("pane {pane}: clipboard set by the program ({} bytes), {} terminals", data.len(), ids.len()));
        for id in ids {
            self.queue(id, &crate::proto::ServerMsg::Output(osc.clone()));
        }
    }

    /// Whether this client has `pane` on its screen: attached, and the pane is in the window it looks at.
    fn client_shows(&self, client: usize, pane: PaneId) -> bool {
        let Some(c) = self.clients.get(&client) else { return false };
        c.size.is_some()
            && !c.closing
            && self.client_session(client).is_some_and(|si| self.sessions[si].window().panes.contains_key(&pane))
    }

    /// Forget rate limit entries that no longer limit anything.
    pub(super) fn prune_clip_times(&mut self) {
        let now = Instant::now();
        self.clip_at.retain(|_, t| now.duration_since(*t) < CLIPBOARD_INTERVAL);
    }

    /// Wait for finished clipboard helpers so they do not linger as zombies.
    pub(super) fn reap_copiers(&mut self) {
        self.copiers.retain_mut(|c| match c.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if !status.success() {
                    log::warn(&format!("copy-command exited with {status}"));
                }
                false
            }
            Err(e) => {
                log::warn(&format!("wait for copy-command: {e}"));
                false
            }
        });
    }

    /// Mouse event for a pane that imux, not its program, handles: the wheel scrolls history, a
    /// drag selects and copies, double and triple click copy a word or a line.
    pub(super) fn copy_mouse(&mut self, client: usize, si: usize, pane: PaneId, rect: Rect, m: Mouse) {
        let rx = m.x.saturating_sub(rect.x).min(rect.w.saturating_sub(1));
        let ry = m.y.saturating_sub(rect.y).min(rect.h.saturating_sub(1));
        let (rx, ry) = (usize::from(rx), usize::from(ry));
        if m.code & MOUSE_WHEEL != 0 {
            if !m.release {
                self.wheel(si, pane, m.code & 1 == 0);
            }
            return;
        }
        match m.code & 3 {
            1 if !m.release && m.code & MOUSE_MOTION == 0 => return self.paste_buffer(client),
            0 => {}
            _ => return,
        }
        if m.release {
            self.autoscroll = None;
            let pending = self.press.take();
            let Some(p) = self.sessions[si].window_mut().panes.get_mut(&pane) else { return };
            let Some(mut mode) = p.copy.take() else { return };
            let action = if pending.is_some() { mode.mouse_up(&p.term.grid) } else { Action::None };
            if matches!(action, Action::None) {
                p.copy = Some(mode);
                return;
            }
            return self.finish_copy_step(si, pane, mode, action);
        }
        if m.code & MOUSE_MOTION != 0 {
            return self.drag(si, pane, rect, m, (rx, ry));
        }
        self.press_at(si, pane, m, (rx, ry));
    }

    fn wheel(&mut self, si: usize, pane: PaneId, up: bool) {
        if up {
            self.start_copy(si, pane, true);
        }
        let Some(p) = self.sessions[si].window_mut().panes.get_mut(&pane) else { return };
        let Some(mut mode) = p.copy.take() else { return };
        let lines = WHEEL_LINES as isize;
        mode.scroll(&p.term.grid, if up { -lines } else { lines });
        let leave = !up && mode.exit_at_bottom && mode.at_bottom(&p.term.grid);
        self.dirty = true;
        if leave {
            return;
        }
        p.copy = Some(mode);
    }

    fn press_at(&mut self, si: usize, pane: PaneId, m: Mouse, (rx, ry): (usize, usize)) {
        let now = Instant::now();
        let count = match self.last_click {
            Some(c) if c.pane == pane && c.x == m.x && c.y == m.y && now.duration_since(c.at) <= Duration::from_millis(self.options.num("double-click-time")) => {
                (c.count % 3) + 1
            }
            _ => 1,
        };
        self.last_click = Some(Click { at: now, pane, x: m.x, y: m.y, count });
        self.press = Some((pane, rx, ry));
        if count == 1 {
            let Some(p) = self.sessions[si].window_mut().panes.get_mut(&pane) else { return };
            if let Some(mode) = p.copy.as_mut() {
                let pos = mode.pos_at(&p.term.grid, rx, ry);
                mode.mouse_down(&p.term.grid, pos);
                self.dirty = true;
            }
            return;
        }
        self.start_copy(si, pane, false);
        let separators = self.options.text("word-separators").to_string();
        let Some(p) = self.sessions[si].window_mut().panes.get_mut(&pane) else { return };
        let Some(mut mode) = p.copy.take() else { return };
        mode.separators = separators;
        let pos = mode.pos_at(&p.term.grid, rx, ry);
        if count == 2 {
            mode.select_word(&p.term.grid, pos);
        } else {
            mode.select_line(&p.term.grid, pos);
        }
        let action = mode.copy_selection(&p.term.grid);
        self.press = None;
        self.finish_copy_step(si, pane, mode, action);
    }

    fn drag(&mut self, si: usize, pane: PaneId, rect: Rect, m: Mouse, (rx, ry): (usize, usize)) {
        let Some((press_pane, px, py)) = self.press else { return };
        if press_pane != pane {
            return;
        }
        if self.sessions[si].window().panes.get(&pane).is_some_and(|p| p.copy.is_none()) {
            if (px, py) == (rx, ry) {
                return;
            }
            self.start_copy(si, pane, false);
            let Some(p) = self.sessions[si].window_mut().panes.get_mut(&pane) else { return };
            if let Some(mode) = p.copy.as_mut() {
                let pos = mode.pos_at(&p.term.grid, px, py);
                mode.mouse_down(&p.term.grid, pos);
            }
        }
        let Some(p) = self.sessions[si].window_mut().panes.get_mut(&pane) else { return };
        if let Some(mode) = p.copy.as_mut() {
            let pos = mode.pos_at(&p.term.grid, rx, ry);
            mode.mouse_drag(&p.term.grid, pos);
        }
        self.dirty = true;
        let (top, bottom) = (rect.y, rect.y + rect.h.saturating_sub(1));
        let client = self.clients.iter().find(|(_, c)| self.sessions.get(si).is_some_and(|s| s.id == c.session)).map(|(id, _)| *id);
        self.autoscroll = match client {
            Some(client) if m.y <= top => {
                Some(AutoScroll { client, pane, dir: -1, lines: (1 + usize::from(top - m.y)).min(MAX_AUTOSCROLL), x: rx, y: 0 })
            }
            Some(client) if m.y >= bottom => Some(AutoScroll {
                client,
                pane,
                dir: 1,
                lines: (1 + usize::from(m.y - bottom)).min(MAX_AUTOSCROLL),
                x: rx,
                y: usize::from(rect.h.saturating_sub(1)),
            }),
            _ => None,
        };
    }

    /// When the next autoscroll step is due.
    pub(super) fn autoscroll_due(&self) -> Option<Instant> {
        self.autoscroll.map(|_| self.autoscroll_tick + AUTOSCROLL_INTERVAL)
    }

    /// One autoscroll step: scroll and drag the selection end along with the edge.
    pub(super) fn autoscroll_step(&mut self) {
        self.autoscroll_tick = Instant::now();
        let Some(a) = self.autoscroll else { return };
        let Some(si) = self.client_session(a.client) else {
            self.autoscroll = None;
            return;
        };
        let Some(p) = self.sessions[si].window_mut().panes.get_mut(&a.pane) else {
            self.autoscroll = None;
            return;
        };
        let Some(mode) = p.copy.as_mut() else {
            self.autoscroll = None;
            return;
        };
        mode.scroll(&p.term.grid, a.dir * a.lines as isize);
        let pos = mode.pos_at(&p.term.grid, a.x, a.y);
        mode.mouse_drag(&p.term.grid, pos);
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_rfc_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
