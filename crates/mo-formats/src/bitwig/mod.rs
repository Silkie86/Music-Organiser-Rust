//! Bitwig Studio's files: `.bwproject`, and `.bwpreset` and `.bwclip`, which are laid out the same.

pub mod encode;
pub mod format;
pub mod reader;

pub use encode::{Copier, clone_object, encode};
pub use format::{Blob, Document, Field, NotBitwig, ObjId, Object, Slot, Text, Value, decode, is_bitwig};
pub use reader::{DAW, EXTENSIONS, VERSION, extract_midi, extract_presets, read, read_document, read_from};
