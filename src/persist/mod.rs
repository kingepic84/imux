//! Saving and restoring sessions: the layout, working directories and names of every window,
//! not scrollback, images or process state. The model here is plain data with no ptys, so it
//! can be tested without a server; `format` turns it into text and back, `Store` keeps the files.

pub mod format;
pub mod writer;

use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::session::layout::Shape;
use crate::tty::ctx;

/// One pane: where it was and what ran in it. `argv` is empty for a plain shell.
#[derive(Clone, Debug, PartialEq)]
pub struct SPane {
    pub cwd: String,
    pub argv: Vec<String>,
    /// The foreground program was Claude Code; restore resumes it instead of re-running `argv`.
    pub claude: bool,
    /// The command the program reported for resuming itself; restore prefers it to the guesses
    /// above, after checking it again.
    pub resume: Option<Vec<String>>,
    /// The programs `argv` runs inside, outermost first, between the pane's shell and it: a nested
    /// shell, `nix develop`. Restore types them one after the other, each only if it is allowed.
    pub outer: Vec<Vec<String>>,
    /// The name of the agent in the pane (`imux agent name`), given back to the first record the
    /// restored pane gets.
    pub name: Option<String>,
}

/// A window's splits with a pane in each leaf, leaves in `Layout::panes` order.
pub type STree = Shape<SPane>;

#[derive(Clone, Debug, PartialEq)]
pub struct SWindow {
    pub name: String,
    pub auto_rename: bool,
    /// Index of the active pane among the leaves.
    pub active_leaf: usize,
    pub tree: STree,
    /// Free-form key/values kept with the window (worktree origin, later agent state).
    pub meta: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SSession {
    pub name: String,
    pub current: usize,
    pub windows: Vec<SWindow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub saved_at: u64,
    /// Index of the session clients attach to first.
    pub recent: usize,
    pub sessions: Vec<SSession>,
}

const AUTOSAVE: &str = "autosave.imux";
const AUTOSAVE_PREV: &str = "autosave.prev";
/// An autosave that did not read back is kept as this name plus the time in milliseconds.
const BAD_PREFIX: &str = "autosave.bad.";
/// How many of those are kept.
const KEEP_BAD: usize = 3;
const SNAPSHOTS: &str = "snapshots";
const EXT: &str = "imux";

/// A name a snapshot may carry: it becomes a file name, so no separators or leading dot.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `$IMUX_STATE_DIR`, else `$XDG_STATE_HOME/imux`, else `~/.local/state/imux`.
pub fn state_dir(env: impl Fn(&str) -> Option<OsString>) -> io::Result<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = set("IMUX_STATE_DIR") {
        return Ok(d);
    }
    if let Some(d) = set("XDG_STATE_HOME") {
        return Ok(d.join("imux"));
    }
    match set("HOME") {
        Some(h) => Ok(h.join(".local/state/imux")),
        None => Err(io::Error::new(io::ErrorKind::NotFound, "no state directory: HOME is not set")),
    }
}

/// The rolling copies of the autosave (`autosave-history`, `autosave-history-interval`), shared by
/// every clone of the store so the server can change them while the autosave thread runs.
#[derive(Default)]
struct History {
    keep: AtomicUsize,
    interval: AtomicU64,
    last: Mutex<Option<Instant>>,
    /// A rolling copy was left out only because the last one was too recent, and the layout it would
    /// have held may not be in any copy yet: it is written once the interval has passed, even if
    /// the layout does not change again (the autosave itself would have nothing new to write).
    owed: AtomicBool,
}

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    history: Arc<History>,
}

/// Where the rolling copies are, below the snapshots directory.
const HISTORY: &str = "history";

