//! Threads for work away from the window (rule 21): each runs at low priority, so the window and the DAW always come
//! first.

use std::io;
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;

/// Starts `work` on a thread of its own called `name`, at low priority. Only the watchdog, which must notice the
/// window freezing however busy the computer is, the session log's writer, which a crash waits for, and Play in FL
/// Studio's start, whose note-timing thread takes its priority on Linux and Mac (a short read of the part's notes
/// first), start their threads another way.
pub fn spawn<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> io::Result<JoinHandle<T>> {
    std::thread::Builder::new().name(name.to_owned()).spawn(move || {
        lower_priority();
        work()
    })
}

/// Runs `work` on the app's pool of low-priority threads, one per core (rule 21), so what it spreads over every core
/// (rayon's `par_iter`: reading many projects, drawing a picture, filtering a list) runs there rather than on rayon's
/// own pool at normal priority. Waits for it, so never call it on the window's thread.
///
/// The pool takes on a new job only once the jobs it's on have nothing left to share out (rayon finishes what it
/// started first), so long work that waits on a disk (a backup, walking a whole drive) goes to [`on_bulk_pool`]
/// instead: here it would hold up every page's quick asks until it ended.
pub fn on_pool<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    install(pool(), work)
}

/// The app's pool of low-priority threads itself, for work that hands jobs to a pool of its own choosing (the MIDI
/// reader). `None` when the system wouldn't start the threads.
pub fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: LazyLock<Option<rayon::ThreadPool>> =
        LazyLock::new(|| build_pool("Background", POOL_THREADS.load(std::sync::atomic::Ordering::Relaxed)));
    POOL.as_ref()
}

/// Runs long work that mostly waits on a disk (a backup's plan and copy, its look at the drive) on a small pool of its
/// own, at low priority, so the pages' quick jobs on [`on_pool`] still come back at once while it runs. A few threads
/// keep a drive busy without making a USB drive jump between many files. Waits for it, so never on the window's thread.
pub fn on_bulk_pool<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    install(bulk_pool(), work)
}

/// The bulk pool itself ([`on_bulk_pool`]), for work that hands jobs to a pool of its own choosing (the MIDI reader,
/// which reads a whole drive of files: a few at a time keep one drive busy, where one per core made it jump between
/// them). `None` when the system wouldn't start the threads.
pub fn bulk_pool() -> Option<&'static rayon::ThreadPool> {
    static BULK: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
        let threads = POOL_THREADS.load(std::sync::atomic::Ordering::Relaxed);
        let threads = if threads == 0 { cores() } else { threads };
        build_bulk_pool("Bulk", threads.clamp(1, BULK_THREADS))
    });
    BULK.as_ref()
}

/// [`on_bulk_pool`] for a backup's copy, which can run for hours: a pool of its own again, so working out the next
/// plan (or reading the drive's older versions) while it copies isn't held up behind it either.
pub fn on_copy_pool<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    static COPY: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
        let threads = POOL_THREADS.load(std::sync::atomic::Ordering::Relaxed);
        let threads = if threads == 0 { cores() } else { threads };
        build_bulk_pool("Backup copy", threads.clamp(1, BULK_THREADS))
    });
    install(COPY.as_ref(), work)
}

/// The most threads [`on_bulk_pool`] (and [`on_copy_pool`]) gets.
const BULK_THREADS: usize = 4;

/// A pool of `threads` low-priority threads called `name 0`, `name 1`...; 0 is rayon's own choice, one per core.
fn build_pool(name: &'static str, threads: usize) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |i| format!("{name} {i}"))
        .start_handler(|_| lower_priority())
        .build()
        .ok()
}

/// [`build_pool`] for long work over a disk: its threads are also low in the system's memory, so what they read
/// (hundreds of gigabytes, for a look over every project) is the first the system lets go of when memory runs short,
/// before the window's own. On a PC with most of its memory in use, the window froze for up to a second while the
/// system read its memory back from the disk (2026-11-04, a look for Bitwig copies).
fn build_bulk_pool(name: &'static str, threads: usize) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |i| format!("{name} {i}"))
        .start_handler(|_| {
            lower_priority();
            lower_memory_priority();
        })
        .build()
        .ok()
}

/// Puts what the thread it's called on reads and uses low in the system's memory (Windows; elsewhere nothing changes).
/// The bulk pools' threads do it themselves; a thread of its own from [`spawn`] that reads gigabytes calls it first.
#[allow(unsafe_code)]
pub fn lower_memory_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_LOW, SetThreadInformation,
            ThreadMemoryPriority,
        };
        let info = MEMORY_PRIORITY_INFORMATION { MemoryPriority: MEMORY_PRIORITY_LOW };
        // SAFETY: the current thread's own pseudo-handle, and a struct of the size given, read during the call only.
        unsafe {
            SetThreadInformation(
                GetCurrentThread(),
                ThreadMemoryPriority,
                (&raw const info).cast(),
                std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            );
        }
    }
}

