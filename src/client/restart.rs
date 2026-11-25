//! `imux restart-server`: end the running server in the way that keeps the sessions, so that the next
//! one (this imux, which may be newer) brings them back.
//!
//! The server is asked to stop with SIGTERM, which makes it write one more autosave first, and the
//! next server restores that autosave when its first client attaches (`restore-on-start`). A signal
//! works against a server of any version: an older one that speaks another protocol cannot be asked
//! anything, but it can still be stopped. What a restart keeps is what a restore keeps: the layout,
//! the directories, the programs in `restore-commands`, and the agents with their names. Programs
//! that are not listed end, and the scrollback is lost, which is why the command asks first.

use std::io::{self, BufRead, IsTerminal, Write};
use std::os::fd::AsFd;
use std::time::{Duration, Instant, SystemTime};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use super::Mode;
use crate::proto::{ClientMsg, RunArgs};
use crate::tty::proc;

/// How long the server has for its last autosave and its exit.
const STOP_WITHIN: Duration = Duration::from_secs(20);

/// What a server says it holds, as far as it can be asked.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub sessions: usize,
    pub windows: usize,
    /// Clients attached now (any terminal, also other ones than this).
    pub clients: usize,
    /// `autosave-interval` is not 0: the server writes the autosave it stops with.
    pub autosave: bool,
    /// `restore-on-start`: the next server brings the autosave back by itself.
    pub restore: bool,
}

/// `list-sessions -F "#{session_windows}"` lines: one session each, with its number of windows.
pub fn count_sessions(text: &str) -> (usize, usize) {
    let mut sessions = 0;
    let mut windows = 0;
    for line in text.lines() {
        if let Ok(n) = line.trim().parse::<usize>() {
            sessions += 1;
            windows += n;
        }
    }
    (sessions, windows)
}

/// Whether an autosave written at `modified` is the one the server wrote when it stopped: not older than
/// the moment the stop was asked for (a second of slack for file systems that keep whole seconds).
pub fn written_since(modified: Option<SystemTime>, asked: SystemTime) -> bool {
    modified.is_some_and(|m| m + Duration::from_secs(1) >= asked)
}

