//! `focus-events`: which terminal window and which pane has the focus.
//!
//! Two kinds of change. The outer terminal reports that its window gained or lost the keyboard
//! (`client-focus-in`/`client-focus-out`); the report is passed on to the pane in front of it when
//! that program asked for focus reports (mode 1004). And the active pane of a session's current
//! window changes by a key, a click or a command (`pane-focus-out` for the old one,
//! `pane-focus-in` for the new one); the panes that asked are told the same way, so an editor in a
//! pane that is selected notices it, as it does when the terminal window itself is selected.
//!
//! The pane change is found by comparing, once per loop turn, with the pane seen last, so nothing
//! that moves the focus needs to know about this.

use std::collections::HashMap;

use super::hooks::clean;
use super::Server;
use crate::log;
use crate::session::layout::PaneId;

const GAINED: &[u8] = b"\x1b[I";
const LOST: &[u8] = b"\x1b[O";

/// The active pane of the current window of each session, by session id, as last seen.
#[derive(Default)]
pub(super) struct Focus {
    seen: HashMap<usize, PaneId>,
}

impl Server {
    /// Tell a pane its focus changed, when its program asked for that.
    fn tell_pane(&mut self, pane: PaneId, gained: bool) {
        let Some(p) = self.pane_mut(pane) else { return };
        if !p.term.grid.modes.focus_events || p.dead.is_some() || p.copy.is_some() {
            return;
        }
        let report = if gained { GAINED } else { LOST };
        if let Err(e) = p.pty.write(report) {
            log::warn(&format!("pane {pane}: write focus report: {e}"));
        }
    }

    /// The names of what a focus event is about, for the hook variables.
    fn focus_vars(&self, session: usize, pane: PaneId) -> Vec<(&'static str, String)> {
        let mut v = vec![("hook_pane", format!("%{pane}"))];
        if let Some(s) = self.sessions.iter().find(|s| s.id == session) {
            v.push(("hook_session", format!("${}", s.id)));
            v.push(("hook_session_name", clean(&s.name)));
            if let Some(w) = s.windows.iter().find(|w| w.panes.contains_key(&pane)) {
                v.push(("hook_window", format!("@{}", w.id)));
                v.push(("hook_window_name", clean(&w.name)));
            }
        }
        v
    }

    /// The terminal of client `id` gained (`gained`) or lost the keyboard focus.
    pub(super) fn client_focus(&mut self, id: usize, gained: bool) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        if c.focused == gained {
            return;
        }
        c.focused = gained;
        let session = c.session;
        let normal = matches!(c.mode, super::Mode::Normal);
        if let Some(si) = self.client_session(id) {
            let win = self.sessions[si].window();
            let active = win.active;
            if normal {
                self.tell_pane(active, gained);
            }
        }
        if self.hooks.is_empty() {
            return;
        }
        self.number_windows();
        let mut vars = vec![("hook_client", id.to_string())];
        if let Some(s) = self.sessions.iter().find(|s| s.id == session) {
            vars.push(("hook_session", format!("${}", s.id)));
            vars.push(("hook_session_name", clean(&s.name)));
        }
        self.emit(if gained { "client-focus-in" } else { "client-focus-out" }, Some(id), vars);
    }

    /// Look for sessions whose active pane changed since the last loop turn.
    pub(super) fn track_focus(&mut self) {
        let now: Vec<(usize, PaneId)> = self.sessions.iter().map(|s| (s.id, s.window().active)).collect();
        self.focus.seen.retain(|sid, _| now.iter().any(|(id, _)| id == sid));
        let on = self.options.flag("focus-events");
        for (sid, pane) in now {
            let Some(before) = self.focus.seen.insert(sid, pane) else { continue };
            if before == pane || !on {
                continue;
            }
            self.tell_pane(before, false);
            self.tell_pane(pane, true);
            if !self.hooks.is_empty() {
                self.number_windows();
                if self.pane_mut(before).is_some() {
                    let vars = self.focus_vars(sid, before);
                    self.emit("pane-focus-out", None, vars);
                }
                let vars = self.focus_vars(sid, pane);
                self.emit("pane-focus-in", None, vars);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reports_are_the_ones_programs_ask_for() {
        assert_eq!(GAINED, b"\x1b[I");
        assert_eq!(LOST, b"\x1b[O");
    }
}
