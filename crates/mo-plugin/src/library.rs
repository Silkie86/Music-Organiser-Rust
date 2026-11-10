//! The sounds the panel lists: read from the app's sound library, read-only, on a background thread.
//!
//! The plugin never writes the app's files. It opens the app's own sound library (`sounds-rust.db`, beside the old
//! `sounds.db` in the app's data folder, wherever Settings moved that), reads every sound's path in one quick read,
//! and closes it again at once: a file left open could stop the app from swapping in a new copy of its library on
//! Windows. If the app hasn't made its library yet, the panel says so.
//!
//! The list is shared by every copy of the plugin in the DAW (one per track): the first to open its panel reads it,
//! and the others use the same one while any panel is open.

use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

/// One sound in the list.
pub struct Entry {
    pub path: String,
    /// Where the file's name starts in `path`.
    name_at: usize,
}

impl Entry {
    pub fn new(path: String) -> Entry {
        let name_at = path.rfind(['\\', '/']).map_or(0, |i| i + 1);
        Entry { path, name_at }
    }

    pub fn name(&self) -> &str {
        &self.path[self.name_at..]
    }

    /// The last two folders it's in: "Drums\Kicks".
    pub fn folder(&self) -> &str {
        let folders = self.path[..self.name_at].trim_end_matches(['\\', '/']);
        let mut cut = folders.len();
        for _ in 0..2 {
            match folders[..cut].rfind(['\\', '/']) {
                Some(i) => cut = i,
                None => return folders,
            }
        }
        &folders[cut + 1..]
    }
}

/// Every sound, by file name (ignoring letter case).
pub struct Library {
    pub entries: Vec<Entry>,
}

impl Library {
    pub fn from_paths(paths: impl IntoIterator<Item = String>) -> Library {
        let mut entries: Vec<Entry> = paths.into_iter().filter(|p| !p.is_empty()).map(Entry::new).collect();
        entries.sort_by_cached_key(|e| e.name().to_lowercase());
        Library { entries }
    }

    /// The places in [`Library::entries`] of the sounds whose name or folder holds `words` (every word, any case).
    pub fn search(&self, words: &str) -> Vec<u32> {
        let words: Vec<String> = words.split_whitespace().map(str::to_lowercase).collect();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                words.is_empty() || {
                    let path = e.path.to_lowercase();
                    words.iter().all(|w| path.contains(w.as_str()))
                }
            })
            .filter_map(|(i, _)| u32::try_from(i).ok())
            .collect()
    }
}

/// How reading the library went.
pub enum Read {
    Ready(Arc<Library>),
    /// Nothing to read, or it couldn't be read: why, in plain words.
    Not(String),
}

static SHARED: Mutex<Weak<Library>> = Mutex::new(Weak::new());

/// The sound library, read once for every copy of the plugin while any of them holds it. Slow the first time
/// (call it off the panel's thread).
pub fn read() -> Read {
    if let Some(library) = SHARED.lock().unwrap_or_else(|p| p.into_inner()).upgrade() {
        return Read::Ready(library);
    }
    let read = read_from(&mo_app::paths::data_dir().join("sounds.db"));
    if let Read::Ready(library) = &read {
        *SHARED.lock().unwrap_or_else(|p| p.into_inner()) = Arc::downgrade(library);
    }
    read
}

/// The app's own sound library beside `sounds_db` (its copy: the plugin never reads the old file).
pub fn read_from(sounds_db: &Path) -> Read {
    let copy = mo_sounds::compact::copy_of(sounds_db);
    if !copy.is_file() {
        return Read::Not("No sound library yet: open the Sounds page in the app once to make it".into());
    }
    let db = match mo_sounds::SoundsDb::open_read_only(&copy) {
        Ok(db) if db.is_copy() => db,
        Ok(_) => return Read::Not("The sound library isn't one this plugin can read".into()),
        Err(e) => return Read::Not(format!("Couldn't open the sound library: {e}")),
    };
    // One read, then the file is let go (`db` drops here).
    match db.known_files() {
        Ok(files) => Read::Ready(Arc::new(Library::from_paths(files.into_iter().map(|f| f.path)))),
        Err(e) => Read::Not(format!("Couldn't read the sound library: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_folders() {
        let e = Entry::new(r"D:\Samples\Drums\Kicks\Kick 01.wav".into());
        assert_eq!(e.name(), "Kick 01.wav");
        assert_eq!(e.folder(), r"Drums\Kicks");
        let e = Entry::new("/home/s/kick.wav".into());
        assert_eq!(e.folder(), "home/s");
        let e = Entry::new("kick.wav".into());
        assert_eq!((e.name(), e.folder()), ("kick.wav", ""));
        let e = Entry::new(r"C:\kick.wav".into());
        assert_eq!(e.folder(), "C:");
    }

    #[test]
    fn sorted_by_name_and_searched_by_every_word() {
        let lib = Library::from_paths([
            r"D:\Drums\snare B.wav".to_owned(),
            r"D:\Drums\Kick A.wav".to_owned(),
            r"D:\Bass\sub kick.wav".to_owned(),
            String::new(),
        ]);
        let names: Vec<_> = lib.entries.iter().map(Entry::name).collect();
        assert_eq!(names, ["Kick A.wav", "snare B.wav", "sub kick.wav"]);
        assert_eq!(lib.search(""), vec![0, 1, 2]);
        assert_eq!(lib.search("KICK"), vec![0, 2]);
        assert_eq!(lib.search("drums kick"), vec![0]);
    }

    #[test]
    fn no_library_yet_says_so() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-folder");
        assert!(matches!(read_from(&dir.join("sounds.db")), Read::Not(why) if why.starts_with("No sound library")));
    }
}
