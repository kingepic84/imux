//! Fuzzy matching for the pickers, the repository wizard and the agent panel: a query matches a
//! text when its characters appear in the text in that order, and a match scores higher the more
//! it looks like what the user meant (a prefix, the start of words, characters side by side).
//!
//! A query of several words (split at spaces) must match every word; the scores add up. A query
//! with a capital letter is case-sensitive, any other is not (smart case).

/// Longest query and text looked at; the rest is ignored so a hostile list costs nothing.
const MAX_QUERY: usize = 64;
const MAX_TEXT: usize = 512;

const MATCH: i32 = 16;
const CONSECUTIVE: i32 = 24;
const BOUNDARY: i32 = 20;
const PREFIX: i32 = 12;
const GAP_OPEN: i32 = 3;
const GAP_EXTEND: i32 = 1;
const LEADING_MAX: i32 = 8;

fn same(a: char, b: char, exact: bool) -> bool {
    a == b || (!exact && a.to_lowercase().eq(b.to_lowercase()))
}

/// Bonus for a match at `i`: the start of the text or of a word.
fn bonus(text: &[char], i: usize) -> i32 {
    let Some(prev) = i.checked_sub(1).map(|p| text[p]) else { return BOUNDARY + PREFIX };
    let here = text[i];
    if !prev.is_alphanumeric() || (prev.is_lowercase() && here.is_uppercase()) { BOUNDARY } else { 0 }
}

/// The score of one word of a query against `text`, `None` when it does not match.
fn score_word(word: &[char], text: &[char], exact: bool) -> Option<i32> {
    const NONE: i32 = i32::MIN / 2;
    if word.is_empty() {
        return Some(0);
    }
    let n = text.len();
    // `prev[i]`: best score with the previous query character matched at text index `i`.
    let mut prev: Vec<i32> = vec![NONE; n];
    for (j, &q) in word.iter().enumerate() {
        let mut cur = vec![NONE; n];
        // Best score of a previous match at least two before `i` with the gap between them paid.
        let mut gapped = NONE;
        for i in 0..n {
            if gapped > NONE {
                gapped = (gapped - GAP_EXTEND).max(NONE);
            }
            if i >= 2 && prev[i - 2] > NONE {
                gapped = gapped.max(prev[i - 2] - GAP_OPEN);
            }
            if !same(q, text[i], exact) {
                continue;
            }
            let before = if j == 0 {
                -(i as i32).min(LEADING_MAX)
            } else {
                let next_to = if i >= 1 && prev[i - 1] > NONE { prev[i - 1] + CONSECUTIVE } else { NONE };
                next_to.max(gapped)
            };
            if before > NONE {
                cur[i] = before + MATCH + bonus(text, i);
            }
        }
        prev = cur;
    }
    prev.into_iter().filter(|&s| s > NONE).max()
}

/// The score of `query` against `text`: `None` when some word of the query does not match.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let exact = query.chars().any(char::is_uppercase);
    let text: Vec<char> = text.chars().take(MAX_TEXT).collect();
    let mut total = 0;
    let mut any = false;
    for word in query.split_whitespace() {
        let word: Vec<char> = word.chars().take(MAX_QUERY).collect();
        total += score_word(&word, &text, exact)?;
        any = true;
    }
    any.then_some(total).or(Some(0))
}

/// The items `query` matches, best first (ties keep their order), as `(index, score)`. An empty
/// query keeps every item in its order.
pub fn filter<S: AsRef<str>>(items: &[S], query: &str) -> Vec<(usize, i32)> {
    let mut found: Vec<(usize, i32)> = items.iter().enumerate().filter_map(|(i, t)| Some((i, score(query, t.as_ref())?))).collect();
    found.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str, t: &str) -> Option<i32> {
        score(q, t)
    }

    #[test]
    fn a_query_matches_when_its_characters_appear_in_order() {
        assert!(s("abc", "a-b-c").is_some());
        assert!(s("abc", "cba").is_none());
        assert!(s("abc", "ab").is_none());
        assert!(s("x", "").is_none());
    }

    #[test]
    fn an_empty_query_matches_everything_with_no_score() {
        assert_eq!(s("", "anything"), Some(0));
        assert_eq!(s("   ", "anything"), Some(0));
        assert_eq!(filter(&["b", "a", "c"], ""), vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn smart_case_ignores_case_until_the_query_has_a_capital() {
        assert!(s("edit", "Editor").is_some());
        assert!(s("EDIT", "editor").is_none());
        assert!(s("Edit", "Editor").is_some());
        assert!(s("Edit", "editor").is_none());
    }

    #[test]
    fn a_prefix_beats_the_middle_of_the_text() {
        assert!(s("ed", "editor") > s("ed", "unedited"));
    }

    #[test]
    fn the_start_of_a_word_beats_scattered_characters() {
        assert!(s("fb", "foo-bar") > s("fb", "xfxxbx"));
        assert!(s("fb", "fooBar") > s("fb", "xfxxbx"));
        assert!(s("fb", "foo bar") > s("fb", "fobar"), "a word start beats the middle of a word");
    }

    #[test]
    fn characters_side_by_side_beat_characters_far_apart() {
        assert!(s("abc", "xabcx") > s("abc", "xaxbxcx"));
        assert!(s("abc", "xaxbxcx") > s("abc", "xaxxxxxbxxxxxcx"), "and a short gap beats a long one");
    }

    #[test]
    fn a_match_earlier_in_the_text_beats_one_later() {
        assert!(s("x", "xaaaa") > s("x", "aaaax"));
    }

    #[test]
    fn every_word_of_a_query_must_match() {
        assert!(s("dev api", "api server dev").is_some());
        assert!(s("dev zzz", "api server dev").is_none());
        assert!(s("dev  api", "api dev").is_some(), "extra spaces do not matter");
    }

    #[test]
    fn filter_orders_by_score_then_by_position() {
        let items = ["xxfoo", "foo", "a foo", "foo bar"];
        let found: Vec<usize> = filter(&items, "foo").into_iter().map(|(i, _)| i).collect();
        assert_eq!(found[0], 1, "the exact start first");
        assert!(found.contains(&0) && found.contains(&2) && found.contains(&3));
        let same_score: Vec<usize> = filter(&["ab", "ab", "ab"], "ab").into_iter().map(|(i, _)| i).collect();
        assert_eq!(same_score, [0, 1, 2], "ties keep the original order");
        assert!(filter(&["abc", "xyz"], "q").is_empty());
    }

    #[test]
    fn long_inputs_are_cut_not_slow() {
        let text = "a".repeat(100_000);
        let query = "a".repeat(1000);
        assert!(s(&query, &text).is_some());
        let none = "b".repeat(1000);
        assert!(s(&none, &text).is_none());
    }

    #[test]
    fn non_ascii_text_works() {
        assert!(s("cafe", "Café").is_none(), "no accent folding");
        assert!(s("caf", "Café").is_some());
        assert!(s("é", "Café").is_some());
    }
}
