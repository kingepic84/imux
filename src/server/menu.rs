//! `display-menu`: a box of items over the panes. A key or a click runs the command of one item.
//! `Menu` (items, selection, keys, size) and `place` (where the box goes) are pure; the `Server`
//! methods at the bottom open the menu, route keys and the mouse, and run the chosen command.

use unicode_width::UnicodeWidthStr;

use super::modes::Key;
use super::position::{self, Where};
use super::{MOUSE_MOTION, MOUSE_WHEEL, Mouse, Server};
use crate::config::{format, options};
use crate::input::bindings::MenuSpec;
use crate::render::overlay::{self, MenuLook, MenuRow, RowKind};
use crate::sanitize;
use crate::term::cell::Style;

/// What an item is: chosen by a key or Enter, shown but never chosen, or a rule between groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Item,
    Disabled,
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub label: String,
    /// The key as written, shown as `(k)`.
    pub key_text: String,
    /// The key that chooses the item, when the text names one a terminal can send.
    pub key: Option<Key>,
    pub command: String,
}

/// An item as the command gave it, with its name already expanded.
pub struct Raw {
    pub name: String,
    pub separator: bool,
    pub key: String,
    pub command: String,
}

/// What a key or a click did to an open menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Choose(usize),
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub items: Vec<Item>,
    pub selected: Option<usize>,
    pub title: String,
    show_keys: bool,
    content: usize,
}

/// The key a shortcut text names: one ASCII character, `C-x`, or `Enter`, `Space`, `Tab`,
/// `Escape`, `Up`, `Down`, `Home`, `End`, `PageUp`, `PageDown`.
pub fn shortcut(text: &str) -> Option<Key> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii() && !c.is_ascii_control() => return Some(Key::Byte(c as u8)),
        _ => {}
    }
    if let Some(c) = text.strip_prefix("C-").filter(|r| r.len() == 1).and_then(|r| r.bytes().next()) {
        return c.is_ascii_alphabetic().then(|| Key::Byte(c.to_ascii_lowercase() & 0x1f));
    }
    match text {
        "Enter" => Some(Key::Enter),
        "Space" => Some(Key::Byte(b' ')),
        "Tab" => Some(Key::Byte(b'\t')),
        "Escape" => Some(Key::Escape),
        "Up" => Some(Key::Up),
        "Down" => Some(Key::Down),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp),
        "PageDown" => Some(Key::PageDown),
        _ => None,
    }
}

fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// `text` cut to at most `max` columns, ending in `>` when something was cut.
fn cut(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        if width(&out) + width(ch.encode_utf8(&mut [0; 4])) + 1 > max {
            break;
        }
        out.push(ch);
    }
    out.push('>');
    out
}

impl Menu {
    /// The menu of `raw` items: a separator at the start, at the end or after another is dropped,
    /// a name starting with `-` is a disabled item, a name that expanded to nothing drops its item.
    /// `start` is the `-C` value: `-` for no choice, a number for that item, else the first one
    /// that can be chosen.
    pub fn build(raw: Vec<Raw>, title: String, start: Option<&str>) -> Menu {
        let mut items: Vec<Item> = Vec::new();
        for r in raw {
            if r.separator {
                if items.last().is_some_and(|i| i.kind != Kind::Separator) {
                    items.push(Item { kind: Kind::Separator, label: String::new(), key_text: String::new(), key: None, command: String::new() });
                }
                continue;
            }
            let (kind, label) = match r.name.strip_prefix('-') {
                Some(rest) => (Kind::Disabled, rest.to_string()),
                None => (Kind::Item, r.name),
            };
            if label.is_empty() {
                continue;
            }
            let key = if kind == Kind::Item { shortcut(&r.key) } else { None };
            items.push(Item { kind, label, key_text: r.key, key, command: r.command });
        }
        while items.last().is_some_and(|i| i.kind == Kind::Separator) {
            items.pop();
        }
        let mut menu = Menu { items, selected: None, title, show_keys: false, content: 0 };
        menu.selected = match start {
            Some("-") => None,
            Some(n) => n.parse::<usize>().ok().and_then(|n| menu.from(n.min(menu.items.len().saturating_sub(1)), true)),
            None => menu.from(0, true),
        };
        menu
    }

    fn can_choose(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(|it| it.kind == Kind::Item)
    }

