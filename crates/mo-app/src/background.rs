//! Threads for work away from the window (rule 21): each runs at low priority, so the window and the DAW always come
//! first.

use std::io;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

/// Starts `work` on a thread of its own called `name`, at low priority. Only the watchdog, which must notice the
/// window freezing however busy the computer is, and the session log's writer, which a crash waits for, start their
/// threads another way.
pub fn spawn<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> io::Result<JoinHandle<T>> {
    std::thread::Builder::new().name(name.to_owned()).spawn(move || {
        lower_priority();
        work()
    })
}

/// Runs `work` on the app's pool of low-priority threads, one per core (rule 21), so what it spreads over every core
/// (rayon's `par_iter`: walking folders, reading many projects) runs there rather than on rayon's own pool at normal
/// priority. Waits for it, so never call it on the window's thread.
pub fn on_pool<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    static POOL: std::sync::LazyLock<Option<rayon::ThreadPool>> = std::sync::LazyLock::new(|| {
        rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("Background {i}"))
            .start_handler(|_| lower_priority())
            .build()
            .ok()
    });
    match POOL.as_ref() {
        Some(pool) => pool.install(work),
        // The system wouldn't start the threads: done here instead, still away from the window.
        None => work(),
    }
}

/// Lowers the priority of the thread it's called on. Where the system won't (some Linux set-ups), the thread runs on
/// as it is.
pub fn lower_priority() {
    let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
}

/// A low-priority thread that does one job at a time. Asking while it's busy replaces the job still waiting, so after
/// a burst of asks (clicking down a list) only the newest is done next (rule 12: newest wins). The thread stops once
/// this is dropped and its current job ends.
pub struct Newest<J: Send + 'static> {
    slot: Arc<Slot<J>>,
}

struct Slot<J> {
    state: Mutex<SlotState<J>>,
    ready: Condvar,
}

struct SlotState<J> {
    job: Option<J>,
    closed: bool,
}

impl<J: Send + 'static> Newest<J> {
    /// Starts the thread called `name`; `work` does each job. What `work` needs between jobs (an open database) lives
    /// in it.
    pub fn start(name: &str, mut work: impl FnMut(J) + Send + 'static) -> io::Result<Newest<J>> {
        let slot = Arc::new(Slot { state: Mutex::new(SlotState { job: None, closed: false }), ready: Condvar::new() });
        let theirs = slot.clone();
        spawn(name, move || {
            loop {
                let job = {
                    let mut state = theirs.state.lock().unwrap_or_else(PoisonError::into_inner);
                    loop {
                        if let Some(job) = state.job.take() {
                            break job;
                        }
                        if state.closed {
                            return;
                        }
                        state = theirs.ready.wait(state).unwrap_or_else(PoisonError::into_inner);
                    }
                };
                work(job);
            }
        })?;
        Ok(Newest { slot })
    }

    /// Gives the thread `job`, in place of any job it hasn't started yet.
    pub fn ask(&self, job: J) {
        self.slot.state.lock().unwrap_or_else(PoisonError::into_inner).job = Some(job);
        self.slot.ready.notify_one();
    }
}

impl<J: Send + 'static> Drop for Newest<J> {
    fn drop(&mut self) {
        self.slot.state.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        self.slot.ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn only_the_newest_waiting_job_is_done() {
        let (done_tx, done) = mpsc::channel();
        let (gate_tx, gate) = mpsc::channel::<()>();
        let (started_tx, started) = mpsc::channel::<()>();
        let worker = Newest::start("Test newest", move |n: u32| {
            if n == 0 {
                // Hold the first job until the others have been asked for.
                let _ = started_tx.send(());
                let _ = gate.recv();
            }
            let _ = done_tx.send(n);
        })
        .unwrap();
        worker.ask(0);
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        for n in 1..=5 {
            worker.ask(n);
        }
        gate_tx.send(()).unwrap();
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), 5);
        drop(worker);
        // The thread stops and its sender goes with it.
        assert!(done.recv_timeout(Duration::from_secs(5)).is_err());
    }

    #[test]
    fn work_runs_on_a_named_thread() {
        let name = spawn("Test job", || std::thread::current().name().map(str::to_owned)).unwrap().join().unwrap();
        assert_eq!(name.as_deref(), Some("Test job"));
    }
}
