//! Bitwig Studio's files: `.bwproject`, and `.bwpreset` and `.bwclip`, which are laid out the same.

pub mod arrangement;
pub mod encode;
pub mod format;
pub mod reader;

pub use encode::{Copier, clone_object, encode};
pub use format::{Blob, Document, Field, FileBytes, NotBitwig, ObjId, Object, Slot, Text, Value, decode, is_bitwig};
pub use reader::{
    DAW, EXTENSIONS, Project, TrackNames, VERSION, extract_midi, extract_presets, read, read_from, read_project,
    track_names,
};