/// The lines of a snapshot that say when it was taken do not make it another picture of the layout.
fn without_time(text: &str) -> String {
    text.lines().filter(|l| !l.starts_with("saved ")).collect::<Vec<_>>().join("\n")
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store { root, history: Arc::new(History::default()) }
    }

    /// Keep at most `keep` rolling copies (none when 0), one per `interval` seconds at most.
    pub fn set_history(&self, keep: usize, interval: u64) {
        self.history.keep.store(keep, Ordering::Relaxed);
        self.history.interval.store(interval, Ordering::Relaxed);
    }

    /// A rolling copy is owed (see `History::owed`) and the interval since the last one has passed.
    pub fn history_due(&self) -> bool {
        if !self.history.owed.load(Ordering::Relaxed) {
            return false;
        }
        let wait = Duration::from_secs(self.history.interval.load(Ordering::Relaxed));
        let last = self.history.last.lock().unwrap_or_else(|p| p.into_inner());
        last.is_none_or(|t| t.elapsed() >= wait)
    }

    /// Write the rolling copy that is owed, from the layout as it is now. The autosave files are
    /// not touched: the autosave already holds this layout.
    pub fn write_owed_history(&self, text: &str) -> io::Result<()> {
        self.save_history(text)
    }

    fn history_dir(&self) -> PathBuf {
        self.root.join(SNAPSHOTS).join(HISTORY)
    }

    /// The seconds of each rolling copy, oldest first.
    fn history_stamps(&self) -> io::Result<Vec<u64>> {
        let dir = self.history_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ctx(e, &format!("read {}", dir.display()))),
        };
        let mut stamps: Vec<u64> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok()?.strip_suffix(&format!(".{EXT}"))?.parse().ok())
            .collect();
        stamps.sort_unstable();
        Ok(stamps)
    }

    /// The rolling copies, newest first: the name to give `restore history/NAME`, and what it holds.
    pub fn list_history(&self) -> io::Result<Vec<(String, Snapshot)>> {
        let mut out = Vec::new();
        for stamp in self.history_stamps()?.into_iter().rev() {
            let path = self.history_dir().join(format!("{stamp}.{EXT}"));
            match self.read(&path) {
                Ok(Some(snap)) => out.push((stamp.to_string(), snap)),
                Ok(None) => {}
                Err(e) => crate::log::warn(&format!("history {stamp} unusable: {e}")),
            }
        }
        Ok(out)
    }

    /// One more rolling copy of the autosave `text`, unless they are off, the last was written less
    /// than the interval ago, or it holds the same layout as the newest one. The oldest copies
    /// beyond the count go afterwards, so a failed write loses none.
    fn save_history(&self, text: &str) -> io::Result<()> {
        let keep = self.history.keep.load(Ordering::Relaxed);
        if keep == 0 {
            self.history.owed.store(false, Ordering::Relaxed);
            return Ok(());
        }
        let wait = Duration::from_secs(self.history.interval.load(Ordering::Relaxed));
        let mut last = self.history.last.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|t| t.elapsed() < wait) {
            // too soon: this layout may be in no copy, so one is owed when the interval is over
            self.history.owed.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let stamps = self.history_stamps()?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if let Some(newest) = stamps.last() {
            let same = read_limited(&self.history_dir().join(format!("{newest}.{EXT}"))).is_ok_and(|old| without_time(&old) == without_time(text));
            if same || *newest >= now {
                *last = Some(Instant::now());
                // the newest copy already holds this layout, or it was made in this very second
                // (copies are named by the second) and holds another one
                self.history.owed.store(!same, Ordering::Relaxed);
                return Ok(());
            }
        }
        self.ensure_dir(&self.root)?;
        self.ensure_dir(&self.root.join(SNAPSHOTS))?;
        self.ensure_dir(&self.history_dir())?;
        write_atomic(&self.history_dir().join(format!("{now}.{EXT}")), text.as_bytes())?;
        *last = Some(Instant::now());
        self.history.owed.store(false, Ordering::Relaxed);
        let mut all = stamps;
        all.push(now);
        let excess = all.len().saturating_sub(keep);
        for stamp in all.into_iter().take(excess) {
            if let Err(e) = remove_if_present(&self.history_dir().join(format!("{stamp}.{EXT}"))) {
                crate::log::warn(&format!("remove history {stamp}: {e}"));
            }
        }
        Ok(())
    }

    pub fn from_env() -> io::Result<Store> {
        state_dir(|k| std::env::var_os(k)).map(|root| match crate::paths::state_suffix() {
            Some(name) => Store::new(root.join(name)),
            None => Store::new(root),
        })
    }

    /// Keep the current autosave as the previous one, then write the new one.
    #[cfg(test)]
    pub fn save_autosave(&self, snap: &Snapshot) -> io::Result<()> {
        self.save_autosave_text(&format::encode(snap))
    }

    /// The same for a snapshot that is already encoded (the autosave thread).
    pub fn save_autosave_text(&self, text: &str) -> io::Result<()> {
        self.ensure_dir(&self.root)?;
        let cur = self.root.join(AUTOSAVE);
        if cur.exists() {
            fs::rename(&cur, self.root.join(AUTOSAVE_PREV))
                .map_err(|e| ctx(e, "keep previous autosave"))?;
        }
        write_atomic(&cur, text.as_bytes())?;
        if let Err(e) = self.save_history(text) {
            crate::log::warn(&format!("autosave history: {e}"));
        }
        Ok(())
    }

    /// Run when the server starts, before anything is restored or saved: an autosave that does not
    /// read back (damaged, or written by a newer imux) is moved aside, so the first save does not
    /// rotate it over the good previous copy. Returns whether one was moved.
    pub fn set_aside_unreadable_autosave(&self) -> bool {
        let cur = self.root.join(AUTOSAVE);
        let Err(e) = self.read(&cur) else { return false };
        crate::log::warn(&format!("{AUTOSAVE} unusable: {e}; kept aside as {BAD_PREFIX}*, the previous copy stays"));
        match self.keep_aside(&cur).and_then(|()| remove_if_present(&cur)) {
            Ok(()) => true,
            Err(e) => {
                crate::log::warn(&format!("set {AUTOSAVE} aside: {e}"));
                false
            }
        }
    }

    /// Copy an unreadable autosave to `autosave.bad.<milliseconds>` and drop the oldest of those
    /// beyond the newest `KEEP_BAD`. Nothing ever restores from them.
    fn keep_aside(&self, file: &Path) -> io::Result<()> {
        let f = OpenOptions::new().read(true).custom_flags(nix::libc::O_NOFOLLOW).open(file)?;
        if !f.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a regular file"));
        }
        let mut bytes = Vec::new();
        f.take(MAX_SNAPSHOT_FILE).read_to_end(&mut bytes)?;
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        write_atomic(&self.root.join(format!("{BAD_PREFIX}{ms}")), &bytes)?;
        let mut aside: Vec<(u128, PathBuf)> = fs::read_dir(&self.root)?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                Some((name.strip_prefix(BAD_PREFIX)?.parse::<u128>().ok()?, e.path()))
            })
            .collect();
        aside.sort();
        let excess = aside.len().saturating_sub(KEEP_BAD);
        for (_, path) in aside.into_iter().take(excess) {
            if let Err(e) = remove_if_present(&path) {
                crate::log::warn(&format!("remove {}: {e}", path.display()));
            }
        }
        Ok(())
    }

    /// The last autosave; a missing or damaged file falls back to the previous one. `Ok(None)`
    /// when there is nothing to restore.
    pub fn load_autosave(&self) -> io::Result<Option<Snapshot>> {
        for name in [AUTOSAVE, AUTOSAVE_PREV] {
            match self.read(&self.root.join(name)) {
                Ok(Some(s)) => return Ok(Some(s)),
                Ok(None) => {}
                Err(e) => crate::log::warn(&format!("{name} unusable: {e}")),
            }
        }
        Ok(None)
    }

    /// Remove both autosave files (a deliberate end of the server). Missing files are fine.
    pub fn delete_autosave(&self) -> io::Result<()> {
        let mut first = Ok(());
        for name in [AUTOSAVE, AUTOSAVE_PREV] {
            if let Err(e) = remove_if_present(&self.root.join(name)) {
                crate::log::warn(&format!("remove {name}: {e}"));
                if first.is_ok() {
                    first = Err(e);
                }
            }
        }
        first
    }

    pub fn save_named(&self, name: &str, snap: &Snapshot) -> io::Result<()> {
        let path = self.named_path(name)?;
        self.ensure_dir(&self.root)?;
        self.ensure_dir(&self.root.join(SNAPSHOTS))?;
        write_atomic(&path, format::encode(snap).as_bytes())
    }

    pub fn load_named(&self, name: &str) -> io::Result<Snapshot> {
        let path = match name.strip_prefix("history/") {
            Some(stamp) if !stamp.is_empty() && stamp.len() <= 20 && stamp.bytes().all(|b| b.is_ascii_digit()) => {
                self.history_dir().join(format!("{stamp}.{EXT}"))
            }
            Some(_) => return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("no rolling copy named {name} (see imux snapshots --history)"))),
            None => self.named_path(name)?,
        };
        match self.read(&path)? {
            Some(s) => Ok(s),
            None => Err(io::Error::new(io::ErrorKind::NotFound, format!("no snapshot named {name}"))),
        }
    }

    pub fn delete_named(&self, name: &str) -> io::Result<()> {
        let path = self.named_path(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(io::Error::new(io::ErrorKind::NotFound, format!("no snapshot named {name}")))
            }
            Err(e) => Err(ctx(e, &format!("remove {}", path.display()))),
        }
    }

    /// Names of the saved snapshots, sorted.
    pub fn list_named(&self) -> io::Result<Vec<String>> {
        let dir = self.root.join(SNAPSHOTS);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ctx(e, &format!("read {}", dir.display()))),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| ctx(e, &format!("read {}", dir.display())))?;
            let file = entry.file_name();
            let Some(file) = file.to_str() else { continue };
            if let Some(stem) = file.strip_suffix(&format!(".{EXT}"))
                && valid_name(stem)
            {
                names.push(stem.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    fn named_path(&self, name: &str) -> io::Result<PathBuf> {
        if !valid_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bad snapshot name {name:?}: use letters, digits, `.`, `_` and `-`"),
            ));
        }
        Ok(self.root.join(SNAPSHOTS).join(format!("{name}.{EXT}")))
    }

    fn read(&self, path: &Path) -> io::Result<Option<Snapshot>> {
        let text = match read_limited(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ctx(e, &format!("read {}", path.display()))),
        };
        match format::decode(&text) {
            Ok(s) => Ok(Some(s)),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        }
    }

    fn ensure_dir(&self, dir: &Path) -> io::Result<()> {
        match DirBuilder::new().recursive(true).mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) => return Err(ctx(e, &format!("create {}", dir.display()))),
        }
        // `chmod` follows a symbolic link, so look at the entry itself first: it has to be a real
        // directory of ours, not a link that would have the mode of some other directory changed.
        let meta = fs::symlink_metadata(dir).map_err(|e| ctx(e, &format!("stat {}", dir.display())))?;
        if !meta.is_dir() || meta.uid() != nix::unistd::getuid().as_raw() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} must be a directory (not a link) owned by you", dir.display()),
            ));
        }
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| ctx(e, &format!("chmod {}", dir.display())))
    }
}

