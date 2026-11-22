//! The same pane output through two terminals: one decodes pictures where it stands, the other hands
//! them to the decoder thread and holds the output after each (as the server does). They must end
//! on the same screen, whatever the chunking, and the held one must never lose or reorder bytes.
#![no_main]
use imux::image::worker::{self, Pool};
use imux::term::Terminal;
use libfuzzer_sys::fuzz_target;

thread_local! {
    static POOL: Pool = worker::spawn(|| {}).expect("decoder thread");
}

fn look(t: &Terminal) -> Vec<Vec<(char, bool)>> {
    t.grid.screen.iter().map(|r| r.cells.iter().map(|c| (c.ch, c.tile != 0)).collect()).collect()
}

fuzz_target!(|data: &[u8]| {
    let Some((&split, rest)) = data.split_first() else { return };
    let chunk = usize::from(split).max(1);
    let mut plain = Terminal::new(40, 12, 100);
    let mut deferred = Terminal::new(40, 12, 100);
    POOL.with(|pool| {
        deferred.set_decoder(pool.handle.with_tag(1));
        for part in rest.chunks(chunk) {
            plain.feed(part);
            deferred.feed(part);
            while deferred.is_held() {
                let done = pool.done.recv().expect("the decoder answers");
                deferred.finish_decode(done.seq, done.result);
            }
        }
        assert_eq!(look(&plain), look(&deferred), "decoding in place and on the worker differ");
        assert_eq!((plain.grid.cx, plain.grid.cy), (deferred.grid.cx, deferred.grid.cy));
        assert_eq!(plain.grid.images.tile_count(), deferred.grid.images.tile_count());
    });
});
