use std::io;
use std::os::fd::{AsFd, BorrowedFd};

use nix::sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr};

use super::ctx;
use crate::log;

/// RAII raw mode on a tty fd. Restores the original settings on drop (including unwinding).
pub struct RawMode<F: AsFd> {
    fd: F,
    orig: Termios,
    raw: Termios,
}

impl<F: AsFd> RawMode<F> {
    pub fn enter(fd: F) -> io::Result<Self> {
        let orig = tcgetattr(fd.as_fd()).map_err(|e| ctx(e, "tcgetattr"))?;
        let mut raw = orig.clone();
        cfmakeraw(&mut raw);
        tcsetattr(fd.as_fd(), SetArg::TCSANOW, &raw).map_err(|e| ctx(e, "tcsetattr raw"))?;
        Ok(RawMode { fd, orig, raw })
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Give the terminal its original settings back for a while (`suspend-client`).
    pub fn pause(&self) -> io::Result<()> {
        tcsetattr(self.fd.as_fd(), SetArg::TCSANOW, &self.orig).map_err(|e| ctx(e, "tcsetattr restore"))
    }

    /// Back to raw mode after `pause`.
    pub fn resume(&self) -> io::Result<()> {
        tcsetattr(self.fd.as_fd(), SetArg::TCSANOW, &self.raw).map_err(|e| ctx(e, "tcsetattr raw"))
    }
}

impl<F: AsFd> Drop for RawMode<F> {
    fn drop(&mut self) {
        if let Err(e) = tcsetattr(self.fd.as_fd(), SetArg::TCSANOW, &self.orig) {
            log::error(&format!("restore termios: {e}"));
        }
    }
}
