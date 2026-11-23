//! Input modes that take over a client's keyboard: the session/window picker and the one-line
//! rename prompt. Key decoding and the mode state machines are pure; the `Server` methods at the
//! bottom apply their outcomes.

use std::time::Instant;

use super::{MOUSE_MOTION, MOUSE_WHEEL, Mouse, Server};
use crate::config::stmt;
use crate::input::bindings::AskSpec;
use crate::log;
use crate::render::overlay::{self, Hit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Backspace,
    /// Ctrl-u: clear the line.
    Clear,
    Byte(u8),
}

/// Turn raw terminal input into keys. Unknown escape sequences are dropped, a lone ESC is Escape.
pub fn decode_keys(data: &[u8]) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        i += 1;
        match b {
            0x1b if i == data.len() => keys.push(Key::Escape),
            0x1b if matches!(data[i], b'[' | b'O') => {
                let start = i + 1;
                let Some(len) = data[start..].iter().position(|b| (0x40..=0x7e).contains(b)) else { break };
                let params = &data[start..start + len];
                let fin = data[start + len];
                i = start + len + 1;
                let key = match (fin, params) {
                    (b'A', _) => Some(Key::Up),
                    (b'B', _) => Some(Key::Down),
                    (b'H', _) => Some(Key::Home),
                    (b'F', _) => Some(Key::End),
                    (b'~', b"1" | b"7") => Some(Key::Home),
                    (b'~', b"4" | b"8") => Some(Key::End),
                    (b'~', b"5") => Some(Key::PageUp),
                    (b'~', b"6") => Some(Key::PageDown),
                    _ => None,
                };
                keys.extend(key);
            }
            0x1b => keys.push(Key::Escape),
            b'\r' | b'\n' => keys.push(Key::Enter),
            0x7f | 0x08 => keys.push(Key::Backspace),
            0x15 => keys.push(Key::Clear),
            _ => keys.push(Key::Byte(b)),
        }
    }
    keys
}

/// One selectable row of the tree picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Session id.
    Session(usize),
    /// Session id and window position.
    Window(usize, usize),
    /// Position in the client's repository list (`repo-wizard`).
    Repo(usize),
    /// Line of the client's text list (`list-keys`, `show-messages`); choosing one closes it.
    Text(usize),
    /// Position in the buffer stack, newest first (`choose-buffer`); choosing one pastes it.
    Buffer(usize),
    /// Client id (`choose-client`); choosing one detaches it.
    Client(usize),
}

pub struct Picker {
    pub entries: Vec<Entry>,
    pub selected: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PickerAction {
    None,
    Cancel,
    Choose(Entry),
}

const PAGE: usize = 10;

/// Messages kept for `show-messages`.
const MESSAGE_LOG: usize = 100;

impl Picker {
    pub fn key(&mut self, key: Key) -> PickerAction {
        let last = self.entries.len().saturating_sub(1);
        match key {
            Key::Up | Key::Byte(b'k') => self.selected = self.selected.saturating_sub(1),
            Key::Down | Key::Byte(b'j') => self.selected = (self.selected + 1).min(last),
            Key::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            Key::PageDown => self.selected = (self.selected + PAGE).min(last),
            Key::Home | Key::Byte(b'g') => self.selected = 0,
            Key::End | Key::Byte(b'G') => self.selected = last,
            Key::Enter => return self.entries.get(self.selected).map_or(PickerAction::Cancel, |&e| PickerAction::Choose(e)),
            Key::Escape | Key::Byte(b'q' | 0x03) => return PickerAction::Cancel,
            _ => {}
        }
        PickerAction::None
    }
}

/// What a key did to a `Filter`.
#[derive(Debug, PartialEq, Eq)]
pub enum FilterKey {
    /// Not the filter's: the picker or panel handles it.
    Pass,
    /// Taken, nothing else to do.
    Taken,
    /// Taken, and the query is different: the list must be filtered again.
    Changed,
}

/// The query typed after `/` in a picker or the agent panel. `all` and `labels` are the picker's
/// entries (and the text they show) before filtering; the agent panel filters its own rows and
/// leaves them empty.
#[derive(Debug)]
pub struct Filter {
    pub text: Vec<u8>,
    pub typing: bool,
    pub all: Vec<Entry>,
    pub labels: Vec<String>,
}

impl Filter {
    /// A filter that has just seen `/`.
    pub fn start(all: Vec<Entry>, labels: Vec<String>) -> Filter {
        Filter { text: Vec::new(), typing: true, all, labels }
    }

    pub fn query(&self) -> String {
        String::from_utf8_lossy(&self.text).into_owned()
    }

