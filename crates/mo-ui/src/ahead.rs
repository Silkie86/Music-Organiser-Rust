//! When each page reads its database ahead of its first showing.
//!
//! No page waits for its first visit to read what it lists: each reads once in the background a little after the
//! window opens ([`after`]), so the list is there when the page is opened. Their starts are half a second apart (the
//! times below, from when the pages are wired up), clear of the first second, when the window is drawn and Songs,
//! the page on show, reads its own. The reads overlap: each is of its own file, mostly on one thread. A page opened
//! before its turn (or Ctrl+K) reads at once: each page reads once, whichever asks first.
//!
//! Songs is read before the window is made ([`crate::songs::read_ahead`]). Space and Backup aren't read ahead: they
//! look through whole drives, the backup drive among them, which may be asleep. What is read ahead looks only where
//! the app looks at every start anyway: its own databases, your project folders (Songs watches them) and the
//! plugins' and Bitwig's folders. The MIDI index is brought up to this app's layout by its read, as the sound library
//! is by its own: once after an update that changes it, by copy, check, rename.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::AppWindow;
use slint::ComponentHandle;

/// The sound library (`sounds-rust.db`).
pub const SOUNDS: Duration = Duration::from_secs(1);
/// The MIDI index (`midi-index.db`).
pub const MIDI: Duration = Duration::from_millis(1500);
/// Presets' sounds: what each song's save holds, and the preset files.
pub const PRESETS: Duration = Duration::from_secs(2);
/// The plugins in the library.
pub const PLUGINS: Duration = Duration::from_millis(2500);
/// The curves: `curves.db`, then the saves and shape files changed since.
pub const CURVES: Duration = Duration::from_secs(3);

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

/// Has `read` start a page's read once `turn` has passed, on the window's thread, if the window is still there.
pub fn after(app: &AppWindow, turn: Duration, read: impl FnOnce(&AppWindow) + 'static) {
    if !on() {
        return;
    }
    let weak = app.as_weak();
    slint::Timer::single_shot(turn, move || {
        if on()
            && let Some(app) = weak.upgrade()
        {
            read(&app);
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
}
