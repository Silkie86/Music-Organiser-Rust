//! Bitwig Studio's files: `.bwproject`, and `.bwpreset` and `.bwclip`, which are laid out the same.

pub mod encode;
pub mod format;

pub use encode::{Copier, clone_object, encode};
pub use format::{Blob, Document, Field, NotBitwig, ObjId, Object, Slot, Text, Value, decode, is_bitwig};
