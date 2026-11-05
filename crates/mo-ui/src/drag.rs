//! Dragging sounds, clips and tracks out of the app into a DAW or a folder: the Sounds page's tiles, lines and pads,
//! and the clips and tracks of a song's arrangement. mo-app's `drag_out` says what a drag carries and `drag_card`
//! draws the card that follows the mouse; this runs the drag itself, the same way for every page:
//!
//! 1. The page sees the left button go down on something and the mouse move a few points, calls [`start`] with how to
//!    make what's carried, and dims the thing where it is ("lifted").
//! 2. That's made on a thread of its own: a sound's file is looked for, a clip's MIDI file is written to Temp.
//! 3. Once a frame with the thing dimmed is on screen, the files go to the system's drag and drop. On Windows that
//!    holds the window's thread until they're let go, so the card follows the mouse from a thread of its own, over every
//!    app, and the freeze watchdog knows the wait is on purpose.
//! 4. Once they're let go, the card lands with a pulse where they went (or goes away), the page is told how it went
//!    (it brings the thing back, with a pulse of its own when it landed), and the window hears the mouse is up again.
//!
//! Windows first. Elsewhere a drag says it isn't there yet: a Mac's and Linux's own drag and drop go in beside the
//! Windows one, taking the same files and the same card.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use mo_app::activity::Feed;
use mo_app::drag_out::Payload;
use mo_app::log;
use slint::ComponentHandle;

use crate::AppWindow;

/// Windows' own drag and drop, which needs `unsafe` calls (see the top of that file).
#[cfg(windows)]
#[allow(unsafe_code)]
mod ole;

/// How a drag went, for the page that started it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Let go somewhere that took the files.
    Landed,
    /// Let go where nothing took them, or called off (Esc, or let go before the drag began).
    Cancelled,
    /// It couldn't go: nothing to carry, or the system's drag wouldn't start. The user has been told why.
    Refused,
}

/// A drag a page starts.
pub struct Drag {
    /// Makes what's carried, on a thread of its own. An error is what to tell the user instead.
    pub make: Make,
    /// Back on the window's thread once it's over, however it went: the page brings the thing back.
    pub done: Done,
}

/// How a page makes what a drag carries (see [`Drag::make`]).
pub type Make = Box<dyn FnOnce() -> Result<Payload, String> + Send>;
/// What a page does once its drag is over (see [`Drag::done`]).
pub type Done = Box<dyn FnOnce(&AppWindow, Outcome)>;

/// A drag between [`start`] and the system's drag beginning.
struct Waiting {
    id: u64,
    done: Done,
    /// The frames drawn when it started ([`crate::gpu::frames_drawn`]): any frame after that shows its thing dimmed.
    frames: u64,
}

/// The longest a drag waits for the frame that shows its thing dimmed, in case no frame comes (nothing else changed).
const LIFT_WAIT: Duration = Duration::from_millis(80);

