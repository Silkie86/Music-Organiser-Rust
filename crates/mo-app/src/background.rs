//! Threads for work away from the window (rule 21): each runs at low priority, so the window and the DAW always come
//! first.

use std::io;
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError, mpsc};
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

/// Lets go of `value` on a low-priority thread instead of here (rule 12, no UI waits). Something made of millions of
/// small parts, as half a million sounds each with their own names, tags and analysis are, takes long enough to free
/// that letting it go on the window's thread freezes the window: on Solomon's PC an old list of 453,885 sounds took
/// 1.4 s. One thread frees for the whole app, one thing at a time, in the order they come; where it can't be started,
/// `value` is freed here after all (see [`freeing_here`]).
pub fn free_later<T: Send + 'static>(value: T) {
    static FREEING: LazyLock<Option<mpsc::Sender<Box<dyn Send>>>> = LazyLock::new(|| {
        let (to_free, freeing) = mpsc::channel::<Box<dyn Send>>();
        spawn("Freeing", move || {
            FREEING_HERE.with(|here| here.set(true));
            for value in freeing {
                // A part that panics as it's let go is logged (the panic hook does that) and the thread goes on, so
                // what comes after is still freed here, not on the window's thread.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(value)));
            }
        })
        .ok()
        .map(|_| to_free)
    });
    #[cfg(test)]
    HANDED_OVER.with(|h| h.set(h.get() + 1));
    let left = match FREEING.as_ref() {
        // Only if the thread has gone (it never ends by itself) does the value come back here.
        Some(to_free) => to_free.send(Box::new(value)).err().map(|e| e.0),
        None => Some(Box::new(value) as Box<dyn Send>),
    };
    if let Some(value) = left {
        FREEING_HERE.with(|here| {
            let was = here.replace(true);
            drop(value);
            here.set(was);
        });
    }
}

/// Whether this thread is letting go of something [`free_later`] was given: its own thread, or here when that thread
/// can't be started. Something that hands itself to [`free_later`] when it's let go (a big sound list) checks this, so
/// it's freed then, not handed on again.
pub fn freeing_here() -> bool {
    FREEING_HERE.with(std::cell::Cell::get)
}

thread_local! {
    static FREEING_HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
thread_local! {
    /// How many things this thread has handed to [`free_later`], for the tests.
    static HANDED_OVER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many things this thread has handed to [`free_later`].
#[cfg(test)]
pub(crate) fn handed_over() -> usize {
    HANDED_OVER.with(std::cell::Cell::get)
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
    fn what_is_let_go_later_is_freed_on_the_freeing_thread() {
        // Says which thread it was freed on, and whether that thread knew it was freeing.
        struct Noted(mpsc::Sender<(Option<String>, bool)>);
        impl Drop for Noted {
            fn drop(&mut self) {
                let _ = self.0.send((std::thread::current().name().map(str::to_owned), freeing_here()));
            }
        }
        // Panics as it's let go.
        struct Breaks;
        impl Drop for Breaks {
            fn drop(&mut self) {
                panic!("a part that breaks as it's freed (the test means it)");
            }
        }
        let (noted, freed_on) = mpsc::channel();
        free_later(Noted(noted.clone()));
        assert_eq!(freed_on.recv_timeout(Duration::from_secs(5)).unwrap(), (Some("Freeing".to_owned()), true));
        assert!(!freeing_here());
        // One that panics doesn't stop the thread: what comes after is still freed there.
        free_later(Breaks);
        free_later(Noted(noted));
        assert_eq!(freed_on.recv_timeout(Duration::from_secs(5)).unwrap(), (Some("Freeing".to_owned()), true));
    }

    #[test]
    fn work_runs_on_a_named_thread() {
        let name = spawn("Test job", || std::thread::current().name().map(str::to_owned)).unwrap().join().unwrap();
        assert_eq!(name.as_deref(), Some("Test job"));
    }
}
