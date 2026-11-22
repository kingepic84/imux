//! Playing the animated pictures of the panes (Kitty `a=a`): the loop wakes when the next frame
//! of any picture is due and redraws when one changed.

use std::time::Instant;

use super::Server;

impl Server {
    /// When the next animation frame of any pane is due, for the event loop's poll.
    pub(super) fn animation_deadline(&self) -> Option<Instant> {
        self.sessions.iter().flat_map(|s| s.windows.iter()).flat_map(|w| w.panes.values()).filter_map(|p| p.term.next_frame_due()).min()
    }

    /// Show the frames that are due; the screen is redrawn if any changed.
    pub(super) fn step_animations(&mut self) {
        let now = Instant::now();
        let mut changed = false;
        for pane in self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).flat_map(|w| w.panes.values_mut()) {
            changed |= pane.term.advance_frames(now);
        }
        if changed {
            self.dirty = true;
        }
    }
}
