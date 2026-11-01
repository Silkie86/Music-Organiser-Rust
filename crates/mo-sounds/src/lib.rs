//! Your sound library: every sample the app knows, what analysis found in it, and your own labels, tags, stars and
//! plays.
//!
//! For now this reads the C# app's `sounds.db` (in `%LOCALAPPDATA%\MusicOrganiser`) and never writes to it: the
//! database is opened read-only, so nothing here can change it. Writing comes once a twin test says the Rust app
//! reads your sounds as the C# one does (CLAUDE.md, "How work is done").
//!
//! What's here so far:
//! - [`SoundsDb`]: the database, read-only.
//! - [`SoundAnalysis`]: what the C# analyser stored for each sound (length, level, pitch, key, chords, waveform…).
//! - [`classify`]: the sound type classifier, learned from your own library (reads the C# app's saved model).
//! - [`learn`]: each sound's type when its name doesn't say, disagreements, and suggested tags.
//! - [`types`]: the sound type a sample's name or folder gives ("Kick", "Pad"…), and loop, tempo and key from names.
//! - [`music`]: note and key names, and Camelot codes.
//! - [`scan`]: finding the sounds in your sample folders, and what a scan has to do (listen, follow moves, forget).

pub mod analyse;
mod analysis;
pub mod classify;
mod db;
mod dotnet_random;
pub mod learn;
pub mod music;
pub mod scan;
pub mod types;

pub use analysis::{ChordSpan, SoundAnalysis};
pub use db::{KnownSound, SoundMatch, SoundsDb, SoundsError, StarPoint, TagCount};

use chrono::NaiveDateTime;
pub(crate) use mo_formats::paths::file_name;

/// One sample in your sound library, as the list shows it.
#[derive(Clone, Debug)]
pub struct Sound {
    pub id: i64,
    pub path: String,
    /// Why the file couldn't be analysed, if it couldn't.
    pub error: Option<String>,
    /// What the analyser found; `None` until it has run, or when the stored analysis can't be read.
    pub analysis: Option<SoundAnalysis>,
    /// The type shown: your own label, else the name's, else what the classifier learned.
    pub sound_type: String,
    /// Where `sound_type` came from: "you", "name", "learned" or "model" ("" when nothing said).
    pub type_source: String,
    pub type_confidence: f64,
    /// Your own label, when you've set one. These are the strongest training examples.
    pub user_type: Option<String>,
    /// The type the file's name or folders give, if any.
    pub name_type: Option<String>,
    /// Sorted by name.
    pub tags: Vec<String>,
    pub favourite: bool,
    /// A copy you've hidden as a duplicate: the file stays, the app just doesn't list it.
    pub hidden: bool,
    /// 0 to 5 stars.
    pub rating: i32,
    pub plays: i32,
    /// When the file was last changed (local time): what "Newest" sorts by.
    pub added: NaiveDateTime,
    /// Plays, drags and sends, recent and older, for Most liked.
    pub activity: SoundActivity,
}

impl Sound {
    /// The file's name without its extension: "Kick_01".
    pub fn name(&self) -> &str {
        stem(file_name(&self.path))
    }

    /// The name of the folder the file is in: "Kicks". Empty for a file at the top of a drive or share ("D:\\x.wav"),
    /// as .NET's `GetFileName(GetDirectoryName(path))` gives it.
    pub fn folder(&self) -> &str {
        let p = self.path.trim_end_matches(['\\', '/']);
        let Some(dir) = p.rfind(['\\', '/']).and_then(|i| p.get(..i)) else {
            return "";
        };
        let is_drive = matches!(dir.as_bytes(), [d, b':'] if d.is_ascii_alphabetic());
        // "\\server\share": a share's root, with no folder of its own.
        let is_share =
            dir.starts_with(r"\\") && dir.get(2..).is_some_and(|rest| rest.matches(['\\', '/']).count() <= 1);
        if is_drive || is_share { "" } else { file_name(dir) }
    }

    /// The length in seconds (0 until analysed).
    pub fn seconds(&self) -> f64 {
        self.analysis.as_ref().map_or(0.0, |a| a.seconds)
    }

    pub fn is_loop(&self) -> bool {
        self.analysis.as_ref().is_some_and(|a| a.is_loop)
    }

    pub fn bpm(&self) -> Option<f64> {
        self.analysis.as_ref().and_then(|a| a.bpm)
    }

    /// The key, short: "Ebm", "F#". Empty when none was found.
    pub fn key(&self) -> String {
        self.analysis.as_ref().and_then(SoundAnalysis::key_name).unwrap_or_default()
    }

    /// The main pitch of a one-shot as a note name: "C2". Empty when none was found.
    pub fn root_note(&self) -> String {
        self.analysis.as_ref().and_then(SoundAnalysis::root_note_name).unwrap_or_default()
    }

    /// The chord, when the whole sound is one.
    pub fn chord(&self) -> &str {
        self.analysis.as_ref().and_then(|a| a.chord.as_deref()).unwrap_or("")
    }

    /// Character words joined: "punchy, dark".
    pub fn character(&self) -> String {
        self.analysis.as_ref().map(|a| a.character.join(", ")).unwrap_or_default()
    }
}

/// What can be done with a sound that says you like it, as `sound_events.kind` stores it.
pub mod event {
    pub const PLAY: &str = "play";
    pub const DRAG: &str = "drag";
    pub const SEND: &str = "send";
}

/// Deliberate plays, drags out of the app and sends into a DAW: in the last year, and before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoundActivity {
    pub plays: i32,
    pub older_plays: i32,
    pub drags: i32,
    pub older_drags: i32,
    pub sends: i32,
    pub older_sends: i32,
}

/// A file name without its extension, as .NET's `GetFileNameWithoutExtension` cuts it: everything before the last dot
/// (so ".wav" alone gives "").
pub(crate) fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) => name.get(..i).unwrap_or(name),
        None => name,
    }
}
