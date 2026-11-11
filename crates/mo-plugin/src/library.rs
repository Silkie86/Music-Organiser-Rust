//! The sounds the panel lists: read from the app's sound library, read-only, on a background thread.
//!
//! The plugin never writes the app's files. It opens the app's own sound library (`sounds-rust.db`, beside the old
//! `sounds.db` in the app's data folder, wherever Settings moved that), reads every sound in one read (leaving out the ones
//! hidden as duplicates, as the app's lists do), and closes it again at once: a file left open could stop the app from swapping in a new copy of its library on
//! Windows. If the app hasn't made its library yet, the panel says so.
//!
//! The list is shared by every copy of the plugin in the DAW (one per track): the first to open its panel reads it,
//! and the others use the same one while any panel is open.

use crate::threads::lock;
use mo_app::sounds::{self, Filters, Length, Order, SoundCounts, SoundList};
use mo_sounds::Sound;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

/// How many types the filter column lists, the most used first.
pub const TYPES_SHOWN: usize = 12;

/// The orders the panel offers, as the app names them.
pub const ORDERS: [Order; 4] = [Order::Shuffled, Order::MostLiked, Order::Name, Order::Newest];

/// A file's name in its path.
pub fn name(path: &str) -> &str {
    &path[path.rfind(['\\', '/']).map_or(0, |i| i + 1)..]
}

/// The last two folders a file is in: "Drums\Kicks".
pub fn folder(path: &str) -> &str {
    let folders = path[..path.len() - name(path).len()].trim_end_matches(['\\', '/']);
    let mut cut = folders.len();
    for _ in 0..2 {
        match folders[..cut].rfind(['\\', '/']) {
            Some(i) => cut = i,
            None => return folders,
        }
    }
    &folders[cut + 1..]
}

/// What a line says after the folder: the sound's type, tempo, key and length, as far as the app knows them.
pub fn facts(sound: &Sound) -> String {
    let mut facts: Vec<String> = Vec::new();
    let kind = sound.sound_type.as_str();
    if !kind.is_empty() && !kind.eq_ignore_ascii_case(mo_sounds::types::UNKNOWN) {
        facts.push(kind.to_owned());
    }
    if let Some(analysis) = &sound.analysis {
        if let Some(bpm) = analysis.bpm.filter(|_| analysis.is_loop) {
            facts.push(sounds::bpm_text(bpm));
        }
        let key = sounds::key_text(sound);
        if !key.is_empty() {
            facts.push(key);
        }
        facts.push(sounds::length_text(analysis.seconds));
    }
    facts.join("  \u{b7}  ")
}

/// The app's sound library as the app's own Sounds page holds it (`mo_app::sounds::SoundList`), so the plugin filters,
/// searches and orders exactly as the app does.
pub struct Library {
    pub list: SoundList,
    /// The types, the most used first, with how many sounds each has.
    pub types: Vec<(String, usize)>,
}

impl Library {
    /// Sounds hidden as duplicates stay out, as in the app's lists (rule 2: their files are still there).
    pub fn new(sounds: Vec<Sound>) -> Library {
        let mut list = SoundList::new(sounds.into_iter().filter(|s| !s.hidden && !s.path.is_empty()).collect());
        // Most liked, until the plugin reads how songs use each sound: favourites first, then the most played.
        list.set_liked(|s| f64::from(u8::from(s.favourite)) * 1000.0 + f64::from(s.plays));
        let types = list.type_counts();
        Library { list, types }
    }

    /// The places in the list of the sounds `asked` lets through, in its order, and how many each filter choice would
    /// show. Worked out on the app's low-priority pool (never the DAW's threads).
    pub fn view(&self, asked: &Asked) -> (Vec<usize>, SoundCounts) {
        mo_app::background::on_pool(|| self.list.view_counted(&asked.filters(), asked.order, asked.seed))
    }
}

/// What the panel asks of the list: its search box, filter column and order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub words: String,
    pub types: Vec<String>,
    pub length: Length,
    pub favourites: bool,
    pub order: Order,
    /// Picks the shuffled order: a new one each time the panel opens or Shuffled is picked again.
    pub seed: u64,
}

impl Asked {
    pub fn filters(&self) -> Filters {
        Filters {
            search: self.words.trim().to_owned(),
            sound_types: self.types.clone(),
            length: self.length,
            favourites: self.favourites,
            ..Filters::default()
        }
    }

    /// Whether anything narrows the list: a filter, or words in the search box.
    pub fn any(&self) -> bool {
        self.filters().any() || !self.words.trim().is_empty()
    }

    /// Picks a type, or takes it off again.
    pub fn toggle_type(&mut self, kind: &str) {
        match self.types.iter().position(|t| t.eq_ignore_ascii_case(kind)) {
            Some(at) => drop(self.types.remove(at)),
            None => self.types.push(kind.to_owned()),
        }
    }

