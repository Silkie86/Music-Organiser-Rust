//! The reader as the fit uses it (`theorybench chordfit`, harmony/design/chord-reader-design.md, "Model, training and
//! data"): some notes and their chord labels in, what the fit needs to make the labelled readings likelier out
//! ([`FitRead`]).
//!
//! The notes are heard and decoded as the reader decodes them, except that the readings the labels accept are kept on
//! every span inside a labelled stretch, whatever their rank. Then every way the decode kept is summed, and the ways
//! the labels allow: a way is allowed when each reading it takes is one the labels accept where it is (reaching at
//! most [`LABEL_TOLERANCE`](super::LABEL_TOLERANCE) into a stretch labelled otherwise) and it starts a chord only
//! within that of a labelled change. Stretches labelled with no chord, or with a chord the reader has no reading for,
//! allow any reading. A labelled change with no place a chord may change within that of it (the labels' time is off,
//! or the music holds on through it) is moved to the nearest place one may.

use super::candidates::Label;
use super::decode::{Decoder, Decoding, FitSums, FitWork, Gold, IdSet};
use super::hear::Hearing;
use super::weights::{Tables, Weights};
use crate::keyfind::Span;
use crate::notes::{Grid, Part};

/// A labelled stretch, as the fit is told it: where it starts and ends, in the music's own ticks (each moved to the
/// nearest unit's start), and the readings it accepts. None accepted (an empty list) leaves the stretch free: no chord,
/// or a chord the reader has no reading for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FitLabel {
    pub start: i64,
    pub end: i64,
    pub accepts: Vec<Label>,
}

/// What one piece of labelled music gives the fit ([`ChordFitter::read`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitRead {
    /// ln Z over every way, and over the ways the labels allow, with the evidence each holds on average.
    pub sums: FitSums,
    /// Units heard, the readings the decode kept only because the labels accept them, and the ends of labelled
    /// stretches moved to the nearest place a chord may change.
    pub units: usize,
    pub forced: usize,
    pub moved: usize,
}

/// A reader for the fit. Keep one and reuse it: it keeps its working space between reads.
#[derive(Debug, Default)]
pub struct ChordFitter {
    hearing: Hearing,
    decoder: Decoder,
    decoding: Decoding,
    work: FitWork,
}

impl ChordFitter {
    pub fn new() -> ChordFitter {
        ChordFitter::default()
    }

    /// Hears some notes, decodes them with `weights` and `tables`, keeping the readings `labels` accept, and sums every
    /// way and the ways the labels allow. None when nothing was heard.
    pub fn read(
        &mut self,
        parts: &[Part],
        grid: Grid,
        span: Span,
        labels: &[FitLabel],
        weights: &Weights,
        tables: &Tables,
    ) -> Option<FitRead> {
        if !self.hearing.hear(parts, grid, span) {
            return None;
        }
        let units = self.hearing.units();
        let hearing = &self.hearing;
        let stretches = labels
            .iter()
            .filter(|l| !l.accepts.is_empty())
            .map(|l| (hearing.nearest_unit(l.start)..hearing.nearest_unit(l.end), IdSet::of(&l.accepts)))
            .collect();
        self.decoder.set_gold(Gold::new(units, stretches));
        let read = self.decoder.decode(&self.hearing, weights, tables, &[], &mut self.decoding);
        let out = read.then(|| {
            let sums = self.decoder.fit_sums(&self.hearing, tables, &mut self.work);
            FitRead { sums, units, forced: self.decoder.forced(), moved: self.decoder.moved() }
        });
        self.decoder.set_gold(Gold::default());
        out
    }
}
