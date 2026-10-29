//! The freeze watchdog (rule 12, no lag): a small thread knocks on the window's thread five times a second. If a knock
//! isn't answered within 250 ms, the window was stuck, and when it answers the log gets a `[Freeze]` line saying for
//! how long, on which page and with which jobs running. One that's still stuck after 5 s says so straight away, in
//! case it never comes back. The watchdog never waits on the window itself, so it can't make a freeze worse.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::log;

/// Longer than this without an answer is a freeze.
pub const STALL: Duration = Duration::from_millis(250);
/// Still stuck after this long gets its own line, before it ends.
pub const STILL_STUCK: Duration = Duration::from_secs(5);
const KNOCK_EVERY: Duration = Duration::from_millis(200);
const LOOK_EVERY: Duration = Duration::from_millis(25);

/// Starts watching. Call it from inside the event loop (for example with `slint::invoke_from_event_loop` before
/// `run`), so the time the window takes to open isn't counted as a freeze. Stops by itself when the window closes.
pub fn start(feed: Feed) {
    let spawned = std::thread::Builder::new().name("Freeze watchdog".into()).spawn(move || watch(&feed));
    if let Err(e) = spawned {
        log::warning("Freeze", &format!("Couldn't start the freeze watchdog: {e}"));
    }
}

fn watch(feed: &Feed) {
    loop {
        std::thread::sleep(KNOCK_EVERY);
        let answered = Arc::new(AtomicBool::new(false));
        let a = answered.clone();
        let sent = Instant::now();
        if slint::invoke_from_event_loop(move || a.store(true, Ordering::Release)).is_err() {
            return; // The event loop has ended.
        }
        let mut said_still_stuck = false;
        while !answered.load(Ordering::Acquire) {
            std::thread::sleep(LOOK_EVERY);
            let waited = sent.elapsed();
            if waited >= STILL_STUCK && !said_still_stuck {
                said_still_stuck = true;
                log::error("Freeze", &line(waited, true, crate::page_name(), &feed.running_titles()));
                log::current().inspect(|l| l.flush());
            }
        }
        let waited = sent.elapsed();
        if waited >= STALL {
            log::warning("Freeze", &line(waited, false, crate::page_name(), &feed.running_titles()));
        }
    }
}

/// The log line for a freeze.
pub fn line(waited: Duration, still_stuck: bool, page: &str, running: &[String]) -> String {
    let jobs =
        if running.is_empty() { "no jobs running".to_owned() } else { format!("running: {}", running.join(", ")) };
    if still_stuck {
        format!("The window is still stuck after {:.1} s (page {page}; {jobs})", waited.as_secs_f32())
    } else {
        format!("The window was stuck for {} ms (page {page}; {jobs})", waited.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_lines_say_how_long_where_and_what_ran() {
        assert_eq!(
            line(Duration::from_millis(612), false, "Sounds", &["Analysing sounds".to_owned()]),
            "The window was stuck for 612 ms (page Sounds; running: Analysing sounds)"
        );
        assert_eq!(
            line(Duration::from_millis(5020), true, "Songs", &[]),
            "The window is still stuck after 5.0 s (page Songs; no jobs running)"
        );
    }
}
