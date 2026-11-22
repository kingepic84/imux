pub mod agent;
pub mod layout;
pub mod popup;

use std::collections::HashMap;

use agent::AgentRecord;
use layout::{Direction, Layout, PaneId, Rect};

use crate::copymode::CopyMode;
use crate::term::Terminal;
use crate::tty::pty::Pty;

/// A running program: its pty and the emulator holding what it drew.
pub struct Pane {
    pub pty: Pty,
    pub term: Terminal,
    pub copy: Option<CopyMode>,
    /// What the program in the pane says it is doing (OSC 7501).
    pub agent: Option<AgentRecord>,
    /// A command to type once the shell has finished starting (see `TypeAhead`).
    pub typeahead: Option<TypeAhead>,
    /// The exit status of a program that ended while `remain-on-exit` kept its pane. Such a pane
    /// has no running program: its child is collected, its pty is out of the poll.
    pub dead: Option<i32>,
    /// The command the pane was started with (`#{pane_start_command}`); none for a plain shell.
    pub start: Option<String>,
    /// Keys and mouse reports typed by a client are not sent to the pane (`select-pane -d`);
    /// `send-keys` still reaches it.
    pub input_off: bool,
}

/// Text for a shell that drops what is typed while it starts (PowerShell reads the terminal for
/// its own queries and throws the typed bytes away with the answers): it is typed when the pane
/// has been quiet for a moment, or when too long has passed.
pub struct TypeAhead {
    pub text: Vec<u8>,
    pub since: std::time::Instant,
    pub last_output: std::time::Instant,
}

impl TypeAhead {
    /// How long a shell must have been silent, and the shortest and longest wait.
    const QUIET: std::time::Duration = std::time::Duration::from_millis(700);
    const LEAST: std::time::Duration = std::time::Duration::from_millis(1500);
    const MOST: std::time::Duration = std::time::Duration::from_secs(10);

    pub fn new(text: Vec<u8>) -> TypeAhead {
        let now = std::time::Instant::now();
        TypeAhead { text, since: now, last_output: now }
    }

    /// When the text may go in, if nothing more is printed.
    pub fn due(&self) -> std::time::Instant {
        (self.last_output + Self::QUIET).max(self.since + Self::LEAST).min(self.since + Self::MOST)
    }
}

/// The options a window can have of its own. A window without an override follows the global
/// option of the same name (`setw -g`). They are not saved with a session: a restored window
/// starts without overrides.
pub const WINDOW_OPTIONS: &[&str] = &["synchronize-panes", "monitor-activity", "monitor-bell"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowOpts {
    pub synchronize_panes: Option<bool>,
    pub monitor_activity: Option<bool>,
    pub monitor_bell: Option<bool>,
}

impl WindowOpts {
    /// The override for `name`, `None` when the window follows the global option (or `name` is no
    /// window option).
    pub fn get(&self, name: &str) -> Option<bool> {
        match name {
            "synchronize-panes" => self.synchronize_panes,
            "monitor-activity" => self.monitor_activity,
            "monitor-bell" => self.monitor_bell,
            _ => None,
        }
    }

    /// Give `name` an override (`Some`) or take it away (`None`); false for a name that is no
    /// window option.
    pub fn set(&mut self, name: &str, value: Option<bool>) -> bool {
        match name {
            "synchronize-panes" => self.synchronize_panes = value,
            "monitor-activity" => self.monitor_activity = value,
            "monitor-bell" => self.monitor_bell = value,
            _ => return false,
        }
        true
    }
}

/// What happened in a window nobody was looking at: the pane that printed or rang. Set only while
/// `monitor-activity` or `monitor-bell` is on for the window, cleared when the window is viewed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Alerts {
    pub activity: Option<PaneId>,
    pub bell: Option<PaneId>,
    /// The pane that was last to print before the window went quiet for `monitor-silence` seconds.
    pub silence: Option<PaneId>,
}

impl Alerts {
    pub fn any(&self) -> bool {
        self.activity.is_some() || self.bell.is_some() || self.silence.is_some()
    }
}

