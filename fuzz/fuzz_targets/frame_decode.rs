//! The client/server wire format: whatever arrives on the socket, in any chunking.
#![no_main]
use imux::proto::{ClientMsg, FrameBuf, ServerMsg};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&split, rest)) = data.split_first() else { return };
    let mut client = FrameBuf::default();
    let mut server = FrameBuf::default();
    for chunk in rest.chunks(usize::from(split).max(1)) {
        client.push(chunk);
        server.push(chunk);
        while let Ok(Some(_)) = client.next::<ClientMsg>() {}
        while let Ok(Some(_)) = server.next::<ServerMsg>() {}
    }
});
