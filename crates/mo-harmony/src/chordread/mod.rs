//! The chords in some notes (spec section 5.1, part 3 of the engine): what a pianist would write over each stretch of
//! a part, a loop or a song, with its extensions, its root and its bass, named as jazz and gospel players name them.
//!
//! The reader works in three steps (harmony/design/chord-reader-design.md): it hears the notes (the bass, the chords
//! and the tune, and which notes only pass by), decides where the chords change and which chord each stretch is, and
//! names each chord from the notes it really plays. It is being built in that order, one pull request at a time.
//!
//! What is here so far:
//!
//! - what every step decides between: the 16 kinds of chord ([`Core`]), what each note is to each of them (one of its
//!   own notes, a tension it can carry, or a note that doesn't belong: [`ToneClass`]), and which kinds the notes heard
//!   allow on each root ([`valid`] and [`valid_rootless`]);
//! - the first step, hearing the notes ([`Hearing`]): the units chords change on, the bass, the chords and the tune,
//!   broken chords, notes held over a bar line, how long each note sounds where, the shapes of the lines' notes
//!   ([`Shaped`]), and the chords that may be pushed ([`Push`]);
//! - the readings worth scoring for a stretch of music ([`candidates()`]): each a root and a core ([`Label`]);
//! - the evidence for and against each reading of a stretch ([`Feature`], worked out by [`Evidence`] into
//!   [`SpanFacts`]), weighed by hand-set weights ([`HAND`]) and ranked, best first ([`SpanFacts::rank`]).

mod candidates;
mod cores;
mod hear;
#[cfg(test)]
mod hear_tests;
mod score;
#[cfg(test)]
mod score_tests;
#[cfg(test)]
mod shape_tests;
mod weights;

pub use candidates::{Candidate, Label, Route, allowed, candidates, escape, open_fifth};
pub use cores::{Core, CoreSet, ToneClass, valid, valid_rootless};
pub use hear::{
    BASS_GAP, BassTexture, HeardNote, Hearing, Level, Line, MOST_UNITS, Mass, Push, Shape, Shaped, Stream, Unit,
    heard_in, presence_of,
};
pub use score::{Evidence, Feature, PerFeature, Scored, SpanFacts};
pub use weights::{EVEN, HAND, Tables, Weights};
