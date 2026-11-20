//! Making something from one set of notes (its key, its shape, its slots) so that notes which trip what makes it are
//! passed over, and never end the reading they're part of.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};

/// Makes something from one set of notes (its key, its shape, its slots), on the thread it's called on: notes kept in
/// the index (the indexer's steps after reading) or just read from a file ([`crate::store::MidiStore::save`]). Notes
/// that trip what makes it (a bug: each of these is meant to take whatever notes a file can hold) give `None`. So one
/// set of notes never ends the whole reading, which left the page "reading" until the app was started again, and
/// again after every start as the same notes came up (the 14 November 2026 review's MI-15).
///
/// Notes passed over keep what they had (an older key finder's key, no shape, no slots), and every later run asks
/// for them again: they get theirs with the build that puts the bug right, whatever it was in.
///
/// `tripped` is the step's own: whether any of its sets has tripped yet in this run. The first that does is logged as
/// the crash it is, in full (where it came from, a crash log of its own), so the bug can be found. Those after it get
/// one warning line each, counted by where they tripped: a crash in full waits for the log to be on disk and takes a
/// backtrace, and a bug here can trip on every set of notes in the index. `make` never waits for other threads' work
/// (`mo_catch::catch`'s rule).
pub(crate) fn made<T>(tripped: &AtomicBool, make: impl FnOnce() -> T) -> Option<T> {
    let make = AssertUnwindSafe(make);
    let caught = if tripped.load(Ordering::Relaxed) { mo_catch::catch(make) } else { mo_catch::catch_crash(make) };
    if caught.is_err() {
        tripped.store(true, Ordering::Relaxed);
    }
    caught.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first of a step's sets to trip is caught as a crash (logged in full); those after it as what a reader
    /// expects (a line each).
    #[test]
    fn only_the_first_to_trip_is_logged_in_full() {
        let tripped = AtomicBool::new(false);
        assert_eq!(made(&tripped, || 5), Some(5));
        assert!(!tripped.load(Ordering::Relaxed));
        let trips = || -> u8 { panic!("tripped (on purpose)") };
        assert_eq!(made(&tripped, trips), None);
        assert!(tripped.load(Ordering::Relaxed));
        assert_eq!(made(&tripped, trips), None);
        assert_eq!(made(&tripped, || 7), Some(7));
    }
}
