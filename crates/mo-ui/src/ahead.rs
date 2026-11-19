//! When each page reads its database ahead of its first showing.
//!
//! No page waits for its first visit to read what it lists: each reads once in the background a little after the
//! window opens, so the list is there when the page is opened. They start one after another (the times below, from
//! when the pages are wired up), so they don't all read from the disk in the first second, when the window is drawn
//! and Songs, the page on show, reads its own. A page opened before its turn (or Ctrl+K) reads at once: each page
//! reads once, whichever asks first.
//!
//! Songs is read before the window is made ([`crate::songs::read_ahead`]). Space and Backup aren't read ahead: they
//! look through whole drives, which may wake one that's asleep.

use std::time::Duration;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_page_has_its_own_turn_soon_after_the_start() {
        let turns = [SOUNDS, MIDI, PRESETS, PLUGINS, CURVES];
        assert!(turns.is_sorted_by(|a, b| a < b));
        assert!(turns.iter().all(|t| *t >= Duration::from_secs(1) && *t <= Duration::from_secs(3)));
    }
}
