//! Backup: a lean mirror of your music (projects, the samples they use, presets, MIDI, sounds) on another drive.
//!
//! Solomon picked a mirror (31 Oct 2026): plain folders you can open in Explorer without the app, copying only what's
//! new or changed, and kept lean: big audio files no project uses stay out, and of a song's big saves only the ones
//! worth keeping go (the same rules as the Space page's Compact and Thin).
//!
//! Nothing here ever changes, moves or deletes anything in your music folders: they're only read. On the backup drive,
//! the only things ever removed are the older versions you choose to remove ([`older::remove`]).
//!
//! - [`plan`]: what goes, what's left out and why, and how big each is.
//! - [`copy`]: copying it, each file under a temporary name, checked, then renamed; older versions of small files;
//!   files whose bytes are already on the backup share them.
//! - [`older`]: the older versions a backup keeps, kept for ever, and removed only when you choose.
//! - [`record`]: the backup's own record of what it holds, kept on the backup drive.
//! - [`gone`]: the files it keeps that are gone from your folders (deleted or moved), which it keeps, with a warning.
//! - [`pack`]: the other format, one zip per song.
//! - [`folders`]: backing up a list of folders in one go, each whole or slimmed.

pub mod copy;
pub mod folders;
pub mod gone;
pub mod older;
pub mod pack;
pub mod plan;
pub mod record;

/// The error a job gives back when you stop it.
pub(crate) fn stopped() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "Stopped")
}
