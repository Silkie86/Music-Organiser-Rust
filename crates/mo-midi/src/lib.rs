//! The MIDI page's backend: every MIDI part found in your projects and MIDI folders, what kind of part each is, the
//! instrument it's for, and searching them.
//!
//! For now the parts come from the C# app's `midi.db` (beside `library.db`), opened read-only: nothing here can change
//! it. Reading projects and `.mid` files into parts in Rust, and a database of the Rust app's own, come next
//! (CLAUDE.md, "How work is done"). Keys and chords are read by `mo-harmony` once it's built (phase 6): until then a
//! part's key and chords are the ones the C# app stored.

pub mod catalogue;
pub mod chord;
pub mod index;
pub mod instruments;
pub mod member;
pub mod roles;

pub use catalogue::{Catalogue, Entry, From, Hit, Query};
pub use chord::PhraseChord;
pub use index::{Failure, Key, MidiError, MidiIndex, Source, SourceKind, Stopper, StoredPhrase};
pub use instruments::{Family, InstrumentGuess};
pub use member::PhraseMember;
pub use roles::Role;

/// Every phrase is kept at this resolution: 96 ticks to a beat (the C# `PhraseAnalyser.Ppq`).
pub const PPQ: i64 = 96;
/// A bar of 4/4 in ticks.
pub const BAR: i64 = 4 * PPQ;
