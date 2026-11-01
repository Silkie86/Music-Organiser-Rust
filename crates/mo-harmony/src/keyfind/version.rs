//! Keeping the answers the app saves honest (rule 7): [`super::KEY_FINDER_VERSION`] must change whenever the same
//! notes could get a different answer, so the MIDI page knows to read them again. Two tests make sure it does:
//!
//! - a hash over every number the finder is built from ([`numbers_hash`]): `weights.rs` keeps the hash it was written
//!   with and the version those numbers were first given with, and the test fails when the hash is out of date or
//!   that version is newer than [`super::KEY_FINDER_VERSION`];
//! - a hash of the hand cases' readings, with the shipped model and with one whose weights have no pattern, kept for
//!   each version, so a change to the code that moves an answer fails too.

use super::colour::{COLOUR_NOTE, I7_SHARE, LEADING_NOTE, NO_SIXTH_BELOW};
use super::{LEAST_SHARPNESS, MIN_NOTES, Model, heard, weights};
use crate::fnv::Fnv;

/// A hash over every number the finder is built from: the model's (weights, calibration, thresholds, the share of
/// notes in one scale a key needs) and those set by hand (the colour rules, the notes an answer needs, how the notes
/// are heard). `theorybench keyfit --write` writes it into `weights.rs` with the weights.
pub fn numbers_hash(model: &Model) -> u64 {
    let mut h = Fnv::new();
    for w in model.weights {
        h.real(*w);
    }
    for c in model.calibration.0 {
        h.real(c);
    }
    let t = model.thresholds;
    for x in t.one.iter().chain(&t.pair).chain(&t.home) {
        h.real(*x);
    }
    h.real(model.not_tonal_below);
    for (num, den) in [COLOUR_NOTE, LEADING_NOTE, NO_SIXTH_BELOW, I7_SHARE] {
        h.bytes(&num.to_le_bytes());
        h.bytes(&den.to_le_bytes());
    }
    h.number(i128::from(MIN_NOTES));
    h.real(LEAST_SHARPNESS);
    for n in heard::NUMBERS {
        h.number(n);
    }
    h.finish()
}

/// The hash `weights.rs` was written with, and the version of the finder its numbers were first given with.
/// `theorybench keyfit --write` keeps the version when the hash is the same, and writes the next one when it isn't.
pub const SHIPPED_HASH: (u64, u32) = (weights::SHIPPED_HASH, weights::HASH_VERSION);
