//! The body of an OSC 7501 agent status report.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = imux::term::status::parse(data);
});
