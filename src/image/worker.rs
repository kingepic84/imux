//! Decoding pictures off the server's thread.
//!
//! The server runs everything on one thread, and a picture that is small on the wire can be huge
//! to decode: a few KB of PNG that expands to 32 Mpx costs tens of milliseconds, and a program can
//! send hundreds of them (the server answered nothing for half a minute). So a pane's terminal
//! hands the bytes to this worker, holds that pane's own output until the answer is back, and the
//! rest of the server (other panes, clients, the keyboard) carries on.
//!
//! One worker thread, on purpose: one decode at a time bounds the memory a decode needs (up to
//! `MAX_DECODE_ALLOC`). The bytes waiting for it are bounded too (`MAX_QUEUED_BYTES`); past that
//! the caller decodes on its own thread, as before.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use super::kitty::{self, Control};
use super::{Bitmap, decode_any};
use crate::log;

/// Bytes of pictures waiting for, or in, the worker, over all panes together.
pub const MAX_QUEUED_BYTES: usize = 256 * 1024 * 1024;

/// What to decode.
pub enum Job {
    /// An inline image of the iTerm2 kind: PNG, JPEG, GIF or Sixel.
    Any(Vec<u8>),
    /// A Kitty graphics payload, with the control data that says how it is encoded.
    Kitty { control: Control, payload: Vec<u8> },
}

impl Job {
    /// Bytes of input, for the queue's bound.
    pub fn bytes(&self) -> usize {
        match self {
            Job::Any(data) => data.len(),
            Job::Kitty { payload, .. } => payload.len(),
        }
    }

    /// Do the work. Pure.
    pub fn run(self) -> Result<Bitmap, String> {
        match self {
            Job::Any(data) => decode_any(&data),
            Job::Kitty { control, payload } => kitty::decode_payload(&control, &payload),
        }
    }
}

struct Request {
    tag: u64,
    seq: u64,
    job: Job,
}

/// An answer: for the terminal `tag` (a pane id), its request number `seq`.
pub struct Done {
    pub tag: u64,
    pub seq: u64,
    pub result: Result<Bitmap, String>,
}

/// What a terminal holds to send work: cloned per pane with its own tag.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<Request>,
    queued: Arc<AtomicUsize>,
    tag: u64,
}

impl Handle {
    /// The same worker, answering to `tag`.
    pub fn with_tag(&self, tag: u64) -> Handle {
        Handle { tx: self.tx.clone(), queued: Arc::clone(&self.queued), tag }
    }

    /// Queue `job` as request `seq`. When the worker is gone or too much is queued, the job comes
    /// back and the caller runs it itself.
    pub fn submit(&self, seq: u64, job: Job) -> Result<(), Box<Job>> {
        let bytes = job.bytes();
        if self.queued.fetch_add(bytes, Ordering::SeqCst) + bytes > MAX_QUEUED_BYTES {
            self.queued.fetch_sub(bytes, Ordering::SeqCst);
            return Err(Box::new(job));
        }
        match self.tx.send(Request { tag: self.tag, seq, job }) {
            Ok(()) => Ok(()),
            Err(returned) => {
                self.queued.fetch_sub(bytes, Ordering::SeqCst);
                Err(Box::new(returned.0.job))
            }
        }
    }
}

/// The worker's two ends as the server holds them: a handle to give out and the answers.
pub struct Pool {
    pub handle: Handle,
    pub done: Receiver<Done>,
}

/// Start the worker. `wake` is called after every answer, from the worker's thread, to get the
/// server loop's attention.
pub fn spawn(wake: impl Fn() + Send + 'static) -> std::io::Result<Pool> {
    let (tx, rx) = channel::<Request>();
    let (done_tx, done) = channel::<Done>();
    let queued = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&queued);
    std::thread::Builder::new().name("decode".into()).spawn(move || {
        while let Ok(Request { tag, seq, job }) = rx.recv() {
            let bytes = job.bytes();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run())).unwrap_or_else(|payload| {
                let why = log::panic_text(payload.as_ref());
                log::error(&format!("picture decoder panicked: {why}"));
                Err("the picture decoder failed".to_string())
            });
            count.fetch_sub(bytes, Ordering::SeqCst);
            if done_tx.send(Done { tag, seq, result }).is_err() {
                return;
            }
            wake();
        }
    })?;
    Ok(Pool { handle: Handle { tx, queued, tag: 0 }, done })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let img = ::image::RgbaImage::from_pixel(w, h, ::image::Rgba([1, 2, 3, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut out), ::image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn a_job_comes_back_decoded_with_its_tag_and_number() {
        let pool = spawn(|| {}).unwrap();
        let h = pool.handle.with_tag(7);
        h.submit(3, Job::Any(png(4, 5))).ok().unwrap();
        let done = pool.done.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((done.tag, done.seq), (7, 3));
        let bitmap = done.result.unwrap();
        assert_eq!((bitmap.w, bitmap.h), (4, 5));
    }

    #[test]
    fn a_bad_picture_is_an_error_not_a_dead_worker() {
        let pool = spawn(|| {}).unwrap();
        let h = pool.handle.with_tag(1);
        h.submit(1, Job::Any(b"not a picture".to_vec())).ok().unwrap();
        assert!(pool.done.recv_timeout(Duration::from_secs(10)).unwrap().result.is_err());
        h.submit(2, Job::Any(png(2, 2))).ok().unwrap();
        assert!(pool.done.recv_timeout(Duration::from_secs(10)).unwrap().result.is_ok(), "it still works");
    }

    #[test]
    fn answers_come_in_the_order_the_jobs_went_in() {
        let pool = spawn(|| {}).unwrap();
        let h = pool.handle.with_tag(1);
        for seq in 1..=5 {
            h.submit(seq, Job::Any(png(seq as u32, 1))).ok().unwrap();
        }
        let seqs: Vec<u64> = (0..5).map(|_| pool.done.recv_timeout(Duration::from_secs(10)).unwrap().seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn too_many_queued_bytes_hand_the_job_back() {
        let pool = spawn(|| {}).unwrap();
        pool.handle.queued.store(MAX_QUEUED_BYTES, Ordering::SeqCst);
        let h = pool.handle.with_tag(1);
        assert!(h.submit(1, Job::Any(vec![0; 16])).is_err(), "over the bound: the caller decodes it");
        pool.handle.queued.store(0, Ordering::SeqCst);
        assert!(h.submit(2, Job::Any(png(1, 1))).is_ok());
    }

    #[test]
    fn the_wake_function_runs_after_an_answer() {
        let woken = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&woken);
        let pool = spawn(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        pool.handle.with_tag(1).submit(1, Job::Any(png(1, 1))).ok().unwrap();
        pool.done.recv_timeout(Duration::from_secs(10)).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while woken.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }
}
