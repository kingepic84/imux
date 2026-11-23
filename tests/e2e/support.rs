//! Shared pieces of the end-to-end tests: an isolated environment (a private TMPDIR, which holds the
//! server socket, and a private state directory), a runner for the `imux` command line, a client
//! attached through a real pseudo-terminal (`portable-pty`, a dev-only dependency), polling waits
//! instead of sleeps, and an assertion recorder that names every check that failed.
#![allow(dead_code)]

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use regex_lite::Regex;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The imux binary under test.
pub const BIN: &str = env!("CARGO_BIN_EXE_imux");

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static BARRIERS: AtomicUsize = AtomicUsize::new(0);

/// What a command printed and how it ended.
#[derive(Debug, Clone)]
pub struct Res {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Res {
    /// Standard output without its trailing newlines, as `$(...)` gives it.
    pub fn text(&self) -> String {
        self.out.trim_end_matches('\n').to_string()
    }
    /// Both streams, standard output first.
    pub fn all(&self) -> String {
        format!("{}{}", self.out, self.err)
    }
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// A private world for one test: its own TMPDIR (the socket directory lives under it), state
/// directory and configuration, so tests run side by side without meeting.
pub struct Env {
    pub dir: PathBuf,
    vars: Vec<(String, String)>,
    removed: Vec<String>,
}

impl Env {
    pub fn new(name: &str) -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = PathBuf::from(format!("/tmp/ie-{}-{}-{}", std::process::id(), n, name));
        std::fs::create_dir_all(&dir).expect("a private directory");
        let mut e = Env { dir: dir.clone(), vars: Vec::new(), removed: Vec::new() };
        e.set("TMPDIR", dir.to_str().unwrap());
        e.set("IMUX_STATE_DIR", dir.join("state").to_str().unwrap());
        e.set("IMUX_CONFIG", "/dev/null");
        e.set("SHELL", "/bin/sh");
        // the tests were written for a terminal with 24-bit color (set here for every test)
        e.set("COLORTERM", "truecolor");
        e.set("BIN", BIN);
        // A test run from inside a pane must not talk to that pane's server or change its names.
        for v in ["IMUX_SOCKET_PATH", "IMUX_SERVER_SOCKET", "IMUX_SOCKET_NAME", "IMUX", "TMUX", "TMUX_PANE", "IMUX_LOG_FILE", "IMUX_RECORD_OUTPUT"] {
            e.removed.push(v.to_string());
        }
        e
    }

    pub fn set(&mut self, k: &str, v: &str) -> &mut Env {
        self.vars.retain(|(a, _)| a != k);
        self.removed.retain(|a| a != k);
        self.vars.push((k.to_string(), v.to_string()));
        self
    }

