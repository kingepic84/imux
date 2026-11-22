//! `remain-on-exit`: a pane whose program ended stays on the screen, with what it printed and a
//! line saying so, until it is killed or started again with `respawn-pane`.
//!
//! A dead pane has no program: its child was collected (so its pid is never signalled again, it
//! could belong to somebody else by now) and its pty is out of the poll. Everything that writes to a
//! pane or signals its child checks `Pane::dead`.

use std::os::fd::AsRawFd;

use mio::unix::SourceFd;

use super::Server;
use crate::log;
use crate::session::layout::PaneId;
use crate::tty::pty;

/// Whether a pane that ended with `code` stays, for the value of `remain-on-exit`.
pub(super) fn keeps(mode: &str, code: i32) -> bool {
    match mode {
        "on" => true,
        "failed" => code != 0,
        _ => false,
    }
}

/// The line written into a dead pane.
pub(super) fn notice(code: i32) -> String {
    format!("\r\n\x1b[0m\x1b[7mPane is dead (status {code})\x1b[0m\r\n")
}

impl Server {
    /// The program of a pane ended: its pty closed or its child was collected (`reaped` is the exit
    /// status then). The pane goes, or stays as a dead one.
    pub(super) fn pane_exited(&mut self, id: PaneId, reaped: Option<i32>) {
        if self.is_popup_pane(id) {
            return self.popup_exited(id, reaped);
        }
        let mode = self.options.text("remain-on-exit").to_string();
        if mode == "off" {
            return self.close_pane(id, reaped);
        }
        let Some(pane) = self.sessions.iter_mut().find_map(|s| s.pane_mut(id)) else { return };
        if pane.dead.is_some() {
            return;
        }
        let child = pane.pty.child;
        // The pty usually closes a moment before the program is collected. The status is not known
        // yet then: the pane waits, and the SIGCHLD that follows brings us back with the status. (A
        // program that closes its terminal and keeps running leaves the pane as it is until it ends
        // or the pane is killed.)
        let code = match reaped.map_or_else(|| pty::try_reap(child), |c| Ok(Some(c))) {
            Ok(Some(code)) => code,
            Ok(None) => return,
            Err(e) => {
                log::warn(&format!("pane {id}: collect child: {e}"));
                0
            }
        };
        if !keeps(&mode, code) {
            return self.close_pane(id, Some(code));
        }
        if let Err(e) = self.registry.deregister(&mut SourceFd(&pane.pty.master.as_raw_fd())) {
            log::debug(&format!("pane {id}: deregister pty: {e}"));
        }
        pane.dead = Some(code);
        pane.agent = None;
        pane.typeahead = None;
        pane.term.feed(notice(code).as_bytes());
        self.backlog.remove(&id);
        self.pipes.close(id, true);
        self.dirty = true;
        log::info(&format!("pane {id}: program ended with status {code}, kept (remain-on-exit {mode})"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modes_keep_the_right_panes() {
        assert!(!keeps("off", 1));
        assert!(keeps("on", 0));
        assert!(keeps("on", 3));
        assert!(!keeps("failed", 0));
        assert!(keeps("failed", 1));
        assert!(!keeps("nonsense", 1));
    }

    #[test]
    fn the_notice_names_the_status_and_starts_on_a_fresh_line() {
        let n = notice(130);
        assert!(n.starts_with("\r\n"));
        assert!(n.contains("Pane is dead (status 130)"));
    }
}
