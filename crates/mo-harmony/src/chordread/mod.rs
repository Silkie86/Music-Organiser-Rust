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
//!   [`SpanFacts`]), weighed by hand-set weights ([`HAND`]) and ranked, best first ([`SpanFacts::rank`]);
//! - the tables the priors read (how often each kind of chord is the labelled one, how often its labels name each
//!   tension, and how chords move from one to the next), counted from POP909-CL's and the Billboard charts' labels by
//!   `theorybench tables` ([`COUNTED`]);
//! - the second step, deciding where the chords change and which chord each stretch is ([`Decoder`]): the best way
//!   to read the whole, the chance of each reading on each stretch, rests, and stretches given by the caller
//!   ([`PinnedSpan`]);
//! - the third step, naming each chord from the notes really played ([`Namer`]): its extensions, its root and its
//!   bass ([`BassKind`]), a seventh heard only in the bass, sus chords over a bass, pedals, the roots of diminished
//!   sevenths and augmented triads, a chord cut in two where its bass moves to its third or seventh, and the chords
//!   given by pins, as given;
//! - the reader as the app uses it ([`ChordReader`], [`read_chords`]): the chords in some notes ([`ChordReading`]),
//!   each stretch with its readings and the answer a person would give ([`ChordAnswer`]: one chord, the same notes
//!   written more than one way, a chord implied by one line, unclear, or no chord), the chart spelled in a key
//!   ([`ChordReading::names`]), Chord check's choices ([`ChordSpan::choices`]), and names found elsewhere checked
//!   against it ([`ChordReading::check_names`]);
//! - the version ([`CHORD_READER_VERSION`]), with the hash of every number the reader is built from
//!   ([`numbers_hash`]).

mod answer;
mod candidates;
mod cores;
mod decode;
#[cfg(test)]
mod decode_tests;
mod hear;
#[cfg(test)]
mod hear_tests;
mod name;
#[cfg(test)]
mod name_tests;
mod reader;
#[cfg(test)]
mod reader_tests;
mod score;
#[cfg(test)]
mod score_tests;
#[cfg(test)]
mod shape_tests;
mod tables;
mod version;
mod weights;

pub use answer::{ChordAnswer, Grading, NoChord, Texture, Thresholds, grade};

pub use candidates::{Candidate, Label, Route, allowed, candidates, escape, open_fifth};
pub use cores::{Core, CoreSet, Group, ToneClass, valid, valid_rootless};
pub use decode::{Chance, DecodedSpan, Decoder, Decoding, KEEP, PinnedSpan, SHARPNESS, SPAN_BARS};
pub use hear::{
    BASS_GAP, BassTexture, HeardNote, Hearing, Level, Line, MOST_UNITS, Mass, Push, Shape, Shaped, Stream, Unit,
    heard_in, played_in, presence_of,
};
pub use name::{BassKind, NamedSpan, Namer, Naming, Reading, SpanFlags};
pub use reader::{
    CHOICE_FLOOR, Choice, ChoiceFrom, ChordContext, ChordReader, ChordReading, ChordSpan, MOST_READINGS, Model,
    NameCheck, Pin, SHIPPED, read_chords,
};
pub use score::{Evidence, Feature, PerFeature, Scored, Scorer, Scoring, SpanFacts};
pub use tables::{COUNTED, COUNTED_FROM};
pub use version::{SHIPPED_HASH, numbers_hash};
pub use weights::{EVEN, HAND, Priors, THRESHOLDS, Tables, Weights};

/// Bumped whenever the reader can read the same notes differently (new weights, a new rule for hearing, decoding,
/// naming or grading), so the chords the app keeps (the MIDI page's index) know to be read again. Version 1 is the
/// first reader, on hand-set weights; the fit's first weights will be version 2.
pub const CHORD_READER_VERSION: u32 = 1;