/// Panes arranged by a split tree, one of them focused.
pub struct Window {
    /// The number after `@` in a target (`@3`); 0 until the server numbers the window.
    pub id: usize,
    pub name: String,
    /// `automatic-rename`: the name follows the foreground process until the user renames it.
    pub auto_rename: bool,
    pub layout: Layout,
    pub panes: HashMap<PaneId, Pane>,
    pub active: PaneId,
    /// Free-form key/values kept with the window and saved with the session.
    pub meta: Vec<(String, String)>,
    /// Position in `Preset::ALL` of the arrangement chosen last, for `next-layout`.
    pub preset: Option<usize>,
    /// Overrides of the global options for this window (`setw` without `-g`).
    pub opts: WindowOpts,
    /// Activity and bell seen while the window was not viewed (`#` and `!` in `window_flags`).
    pub alerts: Alerts,
    /// The popup over this window's panes (`display-popup`), whose pane is in `panes` but in no
    /// layout. One at a time.
    pub popup: Option<popup::Popup>,
    /// When a pane of the window last printed, and which one (`monitor-silence`).
    pub last_output: (std::time::Instant, PaneId),
    last: Option<PaneId>,
}

impl Window {
    pub fn new(name: String, id: PaneId, first: Pane) -> Window {
        Window {
            id: 0,
            name,
            auto_rename: true,
            layout: Layout::new(id),
            panes: HashMap::from([(id, first)]),
            active: id,
            meta: Vec::new(),
            preset: None,
            opts: WindowOpts::default(),
            alerts: Alerts::default(),
            popup: None,
            last_output: (std::time::Instant::now(), id),
            last: None,
        }
    }

    /// How many panes the layout holds: the panes of the window, not the popup's.
    pub fn tiled(&self) -> usize {
        self.layout.panes().len()
    }

    pub fn focus(&mut self, id: PaneId) {
        if id != self.active && self.panes.contains_key(&id) {
            self.last = Some(self.active);
            self.active = id;
        }
    }

    /// Put `inn` (a pane that came from another window, with its record) where `out` was: in the
    /// layout, as the active pane and as the pane before it. `out` is not in the window afterwards.
    pub fn exchange(&mut self, out: PaneId, inn: PaneId, pane: Pane) {
        self.layout.replace(out, inn);
        self.panes.insert(inn, pane);
        if self.active == out {
            self.active = inn;
        }
        if self.last == Some(out) {
            self.last = Some(inn);
        }
    }

    /// Next pane in layout order, wrapping.
    pub fn focus_next(&mut self) {
        let order = self.layout.panes();
        if let Some(i) = order.iter().position(|&p| p == self.active) {
            self.focus(order[(i + 1) % order.len()]);
        }
    }

    pub fn focus_last(&mut self) {
        if let Some(id) = self.last {
            self.focus(id);
        }
    }

    /// The pane that was active before this one, if it still exists.
    pub fn last_pane(&self) -> Option<PaneId> {
        self.last.filter(|p| self.panes.contains_key(p))
    }

    pub fn focus_direction(&mut self, dir: Direction, area: Rect) {
        if let Some(id) = self.layout.neighbor(self.active, dir, area) {
            self.focus(id);
        }
    }

    /// Take a pane out of the window; focus moves to the previously focused pane, else the first.
    /// `None` when it is the last pane.
    pub fn remove(&mut self, id: PaneId) -> Option<Pane> {
        if !self.layout.remove(id) {
            return None;
        }
        let pane = self.panes.remove(&id);
        if self.last == Some(id) {
            self.last = None;
        }
        if self.active == id {
            self.active = self
                .last
                .take()
                .filter(|p| self.panes.contains_key(p))
                .unwrap_or_else(|| self.layout.panes()[0]);
        }
        pane
    }
}

