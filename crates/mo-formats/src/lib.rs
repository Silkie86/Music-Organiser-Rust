//! The DAW project files: what's in them, read the same way the C# app reads them (its readers and its format registry
//! are the reference), and better where a real file shows the C# app was wrong.
//!
//! Every reader works on the file's bytes, read once, and never changes the file.

pub mod fl;
mod model;
pub mod plugins;
pub mod presets;

pub use model::{Confidence, MidiChannel, MidiClip, NoteEvent, PluginKind, PluginRef, ProjectInfo, SampleUse};
