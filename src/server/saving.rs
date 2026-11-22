//! Writing the sessions down: the periodic autosave, the final one on SIGTERM, its removal when
//! the server ends on purpose, and the snapshot commands.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use super::{Server, capture};
use crate::log;
use crate::persist::format;
use crate::persist::writer::Job;
use crate::proto::PersistOp;

impl Server {
    /// The live sessions as a snapshot.
    fn capture(&self) -> crate::persist::Snapshot {
        let commands = self.restore_rules().commands;
        let claude: Vec<String> = self.options.text("claude-programs").split_whitespace().map(str::to_string).collect();
        capture::snapshot(
            &self.sessions,
            self.recent,
            &capture::Capture { claude: &claude, keep: &commands, shell: &self.shell(), kept: &self.kept_dirs, names: &self.pending_names },
        )
    }

    /// Tell the store how many rolling copies of the autosave to keep and how far apart.
    pub(super) fn sync_history(&self) {
        if let Some(store) = &self.store {
            store.set_history(self.options.num("autosave-history") as usize, self.options.num("autosave-history-interval"));
        }
    }

    /// A pane that was restored into the home directory keeps its saved path in the snapshot only
    /// while it has not left that directory: drop the record of one that has moved, or is gone.
    fn forget_moved_dirs(&mut self) {
        if self.kept_dirs.is_empty() {
            return;
        }
        let shell = self.shell();
        let mut here: std::collections::HashMap<crate::session::layout::PaneId, String> = std::collections::HashMap::new();
        for s in &self.sessions {
            for w in &s.windows {
                for (id, p) in &w.panes {
                    if self.kept_dirs.contains_key(id) {
                        here.insert(*id, capture::current_dir(p, &shell));
                    }
                }
            }
        }
        self.kept_dirs.retain(|id, (_, home)| here.get(id).is_some_and(|d| d == home));
    }

    /// The autosave as text with the hash that tells it from the last one, or `None` when there
    /// is nothing to save or nothing changed since the last save (`force` saves anyway). The
    /// snapshot is taken here, on the loop; only the writing is left to the thread.
    fn autosave_job(&self, force: bool) -> Option<Job> {
        self.store.as_ref()?;
        if self.sessions.is_empty() {
            return None;
        }
        let snap = self.capture();
        let mut probe = snap.clone();
        probe.saved_at = 0;
        let mut hasher = DefaultHasher::new();
        format::encode(&probe).hash(&mut hasher);
        let hash = hasher.finish();
        if !force && self.autosaved == Some(hash) {
            return None;
        }
        Some(Job { text: format::encode(&snap), hash })
    }

    /// Take in what the autosave thread has finished; with `wait`, first let it finish everything
    /// it was given, so that whatever the loop does next comes after it.
    fn settle_saves(&mut self, wait: bool) {
        let Some(writer) = &mut self.writer else { return };
        let done = if wait { writer.flush() } else { writer.poll() };
        for d in done {
            match d.result {
                Ok(()) => {
                    self.autosaved = Some(d.hash);
                    log::info("autosaved");
                }
                Err(e) => log::error(&e),
            }
        }
    }

    /// The server is exiting: let the thread finish and stop it.
    pub(super) fn stop_writer(&mut self) {
        self.settle_saves(true);
        if let Some(writer) = self.writer.take() {
            for d in writer.shutdown() {
                if let Err(e) = d.result {
                    log::error(&e);
                }
            }
        }
    }

    /// Write the autosave now, on the loop, unless nothing changed since the last one (`force`
    /// writes anyway). `Ok(false)` when there was nothing to do.
    fn write_autosave(&mut self, force: bool) -> Result<bool, String> {
        self.settle_saves(true);
        self.forget_moved_dirs();
        let Some(job) = self.autosave_job(force) else { return Ok(false) };
        let Some(store) = &self.store else { return Ok(false) };
        store.save_autosave_text(&job.text).map_err(|e| format!("autosave: {e}"))?;
        self.autosaved = Some(job.hash);
        Ok(true)
    }

    /// Called every housekeeping turn: hand the autosave to the thread when `autosave-interval`
    /// seconds have passed and the last write is done.
    pub(super) fn autosave_tick(&mut self) {
        self.settle_saves(false);
        let secs = self.options.num("autosave-interval");
        if secs == 0 || self.shutdown || self.autosave_at.elapsed() < Duration::from_secs(secs) {
            return;
        }
        if self.writer.as_ref().is_some_and(|w| w.busy()) {
            return;
        }
        self.autosave_at = Instant::now();
        self.forget_moved_dirs();
        let Some(job) = self.autosave_job(false) else { return };
        let sent = match &mut self.writer {
            Some(writer) => writer.submit(job),
            None => Err(String::new()),
        };
        if let Err(e) = sent {
            if !e.is_empty() {
                log::warn(&format!("{e}; writing the autosave on the loop from now on"));
            }
            self.writer = None;
            match self.write_autosave(false) {
                Ok(true) => log::info("autosaved"),
                Ok(false) => {}
                Err(e) => log::error(&e),
            }
        }
    }

    /// The server is being stopped from outside (SIGTERM): keep what is running for the next start.
    pub(super) fn final_save(&mut self) {
        if self.options.num("autosave-interval") == 0 {
            return;
        }
        match self.write_autosave(true) {
            Ok(_) => log::info("final autosave written"),
            Err(e) => log::error(&e),
        }
    }

    /// The server ends on purpose (`kill-server`, last pane gone): nothing to come back to.
    pub(super) fn discard_autosave(&mut self) {
        self.settle_saves(true);
        self.autosaved = None;
        if let Some(store) = &self.store
            && let Err(e) = store.delete_autosave()
        {
            log::error(&format!("remove autosave: {e}"));
        }
    }

    /// One snapshot command; the text goes back to the client that asked.
    pub(super) fn persist_op(&mut self, op: &PersistOp) -> Result<String, String> {
        self.forget_moved_dirs();
        let Some(store) = &self.store else { return Err("sessions are not saved: no state directory".to_string()) };
        match op {
            PersistOp::Save(name) => {
                if self.sessions.is_empty() {
                    return Err("nothing to save".to_string());
                }
                let snap = self.capture();
                let n = snap.sessions.len();
                match name {
                    Some(name) => store.save_named(name, &snap).map_err(|e| e.to_string())?,
                    None => {
                        self.write_autosave(true)?;
                    }
                }
                Ok(format!("saved {n} session{} as {}\n", plural(n), name.as_deref().unwrap_or("autosave")))
            }
            PersistOp::Restore(name) => {
                let snap = match name {
                    Some(name) => store.load_named(name).map_err(|e| e.to_string())?,
                    None => store
                        .load_autosave()
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "no autosave to restore".to_string())?,
                };
                if self.size.is_none() {
                    return Err("attach a client first".to_string());
                }
                let (restored, skipped) = self.restore(&snap);
                self.dirty = true;
                let left = if skipped > 0 { format!(", {skipped} left alone (name in use)") } else { String::new() };
                Ok(format!("restored {restored} session{}{left}\n", plural(restored)))
            }
            PersistOp::List => {
                let names = store.list_named().map_err(|e| e.to_string())?;
                Ok(if names.is_empty() { "no snapshots\n".to_string() } else { names.join("\n") + "\n" })
            }
            PersistOp::Delete(name) => {
                store.delete_named(name).map_err(|e| e.to_string())?;
                Ok(format!("deleted {name}\n"))
            }
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
