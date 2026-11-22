//! OSC 7501, the Program Status Protocol (spec rev 0.4): a program tells its terminal whether it is
//! idle, working, done, blocked on the user or failed. `OSC 7501 ; ?` asks whether the terminal
//! understands it. The parsing here is pure; the interceptor has already bounded the length.
//!
//! A report that breaks a limit, has bad base64, a control character in its text, an unknown
//! state or a malformed id is discarded whole. Unknown keys and malformed pairs are skipped.

use crate::image::base64;

/// What a supporting terminal answers to `OSC 7501 ; ?`.
pub const QUERY_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

/// A whole sequence is at most 4096 bytes; the body is what is between `7501;` and the terminator.
const MAX_BODY: usize = 4088;
const MAX_KEY: usize = 16;
const MAX_APP: usize = 32;
const MAX_ID: usize = 128;
const MAX_SEGMENT: usize = 32;
const MAX_DEPTH: usize = 8;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG: usize = 2048;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_TITLE: usize = 192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub state: State,
    /// Only with `Blocked`.
    pub kind: Option<Kind>,
    /// 0 to 100, only with `Working` or `Blocked`; none means indeterminate.
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
    /// Record id; the root record when absent.
    pub id: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Query,
    Report(Report),
}

fn value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b)
}

fn name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"_.+-".contains(&b)
}

fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID
        && id.split('/').count() <= MAX_DEPTH
        && id.split('/').all(|s| !s.is_empty() && s.len() <= MAX_SEGMENT && s.bytes().all(name_byte))
}

/// Base64 UTF-8 text on one line.
fn text(encoded: &str, max_encoded: usize, max: usize) -> Result<String, String> {
    if encoded.len() > max_encoded {
        return Err(format!("text is {} bytes encoded, max {max_encoded}", encoded.len()));
    }
    let bytes = base64::decode(encoded.as_bytes()).ok_or("text is not base64")?;
    if bytes.len() > max {
        return Err(format!("text is {} bytes, max {max}", bytes.len()));
    }
    let s = String::from_utf8(bytes).map_err(|_| "text is not UTF-8")?;
    if s.chars().any(crate::sanitize::is_unsafe) {
        return Err("text has a control or bidirectional formatting character".to_string());
    }
    Ok(s)
}

