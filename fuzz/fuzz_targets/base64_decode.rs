//! The base64 decoder for picture payloads.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = imux::image::base64::decode(data);
});
