//! The agent panel on the server side: which panes are listed and in what order, the keys and
//! mouse clicks that move and choose, jumping to a pane, and what the renderer needs.

use std::time::{Duration, Instant};

use super::modes::{Key, Mode};
use super::{MOUSE_MOTION, MOUSE_WHEEL, Mouse, Server};
use crate::render::agentview::{self, Hit, Shape};
use crate::session::agent::{self, AgentRecord};
use crate::session::layout::PaneId;
use crate::session::{Pane, Session};
use crate::term::status::{Kind, State};

/// Two presses on the same row this close together are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    State,
    Recent,
    Order,
}

#[derive(Clone, Debug)]
pub struct AgentRow {
    pub session: usize,
    pub window: usize,
    pub pane: PaneId,
    pub name: String,
    pub state: State,
    pub msg: String,
    pub since: Instant,
}

/// What a client has open: the selected pane (rows come and go, so the selection is a pane, not an
/// index) and the last click for double click detection.
#[derive(Clone, Copy, Debug)]
pub struct AgentPanel {
    pub pane: Option<PaneId>,
    pub shape: Shape,
    pub last_click: Option<(PaneId, Instant)>,
    /// The dock has the keyboard (the panel itself stays on screen when this ends).
    pub docked: bool,
}

fn describe(record: &AgentRecord, state: State) -> String {
    let progress = record.progress.map_or(String::new(), |p| format!(" {p}%"));
    if let Some(msg) = record.msg.as_ref().or(record.title.as_ref()) {
        return format!("{msg}{progress}");
    }
    let what = match (state, record.kind) {
        (State::Blocked, Some(Kind::Permission)) => "needs permission",
        (State::Blocked, Some(Kind::Question)) => "has a question",
        (State::Blocked, Some(Kind::Auth)) => "needs a login",
        (State::Blocked, None) => "waiting for you",
        (State::Working, _) => "working",
        (State::Done, _) => "finished",
        (State::Error, _) => "failed",
        (State::Idle | State::Clear, _) => "idle",
    };
    match &record.app {
        Some(app) => format!("{app}: {what}{progress}"),
        None => format!("{what}{progress}"),
    }
}