/// A yes to the question: `y` or `yes`, in any case. Anything else, an empty line included, is no.
pub fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The sentence about what is lost, from what the server holds.
pub fn describe(held: &Option<Held>, pid: Option<i32>) -> String {
    let who = pid.map_or_else(|| "The server".to_string(), |p| format!("The server (process {p})"));
    let mut out = format!("{who} will be stopped and started again with this imux ({}).\n", env!("CARGO_PKG_VERSION"));
    match held {
        Some(h) => {
            out.push_str(&format!("  It holds {} session{} with {} window{}", h.sessions, plural(h.sessions), h.windows, plural(h.windows)));
            if h.clients > 0 {
                out.push_str(&format!("; {} client{} attached and will be disconnected", h.clients, plural(h.clients)));
            }
            out.push_str(".\n");
            if !h.autosave {
                out.push_str("  WARNING: autosave-interval is 0, so nothing is saved when it stops: the sessions will NOT come back.\n");
            } else if !h.restore {
                out.push_str("  restore-on-start is off, so the sessions are saved but only `imux restore` brings them back.\n");
            }
        }
        None => out.push_str("  It cannot be asked what it holds (it is another version of imux); it saves its sessions when it stops.\n"),
    }
    out.push_str("  The layout, the directories, and the programs in restore-commands come back; other programs end, and the scrollback is lost.\n");
    out
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn ask(words: &[&str]) -> io::Result<String> {
    super::ask_text(ClientMsg::Run(RunArgs { words: words.iter().map(|w| w.to_string()).collect(), cwd: None }))
}

/// Ask the server what it holds; `None` when it cannot be asked.
fn held() -> Option<Held> {
    let sessions = ask(&["list-sessions", "-F", "#{session_windows}"]).ok()?;
    let (sessions, windows) = count_sessions(&sessions);
    let clients = ask(&["list-clients", "-F", "x"]).ok()?.lines().filter(|l| !l.trim().is_empty()).count();
    let flag = |name: &str| ask(&["show-options", "-gv", name]).ok().map(|v| v.trim().to_string());
    let autosave = flag("autosave-interval")?.parse::<u64>().ok()? != 0;
    let restore = matches!(flag("restore-on-start")?.as_str(), "on" | "1" | "true" | "yes");
    Some(Held { sessions, windows, clients, autosave, restore })
}

/// Whether this process runs in a pane of the server on this socket.
fn inside_a_pane() -> bool {
    std::env::var_os("IMUX_ENV").is_some() && std::env::var_os("IMUX_PANE_ID").is_some()
}

fn gone(pid: i32) -> bool {
    matches!(kill(Pid::from_raw(pid), None), Err(nix::errno::Errno::ESRCH))
}

/// One line (or the text given) on standard output; a closed output is not an error here.
fn say(text: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// `imux restart-server [--yes] [--detach]`.
pub fn restart_server(yes: bool, detach: bool) -> io::Result<i32> {
    if inside_a_pane() {
        return Err(io::Error::other(
            "this shell runs inside an imux pane, which ends with the server: detach first (prefix d) and run `imux restart-server` from a terminal outside imux",
        ));
    }
    let stream = match super::connect_raw(Mode::Attach) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            say(&format!("no server is running: the next imux starts this one ({})\n", env!("CARGO_PKG_VERSION")));
            return Ok(0);
        }
        Err(e) => return Err(e),
    };
    let pid = proc::peer_pid(stream.as_fd());
    let uid = proc::peer_uid(stream.as_fd());
    drop(stream);
    if let Some(uid) = uid
        && uid != nix::unistd::getuid().as_raw()
    {
        return Err(io::Error::other("the server on this socket belongs to another user"));
    }
    let Some(pid) = pid else {
        return Err(io::Error::other("the system does not say which process the server is, so it cannot be stopped from here: use `imux kill-server` (the sessions end)"));
    };
    let held = held();
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if !yes {
        if !interactive {
            return Err(io::Error::other("restart-server asks before it stops the server: run it in a terminal, or give --yes"));
        }
        say(&format!("{}Restart it? [y/N] ", describe(&held, Some(pid))));
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;
        if !is_yes(&answer) {
            say("nothing was changed\n");
            return Ok(1);
        }
    }
    let asked = SystemTime::now();
    match kill(Pid::from_raw(pid), Signal::SIGTERM) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(e) => return Err(io::Error::other(format!("could not stop the server (process {pid}): {e}"))),
    }
    let started = Instant::now();
    while !gone(pid) {
        if started.elapsed() > STOP_WITHIN {
            return Err(io::Error::other(format!(
                "the server (process {pid}) did not end within {} seconds; it may still be writing its last autosave: look at its log, then run this again",
                STOP_WITHIN.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The server writes its last autosave before it exits; if there is none newer than the request,
    // the sessions are not saved, and that is said while the person can still do something about it.
    let expected = held.as_ref().is_none_or(|h| h.autosave);
    let saved = !expected || crate::persist::Store::from_env().is_ok_and(|store| written_since(store.autosave_modified(), asked));
    if !saved {
        let _ = io::stderr().write_all(
            b"imux: the server has ended, but no new autosave was written: the sessions may not come back (see its log; `imux snapshots --history` lists the rolling copies, `imux restore history/NAME` brings one back)\n",
        );
    }
    if detach || !interactive {
        say(&format!("the server has ended and its sessions are saved: the next imux starts this one ({}) and brings them back\n", env!("CARGO_PKG_VERSION")));
        return Ok(0);
    }
    say(&format!("the server has ended; starting this imux ({}) and bringing the sessions back\n", env!("CARGO_PKG_VERSION")));
    super::run(Mode::NewOrAttach, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_and_windows_are_counted_from_one_number_per_session() {
        assert_eq!(count_sessions("3\n1\n12\n"), (3, 16));
        assert_eq!(count_sessions(""), (0, 0));
        assert_eq!(count_sessions("x\n \n2\n"), (1, 2), "what is not a number is not a session");
    }

    #[test]
    fn an_autosave_counts_when_it_is_not_older_than_the_request() {
        let now = SystemTime::now();
        assert!(written_since(Some(now + Duration::from_secs(5)), now));
        assert!(written_since(Some(now), now));
        assert!(written_since(Some(now - Duration::from_millis(900)), now), "a second of slack");
        assert!(!written_since(Some(now - Duration::from_secs(60)), now), "an old file is not the last one");
        assert!(!written_since(None, now), "no file at all");
    }

    #[test]
    fn only_y_and_yes_are_a_yes() {
        for yes in ["y", "Y", "yes", "YES", " y \n", "Yes\r\n"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yep", "yes please", "ok", "1"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn the_question_says_what_the_server_holds_and_what_is_lost() {
        let held = Some(Held { sessions: 2, windows: 5, clients: 3, autosave: true, restore: true });
        let text = describe(&held, Some(4242));
        assert!(text.contains("process 4242") && text.contains("2 sessions with 5 windows"), "{text}");
        assert!(text.contains("3 clients attached and will be disconnected"), "{text}");
        assert!(text.contains("other programs end, and the scrollback is lost"), "{text}");
        assert!(!text.contains("WARNING") && !text.contains("restore-on-start is off"), "{text}");
        let one = describe(&Some(Held { sessions: 1, windows: 1, clients: 1, autosave: true, restore: true }), None);
        assert!(one.starts_with("The server will be stopped") && one.contains("1 session with 1 window; 1 client attached"), "{one}");
        let none = describe(&Some(Held { sessions: 1, windows: 1, clients: 0, autosave: true, restore: true }), None);
        assert!(!none.contains("attached"), "{none}");
    }

    #[test]
    fn it_warns_when_nothing_would_be_saved_and_when_restoring_is_by_hand_and_when_it_cannot_ask() {
        let off = describe(&Some(Held { sessions: 1, windows: 1, clients: 0, autosave: false, restore: true }), None);
        assert!(off.contains("WARNING: autosave-interval is 0") && off.contains("will NOT come back"), "{off}");
        let by_hand = describe(&Some(Held { sessions: 1, windows: 1, clients: 0, autosave: true, restore: false }), None);
        assert!(by_hand.contains("restore-on-start is off") && by_hand.contains("imux restore"), "{by_hand}");
        let unknown = describe(&None, Some(7));
        assert!(unknown.contains("cannot be asked what it holds") && unknown.contains("saves its sessions when it stops"), "{unknown}");
    }
}
