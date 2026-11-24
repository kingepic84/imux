//! Names of agents (P10.6): `imux agent name PANE NAME`, `-t agent:NAME` for every target, and
//! the name kept through a restore.
//!
//! The name lives in the pane's agent record (`AgentRecord::name`), so it goes with the record:
//! released, ended, cleared, or taken over by another program. It is unique among the panes that
//! have a record. A name that matches nothing is `not_found`; it is never read as a pane, window
//! or session name. Names come from the pane or the user, so they are cut to a plain charset
//! (`agent::valid_name`) and are data everywhere they are shown.
//!
//! A restored pane has no record until its program reports again, so the name waits in
//! `Server.pending_names` and is given to the first record the pane gets, if nobody holds it then.

use super::Server;
use super::remote::Place;
use crate::errcode::tag;
use crate::log;
use crate::session::agent;
use crate::session::layout::PaneId;

impl Server {
    /// The pane whose agent has this name.
    pub(super) fn pane_named(&self, name: &str) -> Option<PaneId> {
        self.sessions
            .iter()
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.iter())
            .find(|(_, p)| p.agent.as_ref().and_then(|a| a.name.as_deref()) == Some(name))
            .map(|(id, _)| *id)
    }

    /// `agent:NAME` as a place.
    pub(super) fn place_of_agent(&self, name: &str) -> Result<Place, String> {
        agent::valid_name(name)
            .then(|| self.pane_named(name))
            .flatten()
            .and_then(|id| self.place_of_pane(id))
            .ok_or_else(|| tag("not_found", &format!("can't find agent: {name}")))
    }

    /// `api-agent-resolve WORD`: the pane of an agent named WORD, or of the pane number WORD. What
    /// `imux agent ...` uses to take a name where it takes a pane.
    pub(super) fn cmd_agent_resolve(&self, rest: &[String]) -> Result<String, String> {
        let [word] = rest else { return Err(tag("invalid_argument", "usage: api-agent-resolve name")) };
        if let Some(id) = word.trim_start_matches('%').parse::<PaneId>().ok().filter(|_| word.trim_start_matches('%').bytes().all(|b| b.is_ascii_digit())) {
            return self.place_of_pane(id).map(|_| format!("{id}\n")).ok_or_else(|| tag("not_found", &format!("no pane {id}")));
        }
        let name = word.strip_prefix("agent:").unwrap_or(word);
        self.place_of_agent(name).map(|p| format!("{}\n", p.pane))
    }

    /// `api-agent-name PANE NAME` (an empty NAME takes the name off).
    pub(super) fn cmd_agent_name(&mut self, rest: &[String]) -> Result<String, String> {
        let [pane, name] = rest else { return Err(tag("invalid_argument", "usage: api-agent-name pane name")) };
        let id = pane.trim_start_matches('%').parse::<PaneId>().map_err(|_| tag("invalid_argument", &format!("{pane:?} is not a pane number")))?;
        if !name.is_empty() && !agent::valid_name(name) {
            return Err(tag("invalid_argument", &format!("a name is a lowercase letter, then lowercase letters, digits, _ and -, at most {} characters", agent::MAX_NAME)));
        }
        let place = self.place_of_pane(id).ok_or_else(|| tag("not_found", &format!("no pane {id}")))?;
        let held = (!name.is_empty()).then(|| self.pane_named(name)).flatten();
        if let Some(other) = held.filter(|other| *other != id) {
            return Err(tag("invalid_argument", &format!("the name {name} is already the name of the agent in pane {other}")));
        }
        let pane = self.sessions[place.si].windows[place.wi].panes.get_mut(&id).ok_or_else(|| tag("not_found", &format!("no pane {id}")))?;
        let record = pane.agent.as_mut().ok_or_else(|| tag("agent_not_running", &format!("pane {id} has no agent record to name")))?;
        record.name = (!name.is_empty()).then(|| name.clone());
        self.pending_names.remove(&id);
        self.started_names.remove(&id);
        self.dirty = true;
        log::info(&format!("agent name: pane {id} is {}", if name.is_empty() { "unnamed" } else { name }));
        Ok("ok\n".to_string())
    }

    /// Give a pane that has just got its first record the name it had before a restore.
    pub(super) fn adopt_pending_name(&mut self, id: PaneId) {
        let Some(name) = self.pending_names.get(&id).cloned() else { return };
        let Some(place) = self.place_of_pane(id) else { return };
        let has_record = self.sessions[place.si].windows[place.wi].panes.get(&id).is_some_and(|p| p.agent.is_some());
        if !has_record {
            return;
        }
        // A name from `agent start` stays waiting while its program runs: the program may clear or
        // replace its record on the way up, and the name must go to the next one.
        let sticky = self.started_names.contains(&id);
        if !sticky {
            self.pending_names.remove(&id);
        }
        match self.pane_named(&name) {
            Some(holder) if holder == id => return,
            Some(_) => {
                self.forget_start_name(id);
                log::info(&format!("agent name {name} of pane {id} is taken, not restored"));
                return;
            }
            None => {}
        }
        if let Some(record) = self.sessions[place.si].windows[place.wi].panes.get_mut(&id).and_then(|p| p.agent.as_mut()) {
            record.name = Some(name);
            self.dirty = true;
        }
    }

    /// The program that `agent start` typed into a pane is over (or its name was changed by hand):
    /// stop giving the waiting name to the pane's records.
    pub(super) fn forget_start_name(&mut self, id: PaneId) {
        if self.started_names.remove(&id) {
            self.pending_names.remove(&id);
        }
    }

    /// Forget the waiting names of panes that are gone.
    pub(super) fn prune_pending_names(&mut self) {
        if self.pending_names.is_empty() {
            return;
        }
        let live: Vec<PaneId> = self.pending_names.keys().copied().filter(|id| self.place_of_pane(*id).is_some()).collect();
        self.pending_names.retain(|id, _| live.contains(id));
        self.started_names.retain(|id| live.contains(id));
    }
}
