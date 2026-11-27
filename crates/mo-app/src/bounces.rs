//! A song's bounces: the sound files you exported of it (a mixdown, a render, an MP3 to send round), found beside its
//! saves. Solomon's 1.1 plan (24 Nov 2026), brought into v1 as an Experimental part (25 Nov): the Songs list marks a
//! song that has one, its card lists them, newest first, and the player bar plays them.
//!
//! A bounce is a sound file whose name starts with the song's name, or one of its saves' names ("Night Drive
//! (final).wav", "Night Drive_9.mp3"), in a folder one of its saves is in, or in a folder there named for exports
//! ("Exports", "Renders", "Mixdown"...). Nothing is read from inside the files and nothing is kept: the folders are
//! listed again each time (at the start, and when a song's card opens), so a bounce made a minute ago shows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::songs::SongRow;

/// A sound file exported of a song.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bounce {
    pub path: String,
    /// Its file name, to show.
    pub name: String,
    /// When it was last written (Unix seconds).
    pub mtime: i64,
}

/// The folders beside a song's saves that DAWs and people export into, by their name in lower case.
pub const FOLDERS: [&str; 15] = [
    "export",
    "exports",
    "exported",
    "render",
    "renders",
    "rendered",
    "bounce",
    "bounces",
    "mixdown",
    "mixdowns",
    "master",
    "masters",
    "mixes",
    "audio exports",
    "renders and exports",
];

/// The kinds of sound file a bounce can be (the app can play every one).
const ENDINGS: [&str; 7] = ["wav", "flac", "mp3", "ogg", "aif", "aiff", "m4a"];

/// The sound files in folders, listed once each however many songs share a folder.
#[derive(Default)]
pub struct Folders {
    listed: HashMap<PathBuf, Vec<Bounce>>,
}

impl Folders {
    /// The bounces of the song `row`, newest first. `rivals`: the names of the other songs saved in its folders
    /// ([`rivals`]), so a file named for a longer one ("Night Drive.wav" beside "Night" and "Night Drive") goes to it.
    pub fn of(&mut self, row: &SongRow, rivals: &[String]) -> Vec<Bounce> {
        self.of_song(&BounceSong::of(row), rivals)
    }

    /// [`Folders::of`] for a song as [`songs`] gives it.
    fn of_song(&mut self, song: &BounceSong, rivals: &[String]) -> Vec<Bounce> {
        let names = &song.names;
        if names.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<Bounce> = Vec::new();
        for folder in &song.folders {
            for bounce in self.files(folder) {
                let stem = Path::new(&bounce.name).file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
                let Some(own) = names.iter().filter(|n| starts_with_name(&stem, n)).map(String::len).max() else {
                    continue;
                };
                let taken = rivals.iter().any(|r| r.len() > own && starts_with_name(&stem, r));
                if !taken && !found.iter().any(|f| f.path == bounce.path) {
                    found.push(bounce.clone());
                }
            }
        }
        found.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
        found
    }

    /// The sound files in `folder` and in the export folders inside it.
    fn files(&mut self, folder: &Path) -> &[Bounce] {
        self.listed.entry(folder.to_owned()).or_insert_with(|| {
            let mut files = Vec::new();
            for entry in entries(folder) {
                if entry.is_dir {
                    let name = entry.path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                    if FOLDERS.contains(&name.as_str()) {
                        files.extend(entries(&entry.path).into_iter().filter_map(sound));
                    }
                } else if let Some(bounce) = sound(entry) {
                    files.push(bounce);
                }
            }
            files
        })
    }
}

/// What finding a song's bounces needs of it: a few names and folders, not the whole song with every save (the list
/// isn't copied to look for them on a thread of its own).
#[derive(Clone, Debug, Default)]
pub struct BounceSong {
    key: String,
    /// Its name, plain: what the other songs in its folders are told apart by.
    plain: String,
    /// [`names_of`].
    names: Vec<String>,
    /// [`folders_of`].
    folders: Vec<PathBuf>,
}

impl BounceSong {
    fn of(row: &SongRow) -> BounceSong {
        BounceSong {
            key: row.song.key.clone(),
            plain: plain(&row.song.name),
            names: names_of(row),
            folders: folders_of(row),
        }
    }
}

/// What [`find_for`] needs of each song in `rows`.
pub fn songs(rows: &[SongRow]) -> Vec<BounceSong> {
    rows.iter().map(BounceSong::of).collect()
}

/// The bounces of every song, by its key: only songs that have some.
pub fn find(rows: &[SongRow]) -> HashMap<String, Vec<Bounce>> {
    find_for(&songs(rows))
}

