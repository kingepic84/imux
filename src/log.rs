use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

struct Logger {
    min: Level,
    file: Option<File>,
}

static LOGGER: Mutex<Logger> = Mutex::new(Logger { min: Level::Warn, file: None });

/// A log file for appending: private (0600) and never through a final symbolic link, since logs
/// hold paths and command lines.
fn open_log(path: &std::path::Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().create(true).append(true).mode(0o600).custom_flags(nix::libc::O_NOFOLLOW).open(path)
}

/// `IMUX_LOG` = debug|info|warn|error (default warn); `IMUX_LOG_FILE` redirects from stderr.
pub fn init_from_env() {
    let mut logger = LOGGER.lock().unwrap_or_else(|e| e.into_inner());
    logger.min = match std::env::var("IMUX_LOG").as_deref() {
        Ok("debug") => Level::Debug,
        Ok("info") => Level::Info,
        Ok("error") => Level::Error,
        _ => Level::Warn,
    };
    if let Ok(path) = std::env::var("IMUX_LOG_FILE") {
        match open_log(std::path::Path::new(&path)) {
            Ok(f) => logger.file = Some(f),
            Err(e) => {
                let line = format_line(Level::Error, &format!("open log file {path}: {e}"));
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
        }
    }
}

/// For the daemon, which has no stderr: log to `path` unless the env vars already say otherwise.
pub fn redirect_to_file(path: &std::path::Path, default_level: Level) {
    let mut logger = LOGGER.lock().unwrap_or_else(|e| e.into_inner());
    if std::env::var_os("IMUX_LOG").is_none() {
        logger.min = default_level;
    }
    if std::env::var_os("IMUX_LOG_FILE").is_none() {
        match open_log(path) {
            Ok(f) => logger.file = Some(f),
            Err(e) => {
                let line = format_line(Level::Error, &format!("open log file {}: {e}", path.display()));
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
        }
    }
}

/// Text of a caught panic payload.
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Log every panic as one JSON error line with its location and a backtrace, then call
/// `cleanup` (the client puts the terminal back in order there). Replaces the default hook, which
/// would print free text over the screen.
pub fn install_panic_hook(cleanup: Option<fn()>) {
    std::panic::set_hook(Box::new(move |info| {
        let place = info.location().map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        let backtrace = std::backtrace::Backtrace::force_capture();
        error(&format!("panic: {}{place}\n{backtrace}", panic_text(info.payload())));
        if let Some(cleanup) = cleanup {
            cleanup();
        }
    }));
}

pub fn debug(msg: &str) {
    write(Level::Debug, msg);
}

pub fn info(msg: &str) {
    write(Level::Info, msg);
}

pub fn warn(msg: &str) {
    write(Level::Warn, msg);
}

pub fn error(msg: &str) {
    write(Level::Error, msg);
}

fn write(level: Level, msg: &str) {
    let mut logger = LOGGER.lock().unwrap_or_else(|e| e.into_inner());
    if level < logger.min {
        return;
    }
    let line = format_line(level, msg);
    match logger.file.as_mut() {
        Some(f) => {
            let _ = f.write_all(line.as_bytes());
        }
        None => {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
    }
}

fn format_line(level: Level, msg: &str) -> String {
    let mut out = String::with_capacity(msg.len() + 64);
    out.push_str("{\"time\":\"");
    out.push_str(&rfc3339_now());
    out.push_str("\",\"level\":\"");
    out.push_str(level.name());
    out.push_str("\",\"msg\":\"");
    escape_into(&mut out, msg);
    out.push_str("\"}\n");
    out
}

fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

fn rfc3339_now() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (y, mo, da) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{da:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_516), (2026, 3, 4));
    }

    #[test]
    fn line_is_escaped_json() {
        let line = format_line(Level::Error, "a\"b\\c\nd");
        assert!(line.contains("\"level\":\"error\""));
        assert!(line.contains("\"msg\":\"a\\\"b\\\\c\\nd\""));
        assert!(line.ends_with("}\n"));
    }
}
