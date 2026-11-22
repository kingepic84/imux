//! The buffer commands: list, choose, show, set, delete, paste, save and load. The stack itself
//! is `crate::buffers`; the copy that fills it is in `copy.rs`.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use super::Server;
use super::modes::{Entry, Mode, Picker};
use crate::buffers::{self, MAX_BYTES};
use crate::input::bindings::BufferOp;
use crate::log;
use crate::tty::proc;

/// The lines `show-buffer` lists: tabs widened, other control characters dropped.
fn display_lines(text: &str) -> Vec<String> {
    text.split('\n').map(|l| l.chars().filter_map(|c| if c == '\t' { Some(' ') } else { (!c.is_control()).then_some(c) }).collect()).collect()
}

impl Server {
    pub(super) fn buffer_limit(&self) -> usize {
        self.options.num("buffer-limit") as usize
    }

    pub(super) fn buffer_op(&mut self, client: usize, op: &BufferOp) {
        match op {
            BufferOp::List => {
                let lines = self.buffers.iter().map(buffers::Buffer::summary).collect();
                self.show_text(client, "buffers", lines);
            }
            BufferOp::Choose => self.choose_buffer(client),
            BufferOp::Delete(name) => {
                if self.buffers.remove(name.as_deref()) {
                    self.dirty = true;
                } else {
                    self.no_buffer(client, name.as_deref());
                }
            }
            BufferOp::Set { name, append, data } => {
                let done = match name {
                    Some(n) => self.buffers.set(n, data, *append),
                    None => {
                        if data.len() > MAX_BYTES {
                            Err(format!("a buffer holds at most {MAX_BYTES} bytes"))
                        } else {
                            let limit = self.buffer_limit();
                            self.buffers.push(data.clone(), limit);
                            Ok(())
                        }
                    }
                };
                if let Err(e) = done {
                    self.show_message(client, e);
                }
            }
            BufferOp::Show(name) => match self.buffers.get(name.as_deref()) {
                Some(b) => {
                    let lines = display_lines(&b.text);
                    self.show_text(client, "buffer", lines);
                }
                None => self.no_buffer(client, name.as_deref()),
            },
            BufferOp::Paste { name, delete, raw, sep } => {
                self.paste_from(client, name.as_deref(), *raw, sep.as_deref().unwrap_or("\r"));
                if *delete {
                    self.buffers.remove(name.as_deref());
                }
            }
            BufferOp::Save { name, append, path } => self.save_buffer(client, name.as_deref(), *append, path),
            BufferOp::Load { name, path } => self.load_buffer(client, name.as_deref(), path),
        }
    }

    fn no_buffer(&mut self, client: usize, name: Option<&str>) {
        let text = match name {
            Some(n) => format!("no buffer {n}"),
            None => "no buffers".to_string(),
        };
        self.show_message(client, text);
    }

    /// `]` and `paste-buffer`: type a buffer into the active pane.
    pub(super) fn paste_buffer(&mut self, client: usize) {
        self.paste_from(client, None, false, "\r");
    }

    fn paste_from(&mut self, client: usize, name: Option<&str>, raw: bool, sep: &str) {
        let Some(text) = self.buffers.get(name).map(|b| b.text.clone()) else { return self.no_buffer(client, name) };
        self.paste_text(client, &text, raw, sep);
    }

    /// Type `text` into the client's active pane, inside the bracketed paste markers when the
    /// program there asked for them.
    fn paste_text(&mut self, client: usize, text: &str, raw: bool, sep: &str) {
        let Some(si) = self.client_session(client) else { return };
        let wi = self.sessions[si].current;
        let win = self.sessions[si].window();
        let Some(active) = win.panes.get(&win.active) else { return };
        let body = buffers::paste_bytes(text, sep, raw);
        if body.is_empty() || active.dead.is_some() {
            return;
        }
        for id in self.typing_targets(si, wi, win.active) {
            let Some(pane) = win.panes.get(&id) else { continue };
            let mut bytes = Vec::with_capacity(body.len() + 12);
            let bracketed = pane.term.grid.modes.bracketed_paste;
            if bracketed {
                bytes.extend_from_slice(b"\x1b[200~");
            }
            bytes.extend_from_slice(&body);
            if bracketed {
                bytes.extend_from_slice(b"\x1b[201~");
            }
            if let Err(e) = pane.pty.write(&bytes) {
                log::warn(&format!("paste to pty: {e}"));
            }
        }
    }

