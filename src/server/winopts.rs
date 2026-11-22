//! Options a window can have of its own (`setw` without `-g`), and `synchronize-panes`, the first
//! of them: what is typed into one pane of the window is typed into all of them.

use super::Server;
use crate::input::bindings::WindowOptOp;
use crate::session::layout::PaneId;
use crate::target::Kind;

impl Server {
    /// A window option as the window sees it: its own value, else the global option.
    pub(super) fn window_flag(&self, si: usize, wi: usize, name: &str) -> bool {
        self.sessions[si].windows[wi].opts.get(name).unwrap_or_else(|| self.options.flag(name))
    }

    /// The panes input meant for `first` goes to: `first`, then with `synchronize-panes` every other
    /// pane of its window (in layout order) that is running and not in copy mode.
    pub(super) fn typing_targets(&self, si: usize, wi: usize, first: PaneId) -> Vec<PaneId> {
        let win = &self.sessions[si].windows[wi];
        let mut targets = vec![first];
        if self.window_flag(si, wi, "synchronize-panes") {
            targets.extend(
                win.layout
                    .panes()
                    .into_iter()
                    .filter(|p| *p != first && win.panes.get(p).is_some_and(|pane| pane.dead.is_none() && pane.copy.is_none())),
            );
        }
        targets
    }

    /// `setw name ...` on one window; returns the value in effect afterwards.
    pub(super) fn set_window_option(&mut self, si: usize, wi: usize, name: &str, op: WindowOptOp) -> Result<bool, String> {
        let global = self.options.flag(name);
        let win = &mut self.sessions[si].windows[wi];
        let value = match op {
            WindowOptOp::Set(on) => Some(on),
            WindowOptOp::Unset => None,
            WindowOptOp::Toggle => Some(!win.opts.get(name).unwrap_or(global)),
        };
        if !win.opts.set(name, value) {
            return Err(format!("unknown window option: {name}"));
        }
        self.dirty = true;
        Ok(value.unwrap_or(global))
    }

    /// `setw` from a key or the prompt: the client's window, or the one `-t` names.
    pub(super) fn set_window_option_for(&mut self, client: usize, si: usize, name: &str, op: WindowOptOp, target: Option<&str>) {
        let place = match target {
            None => Ok((si, self.sessions[si].current)),
            Some(t) => self.resolve(t, Kind::Window, None).map(|p| (p.si, p.wi)),
        };
        if let Err(e) = place.and_then(|(si, wi)| self.set_window_option(si, wi, name, op)) {
            self.show_message(client, e);
        }
    }

    /// `setw` in a configuration read at start, where no window exists yet: the global default.
    pub(super) fn set_global_window_option(&mut self, name: &str, op: WindowOptOp) -> Result<(), String> {
        let on = |b: bool| Some(if b { "on" } else { "off" });
        let value = match op {
            WindowOptOp::Set(b) => on(b),
            WindowOptOp::Unset => None,
            WindowOptOp::Toggle => on(!self.options.flag(name)),
        };
        self.set_option(name, value, false)
    }
}