thread_local! {
    static WAITING: RefCell<Option<Waiting>> = const { RefCell::new(None) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
    /// The system's drag is under way (its own events can come in meanwhile).
    static CARRYING: Cell<bool> = const { Cell::new(false) };
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
}

/// Where a drag that can't go says why.
pub fn connect(feed: Feed) {
    FEED.with(|f| *f.borrow_mut() = Some(feed));
}

/// How long after start the sweep of older drags' files waits, so it never competes with the first page for the disk.
const SWEEP_WAIT: Duration = Duration::from_secs(20);

/// Clears, on a low-priority thread a little after start, the MIDI files that drags left in Temp a day or more ago
/// (rule 28: the app cleans up after itself; mo-app's `drag_out::sweep`). Only on Windows, the one system that drags
/// out of the app so far: Windows' Temp folder is the user's own. A Mac's or Linux's (often `/tmp`, shared by every
/// account) is looked at once their drag goes in, which writes its files in a folder of the user's own.
pub fn sweep_later() {
    if !cfg!(windows) {
        return;
    }
    let started = mo_app::background::spawn("Drag files", || {
        std::thread::sleep(SWEEP_WAIT);
        say_swept(mo_app::drag_out::sweep(&std::env::temp_dir(), std::time::SystemTime::now()));
    });
    if let Err(e) = started {
        log::warning("Drag", &format!("Couldn't start clearing older drags' files: {e}"));
    }
}

/// As the app closes: clears this run's drags' MIDI files that are a minute old or more. Quick: it looks only at the
/// folders this run made, a few small files.
pub fn finish_up() {
    if !cfg!(windows) {
        return;
    }
    say_swept(mo_app::drag_out::finish_up(&std::env::temp_dir(), std::time::SystemTime::now()));
}

fn say_swept(done: mo_app::drag_out::Swept) {
    if done.removed > 0 {
        let files = mo_app::drag_out::count_of(done.removed, "file", "files");
        log::info(
            "Drag",
            &format!("Cleared {files} that drags had left in Temp ({})", mo_space::size_text(done.bytes)),
        );
    }
    if done.failed > 0 {
        log::warning("Drag", &format!("{} of the drags' files in Temp couldn't be cleared (in use?)", done.failed));
    }
}

/// Starts a drag (see the top of this file); `drag.done` is always called once. True when it's under way: the page
/// then dims its thing (after this, as a drag still being made is called off here first, and brings its own back).
/// False when it was over at once, and `done` has been called already.
pub fn start(app: &AppWindow, drag: Drag) -> bool {
    if CARRYING.with(Cell::get) {
        (drag.done)(app, Outcome::Cancelled);
        return false;
    }
    // One still being made (a press on something else meanwhile) is over.
    if let Some(old) = WAITING.with(|w| w.borrow_mut().take()) {
        (old.done)(app, Outcome::Cancelled);
    }
    let id = NEXT.with(|n| n.replace(n.get() + 1));
    WAITING.with(|w| *w.borrow_mut() = Some(Waiting { id, done: drag.done, frames: crate::gpu::frames_drawn() }));
    let weak = app.as_weak();
    let make = drag.make;
    let started = mo_app::background::spawn("Drag out", move || {
        // A bug in the making is logged as a crash and ends the drag, rather than leaving it waiting for ever.
        let made = mo_catch::catch_crash(std::panic::AssertUnwindSafe(make))
            .unwrap_or_else(|_| Err("Something went wrong making what's dragged.".to_owned()));
        let _ = weak.upgrade_in_event_loop(move |app| made_ready(&app, id, made));
    });
    if let Err(e) = started {
        log::warning("Drag", &format!("Couldn't start making what's dragged: {e}"));
        tell("The app couldn't start the drag. Try again in a moment.");
        finish(app, id, Outcome::Refused);
        return false;
    }
    true
}

/// A drag is being made or carried: the page leaves what's under the mouse alone until it's over.
pub fn busy() -> bool {
    CARRYING.with(Cell::get) || WAITING.with(|w| w.borrow().is_some())
}

/// The mouse came up before the system's drag began: the drag is off.
pub fn let_go(app: &AppWindow) {
    if let Some(w) = WAITING.with(|w| w.borrow_mut().take()) {
        (w.done)(app, Outcome::Cancelled);
    }
}

fn waiting_for(id: u64) -> bool {
    WAITING.with(|w| w.borrow().as_ref().is_some_and(|w| w.id == id))
}

fn made_ready(app: &AppWindow, id: u64, made: Result<Payload, String>) {
    if !waiting_for(id) {
        return;
    }
    match made {
        Err(why) => {
            tell(&why);
            finish(app, id, Outcome::Refused);
        }
        // Once the thing is on screen dimmed: at once when a frame has been drawn since the drag started (the page
        // dimmed it straight after), else after the next one.
        Ok(payload) if WAITING.with(|w| w.borrow().as_ref().is_some_and(|w| crate::gpu::frames_drawn() > w.frames)) => {
            carry(app, id, payload);
        }
        Ok(payload) => {
            let weak = app.as_weak();
            crate::gpu::after_next_frame(LIFT_WAIT, move || {
                if let Some(app) = weak.upgrade() {
                    carry(&app, id, payload);
                }
            });
        }
    }
}

/// Hands the files to the system's drag and drop and waits until they're let go.
fn carry(app: &AppWindow, id: u64, payload: Payload) {
    // Taken first: from here on a letting go the page hears is this drag's end, which says how it went itself.
    let Some(waiting) = take(id) else { return };
    CARRYING.with(|c| c.set(true));
    let went = {
        // The window's thread is held on purpose until they're let go: not a freeze.
        let _paused = crate::watchdog::pause();
        hand_over(app, &payload)
    };
    CARRYING.with(|c| c.set(false));
    // The system kept the mouse's letting go for itself: tell the window the mouse has gone, so what was pressed
    // isn't held any more (and no click comes of it).
    app.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
    let outcome = match went {
        Ok(o) => o,
        Err(why) => {
            log::warning("Drag", &why);
            tell(&why);
            Outcome::Refused
        }
    };
    if outcome == Outcome::Landed {
        log::info("Drag", &format!("Dragged out: {}", payload.caption));
    }
    (waiting.done)(app, outcome);
}

/// The drag waiting with this number, taken out of waiting.
fn take(id: u64) -> Option<Waiting> {
    WAITING.with(|w| {
        let mut w = w.borrow_mut();
        if w.as_ref().is_some_and(|w| w.id == id) { w.take() } else { None }
    })
}

fn finish(app: &AppWindow, id: u64, outcome: Outcome) {
    if let Some(w) = take(id) {
        (w.done)(app, outcome);
    }
}

/// How long a thing that landed stays lit before its glow fades (the page's glow fades in and out by itself).
const GLOW: Duration = Duration::from_millis(120);

/// Shows that something landed: `glow(true)` now and `glow(false)` a moment later.
pub fn glow(app: &AppWindow, glow: impl Fn(&AppWindow, bool) + 'static) {
    glow(app, true);
    let weak = app.as_weak();
    slint::Timer::single_shot(GLOW, move || {
        if let Some(app) = weak.upgrade() {
            glow(&app, false);
        }
    });
}

fn tell(why: &str) {
    FEED.with(|f| {
        if let Some(feed) = f.borrow().as_ref() {
            feed.start("Couldn't drag it out", "").failed(why);
        }
    });
}

#[cfg(windows)]
fn hand_over(app: &AppWindow, payload: &Payload) -> Result<Outcome, String> {
    let look = crate::look();
    let flight = mo_app::drag_card::Flight::new(payload, &look, card_fonts(look.theme.font), std::time::Instant::now());
    ole::drag(app.window(), &payload.files, flight)
}

#[cfg(not(windows))]
fn hand_over(_app: &AppWindow, _payload: &Payload) -> Result<Outcome, String> {
    Err("Dragging out of the app works on Windows for now.".to_owned())
}

/// The theme's text family for the card, at the weights the app's lines use: 400 for the facts, 600 for the names.
pub fn card_fonts(font: mo_app::themes::ThemeFont) -> mo_app::drag_card::Fonts {
    use mo_app::drag_card::Fonts;
    use mo_app::themes::ThemeFont;
    match font {
        ThemeFont::Plex => Fonts {
            regular: include_bytes!("../fonts/IBMPlexSans-400.ttf"),
            bold: include_bytes!("../fonts/IBMPlexSans-600.ttf"),
        },
        ThemeFont::Space => Fonts {
            regular: include_bytes!("../fonts/SpaceGrotesk-400.ttf"),
            bold: include_bytes!("../fonts/SpaceGrotesk-600.ttf"),
        },
        ThemeFont::Mono => Fonts {
            regular: include_bytes!("../fonts/IBMPlexMono-400.ttf"),
            bold: include_bytes!("../fonts/IBMPlexMono-600.ttf"),
        },
    }
}
