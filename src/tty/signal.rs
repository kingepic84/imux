use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicI32, Ordering};

use nix::libc;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

use super::{ctx, pty::set_cloexec, pty::set_nonblocking, read_some};

static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = sig as u8;
        // SAFETY: write(2) is async-signal-safe; a full pipe just drops the wakeup.
        unsafe { libc::write(fd, (&byte as *const u8).cast(), 1) };
    }
}

/// Self-pipe: signal handlers write the signal number, the event loop reads it.
pub struct SignalPipe {
    pub read: OwnedFd,
    _write: OwnedFd,
}

pub fn install(signals: &[Signal]) -> io::Result<SignalPipe> {
    let (read, write) = nix::unistd::pipe().map_err(|e| ctx(e, "pipe"))?;
    set_nonblocking(read.as_fd())?;
    set_nonblocking(write.as_fd())?;
    set_cloexec(read.as_fd())?;
    set_cloexec(write.as_fd())?;
    WRITE_FD.store(write.as_raw_fd(), Ordering::Relaxed);
    let action = SigAction::new(SigHandler::Handler(on_signal), SaFlags::SA_RESTART, SigSet::empty());
    for &sig in signals {
        // SAFETY: the handler only performs an async-signal-safe write.
        unsafe { sigaction(sig, &action) }.map_err(|e| ctx(e, &format!("sigaction {sig}")))?;
    }
    Ok(SignalPipe { read, _write: write })
}

impl SignalPipe {
    pub fn drain(&self) -> io::Result<Vec<Signal>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 64];
        while let Some(n) = read_some(self.read.as_fd(), &mut buf)? {
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                if let Ok(sig) = Signal::try_from(i32::from(b)) {
                    out.push(sig);
                }
            }
        }
        Ok(out)
    }
}
