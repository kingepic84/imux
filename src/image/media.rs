//! Kitty graphics media other than direct transmission: the payload names a file (`t=f`), a
//! temporary file the terminal deletes after reading (`t=t`) or a POSIX shared memory object
//! (`t=s`), and the server reads it. A program that sends these runs on the machine of the
//! server and nowhere else: over ssh the name would be read on the wrong machine, and a remote
//! program could make this server open files of its user. So the whole feature is behind the
//! option `kitty-media` and off by default.

use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::kitty::Control;

/// Most bytes read for one picture, as for direct transmission.
const MAX_BYTES: usize = 64 * 1024 * 1024;

/// A name longer than this is refused.
const MAX_NAME: usize = 4096;

/// Part of the file name of a temporary file the terminal may delete.
const TEMP_MARK: &str = "tty-graphics-protocol";

/// Directories a `t=t` file must be in.
const TEMP_DIRS: &[&str] = &["/tmp", "/private/tmp", "/var/folders", "/private/var/folders", "/dev/shm"];

/// Where the server may not read from: files there are not pictures.
const FORBIDDEN: &[&str] = &["/proc", "/sys", "/dev", "/private/dev"];

/// How much of the media feature the option `kitty-media` allows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    None,
    Files,
    All,
}

impl Level {
    pub fn of(text: &str) -> Level {
        match text {
            "files" => Level::Files,
            "all" => Level::All,
            _ => Level::None,
        }
    }
}

thread_local! {
    static LEVEL: Cell<Level> = const { Cell::new(Level::None) };
}

/// The server calls this when `kitty-media` changes.
pub fn set_level(level: Level) {
    LEVEL.with(|l| l.set(level));
}

fn level() -> Level {
    LEVEL.with(Cell::get)
}

/// The bytes the media named by `name` (the decoded payload) holds, for `t=f`, `t=t` or `t=s`.
pub fn read(c: &Control, name: &[u8]) -> Result<Vec<u8>, String> {
    read_at(c, name, level())
}

fn read_at(c: &Control, name: &[u8], allowed: Level) -> Result<Vec<u8>, String> {
    let wanted = if c.medium == b's' { Level::All } else { Level::Files };
    if allowed == Level::None || (wanted == Level::All && allowed != Level::All) {
        return Err(format!("medium t={} is switched off (option kitty-media)", char::from(c.medium)));
    }
    if name.is_empty() || name.len() > MAX_NAME || name.contains(&0) {
        return Err("the name of the picture is not usable".into());
    }
    let name = std::str::from_utf8(name).map_err(|_| "the name of the picture is not text")?;
    let limit = if c.size > 0 { c.size.min(MAX_BYTES) } else { MAX_BYTES };
    match c.medium {
        b'f' => read_file(Path::new(name), c.offset, limit),
        b't' => {
            let path = temporary(Path::new(name))?;
            let result = read_file(&path, c.offset, limit);
            if let Err(e) = std::fs::remove_file(&path) {
                crate::log::warn(&format!("kitty graphics: removing {}: {e}", path.display()));
            }
            result
        }
        b's' => read_shared(name, c.offset, limit),
        other => Err(format!("medium t={} is not supported", char::from(other))),
    }
}

/// A file that is a plain regular file and not under a system directory. The file is opened
/// without blocking, so a named pipe cannot hold the server, and checked again once open.
fn read_file(path: &Path, offset: usize, limit: usize) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("the path of the picture is not absolute".into());
    }
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    if FORBIDDEN.iter().any(|d| real.starts_with(d)) {
        return Err("files under system directories are refused".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(&real)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("the picture is not a regular file".into());
    }
    file.seek(SeekFrom::Start(offset as u64)).map_err(|e| format!("seek: {e}"))?;
    let mut data = Vec::new();
    file.take(limit as u64).read_to_end(&mut data).map_err(|e| format!("read: {e}"))?;
    Ok(data)
}

/// The path of a `t=t` file, if it may be read and then deleted: a temporary directory, and the
/// protocol's mark in the name.
fn temporary(path: &Path) -> Result<PathBuf, String> {
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    let marked = real.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains(TEMP_MARK));
    if !marked || !TEMP_DIRS.iter().any(|d| real.starts_with(d)) {
        return Err("a temporary picture must be a file with tty-graphics-protocol in its name in a temporary directory".into());
    }
    Ok(real)
}

