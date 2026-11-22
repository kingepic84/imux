//! `display-popup`: a box over the panes of a window that runs a program and takes the keys. The
//! program's pane is kept in `Window::panes` like any pane (so its output, replies and the poll
//! need nothing new) but in no layout, and `Window::popup` says which pane it is. The pure rules
//! (when it closes, its size) are in `session::popup`; where it goes is in `position`.

use std::os::fd::{AsFd, AsRawFd};
use std::path::Path;

use mio::unix::SourceFd;

use super::position::{self, Where};
use super::{Mouse, MOUSE_MOTION, MOUSE_WHEEL, Server, cell_px, hang_up, pane_size};
use crate::config::{format, options};
use crate::input::bindings::PopupSpec;
use crate::input::keys::{CTRL, Code, Key, Parsed};
use crate::log;
use crate::sanitize;
use crate::session::layout::{PaneId, Rect};
use crate::session::popup::{ClosePolicy, CHORD, ENDED_WITH, KeyFate, Popup, key_fate, size_spec, smallest};
use crate::term::grid::MouseMode;
use crate::tty::pty;

/// The program of a popup as the text the shell runs with `-c`: none for an interactive shell,
/// one word as it is, several words quoted for the shell and joined.
fn program(words: &[String], shell: &str) -> Result<Option<String>, String> {
    match words {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        many => {
            let kind = crate::shell::ShellKind::of(shell);
            let quoted: Option<Vec<String>> = many.iter().map(|w| kind.quote(w)).collect();
            match quoted {
                Some(words) => Ok(Some(words.join(" "))),
                None => Err("display-popup: the shell is not known, give the program as one word".to_string()),
            }
        }
    }
}

impl Server {
    /// The place (session and window position) of the popup the client sees: the current window of
    /// its session, when that has one.
    pub(super) fn client_popup(&self, client: usize) -> Option<(usize, usize)> {
        let si = self.client_session(client)?;
        let wi = self.sessions[si].current;
        self.sessions[si].windows[wi].popup.is_some().then_some((si, wi))
    }

    /// Where the popup with this pane is.
    fn popup_place(&self, pane: PaneId) -> Option<(usize, usize)> {
        self.sessions.iter().enumerate().find_map(|(si, s)| {
            s.windows.iter().position(|w| w.popup.as_ref().is_some_and(|p| p.pane == pane)).map(|wi| (si, wi))
        })
    }

    pub(super) fn is_popup_pane(&self, pane: PaneId) -> bool {
        self.popup_place(pane).is_some()
    }

