//! Error codes of the machine-readable output (`--json`): a closed list of snake_case words.
//!
//! The server writes an error as `code: message` when it knows which kind it is; the client reads
//! the word with `split`, prints only the message in plain mode (so the text people read does not
//! change) and the pair as `{"error":{"code":..,"message":..}}` with `--json`. Only a word in
//! `CODES` is a code: a message that merely starts with some other word and a colon (`usage: ...`,
//! `send-keys: ...`) is left alone.

/// Every code, in the order the schema lists them.
pub const CODES: &[(&str, &str)] = &[
    ("not_found", "the pane, agent or name does not exist"),
    ("ambiguous_target", "the target names more than one thing"),
    ("invalid_argument", "a flag or value is wrong"),
    ("permission_denied", "the caller may not do this; the message names the option that allows it"),
    ("limit_reached", "a count, depth, size or time limit stopped it; the message names the option"),
    ("timeout", "the wait ended before the condition"),
    ("agent_blocked", "the agent waits for an answer to a permission prompt"),
    ("agent_not_running", "the pane has no agent record"),
    ("prompt_stalled", "the agent did not start working after the text was typed"),
    ("invalid_regex", "the pattern is not a valid regular expression"),
    ("not_ready", "the pane is not in a state to take this"),
    ("server_unavailable", "no server runs, or it is stopping"),
    ("protocol_mismatch", "the server speaks another protocol version than this client"),
];

fn known(word: &str) -> Option<&'static str> {
    CODES.iter().map(|(code, _)| *code).find(|code| *code == word)
}

/// `message` with its code in front, as the server sends it.
pub fn tag(code: &str, message: &str) -> String {
    debug_assert!(known(code).is_some(), "{code} is not an error code");
    format!("{code}: {message}")
}

/// The code at the start of `text`, if it has one, and the message after it.
pub fn split(text: &str) -> (Option<&'static str>, &str) {
    match text.split_once(": ") {
        Some((word, rest)) => match known(word) {
            Some(code) => (Some(code), rest),
            None => (None, text),
        },
        None => (None, text),
    }
}

/// The code of a client-side failure that has none from the server: no server running, or a
/// server that does not speak this protocol.
pub fn of_client_failure(message: &str) -> &'static str {
    if message.contains("no server running") || message.contains("stopping") {
        "server_unavailable"
    } else if message.contains("protocol") || message.contains("older imux") {
        "protocol_mismatch"
    } else {
        "invalid_argument"
    }
}

/// `text` as a JSON string, quotes included.
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if crate::sanitize::is_unsafe(c) || matches!(c, '\u{2028}' | '\u{2029}') => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The error object `--json` prints on the standard error: from a server answer with or without a
/// code (the client's own failures get theirs from `of_client_failure`).
pub fn json_error(text: &str) -> String {
    let (code, message) = split(text);
    let code = code.unwrap_or_else(|| of_client_failure(text));
    format!("{{\"error\":{{\"code\":{},\"message\":{}}}}}", json_string(code), json_string(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tagged_message_splits_and_a_plain_one_does_not() {
        assert_eq!(split(&tag("not_found", "no pane 3")), (Some("not_found"), "no pane 3"));
        assert_eq!(split("usage: imux agent get pane"), (None, "usage: imux agent get pane"));
        assert_eq!(split("send-keys: bad flag"), (None, "send-keys: bad flag"));
        assert_eq!(split("not_found:no space"), (None, "not_found:no space"));
        assert_eq!(split(""), (None, ""));
    }

    #[test]
    fn every_code_is_snake_case_and_listed_once() {
        for (i, (code, about)) in CODES.iter().enumerate() {
            assert!(code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{code}");
            assert!(!about.is_empty());
            assert!(CODES.iter().skip(i + 1).all(|(other, _)| other != code), "{code} twice");
        }
    }

    #[test]
    fn the_json_error_has_a_code_and_the_message_with_hostile_text_escaped() {
        let line = json_error(&tag("permission_denied", "say \"no\"\n\u{1b}[31m"));
        assert_eq!(line, "{\"error\":{\"code\":\"permission_denied\",\"message\":\"say \\\"no\\\"\\n\\u001b[31m\"}}");
    }

    #[test]
    fn a_failure_with_no_code_gets_one_from_what_it_says() {
        assert!(json_error("no server running").contains("\"server_unavailable\""));
        assert!(json_error("the server speaks protocol 1").contains("\"protocol_mismatch\""));
        assert!(json_error("something else").contains("\"invalid_argument\""));
    }
}