    /// What `/`, typing, Enter and Escape do; other keys are the list's. While typing, the arrows
    /// and the page keys still move the selection and Ctrl-C still closes.
    pub fn key(&mut self, key: Key) -> FilterKey {
        if !self.typing {
            return match key {
                Key::Byte(b'/') => {
                    self.typing = true;
                    FilterKey::Taken
                }
                Key::Escape if !self.text.is_empty() => {
                    self.text.clear();
                    FilterKey::Changed
                }
                _ => FilterKey::Pass,
            };
        }
        match key {
            Key::Byte(b) if b >= 0x20 && b != 0x7f => {
                self.text.push(b);
                FilterKey::Changed
            }
            Key::Backspace => {
                while let Some(b) = self.text.pop() {
                    if b & 0xc0 != 0x80 {
                        break;
                    }
                }
                FilterKey::Changed
            }
            Key::Clear if !self.text.is_empty() => {
                self.text.clear();
                FilterKey::Changed
            }
            Key::Enter => {
                self.typing = false;
                FilterKey::Taken
            }
            Key::Escape => {
                self.typing = false;
                if self.text.is_empty() {
                    FilterKey::Taken
                } else {
                    self.text.clear();
                    FilterKey::Changed
                }
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End | Key::Byte(0x03) => FilterKey::Pass,
            _ => FilterKey::Taken,
        }
    }

    /// The entries that match the query, best first, and the position to select: the entry that
    /// was selected if it is still there (an emptied query), else the first.
    pub fn apply(&self, selected: Option<Entry>) -> (Vec<Entry>, usize) {
        let query = self.query();
        if query.trim().is_empty() {
            let at = selected.and_then(|e| self.all.iter().position(|a| *a == e)).unwrap_or(0);
            return (self.all.clone(), at);
        }
        let entries: Vec<Entry> = crate::fuzzy::filter(&self.labels, &query).into_iter().filter_map(|(i, _)| self.all.get(i).copied()).collect();
        (entries, 0)
    }

    /// The line drawn on the bottom border: the query and how many entries it keeps.
    pub fn footer(&self, shown: usize, total: usize) -> String {
        format!("/{}{} {shown}/{total}", self.query(), if self.typing { "_" } else { "" })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Window,
    Session,
    Command,
    /// Name (and optional base) of a worktree in the repository the wizard picked.
    Worktree,
    /// Text for `find-window`.
    Find,
    /// A window number to select (`'`).
    Index,
    /// The number to move the current window to (`.`).
    MoveTo,
    /// `command-prompt` with a template, and `confirm-before`: the question is in `Client.ask`.
    Ask,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PromptAction {
    None,
    Cancel,
    Submit,
}

impl Prompt {
    pub fn key(&mut self, key: Key) -> PromptAction {
        match key {
            Key::Enter => return PromptAction::Submit,
            Key::Escape | Key::Byte(0x03) => return PromptAction::Cancel,
            Key::Clear => self.text.clear(),
            Key::Backspace => {
                while let Some(b) = self.text.pop() {
                    if b & 0xc0 != 0x80 {
                        break;
                    }
                }
            }
            Key::Byte(b) if b >= 0x20 => self.text.push(b),
            _ => {}
        }
        PromptAction::None
    }

    pub fn string(&self) -> String {
        String::from_utf8_lossy(&self.text).into_owned()
    }
}

/// Lines kept by the `:` prompt.
const HISTORY: usize = 100;

/// Add a line to the history unless it is empty or repeats the newest entry.
pub fn remember(history: &mut Vec<String>, line: &str) {
    if line.trim().is_empty() || history.last().is_some_and(|l| l == line) {
        return;
    }
    history.push(line.to_string());
    if history.len() > HISTORY {
        history.remove(0);
    }
}

/// Where Up (`older`) or Down moves in the history from `at`; `None` for no entry (the line being
/// typed). Returns `None` when the position does not change.
pub fn recall(len: usize, at: Option<usize>, older: bool) -> Option<Option<usize>> {
    match (at, older) {
        (None, true) => len.checked_sub(1).map(Some),
        (Some(i), true) => i.checked_sub(1).map(Some),
        (Some(i), false) if i + 1 < len => Some(Some(i + 1)),
        (Some(_), false) => Some(None),
        (None, false) => None,
    }
}

pub enum Mode {
    Normal,
    Picker(Picker),
    /// The agent panel (`agent-panel`).
    Agents(super::agents::AgentPanel),
    Prompt(Prompt),
    /// Pane numbers are shown until the deadline; a digit picks the pane.
    Panes(Instant),
    /// `clock-mode`: a clock in the active pane; the time is when it has to be drawn again.
    Clock(Instant),
    /// `display-menu`.
    Menu(super::menu::MenuState),
}

impl Server {
    /// Entries for the picker: every session, with its windows below when `with_windows`.
    pub(super) fn picker_entries(&self, with_windows: bool) -> Vec<Entry> {
        let mut entries = Vec::new();
        for s in &self.sessions {
            entries.push(Entry::Session(s.id));
            if with_windows {
                entries.extend((0..s.windows.len()).map(|w| Entry::Window(s.id, w)));
            }
        }
        entries
    }

    /// One line of text per entry, `*` marking what the client is looking at.
    pub(super) fn picker_labels(&self, picker: &Picker, client: usize) -> Vec<String> {
        let here = self.clients.get(&client).map(|c| c.session);
        let base = self.base_index();
        picker
            .entries
            .iter()
            .map(|entry| match *entry {
                Entry::Repo(i) => self.clients.get(&client).and_then(|c| c.repos.get(i)).map_or_else(String::new, |p| {
                    p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                }),
                Entry::Text(i) => self.clients.get(&client).and_then(|c| c.text.1.get(i)).cloned().unwrap_or_default(),
                Entry::Buffer(i) => self.buffer_label(i),
                Entry::Client(cid) => self.client_label(client, cid),
                Entry::Session(sid) => match self.sessions.iter().find(|s| s.id == sid) {
                    Some(s) => {
                        let glyph = self.plain_glyph(self.windows_state(&s.windows, client));
                        let glyph = if glyph.is_empty() { glyph } else { format!(" {glyph}") };
                        format!("{}{}{glyph}", self.session_summary(s), if here == Some(sid) { " *" } else { "" })
                    }
                    None => String::new(),
                },
                Entry::Window(sid, w) => match self.sessions.iter().find(|s| s.id == sid).and_then(|s| s.windows.get(w).map(|win| (s, win))) {
                    Some((s, win)) => {
                        let current = here == Some(sid) && s.current == w;
                        let glyph = self.plain_glyph(self.windows_state(std::iter::once(win), client));
                        let glyph = if glyph.is_empty() { glyph } else { format!(" {glyph}") };
                        format!("  {}: {}{}{glyph}", w + base, win.name, if current { " *" } else { "" })
                    }
                    None => String::new(),
                },
            })
            .collect()
    }

    pub(super) fn open_picker(&mut self, id: usize, with_windows: bool) {
        let entries = self.picker_entries(with_windows);
        let Some(c) = self.clients.get_mut(&id) else { return };
        let current = self.sessions.iter().find(|s| s.id == c.session);
        let selected = entries
            .iter()
            .position(|e| match (*e, current) {
                (Entry::Window(sid, w), Some(s)) if with_windows => sid == s.id && w == s.current,
                (Entry::Session(sid), Some(s)) if !with_windows => sid == s.id,
                _ => false,
            })
            .unwrap_or(0);
        c.enter(Mode::Picker(Picker { entries, selected }));
        self.dirty = true;
    }

    pub(super) fn open_prompt(&mut self, id: usize, kind: PromptKind) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let Some(s) = self.sessions.iter().find(|s| s.id == c.session) else { return };
        let text = match kind {
            PromptKind::Window => s.window().name.clone(),
            PromptKind::Session => s.name.clone(),
            PromptKind::Command | PromptKind::Worktree | PromptKind::Find | PromptKind::Index | PromptKind::MoveTo | PromptKind::Ask => String::new(),
        };
        c.mode = Mode::Prompt(Prompt { kind, text: text.into_bytes() });
        c.recall = None;
        c.draft.clear();
        c.ask = None;
        self.dirty = true;
    }

    /// `command-prompt` with flags or a template, and `confirm-before`. The label and the initial
    /// text are formats of the client's current pane; the template is expanded when it runs, so
    /// what was typed is never read as a format.
    pub(super) fn open_ask(&mut self, id: usize, spec: &AskSpec) {
        let Some(si) = self.client_session(id) else { return };
        let place = self.current_place(si);
        let label = spec.label.as_ref().map(|l| crate::sanitize::clean(&self.expand_at(place, l)));
        let initial = spec.initial.as_ref().map(|i| crate::sanitize::clean(&self.expand_at(place, i)));
        // A list of prompts (`-p "from,to"`, with `-I "a,b"` for the first texts) asks one after the
        // other, up to nine; a confirmation or a one-key question is one. The answers are `%1` to `%9`.
        let (mut labels, mut initials): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        if !spec.confirm && !spec.single {
            labels = label.iter().flat_map(|l| l.split(',')).take(9).map(str::to_string).collect();
            initials = initial.iter().flat_map(|i| i.split(',')).map(str::to_string).collect();
        }
        let (label, initial) = if labels.len() > 1 { (labels.first().cloned(), initials.first().cloned()) } else { (label, initial) };
        let more = (labels.len() > 1).then(|| super::AskMore {
            steps: labels.iter().zip(initials.iter().map(Some).chain(std::iter::repeat(None))).skip(1).map(|(l, i)| (l.clone(), i.cloned())).collect(),
            answers: Vec::new(),
        });
        let Some(c) = self.clients.get_mut(&id) else { return };
        let text = if spec.confirm { String::new() } else { initial.clone().unwrap_or_default() };
        c.mode = Mode::Prompt(Prompt { kind: PromptKind::Ask, text: text.into_bytes() });
        c.recall = None;
        c.draft.clear();
        c.ask = Some(AskSpec { label, initial, ..spec.clone() });
        c.ask_more = more;
        self.dirty = true;
    }

    /// The answer of an open question: with more prompts to go the next one opens, else the
    /// template, with `%1`..`%9` and `%%` (the last answer) replaced, runs as a command line.
    fn answer_ask(&mut self, id: usize, typed: &str) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let Some(ask) = c.ask.take() else { return };
        let mut answers = match c.ask_more.take() {
            Some(mut more) if !more.steps.is_empty() => {
                let (label, initial) = more.steps.remove(0);
                more.answers.push(typed.to_string());
                c.mode = Mode::Prompt(Prompt { kind: PromptKind::Ask, text: initial.clone().unwrap_or_default().into_bytes() });
                c.recall = None;
                c.draft.clear();
                c.ask = Some(AskSpec { label: Some(label), initial, ..ask });
                c.ask_more = Some(more);
                self.dirty = true;
                return;
            }
            Some(more) => more.answers,
            None => Vec::new(),
        };
        answers.push(typed.to_string());
        let Some(si) = self.client_session(id) else { return };
        let place = self.current_place(si);
        let line = fill_template(&self.expand_at(place, &ask.template), &answers);
        self.run_line(id, &line);
    }

    /// A key while a question is open. A confirmation takes `y` or `Y` and cancels on anything
    /// else; a one-key question runs on the first printable key; the rest is the usual prompt.
    fn ask_key(&mut self, id: usize, key: Key) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let Some(ask) = c.ask.as_ref() else {
            c.mode = Mode::Normal;
            return;
        };
        let (confirm, single) = (ask.confirm, ask.single);
        let Mode::Prompt(p) = &mut c.mode else { return };
        if confirm || single {
            let typed = match key {
                Key::Byte(b) if (0x20..0x7f).contains(&b) => Some(char::from(b).to_string()),
                _ => None,
            };
            c.mode = Mode::Normal;
            match typed {
                Some(t) if confirm && (t == "y" || t == "Y") => self.answer_ask(id, ""),
                Some(t) if single => self.answer_ask(id, &t),
                _ => c.ask = None,
            }
            return;
        }
        match p.key(key) {
            PromptAction::None => {}
            PromptAction::Cancel => {
                c.mode = Mode::Normal;
                c.ask = None;
            }
            PromptAction::Submit => {
                let text = p.string();
                c.mode = Mode::Normal;
                self.answer_ask(id, &text);
            }
        }
    }

