//! The plugin's background threads, its locks and its crash note.
//!
//! The plugin lives inside the DAW's program, so it never takes over the DAW's own crash handling. Its own code's
//! panics (the plugin carries its own copy of Rust's standard library, so its panic hook sees only them) are written
//! to one small file in the app's logs folder, the last one only ([`CRASH_FILE`]: rule 17, crashes always log; rule
//! 28, every file it writes has a limit). A panic in a background thread ends that thread's work, not the DAW.

use std::io::Write;
use std::sync::{Mutex, MutexGuard, Once};

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