    /// `display-popup` for a client. `waiter` is the connection of a command that waits for the
    /// popup to close (answered then); the popup remembers the pane that command came from, so
    /// its program is treated as a pane's, not as the user's. True when a popup opened.
    pub(super) fn open_popup(&mut self, id: usize, spec: &PopupSpec, waiter: Option<usize>) -> bool {
        let Some(si) = self.client_session(id) else { return false };
        let Some(size) = self.size_of(si) else { return false };
        let place = match spec.target.as_deref() {
            Some(t) => match self.resolve(t, crate::target::Kind::Pane, None) {
                Ok(p) => p,
                Err(e) => {
                    self.show_message(id, e);
                    return false;
                }
            },
            None => self.current_place(si),
        };
        if spec.close {
            if let Some(p) = self.sessions[place.si].windows[place.wi].popup.as_ref().map(|p| p.pane) {
                self.close_popup(p, None);
            }
            return false;
        }
        if self.sessions[place.si].windows[place.wi].popup.is_some() {
            return false;
        }
        let bar_rows = self.sessions[place.si].status_rows;
        let area = self.pane_area(size, bar_rows);
        let (sx, sy) = (usize::from(area.w), usize::from(area.h));
        let lines = if spec.no_border {
            "none".to_string()
        } else {
            spec.lines.clone().unwrap_or_else(|| self.options.text("popup-border-lines").to_string())
        };
        let bordered = options::BORDER_LINES.contains(&lines.as_str()) && lines != "none";
        let (w, h) = match (size_spec(spec.width.as_deref(), sx), size_spec(spec.height.as_deref(), sy)) {
            (Ok(w), Ok(h)) => (w, h),
            (Err(e), _) | (_, Err(e)) => {
                self.show_message(id, format!("display-popup: {e}"));
                return false;
            }
        };
        if w < smallest(bordered) || h < smallest(bordered) {
            return false;
        }
        let vars = self.place_vars(place);
        let (options, pane_env) = (&self.options, &self.pane_env);
        let lookup = |name: &str| super::remote::lookup_one(options, pane_env, &vars, name);
        let win = &self.sessions[place.si].windows[place.wi];
        let pane = win.layout.rects(area).iter().find(|(p, _)| *p == place.pane).map_or((0, 0, sx, sy), |(_, r)| {
            (usize::from(r.x - area.x), usize::from(r.y - area.y), usize::from(r.w), usize::from(r.h))
        });
        let status = (bar_rows > 0).then(|| self.status_top());
        let at = Where { sx, sy, w, h, pane, status, last: self.last_popup };
        let pointer = self.pointer_in(area);
        let (x, y) = match position::place_at(spec.x.as_deref(), spec.y.as_deref(), &at, pointer, &lookup) {
            Ok(p) => p,
            Err(e) => {
                self.show_message(id, format!("display-popup: {e}"));
                return false;
            }
        };
        let title = sanitize::clean(&format::expand(&spec.title, &lookup));
        let style = |own: &Option<String>, option: &str| match own.as_deref().map(options::parse_style) {
            Some(Ok(s)) => s.style(),
            _ => self.options.style(option).style(),
        };
        let styles = (style(&spec.style, "popup-style"), style(&spec.border_style, "popup-border-style"));
        let origin = waiter.and_then(|w| self.pane_of_peer(w));
        let mut popup = Popup {
            pane: 0,
            x,
            y,
            w,
            h,
            lines,
            title,
            styles,
            policy: ClosePolicy::from_flags(spec.exit_flags, spec.any_key),
            origin,
            waiter,
            exit: None,
            chord: 0,
        };
        let (ix, iy, iw, ih) = popup.inner();
        let rect = Rect { x: area.x + ix as u16, y: area.y + iy as u16, w: iw as u16, h: ih as u16 };
        let command = match program(&spec.command, &self.shell()) {
            Ok(c) => c,
            Err(e) => {
                self.show_message(id, e);
                return false;
            }
        };
        let cwd = spec.dir.as_deref().map(Path::new).map(Path::to_path_buf).or_else(|| self.active_cwd(place.si));
        let env: Vec<(String, String)> = spec.env.iter().filter_map(|e| e.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let pane_id = self.alloc_pane_id();
        let spawned = self.spawn_pane_env(pane_id, rect, cell_px(size), cwd.as_deref(), command.as_deref(), &env);
        match spawned {
            Ok(pane) => {
                popup.pane = pane_id;
                let win = &mut self.sessions[place.si].windows[place.wi];
                win.panes.insert(pane_id, pane);
                win.popup = Some(popup);
            }
            Err(e) => {
                log::error(&format!("display-popup: {e}"));
                self.show_message(id, format!("display-popup: {e}"));
                return false;
            }
        }
        self.last_popup = Some((x, y));
        log::info(&format!("popup {pane_id} opened{}", origin.map_or_else(String::new, |o| format!(" by pane {o}"))));
        self.dirty = true;
        true
    }

    /// A key typed while the window has a popup: it goes to the program, unless it closes the
    /// popup. Three presses of the prefix key in a row close it whatever the program does.
    pub(super) fn popup_key(&mut self, client: usize, parsed: &Parsed) {
        let Some((si, wi)) = self.client_popup(client) else { return };
        let is_prefix = self.bindings.is_prefix(&parsed.key);
        let win = &mut self.sessions[si].windows[wi];
        let Some(popup) = win.popup.as_mut() else { return };
        popup.chord = if is_prefix { popup.chord + 1 } else { 0 };
        let pane = popup.pane;
        if popup.chord >= CHORD {
            return self.close_popup(pane, None);
        }
        let cancel = parsed.key == Key::new(Code::Escape, 0) || parsed.key == Key::new(Code::Char('c'), CTRL);
        match key_fate(&popup.policy, popup.exit.is_some(), cancel) {
            KeyFate::Close => self.close_popup(pane, None),
            KeyFate::Drop => {}
            KeyFate::Program => {
                if let Some(p) = win.panes.get(&pane).filter(|p| p.dead.is_none())
                    && let Err(e) = p.pty.write(&parsed.raw)
                {
                    log::warn(&format!("popup {pane}: write to pty: {e}"));
                }
            }
        }
    }

    /// The mouse while the window has a popup: events inside its screen go to the program when it
    /// asked for them, everything else is ignored.
    pub(super) fn popup_mouse(&mut self, client: usize, m: Mouse) {
        let Some((si, wi)) = self.client_popup(client) else { return };
        let Some(size) = self.size_of(si) else { return };
        let area = self.pane_area(size, self.sessions[si].status_rows);
        let win = &self.sessions[si].windows[wi];
        let Some(popup) = win.popup.as_ref() else { return };
        let Some(pane) = win.panes.get(&popup.pane).filter(|p| p.dead.is_none()) else { return };
        let (ix, iy, iw, ih) = popup.inner();
        let (left, top) = (usize::from(area.x) + ix, usize::from(area.y) + iy);
        let (x, y) = (usize::from(m.x), usize::from(m.y));
        if x < left || x >= left + iw || y < top || y >= top + ih {
            return;
        }
        let modes = &pane.term.grid.modes;
        let motion = m.code & MOUSE_MOTION != 0;
        let wanted = match modes.mouse {
            MouseMode::None => false,
            MouseMode::Click => !motion,
            MouseMode::Drag => !motion || m.code & 3 != 3,
            MouseMode::Any => true,
        };
        if !wanted || (m.code & MOUSE_WHEEL != 0 && m.release) {
            return;
        }
        let (rx, ry) = (x - left, y - top);
        let bytes = if modes.mouse_sgr {
            format!("\x1b[<{};{};{}{}", m.code, rx + 1, ry + 1, if m.release { 'm' } else { 'M' }).into_bytes()
        } else {
            let code = if m.release { m.code | 3 } else { m.code };
            match (u8::try_from(code + 32), u8::try_from(rx + 33), u8::try_from(ry + 33)) {
                (Ok(b), Ok(x), Ok(y)) => vec![0x1b, b'[', b'M', b, x, y],
                _ => return,
            }
        };
        if let Err(e) = pane.pty.write(&bytes) {
            log::warn(&format!("popup {}: write mouse to pty: {e}", popup.pane));
        }
    }

    /// The program of a popup ended. The popup closes or stays, by its policy and the status; the
    /// status is not known until the child is collected, so it may wait for the SIGCHLD.
    pub(super) fn popup_exited(&mut self, pane: PaneId, reaped: Option<i32>) {
        let Some((si, wi)) = self.popup_place(pane) else { return };
        let win = &mut self.sessions[si].windows[wi];
        let Some(policy) = win.popup.as_ref().map(|p| p.policy) else { return };
        let Some(p) = win.panes.get_mut(&pane) else { return };
        if p.dead.is_some() {
            return;
        }
        let code = match reaped.map_or_else(|| pty::try_reap(p.pty.child), |c| Ok(Some(c))) {
            Ok(Some(code)) => code,
            Ok(None) => return,
            Err(e) => {
                log::warn(&format!("popup {pane}: collect child: {e}"));
                0
            }
        };
        if policy.closes_on_exit(code) {
            return self.close_popup(pane, Some(code));
        }
        if let Err(e) = self.registry.deregister(&mut SourceFd(&p.pty.master.as_raw_fd())) {
            log::debug(&format!("popup {pane}: deregister pty: {e}"));
        }
        p.dead = Some(code);
        p.agent = None;
        p.typeahead = None;
        if let Some(popup) = win.popup.as_mut() {
            popup.exit = Some(code);
        }
        self.backlog.remove(&pane);
        self.pipes.close(pane, true);
        self.dirty = true;
        log::info(&format!("popup {pane}: program ended with status {code}, kept"));
    }

    /// Take a popup away: its program is hung up, the command waiting for it is answered (an error
    /// when the program ended with a status other than 0).
    pub(super) fn close_popup(&mut self, pane: PaneId, status: Option<i32>) {
        let Some((si, wi)) = self.popup_place(pane) else { return };
        let win = &mut self.sessions[si].windows[wi];
        let Some(popup) = win.popup.take() else { return };
        let Some(p) = win.panes.remove(&pane) else { return };
        if self.mouse_target == Some(pane) {
            self.mouse_target = None;
        }
        if p.dead.is_none()
            && let Err(e) = self.registry.deregister(&mut SourceFd(&p.pty.master.as_raw_fd()))
        {
            log::debug(&format!("popup {pane}: deregister pty: {e}"));
        }
        self.backlog.remove(&pane);
        self.pipes.close(pane, true);
        let exited = p.dead.is_some() || status.is_some() || matches!(pty::try_reap(p.pty.child), Ok(Some(_)));
        if !exited {
            hang_up(p.pty.child);
            self.zombies.push(p.pty.child);
        }
        log::info(&format!("popup {pane} closed"));
        if let Some(waiter) = popup.waiter.filter(|w| self.clients.contains_key(w)) {
            let result = match status.or(popup.exit) {
                Some(code) if code != 0 => Err(format!("{ENDED_WITH}{code}")),
                _ => Ok(String::new()),
            };
            self.finish_request(waiter, result);
        }
        self.dirty = true;
    }

    /// The last tiled pane of a window is going: its popup goes first.
    pub(super) fn close_popup_with_window(&mut self, pane: PaneId) {
        let target = self.sessions.iter().find_map(|s| {
            let wi = s.window_of(pane)?;
            let w = &s.windows[wi];
            (w.tiled() == 1 && w.layout.panes().contains(&pane)).then(|| w.popup.as_ref().map(|p| p.pane)).flatten()
        });
        if let Some(p) = target {
            self.close_popup(p, None);
        }
    }

    /// A connection went away: a popup its command was waiting on goes with it.
    pub(super) fn drop_popup_waiter(&mut self, client: usize) {
        let panes: Vec<PaneId> = self
            .sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .filter_map(|w| w.popup.as_ref().filter(|p| p.waiter == Some(client)).map(|p| p.pane))
            .collect();
        for pane in panes {
            self.close_popup(pane, None);
        }
    }
}

/// After the screen or the layout changed: keep the popup inside the window and its program's
/// screen the size of the box.
pub(super) fn fit_popup(win: &mut crate::session::Window, area: Rect, cell: (u16, u16)) {
    let Some(popup) = win.popup.as_mut() else { return };
    popup.fit(usize::from(area.w), usize::from(area.h));
    let (_, _, iw, ih) = popup.inner();
    let Some(pane) = win.panes.get_mut(&popup.pane) else { return };
    let g = &pane.term.grid;
    if g.cols == iw && g.rows == ih && g.cell_px == cell {
        return;
    }
    let rect = Rect { x: 0, y: 0, w: iw as u16, h: ih as u16 };
    if let Err(e) = pty::set_window_size(pane.pty.master.as_fd(), &pane_size(rect, cell).to_winsize()) {
        log::warn(&format!("popup {}: resize pty: {e}", popup.pane));
    }
    pane.term.grid.resize(iw, ih, cell);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_is_none_one_word_or_quoted_words() {
        assert_eq!(program(&[], "/bin/sh"), Ok(None));
        assert_eq!(program(&["fzf --height 5".to_string()], "/bin/sh"), Ok(Some("fzf --height 5".to_string())));
        let words = ["echo".to_string(), "a b".to_string(), "it's".to_string()];
        let text = program(&words, "/bin/sh").unwrap().unwrap();
        assert!(text.starts_with("echo "), "{text}");
        assert!(text.contains("'a b'") || text.contains("\"a b\"") || text.contains("a\\ b"), "{text}");
        assert!(program(&words, "/usr/bin/unknownsh").is_err());
    }
}