    /// `choose-buffer`: every buffer in a list; choosing one pastes it.
    fn choose_buffer(&mut self, client: usize) {
        if self.buffers.len() == 0 {
            return self.no_buffer(client, None);
        }
        let entries = (0..self.buffers.len()).map(Entry::Buffer).collect();
        if let Some(c) = self.clients.get_mut(&client) {
            c.enter(Mode::Picker(Picker { entries, selected: 0 }));
        }
        self.dirty = true;
    }

    /// The label of a buffer in the chooser.
    pub(super) fn buffer_label(&self, index: usize) -> String {
        self.buffers.at(index).map_or_else(String::new, buffers::Buffer::summary)
    }

    /// Chosen in the chooser: paste the buffer at that position.
    pub(super) fn paste_index(&mut self, client: usize, index: usize) {
        let Some(name) = self.buffers.at(index).map(|b| b.name.clone()) else { return };
        self.paste_from(client, Some(&name), false, "\r");
    }

    /// A path from `save-buffer` or `load-buffer`: `~/` is the home directory, a relative path
    /// starts in the directory of the program in the active pane.
    fn buffer_path(&self, client: usize, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME")
        {
            return PathBuf::from(home).join(rest);
        }
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return path;
        }
        let here = self.client_session(client).and_then(|si| {
            let win = self.sessions[si].window();
            let pane = win.panes.get(&win.active)?;
            proc::cwd(proc::foreground_pid(pane.pty.master.as_fd())?)
        });
        here.map_or(path.clone(), |dir| dir.join(&path))
    }

    fn save_buffer(&mut self, client: usize, name: Option<&str>, append: bool, path: &str) {
        let Some(text) = self.buffers.get(name).map(|b| b.text.clone()) else { return self.no_buffer(client, name) };
        let target = self.buffer_path(client, path);
        let mut options = OpenOptions::new();
        options.write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
        if append {
            options.append(true);
        } else {
            options.truncate(true);
        }
        match options.open(&target).and_then(|mut f| f.write_all(text.as_bytes())) {
            Ok(()) => log::info(&format!("client {client}: saved a buffer of {} bytes to {}", text.len(), target.display())),
            Err(e) => {
                log::warn(&format!("save-buffer {}: {e}", target.display()));
                self.show_message(client, format!("save-buffer: {}: {e}", target.display()));
            }
        }
    }

    fn load_buffer(&mut self, client: usize, name: Option<&str>, path: &str) {
        let target = self.buffer_path(client, path);
        let data = match crate::paths::read_regular(&target, MAX_BYTES + 1) {
            Ok(data) => data,
            Err(e) => {
                log::warn(&format!("load-buffer {}: {e}", target.display()));
                return self.show_message(client, format!("load-buffer: {}: {e}", target.display()));
            }
        };
        if data.len() > MAX_BYTES {
            return self.show_message(client, format!("load-buffer: {} is bigger than {MAX_BYTES} bytes", target.display()));
        }
        let text = String::from_utf8_lossy(&data).into_owned();
        log::info(&format!("client {client}: loaded a buffer of {} bytes from {}", text.len(), target.display()));
        let done = match name {
            Some(n) => self.buffers.set(n, &text, false),
            None => {
                let limit = self.buffer_limit();
                self.buffers.push(text, limit);
                Ok(())
            }
        };
        if let Err(e) = done {
            self.show_message(client, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_buffer_lines_are_plain_text() {
        assert_eq!(display_lines("a\tb\nc\x1b[31md\n"), ["a b", "c[31md", ""]);
    }
}
