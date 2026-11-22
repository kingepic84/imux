//! What a pane is running right now, for automatic window names and new-pane directories, and who
//! is on the other end of the socket. macOS reads it through libproc, `sysctl` and `LOCAL_PEERPID`;
//! Linux through `/proc` and `SO_PEERCRED`. The functions have the same names and meaning on both.

#[cfg(target_os = "macos")]
use std::ffi::OsStr;
use std::os::fd::{AsRawFd, BorrowedFd};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Process group currently in the foreground of the pty (what tmux calls the pane's process).
pub fn foreground_pid(master: BorrowedFd<'_>) -> Option<i32> {
    // SAFETY: tcgetpgrp only reads state of the fd we pass.
    let pgid = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    (pgid > 0).then_some(pgid)
}

/// File name of the executable behind `pid`, e.g. `zsh` or `node`.
#[cfg(target_os = "macos")]
pub fn command_name(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; 4 * libc::PATH_MAX as usize];
    // SAFETY: buf is valid for buf.len() bytes and proc_pidpath writes at most that many.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let path = String::from_utf8_lossy(&buf[..n as usize]).into_owned();
    path.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string)
}

/// Command line of `pid` (`argv[0]` first), from `KERN_PROCARGS2`. `None` when the process is
/// gone or belongs to someone else.
#[cfg(target_os = "macos")]
pub fn argv(pid: i32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: a null buffer asks sysctl only for the size it needs.
    let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) };
    if rc != 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: buf is valid for `size` bytes and sysctl updates `size` to what it wrote.
    let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return None;
    }
    buf.truncate(size);
    parse_procargs(&buf)
}

/// `argc` (native int), the executable path, NUL padding, then `argc` NUL-terminated strings
/// (the environment follows and is ignored). The layout of macOS; Linux has `/proc/<pid>/cmdline`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let argc = usize::try_from(argc).ok().filter(|&n| n <= 4096)?;
    let rest = &buf[4..];
    let after_path = rest.iter().position(|&b| b == 0)?;
    let rest = &rest[after_path..];
    let start = rest.iter().position(|&b| b != 0)?;
    let mut args = Vec::with_capacity(argc);
    for part in rest[start..].split(|&b| b == 0).take(argc) {
        args.push(String::from_utf8_lossy(part).into_owned());
    }
    (args.len() == argc).then_some(args)
}

/// Current working directory of `pid`.
#[cfg(target_os = "macos")]
pub fn cwd(pid: i32) -> Option<PathBuf> {
    // SAFETY: proc_vnodepathinfo is plain old data, all-zero is a valid value.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: info is a valid out-buffer of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size)
    };
    if n != size {
        return None;
    }
    let path = &info.pvi_cdir.vip_path;
    // SAFETY: the path field is a contiguous array of 32 * 32 bytes.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(path.as_ptr().cast(), 32 * 32) };
    let end = bytes.iter().position(|&b| b == 0)?;
    (end > 0).then(|| PathBuf::from(OsStr::from_bytes(&bytes[..end])))
}

/// Parent of `pid`.
#[cfg(target_os = "macos")]
pub fn parent_pid(pid: i32) -> Option<i32> {
    // SAFETY: proc_bsdinfo is plain old data, all-zero is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: info is a valid out-buffer of `size` bytes.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    (n == size && info.pbi_ppid > 0).then_some(info.pbi_ppid as i32)
}

/// The processes whose parent is `pid`, at most 64 of them.
#[cfg(target_os = "macos")]
pub fn child_pids(pid: i32) -> Vec<i32> {
    let mut buf = [0i32; 64];
    // SAFETY: buf is valid for the number of bytes passed and proc_listchildpids writes at most that.
    let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr().cast(), std::mem::size_of_val(&buf) as libc::c_int) };
    if n <= 0 {
        return Vec::new();
    }
    buf.iter().copied().take_while(|p| *p > 0).collect()
}

/// Whether `pid` is `ancestor` or one of its descendants, following parents at most 64 steps.
pub fn descends_from(pid: i32, ancestor: i32) -> bool {
    let mut at = pid;
    for _ in 0..64 {
        if at == ancestor {
            return true;
        }
        match parent_pid(at) {
            Some(parent) if parent != at && parent > 1 => at = parent,
            _ => return false,
        }
    }
    false
}

/// The process on the other end of a connected unix socket.
#[cfg(target_os = "macos")]
pub fn peer_pid(socket: BorrowedFd<'_>) -> Option<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: pid and len are valid for the size passed; the fd is a connected unix socket.
    let r = unsafe {
        libc::getsockopt(socket.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len)
    };
    (r == 0 && pid > 0).then_some(pid)
}

/// The user id of the process on the other end of a connected unix socket (`getpeereid`).
#[cfg(target_os = "macos")]
pub fn peer_uid(socket: BorrowedFd<'_>) -> Option<u32> {
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: uid and gid are valid for writes; the fd is a connected unix socket.
    let r = unsafe { libc::getpeereid(socket.as_raw_fd(), &mut uid, &mut gid) };
    (r == 0).then_some(uid)
}