fn install<T: Send>(pool: Option<&rayon::ThreadPool>, work: impl FnOnce() -> T + Send) -> T {
    match pool {
        Some(pool) => pool.install(work),
        // The system wouldn't start the threads: done here instead, still away from the window.
        None => work(),
    }
}

/// Runs `work`, and if it panics (the panic hook has already logged what broke, as a crash: `mo_catch::catch_crash`)
/// gives the words to show instead, so a page that asked for it hears back rather than staying busy. For work that
/// may trip on a damaged file or an odd database row; never for work that leaves something half-changed.
pub fn guarded<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    mo_catch::catch_crash(std::panic::AssertUnwindSafe(work))
        .map_err(|_| "it stopped on something it didn't expect (the log says what)".to_owned())
}

/// How many threads the pool gets, from Settings' pace; 0 for one per core. Read once, when the pool starts (the bulk
/// pool takes no more than this either).
static POOL_THREADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How hard background work may push the computer (Settings › General › Pace; the C# app's `performance` setting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Half the processor's threads, so the window, the galaxy and playback stay smooth even on a busy computer.
    Gentle,
    /// All but two.
    Balanced,
    /// Every thread, still at low priority. This app's default (it always ran this way), where the C# app's is Gentle.
    Fast,
}

impl Pace {
    pub const ALL: [Pace; 3] = [Pace::Gentle, Pace::Balanced, Pace::Fast];

    /// Its name, as the setting keeps it.
    pub fn name(self) -> &'static str {
        match self {
            Pace::Gentle => "Gentle",
            Pace::Balanced => "Balanced",
            Pace::Fast => "Fast",
        }
    }

    /// The pace from its setting; Fast when it isn't set or isn't known.
    pub fn from_setting(value: Option<&str>) -> Pace {
        Pace::ALL.into_iter().find(|p| Some(p.name()) == value).unwrap_or(Pace::Fast)
    }

    /// How many threads it gives background work on a computer with `cores` of them (at least one).
    pub fn threads(self, cores: usize) -> usize {
        let cores = cores.max(1);
        match self {
            Pace::Gentle => (cores / 2).max(1),
            Pace::Balanced => cores.saturating_sub(2).max(1),
            Pace::Fast => cores,
        }
    }
}

/// The processor's threads, as the system tells them (1 if it won't).
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// Sets the pool's size from the pace. Call it once in `main`, before any work goes to the pool: the pool starts at
/// the first job and keeps its size after that, so a pace changed later takes over at the next start.
pub fn set_pace(pace: Pace) {
    POOL_THREADS.store(pace.threads(cores()), std::sync::atomic::Ordering::Relaxed);
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
                let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| drop(value)));
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
/// this is dropped and its current job ends. A job that panics is logged (the panic hook does that) and the thread goes
/// on to the next one, so one damaged file never leaves a page waiting for good.
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
                // `work` keeps nothing half-changed that the next job relies on: each job starts from its own ask.
                let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| work(job)));
            }
        })?;
        Ok(Newest { slot })
    }

    /// Gives the thread `job`, in place of any job it hasn't started yet; that one is let go here, before this
    /// returns (a job that counts itself out as it goes is counted out by then).
    pub fn ask(&self, job: J) {
        drop(self.replace(job));
    }

    /// As [`Newest::ask`], but the job it takes the place of is let go on the freeing thread ([`free_later`]), not
    /// here: for asks from the window's thread whose jobs may hold the last of something big (a list worked out
    /// again since, an index read again).
    pub fn ask_freeing_later(&self, job: J) {
        if let Some(replaced) = self.replace(job) {
            free_later(replaced);
        }
    }

    /// Puts `job` where the thread takes its next one from, and gives back the one that was waiting there.
    fn replace(&self, job: J) -> Option<J> {
        let replaced = self.slot.state.lock().unwrap_or_else(PoisonError::into_inner).job.replace(job);
        self.slot.ready.notify_one();
        replaced
    }
}

impl<J: Send + 'static> Drop for Newest<J> {
    fn drop(&mut self) {
        self.slot.state.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        self.slot.ready.notify_one();
    }
}

/// Small things to write of which only the newest for each key counts (the octave you chose for a part, with the key
/// held down). The newest value said for each key waits here, and a low-priority thread writes all that's waiting in
/// one go, then looks again: it's started when there's something to write and ends when there's nothing. So two
/// values for one key are never written out of order, and a burst of them is written once. A write that panics is
/// logged (the panic hook does that) and what it was given is lost; the thread goes on to what's waiting.
pub struct Latest<K, V> {
    state: Mutex<LatestState<K, V>>,
}

