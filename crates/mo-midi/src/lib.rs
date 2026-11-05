//! The MIDI page's backend: every MIDI part found in your projects and MIDI folders, what kind of part each is, the
//! instrument it's for, and searching them.
//!
//! The parts are read from your projects and `.mid` files ([`indexer`]) into the app's own index, `midi-index.db`
//! ([`store`]), and listed from there; each part's key is read by `mo-harmony`'s key finder ([`keys`]). The C# app's
//! `midi.db` is opened once, read-only, to take in the corrections you made there ([`store::bring_up_to_date`]);
//! [`MidiIndex`] still reads all of it, for the twin test.

pub mod analyser;
pub mod catalogue;
pub mod chord;
pub mod extract;
pub mod gm;
pub mod index;
pub mod indexer;
pub mod instruments;
pub mod keys;
pub mod member;
pub mod roles;
pub mod store;

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