fn age(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

/// Every pane with an agent record, in the order `sort` asks for. `client` decides which finished
/// panes still count as unseen.
pub fn build_rows(sessions: &[Session], client: usize, now: Instant, sort: Sort) -> Vec<AgentRow> {
    let mut rows = Vec::new();
    for session in sessions {
        for (w, window) in session.windows.iter().enumerate() {
            let mut ids: Vec<PaneId> = window.panes.iter().filter(|(_, p)| p.agent.is_some()).map(|(id, _)| *id).collect();
            ids.sort_unstable();
            let several = ids.len() > 1;
            let base = window
                .meta
                .iter()
                .find(|(k, _)| k == "worktree.name")
                .map_or_else(|| window.name.clone(), |(_, v)| v.clone());
            for (n, id) in ids.into_iter().enumerate() {
                let Some(record) = window.panes.get(&id).and_then(|p| p.agent.as_ref()) else { continue };
                let state = agent::shown_to(record, Some(client), now);
                let mut name = if sessions.len() > 1 { format!("{}:{base}", session.name) } else { base.clone() };
                if several {
                    name = format!("{name}.{}", n + 1);
                }
                if let Some(given) = &record.name {
                    name.clone_from(given);
                }
                rows.push(AgentRow {
                    session: session.id,
                    window: w,
                    pane: id,
                    name,
                    state,
                    msg: describe(record, state),
                    since: record.since,
                });
            }
        }
    }
    match sort {
        Sort::Order => {}
        Sort::Recent => rows.sort_by_key(|r| std::cmp::Reverse(r.since)),
        Sort::State => rows.sort_by_key(|r| (std::cmp::Reverse(agent::rank(r.state)), std::cmp::Reverse(r.since))),
    }
    rows
}

/// The last `n` non-empty lines of a pane's visible screen, trailing blanks trimmed.
pub fn last_lines(pane: &Pane, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = pane
        .term
        .grid
        .screen
        .iter()
        .map(|row| crate::term::cell::Cell::text_for_copy(&row.cells).trim_end().to_string())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

impl Server {
    fn agent_sort(&self) -> Sort {
        match self.options.text("agent-panel-sort") {
            "recent" => Sort::Recent,
            "order" => Sort::Order,
            _ => Sort::State,
        }
    }

    fn agent_shape(&self) -> Shape {
        match self.options.text("agent-panel") {
            "full" => Shape::Full,
            _ => Shape::Overlay {
                w: self.options.num("agent-overlay-width") as usize,
                h: self.options.num("agent-overlay-height") as usize,
            },
        }
    }

    pub(super) fn agent_rows(&self, client: usize) -> Vec<AgentRow> {
        let rows = build_rows(&self.sessions, client, Instant::now(), self.agent_sort());
        // The query of an open panel narrows the rows, best match first.
        let query = self
            .clients
            .get(&client)
            .filter(|c| matches!(c.mode, Mode::Agents(_)))
            .and_then(|c| c.filter.as_ref())
            .map(|f| f.query())
            .filter(|q| !q.trim().is_empty());
        let Some(query) = query else { return rows };
        let texts: Vec<String> = rows.iter().map(|r| format!("{} {}", r.name, r.msg)).collect();
        crate::fuzzy::filter(&texts, &query).into_iter().filter_map(|(i, _)| rows.get(i).cloned()).collect()
    }

    /// The text drawn on the panel's title row while a filter is open.
    pub(super) fn agent_title(&self, client: usize) -> String {
        match self.clients.get(&client).filter(|c| matches!(c.mode, Mode::Agents(_))).and_then(|c| c.filter.as_ref()) {
            Some(f) => format!("agents /{}{}", f.query(), if f.typing { "_" } else { "" }),
            None => "agents".to_string(),
        }
    }

    /// Rows to set aside for the preview, 0 when it is off.
    fn preview_lines(&self) -> usize {
        if self.options.flag("agent-panel-preview") { self.options.num("agent-panel-preview-lines") as usize } else { 0 }
    }

    /// `agent-panel`: open the panel on the pane the client is looking at, or the most urgent one.
    /// As a dock it toggles: the first press shows it and gives it the keyboard, a press while it
    /// has the keyboard hides it, and a press while it is shown without the keyboard focuses it.
    pub(super) fn open_agent_panel(&mut self, client: usize) {
        let rows = self.agent_rows(client);
        let docked = self.options.text("agent-panel") == "dock";
        let focused = matches!(self.clients.get(&client).map(|c| &c.mode), Some(Mode::Agents(p)) if p.docked);
        if docked && focused {
            self.set_dock(false);
            return self.close_agent_panel(client);
        }
        if rows.is_empty() && !docked {
            return self.show_message(client, "no agents reporting".to_string());
        }
        let shape = self.agent_shape();
        let here = self.client_session(client).map(|si| {
            let s = &self.sessions[si];
            s.window().active
        });
        let pane = here.filter(|p| rows.iter().any(|r| r.pane == *p)).or_else(|| rows.first().map(|r| r.pane));
        if docked {
            self.set_dock(true);
            let fits = self.client_session(client).is_some_and(|si| {
                self.size_of(si).is_some_and(|size| self.dock_rect(size, self.sessions[si].status_rows).is_some())
            });
            if !fits {
                self.set_dock(false);
                return self.show_message(client, "the dock does not fit on this screen".to_string());
            }
        }
        if let Some(c) = self.clients.get_mut(&client) {
            c.enter(Mode::Agents(AgentPanel { pane, shape, last_click: None, docked }));
        }
        self.dirty = true;
    }

    /// Show or hide the dock; every pane is laid out again for the room it leaves.
    fn set_dock(&mut self, open: bool) {
        self.dock_open = open;
        for c in self.clients.values_mut() {
            c.renderer.invalidate();
        }
        self.relayout();
        self.dirty = true;
    }

    fn view_rows(&self, rows: &[AgentRow]) -> Vec<agentview::Row> {
        let now = Instant::now();
        rows.iter()
            .map(|r| {
                let glyph = match self.plain_glyph(Some(r.state)) {
                    g if g.is_empty() => "\u{b7}".to_string(),
                    g => g,
                };
                let style = match r.state {
                    State::Working => "agent-style-working",
                    State::Blocked => "agent-style-blocked",
                    State::Done => "agent-style-done",
                    State::Error => "agent-style-error",
                    State::Idle | State::Clear => "",
                };
                let glyph_style = if style.is_empty() {
                    crate::term::cell::Style::default()
                } else {
                    self.options.style(style).apply(crate::term::cell::Style {
                        fg: crate::render::status::TEXT,
                        bg: crate::render::status::BAR,
                        attrs: 0,
                    })
                };
                agentview::Row { glyph, glyph_style, name: r.name.clone(), msg: r.msg.clone(), age: age(now.duration_since(r.since)) }
            })
            .collect()
    }

    /// Everything the renderer needs for a client's open overlay or full panel.
    pub(super) fn agent_view(&self, client: usize) -> Option<AgentDraw> {
        let Some(Mode::Agents(panel)) = self.clients.get(&client).map(|c| &c.mode) else { return None };
        if panel.docked {
            return None;
        }
        let rows = self.agent_rows(client);
        let selected = panel.pane.and_then(|p| rows.iter().position(|r| r.pane == p)).unwrap_or(0);
        let preview_lines = self.preview_lines();
        let preview = rows
            .get(selected)
            .and_then(|r| self.sessions.iter().find(|s| s.id == r.session)?.windows.get(r.window)?.panes.get(&r.pane))
            .map(|p| last_lines(p, preview_lines))
            .unwrap_or_default();
        Some(AgentDraw { shape: panel.shape, rows: self.view_rows(&rows), selected, preview_lines, preview })
    }

    fn dock_edge(&self) -> agentview::Edge {
        match self.options.text("agent-dock-position") {
            "left" => agentview::Edge::Left,
            "top" => agentview::Edge::Top,
            "bottom" => agentview::Edge::Bottom,
            _ => agentview::Edge::Right,
        }
    }

    fn dock_area(&self, session_index: usize) -> Option<agentview::Area> {
        let r = self.dock_rect(self.size_of(session_index)?, self.sessions[session_index].status_rows)?;
        Some(agentview::Area { x: usize::from(r.x), y: usize::from(r.y), w: usize::from(r.w), h: usize::from(r.h) })
    }

    /// The dock as drawn for a client looking at session index `si`: none unless it is open and
    /// fits. The selection shows only while the dock has this client's keyboard.
    pub(super) fn dock_view(&self, client: usize, si: usize) -> Option<DockDraw> {
        if !self.dock_open {
            return None;
        }
        let rect = self.dock_area(si)?;
        let rows = self.agent_rows(client);
        let selected = match self.clients.get(&client).map(|c| &c.mode) {
            Some(Mode::Agents(p)) if p.docked => Some(p.pane.and_then(|p| rows.iter().position(|r| r.pane == p)).unwrap_or(0)),
            _ => None,
        };
        Some(DockDraw { rect, edge: self.dock_edge(), rows: self.view_rows(&rows), selected })
    }

    /// A click in the dock while the panes have the keyboard: jump, or with `doubleclick` focus
    /// the dock on that row.
    pub(super) fn dock_click(&mut self, client: usize, si: usize, m: Mouse) {
        let Some(rect) = self.dock_area(si) else { return };
        let rows = self.agent_rows(client);
        let Some(index) = agentview::dock_hit(rect, self.dock_edge(), rows.len(), 0, usize::from(m.x), usize::from(m.y)) else { return };
        let Some(row) = rows.get(index) else { return };
        if self.options.text("agent-panel-jump") == "click" {
            return self.agents_jump(client, row.session, row.window, row.pane);
        }
        let shape = self.agent_shape();
        if let Some(c) = self.clients.get_mut(&client) {
            c.enter(Mode::Agents(AgentPanel { pane: Some(row.pane), shape, last_click: Some((row.pane, Instant::now())), docked: true }));
        }
        self.dirty = true;
    }

    /// Keys while the panel is open.
    pub(super) fn agents_key(&mut self, client: usize, key: Key) {
        let rows = self.agent_rows(client);
        let Some(Mode::Agents(panel)) = self.clients.get(&client).map(|c| &c.mode) else { return };
        let panel = *panel;
        let last = rows.len().saturating_sub(1);
        let at = panel.pane.and_then(|p| rows.iter().position(|r| r.pane == p)).unwrap_or(0);
        let to = match key {
            Key::Up | Key::Byte(b'k') => at.saturating_sub(1),
            Key::Down | Key::Byte(b'j') => (at + 1).min(last),
            Key::PageUp => at.saturating_sub(10),
            Key::PageDown => (at + 10).min(last),
            Key::Home | Key::Byte(b'g') => 0,
            Key::End | Key::Byte(b'G') => last,
            Key::Enter => {
                if let Some(row) = rows.get(at) {
                    return self.agents_jump(client, row.session, row.window, row.pane);
                }
                return self.close_agent_panel(client);
            }
            Key::Escape | Key::Byte(b'q' | 0x03) => return self.close_agent_panel(client),
            Key::Byte(b'A') if panel.docked => return self.open_agent_panel(client),
            _ => at,
        };
        self.set_agent_selection(client, rows.get(to).map(|r| r.pane));
    }

    pub(super) fn set_agent_selection(&mut self, client: usize, pane: Option<PaneId>) {
        if let Some(Mode::Agents(p)) = self.clients.get_mut(&client).map(|c| &mut c.mode) {
            p.pane = pane;
            self.dirty = true;
        }
    }

    fn close_agent_panel(&mut self, client: usize) {
        if let Some(c) = self.clients.get_mut(&client) {
            c.mode = Mode::Normal;
        }
        self.dirty = true;
    }

    /// Leave the panel and show the pane: its session, window and focus.
    fn agents_jump(&mut self, client: usize, session: usize, window: usize, pane: PaneId) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) {
            s.select(window);
            s.window_mut().focus(pane);
            self.attach(client, session);
        } else {
            self.close_agent_panel(client);
        }
    }

    /// Clicks and the wheel while the panel is open.
    pub(super) fn agents_mouse(&mut self, client: usize, m: Mouse) {
        let Some(c) = self.clients.get(&client) else { return };
        let (Some(size), Mode::Agents(panel)) = (c.size, &c.mode) else { return };
        let panel = *panel;
        if m.code & MOUSE_WHEEL != 0 {
            if !m.release {
                let key = if m.code & 1 == 0 { Key::Up } else { Key::Down };
                self.agents_key(client, key);
            }
            return;
        }
        if m.release || m.code & MOUSE_MOTION != 0 || m.code & 3 != 0 {
            return;
        }
        let rows = self.agent_rows(client);
        let selected = panel.pane.and_then(|p| rows.iter().position(|r| r.pane == p)).unwrap_or(0);
        let hit = if panel.docked {
            let area = self.client_session(client).and_then(|si| self.dock_area(si));
            match area {
                Some(a) => match agentview::dock_hit(a, self.dock_edge(), rows.len(), selected, usize::from(m.x), usize::from(m.y)) {
                    Some(i) => Hit::Row(i),
                    None if usize::from(m.x) >= a.x && usize::from(m.x) < a.x + a.w && usize::from(m.y) >= a.y && usize::from(m.y) < a.y + a.h => Hit::Border,
                    None => Hit::Outside,
                },
                None => Hit::Outside,
            }
        } else {
            let g = agentview::geometry(panel.shape, usize::from(size.cols), usize::from(size.rows), rows.len(), selected, self.preview_lines());
            agentview::hit(&g, rows.len(), usize::from(m.x), usize::from(m.y))
        };
        match hit {
            Hit::Row(index) => {
                let Some(row) = rows.get(index) else { return };
                let double = panel.last_click.is_some_and(|(p, t)| p == row.pane && t.elapsed() <= DOUBLE_CLICK);
                if self.options.text("agent-panel-jump") == "click" || double {
                    return self.agents_jump(client, row.session, row.window, row.pane);
                }
                if let Some(Mode::Agents(p)) = self.clients.get_mut(&client).map(|c| &mut c.mode) {
                    p.pane = Some(row.pane);
                    p.last_click = Some((row.pane, Instant::now()));
                }
            }
            Hit::Outside => return self.close_agent_panel(client),
            Hit::Border => {}
        }
        self.dirty = true;
    }
}