struct LatestState<K, V> {
    /// In the order each key was first said.
    waiting: Vec<(K, V)>,
    /// A thread is writing, and looks here again when it's done.
    writing: bool,
}

impl<K: PartialEq + Send + 'static, V: Send + 'static> Latest<K, V> {
    pub const fn new() -> Latest<K, V> {
        Latest { state: Mutex::new(LatestState { waiting: Vec::new(), writing: false }) }
    }

    /// Says `value` for `key`, in place of a value still waiting for that key. Everything waiting is given to `write`
    /// on a thread called `name`, which is started here unless one is writing already (that one then writes this
    /// too, with the `write` it was started with: give every call the same one). Err: the thread couldn't start; the
    /// value stays waiting, and goes with the next one said.
    pub fn say(
        &'static self,
        name: &str,
        key: K,
        value: V,
        write: impl Fn(Vec<(K, V)>) + Send + 'static,
    ) -> io::Result<()> {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match state.waiting.iter_mut().find(|(k, _)| *k == key) {
                Some(waiting) => waiting.1 = value,
                None => state.waiting.push((key, value)),
            }
            if std::mem::replace(&mut state.writing, true) {
                return Ok(());
            }
        }
        let started = spawn(name, move || {
            loop {
                let batch = {
                    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    if state.waiting.is_empty() {
                        state.writing = false;
                        return;
                    }
                    std::mem::take(&mut state.waiting)
                };
                let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| write(batch)));
            }
        });
        if let Err(e) = started {
            self.state.lock().unwrap_or_else(PoisonError::into_inner).writing = false;
            return Err(e);
        }
        Ok(())
    }
}

