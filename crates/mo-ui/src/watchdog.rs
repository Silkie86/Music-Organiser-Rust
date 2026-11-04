//! The freeze watchdog (rule 12, no lag): a small thread knocks on the window's thread five times a second. If a knock
//! isn't answered within 250 ms, the window was stuck, and when it answers the log gets a `[Freeze]` line saying for
//! how long, on which page and with which jobs running. One that's still stuck after 5 s says so straight away, in
//! case it never comes back. The watchdog never waits on the window itself, so it can't make a freeze worse.
//!
//! The window's own steps that might be slow mark themselves with [`doing`], so a freeze line says which step the
//! window was in ("busy with Songs: show the song's details"), or that it was in none of them. Slint drawing the window
//! is marked too ([`drawing`]), so "no marked step" means Slint handling input, laying out or showing the frame. A
//! marked step that takes longer than [`STALL`] gets its own `[Slow]` line with its time.
//!
//! A freeze outside every marked step says more, so the next one can be pinned down: where the window's thread was
//! (looked at once it has been stuck for [`LOOK_AT_THREAD`], and again at [`STILL_STUCK`], see [`crate::stuck`]), how
//! many pages of memory the app had to find while it was stuck (page faults, less those of the looks themselves), and
//! the window's last move, resize, minimising or hiding before it, with how long before.
//!
//! The frame timing (`mo_app::frame_timing`, the `[Frames]` lines) hears from here too: each marked step's own time
//! (less the steps inside it), and every knock answered later than 33 ms with the step the window was in.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::{frame_timing, log};

/// Longer than this without an answer is a freeze.
pub const STALL: Duration = Duration::from_millis(250);
/// Still stuck after this long gets its own line, before it ends.
pub const STILL_STUCK: Duration = Duration::from_secs(5);
/// Stuck this long, the window's thread is looked at to see where it is.
pub const LOOK_AT_THREAD: Duration = Duration::from_secs(1);
const KNOCK_EVERY: Duration = Duration::from_millis(200);
const LOOK_EVERY: Duration = Duration::from_millis(25);

/// The marked step the window is in, and since when.
static DOING: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);

/// The window's thread, kept by [`start`]. Only steps on it are marked: a background thread's long wait isn't the
/// window being busy.
static WINDOW: OnceLock<ThreadId> = OnceLock::new();

/// The window's last move, resize, focus change or hiding, and when (see [`window_event`]).
static LAST_WINDOW_EVENT: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);

thread_local! {
    /// For each marked step going on (on this thread), how long the steps inside it took, so each counts only its own
    /// time in the frame timing.
    static INSIDE: RefCell<Vec<Duration>> = const { RefCell::new(Vec::new()) };
}

fn on_window() -> bool {
    WINDOW.get().is_none_or(|w| *w == std::thread::current().id())
}

/// Marks a step on the window's thread until the guard is dropped: `let _doing = watchdog::doing("Songs: pick");`.
/// On any other thread it marks nothing (before [`start`], as in tests, every thread counts).
pub fn doing(what: &'static str) -> Doing {
    let since = Instant::now();
    let on_window = on_window();
    let outer = if on_window { DOING.lock().ok().and_then(|mut d| d.replace((what, since))) } else { None };
    if on_window {
        INSIDE.with(|i| i.borrow_mut().push(Duration::ZERO));
    }
    Doing { what, since, outer, on_window }
}

/// Slint is drawing the window (`true`, from its "before rendering" call) or has finished (`false`). Marks it like a
/// step, unless a marked step is going on already.
pub fn drawing(started: bool) {
    if !on_window() {
        return;
    }
    let Ok(mut d) = DOING.lock() else { return };
    if started {
        if d.is_none() {
            *d = Some((frame_timing::DRAWING, Instant::now()));
        }
    } else if d.is_some_and(|(what, _)| what == frame_timing::DRAWING) {
        *d = None;
    }
}

/// The window was moved, resized, hidden, shown or changed focus ("resized", "moved"...): kept for the freeze lines.
pub fn window_event(what: &'static str) {
    if let Ok(mut last) = LAST_WINDOW_EVENT.lock() {
        *last = Some((what, Instant::now()));
    }
}

