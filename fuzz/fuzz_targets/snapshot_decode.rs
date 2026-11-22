//! The snapshot and autosave file parser.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = imux::persist::format::decode(text);
    }
});
