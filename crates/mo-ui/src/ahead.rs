//! When each page reads its database ahead of its first showing.
//!
//! No page waits for its first visit to read what it lists: each reads once in the background a little after the
//! window opens ([`after`]), so the list is there when the page is opened. They read one after another, in
//! [`Page`]'s order, the first a second after the pages are wired up: clear of the first second, when the window is
//! drawn and Songs, the page on show, reads its own. Each says when its read has ended ([`done`]), which starts the
//! next. Read all at once, as they were until 24 Nov 2026, they got in each other's way (the MIDI index took 3 to 5
//! seconds to sort beside the sound library, under 1 alone) and everything they needed while reading was held at the
//! same moment; and the log couldn't say which read took how much memory. Now it has a `[Memory]` line after each.
//! A page opened before its turn (or Ctrl+K) reads at once: each page reads once, whichever asks first.
//!
//! Songs is read before the window is made ([`crate::songs::read_ahead`]). Space and Backup aren't read ahead: they
//! look through whole drives, the backup drive among them, which may be asleep. What is read ahead looks only where
//! the app looks at every start anyway: its own databases, your project folders (Songs watches them) and the
//! plugins' and Bitwig's folders. The MIDI index is brought up to this app's layout by its read, as the sound library
//! is by its own: once after an update that changes it, by copy, check, rename.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::AppWindow;
use slint::ComponentHandle;

/// What is read ahead, in the order it's read: the quickest first, and last the one whose time varies most (the
/// preset files, a quarter of a second to half a minute on Solomon's PC), so nothing waits behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The curves: `curves.db`, then the saves and shape files changed since.
    Curves,
    /// The sound library (`sounds-rust.db`).
    Sounds,
    /// The MIDI index (`midi-index.db`).
    Midi,
    /// The plugins in the library.
    Plugins,
    /// Presets' sounds: what each song's save holds, and the preset files.
    Presets,
}

const PAGES: usize = 5;

impl Page {
    fn name(self) -> &'static str {
        ["Curves", "Sounds", "MIDI", "Plugins", "Presets"][self as usize]
    }
}

/// The first read starts this long after the pages are wired up.
const FIRST: Duration = Duration::from_secs(1);
/// A read that hasn't said it ended after this long is waited for no more: the next starts beside it.
const TURN_WAIT: Duration = Duration::from_secs(10);

type Read = Box<dyn FnOnce(&AppWindow)>;

thread_local! {
    /// Each page's read, until its turn (the window's thread).
    static READS: RefCell<[Option<Read>; PAGES]> = const { RefCell::new([None, None, None, None, None]) };
    /// Whose turn it is: a place in [`Page`]'s order, `PAGES` once all have had theirs. `None` until the first
    /// begins, [`FIRST`] after the pages are wired up: a read that ends before then starts nothing.
    static TURN: Cell<Option<usize>> = const { Cell::new(None) };
    /// Which pages' reads have ended, however they were started.
    static DONE: Cell<[bool; PAGES]> = const { Cell::new([false; PAGES]) };
    /// What the app held when the read whose turn it is began.
    static HELD_BEFORE: Cell<Option<u64>> = const { Cell::new(None) };
    /// The first page has asked, so the clock for them all is running.
    static CLOCK: Cell<bool> = const { Cell::new(false) };
    /// A read ran out of its turn this run (its page was opened, Ctrl+K asked, or it was waited for no more): what
    /// the app holds can't be put down to one read any more, so no more `[Memory]` lines.
    static MIXED: Cell<bool> = const { Cell::new(false) };
}

static OFF: AtomicBool = AtomicBool::new(false);

/// Nothing is read ahead from now on: for the screenshots, whose pages are filled with made-up things, which a read
/// of a library that isn't there mustn't replace.
pub fn off() {
    OFF.store(true, Ordering::Relaxed);
}

/// Whether reading ahead is on.
fn on() -> bool {
    !OFF.load(Ordering::Relaxed)
}

