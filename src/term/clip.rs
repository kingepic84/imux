//! OSC 52 requests from a program in a pane. Whatever passes here is forwarded to the user's
//! terminal, so only a well-formed write may: the selection list is fixed letters and digits, the
//! payload is plain base64 of bounded size. The read form (`?`) and anything odd is dropped.

/// Largest payload accepted, in base64 characters (about 768 KiB of text).
pub const MAX_DATA: usize = 1 << 20;
const MAX_TARGET: usize = 8;

/// `(target, data)` of a valid write request, with an empty target meaning the clipboard (`c`).
pub fn request(target: &str, data: &str) -> Option<(String, String)> {
    if target.len() > MAX_TARGET || !target.bytes().all(|b| b"cpqs01234567".contains(&b)) {
        return None;
    }
    if data.is_empty() || data.len() > MAX_DATA {
        return None;
    }
    let body = data.trim_end_matches('=');
    if data.len() - body.len() > 2 || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/') {
        return None;
    }
    let target = if target.is_empty() { "c" } else { target };
    Some((target.to_string(), data.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(target: &str, data: &str) -> Option<(String, String)> {
        Some((target.to_string(), data.to_string()))
    }

    #[test]
    fn a_plain_write_passes_and_an_empty_target_means_the_clipboard() {
        assert_eq!(request("c", "aGk="), ok("c", "aGk="));
        assert_eq!(request("", "aGk="), ok("c", "aGk="));
        assert_eq!(request("pq", "aGk"), ok("pq", "aGk"));
        assert_eq!(request("s0", "a+b/c2Q=="), ok("s0", "a+b/c2Q=="));
    }

    #[test]
    fn the_read_form_and_empty_data_are_dropped() {
        assert_eq!(request("c", "?"), None);
        assert_eq!(request("", "?"), None);
        assert_eq!(request("c", ""), None);
        assert_eq!(request("c?", "aGk="), None);
    }

    #[test]
    fn anything_that_is_not_base64_is_dropped() {
        for data in ["a!b", "a b", "aGk=\u{7}", "aGk=\u{1b}]0;x", "a=b", "=aGk", "aGk===", "\u{e9}", "aGk=\n", "aGk=;x"] {
            assert_eq!(request("c", data), None, "{data:?}");
        }
    }

    #[test]
    fn a_target_other_than_the_selection_letters_is_dropped() {
        for target in ["x", "C", "8", "c;c", "cc\u{1b}", "ccccccccc", " "] {
            assert_eq!(request(target, "aGk="), None, "{target:?}");
        }
    }

    #[test]
    fn the_payload_is_bounded() {
        let big = "A".repeat(MAX_DATA);
        assert!(request("c", &big).is_some());
        assert_eq!(request("c", &format!("{big}A")), None);
    }
}
