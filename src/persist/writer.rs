//! Writing the autosave on a thread of its own, so the `fsync` of a large state directory never
//! stalls the server loop. One thread owns the autosave files: the temporary file name and the
//! rename of the old file to `autosave.prev` are not safe with two writers, and a queue keeps the
//! writes in the order they were asked for.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use super::Store;

/// One autosave to write: the encoded snapshot and the hash the server compares to skip a save
/// that would hold the same thing.
pub struct Job {
    pub text: String,
    pub hash: u64,
}

/// What became of a job.
pub struct Done {
    pub hash: u64,
    pub result: Result<(), String>,
}

pub struct Writer {
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    thread: Option<JoinHandle<()>>,
    pending: usize,
}

impl Writer {
    pub fn spawn(store: Store) -> io::Result<Writer> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let (finished, done) = mpsc::channel::<Done>();
        let thread = thread::Builder::new().name("autosave".to_string()).spawn(move || {
            for job in queue {
                slow_down_for_tests();
                let result = store.save_autosave_text(&job.text).map_err(|e| format!("autosave: {e}"));
                if finished.send(Done { hash: job.hash, result }).is_err() {
                    break;
                }
            }
        })?;
        Ok(Writer { jobs: Some(jobs), done, thread: Some(thread), pending: 0 })
    }

    /// A write is queued or running.
    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    /// Queue a write. An error means the thread is gone; the caller writes the file itself then.
    pub fn submit(&mut self, job: Job) -> Result<(), String> {
        let Some(jobs) = &self.jobs else { return Err("the autosave thread has stopped".to_string()) };
        jobs.send(job).map_err(|_| "the autosave thread has stopped".to_string())?;
        self.pending += 1;
        Ok(())
    }

    /// The writes that finished since the last call; does not wait.
    pub fn poll(&mut self) -> Vec<Done> {
        let mut out = Vec::new();
        while let Ok(done) = self.done.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            out.push(done);
        }
        out
    }

    /// Wait until every queued write is finished and return what became of them. A thread that
    /// died with writes outstanding is counted as done, so this never blocks for ever.
    pub fn flush(&mut self) -> Vec<Done> {
        let mut out = Vec::new();
        while self.pending > 0 {
            match self.done.recv() {
                Ok(done) => {
                    self.pending -= 1;
                    out.push(done);
                }
                Err(_) => self.pending = 0,
            }
        }
        out
    }

    /// Finish the queued writes and stop the thread.
    pub fn shutdown(mut self) -> Vec<Done> {
        let out = self.flush();
        self.jobs = None;
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            crate::log::error("the autosave thread panicked");
        }
        out
    }
}

/// Debug builds only: `IMUX_TEST_SAVE_DELAY_MS` makes every write take that much longer, so a
/// test can show that the server loop keeps answering meanwhile.
fn slow_down_for_tests() {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("IMUX_TEST_SAVE_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        thread::sleep(std::time::Duration::from_millis(ms.min(10_000)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imux-writer-{tag}-{}", std::process::id()));
        if let Err(e) = fs::remove_dir_all(&dir) {
            assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}");
        }
        dir
    }

    #[test]
    fn writes_happen_in_the_order_they_were_asked_for() {
        let dir = scratch("order");
        let mut w = Writer::spawn(Store::new(dir.clone())).unwrap();
        w.submit(Job { text: "first\n".into(), hash: 1 }).unwrap();
        w.submit(Job { text: "second\n".into(), hash: 2 }).unwrap();
        assert!(w.busy());
        let done = w.flush();
        assert_eq!(done.iter().map(|d| d.hash).collect::<Vec<_>>(), vec![1, 2]);
        assert!(done.iter().all(|d| d.result.is_ok()));
        assert!(!w.busy());
        assert_eq!(fs::read_to_string(dir.join("autosave.imux")).unwrap(), "second\n");
        assert_eq!(fs::read_to_string(dir.join("autosave.prev")).unwrap(), "first\n");
        assert!(w.shutdown().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_is_reported_with_its_hash() {
        let dir = scratch("fail");
        fs::write(&dir, "a file where the state directory should be").unwrap();
        let mut w = Writer::spawn(Store::new(dir.clone())).unwrap();
        w.submit(Job { text: "x\n".into(), hash: 7 }).unwrap();
        let done = w.flush();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].hash, 7);
        assert!(done[0].result.as_ref().is_err_and(|e| e.starts_with("autosave: ")), "{:?}", done[0].result);
        w.shutdown();
        fs::remove_file(dir).unwrap();
    }

    #[test]
    fn poll_does_not_wait_and_shutdown_finishes_what_is_queued() {
        let dir = scratch("poll");
        let mut w = Writer::spawn(Store::new(dir.clone())).unwrap();
        assert!(w.poll().is_empty());
        w.submit(Job { text: "kept\n".into(), hash: 3 }).unwrap();
        let rest = w.shutdown();
        assert_eq!(rest.len(), 1);
        assert_eq!(fs::read_to_string(dir.join("autosave.imux")).unwrap(), "kept\n");
        fs::remove_dir_all(dir).unwrap();
    }
}