/// Has `read` start `page`'s read when its turn comes, on the window's thread, if the window is still there.
pub fn after(app: &AppWindow, page: Page, read: impl FnOnce(&AppWindow) + 'static) {
    if !on() {
        return;
    }
    READS.with(|r| r.borrow_mut()[page as usize] = Some(Box::new(read)));
    // The first page to ask starts the clock for them all.
    if !CLOCK.replace(true) {
        let weak = app.as_weak();
        slint::Timer::single_shot(FIRST, move || start_turn(&weak, 0));
    }
}

/// `page`'s first read has ended, well or not, whoever asked for it: the next page's begins. Call it on the window's
/// thread. Only the first call for a page counts.
pub fn done(app: &AppWindow, page: Page) {
    let mut done = DONE.get();
    if std::mem::replace(&mut done[page as usize], true) {
        return;
    }
    DONE.set(done);
    if TURN.get() != Some(page as usize) {
        // Read before its turn, or after it was given up on. (One begun before its turn and ended in it can't be
        // told from one read in its turn: its line then counts what was read beside it.)
        MIXED.set(true);
        return;
    }
    if let Some(line) = memory_after(page, HELD_BEFORE.take(), crate::stuck::memory_now().map(|m| m.held)) {
        mo_app::log::info("Memory", &line);
    }
    start_turn(&app.as_weak(), page as usize + 1);
}

/// The log's line after a page's read ahead: what the app holds now, and how much more than before the read.
fn memory_after(page: Page, before: Option<u64>, now: Option<u64>) -> Option<String> {
    const MB: u64 = 1024 * 1024;
    let (before, now) = (before?, now?);
    let way = if now >= before { "more" } else { "less" };
    Some(format!(
        "After reading {} ahead the app holds {} MB, {} MB {way} than before it",
        page.name(),
        now / MB,
        now.abs_diff(before) / MB
    ))
}

/// Starts the read of the first page from `from` on that is still to be read, and waits for it to end ([`done`]),
/// [`TURN_WAIT`] at most.
fn start_turn(weak: &slint::Weak<AppWindow>, from: usize) {
    // A turn that was given up on, and whose read then ended, mustn't start a second chain.
    if TURN.get().is_some_and(|t| t > from) {
        return;
    }
    let Some(app) = weak.upgrade().filter(|_| on()) else { return };
    let done = DONE.get();
    let Some(turn) = (from..PAGES).find(|&p| !done[p]) else {
        TURN.set(Some(PAGES));
        return;
    };
    TURN.set(Some(turn));
    // Without a read of its own here, the page is on show and reading by itself: waited for all the same.
    let read = READS.with(|r| r.borrow_mut()[turn].take());
    if read.is_none() {
        MIXED.set(true);
    }
    HELD_BEFORE.set(if MIXED.get() { None } else { crate::stuck::memory_now().map(|m| m.held) });
    if let Some(read) = read {
        read(&app);
    }
    let weak = weak.clone();
    slint::Timer::single_shot(TURN_WAIT, move || {
        if TURN.get() == Some(turn) {
            MIXED.set(true);
            HELD_BEFORE.set(None);
            start_turn(&weak, turn + 1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_read_ahead_once_its_turned_off() {
        assert!(on());
        off();
        assert!(!on());
        // Back on for the other tests of this run.
        OFF.store(false, Ordering::Relaxed);
    }

    #[test]
    fn the_line_after_a_read_says_how_much_more_the_app_holds() {
        const MB: u64 = 1024 * 1024;
        assert_eq!(
            memory_after(Page::Sounds, Some(500 * MB), Some(1400 * MB)).as_deref(),
            Some("After reading Sounds ahead the app holds 1400 MB, 900 MB more than before it")
        );
        assert_eq!(
            memory_after(Page::Curves, Some(500 * MB), Some(480 * MB)).as_deref(),
            Some("After reading Curves ahead the app holds 480 MB, 20 MB less than before it")
        );
        // Where the app's memory isn't known (off Windows), nothing is said.
        assert_eq!(memory_after(Page::Midi, None, None), None);
    }

    #[test]
    fn the_pages_are_named_in_their_order() {
        let pages = [Page::Curves, Page::Sounds, Page::Midi, Page::Plugins, Page::Presets];
        assert_eq!(pages.len(), PAGES);
        assert!(pages.iter().enumerate().all(|(i, p)| *p as usize == i));
        assert_eq!(Page::Midi.name(), "MIDI");
    }
}
