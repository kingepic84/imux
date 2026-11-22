use std::fs::{self, DirBuilder};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::tty::ctx;

/// The name of this machine, empty when the system does not say.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length and gethostname NUL-terminates within it.
    if unsafe { nix::libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Longest socket path the system takes (`sun_path` is 104 bytes on macOS, with the NUL).
const MAX_SOCKET_PATH: usize = 100;

/// A name for a server (`imux -L name`): 1 to 32 of `A-Za-z0-9._-`, not starting with a dot.
/// `default` is the name of the server everybody gets without asking.
pub fn valid_socket_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && !name.starts_with('.') && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The socket a process was pointed at by `-S` (kept in `$IMUX_SERVER_SOCKET` so the server it
/// starts, and the panes of that server, find the same one), if it is a full path. Not
/// `$IMUX_SOCKET_PATH`: every pane has that set to its server's socket, and a test or a tool run
/// from a pane with another `$TMPDIR` must not be sent to that server.
fn socket_override() -> Option<PathBuf> {
    override_from(std::env::var_os("IMUX_SERVER_SOCKET"))
}

fn override_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|p| p.is_absolute() && p.file_name().is_some())
}

/// The file name of the socket: that of `-S`, else `$IMUX_SOCKET_NAME` when it is a name, else
/// `default`.
fn socket_file() -> String {
    if let Some(p) = socket_override() {
        return p.file_name().map_or_else(|| "default".to_string(), |n| n.to_string_lossy().into_owned());
    }
    std::env::var("IMUX_SOCKET_NAME").ok().filter(|n| valid_socket_name(n)).unwrap_or_else(|| "default".to_string())
}

/// What tells servers apart in the state directory: the socket's file name when it is not
/// `default`, cut to a name's alphabet, so two servers never share an autosave or snapshots.
pub fn state_suffix() -> Option<String> {
    let file = socket_file();
    (file != "default").then(|| {
        let clean: String = file.chars().take(32).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
        clean.trim_start_matches('.').to_string()
    }).filter(|s| !s.is_empty())
}

/// The file the server writes its log to, in the socket directory.
pub fn log_file_name() -> String {
    match state_suffix() {
        Some(s) => format!("server-{s}.log"),
        None => "server.log".to_string(),
    }
}

/// `$TMPDIR/imux-<uid>` (on Linux without `$TMPDIR`: `$XDG_RUNTIME_DIR/imux-<uid>`, which the
/// system makes private to the user), or `/tmp/imux-<uid>` when neither is set or the socket path
/// would exceed the sun_path limit. With `-S` it is the directory that path is in.
pub fn socket_dir() -> PathBuf {
    if let Some(p) = socket_override() {
        return p.parent().map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    }
    let uid = nix::unistd::getuid();
    let base = base_dir(std::env::var_os("TMPDIR"), std::env::var_os("XDG_RUNTIME_DIR"));
    let dir = base.join(format!("imux-{uid}"));
    if dir.join(socket_file()).as_os_str().len() > MAX_SOCKET_PATH {
        return PathBuf::from(format!("/tmp/imux-{uid}"));
    }
    dir
}

/// Aim this process (and the server it starts, and what that starts) at a named server (`-L`) or a
/// socket path (`-S`; wins over a name). Called once at the start of `main`, before any thread, and
/// by the `tmux` shim for the same flags. The path is made absolute and checked for length; a name
/// for its alphabet.
pub fn choose_socket(name: Option<&str>, path: Option<&str>) -> Result<(), String> {
    let (set, unset) = if let Some(p) = path {
        let abs = if Path::new(p).is_absolute() { PathBuf::from(p) } else { std::env::current_dir().map_err(|e| format!("socket path: {e}"))?.join(p) };
        if abs.file_name().is_none() || abs.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(format!("socket path {p:?} is not a file name in a directory, or is longer than {MAX_SOCKET_PATH} bytes"));
        }
        (("IMUX_SERVER_SOCKET", abs.into_os_string()), "IMUX_SOCKET_NAME")
    } else if let Some(n) = name {
        if !valid_socket_name(n) {
            return Err(format!("server name {n:?}: 1 to 32 of A-Z a-z 0-9 . _ - and not starting with a dot"));
        }
        (("IMUX_SOCKET_NAME", std::ffi::OsString::from(n)), "IMUX_SERVER_SOCKET")
    } else {
        return Ok(());
    };
    // SAFETY: called at the start of `main` (and in the shim) while the process has one thread, so
    // nothing reads the environment at the same time.
    unsafe {
        std::env::set_var(set.0, &set.1);
        std::env::remove_var(unset);
    }
    Ok(())
}

