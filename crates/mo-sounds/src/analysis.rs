//! What the C# analyser stored for each sound: the `analysis` column of `sounds`, as JSON with the C# property names.

use serde::{Deserialize, Serialize};

use crate::music;

/// A stretch of a sound that plays one chord.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ChordSpan {
    /// Seconds from the start.
    pub start: f64,
    pub end: f64,
    pub name: String,
}

/// One sound's analysis. Every field may be missing in an older analysis, and then reads as its default.
///
/// The features vector isn't kept here: the database holds it again in the `features` column, which
/// [`crate::SoundsDb::vectors`] reads.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SoundAnalysis {
    pub seconds: f64,
    pub sample_rate: i32,
    pub channel_count: i32,
    pub peak_db: f64,
    pub rms_db: f64,
    /// Milliseconds from the start to the peak level.
    pub attack_ms: f64,
    /// Milliseconds from the peak until the level falls 30 dB.
    pub decay_ms: f64,
    /// Spectral centre of mass in Hz: low is dark, high is bright.
    pub brightness: f64,
    /// 0 is a pure tone, 1 is white noise.
    pub noisiness: f64,
    /// 0 is mono, 1 is very wide stereo.
    pub width: f64,
    pub is_loop: bool,
    pub bpm: Option<f64>,
    /// Where the tempo came from ("acid", "name", "onsets"…).
    pub bpm_source: String,
    /// The main pitch of a one-shot, as a MIDI note number with cents.
    pub pitch: Option<f64>,
    pub pitch_confidence: f64,
    /// 0 = C … 11 = B.
    pub key_tonic: Option<i32>,
    pub key_minor: bool,
    pub key_confidence: f64,
    pub key_source: String,
    /// The chord's name when the whole sound is one chord (chord stabs).
    pub chord: Option<String>,
    pub chords: Vec<ChordSpan>,
    /// How much of each of the 12 notes is in the sound, C first.
    pub chroma: Vec<f64>,
    /// Character words: punchy, soft, bright, dark, wide, tonal, noisy…
    pub character: Vec<String>,
    /// Peak levels in 0..1, for drawing a small waveform.
    pub waveform: Vec<f32>,
}

impl SoundAnalysis {
    /// Reads the JSON the C# app stored. `None` when it isn't a readable analysis (the C# app then shows none too).
    pub fn from_json(json: &str) -> Option<SoundAnalysis> {
        serde_json::from_str(json).ok()
    }

    /// The key, short ("Ebm", "F#"), when one was found.
    pub fn key_name(&self) -> Option<String> {
        self.key_tonic.map(|t| music::key_name(t, self.key_minor, true))
    }

    /// The key's Camelot code ("8A"), when one was found.
    pub fn camelot(&self) -> Option<String> {
        self.key_tonic.map(|t| music::camelot(t, self.key_minor))
    }

    /// The main pitch as a note name ("C2"), when one was found.
    pub fn root_note_name(&self) -> Option<String> {
        // C# rounds half to even (Math.Round), and casts to int.
        self.pitch.filter(|p| p.is_finite()).map(|p| music::note_name(p.round_ties_even() as i32))
    }
}
