//! Format strings (`#{...}`): conditionals, loops, padding, substitution, quoting.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let lookup = |name: &str| match name.len() % 3 {
        0 => Some("value".to_string()),
        1 => Some(String::new()),
        _ => None,
    };
    let _ = imux::config::format::expand(text, &lookup);
    let _ = imux::config::format::expand_for_shell(text, &lookup, &|name| name.len() % 2 == 0);
});
