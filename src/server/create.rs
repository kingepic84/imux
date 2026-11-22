//! Making sessions, windows and panes by command, with targets and no client needed:
//! `new-session -d`, `new-window`, `split-window` as scripts call them.

use std::path::PathBuf;

use super::remote::{NEW_WINDOW_FLAGS, Place, SPLIT_WINDOW_FLAGS, flags};
use super::{Server, cell_px, default_window_name};
use crate::errcode::tag;
use crate::proto::Size;
use crate::session::layout::{PaneId, Split};
use crate::session::{Session, Window};
use crate::target::Kind;

/// The size asked for the new pane of `split-window`: cells (`-l 20`) or a share (`-l 30%`, `-p 30`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Want {
    Cells(u16),
    Percent(u16),
}

impl Want {
    /// Cells for the new pane out of a pane `along` cells long: a share counts the cells that can
    /// be divided (the divider takes one) and at least one cell goes to each half.
    fn cells(self, along: u16) -> u16 {
        let room = along.saturating_sub(1);
        let n = match self {
            Want::Cells(n) => n,
            Want::Percent(p) => (u32::from(room) * u32::from(p) / 100) as u16,
        };
        n.clamp(1, room.saturating_sub(1).max(1))
    }
}

fn parse_size(text: &str, flag: &str) -> Result<Want, String> {
    let bad = || format!("split-window: bad size for {flag}: {text}");
    match text.strip_suffix('%') {
        Some(p) => p.parse::<u16>().ok().filter(|p| (1..=99).contains(p)).map(Want::Percent).ok_or_else(bad),
        None => text.parse::<u16>().ok().filter(|n| *n >= 1).map(Want::Cells).ok_or_else(bad),
    }
}

/// Most `-e NAME=VALUE` pairs of one command, and the longest value.
const MAX_ENV_PAIRS: usize = 32;
const MAX_ENV_VALUE: usize = 4096;