    /// Picks Hits or Loops, or both again when it's already picked.
    pub fn toggle_length(&mut self, length: Length) {
        self.length = if self.length == length { Length::All } else { length };
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
    if let Some(library) = lock(&SHARED).upgrade() {
        return Read::Ready(library);
    }
    let read = read_from(&mo_app::paths::data_dir().join("sounds.db"));
    if let Read::Ready(library) = &read {
        *lock(&SHARED) = Arc::downgrade(library);
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
    match db.all(chrono::Local::now().naive_local()) {
        Ok(sounds) => Read::Ready(Arc::new(Library::new(sounds))),
        Err(e) => Read::Not(format!("Couldn't read the sound library: {e}")),
    }
}

/// The waveforms of the sounds with these ids, for the lines on show: the library is opened, read and let go again,
/// as [`read`] does. Slow (call it off the panel's thread).
pub fn waves(ids: &[i64]) -> Vec<(i64, mo_sounds::Peaks)> {
    waves_from(&mo_app::paths::data_dir().join("sounds.db"), ids)
}

/// [`waves`] from the app's own sound library beside `sounds_db`. None when it can't be read just now (the app may be
/// swapping in a new copy): those lines stay without a waveform.
pub fn waves_from(sounds_db: &Path, ids: &[i64]) -> Vec<(i64, mo_sounds::Peaks)> {
    let copy = mo_sounds::compact::copy_of(sounds_db);
    if ids.is_empty() || !copy.is_file() {
        return Vec::new();
    }
    mo_sounds::SoundsDb::open_read_only(&copy).and_then(|db| db.waves(ids)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(id: i64, path: &str, kind: &str, favourite: bool) -> Sound {
        let mut s = Sound::new(id, path);
        s.sound_type = mo_sounds::Word::new(kind);
        s.favourite = favourite;
        s
    }

    fn library() -> Library {
        let mut hidden = sound(4, r"D:\Drums\Kick A copy.wav", "Kick", false);
        hidden.hidden = true;
        Library::new(vec![
            sound(1, r"D:\Drums\snare B.wav", "Snare", false),
            sound(2, r"D:\Drums\Kick A.wav", "Kick", true),
            sound(3, r"D:\Bass\sub kick.wav", "Bass", false),
            hidden,
        ])
    }

    fn names(lib: &Library, asked: &Asked) -> Vec<String> {
        let (shown, _) = lib.view(asked);
        shown.iter().filter_map(|&i| lib.list.sound(i)).map(|s| name(&s.path).to_owned()).collect()
    }

    #[test]
    fn names_and_folders() {
        let path = r"D:\Samples\Drums\Kicks\Kick 01.wav";
        assert_eq!((name(path), folder(path)), ("Kick 01.wav", r"Drums\Kicks"));
        assert_eq!(folder("/home/s/kick.wav"), "home/s");
        assert_eq!((name("kick.wav"), folder("kick.wav")), ("kick.wav", ""));
        assert_eq!(folder(r"C:\kick.wav"), "C:");
    }

    #[test]
    fn filtered_searched_and_ordered_as_the_app_does() {
        let lib = library();
        let by_name = Asked { order: Order::Name, ..Asked::default() };
        // The hidden duplicate stays out.
        assert_eq!(names(&lib, &by_name), ["Kick A.wav", "snare B.wav", "sub kick.wav"]);
        assert!(!by_name.any());
        let kick = Asked { words: " KICK ".into(), ..by_name.clone() };
        assert_eq!(names(&lib, &kick), ["Kick A.wav", "sub kick.wav"]);
        assert!(kick.any());
        let mut typed = by_name.clone();
        typed.toggle_type("Kick");
        typed.toggle_type("snare");
        assert_eq!(names(&lib, &typed), ["Kick A.wav", "snare B.wav"]);
        typed.toggle_type("KICK");
        assert_eq!(names(&lib, &typed), ["snare B.wav"]);
        assert_eq!(names(&lib, &Asked { favourites: true, ..by_name.clone() }), ["Kick A.wav"]);
        // Most liked: the favourite first.
        assert_eq!(names(&lib, &Asked { order: Order::MostLiked, ..by_name.clone() })[0], "Kick A.wav");
        // Each type's count is what picking it would show.
        let (_, counts) = lib.view(&kick);
        assert_eq!(
            (counts.types.get("kick"), counts.types.get("bass"), counts.types.get("snare")),
            (Some(&1), Some(&1), None)
        );
        assert_eq!(lib.types.len(), 3);
    }

    #[test]
    fn hits_or_loops_or_both() {
        let mut asked = Asked::default();
        asked.toggle_length(Length::Loops);
        assert_eq!(asked.length, Length::Loops);
        asked.toggle_length(Length::Hits);
        assert_eq!(asked.length, Length::Hits);
        asked.toggle_length(Length::Hits);
        assert_eq!(asked.length, Length::All);
    }

    #[test]
    fn a_line_says_what_the_app_knows() {
        assert_eq!(facts(&sound(1, "a.wav", "Kick", false)), "Kick");
        assert_eq!(facts(&Sound::new(1, "a.wav")), "");
    }

    #[test]
    fn no_library_yet_says_so() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-folder");
        assert!(matches!(read_from(&dir.join("sounds.db")), Read::Not(why) if why.starts_with("No sound library")));
        assert!(waves_from(&dir.join("sounds.db"), &[1, 2]).is_empty());
    }
}