    /// The first item that can be chosen at `i` or after it (or before it when `forward` is false).
    fn from(&self, i: usize, forward: bool) -> Option<usize> {
        if forward { (i..self.items.len()).find(|&j| self.can_choose(j)) } else { (0..=i.min(self.items.len().saturating_sub(1))).rev().find(|&j| self.can_choose(j)) }
    }

    /// Fit the menu to a screen `cols` wide: labels that are too long are cut with a trailing `>`,
    /// keys are shown when they fit beside the longest label, and the width is settled.
    pub fn fit(&mut self, cols: usize, bordered: bool) {
        let pad = usize::from(bordered);
        let avail = cols.saturating_sub(2 * pad + 2);
        let key_col = self.items.iter().filter(|i| !i.key_text.is_empty()).map(|i| width(&i.key_text) + 2).max().unwrap_or(0);
        let name_max = self.items.iter().map(|i| width(&i.label)).max().unwrap_or(0);
        self.show_keys = key_col > 0 && name_max + 1 + key_col <= avail;
        let limit = if self.show_keys { avail - key_col - 1 } else { avail }.max(1);
        for item in &mut self.items {
            item.label = cut(&item.label, limit);
        }
        let name_max = self.items.iter().map(|i| width(&i.label)).max().unwrap_or(0);
        let named = if self.show_keys { name_max + 1 + key_col } else { name_max };
        self.content = named.max(width(&self.title)).min(avail.max(1));
    }

    /// Columns and rows of the box.
    pub fn size(&self, bordered: bool) -> (usize, usize) {
        let pad = usize::from(bordered);
        (self.content + 2 + 2 * pad, self.items.len() + 2 * pad)
    }

    /// Move the choice `by` places, skipping rules and disabled items, wrapping at the ends.
    fn step(&mut self, by: isize) {
        let n = self.items.len() as isize;
        if n == 0 || !self.items.iter().any(|i| i.kind == Kind::Item) {
            return;
        }
        let mut at = self.selected.map_or(if by > 0 { -1 } else { n }, |s| s as isize);
        loop {
            at = (at + by).rem_euclid(n);
            if self.can_choose(at as usize) {
                self.selected = Some(at as usize);
                return;
            }
        }
    }

    /// Move the choice about `by` places without wrapping, onto an item that can be chosen.
    fn page(&mut self, by: isize) {
        let last = self.items.len().saturating_sub(1) as isize;
        let target = (self.selected.map_or(0, |s| s as isize) + by).clamp(0, last) as usize;
        let found = if by > 0 { self.from(target, false).or_else(|| self.from(target, true)) } else { self.from(target, true).or_else(|| self.from(target, false)) };
        if found.is_some() {
            self.selected = found;
        }
    }

    /// A key typed while the menu is open: a shortcut chooses, Enter chooses the choice, the
    /// movement keys move it, `q`, Escape, `C-c` and `C-g` close.
    pub fn key(&mut self, key: Key) -> Action {
        if let Some(i) = self.items.iter().position(|it| it.kind == Kind::Item && it.key == Some(key)) {
            self.selected = Some(i);
            return Action::Choose(i);
        }
        match key {
            Key::Enter => return self.selected.filter(|&i| self.can_choose(i)).map_or(Action::None, Action::Choose),
            Key::Escape | Key::Byte(b'q' | 0x03 | 0x07) => return Action::Close,
            Key::Up | Key::Byte(b'k') => self.step(-1),
            Key::Down | Key::Byte(b'j') => self.step(1),
            Key::PageUp | Key::Byte(0x02) => self.page(-5),
            Key::PageDown | Key::Byte(0x06) => self.page(5),
            Key::Home | Key::Byte(b'g') => self.selected = self.from(0, true).or(self.selected),
            Key::End | Key::Byte(b'G') => self.selected = self.from(self.items.len().saturating_sub(1), false).or(self.selected),
            _ => {}
        }
        Action::None
    }

    /// The rows for drawing.
    pub fn rows(&self) -> Vec<MenuRow<'_>> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, it)| MenuRow {
                label: &it.label,
                key: if self.show_keys { &it.key_text } else { "" },
                kind: match it.kind {
                    Kind::Item => RowKind::Item,
                    Kind::Disabled => RowKind::Disabled,
                    Kind::Separator => RowKind::Separator,
                },
                selected: self.selected == Some(i),
            })
            .collect()
    }
}

