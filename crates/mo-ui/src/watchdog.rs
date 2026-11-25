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
//! (looked at once it has been stuck for [`LOOK_AT_THREAD`], and again at [`LOOK_AGAIN`] and [`STILL_STUCK`], see
//! [`crate::stuck`]), how
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
/// Stuck this long, the window's thread is looked at to see where it is: just past [`STALL`], so every freeze that
/// gets a line also says which functions it was in. (It was a whole second, and the freezes of 0.3 to 0.9 s in
/// Solomon's logs, such as 0.4 s opening the MIDI page on 11 Nov, said only "in no marked step".) The look stops the
/// window's thread only while its stack is copied; the names are worked out on the watchdog's thread.
pub const LOOK_AT_THREAD: Duration = Duration::from_millis(300);
/// Still stuck this long, it is looked at once more: one look at 0.3 s couldn't say whether a freeze of two seconds
/// spent them in the same place (the 2.6 s on Space on 15 Nov 2026 showed a single look, into the graphics code).
pub const LOOK_AGAIN: Duration = Duration::from_secs(1);
const KNOCK_EVERY: Duration = Duration::from_millis(200);
const LOOK_EVERY: Duration = Duration::from_millis(25);

/// The marked step the window is in, and since when.
static DOING: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);

/// The window's thread, kept by [`start`]. Only steps on it are marked: a background thread's long wait isn't the
/// window being busy.
static WINDOW: OnceLock<ThreadId> = OnceLock::new();

/// The window's last move, resize, focus change or hiding, and when (see [`window_event`]).
static LAST_WINDOW_EVENT: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);
/// How many [`pause`]s have begun, so a knock that waited across one isn't put down as a freeze; and how many are
/// going on now.
static PAUSES: AtomicU64 = AtomicU64::new(0);
static PAUSED: AtomicU64 = AtomicU64::new(0);
/// Set by [`stop`]: the event loop has ended, the app is closing.
static STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// The window's thread is held on purpose until the guard is dropped: a drag out of the app, which Windows runs on
/// the window's thread until the files are let go. The watchdog doesn't count that wait as a freeze.
pub fn pause() -> Paused {
    PAUSES.fetch_add(1, Ordering::SeqCst);
    PAUSED.fetch_add(1, Ordering::SeqCst);
    Paused(())
}

/// A [`pause`] going on.
pub struct Paused(());