    /// Keys typed while a picker or prompt is open.
    pub(super) fn mode_keys(&mut self, id: usize, data: &[u8]) {
        for key in decode_keys(data) {
            if self.filter_key(id, key) {
                self.dirty = true;
                continue;
            }
            let Some(c) = self.clients.get_mut(&id) else { return };
            match &mut c.mode {
                Mode::Normal => return,
                Mode::Agents(_) => self.agents_key(id, key),
                Mode::Picker(p) => match p.key(key) {
                    PickerAction::None => {}
                    PickerAction::Cancel => c.mode = Mode::Normal,
                    PickerAction::Choose(entry) => {
                        c.mode = Mode::Normal;
                        self.choose(id, entry);
                    }
                },
                Mode::Prompt(p) if p.kind == PromptKind::Ask => self.ask_key(id, key),
                Mode::Prompt(p) if p.kind == PromptKind::Command && matches!(key, Key::Up | Key::Down) => {
                    if let Some(to) = recall(self.history.len(), c.recall, key == Key::Up) {
                        if c.recall.is_none() {
                            c.draft = p.text.clone();
                        }
                        p.text = to.map_or_else(|| c.draft.clone(), |i| self.history[i].clone().into_bytes());
                        c.recall = to;
                    }
                }
                Mode::Prompt(p) => match p.key(key) {
                    PromptAction::None => {}
                    PromptAction::Cancel => c.mode = Mode::Normal,
                    PromptAction::Submit => {
                        let (kind, text) = (p.kind, p.string());
                        c.mode = Mode::Normal;
                        match kind {
                            PromptKind::Command => {
                                remember(&mut self.history, &text);
                                self.run_line(id, &text)
                            }
                            PromptKind::Worktree => self.wizard_submit(id, &text),
                            PromptKind::Find => self.find_window(id, &text),
                            PromptKind::Index => self.window_by_number(id, &text, false),
                            PromptKind::MoveTo => self.window_by_number(id, &text, true),
                            _ => self.rename(id, kind, text),
                        }
                    }
                },
                Mode::Panes(_) => {
                    c.mode = Mode::Normal;
                    if let Key::Byte(b @ b'0'..=b'9') = key {
                        self.pick_pane(id, usize::from(b - b'0'));
                    }
                }
                Mode::Clock(_) => c.mode = Mode::Normal,
                Mode::Menu(_) => self.menu_key(id, key),
            }
            self.dirty = true;
        }
    }