/// The dock as the renderer draws it.
pub struct DockDraw {
    pub rect: agentview::Area,
    pub edge: agentview::Edge,
    pub rows: Vec<agentview::Row>,
    pub selected: Option<usize>,
}

/// A client's panel as the renderer draws it.
pub struct AgentDraw {
    pub shape: Shape,
    pub rows: Vec<agentview::Row>,
    pub selected: usize,
    pub preview_lines: usize,
    pub preview: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::layout::PaneId;
    use crate::session::{Pane, Window};
    use crate::term::Terminal;
    use crate::term::status::Report;
    use crate::tty::pty;

    fn pane(state: Option<State>, msg: Option<&str>) -> Pane {
        let size = nix::pty::Winsize { ws_row: 2, ws_col: 2, ws_xpixel: 0, ws_ypixel: 0 };
        // Many tests start ptys at once while the machine may be busy: try a few times, and say
        // why when it still fails (this was a one-off failure under load, cause not captured).
        let pty = (0..5)
            .find_map(|attempt| match pty::spawn("/usr/bin/true", &size, None) {
                Ok(p) => Some(Ok(p)),
                Err(e) if attempt == 4 => Some(Err(e)),
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    None
                }
            })
            .expect("a result after five tries")
            .unwrap_or_else(|e| panic!("cannot start a pty for the test: {e}"));
        let mut agent_slot = None;
        if let Some(state) = state {
            let report = Report { state, kind: None, progress: None, app: None, title: None, msg: msg.map(str::to_string), id: None };
            agent::apply(&mut agent_slot, report, Instant::now());
        }
        Pane { pty, term: Terminal::new(10, 3, 0), copy: None, agent: agent_slot, typeahead: None, dead: None, start: None, input_off: false }
    }

    fn window(name: &str, id: PaneId, state: Option<State>) -> Window {
        Window::new(name.to_string(), id, pane(state, None))
    }

    fn sessions() -> Vec<Session> {
        let mut a = Session::new(0, "main".into(), window("one", 0, Some(State::Working)));
        let mut w = window("ticket", 1, Some(State::Blocked));
        w.meta.push(("worktree.name".into(), "ABC-1".into()));
        a.add(w);
        a.add(window("plain", 2, None));
        a.add(window("done", 3, Some(State::Done)));
        vec![a]
    }

    #[test]
    fn only_panes_with_a_record_are_listed_and_worktrees_use_their_ticket_name() {
        let rows = build_rows(&sessions(), 1, Instant::now(), Sort::Order);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["one", "ABC-1", "done"]);
    }

    #[test]
    fn the_state_order_puts_blocked_first_then_working_then_done() {
        let rows = build_rows(&sessions(), 1, Instant::now(), Sort::State);
        let states: Vec<_> = rows.iter().map(|r| r.state).collect();
        assert_eq!(states, [State::Blocked, State::Working, State::Done]);
    }

    #[test]
    fn several_sessions_get_the_session_name_in_front() {
        let mut all = sessions();
        all.push(Session::new(1, "other".into(), window("x", 9, Some(State::Idle))));
        let rows = build_rows(&all, 1, Instant::now(), Sort::Order);
        assert!(rows.iter().all(|r| r.name.contains(':')), "{rows:?}");
        assert!(rows.iter().any(|r| r.name == "other:x"));
    }

    #[test]
    fn a_window_with_two_agent_panes_numbers_them() {
        let mut s = Session::new(0, "s".into(), window("w", 0, Some(State::Working)));
        let extra = pane(Some(State::Idle), None);
        s.window_mut().panes.insert(5, extra);
        let rows = build_rows(&[s], 1, Instant::now(), Sort::Order);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["w.1", "w.2"]);
    }

    #[test]
    fn a_finished_pane_a_client_has_seen_is_listed_as_idle_for_that_client() {
        let mut all = sessions();
        let w = &mut all[0].windows[3];
        let id = w.active;
        w.panes.get_mut(&id).unwrap().agent.as_mut().unwrap().seen_by.insert(7);
        let seen = build_rows(&all, 7, Instant::now(), Sort::Order);
        let unseen = build_rows(&all, 8, Instant::now(), Sort::Order);
        assert_eq!(seen.last().unwrap().state, State::Idle);
        assert_eq!(unseen.last().unwrap().state, State::Done);
    }

    #[test]
    fn the_message_comes_from_the_report_or_a_plain_word() {
        let mut all = sessions();
        let id = all[0].windows[0].active;
        let mut slot = None;
        let report = Report {
            state: State::Blocked,
            kind: Some(Kind::Permission),
            progress: None,
            app: None,
            title: None,
            msg: Some("approve Bash".into()),
            id: None,
        };
        agent::apply(&mut slot, report, Instant::now());
        all[0].windows[0].panes.get_mut(&id).unwrap().agent = slot;
        let rows = build_rows(&all, 1, Instant::now(), Sort::Order);
        assert_eq!(rows[0].msg, "approve Bash");
        assert_eq!(rows[1].msg, "waiting for you");
        assert_eq!(rows[2].msg, "finished");
    }

    #[test]
    fn age_is_seconds_minutes_or_hours() {
        assert_eq!(age(Duration::from_secs(5)), "5s");
        assert_eq!(age(Duration::from_secs(125)), "2m");
        assert_eq!(age(Duration::from_secs(7300)), "2h");
    }

    #[test]
    fn the_preview_is_the_last_non_empty_lines_of_the_screen() {
        let mut p = pane(None, None);
        p.term.feed(b"one\r\ntwo\r\n");
        assert_eq!(last_lines(&p, 5), ["one", "two"]);
        assert_eq!(last_lines(&p, 1), ["two"]);
        assert!(last_lines(&pane(None, None), 4).is_empty());
    }
}
