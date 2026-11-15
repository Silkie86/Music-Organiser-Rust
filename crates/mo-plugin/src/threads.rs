//! The plugin's background threads, its locks and its crash note.
//!
//! The plugin lives inside the DAW's program, so it never takes over the DAW's own crash handling. Its own code's
//! panics (the plugin carries its own copy of Rust's standard library, so its panic hook sees only them) are written
//! to one small file in the app's logs folder, the last one only ([`CRASH_FILE`]: rule 17, crashes always log; rule
//! 28, every file it writes has a limit). A panic in a background thread ends that thread's work, not the DAW.

use std::io::Write;
use std::sync::{Condvar, Mutex, MutexGuard, Once};
use std::time::Duration;

/// The plugin's last crash, in the app's logs folder: replaced by the next one, never more than [`MOST_BYTES`].
pub const CRASH_FILE: &str = "mo-plugin-crash.txt";
const MOST_BYTES: usize = 64 * 1024;

/// A lock that a panic elsewhere left poisoned is still used: what the plugin keeps behind its locks (a queue's end, a
/// newest-wins slot, a name) is whole at every step.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `work` on a thread of its own at low priority (never the DAW's threads). A panic in it is written to the crash
/// note and ends only that work.
pub fn background(name: &str, work: impl FnOnce() + Send + 'static) {
    note_crashes();
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
        let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(work));
    });
}

/// Work handed to a [`OneAtATime`].
type Job = Box<dyn FnOnce() + Send>;

/// One low-priority thread that works out a tab's list, one asking at a time: an asking that comes while it works
/// takes the place of the one waiting, so after a burst (a Tempo drag, fast typing, the Like knob moved by the DAW)
/// only the newest runs next, and no pile of passes builds up on the pool (the 14 Nov review's PW-1). The thread is
/// made at the first asking and ends when this goes (the window closes).
#[derive(Default)]
pub struct OneAtATime {
    worker: Mutex<Option<mo_app::background::Newest<Job>>>,
}

impl OneAtATime {
    /// Hands `job` to the thread, in place of any job still waiting. If the thread can't be made, it runs on a
    /// thread of its own, as before.
    pub fn ask(&self, job: impl FnOnce() + Send + 'static) {
        let mut worker = lock(&self.worker);
        if worker.is_none() {
            *worker = mo_app::background::Newest::start("mo-plugin-list", |job: Job| job()).ok();
        }
        match worker.as_ref() {
            Some(worker) => worker.ask(Box::new(job)),
            None => background("mo-plugin-list", job),
        }
    }
}

/// Puts `found` (the answer to asking `number`) in `slot`, unless an answer to a newer asking is there already.
pub fn newer<T>(slot: &Mutex<Option<(u64, T)>>, number: u64, found: T) {
    let mut slot = lock(slot);
    if slot.as_ref().is_none_or(|(n, _)| *n < number) {
        *slot = Some((number, found));
    }
}

/// A sleep that a stop cuts short: a thread that looks every so often ends at once when it's told to, instead of
/// sleeping on in the plugin's code (the 14 Nov review's PL-1).
#[derive(Default)]
pub struct Nap {
    woken: Mutex<u64>,
    wake: Condvar,
}

impl Nap {
    /// Sleeps for `time`, or until [`Nap::wake`] is called.
    pub fn sleep(&self, time: Duration) {
        let woken = lock(&self.woken);
        let seen = *woken;
        let _ = self.wake.wait_timeout_while(woken, time, |w| *w == seen);
    }

    /// Wakes every thread sleeping now.
    pub fn wake(&self) {
        *lock(&self.woken) += 1;
        self.wake.notify_all();
    }
}

/// Keeps the plugin's file in memory until the DAW quits (Windows): a DAW may free a plugin's file as soon as its last
/// copy goes (FL Studio does), and a thread of the plugin's still finishing its work (a take being saved, a pool
/// thread) would then wake into code that's gone and close the whole DAW (the 14 Nov review's PL-1). Called once,
/// as the first copy is made: a scan that makes no copy can still let the file go.
#[allow(unsafe_code)]
pub fn pin_the_file() {
    #[cfg(windows)]
    {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            use windows::Win32::System::LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
            };
            let mut module = windows::Win32::Foundation::HMODULE::default();
            let inside = pin_the_file as fn() as *const u16;
            // SAFETY: the address is a function in this plugin's own file, which is loaded while this runs; Windows
            // only looks the file up from it and writes its handle to `module`, which outlives the call.
            let pinned = unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                    windows::core::PCWSTR(inside),
                    &raw mut module,
                )
            };
            if let Err(e) = pinned {
                crate::log::line("Plugin", &format!("Couldn't keep the plugin's file loaded: {e}"));
            }
        });
    }
}

/// Sets up the crash note, once for the whole plugin.
pub fn note_crashes() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let thread = std::thread::current().name().unwrap_or("a thread with no name").to_owned();
            let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let backtrace = std::backtrace::Backtrace::force_capture();
            let mut text = format!("Music Organiser test plugin crashed at {at} on {thread}:\n{info}\n\n{backtrace}\n");
            if text.len() > MOST_BYTES {
                let mut cut = MOST_BYTES;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
            }
            let logs = mo_app::paths::data_dir().join("logs");
            if std::fs::create_dir_all(&logs).is_ok()
                && let Ok(mut file) = std::fs::File::create(logs.join(CRASH_FILE))
            {
                let _ = file.write_all(text.as_bytes());
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn a_burst_of_askings_runs_only_the_newest_after_the_one_under_way() {
        let lister = OneAtATime::default();
        let ran = Arc::new(Mutex::new(Vec::new()));
        let (hold, held) = std::sync::mpsc::channel::<()>();
        let first = Arc::clone(&ran);
        lister.ask(move || {
            let _ = held.recv_timeout(Duration::from_secs(10));
            lock(&first).push(0);
        });
        // While the first runs, 50 more come: only the last is done after it.
        std::thread::sleep(Duration::from_millis(50));
        for n in 1..=50 {
            let ran = Arc::clone(&ran);
            lister.ask(move || lock(&ran).push(n));
        }
        hold.send(()).unwrap();
        let started = Instant::now();
        while lock(&ran).len() < 2 && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*lock(&ran), [0, 50]);
        // An older answer never replaces a newer one.
        let slot = Mutex::new(None);
        newer(&slot, 2, "two");
        newer(&slot, 1, "one");
        assert_eq!(*lock(&slot), Some((2, "two")));
    }

    #[test]
    fn a_nap_ends_when_it_is_woken() {
        let nap = Arc::new(Nap::default());
        let sleeper = Arc::clone(&nap);
        let started = Instant::now();
        let slept = std::thread::spawn(move || sleeper.sleep(Duration::from_secs(30)));
        std::thread::sleep(Duration::from_millis(50));
        nap.wake();
        slept.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "woken, not slept out");
        // Unwoken, it sleeps its time.
        let started = Instant::now();
        nap.sleep(Duration::from_millis(30));
        assert!(started.elapsed() >= Duration::from_millis(30));
    }
}