/// Notes the window's moves, resizes, focus changes, minimising and hiding for the freeze lines. Minimised, restored,
/// hidden and shown again are logged too: they're rare, and a window coming back from the page file is one cause of a
/// long freeze. (On Windows, winit says nothing of a covered window, and a minimised one is a resize to nothing.)
/// Nothing else is changed: every event goes on to Slint as before.
///
/// Slint keeps one such watcher a window, so this one is the app's: anything else that needs the window's own events
/// must be called from here, not set up beside it.
pub fn watch_window(app: &crate::AppWindow) {
    use slint::ComponentHandle;
    use slint::winit_030::winit::event::WindowEvent;
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    let mut minimised = false;
    app.window().on_winit_window_event(move |_, event| {
        let what = match event {
            WindowEvent::Resized(size) if size.width == 0 || size.height == 0 => {
                minimised = true;
                "minimised"
            }
            WindowEvent::Resized(_) if std::mem::take(&mut minimised) => "restored",
            WindowEvent::Resized(_) => "resized",
            WindowEvent::Moved(_) => "moved",
            WindowEvent::Focused(true) => "focused",
            WindowEvent::Focused(false) => "lost focus",
            WindowEvent::Occluded(true) => "hidden",
            WindowEvent::Occluded(false) => "shown again",
            WindowEvent::ScaleFactorChanged { .. } => "scale changed",
            _ => return EventResult::Propagate,
        };
        window_event(what);
        if matches!(what, "minimised" | "restored" | "hidden" | "shown again") {
            log::info("Window", &format!("The window was {what}"));
        }
        EventResult::Propagate
    });
}

/// The window's last event before `stuck_at`, for a freeze line: "last window event: resized 40 ms before".
fn window_words(stuck_at: Instant) -> String {
    words_for(LAST_WINDOW_EVENT.lock().ok().and_then(|l| *l), stuck_at)
}