/// The `-e NAME=VALUE` flags of `new-window` and `split-window`: a variable-shaped name, no NUL in
/// the value, and no `IMUX_` names (imux sets those itself).
/// The session part of a `new-window -t` target and the window number asked for: `-t work:3` is
/// session `work`, number `3` (plugins restoring a layout say so); anything else is a session.
pub(super) fn new_window_target(target: &str) -> (&str, Option<&str>) {
    match target.rsplit_once(':') {
        Some((s, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => (s, Some(n)),
        _ => (target, None),
    }
}

fn env_pairs(name: &str, f: &super::remote::Flags) -> Result<Vec<(String, String)>, String> {
    let given = f.values_of('e');
    if given.len() > MAX_ENV_PAIRS {
        return Err(format!("{name}: at most {MAX_ENV_PAIRS} -e values"));
    }
    given
        .into_iter()
        .map(|pair| {
            let (key, value) = pair.split_once('=').ok_or_else(|| format!("{name}: -e takes NAME=VALUE, not {pair:?}"))?;
            let shaped = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if !shaped || key.len() > 64 {
                return Err(format!("{name}: bad variable name {key:?}"));
            }
            if key.starts_with("IMUX_") {
                return Err(format!("{name}: {key} is set by imux"));
            }
            if value.len() > MAX_ENV_VALUE || value.contains('\0') {
                return Err(format!("{name}: bad value for {key}"));
            }
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

/// The size of a screen nobody is looking at yet when `default-size` says nothing usable.
const DETACHED_SIZE: Size = Size { cols: 80, rows: 24, xpix: 0, ypix: 0 };

/// Largest width or height `default-size` and `new-session -x -y` take, and the smallest.
const SIZE_RANGE: std::ops::RangeInclusive<u16> = 10..=1000;

/// `WIDTHxHEIGHT` (or `WIDTH*HEIGHT`) in cells, each within `SIZE_RANGE`.
pub(super) fn parse_screen_size(text: &str) -> Option<Size> {
    let (w, h) = text.trim().split_once(['x', 'X', '*'])?;
    let (cols, rows) = (w.trim().parse::<u16>().ok()?, h.trim().parse::<u16>().ok()?);
    (SIZE_RANGE.contains(&cols) && SIZE_RANGE.contains(&rows)).then_some(Size { cols, rows, xpix: 0, ypix: 0 })
}

/// A size given as `-x` and `-y` words of a command; one alone keeps the other from `base`.
pub(super) fn size_from_flags(x: Option<&str>, y: Option<&str>, base: Size) -> Result<Size, String> {
    let part = |what: &str, v: Option<&str>, keep: u16| -> Result<u16, String> {
        let Some(v) = v else { return Ok(keep) };
        v.trim().parse::<u16>().ok().filter(|n| SIZE_RANGE.contains(n)).ok_or_else(|| format!("bad {what}: {v:?} (cells, {} to {})", SIZE_RANGE.start(), SIZE_RANGE.end()))
    };
    Ok(Size { cols: part("width", x, base.cols)?, rows: part("height", y, base.rows)?, xpix: 0, ypix: 0 })
}

impl Server {
    /// The size of a screen nobody is looking at yet, until a client attaches and sets the real one:
    /// `default-size`, else 80x24.
    pub(super) fn default_size(&self) -> Size {
        parse_screen_size(self.options.text("default-size")).unwrap_or(DETACHED_SIZE)
    }

    fn size_or_default(&mut self) -> Size {
        let default = self.default_size();
        *self.size.get_or_insert(default)
    }

    /// The words of a command as one line for `shell -c`: one word is the line as it is (the
    /// caller quoted it), several are quoted for the shell.
    pub(super) fn command_line(&self, words: &[String]) -> Result<Option<String>, String> {
        match words {
            [] => Ok(None),
            [one] => Ok(Some(one.clone())),
            many => {
                let kind = self.shell_kind();
                let quoted: Option<Vec<String>> = many.iter().map(|w| crate::shell::ShellKind::quote(kind, w)).collect();
                Ok(Some(quoted.ok_or("the shell is not one imux knows how to quote for: give the command as one word")?.join(" ")))
            }
        }
    }

    fn directory(&self, given: Option<&str>, fallback: Option<&str>) -> Result<Option<PathBuf>, String> {
        let Some(dir) = given.or(fallback) else { return Ok(None) };
        let path = PathBuf::from(dir);
        if given.is_some() && !path.is_dir() {
            return Err(format!("not a directory: {dir}"));
        }
        Ok(Some(path))
    }

    /// `respawn-pane [-k] [-c dir] [-t pane] [command]` and `respawn-window [-k] [-c dir] [-t window]
    /// [command]` (for a window of one pane): start the shell or the command again in the pane,
    /// which keeps its place, size and number. The program in it is hung up. A pane never lingers
    /// after its program ends, so `-k` is always needed.
    pub(super) fn cmd_respawn(&mut self, args: &[String], caller: Option<PaneId>, window: bool) -> Result<String, String> {
        use std::os::fd::{AsFd, AsRawFd};
        let name = if window { "respawn-window" } else { "respawn-pane" };
        let f = flags(name, args, "tc", "k")?;
        let kind = if window { Kind::Window } else { Kind::Pane };
        let place = self.resolve(f.value('t').unwrap_or(""), kind, caller)?;
        if window && self.sessions[place.si].windows[place.wi].tiled() != 1 {
            return Err(format!("{name}: the window has several panes, use respawn-pane"));
        }
        let dead = self.sessions[place.si].windows[place.wi].panes.get(&place.pane).is_some_and(|p| p.dead.is_some());
        if !f.has('k') && !dead {
            return Err(format!("{name}: pane %{} is still running; add -k to kill it", place.pane));
        }
        let command = self.command_line(&f.words)?;
        let fallback = self.sessions[place.si].windows[place.wi].panes.get(&place.pane).and_then(|p| {
            let pid = crate::tty::proc::foreground_pid(p.pty.master.as_fd()).unwrap_or(p.pty.child.as_raw());
            crate::tty::proc::cwd(pid)
        });
        let dir = self.directory(f.value('c'), fallback.as_deref().and_then(|p| p.to_str()))?;
        let size = self.size_of(place.si).unwrap_or_else(|| self.default_size());
        let area = self.pane_area(size, self.sessions[place.si].status_rows);
        let rect = self.sessions[place.si].windows[place.wi]
            .layout
            .rects(area)
            .into_iter()
            .find(|(id, _)| *id == place.pane)
            .map(|(_, r)| r)
            .ok_or_else(|| format!("{name}: the pane has no room"))?;
        // The old pty leaves the poll before the new one is registered under the same number.
        let old = self.sessions[place.si].windows[place.wi].panes.remove(&place.pane).ok_or("can't find pane")?;
        if let Err(e) = self.registry.deregister(&mut mio::unix::SourceFd(&old.pty.master.as_raw_fd())) {
            crate::log::debug(&format!("pane {}: deregister pty: {e}", place.pane));
        }
        self.backlog.remove(&place.pane);
        match self.spawn_pane_with(place.pane, rect, cell_px(size), dir.as_deref(), command.as_deref()) {
            Ok(pane) => {
                self.sessions[place.si].windows[place.wi].panes.insert(place.pane, pane);
                // A dead pane's program ended and was collected: there is nothing to stop.
                if old.dead.is_none() {
                    super::hang_up(old.pty.child);
                    self.zombies.push(old.pty.child);
                }
                self.dirty = true;
                Ok(String::new())
            }
            Err(e) => {
                // Put the old pane back: it keeps running, nothing is lost.
                if old.dead.is_none()
                    && let Err(re) = self.registry.register(
                        &mut mio::unix::SourceFd(&old.pty.master.as_raw_fd()),
                        mio::Token(super::PANE_BASE + place.pane as usize),
                        mio::Interest::READABLE,
                    )
                {
                    crate::log::error(&format!("pane {}: register pty again: {re}", place.pane));
                }
                self.sessions[place.si].windows[place.wi].panes.insert(place.pane, old);
                Err(format!("{name}: {e}"))
            }
        }
    }

    /// `join-pane` and `move-pane [-hvd] [-l size] [-s pane] [-t pane]`: take a pane out of its
    /// window (a window left without panes goes) and split it into the target pane's window, where
    /// it becomes the second half and, without `-d`, the active pane.
    pub(super) fn cmd_join_pane(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("join-pane", args, "stl", "hvdbf")?;
        if !f.words.is_empty() {
            return Err("usage: join-pane [-hvdbf] [-l size] [-s pane] [-t pane]".to_string());
        }
        let (full, before_target) = (f.has('f'), f.has('b'));
        let want = f.value('l').map(|l| parse_size(l, "-l")).transpose()?;
        let src = self.resolve(f.value('s').unwrap_or(""), Kind::Pane, caller)?;
        let dst = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        if src.pane == dst.pane {
            return Err("join-pane: the source and the target are the same pane".to_string());
        }
        if (src.si, src.wi) == (dst.si, dst.wi) {
            return Err("join-pane: the pane is in that window already".to_string());
        }
        let kind = if f.has('h') { Split::SideBySide } else { Split::Stacked };
        self.size_or_default();
        let size = self.size_of(dst.si).unwrap_or_else(|| self.default_size());
        let area = self.pane_area(size, self.sessions[dst.si].status_rows);
        let dst_win = &self.sessions[dst.si].windows[dst.wi];
        let along = |r: crate::session::layout::Rect| if kind == Split::SideBySide { r.w } else { r.h };
        let room = if full { along(area) } else { dst_win.layout.rects(area).into_iter().find(|(id, _)| *id == dst.pane).map_or(0, |(_, r)| along(r)) };
        if room < 3 {
            return Err("join-pane: the target pane is too small".to_string());
        }
        let alone = self.sessions[src.si].windows[src.wi].tiled() == 1;
        if alone && self.sessions[src.si].windows.len() < 2 {
            return Err("join-pane: that is the only pane of the only window of its session".to_string());
        }
        let pane = if alone {
            let window = self.sessions[src.si].remove(src.wi).ok_or("join-pane: can't take the window out of its session")?;
            let mut window = window;
            window.panes.remove(&src.pane).ok_or("can't find pane")?
        } else {
            let win = &mut self.sessions[src.si].windows[src.wi];
            win.layout.unzoom();
            win.remove(src.pane).ok_or("can't find pane")?
        };
        // Taking a window out can shift the numbers of the ones after it.
        let dst = self.place_of_pane(dst.pane).ok_or("can't find pane")?;
        let win = &mut self.sessions[dst.si].windows[dst.wi];
        win.layout.unzoom();
        let split = if full { win.layout.split_root(kind, src.pane, before_target, area) } else { win.layout.split(dst.pane, kind, src.pane, area) };
        if let Err(e) = split {
            let name = default_window_name(&self.shell());
            self.sessions[src.si].add(Window::new(name, src.pane, pane));
            self.number_windows();
            return Err(format!("join-pane: {e}"));
        }
        if before_target && !full {
            win.layout.swap(dst.pane, src.pane);
        }
        if let Some(want) = want {
            let n = want.cells(room);
            if !before_target {
                win.layout.size_second(src.pane, kind, n, area);
            } else {
                win.layout.size_first(src.pane, kind, n, area);
            }
        }
        let before = win.active;
        win.panes.insert(src.pane, pane);
        win.focus(src.pane);
        if f.has('d') {
            win.focus(before);
        }
        self.number_windows();
        self.relayout();
        self.dirty = true;
        Ok(String::new())
    }

    fn created(&self, p: Place, f: &super::remote::Flags, default: &str) -> String {
        if f.has('P') { self.expand_at(p, f.value('F').unwrap_or(default)) + "\n" } else { String::new() }
    }

    /// `new-session -d [-s name] [-n window] [-c dir] [-P [-F format]] [command]`
    pub(super) fn cmd_new_session(&mut self, args: &[String], cwd: Option<&str>) -> Result<String, String> {
        let f = flags("new-session", args, "snFcxy", "dP")?;
        if !f.has('d') {
            return Err("new-session: this only creates the session: add -d (to attach, run `imux new -s name`)".to_string());
        }
        let name = f.value('s').map(str::to_string);
        if let Some(n) = &name
            && (n.is_empty() || n.chars().any(char::is_control) || self.sessions.iter().any(|s| &s.name == n))
        {
            return Err(if n.is_empty() || n.chars().any(char::is_control) { format!("new-session: bad name {n:?}") } else { format!("duplicate session: {n}") });
        }
        let command = self.command_line(&f.words)?;
        let dir = self.directory(f.value('c'), cwd)?;
        self.size_or_default();
        let size = size_from_flags(f.value('x'), f.value('y'), self.default_size()).map_err(|e| format!("new-session: {e}"))?;
        let id = self.alloc_pane_id();
        let area = self.pane_area(size, self.first_status_rows());
        let pane = self.spawn_pane_with(id, area, cell_px(size), dir.as_deref(), command.as_deref()).map_err(|e| format!("new-session: {e}"))?;
        let mut window = Window::new(f.value('n').map_or_else(|| default_window_name(&self.shell()), str::to_string), id, pane);
        window.auto_rename = f.value('n').is_none();
        let sid = self.next_session;
        self.next_session += 1;
        let name = name.unwrap_or_else(|| self.free_session_name());
        crate::log::info(&format!("session {sid} ({name}) created by command"));
        self.sessions.push(Session::new(sid, name, window));
        self.session_sizes.insert(sid, size);
        self.dirty = true;
        let si = self.sessions.len() - 1;
        self.number_windows();
        let place = self.current_place(si);
        Ok(self.created(place, &f, "#{session_name}:"))
    }

    /// `new-window [-t session] [-n name] [-c dir] [-d] [-P [-F format]] [command]`
    pub(super) fn cmd_new_window(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("new-window", args, NEW_WINDOW_FLAGS.0, NEW_WINDOW_FLAGS.1)?;
        // `-t session:N` asks for the window to be number N (plugins restoring a layout say so);
        // numbers have no gaps, so N must be the next one or a number that is not taken.
        let (session_part, want) = new_window_target(f.value('t').unwrap_or(""));
        let want_index = match want {
            Some(n) => Some(n.parse::<usize>().map_err(|_| format!("new-window: bad window number {n}"))?),
            None => None,
        };
        let si = self.resolve(session_part, Kind::Session, caller)?.si;
        if let Some(n) = want_index {
            let pos = n.checked_sub(self.base_index()).ok_or_else(|| format!("new-window: window number {n} is below base-index"))?;
            if pos < self.sessions[si].windows.len() && !f.has('a') {
                return Err(format!("new-window: index {n} in use"));
            }
        }
        let env = env_pairs("new-window", &f)?;
        let command = self.command_line(&f.words)?;
        let fallback = self.active_cwd(si).map(|p| p.to_string_lossy().into_owned());
        let dir = self.directory(f.value('c'), fallback.as_deref())?;
        self.size_or_default();
        let size = self.size_of(si).unwrap_or_else(|| self.default_size());
        let id = self.alloc_pane_id();
        let area = self.pane_area(size, self.sessions[si].status_rows);
        let pane = self.spawn_pane_env(id, self.content_rect(area), cell_px(size), dir.as_deref(), command.as_deref(), &env).map_err(|e| format!("new-window: {e}"))?;
        let mut window = Window::new(f.value('n').map_or_else(|| default_window_name(&self.shell()), str::to_string), id, pane);
        window.auto_rename = f.value('n').is_none();
        let before = self.sessions[si].current;
        if f.has('a') {
            self.sessions[si].add_after_current(window);
        } else {
            self.sessions[si].add(window);
        }
        let wi = if f.has('a') { before + 1 } else { self.sessions[si].windows.len() - 1 };
        if f.has('d') {
            self.sessions[si].select(before);
        }
        self.number_windows();
        self.relayout();
        self.dirty = true;
        if let Some(by) = caller {
            self.record_spawn(id, by);
        }
        Ok(self.created(Place { si, wi, pane: id }, &f, "#{session_name}:#{window_index}"))
    }

    /// `split-window [-t pane] [-h|-v] [-c dir] [-d] [-P [-F format]] [command]`
    pub(super) fn cmd_split_window(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("split-window", args, SPLIT_WINDOW_FLAGS.0, SPLIT_WINDOW_FLAGS.1)?;
        let want = match (f.value('l'), f.value('p')) {
            (Some(_), Some(_)) => return Err("split-window: use -l or -p, not both".to_string()),
            (Some(l), None) => Some(parse_size(l, "-l")?),
            (None, Some(p)) => Some(parse_size(&format!("{}%", p.trim_end_matches('%')), "-p")?),
            (None, None) => None,
        };
        let at = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        let kind = if f.has('h') { Split::SideBySide } else { Split::Stacked };
        let env = env_pairs("split-window", &f)?;
        let command = self.command_line(&f.words)?;
        let fallback = {
            let win = &self.sessions[at.si].windows[at.wi];
            win.panes.get(&at.pane).and_then(|p| {
                use std::os::fd::AsFd;
                let pid = crate::tty::proc::foreground_pid(p.pty.master.as_fd()).unwrap_or(p.pty.child.as_raw());
                crate::tty::proc::cwd(pid)
            })
        };
        let dir = self.directory(f.value('c'), fallback.as_deref().and_then(|p| p.to_str()))?;
        self.size_or_default();
        let size = self.size_of(at.si).unwrap_or_else(|| self.default_size());
        let area = self.pane_area(size, self.sessions[at.si].status_rows);
        let new = self.alloc_pane_id();
        let (min_width, min_height) = (self.options.num("pane-min-width"), self.options.num("pane-min-height"));
        let win = &mut self.sessions[at.si].windows[at.wi];
        win.layout.unzoom();
        let full = f.has('f');
        let old_along = if full {
            if kind == Split::SideBySide { area.w } else { area.h }
        } else {
            win.layout.rects(area).into_iter().find(|(id, _)| *id == at.pane).map_or(0, |(_, r)| if kind == Split::SideBySide { r.w } else { r.h })
        };
        if full {
            win.layout.split_root(kind, new, f.has('b'), area).map_err(|e| format!("split-window: {e}"))?;
        } else {
            win.layout.split(at.pane, kind, new, area).map_err(|e| format!("split-window: {e}"))?;
            if f.has('b') {
                win.layout.swap(at.pane, new);
            }
        }
        if let Some(want) = want {
            let n = want.cells(old_along);
            if f.has('b') && !full {
                win.layout.size_second(at.pane, kind, old_along.saturating_sub(1).saturating_sub(n).max(1), area);
            } else if f.has('b') {
                win.layout.size_first(new, kind, n, area);
            } else {
                win.layout.size_second(new, kind, n, area);
            }
        }
        let rects = win.layout.rects(area);
        let rect = rects.iter().find(|(id, _)| *id == new).map(|(_, r)| *r);
        // A program in a pane may not cut panes into slivers: both halves keep the minimum size.
        let too_small = caller.is_some()
            && [new, at.pane].iter().any(|id| rects.iter().any(|(rid, r)| rid == id && (u64::from(r.w) < min_width || u64::from(r.h) < min_height)));
        if too_small {
            win.layout.remove(new);
            return Err(tag(
                "limit_reached",
                &format!("split-window: the panes would be under {min_width} columns or {min_height} rows (pane-min-width, pane-min-height)"),
            ));
        }
        let spawned = match rect {
            Some(rect) => self.spawn_pane_env(new, self.content_rect(rect), cell_px(size), dir.as_deref(), command.as_deref(), &env),
            None => Err(std::io::Error::other("the new pane has no room")),
        };
        let before = self.sessions[at.si].windows[at.wi].active;
        let win = &mut self.sessions[at.si].windows[at.wi];
        match spawned {
            Ok(pane) => {
                win.panes.insert(new, pane);
                win.focus(new);
                if f.has('d') {
                    win.focus(before);
                }
            }
            Err(e) => {
                win.layout.remove(new);
                return Err(format!("split-window: {e}"));
            }
        }
        self.relayout();
        self.dirty = true;
        if let Some(by) = caller {
            self.record_spawn(new, by);
        }
        Ok(self.created(Place { si: at.si, wi: at.wi, pane: new }, &f, "#{session_name}:#{window_index}.#{pane_index}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_cells_or_percent_and_nothing_else() {
        assert_eq!(parse_size("20", "-l"), Ok(Want::Cells(20)));
        assert_eq!(parse_size("30%", "-l"), Ok(Want::Percent(30)));
        for bad in ["", "0", "0%", "100%", "-3", "x", "3x", "%", "70000", "1.5"] {
            assert!(parse_size(bad, "-l").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_screen_size_is_width_x_height_within_limits() {
        assert_eq!(parse_screen_size("120x40"), Some(Size { cols: 120, rows: 40, xpix: 0, ypix: 0 }));
        assert_eq!(parse_screen_size(" 80 X 24 ").map(|s| (s.cols, s.rows)), Some((80, 24)));
        for bad in ["", "80", "x24", "80x", "9x24", "80x9", "1001x24", "80x1001", "-80x24", "80x24x1", "axb"] {
            assert_eq!(parse_screen_size(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn x_and_y_flags_keep_what_they_do_not_name() {
        let base = Size { cols: 80, rows: 24, xpix: 0, ypix: 0 };
        assert_eq!(size_from_flags(None, None, base), Ok(base));
        assert_eq!(size_from_flags(Some("120"), None, base).map(|s| (s.cols, s.rows)), Ok((120, 24)));
        assert_eq!(size_from_flags(None, Some("50"), base).map(|s| (s.cols, s.rows)), Ok((80, 50)));
        assert!(size_from_flags(Some("0"), None, base).is_err());
        assert!(size_from_flags(None, Some("70000"), base).is_err());
        assert!(size_from_flags(Some("wide"), None, base).is_err());
    }

    #[test]
    fn a_share_leaves_room_for_the_divider_and_a_cell_for_each_half() {
        assert_eq!(Want::Percent(50).cells(81), 40);
        assert_eq!(Want::Percent(25).cells(81), 20);
        assert_eq!(Want::Percent(99).cells(11), 9, "ten cells to divide, one stays with the first half");
        assert_eq!(Want::Cells(500).cells(11), 9);
        assert_eq!(Want::Cells(1).cells(11), 1);
        assert_eq!(Want::Percent(1).cells(11), 1);
    }
}