/// [`find`] for the songs as [`songs`] gives them.
pub fn find_for(songs: &[BounceSong]) -> HashMap<String, Vec<Bounce>> {
    // The songs saved in each folder, by their plain names: a song's rivals.
    let mut in_folder: HashMap<&Path, Vec<(usize, &str)>> = HashMap::new();
    for (i, song) in songs.iter().enumerate() {
        for folder in &song.folders {
            in_folder.entry(folder).or_default().push((i, &song.plain));
        }
    }
    let mut folders = Folders::default();
    songs
        .iter()
        .enumerate()
        .filter_map(|(i, song)| {
            let rivals: Vec<String> = song
                .folders
                .iter()
                .flat_map(|f| in_folder.get(f.as_path()).into_iter().flatten())
                .filter(|(j, _)| *j != i)
                .map(|(_, n)| (*n).to_owned())
                .collect();
            let found = folders.of_song(song, &rivals);
            (!found.is_empty()).then(|| (song.key.clone(), found))
        })
        .collect()
}

/// The names of the other songs in `rows` saved in a folder `row` is saved in, for [`Folders::of`].
pub fn rivals(rows: &[SongRow], row: &SongRow) -> Vec<String> {
    let mine = folders_of(row);
    rows.iter()
        .filter(|r| {
            r.song.key != row.song.key && r.song.saves().any(|s| mine.iter().any(|f| *f == Path::new(&s.folder)))
        })
        .map(|r| plain(&r.song.name))
        .collect()
}

/// The names a bounce of `row` can start with, plain: the song's, and each of its saves'.
fn names_of(row: &SongRow) -> Vec<String> {
    let mut names: Vec<String> = std::iter::once(row.song.name.as_str())
        .chain(row.song.saves().map(|s| s.name.as_str()))
        .map(plain)
        .filter(|n| n.chars().count() >= 3)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The folders `row`'s saves are in, each once.
fn folders_of(row: &SongRow) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> =
        row.song.saves().filter(|s| !s.folder.is_empty()).map(|s| PathBuf::from(&s.folder)).collect();
    folders.sort();
    folders.dedup();
    folders
}

/// Whether the file name `stem` starts with the plain name `name` as a whole: "Night Drive (final)" starts with
/// "nightdrive", but "Track 12" doesn't start with "track1", nor "Bassline" with "bass".
fn starts_with_name(stem: &str, name: &str) -> bool {
    let mut got = String::new();
    let mut chars = stem.chars().peekable();
    while got.len() < name.len() {
        let Some(c) = chars.next() else { return false };
        if c.is_alphanumeric() {
            got.extend(c.to_lowercase());
            if !name.starts_with(got.as_str()) {
                return false;
            }
        }
    }
    got == name && chars.peek().is_none_or(|c| !c.is_alphanumeric())
}

/// A thing in a folder: its path, and whether it's a folder.
struct Entry {
    path: PathBuf,
    is_dir: bool,
    /// When it was last written, from the listing itself (no file is opened for it on Windows).
    mtime: i64,
}

/// What's in `folder`; nothing when it can't be listed. A link isn't followed.
fn entries(folder: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(folder) else { return Vec::new() };
    read.flatten()
        .filter_map(|e| {
            let kind = e.file_type().ok()?;
            if !kind.is_dir() && !kind.is_file() {
                return None;
            }
            let mtime = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
            Some(Entry { path: e.path(), is_dir: kind.is_dir(), mtime })
        })
        .collect()
}

/// The file as a bounce, when it's a sound file.
fn sound(entry: Entry) -> Option<Bounce> {
    if entry.is_dir {
        return None;
    }
    let ending = entry.path.extension()?.to_string_lossy().to_lowercase();
    if !ENDINGS.contains(&ending.as_str()) {
        return None;
    }
    Some(Bounce {
        name: entry.path.file_name()?.to_string_lossy().into_owned(),
        path: entry.path.to_string_lossy().into_owned(),
        mtime: entry.mtime,
    })
}