/// An open menu on a client: the items, the box and how it is drawn.
pub struct MenuState {
    pub menu: Menu,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub lines: String,
    /// The menu's own style, the choice's and the border's.
    pub styles: (Style, Style, Style),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Item(usize),
    Border,
    Outside,
}

impl MenuState {
    /// Where the box is on a screen `cols` wide whose pane rows start at `first_row` and number
    /// `rows`; it is moved to stay inside, and None when it no longer fits (the terminal shrank).
    pub fn origin(&self, cols: usize, first_row: usize, rows: usize) -> Option<(usize, usize)> {
        if self.w > cols || self.h > rows {
            return None;
        }
        Some((self.x.min(cols - self.w), self.y.clamp(first_row, first_row + rows - self.h)))
    }

    pub fn hit(&self, (ox, oy): (usize, usize), x: usize, y: usize) -> Hit {
        if x < ox || x >= ox + self.w || y < oy || y >= oy + self.h {
            return Hit::Outside;
        }
        let pad = usize::from(overlay::line_glyphs(&self.lines).is_some());
        let (row, col) = (y - oy, x - ox);
        if row < pad || row >= self.h - pad || col < pad || col >= self.w - pad {
            return Hit::Border;
        }
        let index = row - pad;
        if index < self.menu.items.len() { Hit::Item(index) } else { Hit::Border }
    }

    pub fn look(&self, origin: (usize, usize)) -> MenuLook<'_> {
        MenuLook {
            x: origin.0,
            y: origin.1,
            w: self.w,
            h: self.h,
            title: &self.menu.title,
            lines: &self.lines,
            base: self.styles.0,
            chosen: self.styles.1,
            border: self.styles.2,
        }
    }
}

impl Server {
    /// `display-menu` for a client: expand the names against the target pane, build and place the
    /// box, and take the client's keys until an item is chosen or the menu is closed.
    pub(super) fn open_menu(&mut self, id: usize, spec: &MenuSpec) {
        let Some(si) = self.client_session(id) else { return };
        let Some(size) = self.clients.get(&id).and_then(|c| c.size) else { return };
        let place = match spec.target.as_deref() {
            Some(t) => match self.resolve(t, crate::target::Kind::Pane, None) {
                Ok(p) => p,
                Err(e) => return self.show_message(id, e),
            },
            None => self.current_place(si),
        };
        let vars = self.place_vars(place);
        let (options, pane_env) = (&self.options, &self.pane_env);
        let lookup = |name: &str| super::remote::lookup_one(options, pane_env, &vars, name);
        let title = sanitize::clean(&format::expand(&spec.title, &lookup));
        let raw: Vec<Raw> = spec
            .items
            .iter()
            .map(|i| Raw {
                name: sanitize::clean(&format::expand(&i.name, &lookup)),
                separator: i.name.is_empty(),
                key: sanitize::clean(&i.key),
                command: i.command.clone(),
            })
            .collect();
        let mut menu = Menu::build(raw, title, spec.start.as_deref());
        if menu.items.is_empty() {
            return;
        }
        let lines = spec.lines.clone().unwrap_or_else(|| self.options.text("menu-border-lines").to_string());
        let bordered = overlay::line_glyphs(&lines).is_some();
        let bar_rows = self.sessions[si].status_rows;
        let area = self.pane_area(size, bar_rows);
        let (sx, sy) = (usize::from(area.w), usize::from(area.h));
        menu.fit(sx, bordered);
        let (w, h) = menu.size(bordered);
        if w > sx || h > sy {
            return self.show_message(id, "display-menu: the menu does not fit".to_string());
        }
        let win = &self.sessions[place.si].windows[place.wi];
        let pane = win.layout.rects(area).iter().find(|(p, _)| *p == place.pane).map_or((0, 0, sx, sy), |(_, r)| {
            (usize::from(r.x - area.x), usize::from(r.y - area.y), usize::from(r.w), usize::from(r.h))
        });
        let status = (bar_rows > 0).then(|| self.status_top());
        let at = Where { sx, sy, w, h, pane, status, last: self.last_menu };
        let pointer = self.pointer_in(area);
        let (x, y) = match position::place_at(spec.x.as_deref(), spec.y.as_deref(), &at, pointer, &lookup) {
            Ok(p) => p,
            Err(e) => return self.show_message(id, format!("display-menu: {e}")),
        };
        self.last_menu = Some((x, y));
        let (x, y) = (x + usize::from(area.x), y + usize::from(area.y));
        let style = |own: &Option<String>, option: &str| match own.as_deref().map(options::parse_style) {
            Some(Ok(s)) => s.style(),
            _ => self.options.style(option).style(),
        };
        let styles = (
            style(&spec.style, "menu-style"),
            style(&spec.selected_style, "menu-selected-style"),
            style(&spec.border_style, "menu-border-style"),
        );
        if let Some(c) = self.clients.get_mut(&id) {
            c.enter(super::modes::Mode::Menu(MenuState { menu, x, y, w, h, lines, styles }));
        }
        self.dirty = true;
    }

