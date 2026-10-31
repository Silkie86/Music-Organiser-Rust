//! The Presets page's engine (spec section 3.4a): every sound made inside a song (each plugin's saved settings, by
//! song and channel or track) and preset files on disk, what each one is, and a guess at the part it plays.
//!
//! - [`files`]: preset files on disk (`.fxp`, `.fxb`, `.vstpreset`, `.bwpreset`), read as far as their header.
//! - [`role`]: the part guess (bass, lead, chords, pad, stabs, arp, drums, FX) from the notes as the listener hears them.
//! - [`octave`]: how far a plugin moves its notes, from its saved settings (Serum 1 so far).
//! - [`list`]: one song's sounds, with their parts guessed (FL Studio and Bitwig).
//! - [`library`]: many saves and preset folders at once, over every core, as the page's rows.
//! - [`filter`]: the page's choices and counts.
//! - [`export`]: a song's sounds as preset files in the export folder (copy, check, rename).
//! - [`fixes`]: your fixes to the part guess, in the app's own `presets.db`.
//!
//! No UI here: the screen logic goes in `mo-app`. It writes only `presets.db` ([`fixes`]) and exported presets in the
//! export folder ([`export`]); it never changes a project or a preset file you made.

// Preset files can be damaged in any way, so nothing here indexes past what's there (tests may).
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod export;
pub mod files;
pub mod filter;
pub mod fixes;
pub mod library;
pub mod list;
pub mod names;
pub mod octave;
pub mod role;

pub use role::{Guess, Role, Shift, Thresholds};
