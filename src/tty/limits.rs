//! Resource limits of the server process. A core dump of the server would hold what the panes
//! showed, so it is switched off; the open file limit is raised, since macOS starts at 256 and
//! every pane, client and helper takes descriptors. Programs in the panes get the limits the
//! server was started with, not these.
//!
//! `rlim_t` is `u64` on macOS and on 64-bit Linux; the functions take it as `u64`.

use std::sync::atomic::{AtomicU64, Ordering};

use nix::libc;

use crate::log;

/// The type of an `RLIMIT_*` constant and of the argument of `getrlimit`: glibc names its own
/// unsigned type, everything else uses a plain int.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
type Resource = libc::__rlimit_resource_t;
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
type Resource = libc::c_int;

/// Most descriptors asked for (the macOS per-process maximum).
const MAX_OPEN_FILES: u64 = 10_240;

static CORE: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static FILES: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

fn get(resource: Resource) -> Option<libc::rlimit> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: lim is a valid out-parameter for the call.
    (unsafe { libc::getrlimit(resource, &mut lim) } == 0).then_some(lim)
}

fn set(resource: Resource, cur: u64, max: u64) -> std::io::Result<()> {
    let lim = libc::rlimit { rlim_cur: cur, rlim_max: max };
    // SAFETY: lim is a valid in-parameter for the call.
    if unsafe { libc::setrlimit(resource, &lim) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// The open file limit to ask for given the current limits: raised toward the hard limit, never lowered.
fn wanted_files(cur: u64, max: u64) -> u64 {
    cur.max(max.min(MAX_OPEN_FILES))
}

/// Called once when the server starts: remember the limits panes should get, then switch off core
/// dumps and raise the open file limit. Failures are logged and leave the limit as it was.
pub fn harden() {
    if let Some(lim) = get(libc::RLIMIT_CORE) {
        CORE[0].store(lim.rlim_cur, Ordering::Relaxed);
        CORE[1].store(lim.rlim_max, Ordering::Relaxed);
        if let Err(e) = set(libc::RLIMIT_CORE, 0, lim.rlim_max) {
            log::warn(&format!("cannot switch off core dumps: {e}"));
        }
    }
    if let Some(lim) = get(libc::RLIMIT_NOFILE) {
        FILES[0].store(lim.rlim_cur, Ordering::Relaxed);
        FILES[1].store(lim.rlim_max, Ordering::Relaxed);
        let want = wanted_files(lim.rlim_cur, lim.rlim_max);
        if want > lim.rlim_cur {
            match set(libc::RLIMIT_NOFILE, want, lim.rlim_max) {
                Ok(()) => log::info(&format!("open file limit raised from {} to {want}", lim.rlim_cur)),
                Err(e) => log::warn(&format!("cannot raise the open file limit to {want}: {e}")),
            }
        }
    }
}

/// In the forked child before `execve`: give back the limits the server was started with. Only
/// system calls and atomic loads, so it is safe between `fork` and `exec`.
pub fn restore_for_child() {
    for (resource, saved) in [(libc::RLIMIT_CORE, &CORE), (libc::RLIMIT_NOFILE, &FILES)] {
        let (cur, max) = (saved[0].load(Ordering::Relaxed), saved[1].load(Ordering::Relaxed));
        if max != 0 || cur != 0 {
            let lim = libc::rlimit { rlim_cur: cur, rlim_max: max };
            // SAFETY: setrlimit is async-signal-safe and lim is a valid in-parameter.
            unsafe { libc::setrlimit(resource, &lim) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_limit_is_raised_toward_the_hard_limit_and_never_lowered() {
        assert_eq!(wanted_files(256, u64::MAX), MAX_OPEN_FILES);
        assert_eq!(wanted_files(256, 4096), 4096);
        assert_eq!(wanted_files(256, 256), 256);
        assert_eq!(wanted_files(20_000, 30_000), 20_000);
        assert_eq!(wanted_files(256, 100), 256);
    }
}