/// The name, the path and the words left of a command line (`split_socket_flags`).
pub type SocketFlags = (Option<String>, Option<String>, Vec<String>);

/// `-L name` and `-S path` at the front of an imux command line (the value may be attached to the
/// flag): the name, the path and the words left.
pub fn split_socket_flags(args: &[String]) -> Result<SocketFlags, String> {
    let (mut name, mut path) = (None, None);
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        let (slot, rest) = match arg.strip_prefix("-L") {
            Some(rest) => (&mut name, rest),
            None => match arg.strip_prefix("-S") {
                Some(rest) => (&mut path, rest),
                None => break,
            },
        };
        at += 1;
        let value = if rest.is_empty() {
            let v = args.get(at).ok_or_else(|| format!("{arg} needs a value"))?;
            at += 1;
            v.clone()
        } else {
            rest.to_string()
        };
        *slot = Some(value);
    }
    Ok((name, path, args[at..].to_vec()))
}

/// The directory the socket directory goes in: `$TMPDIR`, else on Linux an absolute
/// `$XDG_RUNTIME_DIR`, else `/tmp`.
fn base_dir(tmpdir: Option<std::ffi::OsString>, runtime: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(dir) = tmpdir {
        return PathBuf::from(dir);
    }
    match runtime.map(PathBuf::from).filter(|d| cfg!(target_os = "linux") && d.is_absolute()) {
        Some(dir) => dir,
        None => PathBuf::from("/tmp"),
    }
}

pub fn socket_path() -> PathBuf {
    socket_override().unwrap_or_else(|| socket_dir().join(socket_file()))
}

/// Create the socket dir (0700) or verify an existing one is ours and private.
pub fn ensure_socket_dir() -> io::Result<PathBuf> {
    let dir = socket_dir();
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(ctx(e, &format!("create {}", dir.display()))),
    }
    check_dir(&dir)?;
    Ok(dir)
}

/// The directory of the `tmux` shim: inside the socket directory, so it has the same owner and
/// mode checks, and only panes of this server get it on their `PATH`.
pub fn shim_dir() -> PathBuf {
    socket_dir().join("shims")
}

/// Make `<socket dir>/shims/tmux` a link to `exe` and return the directory. The directory must be
/// ours and private like the socket directory; the link is replaced when it points elsewhere;
/// anything else at that name (a file, a directory) is an error and nothing is removed.
pub fn ensure_shim(exe: &Path) -> io::Result<PathBuf> {
    ensure_shim_in(&shim_dir(), exe)
}

fn ensure_shim_in(dir: &Path, exe: &Path) -> io::Result<PathBuf> {
    let dir = dir.to_path_buf();
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(ctx(e, &format!("create {}", dir.display()))),
    }
    check_dir(&dir)?;
    let link = dir.join("tmux");
    match fs::symlink_metadata(&link) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Ok(m) if m.file_type().is_symlink() => {
            if fs::read_link(&link).is_ok_and(|t| t == exe) {
                return Ok(dir);
            }
            fs::remove_file(&link).map_err(|e| ctx(e, &format!("replace {}", link.display())))?;
        }
        Ok(_) => return Err(io::Error::other(format!("{} is not a link, left alone", link.display()))),
        Err(e) => return Err(ctx(e, &format!("stat {}", link.display()))),
    }
    std::os::unix::fs::symlink(exe, &link).map_err(|e| ctx(e, &format!("link {}", link.display())))?;
    Ok(dir)
}

