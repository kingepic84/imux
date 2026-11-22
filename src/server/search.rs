//! Looking things up: `find-window`, the window number prompts, the scrollable text list behind
//! `list-keys` and `show-messages`, and the way back to the previous session.

use super::modes::{Entry, Mode, Picker};
use super::{Server, agents};
use crate::input::bindings::Command;
use crate::log;
use crate::proto::ServerMsg;

/// Whether a window called `name` whose panes show `screens` holds `needle`, ignoring case.
pub(super) fn window_matches(needle: &str, name: &str, screens: &[String]) -> bool {
    let needle = needle.to_lowercase();
    name.to_lowercase().contains(&needle) || screens.iter().any(|s| s.to_lowercase().contains(&needle))
}

impl Server {
    /// `find-window`: the windows of the client's session with `text` in their name or on a
    /// pane's screen. One match is selected, several are offered in the window list.
    pub(super) fn find_window(&mut self, id: usize, text: &str) {
        let text = text.trim();
        let Some(si) = self.client_session(id) else { return };
        if text.is_empty() {
            return;
        }
        let hits: Vec<usize> = self.sessions[si]
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                let screens: Vec<String> = w.panes.values().map(|p| agents::last_lines(p, usize::MAX).join("\n")).collect();
                window_matches(text, &w.name, &screens)
            })
            .map(|(i, _)| i)
            .collect();
        match hits.as_slice() {
            [] => self.show_message(id, format!("no windows matching: {text}")),
            [one] => {
                self.sessions[si].select(*one);
                self.dirty = true;
            }
            many => {
                let sid = self.sessions[si].id;
                let entries = many.iter().map(|&w| Entry::Window(sid, w)).collect();
                if let Some(c) = self.clients.get_mut(&id) {
                    c.enter(Mode::Picker(Picker { entries, selected: 0 }));
                }
                self.dirty = true;
            }
        }
    }

    /// The answer to the `'` and `.` prompts: select the window with that number, or move the
    /// current window there.
    pub(super) fn window_by_number(&mut self, id: usize, text: &str, move_there: bool) {
        let Ok(n) = text.trim().parse::<usize>() else {
            return self.show_message(id, format!("{:?} is not a window number", text.trim()));
        };
        let command = if move_there { Command::MoveWindow { src: None, dst: n, select: true } } else { Command::SelectWindow(n) };
        self.run_command(id, &command);
    }

    /// `display-message` typed or bound to a key: `text` with every variable of the client's current
    /// pane filled in, as the socket form of the command does (pane, window and session variables,
    /// a hook's, option names, `@` options and the environment).
    pub(super) fn expand_message(&self, client: usize, text: &str) -> String {
        let Some(si) = self.client_session(client) else { return text.to_string() };
        self.expand_at(self.current_place(si), text)
    }

    /// Show `lines` in a list that scrolls; Enter, `q` and Escape close it.
    pub(super) fn show_text(&mut self, id: usize, title: &str, lines: Vec<String>) {
        if lines.is_empty() {
            return self.show_message(id, format!("{title}: nothing to show"));
        }
        let entries = (0..lines.len()).map(Entry::Text).collect();
        if let Some(c) = self.clients.get_mut(&id) {
            c.text = (title.to_string(), lines);
            c.enter(Mode::Picker(Picker { entries, selected: 0 }));
        }
        self.dirty = true;
    }

    /// The clients that have a screen, by id.
    fn attached_clients(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self.clients.iter().filter(|(_, c)| c.size.is_some() && !c.closing).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids
    }

    /// `choose-client`: the attached clients in a list; choosing one detaches it.
    pub(super) fn choose_client(&mut self, id: usize) {
        let entries: Vec<Entry> = self.attached_clients().into_iter().map(Entry::Client).collect();
        if entries.is_empty() {
            return;
        }
        let selected = entries.iter().position(|e| *e == Entry::Client(id)).unwrap_or(0);
        if let Some(c) = self.clients.get_mut(&id) {
            c.enter(Mode::Picker(Picker { entries, selected }));
        }
        self.dirty = true;
    }

    /// The line of a client in the chooser: id, size and session, `*` for the one asking.
    pub(super) fn client_label(&self, asking: usize, id: usize) -> String {
        let Some(c) = self.clients.get(&id) else { return String::new() };
        let size = c.size.map_or_else(String::new, |s| format!("{}x{}", s.cols, s.rows));
        let session = self.sessions.iter().find(|s| s.id == c.session).map_or("", |s| s.name.as_str());
        format!("client {id}: {size} [{session}]{}", if id == asking { " *" } else { "" })
    }

    /// Chosen in the chooser: send that client away.
    pub(super) fn detach_client(&mut self, id: usize) {
        if self.clients.contains_key(&id) {
            log::info(&format!("client {id} detached"));
            self.queue(id, &ServerMsg::Detached);
            self.close_after_flush(id);
        }
    }

    /// `switch-client -l`: back to the session looked at before this one.
    pub(super) fn last_session(&mut self, id: usize) {
        let Some(prev) = self.clients.get(&id).and_then(|c| c.last_session) else {
            return self.show_message(id, "no last session".to_string());
        };
        if self.sessions.iter().any(|s| s.id == prev) {
            self.attach(id, prev);
        } else {
            self.show_message(id, "the last session is gone".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_matches_by_name_or_screen_ignoring_case() {
        let screens = vec!["$ cargo build\nFinished dev".to_string()];
        assert!(window_matches("EDIT", "editor", &[]));
        assert!(window_matches("finished", "zsh", &screens));
        assert!(window_matches("cargo", "zsh", &screens));
        assert!(!window_matches("nothing", "zsh", &screens));
        assert!(!window_matches("x", "zsh", &[]));
    }
}
