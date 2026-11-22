//! `imux events --wait EVENT [--match key=value ...]`: reading one line of the event stream. A
//! line is one flat JSON object whose values are strings or numbers (`server/events.rs` writes
//! it), so a small reader is enough; a field is found by parsing the line, never by searching it,
//! so text inside a value cannot pass for a field.

/// The fields of one event line, in order, values as text (numbers as written). A line that is
/// not an object of that shape gives the fields read before the problem.
pub fn fields(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = line.trim().chars().peekable();
    if chars.next() != Some('{') {
        return out;
    }
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        if chars.peek() != Some(&'"') {
            return out;
        }
        chars.next();
        let Some(key) = string_body(&mut chars) else { return out };
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.next() != Some(':') {
            return out;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let value = if chars.peek() == Some(&'"') {
            chars.next();
            match string_body(&mut chars) {
                Some(v) => v,
                None => return out,
            }
        } else {
            let mut bare = String::new();
            while let Some(c) = chars.peek().copied().filter(|c| *c != ',' && *c != '}') {
                bare.push(c);
                chars.next();
            }
            if bare.trim().is_empty() {
                return out;
            }
            bare.trim().to_string()
        };
        out.push((key, value));
    }
}

/// The rest of a string whose opening quote was read, escapes resolved; `None` if it never ends.
fn string_body(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                    out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('\u{fffd}'));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
}

/// Whether `line` is an event called `event` whose fields hold every `key=value` of `wanted`.
pub fn matches(line: &str, event: &str, wanted: &[(String, String)]) -> bool {
    let found = fields(line);
    let get = |key: &str| found.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    get("event") == Some(event) && wanted.iter().all(|(k, v)| get(k) == Some(v.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_line_is_read_into_its_fields() {
        let line = r#"{"event":"agent-state-changed","seq":12,"time":1731160000000,"pane":"%3","state":"idle"}"#;
        assert_eq!(fields(line), pairs(&[("event", "agent-state-changed"), ("seq", "12"), ("time", "1731160000000"), ("pane", "%3"), ("state", "idle")]));
    }

    #[test]
    fn escapes_are_resolved_and_a_value_cannot_pass_for_a_field() {
        let line = r#"{"event":"x","message":"say \"state\":\"idle\" \\ \n A","state":"working"}"#;
        let found = fields(line);
        assert_eq!(found[1], ("message".to_string(), "say \"state\":\"idle\" \\ \n A".to_string()));
        assert!(matches(line, "x", &pairs(&[("state", "working")])));
        assert!(!matches(line, "x", &pairs(&[("state", "idle")])));
    }

    #[test]
    fn a_match_needs_the_event_and_every_field() {
        let line = r#"{"event":"pane-died","seq":4,"pane":"%2"}"#;
        assert!(matches(line, "pane-died", &[]));
        assert!(matches(line, "pane-died", &pairs(&[("pane", "%2"), ("seq", "4")])));
        assert!(!matches(line, "pane-died", &pairs(&[("pane", "%3")])));
        assert!(!matches(line, "window-renamed", &[]));
        assert!(!matches(line, "pane-died", &pairs(&[("nosuch", "")])));
    }

    #[test]
    fn a_line_that_is_not_an_object_gives_nothing_or_what_came_before() {
        assert!(fields("").is_empty() && fields("[1]").is_empty() && fields("hello").is_empty());
        assert_eq!(fields(r#"{"a":"b","c":"#), pairs(&[("a", "b")]));
        assert_eq!(fields(r#"{"a":"unterminated"#), vec![]);
    }
}
