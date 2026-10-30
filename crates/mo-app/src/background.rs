//! Threads for work away from the window (rule 21): each runs at low priority, so the window and the DAW always come
//! first.

use std::io;
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

/// Lowers the priority of the thread it's called on. Where the system won't (some Linux set-ups), the thread runs on
/// as it is.
pub fn lower_priority() {
    let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_runs_on_a_named_thread() {
        let name = spawn("Test job", || std::thread::current().name().map(str::to_owned)).unwrap().join().unwrap();
        assert_eq!(name.as_deref(), Some("Test job"));
    }
}
