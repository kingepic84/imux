//! Who made a pane, and what a program in a pane may do because of it (P10.1).
//!
//! A pane the user made (key, prompt, configuration, wizard) has no record. A pane a
//! program in another pane asked for over the socket (`split-window`, `new-window`) records that
//! pane as its maker, taken from the peer of the connection, never from anything a pane printed.
//! A restore numbers the panes anew and gives back the makers that the snapshot holds, by position
//! (`capture::SPAWN_KEY`, `restore::relink_spawned`), so a program keeps its panes through a restart.
//! The records form a tree; `command-from-panes` says what a program in a user-made pane may do:
//!
//! - `off`: look only (the lists, `display-message`, its own pane's text, `wait-for`).
//! - `spawned`: act on the panes it spawned, directly or through them, and on nothing else.
//! - `on`: everything the socket allows a pane (as before).
//!
//! A program in a spawned pane is always held to its own subtree, whatever the option says, and
//! to nothing at all while the option is `off`: it never has more than the pane that made it.
//! Spawning is always without taking focus, at most `spawned-depth` levels below a user-made
//! pane and `spawned-panes-max` live panes under one. Text-running and configuration commands
//! (`run-shell`, `pipe-pane`, options, hooks, bindings, `source-file`, `set-environment`, menus)
//! are refused below `on`: hooks and bindings outlive the pane and fire in the user's context.
//! `display-popup` is not part of this ladder (`popup-from-panes`).

use std::collections::HashSet;

use super::remote::{NEW_WINDOW_FLAGS, SPLIT_WINDOW_FLAGS, flags, is_read_only};
use super::{Server, create};
use crate::log;
use crate::session::layout::PaneId;
use crate::target::Kind;

/// A chain of makers is never longer than this, whatever the option says.
pub(super) const CHAIN_MAX: usize = 64;

/// What a program that talks to the socket from a pane may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Power {
    /// Everything the socket allows: the client's own commands, or `command-from-panes on`.
    Full,
    /// Act on the panes this pane spawned, and look.
    Subtree(PaneId),
    /// Look only.
    Look,
}

/// Commands that change nothing: allowed to every pane caller at every tier.
fn looks_only(name: &str) -> bool {
    is_read_only(name) || matches!(name, "display-message" | "display" | "wait-for" | "wait")
}

/// Whether a command from a pane is worth a log line: everything that is not a look.
pub(super) fn is_write(name: &str) -> bool {
    !looks_only(name) && !matches!(name, "capture-pane" | "capturep" | "api-pane-wait")
}

impl Server {
    /// The pane, the pane that made it, the one that made that, and so on, at most `CHAIN_MAX`.
    pub(super) fn maker_chain(&self, pane: PaneId) -> Vec<PaneId> {
        let mut chain = vec![pane];
        while chain.len() < CHAIN_MAX
            && let Some(by) = chain.last().and_then(|p| self.spawned_by.get(p)).copied()
        {
            chain.push(by);
        }
        chain
    }

    /// What a command from `caller` (`None` is the client, or a process that is in no pane) may do.
    pub(super) fn power_of(&self, caller: Option<PaneId>) -> Power {
        let Some(me) = caller else { return Power::Full };
        match (self.spawned_by.contains_key(&me), self.options.text("command-from-panes")) {
            (_, "off") => Power::Look,
            (false, "on") => Power::Full,
            _ => Power::Subtree(me),
        }
    }

    /// Whether `pane` is `root` (unless `strict`) or was spawned by it, directly or not.
    pub(super) fn in_subtree(&self, root: PaneId, pane: PaneId, strict: bool) -> bool {
        self.maker_chain(pane).iter().skip(usize::from(strict)).any(|p| *p == root)
    }

    /// Remember that `by` made `new`. Records of panes that are gone and have no live descendant
    /// are dropped first, so the table stays as small as the live panes (ids are never reused).
    pub(super) fn record_spawn(&mut self, new: PaneId, by: PaneId) {
        self.prune_spawned();
        self.spawned_by.insert(new, by);
    }

