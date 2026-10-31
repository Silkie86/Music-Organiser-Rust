//! The Presets page's engine (spec section 3.4a): every sound made inside a song (each plugin's saved settings, by
//! song and channel or track) and preset files on disk, what each one is, and a guess at the part it plays.
//!
//! - [`files`]: preset files on disk (`.fxp`, `.fxb`, `.vstpreset`, `.bwpreset`), read as far as their header.
//! - [`role`]: the part guess (bass, lead, chords, pad, stabs, arp, drums, FX) from the notes as the listener hears them.
//! - [`list`]: one song's sounds, with their parts guessed (FL Studio and Bitwig).
//! - [`library`]: many saves and preset folders at once, over every core, as the page's rows.
//! - [`filter`]: the page's choices and counts.
//!
//! No UI, no database and no writing here: the screen logic goes in `mo-app`, and keeping your fixes to a guess waits
//! on where the library keeps them.

// Preset files can be damaged in any way, so nothing here indexes past what's there (tests may).
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod files;
pub mod filter;
pub mod library;
pub mod list;
pub mod role;

pub use role::{Guess, Role, Shift, Thresholds};