/// The directory must be a real directory (no link) owned by us and closed to everybody else.
fn check_dir(dir: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(dir).map_err(|e| ctx(e, &format!("stat {}", dir.display())))?;
    let uid = nix::unistd::getuid().as_raw();
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} must be a directory owned by uid {uid} with mode 0700", dir.display()),
        ));
    }
    Ok(())
}

/// What a client checks before it talks to a socket: the directory is ours and private, so
/// nobody else could have put a server there (security review F-E). A missing directory is fine
/// here (no server runs); the server creates it.
pub fn check_socket_dir() -> io::Result<()> {
    let dir = socket_dir();
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => check_dir(&dir),
    }
}

/// The most files one `source-file` pattern may name.
pub const MAX_GLOB_FILES: usize = 64;

/// The files a `source-file` path names: a path without `*` or `?` is itself (with `~/` taken as
/// the home directory); with them in the file name (the last part), the entries of that directory
/// whose names match, in name order, without hidden ones unless the pattern starts with a dot, at
/// most `MAX_GLOB_FILES`. A pattern in a directory part matches nothing.
pub fn expand_source_path(text: &str) -> Vec<PathBuf> {
    let path = match (text.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(text),
    };
    let wild = |s: &str| s.contains(['*', '?']);
    let Some(pattern) = path.file_name().and_then(|n| n.to_str()).filter(|n| wild(n)).map(str::to_string) else {
        return if path.to_str().is_some_and(wild) { Vec::new() } else { vec![path] };
    };
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if dir.to_str().is_none_or(wild) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| (pattern.starts_with('.') || !n.starts_with('.')) && crate::target::glob(&pattern, n))
        .collect();
    names.sort();
    names.truncate(MAX_GLOB_FILES);
    names.into_iter().map(|n| dir.join(n)).collect()
}

