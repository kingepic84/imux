//! The parameters and payload of an iTerm2 inline picture.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = imux::image::iterm::parse(data);
});