/// What `/proc/<pid>/<file>` holds, at most `limit` bytes of it; `None` when the process is gone
/// or belongs to someone who does not let this user look.
#[cfg(target_os = "linux")]
fn proc_file(pid: i32, file: &str, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    std::fs::File::open(format!("/proc/{pid}/{file}")).ok()?.take(limit).read_to_end(&mut out).ok()?;
    Some(out)
}

/// File name of the executable behind `pid`, e.g. `zsh` or `node`: the target of `/proc/<pid>/exe`,
/// or the (15 character) `comm` when the link cannot be read.
#[cfg(target_os = "linux")]
pub fn command_name(pid: i32) -> Option<String> {
    let from_link = std::fs::read_link(format!("/proc/{pid}/exe")).ok().and_then(|p| {
        let text = p.to_string_lossy().into_owned();
        let text = text.strip_suffix(" (deleted)").unwrap_or(&text).to_string();
        text.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string)
    });
    from_link.or_else(|| {
        let comm = proc_file(pid, "comm", 64)?;
        let name = String::from_utf8_lossy(&comm).trim_end_matches('\n').to_string();
        (!name.is_empty()).then_some(name)
    })
}

/// Command line of `pid` (`argv[0]` first), from `/proc/<pid>/cmdline`: the arguments each end in
/// a NUL. `None` when the process is gone, belongs to someone else or has no command line (a
/// kernel thread, a zombie).
#[cfg(target_os = "linux")]
pub fn argv(pid: i32) -> Option<Vec<String>> {
    let raw = proc_file(pid, "cmdline", 4 << 20)?;
    parse_cmdline(&raw)
}

/// The arguments of a `/proc/<pid>/cmdline`.
#[cfg(target_os = "linux")]
fn parse_cmdline(raw: &[u8]) -> Option<Vec<String>> {
    if raw.is_empty() {
        return None;
    }
    let body = raw.strip_suffix(&[0]).unwrap_or(raw);
    let args: Vec<String> = body.split(|&b| b == 0).map(|part| String::from_utf8_lossy(part).into_owned()).collect();
    (args.len() <= 4096).then_some(args)
}

/// Current working directory of `pid`, from the link `/proc/<pid>/cwd`; a directory that has been
/// removed gives nothing.
#[cfg(target_os = "linux")]
pub fn cwd(pid: i32) -> Option<PathBuf> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    (!path.as_os_str().is_empty() && !path.to_string_lossy().ends_with(" (deleted)")).then_some(path)
}