/// An ordered list of windows with one current. Window numbers are positions (`base-index 1`
/// is applied when displaying) and always compact, as with `renumber-windows on`.
pub struct Session {
    /// Stable handle for clients; names can change and positions shift as sessions come and go.
    pub id: usize,
    pub name: String,
    pub windows: Vec<Window>,
    pub current: usize,
    /// Rows the status bar needs at the smallest client's width; grows as tabs wrap.
    pub status_rows: u16,
    last: Option<usize>,
}

impl Session {
    pub fn new(id: usize, name: String, first: Window) -> Session {
        Session { id, name, windows: vec![first], current: 0, status_rows: 1, last: None }
    }

    pub fn window(&self) -> &Window {
        &self.windows[self.current]
    }

    /// Position of the window that was current before this one.
    pub fn last_index(&self) -> Option<usize> {
        self.last.filter(|l| *l < self.windows.len())
    }

    pub fn window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.current]
    }

    pub fn select(&mut self, index: usize) {
        if index < self.windows.len() && index != self.current {
            self.last = Some(self.current);
            self.current = index;
        }
    }

    /// Go back to the window that was current before this one.
    pub fn select_last(&mut self) {
        if let Some(last) = self.last {
            self.select(last);
        }
    }

    pub fn select_next(&mut self) {
        self.select((self.current + 1) % self.windows.len());
    }

    pub fn select_previous(&mut self) {
        self.select((self.current + self.windows.len() - 1) % self.windows.len());
    }

    /// Go to the next (or previous) window with an alert, wrapping; false when no other window has
    /// one.
    pub fn select_alert(&mut self, forward: bool) -> bool {
        let n = self.windows.len();
        let found = (1..n)
            .map(|step| if forward { (self.current + step) % n } else { (self.current + n - step) % n })
            .find(|&i| self.windows[i].alerts.any());
        found.map(|i| self.select(i)).is_some()
    }

    /// Append a window and make it current.
    pub fn add(&mut self, window: Window) {
        self.windows.push(window);
        self.select(self.windows.len() - 1);
    }

    /// Insert a window right after the current one and make it current (`new-window -a`).
    pub fn add_after_current(&mut self, window: Window) {
        let at = self.current + 1;
        self.windows.insert(at, window);
        if let Some(last) = self.last.as_mut().filter(|l| **l >= at) {
            *last += 1;
        }
        self.select(at);
    }

    /// Put a window at position `at` (the end when `at` is past it), keeping the current window
    /// current, or making the new one current with `select`.
    pub fn insert_at(&mut self, at: usize, window: Window, select: bool) {
        let at = at.min(self.windows.len());
        self.windows.insert(at, window);
        if self.current >= at {
            self.current += 1;
        }
        if let Some(last) = self.last.as_mut().filter(|l| **l >= at) {
            *last += 1;
        }
        if select {
            self.select(at);
        }
    }

    /// Index of the window holding `pane`.
    pub fn window_of(&self, pane: PaneId) -> Option<usize> {
        self.windows.iter().position(|w| w.panes.contains_key(&pane))
    }

    pub fn pane_mut(&mut self, pane: PaneId) -> Option<&mut Pane> {
        self.windows.iter_mut().find_map(|w| w.panes.get_mut(&pane))
    }

    /// Exchange the windows at two positions. With `select` the window now at `b` becomes current,
    /// otherwise the current position stays. `false` when a position does not exist.
    pub fn swap(&mut self, a: usize, b: usize, select: bool) -> bool {
        if a >= self.windows.len() || b >= self.windows.len() {
            return false;
        }
        self.windows.swap(a, b);
        if select {
            self.select(b);
        }
        true
    }

    /// Take the window at `from` and put it at `to`; those in between shift by one. With `select`
    /// the moved window becomes current, otherwise the current window stays current.
    pub fn move_to(&mut self, from: usize, to: usize, select: bool) -> bool {
        if from >= self.windows.len() || to >= self.windows.len() {
            return false;
        }
        let at = |i: usize| match i {
            i if i == from => to,
            i if from < to && i > from && i <= to => i - 1,
            i if to < from && i >= to && i < from => i + 1,
            i => i,
        };
        let window = self.windows.remove(from);
        self.windows.insert(to, window);
        self.current = at(self.current);
        self.last = self.last.map(at);
        if select {
            self.select(to);
        }
        true
    }

    /// Remove a window (the last one stays). The previously current window takes over.
    pub fn remove(&mut self, index: usize) -> Option<Window> {
        if self.windows.len() < 2 || index >= self.windows.len() {
            return None;
        }
        let window = self.windows.remove(index);
        let shift = |i: usize| if i > index { i - 1 } else { i };
        self.last = self.last.filter(|&l| l != index).map(shift);
        if self.current == index {
            self.current = self.last.take().unwrap_or(index.min(self.windows.len() - 1));
        } else {
            self.current = shift(self.current);
        }
        Some(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tty::pty;
    use std::time::Duration;

    #[test]
    fn waiting_text_is_due_after_a_quiet_moment_never_too_early_and_never_too_late() {
        let mut t = TypeAhead::new(b"x\r".to_vec());
        let start = t.since;
        assert_eq!(t.due(), start + Duration::from_millis(1500));
        t.last_output = start + Duration::from_millis(1400);
        assert_eq!(t.due(), start + Duration::from_millis(2100));
        t.last_output = start + Duration::from_secs(60);
        assert_eq!(t.due(), start + Duration::from_secs(10));
    }

    fn dummy_window(name: &str, id: PaneId) -> Window {
        let size = nix::pty::Winsize { ws_row: 2, ws_col: 2, ws_xpixel: 0, ws_ypixel: 0 };
        let pty = pty::spawn("/usr/bin/true", &size, None).unwrap();
        Window::new(name.to_string(), id, Pane { pty, term: Terminal::new(2, 2, 0), copy: None, agent: None, typeahead: None, dead: None, start: None, input_off: false })
    }

    fn names(s: &Session) -> Vec<&str> {
        s.windows.iter().map(|w| w.name.as_str()).collect()
    }

    fn session_of(n: usize) -> Session {
        let mut s = Session::new(0, "0".into(), dummy_window("w0", 0));
        for i in 1..n {
            s.add(dummy_window(&format!("w{i}"), i as PaneId));
        }
        s
    }

    #[test]
    fn adding_selects_the_new_window() {
        let s = session_of(3);
        assert_eq!(s.current, 2);
        assert_eq!(names(&s), ["w0", "w1", "w2"]);
    }

    #[test]
    fn next_and_previous_wrap() {
        let mut s = session_of(3);
        s.select_next();
        assert_eq!(s.current, 0);
        s.select_previous();
        assert_eq!(s.current, 2);
    }

    #[test]
    fn removing_the_current_window_goes_back_to_the_previous_one() {
        let mut s = session_of(4);
        s.select(0);
        s.select(2);
        s.remove(2);
        assert_eq!(names(&s), ["w0", "w1", "w3"]);
        assert_eq!(s.current, 0);
    }

    #[test]
    fn removing_an_earlier_window_keeps_the_current_one() {
        let mut s = session_of(3);
        s.remove(0);
        assert_eq!(s.window().name, "w2");
        assert_eq!(s.current, 1);
    }

    #[test]
    fn select_last_toggles_between_two_windows() {
        let mut s = session_of(3);
        s.select(0);
        s.select_last();
        assert_eq!(s.current, 2);
        s.select_last();
        assert_eq!(s.current, 0);
    }

    #[test]
    fn swap_exchanges_two_windows_and_follows_when_selecting() {
        let mut s = session_of(4);
        assert!(s.swap(0, 2, true));
        assert_eq!(names(&s), ["w2", "w1", "w0", "w3"]);
        assert_eq!(s.current, 2);
        assert_eq!(s.window().name, "w0");
        assert!(!s.swap(0, 9, true));
    }

    #[test]
    fn swap_without_select_keeps_the_position() {
        let mut s = session_of(3);
        s.swap(0, 1, false);
        assert_eq!(names(&s), ["w1", "w0", "w2"]);
        assert_eq!(s.current, 2);
    }

    #[test]
    fn move_shifts_the_windows_in_between() {
        let mut s = session_of(4);
        assert!(s.move_to(3, 1, true));
        assert_eq!(names(&s), ["w0", "w3", "w1", "w2"]);
        assert_eq!(s.current, 1);
        assert!(s.move_to(1, 3, true));
        assert_eq!(names(&s), ["w0", "w1", "w2", "w3"]);
        assert!(!s.move_to(0, 4, true));
    }

    #[test]
    fn move_without_select_keeps_the_current_window_current() {
        let mut s = session_of(4);
        s.select(1);
        s.move_to(3, 0, false);
        assert_eq!(names(&s), ["w3", "w0", "w1", "w2"]);
        assert_eq!(s.window().name, "w1");
    }

    #[test]
    fn the_last_window_cannot_be_removed() {
        let mut s = session_of(1);
        assert!(s.remove(0).is_none());
    }

    #[test]
    fn panes_are_found_across_windows() {
        let mut s = session_of(3);
        assert_eq!(s.window_of(1), Some(1));
        assert!(s.pane_mut(2).is_some());
        assert!(s.pane_mut(9).is_none());
    }

    #[test]
    fn a_window_option_override_is_set_and_taken_away_by_name() {
        let mut o = WindowOpts::default();
        assert_eq!(o.get("synchronize-panes"), None);
        assert!(o.set("synchronize-panes", Some(true)));
        assert_eq!(o.get("synchronize-panes"), Some(true));
        assert!(o.set("synchronize-panes", Some(false)));
        assert_eq!(o.get("synchronize-panes"), Some(false), "off is an override too, not 'follow the global'");
        assert!(o.set("synchronize-panes", None));
        assert_eq!(o.get("synchronize-panes"), None);
        assert!(!o.set("mouse", Some(true)), "only window options");
        assert_eq!(o.get("mouse"), None);
        assert!(WINDOW_OPTIONS.contains(&"synchronize-panes"));
    }

    #[test]
    fn a_new_window_follows_the_global_option() {
        let s = session_of(1);
        assert_eq!(s.window().opts, WindowOpts::default());
    }

    #[test]
    fn the_monitor_options_are_window_options() {
        let mut o = WindowOpts::default();
        for name in ["monitor-activity", "monitor-bell"] {
            assert!(WINDOW_OPTIONS.contains(&name));
            assert_eq!(o.get(name), None);
            assert!(o.set(name, Some(false)));
            assert_eq!(o.get(name), Some(false));
            assert!(o.set(name, None));
            assert_eq!(o.get(name), None);
        }
        assert_eq!(o, WindowOpts::default());
    }

    #[test]
    fn a_new_window_has_no_alert() {
        let s = session_of(1);
        assert!(!s.window().alerts.any());
    }

    #[test]
    fn alert_selection_skips_quiet_windows_and_wraps_both_ways() {
        let mut s = session_of(5);
        s.select(0);
        s.windows[2].alerts.bell = Some(2);
        s.windows[4].alerts.activity = Some(4);
        assert!(s.select_alert(true));
        assert_eq!(s.current, 2);
        assert!(s.select_alert(true));
        assert_eq!(s.current, 4);
        assert!(s.select_alert(true), "wraps past the end");
        assert_eq!(s.current, 2);
        assert!(s.select_alert(false));
        assert_eq!(s.current, 4, "and backwards past the start");
        s.select(0);
        assert!(s.select_alert(false));
        assert_eq!(s.current, 4);
    }

    #[test]
    fn alert_selection_stays_put_without_another_alert() {
        let mut s = session_of(3);
        s.select(1);
        assert!(!s.select_alert(true));
        assert_eq!(s.current, 1);
        s.windows[1].alerts.bell = Some(1);
        assert!(!s.select_alert(false), "the window you are in does not count");
        assert_eq!(s.current, 1);
    }
}