/// A name with only its letters and digits, in lower case: "Night Drive_9" and "night-drive 9" read alike.
fn plain(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// "1:12" from seconds, for the player bar.
pub fn clock(seconds: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = if seconds.is_finite() { seconds.max(0.0).round() as u64 } else { 0 };
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// Where a bounce `length` seconds long (0 when not known) plays on from after it stopped at `at`: there, unless it was
/// within a second of its end (it played to the end), then its start.
pub fn resume_at(length: f64, at: f64) -> f64 {
    if length <= 0.0 || length - at > 1.0 { at.max(0.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_library::{Save, SongChoices};

    fn folder(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/mo-app-bounces").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, seconds_ago: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"RIFF").unwrap();
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds_ago);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    #[test]
    fn a_songs_bounces_are_its_sound_files_beside_its_saves_and_in_their_export_folders_newest_first() {
        let dir = folder("found");
        let save = |name: &str| Save {
            name: name.into(),
            path: dir.join(format!("{name}.flp")).to_string_lossy().into_owned(),
            folder: dir.to_string_lossy().into_owned(),
            daw: "FL Studio".into(),
            modified: chrono::Local::now().naive_local(),
            ..Save::default()
        };
        touch(&dir.join("Night Drive (final).wav"), 100);
        touch(&dir.join("Exports").join("night-drive 9.MP3"), 50);
        touch(&dir.join("Night Drive_14.flac"), 500);
        // Not sound files, not the song's, or in a folder that isn't for exports.
        touch(&dir.join("Night Drive notes.txt"), 10);
        touch(&dir.join("Kick 01.wav"), 10);
        touch(&dir.join("Samples").join("Night Drive vox.wav"), 10);
        touch(&dir.join("Glass Harbour.wav"), 10);
        let songs = crate::songs::Songs::from_saves(
            &[save("Night Drive_14"), save("Night Drive_13")],
            &SongChoices::default(),
            Vec::new(),
            3,
            Some(false),
        );
        let found = find(&songs.rows);
        let names: Vec<&str> = found.values().flatten().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["night-drive 9.MP3", "Night Drive (final).wav", "Night Drive_14.flac"]);
        // Looked for again on its own (its card), the same.
        let again = Folders::default().of(&songs.rows[0], &rivals(&songs.rows, &songs.rows[0]));
        assert_eq!(again.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), names);
    }

    #[test]
    fn a_folder_that_isnt_there_has_no_bounces() {
        let row = crate::songs::Songs::from_saves(
            &[Save {
                name: "Gone".into(),
                path: "/nowhere/at/all/Gone.flp".into(),
                folder: "/nowhere/at/all".into(),
                ..Save::default()
            }],
            &SongChoices::default(),
            Vec::new(),
            3,
            Some(false),
        );
        assert!(find(&row.rows).is_empty());
    }

    #[test]
    fn a_file_goes_to_the_song_whose_whole_name_it_starts_with_the_longest() {
        let dir = folder("rivals");
        let save = |name: &str| Save {
            name: name.into(),
            path: dir.join(format!("{name}.flp")).to_string_lossy().into_owned(),
            folder: dir.to_string_lossy().into_owned(),
            daw: "FL Studio".into(),
            modified: chrono::Local::now().naive_local(),
            ..Save::default()
        };
        for file in ["Night Drive (final).wav", "Night.wav", "Kite Season mix.wav", "Kite.wav", "Bassline.wav"] {
            touch(&dir.join(file), 10);
        }
        let songs = crate::songs::Songs::from_saves(
            &[save("Night"), save("Night Drive"), save("Kite"), save("Kite Season"), save("Bass")],
            &SongChoices::default(),
            Vec::new(),
            3,
            Some(false),
        );
        let found = find(&songs.rows);
        let of = |song: &str| {
            let row = songs.rows.iter().find(|r| r.song.name == song).unwrap();
            let mut names: Vec<&str> =
                found.get(&row.song.key).into_iter().flatten().map(|b| b.name.as_str()).collect();
            names.sort_unstable();
            names
        };
        assert_eq!(of("Night"), ["Night.wav"]);
        assert_eq!(of("Night Drive"), ["Night Drive (final).wav"]);
        assert_eq!(of("Kite"), ["Kite.wav"]);
        assert_eq!(of("Kite Season"), ["Kite Season mix.wav"]);
        assert!(of("Bass").is_empty(), "Bassline isn't Bass's");
    }

    #[test]
    fn the_player_bar_plays_on_from_where_it_stopped_unless_that_was_its_end() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(72.4), "1:12");
        assert_eq!(clock(222.0), "3:42");
        assert_eq!(clock(-3.0), "0:00");
        assert_eq!(clock(f64::NAN), "0:00");
        assert!((resume_at(222.0, 72.0) - 72.0).abs() < 1e-9);
        assert!(resume_at(222.0, 221.5).abs() < 1e-9, "at its end: from the start");
        assert!((resume_at(0.0, 10.0) - 10.0).abs() < 1e-9, "length unknown: from where it stopped");
    }

    #[test]
    fn names_read_alike_whatever_their_marks() {
        assert_eq!(plain("Night Drive_9"), "nightdrive9");
        assert_eq!(plain("night-drive (9)"), "nightdrive9");
    }
}
