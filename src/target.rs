//! Targets of commands (`-t`): which session, window and pane a command works on.
//!
//! A target is written `session:window.pane`; every part may be left out, and a pane may also be
//! written alone as `%12` (its id). The parts are text here; the server looks them up. A part is
//! one of: a number (an index), a name (exact, else a unique prefix, else a pattern with `*` and
//! `?`), an id (`$3` session, `@5` window, `%12` pane), or a position (`!` the last one, `+` and
//! `-` the next and the previous, `+2`, `-1`, `^` the first, `$` the last, `{next}`, `{previous}`,
//! `{last}`, `{start}`, `{end}`).

/// What a command expects its target to be, which decides how a bare word is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Session,
    Window,
    Pane,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Target {
    pub session: Option<String>,
    pub window: Option<String>,
    pub pane: Option<String>,
}

/// One part of a target, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// An index or position number as typed (`2`).
    Number(usize),
    /// `+`, `-`, `+3`, `-2`, `{next}`, `{previous}`: relative to the current one.
    Offset(isize),
    /// `!` or `{last}`: the one used before.
    Last,
    /// `^` or `{start}`: the first.
    Start,
    /// `$` or `{end}`: the last.
    End,
    /// `$3`, `@5` or `%12` with the number.
    Id(char, usize),
    Name(String),
    /// `=name`: the name must match exactly, no prefix and no pattern.
    Exact(String),
    /// `{up-of}`, `{down-of}`, `{left-of}`, `{right-of}`: the pane beside the current one.
    Beside(Dir),
    /// `{top}`, `{bottom}`, `{left}`, `{right}` and the four corners such as `{top-left}`: the
    /// pane at that edge or corner of the window.
    Edge(Option<Vert>, Option<Horiz>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vert {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Horiz {
    Left,
    Right,
}

const MAX_TARGET: usize = 256;

fn part(word: &str) -> Option<String> {
    (!word.is_empty()).then(|| word.to_string())
}

/// Split `spec` into session, window and pane for a command that wants a `kind`.
pub fn parse(spec: &str, kind: Kind) -> Result<Target, String> {
    if spec.len() > MAX_TARGET || spec.chars().any(char::is_control) {
        return Err(format!("bad target {spec:?}"));
    }
    if spec.is_empty() {
        return Ok(Target::default());
    }
    let is_pane_id = |s: &str| s.strip_prefix('%').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    Ok(match kind {
        Kind::Session => Target { session: part(spec.strip_suffix(':').unwrap_or(spec)), ..Target::default() },
        Kind::Window => match spec.split_once(':') {
            Some((s, w)) => Target { session: part(s), window: part(w), pane: None },
            None => Target { window: part(spec), ..Target::default() },
        },
        Kind::Pane if is_pane_id(spec) => Target { pane: part(spec), ..Target::default() },
        Kind::Pane => {
            let (session, rest) = match spec.split_once(':') {
                Some((s, rest)) => (part(s), rest),
                None => (None, spec),
            };
            match rest.rsplit_once('.') {
                Some((w, p)) => Target { session, window: part(w), pane: part(p) },
                None if session.is_some() || spec.contains(':') => Target { session, window: part(rest), pane: None },
                None => Target { pane: part(rest), ..Target::default() },
            }
        }
    })
}

/// Read one part of a target.
pub fn reference(word: &str) -> Part {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match word {
        "!" | "{last}" => return Part::Last,
        "^" | "{start}" => return Part::Start,
        "$" | "{end}" => return Part::End,
        "+" | "{next}" => return Part::Offset(1),
        "-" | "{previous}" => return Part::Offset(-1),
        "{up-of}" => return Part::Beside(Dir::Up),
        "{down-of}" => return Part::Beside(Dir::Down),
        "{left-of}" => return Part::Beside(Dir::Left),
        "{right-of}" => return Part::Beside(Dir::Right),
        "{top}" => return Part::Edge(Some(Vert::Top), None),
        "{bottom}" => return Part::Edge(Some(Vert::Bottom), None),
        "{left}" => return Part::Edge(None, Some(Horiz::Left)),
        "{right}" => return Part::Edge(None, Some(Horiz::Right)),
        "{top-left}" => return Part::Edge(Some(Vert::Top), Some(Horiz::Left)),
        "{top-right}" => return Part::Edge(Some(Vert::Top), Some(Horiz::Right)),
        "{bottom-left}" => return Part::Edge(Some(Vert::Bottom), Some(Horiz::Left)),
        "{bottom-right}" => return Part::Edge(Some(Vert::Bottom), Some(Horiz::Right)),
        _ => {}
    }
    if let Some(name) = word.strip_prefix('=').filter(|n| !n.is_empty()) {
        return Part::Exact(name.to_string());
    }
    if let Some(n) = word.strip_prefix('+').filter(|n| digits(n)) {
        return n.parse().map_or_else(|_| Part::Name(word.to_string()), |n: isize| Part::Offset(n));
    }
    if let Some(n) = word.strip_prefix('-').filter(|n| digits(n)) {
        return n.parse().map_or_else(|_| Part::Name(word.to_string()), |n: isize| Part::Offset(-n));
    }
    if let Some(sigil) = word.chars().next().filter(|c| matches!(c, '$' | '@' | '%'))
        && digits(&word[1..])
    {
        return word[1..].parse().map_or_else(|_| Part::Name(word.to_string()), |n| Part::Id(sigil, n));
    }
    if digits(word) {
        return word.parse().map_or_else(|_| Part::Name(word.to_string()), Part::Number);
    }
    Part::Name(word.to_string())
}

/// Whether `text` matches `pattern`, where `*` stands for any run of characters and `?` for any one.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// The position of the name `want` among `names`: an exact match, else the only name that starts
/// with it, else the only one that matches it as a pattern. `what` is for the error.
pub fn pick_name<'a>(names: impl Iterator<Item = &'a str> + Clone, want: &str, what: &str) -> Result<usize, String> {
    if let Some(i) = names.clone().position(|n| n == want) {
        return Ok(i);
    }
    let unique = |hits: Vec<usize>| match hits.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => Err(format!("{what} {want:?} is ambiguous")),
    };
    if let Some(i) = unique(names.clone().enumerate().filter(|(_, n)| n.starts_with(want)).map(|(i, _)| i).collect())? {
        return Ok(i);
    }
    if let Some(i) = unique(names.enumerate().filter(|(_, n)| glob(want, n)).map(|(i, _)| i).collect())? {
        return Ok(i);
    }
    Err(format!("can't find {what}: {want}"))
}

