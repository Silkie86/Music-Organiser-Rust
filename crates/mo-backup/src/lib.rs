//! Backup: a lean mirror of your music (projects, the samples they use, presets, MIDI, sounds) on another drive.
//!
//! Solomon picked a mirror (31 Oct 2026): plain folders you can open in Explorer without the app, copying only what's
//! new or changed, and kept lean: big audio files no project uses stay out, and of a song's big saves only the ones
//! worth keeping go (the same rules as the Space page's Compact and Thin).
//!
//! Nothing here ever changes, moves or deletes anything in your music folders: they're only read.
//!
//! - [`plan`]: what goes, what's left out and why, and how big each is.
//! - [`copy`]: copying it, each file under a temporary name, checked, then renamed; older versions of small files;
//!   files whose bytes are already on the backup share them.
//! - [`record`]: the backup's own record of what it holds, kept on the backup drive.

pub mod copy;
pub mod plan;
pub mod record;
