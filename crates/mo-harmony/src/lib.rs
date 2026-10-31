//! The harmony engine: the key of a song or a part, the chords in its notes, and what each note does over them, for
//! the Songs and MIDI pages and, later, the Playground app.
//!
//! Planned again from the ground up (spec section 5.1): the C# app's engine was its weakest part, so this one is built
//! from the research, measured against labelled music and Solomon's own keys and chord picks, and only changed where
//! a change measures better. Reading keys and chords has no style; suggestions take a style as a set of weights.
//!
//! Everything works on pitch classes ([`pitch`]) and has no UI, no files and no database in it, so every part can be
//! tested on its own.

// Notes can come from a damaged file and be anything, so nothing here indexes past the end of a list: tables of twelve
// are picked by pitch class ([`pitch::PerPitch`]), and lists by `get` (tests may index).
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod chord;
pub mod key;
pub mod keyfind;
pub mod names;
pub mod notes;
pub mod pitch;
pub mod spell;

pub use chord::{Chord, Family, ToneRole};
pub use key::{Key, KeyRelation, Mode};
pub use keyfind::{
    Colours, Heard, HintCheck, HintSource, KEY_FINDER_VERSION, KeyAnswer, KeyFact, KeyReader, KeyReading, Span, find_key,
};
pub use notes::{Grid, Note, Part, Role};
pub use pitch::{PcSet, PerPitch, PitchClass};
pub use spell::{Degree, Spelled, Tpc};