    /// `/` in a picker or the agent panel starts a filter; while one is open it takes the typing.
    /// True when the key was the filter's.
    fn filter_key(&mut self, id: usize, key: Key) -> bool {
        let Some(c) = self.clients.get(&id) else { return false };
        let agents = match &c.mode {
            Mode::Picker(_) => false,
            Mode::Agents(_) => true,
            _ => return false,
        };
        if c.filter.is_none() {
            if key != Key::Byte(b'/') {
                return false;
            }
            let (all, labels) = match &c.mode {
                Mode::Picker(p) => (p.entries.clone(), self.picker_labels(p, id)),
                _ => (Vec::new(), Vec::new()),
            };
            if let Some(c) = self.clients.get_mut(&id) {
                c.filter = Some(Filter::start(all, labels));
            }
            return true;
        }
        let Some(c) = self.clients.get_mut(&id) else { return false };
        let Some(filter) = c.filter.as_mut() else { return false };
        match filter.key(key) {
            FilterKey::Pass => false,
            FilterKey::Taken => true,
            FilterKey::Changed => {
                if let Mode::Picker(p) = &mut c.mode {
                    let (entries, selected) = filter.apply(p.entries.get(p.selected).copied());
                    p.entries = entries;
                    p.selected = selected;
                }
                if agents {
                    let first = self.agent_rows(id).first().map(|r| r.pane);
                    self.set_agent_selection(id, first);
                }
                true
            }
        }
    }

