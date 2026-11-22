//! One line of a configuration file: the lexer, variables, braces and the statement parser.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(line) = std::str::from_utf8(data) else { return };
    let mut vars = imux::config::lex::Vars::default();
    let _ = imux::config::lex::statements(line, &mut vars);
    let mut vars = imux::config::lex::Vars::default();
    let _ = imux::config::stmt::parse_line(line, &mut vars);
    let _ = imux::config::options::parse_style(line);
});