impl<K: PartialEq + Send + 'static, V: Send + 'static> Default for Latest<K, V> {
    fn default() -> Latest<K, V> {
        Latest::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn the_newest_said_for_each_key_is_written_in_one_go() {
        static SAID: Latest<&str, u32> = Latest::new();
        let (done_tx, done) = mpsc::channel::<Vec<(&str, u32)>>();
        let (gate_tx, gate) = mpsc::channel::<()>();
        let (started_tx, started) = mpsc::channel::<()>();
        let write = |done: mpsc::Sender<Vec<(&'static str, u32)>>| move |batch| drop(done.send(batch));
        // The first write is held until more has been said.
        let (first_done, first_started) = (done_tx.clone(), started_tx);
        SAID.say("Test latest", "a", 0, move |batch| {
            if batch == [("a", 0)] {
                let _ = first_started.send(());
                let _ = gate.recv();
            }
            let _ = first_done.send(batch);
        })
        .unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        for n in 1..=3 {
            SAID.say("Test latest", "a", n, write(done_tx.clone())).unwrap();
            SAID.say("Test latest", "b", 10 + n, write(done_tx.clone())).unwrap();
        }
        gate_tx.send(()).unwrap();
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), [("a", 0)]);
        // What was said meanwhile: the newest for each key, once, in the order the keys were first said.
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), [("a", 3), ("b", 13)]);
        // With nothing waiting the thread ends, and the next thing said starts another.
        let ended = || !SAID.state.lock().unwrap().writing;
        for _ in 0..500 {
            if ended() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ended());
        SAID.say("Test latest", "b", 99, write(done_tx)).unwrap();
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), [("b", 99)]);
        assert!(done.recv_timeout(Duration::from_secs(5)).is_err(), "nothing is written twice");
    }

    #[test]
    fn a_write_that_panics_leaves_the_next_thing_said_written() {
        static SAID: Latest<&str, u32> = Latest::new();
        let (done_tx, done) = mpsc::channel::<Vec<(&str, u32)>>();
        let (started_tx, started) = mpsc::channel::<()>();
        let write = |started: mpsc::Sender<()>, done: mpsc::Sender<Vec<(&'static str, u32)>>| {
            move |batch: Vec<(&'static str, u32)>| {
                let _ = started.send(());
                assert!(batch != [("a", 1)], "a write that breaks (the test means it)");
                let _ = done.send(batch);
            }
        };
        SAID.say("Test latest panics", "a", 1, write(started_tx.clone(), done_tx.clone())).unwrap();
        // Said once the first is breaking: still written, by that thread or by a new one.
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        SAID.say("Test latest panics", "b", 2, write(started_tx, done_tx)).unwrap();
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), [("b", 2)]);
    }

    #[test]
    fn paces_share_the_threads_like_the_csharp_app() {
        assert_eq!(Pace::Gentle.threads(16), 8);
        assert_eq!(Pace::Balanced.threads(16), 14);
        assert_eq!(Pace::Fast.threads(16), 16);
        // Never none, on the smallest computer.
        assert_eq!(Pace::Gentle.threads(1), 1);
        assert_eq!(Pace::Balanced.threads(2), 1);
        assert_eq!(Pace::from_setting(Some("Balanced")), Pace::Balanced);
        assert_eq!(Pace::from_setting(Some("?")), Pace::Fast);
        assert_eq!(Pace::from_setting(None), Pace::Fast);
    }

    #[test]
    fn long_disk_work_leaves_the_main_pool_free() {
        use rayon::prelude::*;
        let (release_tx, release) = mpsc::channel::<()>();
        let release = Mutex::new(release);
        let (started_tx, started) = mpsc::channel::<()>();
        // A long par_iter on the disk pool, held until the main pool's job has answered.
        let long = std::thread::spawn(move || {
            on_bulk_pool(|| {
                let _ = started_tx.send(());
                (0..64).into_par_iter().for_each(|_| {
                    let r = release.lock().unwrap_or_else(PoisonError::into_inner);
                    let _ = r.recv_timeout(Duration::from_secs(10));
                });
            });
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let (answer_tx, answer) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer_tx.send(on_pool(|| (0..1000).into_par_iter().sum::<u64>()));
        });
        assert_eq!(answer.recv_timeout(Duration::from_secs(5)).unwrap(), 499_500);
        drop(release_tx);
        long.join().unwrap();
    }

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
    fn a_job_replaced_while_it_waits_is_let_go_on_the_freeing_thread() {
        // Says which thread it was let go on.
        struct Job(u32, mpsc::Sender<(u32, Option<String>)>);
        impl Drop for Job {
            fn drop(&mut self) {
                let _ = self.1.send((self.0, std::thread::current().name().map(str::to_owned)));
            }
        }
        let (gone_tx, gone) = mpsc::channel();
        let (gate_tx, gate) = mpsc::channel::<()>();
        let (started_tx, started) = mpsc::channel::<()>();
        let worker = Newest::start("Test replaced", move |job: Job| {
            if job.0 == 0 {
                let _ = started_tx.send(());
                let _ = gate.recv();
            }
        })
        .unwrap();
        worker.ask(Job(0, gone_tx.clone()));
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        // The first is being done; the second waits, and the third takes its place.
        worker.ask(Job(1, gone_tx.clone()));
        worker.ask_freeing_later(Job(2, gone_tx.clone()));
        assert_eq!(gone.recv_timeout(Duration::from_secs(5)).unwrap(), (1, Some("Freeing".to_owned())));
        // A plain ask lets the one it replaces go here, before it returns.
        worker.ask(Job(3, gone_tx));
        let here = std::thread::current().name().map(str::to_owned);
        assert_eq!(gone.try_recv().unwrap(), (2, here));
        gate_tx.send(()).unwrap();
        // The jobs that are done end on the thread that does them.
        let mut done = vec![gone.recv_timeout(Duration::from_secs(5)).unwrap()];
        done.push(gone.recv_timeout(Duration::from_secs(5)).unwrap());
        assert_eq!(done, [(0, Some("Test replaced".to_owned())), (3, Some("Test replaced".to_owned()))]);
    }

    #[test]
    fn guarded_work_says_when_it_broke() {
        assert_eq!(guarded(|| 2 + 2), Ok(4));
        let broke: Result<(), String> = guarded(|| panic!("a job that breaks (the test means it)"));
        assert!(broke.unwrap_err().contains("didn't expect"));
    }

    #[test]
    fn a_job_that_panics_leaves_the_thread_working() {
        let (done_tx, done) = mpsc::channel();
        let (started_tx, started) = mpsc::channel();
        let worker = Newest::start("Test panics", move |n: u32| {
            let _ = started_tx.send(n);
            assert!(n != 1, "a job that breaks (the test means it)");
            let _ = done_tx.send(n);
        })
        .unwrap();
        worker.ask(1);
        // Asked once the first is breaking: still done.
        assert_eq!(started.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        worker.ask(2);
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
    }

    #[test]
    fn quick_jobs_come_back_while_long_bulk_work_runs() {
        use rayon::prelude::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        let hold = Arc::new(AtomicBool::new(true));
        let held = hold.clone();
        let (started_tx, started) = mpsc::channel::<()>();
        // Long work that keeps every bulk thread busy until it's let go.
        let long = spawn("Test bulk", move || {
            on_bulk_pool(|| {
                (0..64).into_par_iter().for_each(|_| {
                    let _ = started_tx.send(());
                    while held.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                });
            });
        })
        .unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        // A quick job on the app's pool still comes straight back.
        let (quick_tx, quick) = mpsc::channel();
        let _quick = spawn("Test quick", move || {
            let sum: u64 = on_pool(|| (0..1000u64).into_par_iter().sum());
            let _ = quick_tx.send(sum);
        })
        .unwrap();
        let answer = quick.recv_timeout(Duration::from_secs(5));
        hold.store(false, Ordering::Relaxed);
        long.join().unwrap();
        assert_eq!(answer.unwrap(), 499_500);
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