    pub fn unset(&mut self, k: &str) -> &mut Env {
        self.vars.retain(|(a, _)| a != k);
        self.removed.push(k.to_string());
        self
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.vars.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    pub fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.path(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&p, text).unwrap();
        p
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read(self.path(rel)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    }

    /// The server's log, where a test looks for what the server noted.
    pub fn server_log(&self) -> String {
        let uid = unsafe { libc::getuid() };
        self.read(&format!("imux-{uid}/server.log"))
    }

    fn apply(&self, c: &mut Command) {
        for v in &self.removed {
            c.env_remove(v);
        }
        for (k, v) in &self.vars {
            c.env(k, v);
        }
    }

    /// A command with this environment.
    pub fn command(&self, prog: &str) -> Command {
        let mut c = Command::new(prog);
        self.apply(&mut c);
        c
    }

    /// `imux ARGS`, waited for (at most 60 seconds).
    pub fn run(&self, args: &[&str]) -> Res {
        let mut c = self.command(BIN);
        c.args(args);
        run_command(c, Duration::from_secs(60), None)
    }

    /// `imux ARGS` run from the directory `cwd`.
    pub fn run_cwd(&self, cwd: &Path, args: &[&str]) -> Res {
        let mut c = self.command(BIN);
        c.args(args).current_dir(cwd);
        run_command(c, Duration::from_secs(60), None)
    }

    /// `imux ARGS` with some variables set (or, with an empty value, removed) for this one command.
    pub fn run_env(&self, args: &[&str], vars: &[(&str, &str)]) -> Res {
        let mut c = self.command(BIN);
        c.args(args);
        for (k, v) in vars {
            c.env(k, v);
        }
        run_command(c, Duration::from_secs(60), None)
    }

    /// `imux ARGS` with `input` on its standard input.
    pub fn run_in(&self, args: &[&str], input: &str) -> Res {
        let mut c = self.command(BIN);
        c.args(args);
        run_command(c, Duration::from_secs(60), Some(input.as_bytes().to_vec()))
    }

    /// `imux ARGS` that must not take longer than `limit`.
    pub fn run_for(&self, args: &[&str], limit: Duration) -> Res {
        let mut c = self.command(BIN);
        c.args(args);
        run_command(c, limit, None)
    }

    /// Standard output of `imux ARGS`, trailing newlines removed.
    pub fn out(&self, args: &[&str]) -> String {
        self.run(args).text()
    }

    /// Both streams of `imux ARGS`.
    pub fn all(&self, args: &[&str]) -> String {
        self.run(args).all()
    }

    /// `imux ARGS` and its exit status.
    pub fn status(&self, args: &[&str]) -> i32 {
        self.run(args).code
    }

    /// A line of `sh` with this environment (BIN is set), for the odd thing that is easiest said that way.
    pub fn sh(&self, script: &str) -> Res {
        let mut c = self.command("sh");
        c.arg("-c").arg(script);
        run_command(c, Duration::from_secs(60), None)
    }

    /// The server's session, started detached, named `name` with a first window `window`.
    pub fn session(&self, name: &str, window: &str) {
        let r = self.run(&["new-session", "-d", "-s", name, "-n", window]);
        assert!(r.success(), "new-session failed: {}", r.all());
    }

    /// The text of a pane as capture-pane prints it.
    pub fn capture(&self, pane: &str) -> String {
        self.run(&["capture-pane", "-p", "-t", pane]).out
    }

    /// `display-message -p` for a pane or window.
    pub fn var(&self, target: &str, format: &str) -> String {
        self.run(&["display-message", "-p", "-t", target, format]).all().trim_end_matches('\n').to_string()
    }

    /// Wait until a pane's text matches the extended regular expression `pattern` (10 seconds).
    pub fn shows(&self, pane: &str, pattern: &str) -> bool {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        wait_until(Duration::from_secs(10), || re.is_match(&self.capture(pane)))
    }

    /// Does a pane show a line that is exactly `text` (grep -x)? Checked once.
    pub fn has_line(&self, pane: &str, text: &str) -> bool {
        self.capture(pane).lines().any(|l| l == text)
    }

    /// Wait until a pane shows a line that is exactly `text`.
    pub fn shows_line(&self, pane: &str, text: &str) -> bool {
        wait_until(Duration::from_secs(10), || self.has_line(pane, text))
    }

    /// Wait until no client is attached any more.
    pub fn no_clients(&self) -> bool {
        wait_until(Duration::from_secs(15), || self.out(&["list-clients"]).is_empty())
    }

    /// Wait until a client is attached.
    pub fn has_client(&self) -> bool {
        wait_until(Duration::from_secs(15), || !self.out(&["list-clients"]).is_empty())
    }

    /// A client attached to `session` for the length of the closure, then gone. What it wrote is
    /// returned. A popup or menu left open goes with its client.
    pub fn with_client(&self, session: &str, f: impl FnOnce(&mut Client)) -> String {
        let mut c = self.client(100, 30, &["attach", "-t", session]);
        assert!(self.has_client(), "the client did not attach");
        assert!(c.wait_drawn(Duration::from_secs(15)), "the client drew nothing");
        f(&mut c);
        c.send(b"\x02d");
        if c.wait_exit(Duration::from_secs(3)).is_none() {
            c.kill();
        }
        assert!(self.no_clients(), "the client did not go away");
        c.text()
    }

    /// A point in the client's output after which everything the server had queued for it is in:
    /// a message to the session is drawn behind whatever was sent before it. Use it before a check
    /// that something was *not* drawn, instead of sleeping and hoping it would have shown up.
    pub fn barrier(&self, c: &Client, session: &str) {
        let n = BARRIERS.fetch_add(1, Ordering::Relaxed);
        let mark = format!("SYNC{n}Q");
        self.run(&["display-message", "-t", &format!("{session}:"), &mark]);
        assert!(c.wait_seen(&mark, Duration::from_secs(15)), "the client never drew {mark}");
    }

    /// Wait until a pane's shell is ready for typing: the pane shows something and has stopped
    /// changing for `quiet` (a slow shell prints its banner long before its prompt takes input).
    pub fn ready(&self, pane: &str, quiet: Duration) -> bool {
        let start = Instant::now();
        let mut last = String::new();
        let mut since = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            // a pane that does not exist yet (the client is still attaching) is not ready
            let r = self.run(&["capture-pane", "-p", "-t", pane]);
            if r.code != 0 {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            let now = r.out;
            if now != last {
                last = now;
                since = Instant::now();
            } else if !last.trim().is_empty() && since.elapsed() >= quiet {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Kill the server of this environment, as a test does to start clean.
    pub fn kill_server(&self) {
        let _ = self.run_for(&["kill-server"], Duration::from_secs(10));
        // the next command must not find the dying server
        wait_until(Duration::from_secs(15), || {
            let r = self.run(&["list-sessions"]);
            r.code != 0 && r.all().contains("no server running")
        });
    }

    /// A client attached through a pseudo-terminal of the given size.
    pub fn client(&self, cols: u16, rows: u16, args: &[&str]) -> Client {
        Client::spawn(self, cols, rows, args, &[], &[])
    }

    /// A client whose own environment has some variables set and others removed (a different
    /// terminal, say).
    pub fn client_env(&self, cols: u16, rows: u16, args: &[&str], set: &[(&str, &str)], unset: &[&str]) -> Client {
        Client::spawn(self, cols, rows, args, set, unset)
    }

    /// A client started in the directory `cwd` (the pane it makes starts there too).
    pub fn client_in(&self, cwd: &Path, cols: u16, rows: u16, args: &[&str], set: &[(&str, &str)]) -> Client {
        Client::spawn_in(self, Some(cwd), cols, rows, args, set, &[])
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let mut c = Command::new(BIN);
        self.apply(&mut c);
        c.arg("kill-server").stdout(Stdio::null()).stderr(Stdio::null());
        let _ = run_command(c, Duration::from_secs(10), None);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn run_command(mut c: Command, limit: Duration, input: Option<Vec<u8>>) -> Res {
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().expect("the command starts");
    if let Some(bytes) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(&bytes);
    }
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t_out = thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let t_err = thread::spawn(move || {
        let mut v = Vec::new();
        let _ = se.read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(-1),
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break 124;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => break -1,
        }
    };
    Res { code, out: String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned(), err: String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned() }
}

/// grep -E patterns use `\<`-style extensions rarely; this maps the few differences to regex-lite.
pub fn translate_pattern(p: &str) -> String {
    // grep matches line by line, so `^` and `$` are those of a line.
    format!("(?m){}", p)
        .replace("[[:space:]]", r"\s").replace("[[:digit:]]", r"\d").replace("[[:alpha:]]", "[A-Za-z]").replace("[[:alnum:]]", "[A-Za-z0-9]")
}

/// Poll `f` every 20 ms until it is true or `limit` has passed.
pub fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if f() {
            return true;
        }
        if start.elapsed() >= limit {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// The value `f` gives once it has stood still for `quiet`: for something that runs a few times and
/// then stops, where "stopped" can only be seen as nothing happening for a while.
pub fn stable<T: PartialEq>(quiet: Duration, mut f: impl FnMut() -> T) -> T {
    let mut last = f();
    let mut since = Instant::now();
    while since.elapsed() < quiet {
        thread::sleep(Duration::from_millis(50));
        let now = f();
        if now != last {
            last = now;
            since = Instant::now();
        }
    }
    last
}

/// Records the checks of one test. Every check is printed as `ok` or `FAIL` with its label, and the
/// test fails at the end if any did not hold, naming all of them.
pub struct Check {
    name: String,
    failed: Vec<String>,
    total: usize,
    last: Instant,
}

impl Check {
    pub fn new(name: &str) -> Check {
        Check { name: name.to_string(), failed: Vec::new(), total: 0, last: Instant::now() }
    }

    /// A heading, as `echo "-- ..."` was.
    pub fn section(&mut self, title: &str) {
        println!("-- {title}");
    }

    /// One check.
    pub fn ok(&mut self, label: &str, condition: bool) {
        self.total += 1;
        // Steps that took long to reach this check are marked, so a slow wait is easy to find.
        let took = self.last.elapsed();
        self.last = Instant::now();
        let slow = if took >= Duration::from_secs(3) { format!(" ({}s)", took.as_secs()) } else { String::new() };
        if condition {
            println!("ok   {label}{slow}");
        } else {
            println!("FAIL {label}{slow}");
            self.failed.push(label.to_string());
        }
    }

    /// `grep -E pattern` on `text` held, as the shell's `check` did.
    pub fn matches(&mut self, label: &str, pattern: &str, text: &str) {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        let hit = re.is_match(text);
        if !hit {
            println!("     (missing: {pattern})");
        }
        self.ok(label, hit);
    }

    /// `text` does not match `pattern`.
    pub fn lacks(&mut self, label: &str, pattern: &str, text: &str) {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        self.ok(label, !re.is_match(text));
    }

    /// Two values are equal; the label says what they are.
    pub fn eq<T: PartialEq + std::fmt::Debug>(&mut self, label: &str, got: T, want: T) {
        let same = got == want;
        if !same {
            println!("     (got {got:?}, wanted {want:?})");
        }
        self.ok(label, same);
    }

    /// End the test: fail it when a check failed.
    pub fn finish(self) {
        if !self.failed.is_empty() {
            panic!("{}: {} of {} checks failed:\n  {}", self.name, self.failed.len(), self.total, self.failed.join("\n  "));
        }
        println!("{}: all {} checks held", self.name, self.total);
    }
}

/// A client attached to a server through a real pseudo-terminal of a given size. Everything the
/// client writes to its terminal is kept, and what a test types goes in as keystrokes.
pub struct Client {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    buf: Arc<Mutex<Vec<u8>>>,
    screen: Arc<Mutex<vt100::Parser>>,
    /// The reader thread has taken in everything the client wrote (the terminal reached its end).
    eof: Arc<std::sync::atomic::AtomicBool>,
}

impl Client {
    fn spawn(env: &Env, cols: u16, rows: u16, args: &[&str], set: &[(&str, &str)], unset: &[&str]) -> Client {
        Client::spawn_in(env, None, cols, rows, args, set, unset)
    }

    fn spawn_in(env: &Env, cwd: Option<&Path>, cols: u16, rows: u16, args: &[&str], set: &[(&str, &str)], unset: &[&str]) -> Client {
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).expect("a pseudo-terminal");
        let mut cmd = CommandBuilder::new(BIN);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
        for v in &env.removed {
            cmd.env_remove(v);
        }
        for (k, v) in &env.vars {
            cmd.env(k, v);
        }
        if env.get("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }
        for k in unset {
            cmd.env_remove(k);
        }
        for (k, v) in set {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).expect("the client starts");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("a reader");
        let writer = pair.master.take_writer().expect("a writer");
        let buf = Arc::new(Mutex::new(Vec::new()));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(std::num::NonZeroU16::new(rows).unwrap(), std::num::NonZeroU16::new(cols).unwrap(), 0)));
        let sink = Arc::clone(&buf);
        let model = Arc::clone(&screen);
        let eof = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ended = Arc::clone(&eof);
        thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        sink.lock().unwrap().extend_from_slice(&chunk[..n]);
                        model.lock().unwrap().process(&chunk[..n]);
                    }
                }
            }
            ended.store(true, Ordering::SeqCst);
        });
        Client { master: pair.master, writer, child, buf, screen, eof }
    }

