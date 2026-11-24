//! `imux api schema`: the fields of the `--json` output and the error codes, as JSON, so a tool or
//! a skill can check what it reads against the imux it runs with. The field names are those of the
//! event stream where the thing is the same (`session`, `window`, `pane`, `state`, `message`);
//! ids are strings (`$1`, `@2`, `%3`); a value that is not there is a missing field.

use crate::errcode::{CODES, json_string};

/// (name, type, what it is, whether it is always there) of an agent record, as `agent get --json`
/// prints it and `agent list --json` prints each element.
pub const AGENT_FIELDS: &[(&str, &str, &str, bool)] = &[
    ("session", "string", "id of the session, $N", true),
    ("session_name", "string", "name of the session", true),
    ("window", "string", "id of the window, @N", true),
    ("window_name", "string", "name of the window", true),
    ("pane", "string", "id of the pane, %N", true),
    ("state", "string", "idle, working, done, blocked or error", true),
    ("source", "string", "who made the last change: osc (OSC 7501) or the --source of a report", true),
    ("agent", "string", "the program's name as it reported it", false),
    ("name", "string", "the name given with agent name, usable as agent:NAME", false),
    ("agent_session", "string", "the name of the program's own session (Claude Code's /rename), given by its hook; text from the program, data", false),
    ("previous_agent_session", "string", "the session name before the last change; never the same as agent_session", false),
    ("kind", "string", "with blocked: permission, question or auth", false),
    ("progress", "number", "0 to 100, with working or blocked", false),
    ("message", "string", "the short note shown with the state", false),
    ("state_since", "number", "seconds since the state last changed", true),
    ("seen", "boolean", "a client has looked at the pane since the state changed", true),
    ("seqs", "object", "the highest accepted --seq of each source", true),
    ("resume", "array", "the reported command that brings the program back, one string per word", false),
    ("pid", "number", "process id of the pane's shell", true),
    ("cwd", "string", "directory of the pane's shell", false),
];

/// The field that ends the result of a command that changes something.
const SEQ: &str = "number, the last event before the change: imux events --wait ... --since seq sees what it caused";

/// What the commands print with `--json`: (command, fields).
pub const RESULTS: &[(&str, &[(&str, &str)])] = &[
    ("agent wait", &[("pane", "string"), ("state", "string")]),
    ("agent prompt", &[("pane", "string"), ("sent", "boolean"), ("state", "string, only with --wait"), ("seq", SEQ)]),
    ("agent name", &[("pane", "string"), ("name", "string or null"), ("seq", SEQ)]),
    ("agent session", &[("pane", "string"), ("session", "string or null"), ("seq", SEQ)]),
    ("pane run", &[("pane", "string"), ("sent", "boolean"), ("seq", SEQ)]),
    ("pane wait-output", &[("pane", "string"), ("line", "string, the first line that matched"), ("text", "string, what was searched")]),
    ("agent start", &[("pane", "string"), ("name", "string, the name the agent got"), ("state", "string, idle or done: the first report"), ("seq", SEQ)]),
    ("agent read", &[("pane", "string"), ("lines", "number, how many were asked for"), ("text", "string")]),
];

/// The schema document, one line of JSON.
pub fn document() -> String {
    let errors: Vec<String> = CODES.iter().map(|(code, about)| format!("{{\"code\":{},\"about\":{}}}", json_string(code), json_string(about))).collect();
    let fields: Vec<String> = AGENT_FIELDS
        .iter()
        .map(|(name, kind, about, always)| format!("{{\"name\":{},\"type\":{},\"about\":{},\"always\":{always}}}", json_string(name), json_string(kind), json_string(about)))
        .collect();
    let results: Vec<String> = RESULTS
        .iter()
        .map(|(command, fields)| {
            let list: Vec<String> = fields.iter().map(|(name, kind)| format!("{{\"name\":{},\"type\":{}}}", json_string(name), json_string(kind))).collect();
            format!("{}:[{}]", json_string(command), list.join(","))
        })
        .collect();
    format!(
        "{{\"api\":{},\"error\":{{\"stream\":\"stderr\",\"exit\":1,\"usage_exit\":2,\"codes\":[{}]}},\"agent\":[{}],\"results\":{{{}}}}}\n",
        crate::proto::API_VERSION,
        errors.join(","),
        fields.join(","),
        results.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_is_one_line_with_every_code_and_field() {
        let doc = document();
        assert_eq!(doc.matches('\n').count(), 1);
        for (code, _) in CODES {
            assert!(doc.contains(&format!("\"code\":\"{code}\"")), "{code}");
        }
        for (name, ..) in AGENT_FIELDS {
            assert!(doc.contains(&format!("\"name\":\"{name}\"")), "{name}");
        }
        assert!(doc.starts_with(&format!("{{\"api\":{}", crate::proto::API_VERSION)));
    }

    #[test]
    fn field_names_are_unique_snake_case() {
        for (i, (name, ..)) in AGENT_FIELDS.iter().enumerate() {
            assert!(name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{name}");
            assert!(AGENT_FIELDS.iter().skip(i + 1).all(|(other, ..)| other != name), "{name} twice");
        }
    }
}