fn words_for(last: Option<(&'static str, Instant)>, stuck_at: Instant) -> String {
    match last {
        Some((what, at)) if at <= stuck_at => {
            format!("last window event: {what} {} ms before", stuck_at.duration_since(at).as_millis())
        }
        Some((what, _)) => format!("window event while stuck: {what}"),
        None => "no window event yet".to_owned(),
    }
}

/// A step being done; see [`doing`].
pub struct Doing {
    what: &'static str,
    since: Instant,
    /// The step this one is inside, put back when this one ends.
    outer: Option<(&'static str, Instant)>,
    /// Off the window's thread: nothing was marked, so nothing is put back or logged.
    on_window: bool,
}

impl Drop for Doing {
    fn drop(&mut self) {
        if !self.on_window {
            return;
        }
        let took = self.since.elapsed();
        // Its own time goes to the frame timing; all of it counts as time inside the step it's in.
        let inside = INSIDE.with(|i| {
            let mut i = i.borrow_mut();
            let inside = i.pop().unwrap_or_default();
            if let Some(outer) = i.last_mut() {
                *outer += took;
            }
            inside
        });
        report(self.what, took.saturating_sub(inside));
        if took >= STALL {
            log::warning("Slow", &format!("{} took {} ms on the window's thread", self.what, took.as_millis()));
        }
        if let Ok(mut d) = DOING.lock() {
            *d = self.outer.take();
        }
    }
}

/// Gives a marked step's own time to the frame timing (and, in the tests, to a list they can look at).
fn report(what: &'static str, own: Duration) {
    #[cfg(test)]
    tests::REPORTED.with(|r| r.borrow_mut().push((what, own)));
    // Before [`start`] every thread's steps are marked, so the frame timing hears only once the window's thread is
    // known: a background job's step at start-up isn't the window being busy.
    if WINDOW.get().is_some() {
        frame_timing::step(what, own);
    }
}

/// Which step a late answer is put down to, from the step seen at each look while it waited: the one seen most, and
/// of those the one seen last (`None` is no marked step).
fn blame(seen: &[Option<&'static str>]) -> Option<&'static str> {
    let mut counts: Vec<(Option<&'static str>, usize)> = Vec::new();
    let mut best: Option<(Option<&'static str>, usize)> = None;
    for &step in seen {
        let n = match counts.iter_mut().find(|(s, _)| *s == step) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                counts.push((step, 1));
                1
            }
        };
        if best.is_none_or(|(_, most)| n >= most) {
            best = Some((step, n));
        }
    }
    best.and_then(|(step, _)| step)
}

/// The marked step the window is in now, if any.
fn busy_step() -> Option<&'static str> {
    DOING.lock().ok().and_then(|d| *d).map(|(what, _)| what)
}

/// What the window is busy with, for a freeze line.
fn busy_with() -> String {
    match DOING.lock().ok().and_then(|d| *d) {
        Some((what, since)) => format!("busy with {what} for {} ms", since.elapsed().as_millis()),
        None => format!("in {}", frame_timing::UNMARKED),
    }
}

/// Starts watching. Call it from inside the event loop (for example with `slint::invoke_from_event_loop` before
/// `run`), so the time the window takes to open isn't counted as a freeze. Stops by itself when the window closes.
pub fn start(feed: Feed) {
    let _ = WINDOW.set(std::thread::current().id());
    crate::stuck::remember_this_thread();
    // The app's debugging names are read now, away from the window and at low priority, not at the first freeze: on a
    // busy disk that took over 5 s, which held up the watchdog's look at 5 s (Solomon's PC, 2026-11-04).
    if let Err(e) = mo_app::background::spawn("Freeze names", crate::stuck::get_names_ready) {
        log::info("Freeze", &format!("Couldn't read the app's debugging names ahead of time: {e}"));
    }
    let spawned = std::thread::Builder::new().name("Freeze watchdog".into()).spawn(move || watch(&feed));
    if let Err(e) = spawned {
        log::warning("Freeze", &format!("Couldn't start the freeze watchdog: {e}"));
    }
}

fn watch(feed: &Feed) {
    loop {
        std::thread::sleep(KNOCK_EVERY);
        // How long the knock waited, in microseconds (at least 1), once it's answered; 0 until then.
        let answered = Arc::new(AtomicU64::new(0));
        let a = answered.clone();
        let sent = Instant::now();
        let memory_before = crate::stuck::memory_now();
        let answer = move || {
            let waited = u64::try_from(sent.elapsed().as_micros()).unwrap_or(u64::MAX).max(1);
            a.store(waited, Ordering::Release);
        };
        if slint::invoke_from_event_loop(answer).is_err() {
            return; // The event loop has ended.
        }
        let mut said_still_stuck = false;
        let mut looked_at_thread = false;
        // The page faults of the watchdog's own looks at the window's thread (reading the app's symbols), left out
        // of the freeze's count.
        let mut own_faults = 0;
        // What the window was busy with once it had been stuck for STALL: the step the freeze is in.
        let mut busy = None;
        // The step it was in at each look, for a late answer (answered before the first, it wasn't late). Kept to the
        // first few hundred looks: a longer wait is a freeze, which has its own line.
        let mut seen: Vec<Option<&'static str>> = Vec::new();
        while answered.load(Ordering::Acquire) == 0 {
            std::thread::sleep(LOOK_EVERY);
            // Answered while this look slept: what the window does now came after the answer, so it isn't looked at.
            if answered.load(Ordering::Acquire) != 0 {
                break;
            }
            let waited = sent.elapsed();
            if seen.len() < 400 {
                seen.push(busy_step());
            }
            if waited >= STALL && busy.is_none() {
                busy = Some(busy_with());
            }
            if waited >= LOOK_AT_THREAD && !looked_at_thread {
                looked_at_thread = true;
                own_faults += log_thread(waited);
                log::current().inspect(|l| l.flush());
            }
            if waited >= STILL_STUCK && !said_still_stuck {
                said_still_stuck = true;
                let now = format!("{}; {}", busy_with(), window_words(sent));
                log::error("Freeze", &line(waited, true, crate::page_name(), &feed.running_titles(), &now));
                log::current().inspect(|l| l.flush());
                own_faults += log_thread(waited);
                log::current().inspect(|l| l.flush());
            }
        }
        let waited = Duration::from_micros(answered.load(Ordering::Acquire));
        frame_timing::late(waited, blame(&seen), crate::page_name());
        if waited >= STALL {
            let mut busy = busy.unwrap_or_else(busy_with);
            busy.push_str("; ");
            busy.push_str(&window_words(sent));
            if let Some(memory) = crate::stuck::memory_words(memory_before, crate::stuck::memory_now(), own_faults) {
                busy.push_str("; ");
                busy.push_str(&memory);
            }
            log::warning("Freeze", &line(waited, false, crate::page_name(), &feed.running_titles(), &busy));
        }
    }
}

/// Where the window's thread is, stuck for `waited`, as a `[Freeze]` line of its own (nothing where it can't be seen).
/// Gives back the page faults the look itself made.
fn log_thread(waited: Duration) -> u64 {
    let before = crate::stuck::memory_now();
    if let Some(frames) = crate::stuck::window_thread_now() {
        log::warning("Freeze", &thread_line(waited, &frames));
    }
    match (before, crate::stuck::memory_now()) {
        (Some(b), Some(a)) => a.page_faults.saturating_sub(b.page_faults),
        _ => 0,
    }
}

/// "Stuck for 1.0 s, the window's thread was in:" and its functions, innermost first, one a line.
pub fn thread_line(waited: Duration, frames: &[String]) -> String {
    let mut text = format!("Stuck for {:.1} s, the window's thread was in:", waited.as_secs_f32());
    for f in frames {
        text.push_str("\n    ");
        text.push_str(f);
    }
    text
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

    /// The marked step is kept for the whole app, so the tests that mark steps take turns.
    static TURN: Mutex<()> = Mutex::new(());

    thread_local! {
        /// The steps' own times as given to the frame timing, on this thread.
        pub(super) static REPORTED: RefCell<Vec<(&'static str, Duration)>> = const { RefCell::new(Vec::new()) };
    }

    #[test]
    fn each_step_counts_only_its_own_time_not_the_steps_inside_it() {
        let _turn = TURN.lock();
        REPORTED.with(|r| r.borrow_mut().clear());
        let began = Instant::now();
        let outer = doing("outer");
        std::thread::sleep(Duration::from_millis(30));
        {
            let _inner = doing("inner");
            std::thread::sleep(Duration::from_millis(60));
        }
        std::thread::sleep(Duration::from_millis(10));
        drop(outer);
        let whole = began.elapsed();
        let reported = REPORTED.with(|r| r.borrow().clone());
        let own = |what| reported.iter().find(|(w, _)| *w == what).map(|(_, t)| *t).expect("reported");
        assert_eq!(reported.len(), 2);
        assert!(own("inner") >= Duration::from_millis(60));
        // The outer step's own 40 ms or so, not its whole 100: its time less the inner one's. (Sleeps can overrun a
        // lot on a busy machine, so it's checked against what was measured, not against fixed times.)
        assert!(own("outer") >= Duration::from_millis(40), "{:?}", own("outer"));
        assert!(own("outer") + own("inner") <= whole, "{:?} + {:?} > {whole:?}", own("outer"), own("inner"));
        assert!(whole.saturating_sub(own("outer") + own("inner")) < Duration::from_millis(5), "{whole:?}");
    }

    #[test]
    fn a_late_answer_is_put_down_to_the_step_seen_most_or_last() {
        assert_eq!(blame(&[]), None);
        assert_eq!(blame(&[Some("a"), Some("b"), Some("b")]), Some("b"));
        // A tie goes to the one seen last before the answer.
        assert_eq!(blame(&[Some("a"), None]), None);
        assert_eq!(blame(&[None, Some("a")]), Some("a"));
        assert_eq!(blame(&[Some("a"), Some("a"), None, Some("b")]), Some("a"));
    }

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
            line(Duration::from_millis(5020), true, "Songs", &[], "in no marked step"),
            "The window is still stuck after 5.0 s (page Songs; no jobs running; in no marked step)"
        );
    }

    #[test]
    fn the_thread_line_lists_its_functions_one_a_line() {
        assert_eq!(
            thread_line(Duration::from_millis(1040), &["inner (a.rs:3)".to_owned(), "outer".to_owned()]),
            "Stuck for 1.0 s, the window's thread was in:\n    inner (a.rs:3)\n    outer"
        );
    }

    #[test]
    fn freeze_lines_say_what_the_window_last_did() {
        let at = Instant::now();
        let later = at + Duration::from_millis(40);
        assert_eq!(words_for(Some(("restored", at)), later), "last window event: restored 40 ms before");
        assert_eq!(words_for(Some(("resized", later)), at), "window event while stuck: resized");
        assert_eq!(words_for(None, at), "no window event yet");
    }

    #[test]
    fn a_step_marks_itself_and_puts_back_the_one_it_was_in() {
        let _turn = TURN.lock();
        let outer = doing("outer step");
        assert!(busy_with().starts_with("busy with outer step"));
        {
            let _inner = doing("inner step");
            assert!(busy_with().starts_with("busy with inner step"));
        }
        assert!(busy_with().starts_with("busy with outer step"));
        drop(outer);
        assert_eq!(busy_with(), format!("in {}", frame_timing::UNMARKED));
    }

    #[test]
    fn drawing_is_marked_only_outside_other_steps() {
        let _turn = TURN.lock();
        drawing(true);
        assert!(busy_with().starts_with("busy with drawing the window"));
        drawing(false);
        assert_eq!(busy_step(), None);
        let step = doing("a step");
        drawing(true);
        assert_eq!(busy_step(), Some("a step"));
        drawing(false);
        assert_eq!(busy_step(), Some("a step"));
        drop(step);
    }
}