/// `body` is what followed `OSC 7501 ;`.
pub fn parse(body: &[u8]) -> Result<Parsed, String> {
    if body.len() > MAX_BODY {
        return Err(format!("report is {} bytes, max {MAX_BODY}", body.len()));
    }
    if body.first() == Some(&b'?') {
        return Ok(Parsed::Query);
    }
    let body = std::str::from_utf8(body).map_err(|_| "report is not text")?;
    let (mut state, mut kind, mut progress, mut app) = (None, None, None, None);
    let (mut title, mut msg, mut id) = (None, None, None);
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) || !value.bytes().all(value_byte) {
            continue;
        }
        if key.len() > MAX_KEY {
            return Err(format!("key {key} is over {MAX_KEY} bytes"));
        }
        match key {
            "state" => state = Some(value),
            "kind" => kind = Some(value),
            "progress" => progress = Some(value),
            "app" => app = Some(value),
            "title" => title = Some(value),
            "msg" => msg = Some(value),
            "id" => id = Some(value),
            _ => {}
        }
    }
    let state = match state {
        Some("idle") => State::Idle,
        Some("working") => State::Working,
        Some("done") => State::Done,
        Some("blocked") => State::Blocked,
        Some("error") => State::Error,
        Some("clear") => State::Clear,
        Some(other) => return Err(format!("unknown state {other}")),
        None => return Err("no state".to_string()),
    };
    if let Some(id) = id
        && !valid_id(id)
    {
        return Err(format!("bad id {id:?}"));
    }
    let kind = match (state, kind) {
        (State::Blocked, Some("permission")) => Some(Kind::Permission),
        (State::Blocked, Some("question")) => Some(Kind::Question),
        (State::Blocked, Some("auth")) => Some(Kind::Auth),
        _ => None,
    };
    let progress = match state {
        State::Working | State::Blocked => progress.and_then(|p| p.parse::<u8>().ok()).filter(|&p| p <= 100),
        _ => None,
    };
    if app.is_some_and(|a| a.len() > MAX_APP) {
        return Err(format!("app is over {MAX_APP} bytes"));
    }
    let app = app.filter(|a| !a.is_empty() && a.bytes().all(name_byte)).map(str::to_string);
    let title = title.map(|t| text(t, MAX_TITLE_ENCODED, MAX_TITLE)).transpose()?;
    let msg = msg.map(|m| text(m, MAX_MSG_ENCODED, MAX_MSG)).transpose()?;
    Ok(Parsed::Report(Report { state, kind, progress, app, title, msg, id: id.map(str::to_string) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(body: &str) -> Report {
        match parse(body.as_bytes()) {
            Ok(Parsed::Report(r)) => r,
            other => panic!("{body}: {other:?}"),
        }
    }

    #[test]
    fn the_query_is_recognized() {
        assert_eq!(parse(b"?"), Ok(Parsed::Query));
        assert_eq!(parse(b"?:states=idle"), Ok(Parsed::Query));
    }

    #[test]
    fn what_claude_code_sends_parses() {
        let r = report("state=idle:app=claude-code");
        assert_eq!((r.state, r.app.as_deref()), (State::Idle, Some("claude-code")));
        let r = report("state=blocked:app=claude-code:kind=permission:msg=YXBwcm92ZSBCYXNoOiB0b3VjaCAvdG1wL2hvb2stZXhwL2RlbnlBLnR4dA==");
        assert_eq!((r.state, r.kind), (State::Blocked, Some(Kind::Permission)));
        assert_eq!(r.msg.as_deref(), Some("approve Bash: touch /tmp/hook-exp/denyA.txt"));
        assert_eq!(report("state=clear").state, State::Clear);
        assert_eq!(report("state=done:app=claude-code").state, State::Done);
    }

    #[test]
    fn padding_is_optional_and_text_is_utf8() {
        assert_eq!(report("state=working:msg=SW5zdGFsbGluZw").msg.as_deref(), Some("Installing"));
        assert_eq!(report("state=working:msg=SW5zdGFsbGluZw==").msg.as_deref(), Some("Installing"));
        assert_eq!(report("state=done:title=w6k=").title.as_deref(), Some("é"));
    }

    #[test]
    fn the_last_duplicate_wins_and_unknown_keys_and_bad_pairs_are_skipped() {
        let r = report("state=idle:state=working:future=x:oops:=1:Bad=2:app=a b:progress=40");
        assert_eq!(r.state, State::Working);
        assert_eq!(r.progress, Some(40));
        assert_eq!(r.app, None);
    }

    #[test]
    fn kind_and_progress_only_count_in_their_states() {
        let r = report("state=idle:kind=permission:progress=50");
        assert_eq!((r.kind, r.progress), (None, None));
        let r = report("state=blocked:kind=nonsense:progress=101");
        assert_eq!((r.kind, r.progress), (None, None));
        let r = report("state=working:progress=100");
        assert_eq!(r.progress, Some(100));
    }

    #[test]
    fn a_bad_report_is_discarded_whole() {
        for bad in [
            "app=x",
            "state=sleeping",
            "state=idle:id=a//b",
            "state=idle:id=/a",
            "state=working:msg=a-b_",
            "state=working:msg=AAE=",
            "state=working:title=/w==",
            "state=idle:app=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "state=idle:abcdefghijklmnopq=1",
        ] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(parse(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn limits_are_enforced() {
        let long = "QQ".repeat(1367);
        assert!(parse(format!("state=idle:msg={long}").as_bytes()).is_err());
        let fits = base64::encode("x".repeat(2048).as_bytes());
        assert_eq!(report(&format!("state=idle:msg={fits}")).msg.map(|m| m.len()), Some(2048));
        let over = base64::encode("x".repeat(2049).as_bytes());
        assert!(parse(format!("state=idle:msg={over}").as_bytes()).is_err());
        assert!(parse(&vec![b'a'; MAX_BODY + 1]).is_err());
        let deep = ["a"; 9].join("/");
        assert!(parse(format!("state=idle:id={deep}").as_bytes()).is_err());
        let ok = ["a"; 8].join("/");
        assert_eq!(report(&format!("state=idle:id={ok}")).id.as_deref(), Some(ok.as_str()));
    }
}
