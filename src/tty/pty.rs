use std::cell::Cell;
use std::ffi::CString;
use std::io;
use std::time::{Duration, Instant};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::libc;
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, execve};

use super::ctx;

nix::ioctl_write_ptr_bad!(tiocswinsz, libc::TIOCSWINSZ, Winsize);
nix::ioctl_read_bad!(tiocgwinsz, libc::TIOCGWINSZ, Winsize);

pub struct Pty {
    pub master: OwnedFd,
    pub child: Pid,
    /// Set when the program stopped reading its input: writes are refused until then.
    stalled_until: Cell<Option<Instant>>,
}

impl Pty {
    /// The path of the terminal device the program runs on (`/dev/ttys004`), empty when the system
    /// does not say.
    pub fn tty_name(&self) -> String {
        let mut buf = [0u8; 128];
        #[cfg(target_os = "macos")]
        // SAFETY: TIOCPTYGNAME writes a NUL-terminated path of at most 128 bytes into the buffer.
        let ok = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::c_ulong::from(libc::TIOCPTYGNAME), buf.as_mut_ptr()) } == 0;
        #[cfg(not(target_os = "macos"))]
        // SAFETY: ptsname_r writes a NUL-terminated path into the buffer and never past its length.
        let ok = unsafe { libc::ptsname_r(self.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } == 0;
        if !ok {
            return String::new();
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }

    /// How long one wait for room may take, how long a whole write may take, and how long writes
    /// are refused outright after one gave up.
    const IDLE: Duration = Duration::from_millis(100);
    const TOTAL: Duration = Duration::from_secs(1);
    const REFUSE: Duration = Duration::from_secs(2);

    /// Send bytes to the program. Never waits long: a program that does not read (raw mode, then
    /// nothing) costs the server at most one short wait, and for the next two seconds anything
    /// meant for it is dropped, so a pane cannot hold up the other sessions.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if let Some(until) = self.stalled_until.get() {
            if Instant::now() < until {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "the pane does not read its input; dropped"));
            }
            self.stalled_until.set(None);
        }
        match super::write_bounded(self.master.as_fd(), bytes, Self::IDLE, Self::TOTAL) {
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                self.stalled_until.set(Some(Instant::now() + Self::REFUSE));
                Err(e)
            }
            other => other,
        }
    }
}

pub fn window_size(fd: BorrowedFd<'_>) -> io::Result<Winsize> {
    let mut ws = Winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: fd is valid for the call and ws is a properly sized out-parameter.
    unsafe { tiocgwinsz(fd.as_raw_fd(), &mut ws) }.map_err(|e| ctx(e, "TIOCGWINSZ"))?;
    Ok(ws)
}

pub fn set_window_size(fd: BorrowedFd<'_>, ws: &Winsize) -> io::Result<()> {
    // SAFETY: fd is valid for the call and ws points to a valid winsize.
    unsafe { tiocswinsz(fd.as_raw_fd(), ws) }.map_err(|e| ctx(e, "TIOCSWINSZ"))?;
    Ok(())
}

pub fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    let bits = fcntl(fd, FcntlArg::F_GETFL).map_err(|e| ctx(e, "fcntl F_GETFL"))?;
    let flags = OFlag::from_bits_truncate(bits) | OFlag::O_NONBLOCK;
    fcntl(fd, FcntlArg::F_SETFL(flags)).map_err(|e| ctx(e, "fcntl F_SETFL"))?;
    Ok(())
}

/// Close `fd` in programs this process starts. Everything the server opens that is not meant
/// for a child needs this: `forkpty` and `pipe` do not set it, and a pane's shell would hold the
/// pty masters of the panes created before it and the server's signal pipe.
pub fn set_cloexec(fd: BorrowedFd<'_>) -> io::Result<()> {
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(|e| ctx(e, "fcntl F_SETFD"))?;
    Ok(())
}

/// Fork a login shell on a new pty, starting in `cwd` when given. The master side is returned
/// non-blocking and close-on-exec.
#[cfg(test)]
pub fn spawn(shell: &str, size: &Winsize, cwd: Option<&Path>) -> io::Result<Pty> {
    spawn_command(shell, None, size, cwd, &[], &[])
}