/// A POSIX shared memory object, read and then removed.
fn read_shared(name: &str, offset: usize, limit: usize) -> Result<Vec<u8>, String> {
    use std::os::fd::FromRawFd;
    let cname = std::ffi::CString::new(name).map_err(|_| "the name of the picture is not usable")?;
    // SAFETY: `cname` is a valid NUL-terminated string that lives through the call; shm_open
    // only reads it and returns a new descriptor or -1.
    let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0) };
    if fd < 0 {
        return Err(format!("shared memory: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just returned by shm_open and nothing else owns it.
    let file = unsafe { File::from_raw_fd(fd) };
    let result = read_shared_file(&file, offset, limit);
    // SAFETY: as above for `cname`; unlink only removes the name.
    if unsafe { libc::shm_unlink(cname.as_ptr()) } != 0 {
        crate::log::warn(&format!("kitty graphics: removing shared memory {name}: {}", std::io::Error::last_os_error()));
    }
    result
}

/// The bytes of a shared memory object, mapped for the copy (on macOS such an object cannot be
/// read with `read`). The program that made it could shrink it while it is mapped, and a read
/// past the new end is a SIGBUS: that is why `t=s` is the most trusting level of `kitty-media`.
fn read_shared_file(file: &File, offset: usize, limit: usize) -> Result<Vec<u8>, String> {
    use std::os::fd::AsRawFd;
    let size = file.metadata().map_err(|e| format!("shared memory: {e}"))?.len() as usize;
    if size > MAX_BYTES {
        return Err("the shared memory is over the size limit".into());
    }
    if size == 0 || offset >= size {
        return Ok(Vec::new());
    }
    // SAFETY: a read-only shared mapping of `size` bytes of a valid descriptor; the result is
    // checked against MAP_FAILED and the mapping is removed below.
    let map = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, file.as_raw_fd(), 0) };
    if map == libc::MAP_FAILED {
        return Err(format!("shared memory: {}", std::io::Error::last_os_error()));
    }
    let len = (size - offset).min(limit);
    // SAFETY: `map` points at `size` readable bytes and `offset + len <= size`.
    let data = unsafe { std::slice::from_raw_parts((map as *const u8).add(offset), len) }.to_vec();
    // SAFETY: unmapping exactly what was mapped above; `data` is a copy.
    unsafe { libc::munmap(map, size) };
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(medium: u8) -> Control {
        Control { medium, ..Control::default() }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imux-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn nothing_is_read_unless_the_option_allows_it() {
        let path = scratch("off.bin");
        std::fs::write(&path, b"abc").unwrap();
        let name = path.to_str().unwrap().as_bytes();
        assert!(read_at(&control(b'f'), name, Level::None).unwrap_err().contains("switched off"));
        assert_eq!(read_at(&control(b'f'), name, Level::Files).unwrap(), b"abc");
        assert!(read_at(&control(b's'), b"/x", Level::Files).unwrap_err().contains("switched off"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn size_and_offset_pick_a_part_of_the_file() {
        let path = scratch("part.bin");
        std::fs::write(&path, b"0123456789").unwrap();
        let c = Control { medium: b'f', size: 4, offset: 3, ..Control::default() };
        assert_eq!(read_at(&c, path.to_str().unwrap().as_bytes(), Level::Files).unwrap(), b"3456");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn relative_system_and_special_files_are_refused() {
        assert!(read_at(&control(b'f'), b"relative.png", Level::All).is_err());
        assert!(read_at(&control(b'f'), b"/dev/zero", Level::All).is_err());
        assert!(read_at(&control(b'f'), b"/proc/self/environ", Level::All).is_err());
        assert!(read_at(&control(b'f'), b"/tmp", Level::All).is_err(), "a directory");
        assert!(read_at(&control(b'f'), b"", Level::All).is_err());
        assert!(read_at(&control(b'f'), b"/tmp/a\0b", Level::All).is_err());
    }

    #[test]
    fn a_symlink_to_a_system_file_is_refused_too() {
        let link = scratch("link-to-dev");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
        assert!(read_at(&control(b'f'), link.to_str().unwrap().as_bytes(), Level::Files).is_err());
        std::fs::remove_file(link).unwrap();
    }

    #[test]
    fn a_temporary_file_is_read_and_deleted_only_when_marked() {
        let marked = scratch("x-tty-graphics-protocol-1");
        std::fs::write(&marked, b"pixels").unwrap();
        let name = marked.to_str().unwrap().as_bytes();
        assert_eq!(read_at(&control(b't'), name, Level::Files).unwrap(), b"pixels");
        assert!(!marked.exists(), "deleted after reading");
        let plain = scratch("precious.png");
        std::fs::write(&plain, b"keep").unwrap();
        assert!(read_at(&control(b't'), plain.to_str().unwrap().as_bytes(), Level::Files).is_err());
        assert!(plain.exists(), "an unmarked file is never deleted");
        std::fs::remove_file(plain).unwrap();
    }

    #[test]
    fn shared_memory_is_read_and_unlinked() {
        let name = format!("/imux-test-{}", std::process::id());
        let cname = std::ffi::CString::new(name.clone()).unwrap();
        // SAFETY: valid C string; the descriptor is wrapped at once and closed by File's drop.
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_CREAT | libc::O_RDWR | libc::O_EXCL, 0o600) };
        assert!(fd >= 0, "shm_open: {}", std::io::Error::last_os_error());
        let file = {
            use std::os::fd::FromRawFd;
            // SAFETY: `fd` was just returned by shm_open and nothing else owns it.
            unsafe { File::from_raw_fd(fd) }
        };
        file.set_len(6).unwrap();
        {
            use std::os::fd::AsRawFd;
            // SAFETY: a read-write shared mapping of the 6 bytes just made; checked, written
            // within bounds and unmapped.
            unsafe {
                let map = libc::mmap(std::ptr::null_mut(), 6, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0);
                assert_ne!(map, libc::MAP_FAILED);
                std::ptr::copy_nonoverlapping(b"shmpix".as_ptr(), map as *mut u8, 6);
                libc::munmap(map, 6);
            }
        }
        let six = Control { medium: b's', size: 6, ..Control::default() };
        assert_eq!(read_at(&six, name.as_bytes(), Level::All).unwrap(), b"shmpix");
        assert!(read_at(&six, name.as_bytes(), Level::All).is_err(), "unlinked by the first read");
    }
}