impl Drop for Paused {
    fn drop(&mut self) {
        PAUSED.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How the [`pause`]s stood when a knock was sent.
#[derive(Clone, Copy, Debug)]
struct PausesAt {
    /// How many had begun.
    begun: u64,
    /// Whether one was going on.
    going: bool,
}

/// How the pauses stand now, for a knock about to be sent.
fn pauses_now() -> PausesAt {
    // In the order `pause` counts them, so one beginning between the two looks is seen by `paused_since` later.
    let begun = PAUSES.load(Ordering::SeqCst);
    PausesAt { begun, going: PAUSED.load(Ordering::SeqCst) > 0 }
}

/// Whether a knock sent when the pauses stood at `sent` waited through any of one: sent while one was going on (a
/// knock can be sent in the middle of a drag, and answered only once the drag has ended), one begun since, or one
/// going on now.
fn paused_since(sent: PausesAt) -> bool {
    sent.going || PAUSED.load(Ordering::SeqCst) > 0 || PAUSES.load(Ordering::SeqCst) != sent.begun
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
/// Whether the window is the one in front goes to the pages too (`AppKeys.window-active`, before Slint hears of it):
/// a text box that loses the keys to another app's window isn't one whose keys went elsewhere in this one (the
/// code review of the 10 Nov review's U-1 fix). Nothing else is changed: every event goes on to Slint as before.
///
/// Slint keeps one such watcher a window, so this one is the app's: anything else that needs the window's own events
/// must be called from here, not set up beside it.
pub fn watch_window(app: &crate::AppWindow) {
    use slint::ComponentHandle;
    use slint::winit_030::winit::event::WindowEvent;
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    let mut minimised = false;
    let mut hands = Hands::default();
    let weak = app.as_weak();
    app.window().on_winit_window_event(move |_, event| {
        hands.see(event);
        if let WindowEvent::Focused(active) = event
            && let Some(app) = weak.upgrade()
        {
            app.global::<crate::AppKeys>().set_window_active(*active);
            if *active {
                // A curve kept from the plugin's editor while the app sat behind the DAW is listed now.
                crate::presets::came_to_front(&app);
            }
        }
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
        if what == "restored"
            && let Some(app) = weak.upgrade()
        {
            crate::settings::restored(&app);
        }
        // A screen with another scale: the size picked in Settings goes back on at once, once Slint has taken the new
        // scale, rather than up to a second later by the settings' check (the window looked to jump twice).
        // One timer, started again by each change, so a burst of changes puts the size back once.
        if what == "scale changed" {
            let weak = weak.clone();
            RESCALE.with(|t| {
                t.start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                    if let Some(app) = weak.upgrade() {
                        crate::settings::apply_size(&app);
                    }
                });
            });
        }
        if what == "resized"
            && let Some(app) = weak.upgrade()
        {
            crate::settings::resized(&app, hands.on_the_frame());
        }
        EventResult::Propagate
    });
}

thread_local! {
    /// Puts the size picked in Settings back on after the screen's scale changed (see [`watch_window`]).
    static RESCALE: slint::Timer = slint::Timer::default();
}

/// What the user's hands were last doing with the window, to tell their own Restore from one the app didn't ask for
/// (Solomon's log of 24 Nov: the window left maximised on its own 10 s after a restore, with no page change). Windows
/// gives the app no word of a click on its title bar, so it goes by what it can see: the pointer is off the window's
/// inside (on the title bar or an edge, which is where Restore, a double-click and a drag happen) while the window is
/// in front, or the Windows key or Alt+Space was pressed a moment ago (Win+Down, the window's own menu).
#[derive(Default)]
struct Hands {
    /// Where the pointer is: `None` until it is seen after the window came to the front (brought back from the
    /// taskbar, the pointer stays on the taskbar, which says nothing of the user's hands on the window).
    pointer_inside: Option<bool>,
    in_front: bool,
    size: (f64, f64),
    alt_held: bool,
    key_at: Option<Instant>,
}

impl Hands {
    fn see(&mut self, event: &slint::winit_030::winit::event::WindowEvent) {
        use slint::winit_030::winit::event::{ElementState, WindowEvent};
        use slint::winit_030::winit::keyboard::{Key, NamedKey};
        match event {
            WindowEvent::Resized(size) => self.size = (f64::from(size.width), f64::from(size.height)),
            WindowEvent::CursorEntered { .. } => self.pointer_inside = Some(true),
            // While a drag holds the pointer, it moves on outside the window: only a place inside counts.
            WindowEvent::CursorMoved { position, .. } => {
                let inside = (0.0..self.size.0).contains(&position.x) && (0.0..self.size.1).contains(&position.y);
                self.pointer_inside = Some(inside);
            }
            WindowEvent::CursorLeft { .. } => self.pointer_inside = Some(false),
            WindowEvent::Focused(active) => {
                self.in_front = *active;
                self.pointer_inside = None;
                self.alt_held = false;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                match &event.logical_key {
                    Key::Named(NamedKey::Alt) => self.alt_held = pressed,
                    // Win+Down and the like, and Alt+Space (the window's own menu: Restore, Size, Move).
                    Key::Named(NamedKey::Super) if pressed => self.key_at = Some(Instant::now()),
                    Key::Named(NamedKey::Space) if pressed && self.alt_held => self.key_at = Some(Instant::now()),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Whether the user may have changed the window's size themselves just now.
    fn on_the_frame(&self) -> bool {
        let key = self.key_at.is_some_and(|at| at.elapsed() < Duration::from_secs(5));
        key || (self.in_front && self.pointer_inside == Some(false))
    }
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
/// `run`), so the time the window takes to open isn't counted as a freeze. Stops when the window is asked to close ([`stop`]).
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

/// The event loop has ended and the app is closing: the watchdog stops, and a knock still waiting isn't a freeze. The
/// window's thread goes on to write what's waiting (up to a minute, for a plugin move), and without this each close
/// that took over five seconds was logged as "still stuck", an error line for nothing wrong.
pub fn stop() {
    STOPPED.store(true, Ordering::Release);
}

fn watch(feed: &Feed) {
    // What the app held at its last `[Memory]` line, and when.
    let mut held: Option<(u64, Instant)> = None;
    loop {
        std::thread::sleep(KNOCK_EVERY);
        if STOPPED.load(Ordering::Acquire) {
            return;
        }
        // How long the knock waited, in microseconds (at least 1), once it's answered; 0 until then.
        let answered = Arc::new(AtomicU64::new(0));
        let a = answered.clone();
        let memory_before = crate::stuck::memory_now();
        let work_before = crate::stuck::window_thread_work();
        // Before the knock is timed, so writing the line isn't counted as the window answering late; the page and
        // the jobs are only asked for when there's a line to write.
        if let Some(now) = memory_before {
            let (last, since) = held.unwrap_or((0, Instant::now()));
            if now.held.abs_diff(last) >= MEMORY_STEP
                && let Some(words) = memory_line(last, since.elapsed(), now, crate::page_name(), &feed.running_titles())
            {
                log::info("Memory", &words);
                held = Some((now.held, Instant::now()));
            }
        }
        let sent = Instant::now();
        let pauses = pauses_now();
        let answer = move || {
            let waited = u64::try_from(sent.elapsed().as_micros()).unwrap_or(u64::MAX).max(1);
            a.store(waited, Ordering::Release);
        };
        if slint::invoke_from_event_loop(answer).is_err() {
            return; // The event loop has ended.
        }
        let mut said_still_stuck = false;
        let mut looked_at_thread = false;
        let mut looked_again = false;
        // How long the watchdog itself was held up while waiting (its short sleeps that came back late).
        let mut held_up = Duration::ZERO;
        // The page faults of the watchdog's own looks at the window's thread (reading the app's symbols), left out
        // of the freeze's count.
        let mut own_faults = 0;
        // What the window was busy with once it had been stuck for STALL: the step the freeze is in.
        let mut busy = None;
        // The step it was in at each look, for a late answer (answered before the first, it wasn't late). Kept to the
        // first few hundred looks: a longer wait is a freeze, which has its own line.
        let mut seen: Vec<Option<&'static str>> = Vec::new();
        while answered.load(Ordering::Acquire) == 0 {
            let slept = Instant::now();
            std::thread::sleep(LOOK_EVERY);
            held_up += late(slept.elapsed());
            // Answered while this look slept: what the window does now came after the answer, so it isn't looked at.
            if answered.load(Ordering::Acquire) != 0 {
                break;
            }
            if STOPPED.load(Ordering::Acquire) {
                return;
            }
            let waited = sent.elapsed();
            if seen.len() < 400 {
                seen.push(busy_step());
            }
            if waited >= STALL && busy.is_none() {
                busy = Some(busy_with());
            }
            if waited >= LOOK_AT_THREAD && !looked_at_thread && !paused_since(pauses) {
                looked_at_thread = true;
                own_faults += log_thread(waited);
                log::current().inspect(|l| l.flush());
            }
            // (Not in the same turn as the first look: a watchdog that was itself held up for a second would look
            // twice at one moment.)
            else if waited >= LOOK_AGAIN && !looked_again && !paused_since(pauses) {
                looked_again = true;
                own_faults += log_thread(waited);
                log::current().inspect(|l| l.flush());
            }
            if waited >= STILL_STUCK && !said_still_stuck && !paused_since(pauses) {
                said_still_stuck = true;
                let now = format!("{}; {}", busy_with(), window_words(sent));
                log::error("Freeze", &line(waited, true, crate::page_name(), &feed.running_titles(), &now));
                log::current().inspect(|l| l.flush());
                own_faults += log_thread(waited);
                log::current().inspect(|l| l.flush());
            }
        }
        let waited = Duration::from_micros(answered.load(Ordering::Acquire));
        // Held on purpose meanwhile (a drag out): not a freeze, and not a slow frame either.
        if paused_since(pauses) {
            continue;
        }
        frame_timing::late(waited, blame(&seen), crate::page_name());
        if waited >= STALL {
            let mut busy = busy.unwrap_or_else(busy_with);
            busy.push_str("; ");
            busy.push_str(&window_words(sent));
            if let Some(memory) = crate::stuck::memory_words(memory_before, crate::stuck::memory_now(), own_faults) {
                busy.push_str("; ");
                busy.push_str(&memory);
            }
            if let Some(work) = crate::stuck::work_words(work_before, crate::stuck::window_thread_work(), waited) {
                busy.push_str("; ");
                busy.push_str(&work);
            }
            if let Some(whole) = held_up_words(held_up) {
                busy.push_str("; ");
                busy.push_str(&whole);
            }
            log::warning("Freeze", &line(waited, false, crate::page_name(), &feed.running_titles(), &busy));
        }
    }
}

/// The app's memory gets a `[Memory]` line each time what it holds has changed by this much since the last one.
const MEMORY_STEP: u64 = 100 * 1024 * 1024;

/// A `[Memory]` line when what the app holds has moved [`MEMORY_STEP`] or more from `last` (what the line before
/// said, `since` ago; 0 at the start): how much, on which page and with which jobs running, so the log shows what
/// made it grow. The app held 1,700 MB on 17 Nov 2026 with nothing in the log to say what for; when Windows then
/// needed the room for Bitwig, reading it all back from disk froze the window for up to 23 s.
fn memory_line(
    last: u64,
    since: Duration,
    now: crate::stuck::Memory,
    page: &str,
    running: &[String],
) -> Option<String> {
    const MB: u64 = 1024 * 1024;
    if now.held.abs_diff(last) < MEMORY_STEP {
        return None;
    }
    let change = if last == 0 {
        String::new()
    } else {
        let way = if now.held > last { "more" } else { "less" };
        format!(", {} MB {way} than {:.0} s ago", now.held.abs_diff(last) / MB, since.as_secs_f64())
    };
    let jobs =
        if running.is_empty() { "no jobs running".to_owned() } else { format!("running: {}", running.join(", ")) };
    Some(format!(
        "The app holds {} MB{change} (page {page}; {jobs}; {} MB of it in RAM)",
        now.held / MB,
        now.in_ram / MB
    ))
}

/// How much of a sleep of [`LOOK_EVERY`] that took `slept` was the watchdog being held up itself: nothing for the
/// usual few milliseconds over, the rest of it once that's past [`STALL`].
fn late(slept: Duration) -> Duration {
    let over = slept.saturating_sub(LOOK_EVERY);
    if over >= STALL { over } else { Duration::ZERO }
}

/// For a freeze line, that the watchdog was held up too, when it was: then it wasn't the window's thread being busy
/// but the whole app standing still (the computer asleep or out of memory, the app's memory on disk). On 17 Nov 2026
/// a 23 s freeze got its first look only after 19.4 s, because the watchdog hadn't run either.
fn held_up_words(held_up: Duration) -> Option<String> {
    (held_up >= STALL).then(|| {
        format!(
            "the watchdog itself was held up for {:.1} s of it, so the whole app stood still, not only the window",
            held_up.as_secs_f64()
        )
    })
}

/// Where the window's thread is, stuck for `waited`, as a `[Freeze]` line of its own (nothing where it can't be seen).
/// Gives back the page faults the look itself made.
fn log_thread(waited: Duration) -> u64 {
    let before = crate::stuck::memory_now();
    if let Some(look) = crate::stuck::window_thread_now() {
        log::warning("Freeze", &thread_line(waited, look.to_stop, &look.frames));
    }
    match (before, crate::stuck::memory_now()) {
        (Some(b), Some(a)) => a.page_faults.saturating_sub(b.page_faults),
        _ => 0,
    }
}

/// A thread that takes this long to stop for a look was somewhere it couldn't be stopped, and the line says so.
const SLOW_TO_STOP: Duration = Duration::from_millis(100);

/// "Stuck for 1.0 s, the window's thread was in:" and its functions, innermost first, one a line. `waited` is how
/// long it had been stuck when the look was asked for, `to_stop` how long the thread then took to stop: when that was
/// long, the line gives the moment it did stop and says what the wait means, since the functions are then where the
/// thread came out to, not where it had been all along.
pub fn thread_line(waited: Duration, to_stop: Duration, frames: &[String]) -> String {
    let mut text = if to_stop >= SLOW_TO_STOP {
        format!(
            "Stuck for {:.1} s, the window's thread was in (the look was asked for at {:.1} s and the thread took {} ms \
             to stop, as it does inside a part of Windows that can't be interrupted; these are the functions it came \
             out to):",
            (waited + to_stop).as_secs_f32(),
            waited.as_secs_f32(),
            to_stop.as_millis()
        )
    } else {
        format!("Stuck for {:.1} s, the window's thread was in:", waited.as_secs_f32())
    };
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
    fn a_knock_that_waited_through_any_of_a_pause_is_not_a_freeze() {
        let _turn = TURN.lock();
        // No pause: a late answer counts.
        let quiet = pauses_now();
        assert!(!paused_since(quiet));
        // Sent before a drag began and answered after it ended.
        let before = pauses_now();
        let drag = pause();
        assert!(paused_since(before), "still going on");
        // Sent in the middle of the drag (the window answers some knocks while Windows runs the drag)...
        let during = pauses_now();
        drop(drag);
        assert!(paused_since(before), "one began since");
        // ...and answered only once it had ended: no pause is going on and none has begun since, but it waited
        // through the drag's end all the same. This one was logged as a freeze of a second or more after each drag.
        assert!(paused_since(during), "sent while one was going on");
        // The next knock, sent after the drag, counts again.
        assert!(!paused_since(pauses_now()));
    }

    #[test]
    fn a_freeze_long_enough_for_a_line_is_looked_at_before_it_ends() {
        // Past the stall (a shorter wait is no freeze), and within two looks of it, so a freeze only a little over
        // the stall is still caught in the act.
        assert!(LOOK_AT_THREAD > STALL);
        assert!(LOOK_AT_THREAD <= STALL + 2 * LOOK_EVERY);
        assert!(LOOK_AT_THREAD < STILL_STUCK);
        // And once more between the two, so a freeze of a second or two has two looks to compare.
        assert!(LOOK_AT_THREAD < LOOK_AGAIN && LOOK_AGAIN < STILL_STUCK);
        assert_eq!(
            thread_line(LOOK_AT_THREAD, Duration::ZERO, &["a".to_owned()]),
            "Stuck for 0.3 s, the window's thread was in:\n    a"
        );
    }

    #[test]
    fn memory_gets_a_line_each_time_it_has_moved_a_hundred_megabytes() {
        const MB: u64 = 1024 * 1024;
        let at = |held: u64, in_ram: u64| crate::stuck::Memory { page_faults: 0, in_ram: in_ram * MB, held: held * MB };
        let jobs = ["Reading sounds".to_owned()];
        assert_eq!(
            memory_line(0, Duration::ZERO, at(420, 400), "Songs", &[]).as_deref(),
            Some("The app holds 420 MB (page Songs; no jobs running; 400 MB of it in RAM)")
        );
        assert_eq!(memory_line(420 * MB, Duration::from_secs(3), at(500, 480), "Songs", &[]), None);
        assert_eq!(
            memory_line(420 * MB, Duration::from_secs(12), at(1_060, 1_000), "Sounds", &jobs).as_deref(),
            Some(
                "The app holds 1060 MB, 640 MB more than 12 s ago (page Sounds; running: Reading sounds; 1000 MB of it in RAM)"
            )
        );
        assert_eq!(
            memory_line(1_060 * MB, Duration::from_secs(90), at(700, 49), "Space", &[]).as_deref(),
            Some("The app holds 700 MB, 360 MB less than 90 s ago (page Space; no jobs running; 49 MB of it in RAM)")
        );
        // Nothing known, nothing said.
        assert_eq!(memory_line(0, Duration::ZERO, at(0, 0), "Songs", &[]), None);
    }

    #[test]
    fn a_watchdog_held_up_itself_says_the_whole_app_stood_still() {
        // A sleep a few milliseconds long is the usual; one that comes back 19 s late is the app not running.
        assert_eq!(late(LOOK_EVERY + Duration::from_millis(15)), Duration::ZERO);
        assert_eq!(late(LOOK_EVERY + Duration::from_secs(19)), Duration::from_secs(19));
        assert_eq!(held_up_words(Duration::ZERO), None);
        assert_eq!(
            held_up_words(Duration::from_millis(19_400)).as_deref(),
            Some("the watchdog itself was held up for 19.4 s of it, so the whole app stood still, not only the window")
        );
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
            thread_line(
                Duration::from_millis(1040),
                Duration::from_millis(2),
                &["inner (a.rs:3)".to_owned(), "outer".to_owned()]
            ),
            "Stuck for 1.0 s, the window's thread was in:\n    inner (a.rs:3)\n    outer"
        );
        // Solomon's 25 Nov: asked for at 0.3 s, stopped 3.8 s later.
        assert_eq!(
            thread_line(Duration::from_millis(310), Duration::from_millis(3_774), &["outer".to_owned()]),
            "Stuck for 4.1 s, the window's thread was in (the look was asked for at 0.3 s and the thread took 3774 ms \
             to stop, as it does inside a part of Windows that can't be interrupted; these are the functions it came \
             out to):\n    outer"
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
