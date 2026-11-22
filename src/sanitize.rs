//! One answer to "may this character be shown or passed on as text from a program?".
//!
//! Text that a program in a pane controls (agent messages, process names, command output) ends
//! up in the status bar, the pickers, hook variables and the event stream. Control characters
//! (C0, DEL and the C1 range, which some terminals read as introducers when they come from UTF-8)
//! are never wanted there, and neither are the bidirectional controls, which reorder the text
//! around them and so can make a window name or a message read as something else.

/// Bidirectional formatting characters: marks, embeddings, overrides and isolates.
pub fn is_bidi(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A character that must not be passed on in program-supplied text.
pub fn is_unsafe(c: char) -> bool {
    c.is_control() || is_bidi(c)
}

/// `text` without its unsafe characters.
pub fn clean(text: &str) -> String {
    text.chars().filter(|&c| !is_unsafe(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_c1_and_bidi_are_unsafe_and_ordinary_text_is_not() {
        for c in ['\0', '\x07', '\x1b', '\x7f', '\u{80}', '\u{9b}', '\u{9d}', '\u{9f}', '\u{202e}', '\u{2066}', '\u{2069}', '\u{200f}', '\u{061c}'] {
            assert!(is_unsafe(c), "{c:?}");
        }
        for c in ['a', ' ', 'é', '日', '😀', '\u{200d}', '\u{fe0f}', '\u{a0}'] {
            assert!(!is_unsafe(c), "{c:?}");
        }
    }

    #[test]
    fn clean_keeps_the_rest_in_order() {
        assert_eq!(clean("ab\u{202e}cd\x1b[31m\u{9b}e"), "abcd[31me");
    }

    #[test]
    fn every_code_point_is_decided_without_panicking() {
        let unsafe_count = (0..=0x10FFFFu32).filter_map(char::from_u32).filter(|&c| is_unsafe(c)).count();
        assert!(unsafe_count > 60 && unsafe_count < 100, "{unsafe_count}");
    }
}
