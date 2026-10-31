//! The key finder's weights. Made by `theorybench keyfit`; don't edit by hand.
//!
//! Placeholder until the first fit: every key scores the same.

use super::{Calibration, FEATURES, THRESHOLD_KNOTS, Thresholds};

/// Whether these weights were fitted (`false` for the placeholder).
pub(super) const FITTED: bool = false;

/// One weight per feature (see `Heard::features`).
pub(super) const WEIGHTS: [f64; FEATURES] = [0.0; FEATURES];
/// How much the scores are sharpened (see `Calibration`).
pub(super) const CALIBRATION: Calibration =
    Calibration([1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
/// How likely each kind of answer must be (see `Thresholds`) at 1, 2, 4, 8, 16 and 32 bars: the design's starting
/// values, until the bench fits them.
pub(super) const THRESHOLDS: Thresholds =
    Thresholds { one: [0.85, 0.85, 0.8, 0.8, 0.75, 0.7], pair: [0.9; THRESHOLD_KNOTS], home: [0.85; THRESHOLD_KNOTS] };
/// Notes with less than this share of their time inside one scale have no key (see `Why::NotTonal`).
pub(super) const NOT_TONAL_BELOW: f64 = 0.75;
/// The hash of every number the finder is built from (see `numbers_hash`) when this file was written, and the version
/// of the finder these numbers were first given with: a test checks both, so changing any number fails until the
/// version is bumped.
pub(super) const SHIPPED_HASH: u64 = 0x1380_78d5_25cd_f699;
pub(super) const HASH_VERSION: u32 = 3;