/// Apply a relative or positional part to `current` among `count` things, wrapping round.
pub fn step(part: &Part, current: usize, last: Option<usize>, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match part {
        Part::Number(n) => (*n < count).then_some(*n),
        Part::Offset(d) => Some((current as isize + d).rem_euclid(count as isize) as usize),
        Part::Last => last.filter(|l| *l < count),
        Part::Start => Some(0),
        Part::End => Some(count - 1),
        Part::Id(..) | Part::Name(_) | Part::Exact(_) | Part::Beside(_) | Part::Edge(..) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(session: Option<&str>, window: Option<&str>, pane: Option<&str>) -> Target {
        Target { session: session.map(String::from), window: window.map(String::from), pane: pane.map(String::from) }
    }

    #[test]
    fn a_session_target_is_the_whole_word_without_a_trailing_colon() {
        assert_eq!(parse("work", Kind::Session).unwrap(), t(Some("work"), None, None));
        assert_eq!(parse("work:", Kind::Session).unwrap(), t(Some("work"), None, None));
        assert_eq!(parse("$3", Kind::Session).unwrap(), t(Some("$3"), None, None));
        assert_eq!(parse("", Kind::Session).unwrap(), Target::default());
    }

    #[test]
    fn a_window_target_is_session_colon_window_or_a_bare_window() {
        assert_eq!(parse("work:2", Kind::Window).unwrap(), t(Some("work"), Some("2"), None));
        assert_eq!(parse(":2", Kind::Window).unwrap(), t(None, Some("2"), None));
        assert_eq!(parse("work:", Kind::Window).unwrap(), t(Some("work"), None, None));
        assert_eq!(parse("2", Kind::Window).unwrap(), t(None, Some("2"), None));
        assert_eq!(parse("@4", Kind::Window).unwrap(), t(None, Some("@4"), None));
        assert_eq!(parse("work:editor", Kind::Window).unwrap(), t(Some("work"), Some("editor"), None));
    }

    #[test]
    fn a_pane_target_has_up_to_three_parts() {
        assert_eq!(parse("work:2.1", Kind::Pane).unwrap(), t(Some("work"), Some("2"), Some("1")));
        assert_eq!(parse(":2.1", Kind::Pane).unwrap(), t(None, Some("2"), Some("1")));
        assert_eq!(parse("2.1", Kind::Pane).unwrap(), t(None, Some("2"), Some("1")));
        assert_eq!(parse(".1", Kind::Pane).unwrap(), t(None, None, Some("1")));
        assert_eq!(parse("1", Kind::Pane).unwrap(), t(None, None, Some("1")));
        assert_eq!(parse("%12", Kind::Pane).unwrap(), t(None, None, Some("%12")));
        assert_eq!(parse("work:2", Kind::Pane).unwrap(), t(Some("work"), Some("2"), None));
        assert_eq!(parse("work:", Kind::Pane).unwrap(), t(Some("work"), None, None));
        assert_eq!(parse("{last}", Kind::Pane).unwrap(), t(None, None, Some("{last}")));
        assert_eq!(parse("!", Kind::Pane).unwrap(), t(None, None, Some("!")));
    }

    #[test]
    fn control_characters_and_long_targets_are_refused() {
        assert!(parse("a\nb", Kind::Session).is_err());
        assert!(parse(&"x".repeat(300), Kind::Pane).is_err());
    }

    #[test]
    fn neighbors_edges_corners_and_exact_names_are_parts() {
        assert_eq!(reference("{up-of}"), Part::Beside(Dir::Up));
        assert_eq!(reference("{down-of}"), Part::Beside(Dir::Down));
        assert_eq!(reference("{left-of}"), Part::Beside(Dir::Left));
        assert_eq!(reference("{right-of}"), Part::Beside(Dir::Right));
        assert_eq!(reference("{top}"), Part::Edge(Some(Vert::Top), None));
        assert_eq!(reference("{bottom}"), Part::Edge(Some(Vert::Bottom), None));
        assert_eq!(reference("{left}"), Part::Edge(None, Some(Horiz::Left)));
        assert_eq!(reference("{right}"), Part::Edge(None, Some(Horiz::Right)));
        assert_eq!(reference("{top-left}"), Part::Edge(Some(Vert::Top), Some(Horiz::Left)));
        assert_eq!(reference("{bottom-right}"), Part::Edge(Some(Vert::Bottom), Some(Horiz::Right)));
        assert_eq!(reference("=work"), Part::Exact("work".into()));
        assert_eq!(reference("="), Part::Name("=".into()));
        assert_eq!(reference("{nonsense}"), Part::Name("{nonsense}".into()));
    }

    #[test]
    fn braced_parts_survive_the_split_of_a_target() {
        assert_eq!(parse("work:{top}.{left-of}", Kind::Pane).unwrap(), Target { session: Some("work".into()), window: Some("{top}".into()), pane: Some("{left-of}".into()) });
        assert_eq!(parse("=work:=edit", Kind::Window).unwrap(), Target { session: Some("=work".into()), window: Some("=edit".into()), pane: None });
    }

    #[test]
    fn parts_are_read_as_numbers_offsets_ids_and_positions() {
        assert_eq!(reference("3"), Part::Number(3));
        assert_eq!(reference("+"), Part::Offset(1));
        assert_eq!(reference("-"), Part::Offset(-1));
        assert_eq!(reference("+3"), Part::Offset(3));
        assert_eq!(reference("-2"), Part::Offset(-2));
        assert_eq!(reference("{next}"), Part::Offset(1));
        assert_eq!(reference("{previous}"), Part::Offset(-1));
        assert_eq!(reference("!"), Part::Last);
        assert_eq!(reference("{last}"), Part::Last);
        assert_eq!(reference("^"), Part::Start);
        assert_eq!(reference("$"), Part::End);
        assert_eq!(reference("%12"), Part::Id('%', 12));
        assert_eq!(reference("@5"), Part::Id('@', 5));
        assert_eq!(reference("$3"), Part::Id('$', 3));
        assert_eq!(reference("editor"), Part::Name("editor".to_string()));
        assert_eq!(reference("-x"), Part::Name("-x".to_string()));
        assert_eq!(reference("99999999999999999999999"), Part::Name("99999999999999999999999".to_string()));
    }

    #[test]
    fn patterns_match_with_star_and_question_mark() {
        assert!(glob("ed*", "editor"));
        assert!(glob("*tor", "editor"));
        assert!(glob("e?itor", "editor"));
        assert!(glob("*", ""));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("ed", "editor"));
        assert!(!glob("a*b", "acd"));
        assert!(!glob("?", ""));
    }

    #[test]
    fn names_are_found_exact_then_by_prefix_then_by_pattern() {
        let names = ["work", "workshop", "play", "alpha-1", "alpha-2"];
        let pick = |w: &str| pick_name(names.iter().copied(), w, "session");
        assert_eq!(pick("work"), Ok(0), "an exact match wins over a longer name");
        assert_eq!(pick("pl"), Ok(2));
        assert_eq!(pick("*shop"), Ok(1));
        assert_eq!(pick("alpha-?").unwrap_err(), "session \"alpha-?\" is ambiguous");
        assert_eq!(pick("alpha").unwrap_err(), "session \"alpha\" is ambiguous");
        assert_eq!(pick("zzz").unwrap_err(), "can't find session: zzz");
    }

    #[test]
    fn steps_wrap_and_stay_inside() {
        assert_eq!(step(&Part::Number(2), 0, None, 3), Some(2));
        assert_eq!(step(&Part::Number(3), 0, None, 3), None);
        assert_eq!(step(&Part::Offset(1), 2, None, 3), Some(0));
        assert_eq!(step(&Part::Offset(-1), 0, None, 3), Some(2));
        assert_eq!(step(&Part::Offset(-7), 0, None, 3), Some(2));
        assert_eq!(step(&Part::Last, 0, Some(1), 3), Some(1));
        assert_eq!(step(&Part::Last, 0, None, 3), None);
        assert_eq!(step(&Part::Last, 0, Some(5), 3), None);
        assert_eq!(step(&Part::Start, 2, None, 3), Some(0));
        assert_eq!(step(&Part::End, 0, None, 3), Some(2));
        assert_eq!(step(&Part::Offset(1), 0, None, 0), None);
        assert_eq!(step(&Part::Name("x".into()), 0, None, 3), None);
    }
}