    /// Clicking an item picks it, clicking outside the box cancels, the wheel moves the selection.
    pub(super) fn picker_mouse(&mut self, id: usize, m: Mouse) {
        let Some(c) = self.clients.get(&id) else { return };
        let (Some(size), Mode::Picker(picker)) = (c.size, &c.mode) else { return };
        let bar_rows = self.sessions.iter().find(|s| s.id == c.session).map_or(1, |s| s.status_rows);
        let pane_rows = usize::from(size.rows.saturating_sub(bar_rows));
        let first_row = if self.status_top() { usize::from(bar_rows) } else { 0 };
        let labels = self.picker_labels(picker, id);
        if m.code & MOUSE_WHEEL != 0 {
            if !m.release {
                let key = if m.code & 1 == 0 { Key::Up } else { Key::Down };
                self.mode_keys_decoded(id, key);
            }
            return;
        }
        if m.release || m.code & MOUSE_MOTION != 0 || m.code & 3 != 0 {
            return;
        }
        let mut g = overlay::geometry(usize::from(size.cols), pane_rows, &labels, picker.selected);
        g.y += first_row;
        match overlay::hit(&g, labels.len(), usize::from(m.x), usize::from(m.y)) {
            Hit::Item(index) => {
                let Some(&entry) = picker.entries.get(index) else { return };
                if let Some(c) = self.clients.get_mut(&id) {
                    c.mode = Mode::Normal;
                }
                self.choose(id, entry);
            }
            Hit::Outside => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.mode = Mode::Normal;
                }
            }
            Hit::Border => {}
        }
        self.dirty = true;
    }

    fn mode_keys_decoded(&mut self, id: usize, key: Key) {
        if let Some(Mode::Picker(p)) = self.clients.get_mut(&id).map(|c| &mut c.mode) {
            p.key(key);
            self.dirty = true;
        }
    }

    /// Switch the client to the session (and window) the picker entry names.
    fn choose(&mut self, id: usize, entry: Entry) {
        let (sid, window) = match entry {
            Entry::Repo(i) => return self.wizard_pick(id, i),
            Entry::Text(_) => return,
            Entry::Buffer(i) => return self.paste_index(id, i),
            Entry::Client(cid) => return self.detach_client(cid),
            Entry::Session(sid) => (sid, None),
            Entry::Window(sid, w) => (sid, Some(w)),
        };
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        if let Some(w) = window {
            s.select(w);
        }
        self.attach(id, sid);
    }

    /// Run a line typed at the `:` prompt; a mistake shows as a message, never a failure.
    pub(super) fn run_line(&mut self, id: usize, line: &str) {
        match stmt::parse_line_aliased(line, &mut self.vars, &self.aliases) {
            Ok(stmts) => {
                if let Err(e) = self.apply_queue(id, stmts.into()) {
                    self.emit_command_error(Some(id), line.split_whitespace().next().unwrap_or(""), &e);
                    self.show_message(id, e);
                }
            }
            Err(e) => {
                self.emit_command_error(Some(id), line.split_whitespace().next().unwrap_or(""), &e);
                self.show_message(id, e);
            }
        }
    }

    pub(super) fn show_message(&mut self, id: usize, text: String) {
        log::info(&format!("client {id}: {text}"));
        self.messages.push_back(format!("{} {text}", crate::config::format::strftime("%H:%M:%S")));
        while self.messages.len() > MESSAGE_LOG {
            self.messages.pop_front();
        }
        let time = self.opt_ms("display-time");
        if let Some(c) = self.clients.get_mut(&id) {
            c.message = Some((text, Instant::now() + time));
        }
        self.dirty = true;
    }

    /// `q`: number the panes for `display-panes-time`.
    pub(super) fn display_panes(&mut self, id: usize) {
        let time = self.opt_ms("display-panes-time");
        self.display_panes_for(id, time);
    }

    /// `display-panes -d ms`: the numbers stay up that long, at most a minute.
    pub(super) fn display_panes_for(&mut self, id: usize, time: std::time::Duration) {
        let time = time.min(std::time::Duration::from_secs(60));
        if let Some(c) = self.clients.get_mut(&id) {
            c.mode = Mode::Panes(Instant::now() + time);
            c.panes_template = None;
        }
        self.dirty = true;
    }

    /// `display-panes [-d ms] template`: choosing a number runs the template with the pane's id.
    pub(super) fn display_panes_run(&mut self, id: usize, ms: Option<u64>, template: &str) {
        let time = ms.map_or_else(|| self.opt_ms("display-panes-time"), std::time::Duration::from_millis);
        self.display_panes_for(id, time);
        if let Some(c) = self.clients.get_mut(&id) {
            c.panes_template = Some(template.to_string());
        }
    }

    /// Focus the pane shown with this number.
    fn pick_pane(&mut self, id: usize, number: usize) {
        let Some(si) = self.client_session(id) else { return };
        let Some(size) = self.size_of(si) else { return };
        let area = self.pane_area(size, self.sessions[si].status_rows);
        let pane_base = self.pane_base_index();
        let template = self.clients.get_mut(&id).and_then(|c| c.panes_template.take());
        let win = self.sessions[si].window_mut();
        let Some(index) = number.checked_sub(pane_base) else { return };
        if let Some(&(pane, _)) = win.layout.rects(area).get(index) {
            match template {
                Some(t) => {
                    let line = t.replace("%%", &format!("%{pane}"));
                    self.run_line(id, &line);
                }
                None => win.focus(pane),
            }
            self.dirty = true;
        }
    }

    /// Rename the current window or session; shared by the rename prompts and the commands.
    pub(super) fn set_name(&mut self, id: usize, kind: PromptKind, text: String) {
        self.rename(id, kind, text);
    }

    fn rename(&mut self, id: usize, kind: PromptKind, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(sid) = self.clients.get(&id).map(|c| c.session) else { return };
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        match kind {
            PromptKind::Command | PromptKind::Worktree | PromptKind::Find | PromptKind::Index | PromptKind::MoveTo | PromptKind::Ask => {}
            PromptKind::Window => {
                let win = s.window_mut();
                win.name = text;
                win.auto_rename = false;
            }
            PromptKind::Session => {
                if self_has_name(&self.sessions, &text, sid) {
                    log::info(&format!("rename refused: session {text} exists"));
                    return;
                }
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.name = text;
                }
            }
        }
        self.dirty = true;
    }
}