/// Read a file the user named (a configuration, a buffer): it must be a regular file, which also
/// keeps a FIFO from blocking the server's one thread and `/dev/zero` from filling its memory, and
/// it may not hold more than `max` bytes. A link is followed.
pub fn read_regular(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    // `IMUX_CONFIG=/dev/null` is the way to say "no configuration".
    if path == Path::new("/dev/null") {
        return Ok(Vec::new());
    }
    let file = fs::OpenOptions::new().read(true).custom_flags(nix::libc::O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let mut data = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut data)?;
    if data.len() > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bigger than {max} bytes")));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_regular_file_of_a_sane_size_is_read() {
        let dir = std::env::temp_dir().join(format!("imux-readreg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small");
        fs::write(&small, b"hello").unwrap();
        assert_eq!(read_regular(&small, 100).unwrap(), b"hello");
        assert!(read_regular(&small, 3).is_err(), "over the limit");
        assert!(read_regular(Path::new("/dev/zero"), 1 << 20).is_err(), "a device is no regular file");
        let fifo = dir.join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        let started = std::time::Instant::now();
        assert!(read_regular(&fifo, 100).is_err(), "a pipe is no regular file");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "it did not wait for a writer");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_source_path_may_end_in_a_pattern() {
        let dir = std::env::temp_dir().join(format!("imux-glob-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for n in ["b.conf", "a.conf", ".h.conf", "c.txt"] {
            fs::write(dir.join(n), b"").unwrap();
        }
        let at = |p: &str| expand_source_path(&format!("{}/{p}", dir.display()));
        assert_eq!(at("*.conf"), vec![dir.join("a.conf"), dir.join("b.conf")], "in name order, no hidden file");
        assert_eq!(at(".*.conf"), vec![dir.join(".h.conf")]);
        assert_eq!(at("?.txt"), vec![dir.join("c.txt")]);
        assert!(at("*.none").is_empty());
        assert_eq!(at("plain"), vec![dir.join("plain")], "a path without a pattern is itself, there or not");
        assert!(expand_source_path(&format!("{}*/a.conf", dir.display())).is_empty(), "no pattern in a directory part");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod base_dir_tests {
    use super::*;
    use std::ffi::OsString;

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn tmpdir_wins_wherever_it_is_set() {
        assert_eq!(base_dir(os("/var/tmp/x"), os("/run/user/1000")), PathBuf::from("/var/tmp/x"));
        assert_eq!(base_dir(os("/var/tmp/x"), None), PathBuf::from("/var/tmp/x"));
    }

    #[test]
    fn nothing_set_is_tmp() {
        assert_eq!(base_dir(None, None), PathBuf::from("/tmp"));
    }

    #[test]
    fn the_runtime_directory_is_used_on_linux_only_and_only_when_absolute() {
        let got = base_dir(None, os("/run/user/1000"));
        assert_eq!(got, PathBuf::from(if cfg!(target_os = "linux") { "/run/user/1000" } else { "/tmp" }));
        assert_eq!(base_dir(None, os("relative/dir")), PathBuf::from("/tmp"));
        assert_eq!(base_dir(None, os("")), PathBuf::from("/tmp"));
    }

    #[test]
    fn the_shim_is_a_link_that_is_made_kept_and_replaced_and_nothing_else_is_touched() {
        let root = std::env::temp_dir().join(format!("imux-shim-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let dir = root.join("shims");
        let (a, b) = (root.join("imux-a"), root.join("imux-b"));
        assert_eq!(ensure_shim_in(&dir, &a).unwrap(), dir);
        assert_eq!(fs::read_link(dir.join("tmux")).unwrap(), a, "made");
        ensure_shim_in(&dir, &a).unwrap();
        assert_eq!(fs::read_link(dir.join("tmux")).unwrap(), a, "kept");
        ensure_shim_in(&dir, &b).unwrap();
        assert_eq!(fs::read_link(dir.join("tmux")).unwrap(), b, "replaced when it points elsewhere");
        fs::remove_file(dir.join("tmux")).unwrap();
        fs::write(dir.join("tmux"), b"mine").unwrap();
        assert!(ensure_shim_in(&dir, &a).is_err(), "a regular file is left alone");
        assert_eq!(fs::read(dir.join("tmux")).unwrap(), b"mine");
        fs::remove_file(dir.join("tmux")).unwrap();
        fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(ensure_shim_in(&dir, &a).is_err(), "a directory open to others is refused");
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod socket_choice_tests {
    use super::*;

    fn words(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_socket_name("work"));
        assert!(valid_socket_name("a.b_c-d"));
        assert!(valid_socket_name("default"));
        assert!(!valid_socket_name(""));
        assert!(!valid_socket_name(".hidden"));
        assert!(!valid_socket_name("a/b"));
        assert!(!valid_socket_name("a b"));
        assert!(!valid_socket_name(&"x".repeat(33)));
        assert!(valid_socket_name(&"x".repeat(32)));
    }

    #[test]
    fn only_a_full_path_with_a_file_name_is_an_override() {
        assert_eq!(override_from(Some("/a/b/sock".into())), Some(PathBuf::from("/a/b/sock")));
        assert_eq!(override_from(Some("rel/sock".into())), None);
        assert_eq!(override_from(Some("".into())), None);
        assert_eq!(override_from(Some("/".into())), None);
        assert_eq!(override_from(None), None);
    }

    #[test]
    fn socket_flags_come_off_the_front_of_a_command_line() {
        assert_eq!(split_socket_flags(&words(&["-L", "w", "ls"])).unwrap(), (Some("w".into()), None, words(&["ls"])));
        assert_eq!(split_socket_flags(&words(&["-Lw", "-S", "/p/s", "kill-server"])).unwrap(), (Some("w".into()), Some("/p/s".into()), words(&["kill-server"])));
        assert_eq!(split_socket_flags(&words(&["ls", "-L", "w"])).unwrap(), (None, None, words(&["ls", "-L", "w"])));
        assert_eq!(split_socket_flags(&words(&[])).unwrap(), (None, None, words(&[])));
        assert!(split_socket_flags(&words(&["-L"])).is_err());
    }

    #[test]
    fn a_bad_name_or_path_is_refused_before_anything_is_set() {
        assert!(choose_socket(Some("a/b"), None).is_err());
        assert!(choose_socket(None, Some(&format!("/{}", "x".repeat(120)))).is_err());
        assert!(choose_socket(None, None).is_ok());
    }
}