/// Largest snapshot file read back, in bytes (a real one is a few KiB).
const MAX_SNAPSHOT_FILE: u64 = 16 * 1024 * 1024;

/// The text of a snapshot file: opened without following a final symbolic link, a regular file,
/// at most `MAX_SNAPSHOT_FILE` bytes.
fn read_limited(path: &Path) -> io::Result<String> {
    let f = OpenOptions::new().read(true).custom_flags(nix::libc::O_NOFOLLOW).open(path)?;
    let meta = f.metadata()?;
    if !meta.is_file() || meta.len() > MAX_SNAPSHOT_FILE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a regular file of reasonable size"));
    }
    let mut text = String::new();
    f.take(MAX_SNAPSHOT_FILE).read_to_string(&mut text)?;
    Ok(text)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ctx(e, &format!("remove {}", path.display()))),
    }
}

/// Write `data` to `path` so a reader sees the old file or the whole new one, never half:
/// a private temporary file next to it, flushed to disk, then renamed over it.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"));
    };
    let tmp = path.with_file_name(format!(".{name}.tmp{}", std::process::id()));
    let result = write_synced(&tmp, data).and_then(|()| {
        fs::rename(&tmp, path).map_err(|e| ctx(e, &format!("rename to {}", path.display())))
    });
    if result.is_err()
        && let Err(e) = remove_if_present(&tmp)
    {
        crate::log::warn(&format!("remove temporary file: {e}"));
    }
    result
}