/// `spawn` with extra environment variables (they replace inherited ones of the same name), and
/// with `command` the shell runs `-c command` (not as a login shell) instead of waiting for
/// input; the pane ends when the command does.
pub fn spawn_command(
    shell: &str,
    command: Option<&str>,
    size: &Winsize,
    cwd: Option<&Path>,
    extra: &[(String, String)],
    unset: &[String],
) -> io::Result<Pty> {
    let path = CString::new(shell).map_err(|e| ctx(io::Error::other(e), "shell path"))?;
    let cwd = cwd
        .map(|p| CString::new(p.as_os_str().as_bytes()).map_err(|e| ctx(io::Error::other(e), "cwd path")))
        .transpose()?;
    const EXEC_FAILED: &[u8] = b"imux: cannot execute the shell (check $SHELL)\r\n";
    let base = shell.rsplit('/').next().unwrap_or(shell);
    let argv: Vec<CString> = match command {
        None => vec![CString::new(format!("-{base}")).map_err(|e| ctx(io::Error::other(e), "argv0"))?],
        Some(text) => vec![
            CString::new(base).map_err(|e| ctx(io::Error::other(e), "argv0"))?,
            CString::new("-c").map_err(|e| ctx(io::Error::other(e), "argv"))?,
            CString::new(text).map_err(|e| ctx(io::Error::other(e), "command"))?,
        ],
    };
    // `vars_os`: a variable that is not UTF-8 must not make every pane start panic (security
    // review P-13).
    let inherited = std::env::vars_os()
        .filter(|(k, _)| {
            let name = k.to_str();
            !name.is_some_and(|k| extra.iter().any(|(n, _)| n == k) || unset.iter().any(|u| u == k))
        })
        .map(|(k, v)| [k.as_bytes(), b"=", v.as_bytes()].concat());
    let added = extra.iter().map(|(k, v)| format!("{k}={v}").into_bytes());
    let env: Vec<CString> = inherited.chain(added).filter_map(|b| CString::new(b).ok()).collect();

    // SAFETY: helper threads (git, shell commands, autosave) may exist; the child only calls
    // chdir, signal, setrlimit, execve and _exit, and the argument and environment arrays were
    // built before the fork. `nix::unistd::execve` itself builds pointer arrays after the fork
    // (an allocation), which macOS's allocator survives; the Linux port should pass them in ready.
    match unsafe { forkpty(Some(size), None) }.map_err(|e| ctx(e, "forkpty"))? {
        ForkptyResult::Child => {
            if let Some(dir) = &cwd {
                // SAFETY: chdir is async-signal-safe and dir is a valid NUL-terminated string.
                // A failure leaves the shell in the inherited directory, which is acceptable.
                unsafe { libc::chdir(dir.as_ptr()) };
            }
            // SAFETY: signal is async-signal-safe. Rust starts with SIGPIPE ignored and an ignored
            // signal stays ignored across execve; the shell and what it starts expect the default.
            unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
            super::limits::restore_for_child();
            let _ = execve(&path, &argv, &env);
            // SAFETY: write and _exit are async-signal-safe; the buffer is a valid static slice.
            unsafe { libc::write(2, EXEC_FAILED.as_ptr().cast(), EXEC_FAILED.len()) };
            // SAFETY: _exit is async-signal-safe and avoids running parent destructors.
            unsafe { libc::_exit(127) }
        }
        ForkptyResult::Parent { child, master } => {
            set_cloexec(master.as_fd())?;
            set_nonblocking(master.as_fd())?;
            Ok(Pty { master, child, stalled_until: Cell::new(None) })
        }
    }
}

/// Block until the child exits. Used after the pty closed, so this returns promptly.
pub fn wait_blocking(child: Pid) -> io::Result<i32> {
    loop {
        match waitpid(child, None).map_err(|e| ctx(e, "waitpid"))? {
            WaitStatus::Exited(_, code) => return Ok(code),
            WaitStatus::Signaled(_, sig, _) => return Ok(128 + sig as i32),
            _ => {}
        }
    }
}

/// Reap the child if it has exited. Returns its exit code (128+signal when signaled).
pub fn try_reap(child: Pid) -> io::Result<Option<i32>> {
    match waitpid(child, Some(WaitPidFlag::WNOHANG)).map_err(|e| ctx(e, "waitpid"))? {
        WaitStatus::Exited(_, code) => Ok(Some(code)),
        WaitStatus::Signaled(_, sig, _) => Ok(Some(128 + sig as i32)),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a check: how often does spawning a program that ends at once fail, and with what?
    /// `cargo test --lib spawning_programs_that_end_at_once -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn spawning_programs_that_end_at_once() {
        let size = Winsize { ws_row: 2, ws_col: 2, ws_xpixel: 0, ws_ypixel: 0 };
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    let mut errors = Vec::new();
                    for _ in 0..400 {
                        match spawn("/usr/bin/true", &size, None) {
                            Ok(p) => drop(p),
                            Err(e) => errors.push(e.to_string()),
                        }
                    }
                    errors
                })
            })
            .collect();
        let errors: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{} failures of 3200", errors.len());
        for e in errors.iter().take(5) {
            let _ = writeln!(out, "  {e}");
        }
    }
}