/// The template of a `command-prompt` with the answers put in: `%1` to `%9` are the answers in the
/// order asked, `%%` the last one. One pass, so an answer is never read for `%` again.
fn fill_template(template: &str, answers: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                chars.next();
                out.push_str(answers.last().map_or("", String::as_str));
            }
            Some(d @ '1'..='9') => {
                chars.next();
                out.push_str(answers.get(d as usize - '1' as usize).map_or("", String::as_str));
            }
            _ => out.push('%'),
        }
    }
    out
}

fn self_has_name(sessions: &[crate::session::Session], name: &str, except: usize) -> bool {
    sessions.iter().any(|s| s.id != except && s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_gets_its_answers_once() {
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(fill_template("mv %1 %2", &a(&["x", "y"])), "mv x y");
        assert_eq!(fill_template("echo %%", &a(&["x", "y"])), "echo y", "%% is the last answer");
        assert_eq!(fill_template("%3 and 100%", &a(&["x"])), " and 100%", "a missing answer is empty, a lone % stays");
        assert_eq!(fill_template("%%", &a(&["%1"])), "%1", "an answer is not read again");
        assert_eq!(fill_template("%0 %a", &a(&["x"])), "%0 %a");
    }

    #[test]
    fn arrows_and_plain_keys_decode() {
        assert_eq!(decode_keys(b"\x1b[A\x1b[B\x1bOA"), [Key::Up, Key::Down, Key::Up]);
        assert_eq!(decode_keys(b"j\r"), [Key::Byte(b'j'), Key::Enter]);
        assert_eq!(decode_keys(b"\x7f\x15"), [Key::Backspace, Key::Clear]);
    }

    #[test]
    fn a_lone_escape_is_escape_but_unknown_sequences_vanish() {
        assert_eq!(decode_keys(b"\x1b"), [Key::Escape]);
        assert_eq!(decode_keys(b"\x1b[5~\x1b[9~"), [Key::PageUp]);
        assert_eq!(decode_keys(b"\x1b[1;5C"), []);
        assert_eq!(decode_keys(b"\x1b["), []);
    }

    fn picker(n: usize) -> Picker {
        Picker { entries: (0..n).map(Entry::Session).collect(), selected: 0 }
    }

    #[test]
    fn picker_moves_clamps_and_chooses() {
        let mut p = picker(3);
        assert_eq!(p.key(Key::Up), PickerAction::None);
        assert_eq!(p.selected, 0);
        p.key(Key::Down);
        p.key(Key::Byte(b'j'));
        p.key(Key::Down);
        assert_eq!(p.selected, 2);
        assert_eq!(p.key(Key::Enter), PickerAction::Choose(Entry::Session(2)));
        p.key(Key::Home);
        assert_eq!(p.selected, 0);
        p.key(Key::Byte(b'G'));
        assert_eq!(p.selected, 2);
    }

    #[test]
    fn picker_chooses_a_repository_entry() {
        let mut p = Picker { entries: (0..3).map(Entry::Repo).collect(), selected: 0 };
        p.key(Key::Down);
        assert_eq!(p.key(Key::Enter), PickerAction::Choose(Entry::Repo(1)));
    }

    fn filter_of(labels: &[&str]) -> (Picker, Filter) {
        let entries: Vec<Entry> = (0..labels.len()).map(Entry::Session).collect();
        let f = Filter::start(entries.clone(), labels.iter().map(|s| s.to_string()).collect());
        (Picker { entries, selected: 0 }, f)
    }

    fn type_text(p: &mut Picker, f: &mut Filter, text: &str) {
        for &b in text.as_bytes() {
            if f.key(Key::Byte(b)) == FilterKey::Changed {
                let (e, at) = f.apply(p.entries.get(p.selected).copied());
                p.entries = e;
                p.selected = at;
            }
        }
    }

    #[test]
    fn typing_narrows_the_list_to_the_matches_best_first() {
        let (mut p, mut f) = filter_of(&["alpha", "beta", "gamma", "delta"]);
        type_text(&mut p, &mut f, "ta");
        assert_eq!(p.entries, [Entry::Session(1), Entry::Session(3)], "beta and delta, in order of match");
        assert_eq!(p.selected, 0);
        assert_eq!(p.key(Key::Enter), PickerAction::Choose(Entry::Session(1)), "Enter chooses what is left");
    }

    #[test]
    fn the_chosen_entry_is_the_original_one_not_a_position_in_the_shown_list() {
        let (mut p, mut f) = filter_of(&["one", "two", "three", "four"]);
        type_text(&mut p, &mut f, "th");
        assert_eq!(p.entries, [Entry::Session(2)]);
        assert_eq!(p.key(Key::Enter), PickerAction::Choose(Entry::Session(2)));
    }

    #[test]
    fn enter_while_typing_keeps_the_list_and_moves_on_to_the_keys() {
        let (mut p, mut f) = filter_of(&["alpha", "beta", "delta"]);
        f.key(Key::Byte(b'/')); // already typing: this is a character of the query
        assert!(f.typing);
        f.text.clear();
        type_text(&mut p, &mut f, "ta");
        assert_eq!(f.key(Key::Enter), FilterKey::Taken);
        assert!(!f.typing);
        assert_eq!(p.entries.len(), 2, "the filtered list stays");
        assert_eq!(f.key(Key::Byte(b'j')), FilterKey::Pass, "j is the picker's again");
        assert_eq!(f.key(Key::Byte(b'q')), FilterKey::Pass);
    }

    #[test]
    fn escape_clears_the_query_and_a_second_escape_is_the_pickers() {
        let (mut p, mut f) = filter_of(&["alpha", "beta", "delta"]);
        type_text(&mut p, &mut f, "ta");
        f.key(Key::Enter);
        assert_eq!(f.key(Key::Escape), FilterKey::Changed, "first Escape clears");
        let (e, _) = f.apply(None);
        assert_eq!(e.len(), 3);
        assert_eq!(f.key(Key::Escape), FilterKey::Pass, "second Escape closes the picker");
    }

    #[test]
    fn escape_while_typing_clears_and_stops_typing() {
        let (mut p, mut f) = filter_of(&["alpha", "beta"]);
        type_text(&mut p, &mut f, "be");
        assert_eq!(f.key(Key::Escape), FilterKey::Changed);
        assert!(!f.typing && f.text.is_empty());
        let (mut p, mut f) = filter_of(&["alpha"]);
        assert_eq!(f.key(Key::Escape), FilterKey::Taken, "nothing typed: it only ends the typing");
        assert!(!f.typing);
        let _ = &mut p;
    }

    #[test]
    fn emptying_the_query_brings_the_list_back_on_the_entry_that_was_selected() {
        let (mut p, mut f) = filter_of(&["alpha", "beta", "gamma", "delta"]);
        type_text(&mut p, &mut f, "ta");
        p.key(Key::Down);
        assert_eq!(p.entries[p.selected], Entry::Session(3));
        assert_eq!(f.key(Key::Clear), FilterKey::Changed);
        let (e, at) = f.apply(p.entries.get(p.selected).copied());
        p.entries = e;
        p.selected = at;
        assert_eq!(p.entries.len(), 4, "everything is back");
        assert_eq!(p.entries[p.selected], Entry::Session(3), "on delta again");
    }

    #[test]
    fn a_query_that_matches_nothing_leaves_an_empty_list_that_enter_does_not_choose_from() {
        let (mut p, mut f) = filter_of(&["alpha", "beta"]);
        type_text(&mut p, &mut f, "zzz");
        assert!(p.entries.is_empty());
        assert_eq!(p.selected, 0);
        assert_eq!(p.key(Key::Enter), PickerAction::Cancel);
        assert_eq!(p.key(Key::Down), PickerAction::None);
    }

    #[test]
    fn while_typing_the_arrows_and_ctrl_c_still_reach_the_picker_and_letters_do_not() {
        let (_, mut f) = filter_of(&["alpha"]);
        for k in [Key::Up, Key::Down, Key::PageUp, Key::PageDown, Key::Home, Key::End, Key::Byte(3)] {
            assert_eq!(f.key(k), FilterKey::Pass, "{k:?}");
        }
        for &b in b"jkqgG" {
            assert_eq!(f.key(Key::Byte(b)), FilterKey::Changed, "{} is typed", b as char);
        }
        assert_eq!(f.query(), "jkqgG");
    }

    #[test]
    fn the_keys_of_a_picker_are_free_until_slash() {
        let mut f = Filter { text: Vec::new(), typing: false, all: Vec::new(), labels: Vec::new() };
        for &b in b"jkqgGa" {
            assert_eq!(f.key(Key::Byte(b)), FilterKey::Pass);
        }
        assert_eq!(f.key(Key::Escape), FilterKey::Pass, "no query: Escape closes");
        assert_eq!(f.key(Key::Byte(b'/')), FilterKey::Taken);
        assert!(f.typing);
    }

    #[test]
    fn control_characters_are_not_typed_into_the_query_and_utf8_backspaces_whole() {
        let (mut p, mut f) = filter_of(&["café"]);
        assert_eq!(f.key(Key::Byte(0x07)), FilterKey::Taken);
        assert_eq!(f.key(Key::Byte(0x7f)), FilterKey::Taken);
        type_text(&mut p, &mut f, "é");
        assert_eq!(f.query(), "é");
        assert_eq!(f.key(Key::Backspace), FilterKey::Changed);
        assert_eq!(f.query(), "", "both bytes of the character went");
    }

    #[test]
    fn the_footer_shows_the_query_a_cursor_while_typing_and_the_counts() {
        let (mut p, mut f) = filter_of(&["alpha", "beta"]);
        type_text(&mut p, &mut f, "be");
        assert_eq!(f.footer(p.entries.len(), 2), "/be_ 1/2");
        f.key(Key::Enter);
        assert_eq!(f.footer(p.entries.len(), 2), "/be 1/2");
    }

    #[test]
    fn picker_cancels_on_escape_and_q() {
        assert_eq!(picker(2).key(Key::Escape), PickerAction::Cancel);
        assert_eq!(picker(2).key(Key::Byte(b'q')), PickerAction::Cancel);
        assert_eq!(picker(0).key(Key::Enter), PickerAction::Cancel);
    }

    #[test]
    fn prompt_edits_text_including_multibyte_characters() {
        let mut p = Prompt { kind: PromptKind::Window, text: Vec::new() };
        for &b in "aé".as_bytes() {
            p.key(Key::Byte(b));
        }
        assert_eq!(p.string(), "aé");
        p.key(Key::Backspace);
        assert_eq!(p.string(), "a");
        p.key(Key::Byte(b'b'));
        assert_eq!(p.key(Key::Enter), PromptAction::Submit);
        assert_eq!(p.string(), "ab");
        p.key(Key::Clear);
        assert_eq!(p.string(), "");
        assert_eq!(p.key(Key::Escape), PromptAction::Cancel);
    }

    #[test]
    fn history_skips_empty_lines_and_repeats_and_is_capped() {
        let mut h = Vec::new();
        remember(&mut h, "  ");
        remember(&mut h, "new-window");
        remember(&mut h, "new-window");
        remember(&mut h, "kill-pane");
        assert_eq!(h, ["new-window", "kill-pane"]);
        for n in 0..HISTORY + 5 {
            remember(&mut h, &format!("display {n}"));
        }
        assert_eq!(h.len(), HISTORY);
        assert_eq!(h.last().unwrap(), &format!("display {}", HISTORY + 4));
    }

    #[test]
    fn recall_walks_back_and_forth_and_returns_to_the_draft() {
        assert_eq!(recall(0, None, true), None);
        assert_eq!(recall(3, None, true), Some(Some(2)));
        assert_eq!(recall(3, Some(2), true), Some(Some(1)));
        assert_eq!(recall(3, Some(0), true), None);
        assert_eq!(recall(3, Some(1), false), Some(Some(2)));
        assert_eq!(recall(3, Some(2), false), Some(None));
        assert_eq!(recall(3, None, false), None);
    }
}