    /// Type bytes into the client's terminal.
    pub fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn send_str(&mut self, text: &str) {
        self.send(text.as_bytes());
    }

    /// Type each chunk of text in order. The server handles one client's keys in the order they
    /// arrive, so nothing waits between chunks, except after a lone Escape: it is only an Escape
    /// (and not the start of a key sequence) once `escape-time` has passed without more input.
    pub fn type_chunks(&mut self, chunks: &[&str], _gap: Duration) {
        for chunk in chunks {
            self.send(chunk.as_bytes());
            if *chunk == "\x1b" {
                self.escape_pause();
            }
        }
    }

    /// Let a lone Escape just typed settle: the default `escape-time` is 10 ms.
    pub fn escape_pause(&self) {
        thread::sleep(Duration::from_millis(60));
    }

    /// A point after which every key typed so far has been handled by the server: a command typed
    /// at the prompt goes the same way the keys went, and its message comes back after what they
    /// did. (`Env::barrier` is the one for things that go the other way, from the server to the
    /// screen.) Nothing may be open that would take the prompt key itself.
    pub fn sync(&mut self) {
        let n = BARRIERS.fetch_add(1, Ordering::Relaxed);
        // what is typed shows `#{session_id}`, the message shows its value: only the message matches
        self.send(b"\x02:");
        self.send_str(&format!("display-message \"SYNC#{{session_id}}-{n}\"\r"));
        assert!(self.wait_seen(&format!("SYNC\\$[0-9]+-{n}"), Duration::from_secs(15)), "the client never showed the sync message {n}");
    }

