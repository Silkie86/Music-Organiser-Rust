//! Keeping the answers the app saves honest (rule 7): [`super::KEY_FINDER_VERSION`] must change whenever the same
//! notes could get a different answer, so the MIDI page knows to read them again. Two tests make sure it does:
//!
//! - a hash over every number the finder is built from ([`numbers_hash`]): `weights.rs` keeps the hash it was written
//!   with and the version it belongs to, and the test fails when either is out of date;
//! - a hash of the hand cases' readings, kept for each version, so a change to the code that moves an answer fails
//!   too.

use super::colour::{COLOUR_NOTE, I7_SHARE, LEADING_NOTE, NO_SIXTH_BELOW};
use super::{LEAST_SHARPNESS, MIN_NOTES, Model, heard, weights};

/// FNV-1a over bytes, 64 bits: a hash that's the same on every machine and in every version of Rust.
#[derive(Clone, Copy, Debug)]
pub(super) struct Fnv(u64);

impl Fnv {
    pub(super) const fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub(super) fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub(super) fn number(&mut self, n: i128) {
        self.bytes(&n.to_le_bytes());
    }

    pub(super) fn real(&mut self, x: f64) {
        self.bytes(&x.to_bits().to_le_bytes());
    }

    pub(super) const fn finish(self) -> u64 {
        self.0
    }
}

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

/// The hash `weights.rs` was written with, and the version of the finder it belongs to. `theorybench keyfit --write`
/// keeps the version when the hash is the same, and writes the next one when it isn't.
pub const SHIPPED_HASH: (u64, u32) = (weights::SHIPPED_HASH, weights::HASH_VERSION);
