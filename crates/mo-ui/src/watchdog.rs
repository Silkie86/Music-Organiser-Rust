//! The freeze watchdog (rule 12, no lag): a small thread knocks on the window's thread five times a second. If a knock
//! isn't answered within 250 ms, the window was stuck, and when it answers the log gets a `[Freeze]` line saying for
//! how long, on which page and with which jobs running. One that's still stuck after 5 s says so straight away, in
//! case it never comes back. The watchdog never waits on the window itself, so it can't make a freeze worse.
//!
//! The window's own steps that might be slow mark themselves with [`doing`], so a freeze line says which step the
//! window was in ("busy with Songs: show the song's details"), or that it was in none of them (then it was drawing or
//! laying out). A marked step that takes longer than [`STALL`] gets its own `[Slow]` line with its time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::log;

/// Longer than this without an answer is a freeze.
pub const STALL: Duration = Duration::from_millis(250);
/// Still stuck after this long gets its own line, before it ends.
pub const STILL_STUCK: Duration = Duration::from_secs(5);
const KNOCK_EVERY: Duration = Duration::from_millis(200);
const LOOK_EVERY: Duration = Duration::from_millis(25);

/// The marked step the window is in, and since when.
static DOING: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);

/// Marks a step on the window's thread until the guard is dropped: `let _doing = watchdog::doing("Songs: pick");`.
pub fn doing(what: &'static str) -> Doing {
    let since = Instant::now();
    let outer = DOING.lock().ok().and_then(|mut d| d.replace((what, since)));
    Doing { what, since, outer }
}

/// A step being done; see [`doing`].
pub struct Doing {
    what: &'static str,
    since: Instant,
    /// The step this one is inside, put back when this one ends.
    outer: Option<(&'static str, Instant)>,
}

impl Drop for Doing {
    fn drop(&mut self) {
        let took = self.since.elapsed();
        if took >= STALL {
            log::warning("Slow", &format!("{} took {} ms on the window's thread", self.what, took.as_millis()));
        }
        if let Ok(mut d) = DOING.lock() {
            *d = self.outer.take();
        }
    }
}

/// What the window is busy with, for a freeze line.
fn busy_with() -> String {
    match DOING.lock().ok().and_then(|d| *d) {
        Some((what, since)) => format!("busy with {what} for {} ms", since.elapsed().as_millis()),
        None => "in no marked step, so drawing or laying out".to_owned(),
    }
}

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
        // What the window was busy with once it had been stuck for STALL: the step the freeze is in.
        let mut busy = None;
        while !answered.load(Ordering::Acquire) {
            std::thread::sleep(LOOK_EVERY);
            let waited = sent.elapsed();
            if waited >= STALL && busy.is_none() {
                busy = Some(busy_with());
            }
            if waited >= STILL_STUCK && !said_still_stuck {
                said_still_stuck = true;
                let now = busy_with();
                log::error("Freeze", &line(waited, true, crate::page_name(), &feed.running_titles(), &now));
                log::current().inspect(|l| l.flush());
            }
        }
        let waited = sent.elapsed();
        if waited >= STALL {
            let busy = busy.unwrap_or_else(busy_with);
            log::warning("Freeze", &line(waited, false, crate::page_name(), &feed.running_titles(), &busy));
        }
    }
}

/// The log line for a freeze.
pub fn line(waited: Duration, still_stuck: bool, page: &str, running: &[String], busy: &str) -> String {
    let jobs =
        if running.is_empty() { "no jobs running".to_owned() } else { format!("running: {}", running.join(", ")) };
    if still_stuck {
        format!("The window is still stuck after {:.1} s (page {page}; {jobs}; {busy})", waited.as_secs_f32())
    } else {
        format!("The window was stuck for {} ms (page {page}; {jobs}; {busy})", waited.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_lines_say_how_long_where_and_what_ran() {
        assert_eq!(
            line(
                Duration::from_millis(612),
                false,
                "Sounds",
                &["Analysing sounds".to_owned()],
                "busy with x for 600 ms"
            ),
            "The window was stuck for 612 ms (page Sounds; running: Analysing sounds; busy with x for 600 ms)"
        );
        assert_eq!(
            line(Duration::from_millis(5020), true, "Songs", &[], "in no marked step, so drawing or laying out"),
            "The window is still stuck after 5.0 s (page Songs; no jobs running; in no marked step, so drawing or \
             laying out)"
        );
    }

    #[test]
    fn a_step_marks_itself_and_puts_back_the_one_it_was_in() {
        let outer = doing("outer step");
        assert!(busy_with().starts_with("busy with outer step"));
        {
            let _inner = doing("inner step");
            assert!(busy_with().starts_with("busy with inner step"));
        }
        assert!(busy_with().starts_with("busy with outer step"));
        drop(outer);
        assert_eq!(busy_with(), "in no marked step, so drawing or laying out");
    }
}
