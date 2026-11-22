//! The PNG, JPEG and GIF decoders behind the size limits.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = imux::image::decode_image(data);
});
