//! Kitty graphics commands, chunked transmissions and the pixel assembly (media files stay off).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut assembler = imux::image::kitty::Assembler::default();
    // A NUL byte splits the input into several commands, so chunk sequences are reachable.
    for part in data.split(|&b| b == 0) {
        if let Ok(Some(command)) = assembler.push(part) {
            let _ = imux::image::kitty::bitmap(&command);
        }
    }
});
