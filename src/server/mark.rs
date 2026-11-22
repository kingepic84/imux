//! The marked pane (`select-pane -m`): one pane of the server that `swap-pane` without a
//! direction trades places with. Its window shows the flag `M` in the status line.

use super::Server;
use crate::session::layout::PaneId;

impl Server {
    /// The marked pane, as long as some window still holds it.
    pub(super) fn marked_pane(&self) -> Option<PaneId> {
        let m = self.marked?;
        self.sessions.iter().any(|s| s.windows.iter().any(|w| w.panes.contains_key(&m))).then_some(m)
    }

    /// `m` marks the active pane (or unmarks it when it is the marked one), `M` clears the mark.
    pub(super) fn mark_pane(&mut self, client: usize, si: usize, set: bool) {
        let active = self.sessions[si].window().active;
        let unmark = !set || self.marked_pane() == Some(active);
        self.marked = if unmark { None } else { Some(active) };
        self.dirty = true;
        let text = if self.marked.is_some() { format!("marked pane %{active}") } else { "mark cleared".to_string() };
        self.show_message(client, text);
    }

    /// `swap-pane`: the active pane and the marked pane of the same window trade places; the
    /// active pane stays the active one.
    pub(super) fn swap_with_marked(&mut self, client: usize, si: usize) {
        let Some(marked) = self.marked_pane() else { return self.show_message(client, "no marked pane".to_string()) };
        let win = self.sessions[si].window_mut();
        if !win.panes.contains_key(&marked) {
            return self.show_message(client, "the marked pane is in another window".to_string());
        }
        if marked == win.active {
            return self.show_message(client, "the marked pane is the active one".to_string());
        }
        win.layout.unzoom();
        let active = win.active;
        win.layout.swap(active, marked);
        self.relayout();
        self.dirty = true;
    }
}