    fn prune_spawned(&mut self) {
        let live: Vec<PaneId> = self.spawned_by.keys().copied().filter(|p| self.place_of_pane(*p).is_some()).collect();
        let mut keep = HashSet::new();
        for pane in live {
            keep.extend(self.maker_chain(pane).into_iter().filter(|p| self.spawned_by.contains_key(p)));
        }
        self.spawned_by.retain(|pane, _| keep.contains(pane));
    }

    /// Refuse a spawn that would go too deep, or pile up too many live panes under one user-made pane.
    fn check_spawn_limits(&self, name: &str, me: PaneId) -> Result<(), String> {
        let chain = self.maker_chain(me);
        let depth = self.options.num("spawned-depth") as usize;
        if chain.len() > depth {
            return Err(format!("{name}: limit reached: a pane may spawn at most {depth} levels below a pane you made (spawned-depth)"));
        }
        let root = chain.last().copied().unwrap_or(me);
        let max = self.options.num("spawned-panes-max") as usize;
        let live = self.spawned_by.keys().filter(|p| self.place_of_pane(**p).is_some() && self.maker_chain(**p).last() == Some(&root)).count();
        if live >= max {
            return Err(format!("{name}: limit reached: {live} panes are already spawned under %{root} (spawned-panes-max {max})"));
        }
        Ok(())
    }

    fn outside(&self, name: &str, pane: PaneId) -> String {
        format!("{name}: pane %{pane} is not one this pane spawned (command-from-panes {} holds a pane to the panes it spawned)", self.options.text("command-from-panes"))
    }

