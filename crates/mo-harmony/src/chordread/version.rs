//! Keeping the chords the app saves honest (rule 7): [`super::CHORD_READER_VERSION`] must change whenever the same
//! notes could read differently, so the MIDI page knows to read them again. Two tests make sure it does, as the key
//! finder's do:
//!
//! - a hash over every number the reader is built from ([`numbers_hash`]): `weights.rs` keeps the hash it was written
//!   with and the version those numbers were first given with, and the test fails when the hash is out of date or
//!   that version is newer than [`super::CHORD_READER_VERSION`];
//! - a hash of what the hand cases and the tests' music read, kept for each version, so a change to the code that
//!   moves a reading (a chord, a name, a chance, an answer) fails too.

use super::decode::{KEEP, SHARPNESS, SPAN_BARS};
use super::hear::{BASS_GAP, MOST_BAR_BEATS, MOST_UNITS};
use super::reader::{MOST_READINGS, Model};
use super::weights;
use crate::fnv::Fnv;

/// A hash over every number the reader is built from: the model's (the weights, the counted tables, the thresholds
/// the answers are graded by) and the named ones set by hand (the sharpness, the least share a prior reads, the
/// readings kept, the longest span, the most units heard, the longest bar, the bass's gap, the most readings shown).
/// Numbers written into the code are covered by the hash of what the music reads. Left out are the two that only
/// choose how the decode adds its sums up (in masses or in logarithms): they move a chance in its last bits at most.
pub fn numbers_hash(model: &Model) -> u64 {
    let mut h = Fnv::new();
    let w = model.weights;
    for x in w.features.0.iter().chain([&w.pair, &w.none]) {
        h.real(*x);
    }
    let t = model.tables;
    for x in &t.core {
        h.real(*x);
    }
    for x in t.tension.iter().flatten() {
        h.real(*x);
    }
    for x in t.motion.iter().flatten().flatten() {
        h.real(*x);
    }
    let thresholds = model.thresholds;
    for x in thresholds.one.iter().chain(&thresholds.same) {
        h.real(*x);
    }
    h.real(SHARPNESS);
    h.real(weights::FLOOR);
    for n in [KEEP, SPAN_BARS, MOST_UNITS, usize::from(BASS_GAP), usize::from(MOST_READINGS)] {
        h.number(i128::try_from(n).unwrap_or(-1));
    }
    h.number(i128::from(MOST_BAR_BEATS));
    h.finish()
}

/// The hash `weights.rs` was written with, and the version of the reader its numbers were first given with.
pub const SHIPPED_HASH: (u64, u32) = (weights::SHIPPED_HASH, weights::HASH_VERSION);
