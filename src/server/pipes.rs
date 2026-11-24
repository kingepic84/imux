//! `pipe-pane`: a command that gets everything a pane prints on its standard input (`-O`, the
//! default) and/or whose standard output is typed into the pane (`-I`). The command runs under
//! `sh -c` in a process group of its own. Writing to it never blocks the loop: what it has not
//! taken yet waits in a buffer of at most 1 MiB, and a command that falls further behind than that
//! is stopped with a message instead of slowing the pane down.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use super::Server;
use super::remote::flags;
use crate::log;
use crate::session::layout::PaneId;
use crate::target::Kind;

const MAX_PENDING: usize = 1 << 20;
const MAX_PIPES: usize = 16;
/// A command that was told to stop (its input closed) gets this long before it is killed.
const GRACE: Duration = Duration::from_secs(10);
/// Bytes taken from a command's output per round, to be typed into the pane.
const READ_ROUND: usize = 64 * 1024;

pub struct PanePipe {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    pending: Vec<u8>,
    pub command: String,
}

fn set_nonblocking(fd: &impl AsRawFd) {
    // SAFETY: fcntl on a descriptor we own, only changing its O_NONBLOCK flag.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

impl PanePipe {
    fn start(command: &str, pane: PaneId, output: bool, input: bool) -> std::io::Result<PanePipe> {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .env("IMUX_ENV", "1")
            .env("IMUX_PANE_ID", pane.to_string())
            .env("IMUX_SOCKET_PATH", crate::paths::socket_path())
            .process_group(0)
            .stdin(if output { Stdio::piped() } else { Stdio::null() })
            .stdout(if input { Stdio::piped() } else { Stdio::null() })
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        if let Some(s) = &stdin {
            set_nonblocking(s);
        }
        if let Some(s) = &stdout {
            set_nonblocking(s);
        }
        Ok(PanePipe { child, stdin, stdout, pending: Vec::new(), command: command.chars().take(80).collect() })
    }

    /// What the pane printed. `Err` says why the pipe must be closed.
    pub fn feed(&mut self, data: &[u8]) -> Result<(), String> {
        if self.stdin.is_none() {
            return Ok(());
        }
        if self.pending.len() + data.len() > MAX_PENDING {
            return Err("the command does not keep up with the pane".to_string());
        }
        self.pending.extend_from_slice(data);
        self.flush()
    }

    /// Hand the command as much of the waiting output as it will take now.
    pub fn flush(&mut self) -> Result<(), String> {
        let Some(stdin) = &mut self.stdin else { return Ok(()) };
        while !self.pending.is_empty() {
            match stdin.write(&self.pending) {
                Ok(0) => return Err("the command stopped reading".to_string()),
                Ok(n) => {
                    self.pending.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("the command stopped reading: {e}")),
            }
        }
        Ok(())
    }

    /// What the command printed since the last call, to be typed into the pane.
    fn read_input(&mut self) -> Vec<u8> {
        let Some(stdout) = &mut self.stdout else { return Vec::new() };
        let mut buf = vec![0u8; READ_ROUND];
        match stdout.read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                if n == 0 {
                    self.stdout = None;
                }
                buf
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => Vec::new(),
            Err(_) => {
                self.stdout = None;
                Vec::new()
            }
        }
    }

    fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    fn signal_group(&self, signal: Signal) {
        let pid = Pid::from_raw(self.child.id() as i32);
        if let Err(e) = killpg(pid, signal) {
            log::debug(&format!("pipe-pane: {signal} to group {pid}: {e}"));
        }
    }
}

/// Pipes that were closed and may still be running, with the time they were closed.
#[derive(Default)]
pub struct Pipes {
    open: HashMap<PaneId, PanePipe>,
    closing: Vec<(PanePipe, Instant)>,
}

impl Pipes {
    pub fn feed(&mut self, pane: PaneId, data: &[u8]) -> Option<String> {
        let p = self.open.get_mut(&pane)?;
        p.feed(data).err()
    }

    pub fn has(&self, pane: PaneId) -> bool {
        self.open.contains_key(&pane)
    }

    /// Close the pipe of `pane`: the command sees the end of its input and has `GRACE` to finish,
    /// or with `kill` is hung up at once (the pane is gone).
    pub fn close(&mut self, pane: PaneId, kill: bool) -> Option<String> {
        let mut p = self.open.remove(&pane)?;
        let command = p.command.clone();
        p.stdin = None;
        p.stdout = None;
        if kill {
            p.signal_group(Signal::SIGHUP);
        }
        self.closing.push((p, Instant::now()));
        Some(command)
    }

    pub fn close_all(&mut self) {
        let panes: Vec<PaneId> = self.open.keys().copied().collect();
        for pane in panes {
            self.close(pane, true);
        }
        for (p, _) in &self.closing {
            p.signal_group(Signal::SIGHUP);
        }
    }

    /// Reap the commands that ended and stop the ones that overstayed.
    pub fn sweep(&mut self) {
        self.closing.retain_mut(|(p, since)| {
            if p.exited() {
                return false;
            }
            if since.elapsed() > GRACE {
                p.signal_group(Signal::SIGKILL);
                let _ = p.child.wait();
                return false;
            }
            true
        });
    }
}