    /// Keys typed while a menu is open.
    pub(super) fn menu_key(&mut self, id: usize, key: Key) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        let super::modes::Mode::Menu(state) = &mut c.mode else { return };
        match state.menu.key(key) {
            Action::None => {}
            Action::Close => c.mode = super::modes::Mode::Normal,
            Action::Choose(i) => {
                let command = state.menu.items[i].command.clone();
                c.mode = super::modes::Mode::Normal;
                self.run_menu_command(id, &command);
            }
        }
        self.dirty = true;
    }

    /// A click on an item chooses it, a click outside the box closes it, the wheel moves the
    /// choice and the pointer moving over an item makes it the choice.
    pub(super) fn menu_mouse(&mut self, id: usize, m: Mouse) {
        let Some(c) = self.clients.get(&id) else { return };
        let (Some(size), super::modes::Mode::Menu(state)) = (c.size, &c.mode) else { return };
        let bar_rows = self.sessions.iter().find(|s| s.id == c.session).map_or(1, |s| s.status_rows);
        let area = self.pane_area(size, bar_rows);
        let Some(origin) = state.origin(usize::from(size.cols), usize::from(area.y), usize::from(area.h)) else { return };
        let hit = state.hit(origin, usize::from(m.x), usize::from(m.y));
        if m.code & MOUSE_WHEEL != 0 {
            if !m.release {
                self.menu_key(id, if m.code & 1 == 0 { Key::Up } else { Key::Down });
            }
            return;
        }
        if m.code & MOUSE_MOTION != 0 {
            if let (Hit::Item(i), Some(c)) = (hit, self.clients.get_mut(&id))
                && let super::modes::Mode::Menu(state) = &mut c.mode
                && state.menu.items[i].kind == Kind::Item
            {
                state.menu.selected = Some(i);
                self.dirty = true;
            }
            return;
        }
        if m.release || m.code & 3 != 0 {
            return;
        }
        match hit {
            Hit::Item(i) => {
                let enabled = state.menu.items[i].kind == Kind::Item;
                if enabled && let Some(c) = self.clients.get_mut(&id) {
                    if let super::modes::Mode::Menu(state) = &mut c.mode {
                        state.menu.selected = Some(i);
                    }
                    self.menu_key(id, Key::Enter);
                }
            }
            Hit::Outside => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.mode = super::modes::Mode::Normal;
                }
            }
            Hit::Border => {}
        }
        self.dirty = true;
    }

    /// Run what an item says as a command line typed at the prompt; a mistake shows as a message.
    fn run_menu_command(&mut self, id: usize, command: &str) {
        if !command.trim().is_empty() {
            self.run_line(id, command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, key: &str, command: &str) -> Raw {
        Raw { name: name.to_string(), separator: false, key: key.to_string(), command: command.to_string() }
    }

    fn rule() -> Raw {
        Raw { name: String::new(), separator: true, key: String::new(), command: String::new() }
    }

    fn menu(items: Vec<Raw>) -> Menu {
        Menu::build(items, String::new(), None)
    }

    fn kinds(m: &Menu) -> Vec<Kind> {
        m.items.iter().map(|i| i.kind).collect()
    }

    #[test]
    fn leading_trailing_and_repeated_rules_are_dropped() {
        let m = menu(vec![rule(), raw("a", "a", ""), rule(), rule(), raw("b", "b", ""), rule()]);
        assert_eq!(kinds(&m), [Kind::Item, Kind::Separator, Kind::Item]);
    }

    #[test]
    fn a_dash_disables_and_an_empty_expansion_drops_the_item() {
        let m = menu(vec![raw("-off", "x", "cmd"), raw("", "z", "cmd"), raw("on", "y", "cmd")]);
        assert_eq!(kinds(&m), [Kind::Disabled, Kind::Item]);
        assert_eq!(m.items[0].label, "off");
        assert_eq!(m.items[0].key, None, "a disabled item has no shortcut");
        assert_eq!(m.selected, Some(1), "the first item that can be chosen starts chosen");
    }

    #[test]
    fn the_start_choice_is_a_number_or_none() {
        let items = || vec![raw("a", "", ""), raw("b", "", ""), raw("c", "", "")];
        assert_eq!(Menu::build(items(), String::new(), Some("2")).selected, Some(2));
        assert_eq!(Menu::build(items(), String::new(), Some("-")).selected, None);
        assert_eq!(Menu::build(items(), String::new(), Some("9")).selected, Some(2), "past the end is the last");
    }

    #[test]
    fn shortcuts_name_keys() {
        assert_eq!(shortcut("a"), Some(Key::Byte(b'a')));
        assert_eq!(shortcut("C-x"), Some(Key::Byte(0x18)));
        assert_eq!(shortcut("Enter"), Some(Key::Enter));
        assert_eq!(shortcut("Space"), Some(Key::Byte(b' ')));
        assert_eq!(shortcut(""), None);
        assert_eq!(shortcut("é"), None);
        assert_eq!(shortcut("nonsense"), None);
    }

    #[test]
    fn movement_skips_rules_and_disabled_items_and_wraps() {
        let mut m = menu(vec![raw("a", "", ""), rule(), raw("-b", "", ""), raw("c", "", "")]);
        assert_eq!(m.selected, Some(0));
        m.key(Key::Down);
        assert_eq!(m.selected, Some(3));
        m.key(Key::Down);
        assert_eq!(m.selected, Some(0), "wraps");
        m.key(Key::Up);
        assert_eq!(m.selected, Some(3), "wraps back");
        m.key(Key::Byte(b'k'));
        assert_eq!(m.selected, Some(0));
        m.key(Key::Byte(b'j'));
        assert_eq!(m.selected, Some(3));
    }

    #[test]
    fn with_no_choice_down_takes_the_first_and_up_the_last() {
        let items = || vec![raw("a", "", ""), raw("b", "", "")];
        let mut m = Menu::build(items(), String::new(), Some("-"));
        m.key(Key::Down);
        assert_eq!(m.selected, Some(0));
        let mut m = Menu::build(items(), String::new(), Some("-"));
        m.key(Key::Up);
        assert_eq!(m.selected, Some(1));
    }

    #[test]
    fn home_end_and_pages_move_to_items_that_can_be_chosen() {
        let mut m = menu((0..12).map(|i| raw(&format!("i{i}"), "", "")).collect());
        m.key(Key::End);
        assert_eq!(m.selected, Some(11));
        m.key(Key::Home);
        assert_eq!(m.selected, Some(0));
        m.key(Key::PageDown);
        assert_eq!(m.selected, Some(5));
        m.key(Key::Byte(0x06));
        assert_eq!(m.selected, Some(10));
        m.key(Key::PageDown);
        assert_eq!(m.selected, Some(11), "stops at the end");
        m.key(Key::Byte(0x02));
        assert_eq!(m.selected, Some(6));
        m.key(Key::Byte(b'G'));
        assert_eq!(m.selected, Some(11));
        m.key(Key::Byte(b'g'));
        assert_eq!(m.selected, Some(0));
    }

    #[test]
    fn a_shortcut_chooses_and_beats_the_closing_keys() {
        let mut m = menu(vec![raw("a", "a", "one"), raw("quit", "q", "two")]);
        assert_eq!(m.key(Key::Byte(b'a')), Action::Choose(0));
        assert_eq!(m.key(Key::Byte(b'q')), Action::Choose(1));
        let mut m = menu(vec![raw("a", "a", "one")]);
        assert_eq!(m.key(Key::Byte(b'q')), Action::Close);
    }

    #[test]
    fn enter_chooses_the_choice_and_escape_closes() {
        let mut m = menu(vec![raw("a", "", ""), raw("b", "", "")]);
        m.key(Key::Down);
        assert_eq!(m.key(Key::Enter), Action::Choose(1));
        for key in [Key::Escape, Key::Byte(0x03), Key::Byte(0x07), Key::Byte(b'q')] {
            assert_eq!(m.key(key), Action::Close, "{key:?}");
        }
        let mut none = Menu::build(vec![raw("a", "", "")], String::new(), Some("-"));
        assert_eq!(none.key(Key::Enter), Action::None, "nothing is chosen");
    }

    #[test]
    fn a_disabled_item_is_never_chosen() {
        let mut m = menu(vec![raw("-off", "o", "x")]);
        assert_eq!(m.selected, None);
        assert_eq!(m.key(Key::Byte(b'o')), Action::None);
        assert_eq!(m.key(Key::Enter), Action::None);
    }

    #[test]
    fn size_is_the_widest_row_plus_the_border_and_the_spaces() {
        let mut m = menu(vec![raw("abc", "", ""), raw("abcdef", "", "")]);
        m.fit(80, true);
        assert_eq!(m.size(true), (6 + 4, 2 + 2));
        assert_eq!(m.size(false), (6 + 2, 2));
    }

    #[test]
    fn keys_widen_the_box_when_they_fit_and_are_left_out_when_they_do_not() {
        let mut m = menu(vec![raw("abc", "x", ""), raw("abcdef", "y", "")]);
        m.fit(80, true);
        assert_eq!(m.size(true).0, 6 + 1 + 3 + 4);
        assert!(m.rows().iter().all(|r| !r.key.is_empty()));
        let mut m = menu(vec![raw("abcdefgh", "x", "")]);
        m.fit(14, true);
        assert!(m.rows().iter().all(|r| r.key.is_empty()), "no room beside the name");
    }

    #[test]
    fn a_title_widens_the_box() {
        let mut m = Menu::build(vec![raw("a", "", "")], "a long title".to_string(), None);
        m.fit(80, true);
        assert_eq!(m.size(true).0, 12 + 4);
    }

    #[test]
    fn a_name_too_long_for_the_screen_is_cut_with_a_marker() {
        let mut m = menu(vec![raw(&"w".repeat(50), "", "")]);
        m.fit(20, true);
        assert_eq!(m.items[0].label, format!("{}>", "w".repeat(15)));
        assert_eq!(m.size(true).0, 20);
    }

    fn state(items: Vec<Raw>) -> MenuState {
        let mut m = menu(items);
        m.fit(80, true);
        let (w, h) = m.size(true);
        MenuState { menu: m, x: 10, y: 5, w, h, lines: "single".to_string(), styles: Default::default() }
    }

    #[test]
    fn clicks_map_to_items_borders_and_outside() {
        let s = state(vec![raw("one", "", ""), raw("two", "", "")]);
        let o = (10, 5);
        assert!(matches!(s.hit(o, 12, 6), Hit::Item(0)));
        assert!(matches!(s.hit(o, 12, 7), Hit::Item(1)));
        assert!(matches!(s.hit(o, 12, 5), Hit::Border));
        assert!(matches!(s.hit(o, 10, 6), Hit::Border));
        assert!(matches!(s.hit(o, 0, 0), Hit::Outside));
        let mut bare = state(vec![raw("one", "", "")]);
        bare.lines = "none".to_string();
        bare.h = 1;
        assert!(matches!(bare.hit(o, 10, 5), Hit::Item(0)), "no border: the first column is an item");
    }

    #[test]
    fn the_box_is_moved_inside_a_smaller_screen_and_hidden_when_it_cannot_fit() {
        let s = state(vec![raw("one", "", "")]);
        assert_eq!(s.origin(80, 0, 23), Some((10, 5)));
        assert_eq!(s.origin(s.w - 1, 0, 23), None, "wider than the screen");
        let narrow = s.origin(s.w + 2, 0, 23).unwrap();
        assert_eq!(narrow.0, 2, "pulled back from the right edge");
        assert_eq!(s.origin(80, 0, s.h + 1).map(|o| o.1), Some(1));
        assert_eq!(s.origin(80, 0, s.h - 1), None, "taller than the pane area");
    }
}
