//! Pushed chords: a chord struck just before a felt beat and held over it, as gospel and R&B keys come in an eighth
//! early while the bass lands on the beat. The reader may count such a chord from the beat; that's a choice it scores,
//! not a rule, so a chord that really changes early can still be read where it's struck.

use std::ops::Range;

use super::{HeardNote, Hearing, Level, Stream};
use crate::pitch::PcSet;

/// A chord that may be pushed: struck in the unit before a felt beat and held over it (the design's E).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Push {
    /// The felt beat it may be counted from.
    pub unit: usize,
    /// Where its first note is struck, in ticks from the first unit's start.
    pub at: i64,
    /// Its pitch classes.
    pub pcs: PcSet,
    /// Its notes, as places in the list of pushed notes ([`Hearing::push_notes`]).
    notes: Range<usize>,
}

impl Hearing {
    /// The felt beats a chord may be pushed to, in order (a loop's copied bar included).
    pub fn pushes(&self) -> &[Push] {
        &self.pushes
    }

    /// A push's notes: their places in [`Hearing::notes`], and the notes.
    pub fn push_notes(&self, push: &Push) -> impl Iterator<Item = (usize, &HeardNote)> + '_ {
        let places = self.pushed.get(push.notes.clone()).unwrap_or_default();
        places.iter().filter_map(|&i| self.notes.get(i).map(|n| (i, n)))
    }

    /// Finds the pushes. A felt beat is one when the chord and bass notes struck in the unit before it and held at
    /// least a unit past it:
    ///
    /// - have two or more pitch classes, making up at least half of the chords' pitch classes sounding in the felt
    ///   beat's unit;
    /// - and the bass there is one of them, is struck there, or is silent.
    ///
    /// So a chord whose bass is still held from before isn't pushed. In a loop, a note held to the loop's end is held
    /// past its start, as the loop goes round: a chord struck in the loop's last unit can be pushed to its start.
    pub(super) fn find_pushes(&mut self) {
        self.pushes.clear();
        self.pushed.clear();
        let layout = self.layout;
        for unit in 1..self.units.len() {
            if layout.level(unit) < Level::Beat {
                continue;
            }
            let (from, to, past) = (layout.start(unit - 1), layout.start(unit), layout.start(unit + 1));
            // The notes struck in the unit before lie together: the notes are in order of where they start.
            let first = self.notes.partition_point(|n| n.start < from);
            let begin = self.pushed.len();
            let (mut pcs, mut at) = (PcSet::EMPTY, to);
            for (i, n) in self.notes.iter().enumerate().skip(first).take_while(|(_, n)| n.start < to) {
                let held = n.end >= past || (n.wraps && n.copy && n.end >= to);
                if n.struck && n.stream != Stream::Melody && held {
                    self.pushed.push(i);
                    pcs = pcs.with(n.pitch_class());
                    at = at.min(n.start);
                }
            }
            let chords = self.sounding(unit, Stream::Harmony);
            let bass = self.sounding(unit, Stream::Bass);
            let bass_fits = bass.difference(pcs.union(self.struck(unit, Stream::Bass))).is_empty();
            if pcs.len() >= 2 && 2 * pcs.intersection(chords).len() >= chords.len() && bass_fits {
                self.pushes.push(Push { unit, at, pcs, notes: begin..self.pushed.len() });
            } else {
                self.pushed.truncate(begin);
            }
        }
    }
}
