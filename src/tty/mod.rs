pub mod limits;
pub mod proc;
pub mod pty;
pub mod raw;
pub mod signal;

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

pub fn ctx(err: impl Into<io::Error>, what: &str) -> io::Error {
    let err = err.into();
    io::Error::new(err.kind(), format!("{what}: {err}"))
}

/// Write the whole buffer to a possibly non-blocking fd, waiting for POLLOUT on EAGAIN.
pub fn write_all(fd: BorrowedFd<'_>, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        match nix::unistd::write(fd, buf) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0")),
            Ok(n) => buf = &buf[n..],
            Err(Errno::EINTR) => {}
            Err(Errno::EAGAIN) => {
                let mut fds = [PollFd::new(fd, PollFlags::POLLOUT)];
                match poll(&mut fds, PollTimeout::NONE) {
                    Ok(_) | Err(Errno::EINTR) => {}
                    Err(e) => return Err(ctx(e, &format!("poll fd {}", fd.as_raw_fd()))),
                }
            }
            Err(e) => return Err(ctx(e, &format!("write fd {}", fd.as_raw_fd()))),
        }
    }
    Ok(())
}

/// Like `write_all`, for a descriptor whose reader may never read (the input side of a pane's
/// pty): waits at most `idle` for room each time the descriptor is full and `total` for the whole
/// buffer, then gives up with `TimedOut`. The server's single thread must not wait for a program
/// that sets raw mode and never reads (found by the security review: a pane printing terminal
/// queries filled its own input queue after about a kilobyte and froze every session).
pub fn write_bounded(fd: BorrowedFd<'_>, mut buf: &[u8], idle: Duration, total: Duration) -> io::Result<()> {
    let end = Instant::now() + total;
    while !buf.is_empty() {
        match nix::unistd::write(fd, buf) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0")),
            Ok(n) => buf = &buf[n..],
            Err(Errno::EINTR) => {}
            Err(Errno::EAGAIN) => {
                let left = end.saturating_duration_since(Instant::now());
                let wait = idle.min(left);
                if wait.is_zero() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the pane does not read its input"));
                }
                let millis = u16::try_from(wait.as_millis()).unwrap_or(u16::MAX);
                let mut fds = [PollFd::new(fd, PollFlags::POLLOUT)];
                match poll(&mut fds, PollTimeout::from(millis.max(1))) {
                    Ok(0) => return Err(io::Error::new(io::ErrorKind::TimedOut, "the pane does not read its input")),
                    Ok(_) | Err(Errno::EINTR) => {}
                    Err(e) => return Err(ctx(e, &format!("poll fd {}", fd.as_raw_fd()))),
                }
            }
            Err(e) => return Err(ctx(e, &format!("write fd {}", fd.as_raw_fd()))),
        }
    }
    Ok(())
}

/// One non-blocking read. `Ok(None)` = would block, `Ok(Some(0))` = EOF.
pub fn read_some(fd: BorrowedFd<'_>, buf: &mut [u8]) -> io::Result<Option<usize>> {
    loop {
        match nix::unistd::read(fd, buf) {
            Ok(n) => return Ok(Some(n)),
            Err(Errno::EINTR) => continue,
            Err(Errno::EAGAIN) => return Ok(None),
            Err(Errno::EIO) => return Ok(Some(0)),
            Err(e) => return Err(ctx(e, &format!("read fd {}", fd.as_raw_fd()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn a_write_to_a_descriptor_nobody_reads_gives_up_instead_of_waiting_for_ever() {
        let (rd, wr) = nix::unistd::pipe().unwrap();
        pty::set_nonblocking(wr.as_fd()).unwrap();
        let big = vec![b'x'; 1 << 20];
        let started = Instant::now();
        let err = write_bounded(wr.as_fd(), &big, Duration::from_millis(50), Duration::from_millis(300)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
        drop(rd);
    }

    #[test]
    fn a_write_to_a_reader_that_keeps_up_goes_through() {
        let (rd, wr) = nix::unistd::pipe().unwrap();
        pty::set_nonblocking(wr.as_fd()).unwrap();
        let reader = std::thread::spawn(move || {
            let mut total = 0;
            let mut buf = [0u8; 4096];
            while total < 300_000 {
                match nix::unistd::read(&rd, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => total += n,
                }
            }
            total
        });
        let data = vec![b'y'; 300_000];
        write_bounded(wr.as_fd(), &data, Duration::from_millis(500), Duration::from_secs(5)).unwrap();
        assert_eq!(reader.join().unwrap(), 300_000);
    }
}