    /// Whether `name` with `rest` may run from pane `me` when it is held to its subtree, and the
    /// words to run: the same, except that a spawn gets `-d` so the user's focus never moves.
    pub(super) fn gate_subtree(&self, me: PaneId, name: &str, rest: &[String]) -> Result<Vec<String>, String> {
        let same = || std::iter::once(name.to_string()).chain(rest.iter().cloned()).collect::<Vec<String>>();
        let held = || format!("{name}: not allowed from a pane that may only act on the panes it spawned (command-from-panes {})", self.options.text("command-from-panes"));
        match name {
            _ if looks_only(name) => Ok(same()),
            "capture-pane" | "capturep" => {
                let f = flags(name, rest, "tSEb", "pJeNCq")?;
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, Some(me))?;
                if !self.in_subtree(me, place.pane, false) {
                    return Err(self.outside(name, place.pane));
                }
                Ok(same())
            }
            "display-popup" | "popup" => {
                if !self.options.flag("popup-from-panes") {
                    return Err(format!("{name}: not allowed from inside a pane (set popup-from-panes or command-from-panes on to allow it)"));
                }
                Ok(same())
            }
            "split-window" | "splitw" => {
                let f = flags(name, rest, SPLIT_WINDOW_FLAGS.0, SPLIT_WINDOW_FLAGS.1)?;
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, Some(me))?;
                if !self.in_subtree(me, place.pane, false) {
                    return Err(self.outside(name, place.pane));
                }
                self.check_spawn_limits(name, me)?;
                Ok(std::iter::once(name.to_string()).chain(std::iter::once("-d".to_string())).chain(rest.iter().cloned()).collect())
            }
            "new-window" | "neww" => {
                let f = flags(name, rest, NEW_WINDOW_FLAGS.0, NEW_WINDOW_FLAGS.1)?;
                let (session, _) = create::new_window_target(f.value('t').unwrap_or(""));
                let wanted = self.resolve(session, Kind::Session, Some(me))?;
                if self.place_of_pane(me).map(|p| p.si) != Some(wanted.si) {
                    return Err(format!("{name}: a pane may only open windows in its own session (command-from-panes {})", self.options.text("command-from-panes")));
                }
                self.check_spawn_limits(name, me)?;
                Ok(std::iter::once(name.to_string()).chain(std::iter::once("-d".to_string())).chain(rest.iter().cloned()).collect())
            }
            "send-keys" | "send" | "respawn-pane" | "respawnp" | "kill-pane" | "killp" => {
                let (valued, plain) = match name {
                    "send-keys" | "send" => ("tN", "lXRMFH"),
                    "respawn-pane" | "respawnp" => ("tc", "k"),
                    _ => ("t", "a"),
                };
                let f = flags(name, rest, valued, plain)?;
                if f.has('a') {
                    return Err(held());
                }
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, Some(me))?;
                if !self.in_subtree(me, place.pane, true) {
                    return Err(self.outside(name, place.pane));
                }
                Ok(same())
            }
            "api-agent-name" | "api-agent-session" => {
                let id = rest.first().and_then(|p| p.trim_start_matches('%').parse::<PaneId>().ok());
                match id {
                    Some(id) if self.in_subtree(me, id, false) => Ok(same()),
                    Some(id) => Err(crate::errcode::tag("permission_denied", &self.outside(name, id))),
                    None => Err(crate::errcode::tag("invalid_argument", "usage: api-agent-name pane name")),
                }
            }
            "api-agent-start" => {
                let spec = rest.get(2).map(String::as_str).unwrap_or("");
                if spec.is_empty() {
                    return Err(crate::errcode::tag("invalid_argument", "from a pane, say which pane (--pane): spawn one first with split-window"));
                }
                let place = self.resolve(spec, Kind::Pane, Some(me)).map_err(|e| if crate::errcode::split(&e).0.is_some() { e } else { crate::errcode::tag("not_found", &e) })?;
                if !self.in_subtree(me, place.pane, true) {
                    return Err(crate::errcode::tag("permission_denied", &self.outside(name, place.pane)));
                }
                Ok(same())
            }
            "api-pane-wait" => {
                let id = rest.first().and_then(|p| p.trim_start_matches('%').parse::<PaneId>().ok());
                match id {
                    Some(id) if self.in_subtree(me, id, false) => Ok(same()),
                    Some(id) => Err(crate::errcode::tag("permission_denied", &self.outside(name, id))),
                    None => Err(crate::errcode::tag("invalid_argument", "usage: api-pane-wait pane ...")),
                }
            }
            "api-pane-run" => {
                let id = rest.first().and_then(|p| p.trim_start_matches('%').parse::<PaneId>().ok());
                match id {
                    Some(id) if self.in_subtree(me, id, true) => Ok(same()),
                    Some(id) => Err(crate::errcode::tag("permission_denied", &self.outside(name, id))),
                    None => Err(crate::errcode::tag("invalid_argument", "usage: api-pane-run pane text")),
                }
            }
            "kill-window" | "killw" => {
                let f = flags(name, rest, "t", "a")?;
                if f.has('a') {
                    return Err(held());
                }
                let place = self.resolve(f.value('t').unwrap_or(""), Kind::Window, Some(me))?;
                let window = &self.sessions[place.si].windows[place.wi];
                if let Some(other) = window.panes.keys().copied().find(|p| !self.in_subtree(me, *p, true)) {
                    return Err(self.outside(name, other));
                }
                Ok(same())
            }
            _ => Err(held()),
        }
    }

    /// One log line for a command from a pane that is not a look: who, what, at which target and
    /// how it went. The target is what the caller wrote, cleaned and cut.
    pub(super) fn log_pane_write(&self, me: PaneId, power: Power, words: &[String], result: &Result<String, String>) {
        let Some((name, rest)) = words.split_first() else { return };
        let target = rest.windows(2).find(|w| w[0] == "-t").map(|w| w[1].as_str()).unwrap_or("");
        let target: String = crate::sanitize::clean(target).chars().take(64).collect();
        let how = match power {
            Power::Full => "full",
            Power::Subtree(_) => "subtree",
            Power::Look => "look",
        };
        let outcome = if result.is_ok() { "ok" } else { "failed" };
        log::info(&format!("pane-write caller=%{me} power={how} command={name} target={target:?} result={outcome}"));
    }
}
