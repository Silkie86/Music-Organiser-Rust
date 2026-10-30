//! The DAW project files: what's in them, read the same way the C# app reads them (its readers and its format registry
//! are the reference), and better where a real file shows the C# app was wrong.
//!
//! Every reader works on the file's bytes, read once, and never changes the file.

// A damaged file can say anything, so the readers never index or slice bytes or lists where that could run past the
// end: they use `get` and say what's wrong instead (tests may index). Text is still cut where its own search (`find`,
// `match_indices`) says, which is always between letters.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod bitwig;
pub mod fl;
pub mod midi;
mod model;
mod paths;
pub mod plugins;
pub mod presets;
pub mod studio_one;
pub mod textscan;

pub use model::{
    Confidence, MidiChannel, MidiClip, NoteEvent, PluginKind, PluginRef, ProjectInfo, SampleUse, velocity,
};
