//! The library: every project save the app knows, your own fields on them (status, rating, key, notes, tags), and the
//! saves grouped into songs.
//!
//! For now this reads the C# app's `library.db` (in `%LOCALAPPDATA%\MusicOrganiser`) and never writes to it: the
//! database is opened read-only, so nothing here can change it. Writing comes once the twin test says the Rust app
//! reads your library as the C# one does (CLAUDE.md, "How work is done").

mod db;
pub mod songs;

pub use db::{Library, LibraryError, ProjectPlugin, ProjectSample, SaveDetail};
pub use mo_formats::{Confidence, PluginKind};
pub use songs::{SaveRun, Song, SongChoices, SongDoubt};

use chrono::NaiveDateTime;

/// The statuses a song can have, in the order they're offered.
pub const STATUSES: [&str; 7] = ["Idea", "In progress", "Arranging", "Mixing", "Finished", "Released", "Archived"];

/// One project save as the library lists it (a row of `projects`, with its counts). Times are local time.
#[derive(Clone, Debug)]
pub struct Save {
    pub id: i64,
    pub path: String,
    /// The file's name without its extension: "Didums_77".
    pub name: String,
    /// "FL Studio", "Bitwig", "Studio One", "MIDI" …
    pub daw: String,
    pub folder: String,
    pub size: i64,
    /// When the file was last written, as the scan saw it.
    pub modified: NaiveDateTime,
    /// The same, as stored: Unix seconds.
    pub mtime: i64,
    pub tempo: Option<f64>,
    /// The DAW version that saved it.
    pub version: String,
    pub confidence: Confidence,
    /// The file has moved or gone since the scan.
    pub file_missing: bool,
    /// Why the file couldn't be read, when it couldn't.
    pub error: Option<String>,
    pub last_opened: Option<NaiveDateTime>,

    pub plugin_count: u32,
    pub sample_count: u32,
    pub missing_plugins: u32,
    pub missing_samples: u32,
    /// Your tags, in order.
    pub tags: Vec<String>,
    pub plugin_names: Vec<String>,
    /// Hardware it uses (Elektron boxes and the like).
    pub hardware: Vec<String>,

    // Your own fields, kept across rescans.
    pub status: String,
    pub rating: i32,
    pub favourite: bool,
    /// The key you gave it.
    pub musical_key: String,
    /// The key its notes point to (drums left out), or "" when they don't point to one.
    pub detected_key: String,
    /// When the project was first made, as its file records it (FL Studio does).
    pub created: Option<NaiveDateTime>,
    /// How many notes play in it, how many bars they run, and how many channels or tracks it has; -1 when unknown.
    pub note_count: i32,
    pub length_bars: i32,
    pub track_count: i32,
    pub notes: String,
}

impl Default for Save {
    fn default() -> Save {
        Save {
            id: 0,
            path: String::new(),
            name: String::new(),
            daw: String::new(),
            folder: String::new(),
            size: 0,
            modified: NaiveDateTime::default(),
            mtime: 0,
            tempo: None,
            version: String::new(),
            confidence: Confidence::Full,
            file_missing: false,
            error: None,
            last_opened: None,
            plugin_count: 0,
            sample_count: 0,
            missing_plugins: 0,
            missing_samples: 0,
            tags: Vec::new(),
            plugin_names: Vec::new(),
            hardware: Vec::new(),
            status: STATUSES[0].to_owned(),
            rating: 0,
            favourite: false,
            musical_key: String::new(),
            detected_key: String::new(),
            created: None,
            note_count: -1,
            length_bars: -1,
            track_count: -1,
            notes: String::new(),
        }
    }
}
