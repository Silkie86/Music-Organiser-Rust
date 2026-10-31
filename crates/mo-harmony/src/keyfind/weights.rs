//! The key finder's weights. Made by `theorybench keyfit`; don't edit by hand.
//!
//! Placeholder until the first fit: every key scores the same.

use super::{Calibration, FEATURES};

/// Whether these weights were fitted (`false` for the placeholder).
pub(super) const FITTED: bool = false;

/// One weight per feature (see `Heard::features`).
pub(super) const WEIGHTS: [f64; FEATURES] = [0.0; FEATURES];
/// How much the scores are sharpened (see `Calibration`).
pub(super) const CALIBRATION: Calibration =
    Calibration([1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