impl Server {
    /// `pipe-pane [-IOo] [-t pane] [command]`
    pub(super) fn cmd_pipe_pane(&mut self, args: &[String], caller: Option<PaneId>) -> Result<String, String> {
        let f = flags("pipe-pane", args, "t", "IOo")?;
        let place = self.resolve(f.value('t').unwrap_or(""), Kind::Pane, caller)?;
        let id = place.pane;
        let Some(command) = self.command_line(&f.words)? else {
            if let Some(command) = self.pipes.close(id, false) {
                log::info(&format!("pane {id}: pipe closed ({command})"));
            }
            return Ok(String::new());
        };
        if f.has('o') && self.pipes.has(id) {
            return Ok(String::new());
        }
        if let Some(old) = self.pipes.close(id, false) {
            log::info(&format!("pane {id}: pipe replaced ({old})"));
        }
        if self.pipes.open.len() >= MAX_PIPES {
            return Err(format!("pipe-pane: too many pipes ({MAX_PIPES})"));
        }
        let input = f.has('I');
        let output = f.has('O') || !input;
        let pipe = PanePipe::start(&command, id, output, input).map_err(|e| format!("pipe-pane: {e}"))?;
        log::info(&format!("pane {id}: pipe to `{}` pid={} output={output} input={input}", pipe.command, pipe.child.id()));
        self.pipes.open.insert(id, pipe);
        Ok(String::new())
    }

    /// The command of a pipe could not take what the pane printed: close the pipe and say so.
    pub(super) fn pipe_failed(&mut self, pane: PaneId, why: &str) {
        self.pipes.close(pane, false);
        log::warn(&format!("pane {pane}: pipe closed: {why}"));
        let sid = self.sessions.iter().find(|s| s.window_of(pane).is_some()).map(|s| s.id);
        let watching: Vec<usize> = self.clients.iter().filter(|(_, c)| Some(c.session) == sid).map(|(id, _)| *id).collect();
        for client in watching {
            self.show_message(client, format!("pipe-pane closed: {why}"));
        }
    }

    /// Every housekeeping turn: finish the writes that blocked, type what `-I` commands printed,
    /// close pipes whose command has gone, reap the ones that were closed.
    pub(super) fn service_pipes(&mut self) {
        let ids: Vec<PaneId> = self.pipes.open.keys().copied().collect();
        for id in ids {
            let Some(p) = self.pipes.open.get_mut(&id) else { continue };
            let flushed = p.flush();
            let typed = p.read_input();
            let gone = p.exited() && p.pending.is_empty() && p.stdout.is_none();
            if !typed.is_empty()
                && let Some(pane) = self.pane_mut(id)
                && let Err(e) = pane.pty.write_typed(&typed)
            {
                log::warn(&format!("pane {id}: typing the output of the pipe: {e}"));
            }
            if let Err(why) = flushed {
                self.pipes.close(id, false);
                log::info(&format!("pane {id}: pipe closed: {why}"));
            } else if gone {
                self.pipes.close(id, false);
                log::info(&format!("pane {id}: pipe closed: the command ended"));
            }
        }
        self.pipes.sweep();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipe(command: &str, output: bool, input: bool) -> PanePipe {
        PanePipe::start(command, 7, output, input).unwrap()
    }

    fn wait_for_file(path: &std::path::Path, want: &str) -> bool {
        for _ in 0..100 {
            if std::fs::read_to_string(path).is_ok_and(|t| t == want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn output_reaches_the_command_in_order() {
        let out = std::env::temp_dir().join(format!("imux-pipe-{}", std::process::id()));
        let mut p = pipe(&format!("cat > {}", out.display()), true, false);
        p.feed(b"hello ").unwrap();
        p.feed(b"world\n").unwrap();
        p.stdin = None;
        assert!(wait_for_file(&out, "hello world\n"));
        assert!(p.exited() || p.child.wait().is_ok());
        std::fs::remove_file(out).unwrap();
    }

    #[test]
    fn a_command_that_does_not_read_gets_a_full_buffer_not_a_blocked_loop() {
        let mut p = pipe("sleep 30", true, false);
        let chunk = vec![b'x'; 64 * 1024];
        let started = Instant::now();
        let mut failed = None;
        for _ in 0..64 {
            if let Err(e) = p.feed(&chunk) {
                failed = Some(e);
                break;
            }
        }
        assert!(started.elapsed() < Duration::from_secs(5), "feeding never blocks");
        assert!(failed.is_some_and(|e| e.contains("keep up")), "the buffer limit stops it");
        p.signal_group(Signal::SIGKILL);
        let _ = p.child.wait();
    }

    #[test]
    fn a_command_that_exits_is_noticed_when_it_is_written_to() {
        let mut p = pipe("true", true, false);
        std::thread::sleep(Duration::from_millis(300));
        let mut result = Ok(());
        for _ in 0..10 {
            result = p.feed(b"data");
            if result.is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(result.is_err());
        let _ = p.child.wait();
    }

    #[test]
    fn what_a_command_prints_can_be_read_for_the_pane() {
        let mut p = pipe("printf typed", false, true);
        let mut got = Vec::new();
        for _ in 0..100 {
            got.extend(p.read_input());
            if got == b"typed" {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(got, b"typed");
        let _ = p.child.wait();
    }

    #[test]
    fn closing_stops_a_stubborn_command_after_the_grace_period_is_over() {
        let mut pipes = Pipes::default();
        pipes.open.insert(3, pipe("sleep 30", true, false));
        assert!(pipes.has(3));
        assert!(pipes.close(3, true).is_some());
        assert!(!pipes.has(3));
        assert!(pipes.close(3, true).is_none());
        for _ in 0..100 {
            pipes.sweep();
            if pipes.closing.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(pipes.closing.is_empty(), "the hung up command was reaped");
    }
}