/// The parent field of a `/proc/<pid>/stat`: `pid (name) state ppid ...`, where the name may hold
/// spaces and brackets, so the fields are counted from its last `)`.
#[cfg(target_os = "linux")]
fn parse_stat_parent(stat: &[u8]) -> Option<i32> {
    let text = String::from_utf8_lossy(stat);
    let after = &text[text.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

/// Parent of `pid`.
#[cfg(target_os = "linux")]
pub fn parent_pid(pid: i32) -> Option<i32> {
    parse_stat_parent(&proc_file(pid, "stat", 4096)?).filter(|&p| p > 0)
}

/// The processes whose parent is `pid`, at most 64 of them (the newest): the `children` file of
/// each of its threads when the kernel has them, else a scan of `/proc` for processes with that
/// parent.
#[cfg(target_os = "linux")]
pub fn child_pids(pid: i32) -> Vec<i32> {
    let mut found = Vec::new();
    let mut complete = true;
    match std::fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => {
            for task in tasks.flatten() {
                match std::fs::read_to_string(task.path().join("children")) {
                    Ok(text) => found.extend(text.split_whitespace().filter_map(|w| w.parse::<i32>().ok())),
                    Err(_) => complete = false,
                }
            }
        }
        Err(_) => return Vec::new(),
    }
    if !complete {
        found.clear();
        if let Ok(all) = std::fs::read_dir("/proc") {
            for entry in all.flatten() {
                let Some(child) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
                if parent_pid(child) == Some(pid) {
                    found.push(child);
                }
            }
        }
    }
    found.sort_unstable();
    // The newest ones when there are more: a new child is what a caller looks for.
    found.drain(..found.len().saturating_sub(64));
    found
}

/// Who is on the other end of a connected unix socket, as the kernel recorded it at `connect`.
#[cfg(target_os = "linux")]
fn peer_cred(socket: BorrowedFd<'_>) -> Option<libc::ucred> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len are valid for the size passed; the fd is a connected unix socket.
    let r = unsafe { libc::getsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (r == 0 && len as usize == std::mem::size_of::<libc::ucred>()).then_some(cred)
}

/// The process on the other end of a connected unix socket.
#[cfg(target_os = "linux")]
pub fn peer_pid(socket: BorrowedFd<'_>) -> Option<i32> {
    peer_cred(socket).map(|c| c.pid).filter(|&p| p > 0)
}

/// The user id of the process on the other end of a connected unix socket (`SO_PEERCRED`).
#[cfg(target_os = "linux")]
pub fn peer_uid(socket: BorrowedFd<'_>) -> Option<u32> {
    peer_cred(socket).map(|c| c.uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peer_of_a_socket_pair_is_this_user_and_this_process() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(peer_uid(std::os::fd::AsFd::as_fd(&a)), Some(nix::unistd::getuid().as_raw()));
        assert_eq!(peer_pid(std::os::fd::AsFd::as_fd(&a)), Some(std::process::id() as i32));
    }

    #[test]
    fn the_children_of_a_process_are_listed() {
        let me = std::process::id() as i32;
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let pid = child.id() as i32;
        assert!(child_pids(me).contains(&pid));
        assert!(child_pids(pid).is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!child_pids(me).contains(&pid));
    }

    #[test]
    fn a_process_knows_its_parent_and_descends_from_it_and_from_itself() {
        let me = std::process::id() as i32;
        let parent = std::os::unix::process::parent_id() as i32;
        assert_eq!(parent_pid(me), Some(parent));
        assert!(descends_from(me, me));
        assert!(descends_from(me, parent));
        assert!(!descends_from(parent, me));
        assert!(!descends_from(me, i32::MAX));
    }

    #[test]
    fn the_peer_of_a_socket_pair_is_this_process() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        use std::os::fd::AsFd;
        assert_eq!(peer_pid(a.as_fd()), Some(std::process::id() as i32));
    }

    #[test]
    fn own_process_has_a_name_and_a_cwd() {
        let me = std::process::id() as i32;
        assert!(command_name(me).is_some());
        assert_eq!(cwd(me), std::env::current_dir().ok().and_then(|d| d.canonicalize().ok()));
    }

    #[test]
    fn unknown_pid_gives_nothing() {
        assert_eq!(command_name(i32::MAX), None);
        assert_eq!(cwd(i32::MAX), None);
    }

    #[test]
    fn own_command_line_matches_std_args() {
        let me = std::process::id() as i32;
        let got = argv(me).expect("own argv");
        let want: Vec<String> = std::env::args().collect();
        assert_eq!(got.len(), want.len());
        assert_eq!(&got[1..], &want[1..]);
        assert_eq!(argv(i32::MAX), None);
    }

    fn procargs(argc: i32, path: &str, pad: usize, args: &[&str], env: &[&str]) -> Vec<u8> {
        let mut b = argc.to_ne_bytes().to_vec();
        b.extend_from_slice(path.as_bytes());
        b.extend(std::iter::repeat_n(0u8, 1 + pad));
        for s in args.iter().chain(env) {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    }

    #[test]
    fn procargs_gives_the_arguments_without_path_padding_or_environment() {
        let b = procargs(3, "/usr/bin/vim", 6, &["vim", "a b", ""], &["HOME=/x", "TERM=y"]);
        assert_eq!(parse_procargs(&b), Some(vec!["vim".into(), "a b".into(), "".into()]));
    }

    #[test]
    fn procargs_that_are_cut_short_or_absurd_give_nothing() {
        assert_eq!(parse_procargs(&[]), None);
        assert_eq!(parse_procargs(&procargs(-1, "/x", 0, &["x"], &[])), None);
        assert_eq!(parse_procargs(&procargs(99_999, "/x", 0, &["x"], &[])), None);
        assert_eq!(parse_procargs(&procargs(3, "/x", 0, &["x", "y"], &[])[..8]), None);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[test]
    fn a_cmdline_is_its_arguments_each_ended_by_a_nul() {
        assert_eq!(parse_cmdline(b"vim\0a b\0\0"), Some(vec!["vim".to_string(), "a b".to_string(), String::new()]));
        assert_eq!(parse_cmdline(b"zsh\0"), Some(vec!["zsh".to_string()]));
        assert_eq!(parse_cmdline(b"x"), Some(vec!["x".to_string()]), "a program that rewrote its title may leave no final NUL");
    }

    #[test]
    fn a_process_without_a_command_line_has_none() {
        assert_eq!(parse_cmdline(b""), None);
    }

    #[test]
    fn the_parent_is_counted_from_the_last_bracket_so_a_name_with_spaces_and_brackets_is_fine() {
        assert_eq!(parse_stat_parent(b"42 (zsh) S 7 42 42 0 -1"), Some(7));
        assert_eq!(parse_stat_parent(b"42 (a b) c) R 9 1 1"), Some(9));
        assert_eq!(parse_stat_parent(b"42 (x"), None);
        assert_eq!(parse_stat_parent(b"42 (x) S"), None);
        assert_eq!(parse_stat_parent(b"42 (x) S notanumber"), None);
    }

    #[test]
    fn a_name_of_a_gone_executable_loses_its_deleted_marker() {
        let me = std::process::id() as i32;
        let name = command_name(me).unwrap();
        assert!(!name.contains('/') && !name.ends_with("(deleted)"), "{name}");
    }

    #[test]
    fn a_process_gone_has_no_stat_and_no_children() {
        assert_eq!(parent_pid(i32::MAX), None);
        assert!(child_pids(i32::MAX).is_empty());
    }
}
