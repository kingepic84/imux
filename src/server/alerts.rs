//! Activity and bell monitors: a window nobody looks at that prints (`monitor-activity`) or rings
//! (`monitor-bell`) gets a flag (`#` or `!` in `window_flags`) until someone views it. A window is
//! viewed when it is the current window of a session that has an attached client.
//!
//! The flags live in `Window::alerts` and name the pane that caused them. The `alert-activity` and
//! `alert-bell` hooks are found by `hooks::diff` when a flag appears.

use super::Server;
use crate::session::layout::PaneId;

/// Where a bell is heard, from `bell-action`.
fn rings(action: &str, current: bool) -> bool {
    match action {
        "none" => false,
        "current" => current,
        "other" => !current,
        _ => true,
    }
}

impl Server {
    /// Whether a client looks at window `wi` of session `si`.
    fn viewed(&self, si: usize, wi: usize) -> bool {
        let s = &self.sessions[si];
        s.current == wi && self.clients.values().any(|c| c.session == s.id && c.size.is_some() && !c.closing)
    }

    fn place_of(&self, pane: PaneId) -> Option<(usize, usize)> {
        let si = self.sessions.iter().position(|s| s.window_of(pane).is_some())?;
        Some((si, self.sessions[si].window_of(pane)?))
    }

    /// A pane printed: flag its window unless it is viewed. Called once per read, not per byte.
    pub(super) fn note_activity(&mut self, pane: PaneId) {
        let Some((si, wi)) = self.place_of(pane) else { return };
        let win = &mut self.sessions[si].windows[wi];
        win.last_output = (std::time::Instant::now(), pane);
        if win.alerts.silence.take().is_some() {
            self.dirty = true;
        }
        if self.sessions[si].windows[wi].alerts.activity.is_some()
            || !self.window_flag(si, wi, "monitor-activity")
            || self.viewed(si, wi)
        {
            return;
        }
        self.sessions[si].windows[wi].alerts.activity = Some(pane);
        self.dirty = true;
        if self.options.flag("visual-activity") {
            self.tell_session(si, format!("Activity in window {}", wi + self.base_index()));
        }
    }

    /// Once in a while: flag the windows nobody views that have printed nothing for
    /// `monitor-silence` seconds (`~` in `window_flags`), as `silence-action` allows, and say so
    /// with `visual-silence`. The flag goes when the window prints again or is viewed.
    pub(super) fn check_silence(&mut self) {
        let secs = self.options.num("monitor-silence");
        if secs == 0 {
            return;
        }
        let limit = std::time::Duration::from_secs(secs);
        let action = self.options.text("silence-action").to_string();
        for si in 0..self.sessions.len() {
            for wi in 0..self.sessions[si].windows.len() {
                if self.viewed(si, wi) {
                    let win = &mut self.sessions[si].windows[wi];
                    win.last_output.0 = std::time::Instant::now();
                    continue;
                }
                let win = &self.sessions[si].windows[wi];
                let (at, pane) = win.last_output;
                if win.alerts.silence.is_some() || at.elapsed() < limit || !rings(&action, self.sessions[si].current == wi) {
                    continue;
                }
                self.sessions[si].windows[wi].alerts.silence = Some(pane);
                self.dirty = true;
                if self.options.flag("visual-silence") {
                    self.tell_session(si, format!("Silence in window {}", wi + self.base_index()));
                }
            }
        }
    }

    /// A message on every client that shows session `si`.
    fn tell_session(&mut self, si: usize, text: String) {
        let sid = self.sessions[si].id;
        let mut clients: Vec<usize> = self.clients.iter().filter(|(_, c)| c.session == sid && c.size.is_some() && !c.closing).map(|(id, _)| *id).collect();
        clients.sort_unstable();
        for id in clients {
            self.show_message(id, text.clone());
        }
    }

    /// A pane rang: flag its window unless it is viewed, and say what to do with the bell. `Some`
    /// holds the bytes to send to every client; `None` means nothing is sent (`bell-action`, or
    /// `visual-bell`, which shows a message instead).
    pub(super) fn bell_in(&mut self, pane: PaneId) -> Option<Vec<u8>> {
        let ring = || Some(b"\x07".to_vec());
        let Some((si, wi)) = self.place_of(pane) else { return ring() };
        if self.window_flag(si, wi, "monitor-bell") && !self.viewed(si, wi) && self.sessions[si].windows[wi].alerts.bell.is_none() {
            self.sessions[si].windows[wi].alerts.bell = Some(pane);
            self.dirty = true;
        }
        if !rings(self.options.text("bell-action"), self.sessions[si].current == wi) {
            return None;
        }
        if !self.options.flag("visual-bell") {
            return ring();
        }
        self.tell_session(si, format!("Bell in window {}", wi + self.base_index()));
        None
    }

    /// Once a loop turn: take the flag off windows that are viewed now, and off a monitor that has
    /// been switched off.
    pub(super) fn settle_alerts(&mut self) {
        for si in 0..self.sessions.len() {
            for wi in 0..self.sessions[si].windows.len() {
                let before = self.sessions[si].windows[wi].alerts;
                if !before.any() {
                    continue;
                }
                let mut now = before;
                if self.viewed(si, wi) {
                    now = Default::default();
                } else {
                    if !self.window_flag(si, wi, "monitor-activity") {
                        now.activity = None;
                    }
                    if !self.window_flag(si, wi, "monitor-bell") {
                        now.bell = None;
                    }
                    if self.options.num("monitor-silence") == 0 {
                        now.silence = None;
                    }
                }
                if now != before {
                    self.sessions[si].windows[wi].alerts = now;
                    self.dirty = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rings;

    #[test]
    fn bell_action_picks_which_windows_ring() {
        for (action, current, other) in [("any", true, true), ("none", false, false), ("current", true, false), ("other", false, true)] {
            assert_eq!(rings(action, true), current, "{action} in the current window");
            assert_eq!(rings(action, false), other, "{action} in another window");
        }
    }
}
