//! Pane output into the terminal emulator: escape sequences, images, OSC and DCS strings. The first
//! byte picks the chunking, so reads that split a sequence anywhere are covered too.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&split, rest)) = data.split_first() else { return };
    let mut term = imux::term::Terminal::new(80, 24, 1000);
    for chunk in rest.chunks(usize::from(split).max(1)) {
        term.feed(chunk);
    }
    term.advance_frames(std::time::Instant::now());
});