    /// Wait until the screen no longer shows text matching `pattern`.
    pub fn wait_gone(&self, pattern: &str, limit: Duration) -> bool {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        wait_until(limit, || !re.is_match(&self.screen_text()))
    }

    /// Wait until the client has drawn its first screen (anything at all on it).
    pub fn wait_drawn(&self, limit: Duration) -> bool {
        wait_until(limit, || !self.screen_text().trim().is_empty())
    }

    /// Type keys and shell lines in order, waiting where a later step depends on an earlier one
    /// having happened. The server handles the keys in order, but a shell runs a line when it gets to
    /// it, and what the client draws is only what is on the screen when a frame is made:
    /// - after `echo LABEL-$X` (a shell line) the output `LABEL-` and a digit or letter is waited for
    ///   (after `echo TEXT`, the line `TEXT`), so the line has run (with the layout it was typed in)
    ///   and has been drawn;
    /// - after `exit` the pane is waited for to be gone (the next line goes to another pane);
    /// - after a lone Escape the escape time is waited out.
    pub fn type_steps(&mut self, e: &Env, chunks: &[&str]) {
        let limit = Duration::from_secs(20);
        let panes = || {
            let r = e.run(&["list-panes", "-a"]);
            if r.code == 0 { r.out.lines().count() } else { 0 }
        };
        for chunk in chunks {
            let before = if *chunk == "exit\r" { panes() } else { 0 };
            self.send_str(chunk);
            if let Some(rest) = chunk.strip_prefix("echo ") {
                let line = rest.trim_end_matches('\r');
                if let Some(at) = line.find('$') {
                    let label = &line[..at];
                    assert!(self.wait_seen(&format!("{label}[0-9a-z]"), limit), "the output of {label} never showed");
                } else {
                    // no variable in it: the output is a line of its own (the typed line has `echo` in front)
                    assert!(self.wait_screen(&format!("(?m)^{line} *$"), limit), "the output {line} never showed");
                }
            }
            if *chunk == "exit\r" && before > 0 {
                assert!(wait_until(limit, || panes() < before), "the pane did not end");
            }
            if *chunk == "\x1b" {
                self.escape_pause();
            }
        }
    }

