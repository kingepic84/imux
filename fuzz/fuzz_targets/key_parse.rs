//! Keys typed by the user (CSI u, mouse, escape sequences) and key names in bindings.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&flush, rest)) = data.split_first() else { return };
    let _ = imux::input::keys::parse(rest, flush & 1 == 1);
    if let Ok(text) = std::str::from_utf8(rest) {
        let _ = imux::input::keys::Key::parse(text);
        let _ = imux::input::keys::MouseKey::parse(text);
    }
});
