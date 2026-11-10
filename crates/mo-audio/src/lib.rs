//! Sound files: reading them into samples, and writing WAV files (for kit exports).
//!
//! WAV (PCM, float, and the Microsoft and IMA ADPCM kinds older packs use) and AIFF are read here, byte for byte as the C# app's `Sounds/AudioFile.cs` reads them, with the notes some
//! sample packs keep inside them (an ACID chunk's tempo and loop flag, a sampler chunk's root note). MP3, FLAC, M4A,
//! OGG and AAC are decoded with symphonia, on every system. [`play::Player`] plays one sound at a time through the
//! output chosen, and carries on when that output goes and comes back.
//!
//! Every read is bounded: a damaged or cut-short file gives an error or the part that's there, never a crash.

mod adpcm;
pub mod dsp;
#[cfg(feature = "gm")]
pub mod gm;
mod other;
#[cfg(feature = "play")]
pub mod play;
mod read;
mod wma;
mod write;

pub use other::OTHER_EXTENSIONS;
pub use read::{MAX_SECONDS, can_read_from, read, read_from, reads_exactly};
pub use wma::WMA_EXTENSIONS;
pub use write::write_wav;

/// Whether this computer can decode this sound file (by its extension, ignoring case): WAV, AIFF, MP3, FLAC, M4A,
/// OGG and AAC everywhere, WMA on Windows only. A sound it can't decode can be listed but not played or analysed.
pub fn can_decode(path: &std::path::Path) -> bool {
    can_read_from(path) || other::is_other(path) || (cfg!(windows) && wma::is_wma(path))
}

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

    /// Fades the last few milliseconds out when less than the whole file was decoded (a sweep reads only a sound's
    /// start), so its cut end doesn't click. A sound decoded to its own end is left as it is.
    pub fn fade_cut_end(&mut self) {
        const FADE_SECONDS: f64 = 0.005;
        let frames = self.len();
        if frames == 0
            || self.sample_rate == 0
            || self.seconds() + 1.0 / f64::from(self.sample_rate) >= self.file_seconds
        {
            return;
        }
        let n = ((FADE_SECONDS * f64::from(self.sample_rate)).ceil() as usize).clamp(1, frames);
        for channel in &mut self.channels {
            for (i, s) in channel[frames - n..].iter_mut().enumerate() {
                *s *= (n - 1 - i) as f32 / n as f32;
            }
        }
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
    /// A kind of sound this reader doesn't decode ("WAV format 85 isn't PCM", "WMA files"…).
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

#[cfg(test)]
mod tests {
    use super::AudioData;

    fn sound(frames: usize, file_seconds: f64) -> AudioData {
        AudioData {
            sample_rate: 1000,
            channels: vec![vec![1.0; frames], vec![0.5; frames]],
            file_seconds,
            embedded_tempo: None,
            embedded_root_note: None,
            embedded_is_loop: None,
        }
    }

    #[test]
    fn a_cut_sound_fades_out_at_its_end_and_a_whole_one_does_not() {
        // 1 s of a 4 s file at 1000 frames a second: its last 5 frames go down to silence.
        let mut cut = sound(1000, 4.0);
        cut.fade_cut_end();
        assert_eq!(cut.channels[0][..995], vec![1.0; 995][..]);
        assert_eq!(cut.channels[0][995..], [0.8, 0.6, 0.4, 0.2, 0.0]);
        assert_eq!(cut.channels[1][999], 0.0);
        assert_eq!(cut.channels[1][995], 0.4);
        // Decoded to its own end: left as it is.
        let mut whole = sound(1000, 1.0);
        whole.fade_cut_end();
        assert!(whole.channels[0].iter().all(|&s| s == 1.0));
        // Shorter than the fade: all of it fades.
        let mut tiny = sound(2, 4.0);
        tiny.fade_cut_end();
        assert_eq!(tiny.channels[0], [0.5, 0.0]);
    }
}
