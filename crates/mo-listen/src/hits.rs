//! Finding hits (drum hits, plucks, note starts) in the audio as it comes: a short frame that's clearly louder than
//! the few before it. Rough on purpose: it feeds the tempo guess and "when hits land", not a transcription.

use std::collections::VecDeque;

/// Samples in one of the hit finder's frames: 5.3 ms at 48 kHz, short enough to place a hit closely.
const FRAME: usize = 256;
/// How many earlier frames a frame is compared with (about 43 ms at 48 kHz).
const BEFORE: usize = 8;
/// How much louder than those a frame must be to be a hit, in dB.
const RISE_DB: f32 = 6.0;
/// Quieter than this (dB below full scale) is never a hit, so noise can't make them.
const FLOOR_DB: f32 = -50.0;
/// The shortest gap between two hits, in seconds: a hit's own tail can't count again.
const GAP: f64 = 0.05;

/// One hit: where it starts in the stream and how sharp it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The stream sample its frame starts at (the same count as [`crate::Heard::samples_to`]).
    pub at: u64,
    /// How far above the frames before it, in dB (at least 6).
    pub rise_db: f32,
}

pub(crate) struct Hits {
    gap: u64,
    /// The levels of the last few frames, in dB.
    before: VecDeque<f32>,
    /// Samples of a frame that hasn't filled yet, and the stream sample it starts at.
    partial: Vec<f32>,
    partial_at: u64,
    last: Option<u64>,
}

impl Hits {
    pub(crate) fn new(sample_rate: f64) -> Hits {
        Hits {
            gap: (GAP * sample_rate.max(1.0)) as u64,
            before: VecDeque::with_capacity(BEFORE + 1),
            partial: Vec::with_capacity(FRAME),
            partial_at: 0,
            last: None,
        }
    }

    /// Takes the audio from stream sample `at` on, and adds the hits found in it to `out`.
    pub(crate) fn feed(&mut self, at: u64, samples: &[f32], out: &mut Vec<Hit>) {
        if at != self.partial_at + self.partial.len() as u64 {
            // A gap (the ear fell a ring behind): start again from here.
            self.partial.clear();
            self.before.clear();
            self.partial_at = at;
        }
        for &s in samples {
            self.partial.push(s);
            if self.partial.len() < FRAME {
                continue;
            }
            let power = self.partial.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / FRAME as f64;
            let db = (10.0 * (power + 1e-12).log10()) as f32;
            if self.before.len() == BEFORE {
                let average = self.before.iter().sum::<f32>() / BEFORE as f32;
                let rise = db - average;
                let rising = self.before.back().is_none_or(|b| db > *b);
                let apart = self.last.is_none_or(|l| self.partial_at.saturating_sub(l) >= self.gap);
                if rise >= RISE_DB && db >= FLOOR_DB && rising && apart {
                    out.push(Hit { at: self.partial_at, rise_db: rise });
                    self.last = Some(self.partial_at);
                }
                self.before.pop_front();
            }
            self.before.push_back(db);
            self.partial_at += FRAME as u64;
            self.partial.clear();
        }
    }
}
