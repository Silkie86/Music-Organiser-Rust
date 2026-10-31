//! Sound files: reading them into samples, and writing WAV files (for kit exports).
//!
//! WAV and AIFF are read here, byte for byte as the C# app's `Sounds/AudioFile.cs` reads them, with the notes some
//! sample packs keep inside them (an ACID chunk's tempo and loop flag, a sampler chunk's root note). MP3, FLAC, M4A,
//! OGG and AAC are decoded with symphonia, on every system. [`play::Player`] plays one sound at a time through the
//! output chosen, and carries on when that output goes and comes back.
//!
//! Every read is bounded: a damaged or cut-short file gives an error or the part that's there, never a crash.

pub mod dsp;
mod other;
#[cfg(feature = "play")]
pub mod play;
mod read;
mod write;

pub use other::OTHER_EXTENSIONS;
pub use read::{MAX_SECONDS, can_read_from, read, read_from};
pub use write::write_wav;

use std::fmt;

/// Decoded audio: one list of samples (-1 to 1) per channel, plus the loop notes some sample packs keep in the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioData {
    pub sample_rate: u32,
    /// One list per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
    /// The length of the whole file in seconds (may be longer than what was decoded).
    pub file_seconds: f64,
    /// The tempo an ACID chunk gives, when the pack's maker set one.
    pub embedded_tempo: Option<f64>,
    /// The MIDI root note an ACID or sampler chunk gives.
    pub embedded_root_note: Option<i32>,
    /// `Some(true)` when an ACID chunk marks the file as a loop, `Some(false)` when it marks a one-shot.
    pub embedded_is_loop: Option<bool>,
}

impl AudioData {
    /// Frames decoded (samples per channel).
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Seconds decoded.
    pub fn seconds(&self) -> f64 {
        if self.sample_rate == 0 { 0.0 } else { self.len() as f64 / f64::from(self.sample_rate) }
    }

    /// The channels mixed to one, each at an equal share. Borrows the only channel of a mono file rather than copying it.
    pub fn mono(&self) -> std::borrow::Cow<'_, [f32]> {
        match self.channels.as_slice() {
            [only] => std::borrow::Cow::Borrowed(only),
            channels => {
                let mut mono = vec![0f32; self.len()];
                for ch in channels {
                    for (m, s) in mono.iter_mut().zip(ch) {
                        *m += s;
                    }
                }
                let scale = 1.0 / channels.len().max(1) as f32;
                for m in &mut mono {
                    *m *= scale;
                }
                std::borrow::Cow::Owned(mono)
            }
        }
    }
}

/// Why a sound file couldn't be read or written.
#[derive(Debug)]
pub enum AudioError {
    /// The file couldn't be opened, read or written.
    Io(std::io::Error),
    /// It isn't a sound file the app reads, or its header is damaged: says what's wrong.
    Format(String),
    /// A kind of sound this reader doesn't decode ("WAV format 2 isn't PCM", "WMA files"…).
    NotSupported(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Io(e) => write!(f, "{e}"),
            AudioError::Format(why) | AudioError::NotSupported(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<std::io::Error> for AudioError {
    fn from(e: std::io::Error) -> AudioError {
        AudioError::Io(e)
    }
}