    /// The terminal changed size.
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        self.screen.lock().unwrap().set_size(std::num::NonZeroU16::new(rows).unwrap(), std::num::NonZeroU16::new(cols).unwrap());
    }

    /// The text of the screen as the user sees it now (what the client drew, with cursor moves and
    /// partial repaints applied), one line per row.
    pub fn screen_text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    /// Wait until the screen shows text matching `pattern`.
    pub fn wait_screen(&self, pattern: &str, limit: Duration) -> bool {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        wait_until(limit, || re.is_match(&self.screen_text()))
    }

    /// Has the client written these exact bytes (grep -F on the raw stream)?
    pub fn has_bytes(&self, needle: &[u8]) -> bool {
        let b = self.buf.lock().unwrap();
        !needle.is_empty() && b.windows(needle.len()).any(|w| w == needle)
    }

    /// Everything the client has written so far.
    pub fn bytes(&self) -> Vec<u8> {
        self.buf.lock().unwrap().clone()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }

    /// Forget what was written so far, to look only at what comes next.
    pub fn clear(&self) {
        self.buf.lock().unwrap().clear();
    }

    /// Wait until the output matches `pattern` (a regular expression).
    pub fn wait_for(&self, pattern: &str, limit: Duration) -> bool {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        wait_until(limit, || re.is_match(&self.text()))
    }

    /// Wait until `pattern` was written by the client (the byte stream) or is on its screen.
    pub fn wait_seen(&self, pattern: &str, limit: Duration) -> bool {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        wait_until(limit, || re.is_match(&self.text()) || re.is_match(&self.screen_text()))
    }

    /// Has `pattern` been written or shown so far (checked once)?
    pub fn seen(&self, pattern: &str) -> bool {
        let re = Regex::new(&translate_pattern(pattern)).expect("a valid pattern");
        re.is_match(&self.text()) || re.is_match(&self.screen_text())
    }

    /// Wait until the client has ended; its exit status, or None when it is still running.
    pub fn wait_exit(&mut self, limit: Duration) -> Option<u32> {
        let start = Instant::now();
        loop {
            if let Ok(Some(s)) = self.child.try_wait() {
                // what the client wrote last may still be on its way from the terminal to `text()`
                wait_until(Duration::from_secs(10), || self.eof.load(Ordering::SeqCst));
                return Some(s.exit_code());
            }
            if start.elapsed() >= limit {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// End the client.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A path as text.
pub fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The full path of a program on PATH, if it is installed.
pub fn which(name: &str) -> Option<String> {
    let out = Command::new("sh").arg("-c").arg(format!("command -v {name}")).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !text.is_empty() { Some(text) } else { None }
}

/// Is a process whose command line contains `pattern` running (pgrep -f)?
pub fn pgrep(pattern: &str) -> bool {
    Command::new("pgrep").arg("-f").arg(pattern).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// How many processes have a command line that contains `pattern` (pgrep -fc).
pub fn pgrep_count(pattern: &str) -> usize {
    let out = Command::new("pgrep").arg("-fc").arg(pattern).stderr(Stdio::null()).output();
    out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok()).unwrap_or(0)
}

/// End the processes whose command line contains `pattern` (pkill -f).
pub fn pkill(pattern: &str) {
    let _ = Command::new("pkill").arg("-f").arg(pattern).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// The number of lines in a file, 0 when it is not there.
pub fn lines_of(path: &Path) -> usize {
    std::fs::read_to_string(path).map(|t| t.lines().count()).unwrap_or(0)
}

/// A git command in `dir` with a fixed identity and no user or system configuration.
pub fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Standard base64 of some bytes.
pub fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// A git command in `dir` (fixed identity, no configuration files); its standard output without the
/// trailing newline.
pub fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string()
}

static HOLD_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Write a line into a named pipe from a thread (the open blocks until a reader has it).
pub fn feed(path: String, line: String) {
    thread::spawn(move || {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    });
}

/// Keep the foreground of pane `%N` busy with `fixtures/pane_holder.sh`, which runs each line
/// written to `<dir>/fifoN`, so a command can run as a descendant of the pane without the shell
/// going back to its prompt.
pub fn hold(e: &Env, pane: &str) {
    let pipe = p(&e.path(&format!("fifo{pane}")));
    assert!(Command::new("mkfifo").arg(&pipe).status().unwrap().success());
    let holder = format!("{}/tests/fixtures/pane_holder.sh", env!("CARGO_MANIFEST_DIR"));
    e.run(&["send-keys", "-t", &format!("%{pane}"), "-l", &format!("sh {holder} {pipe}")]);
    e.run(&["send-keys", "-t", &format!("%{pane}"), "Enter"]);
    let ready = e.path(&format!("ready{pane}"));
    for _ in 0..50 {
        if ready.exists() {
            break;
        }
        feed(pipe.clone(), format!("echo >{}", ready.display()));
        thread::sleep(Duration::from_millis(200));
    }
    assert!(ready.exists(), "the holder of pane {pane} did not start");
}

/// Run `command` as a descendant of the holder of pane `%N` and wait (20 seconds) for it. The exit
/// status (-1 if it never ended) and what it printed.
pub fn in_pane(e: &Env, pane: &str, command: &str) -> (i32, String) {
    let n = HOLD_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let rc = e.path(&format!("rc{n}"));
    let out = e.path(&format!("o{n}"));
    feed(p(&e.path(&format!("fifo{pane}"))), format!("{{ {command}; }} >{} 2>&1; echo $? >{}", out.display(), rc.display()));
    wait_until(Duration::from_secs(20), || rc.exists() && std::fs::read_to_string(&rc).map(|s| !s.trim().is_empty()).unwrap_or(false));
    let code = std::fs::read_to_string(&rc).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(-1);
    (code, std::fs::read_to_string(&out).unwrap_or_default())
}

/// Is a process whose command line contains `pattern` running with `dir` somewhere in its
/// environment (a plugin started by this test's server has the socket path under `dir`)? Tests run
/// side by side, so a bare `pgrep -f` could meet another test's process.
pub fn pgrep_in(pattern: &str, dir: &Path) -> bool {
    let out = Command::new("pgrep").arg("-f").arg(pattern).output();
    let Ok(out) = out else { return false };
    let marker = p(dir);
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()).any(|pid| {
        std::fs::read(format!("/proc/{pid}/environ")).map(|env| env.windows(marker.len()).any(|w| w == marker.as_bytes())).unwrap_or(false)
    })
}