fn write_synced(tmp: &Path, data: &[u8]) -> io::Result<()> {
    let mut f: File = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(tmp)
        .map_err(|e| ctx(e, &format!("create {}", tmp.display())))?;
    f.write_all(data).map_err(|e| ctx(e, &format!("write {}", tmp.display())))?;
    f.sync_all().map_err(|e| ctx(e, &format!("sync {}", tmp.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::layout::Split;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imux-persist-{tag}-{}", std::process::id()));
        if let Err(e) = fs::remove_dir_all(&dir) {
            assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}");
        }
        dir
    }

    fn snap(name: &str) -> Snapshot {
        let leaf = |cwd: &str| Shape::Leaf(SPane { cwd: cwd.into(), argv: vec![], claude: false, resume: None, outer: Vec::new(), name: None });
        Snapshot {
            saved_at: 7,
            recent: 0,
            sessions: vec![SSession {
                name: name.into(),
                current: 0,
                windows: vec![SWindow {
                    name: "w".into(),
                    auto_rename: false,
                    active_leaf: 1,
                    tree: Shape::Branch {
                        kind: Split::Stacked,
                        ratio: 0.5,
                        a: Box::new(leaf("/a")),
                        b: Box::new(leaf("/b")),
                    },
                    meta: vec![],
                }],
            }],
        }
    }

    #[test]
    fn names_that_could_escape_the_directory_are_refused() {
        for ok in ["work", "a.b-c_d", "2026-11-05"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", ".hidden", "..", "a/b", "a b", "x\0y", "\u{e9}", &"n".repeat(65)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_state_dir_prefers_the_override_then_xdg_then_home() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
        };
        let all = &[("IMUX_STATE_DIR", "/o"), ("XDG_STATE_HOME", "/x"), ("HOME", "/h")];
        assert_eq!(state_dir(env(all)).unwrap(), PathBuf::from("/o"));
        assert_eq!(state_dir(env(&all[1..])).unwrap(), PathBuf::from("/x/imux"));
        assert_eq!(state_dir(env(&all[2..])).unwrap(), PathBuf::from("/h/.local/state/imux"));
        assert!(state_dir(env(&[("IMUX_STATE_DIR", "")])).is_err());
    }

    #[test]
    fn an_autosave_is_kept_with_the_one_before_it_and_removed_on_delete() {
        let root = scratch("auto");
        let store = Store::new(root.clone());
        assert_eq!(store.load_autosave().unwrap(), None);
        store.save_autosave(&snap("one")).unwrap();
        store.save_autosave(&snap("two")).unwrap();
        assert_eq!(store.load_autosave().unwrap(), Some(snap("two")));
        assert!(root.join(AUTOSAVE_PREV).exists());
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&root.join(AUTOSAVE)), 0o600);
        store.delete_autosave().unwrap();
        store.delete_autosave().unwrap();
        assert_eq!(store.load_autosave().unwrap(), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_damaged_autosave_falls_back_to_the_previous_one() {
        let root = scratch("damaged");
        let store = Store::new(root.clone());
        store.save_autosave(&snap("one")).unwrap();
        store.save_autosave(&snap("two")).unwrap();
        fs::write(root.join(AUTOSAVE), "imux-snapshot 1\nsav").unwrap();
        assert_eq!(store.load_autosave().unwrap(), Some(snap("one")));
        fs::write(root.join(AUTOSAVE_PREV), "junk").unwrap();
        assert_eq!(store.load_autosave().unwrap(), None);
        fs::remove_dir_all(&root).unwrap();
    }

    fn aside(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.starts_with(BAD_PREFIX)).collect();
        names.sort();
        names
    }

    #[test]
    fn a_damaged_autosave_does_not_push_the_good_previous_copy_out() {
        let root = scratch("keepgood");
        let store = Store::new(root.clone());
        store.save_autosave(&snap("one")).unwrap();
        store.save_autosave(&snap("two")).unwrap();
        fs::write(root.join(AUTOSAVE), "imux-snapshot 1\nsav").unwrap();
        assert!(store.set_aside_unreadable_autosave());
        assert!(!root.join(AUTOSAVE).exists());
        store.save_autosave(&snap("three")).unwrap();
        assert_eq!(fs::read_to_string(root.join(AUTOSAVE_PREV)).unwrap(), format::encode(&snap("one")), "the good previous copy stays, the damaged one never replaced it");
        assert_eq!(store.load_autosave().unwrap(), Some(snap("three")));
        assert_eq!(aside(&root).len(), 1, "the damaged file is kept aside");
        assert_eq!(fs::read(root.join(&aside(&root)[0])).unwrap(), b"imux-snapshot 1\nsav");
        assert!(!store.set_aside_unreadable_autosave(), "a good file is left alone");
        assert_eq!(aside(&root).len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    fn history_files(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root.join(SNAPSHOTS).join(HISTORY))
            .map(|d| d.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    fn old_copy(root: &Path, stamp: u64, snap: &Snapshot) {
        let dir = root.join(SNAPSHOTS).join(HISTORY);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{stamp}.{EXT}")), format::encode(snap)).unwrap();
    }

    #[test]
    fn rolling_copies_are_off_until_asked_for() {
        let root = scratch("hist-off");
        let store = Store::new(root.clone());
        store.save_autosave(&snap("one")).unwrap();
        assert!(history_files(&root).is_empty());
        store.set_history(0, 1);
        store.save_autosave(&snap("two")).unwrap();
        assert!(history_files(&root).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_rolling_copy_is_written_once_per_interval() {
        let root = scratch("hist-interval");
        let store = Store::new(root.clone());
        store.set_history(5, 3600);
        store.save_autosave(&snap("one")).unwrap();
        assert_eq!(history_files(&root).len(), 1);
        store.save_autosave(&snap("two")).unwrap();
        assert_eq!(history_files(&root).len(), 1, "the interval has not passed");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root.join(SNAPSHOTS).join(HISTORY)), 0o700);
        let file = root.join(SNAPSHOTS).join(HISTORY).join(&history_files(&root)[0]);
        assert_eq!(mode(&file), 0o600);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_copy_left_out_for_coming_too_soon_is_owed_and_written_when_the_interval_is_over() {
        let root = scratch("hist-owed");
        let store = Store::new(root.clone());
        old_copy(&root, 100, &snap("one"));
        store.set_history(5, 3600);
        *store.history.last.lock().unwrap() = Some(Instant::now());
        assert!(!store.history_due(), "nothing is owed yet");
        // a change comes inside the interval: no copy, but one is owed
        store.save_autosave(&snap("two")).unwrap();
        assert_eq!(history_files(&root), ["100.imux"]);
        assert!(!store.history_due(), "owed, but the interval has not passed");
        // the interval passes with no further change: the owed copy is written from the layout as it is
        *store.history.last.lock().unwrap() = Some(Instant::now() - Duration::from_secs(7200));
        assert!(store.history_due());
        store.write_owed_history(&format::encode(&snap("two"))).unwrap();
        assert_eq!(history_files(&root).len(), 2);
        assert!(!store.history_due(), "paid");
        let newest = history_files(&root).pop().unwrap();
        assert_eq!(fs::read_to_string(root.join(SNAPSHOTS).join(HISTORY).join(newest)).unwrap(), format::encode(&snap("two")));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nothing_is_owed_when_the_newest_copy_already_holds_the_layout() {
        let root = scratch("hist-not-owed");
        let store = Store::new(root.clone());
        old_copy(&root, 100, &snap("one"));
        store.set_history(5, 0);
        store.save_autosave(&snap("one")).unwrap();
        assert!(!store.history_due());
        assert_eq!(history_files(&root), ["100.imux"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_layout_equal_to_the_newest_copy_is_not_copied_again() {
        let root = scratch("hist-same");
        let store = Store::new(root.clone());
        old_copy(&root, 100, &snap("one"));
        store.set_history(5, 0);
        let mut later = snap("one");
        later.saved_at = 999;
        store.save_autosave(&later).unwrap();
        assert_eq!(history_files(&root), ["100.imux"], "only the time differs");
        store.save_autosave(&snap("other")).unwrap();
        assert_eq!(history_files(&root).len(), 2, "another layout is copied");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_oldest_copies_beyond_the_count_go_after_the_new_one_is_written() {
        let root = scratch("hist-prune");
        let store = Store::new(root.clone());
        for stamp in [100, 200, 300, 400] {
            old_copy(&root, stamp, &snap(&format!("s{stamp}")));
        }
        store.set_history(3, 0);
        store.save_autosave(&snap("new")).unwrap();
        let mut stamps: Vec<u64> = history_files(&root).iter().map(|f| f.trim_end_matches(".imux").parse().unwrap()).collect();
        stamps.sort_unstable();
        assert_eq!(stamps.len(), 3);
        assert_eq!(&stamps[..2], &[300, 400], "the two oldest went");
        assert!(stamps[2] > 1_000_000, "the new one is a clock time");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rolling_copies_list_newest_first_and_restore_by_name_only() {
        let root = scratch("hist-list");
        let store = Store::new(root.clone());
        old_copy(&root, 100, &snap("a"));
        old_copy(&root, 300, &snap("c"));
        old_copy(&root, 200, &snap("b"));
        fs::write(root.join(SNAPSHOTS).join(HISTORY).join("notes.txt"), "x").unwrap();
        let names: Vec<String> = store.list_history().unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["300", "200", "100"]);
        assert_eq!(store.load_named("history/200").unwrap(), snap("b"));
        assert_eq!(store.load_named("history/999").unwrap_err().kind(), io::ErrorKind::NotFound);
        for bad in ["history/", "history/../x", "history/2/3", "history/abc", "history/-1"] {
            assert_eq!(store.load_named(bad).unwrap_err().kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nothing_is_set_aside_when_there_is_no_autosave() {
        let root = scratch("noauto");
        let store = Store::new(root.clone());
        assert!(!store.set_aside_unreadable_autosave());
        fs::create_dir_all(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_file_from_a_newer_imux_is_kept_aside_not_rotated() {
        let root = scratch("newer");
        let store = Store::new(root.clone());
        store.save_autosave(&snap("one")).unwrap();
        store.save_autosave(&snap("two")).unwrap();
        let text = format::encode(&snap("future")).replacen("imux-snapshot 1", "imux-snapshot 99", 1);
        fs::write(root.join(AUTOSAVE), &text).unwrap();
        assert!(store.set_aside_unreadable_autosave());
        store.save_autosave(&snap("three")).unwrap();
        assert_eq!(fs::read_to_string(root.join(AUTOSAVE_PREV)).unwrap(), format::encode(&snap("one")));
        assert_eq!(fs::read_to_string(root.join(&aside(&root)[0])).unwrap(), text);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_the_newest_files_kept_aside_stay() {
        let root = scratch("bounded");
        let store = Store::new(root.clone());
        store.save_autosave(&snap("good")).unwrap();
        for i in 0..6 {
            fs::write(root.join(AUTOSAVE), format!("junk {i}")).unwrap();
            assert!(store.set_aside_unreadable_autosave());
            store.save_autosave(&snap("next")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        let kept = aside(&root);
        assert_eq!(kept.len(), KEEP_BAD);
        let texts: Vec<String> = kept.iter().map(|n| fs::read_to_string(root.join(n)).unwrap()).collect();
        assert_eq!(texts, ["junk 3", "junk 4", "junk 5"], "the oldest went");
        assert!(store.load_autosave().unwrap().is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn named_snapshots_save_list_load_and_delete() {
        let root = scratch("named");
        let store = Store::new(root.clone());
        assert_eq!(store.list_named().unwrap(), Vec::<String>::new());
        store.save_named("beta", &snap("b")).unwrap();
        store.save_named("alpha", &snap("a")).unwrap();
        fs::write(root.join(SNAPSHOTS).join("notes.txt"), "x").unwrap();
        assert_eq!(store.list_named().unwrap(), vec!["alpha", "beta"]);
        assert_eq!(store.load_named("beta").unwrap(), snap("b"));
        assert_eq!(store.load_named("nope").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(store.save_named("../x", &snap("x")).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        store.delete_named("alpha").unwrap();
        assert_eq!(store.delete_named("alpha").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(store.list_named().unwrap(), vec!["beta"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_failed_atomic_write_leaves_the_old_file_and_no_temporary() {
        let root = scratch("atomic");
        let store = Store::new(root.clone());
        store.save_named("keep", &snap("old")).unwrap();
        let dir = root.join(SNAPSHOTS);
        let target = dir.join("keep.imux");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let res = write_atomic(&target, b"new");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        if nix::unistd::getuid().is_root() {
            assert!(res.is_ok());
        } else {
            assert!(res.is_err());
            assert_eq!(store.load_named("keep").unwrap(), snap("old"));
            let stray: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
            assert_eq!(stray, vec![OsString::from("keep.imux")]);
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_state_directory_that_is_a_link_is_refused_and_its_target_keeps_its_mode() {
        let root = scratch("linkdir");
        let target = root.join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.join("state");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = Store::new(link).save_named("x", &snap("x")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o755);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_snapshot_that_is_a_link_or_too_big_is_not_read() {
        let root = scratch("linkfile");
        let store = Store::new(root.clone());
        store.save_named("real", &snap("real")).unwrap();
        let dir = root.join(SNAPSHOTS);
        std::os::unix::fs::symlink(dir.join("real.imux"), dir.join("link.imux")).unwrap();
        assert!(store.load_named("link").is_err());
        let big = File::create(dir.join("big.imux")).unwrap();
        big.set_len(MAX_SNAPSHOT_FILE + 1).unwrap();
        assert!(store.load_named("big").is_err());
        assert_eq!(store.load_named("real").unwrap(), snap("real"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_temporary_file_that_is_a_link_is_not_written_through() {
        let root = scratch("linktmp");
        fs::create_dir_all(&root).unwrap();
        let victim = root.join("victim");
        fs::write(&victim, "keep").unwrap();
        let path = root.join("auto.imux");
        let tmp = root.join(format!(".auto.imux.tmp{}", std::process::id()));
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
        assert!(write_atomic(&path, b"new").is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
        fs::remove_dir_all(&root).unwrap();
    }
}
