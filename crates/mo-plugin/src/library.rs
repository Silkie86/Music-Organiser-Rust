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
use mo_app::sounds::{self, Filters, LikeFavourites, Order, SoundChips, SoundCounts, SoundList};
use mo_sounds::Sound;
use mo_sounds::index::SoundIndex;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

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
    /// Every tag on a sound, sorted.
    pub tags: Vec<String>,
    /// How many sounds were added in each year, newest first: the year strip.
    pub years: Vec<(i32, usize)>,
    /// The app's sound library it was read from (its old `sounds.db` beside it), for the fingerprints read later
    /// ([`Library::read_like`]), and how many sounds CLAP could have heard then (as the app counts them, hidden copies
    /// too), which says whether its fingerprints are the ones to use.
    from: Option<(PathBuf, usize)>,
    /// How close each sound is to your favourites, once worked out (or why it couldn't be).
    like: Mutex<Like>,
}

/// Sounds like your favourites, in the plugin: worked out once, the first time they're asked for.
#[derive(Clone, Default)]
pub enum Like {
    #[default]
    NotYet,
    Reading,
    Ready(Arc<LikeFavourites>),
    /// Why it couldn't be, in plain words.
    Not(String),
}

impl Library {
    /// Sounds hidden as duplicates stay out, as in the app's lists (rule 2: their files are still there), and so do
    /// the files programs leave in a temp folder, by the app's own rule (`SoundList::new`, with
    /// `mo_sounds::scan::in_temp_folder`).
    pub fn new(sounds: Vec<Sound>) -> Library {
        let mut list = SoundList::new(sounds.into_iter().filter(|s| !s.hidden && !s.path.is_empty()).collect());
        // Most liked, until the plugin reads how songs use each sound: favourites first, then the most played.
        list.set_liked(|s| f64::from(u8::from(s.favourite)) * 1000.0 + f64::from(s.plays));
        let types = list.type_counts();
        let tags = list.tags();
        // By the year each sound was added (the plugin doesn't read which songs use a sound).
        let years = list.year_counts(false);
        Library { list, types, tags, years, from: None, like: Mutex::default() }
    }

    /// The places in the list of the sounds `asked` lets through, in its order, and how many each filter choice would
    /// show. Worked out on the app's low-priority pool (never the DAW's threads). With Favourites on and sounds like
    /// them asked for, those show too once they're known ([`Library::read_like`]).
    pub fn view(&self, asked: &Asked) -> (Vec<usize>, SoundCounts) {
        let like = match self.like() {
            Like::Ready(like) if asked.filters.wants_like_favourites() => Some(like),
            _ => None,
        };
        mo_app::background::on_pool(|| {
            let list = like.map(|like| self.list.with_like_favourites(like));
            list.as_ref().unwrap_or(&self.list).view_counted(&asked.filters, asked.order, asked.seed)
        })
    }

    /// How far working out the sounds like your favourites has got.
    pub fn like(&self) -> Like {
        lock(&self.like).clone()
    }

    /// Whether the sounds like your favourites should be worked out now for `asked`: they're asked for and haven't
    /// been yet. Says so once: it's then under way ([`Library::read_like`] does it).
    pub fn start_like(&self, asked: &Asked) -> bool {
        let mut like = lock(&self.like);
        let start = asked.filters.wants_like_favourites() && matches!(*like, Like::NotYet);
        if start {
            *like = Like::Reading;
        }
        start
    }

    /// Works out how close each sound is to your favourites, from the sounds' fingerprints in the app's library: read
    /// once (it's opened, read and let go again, as [`read`] does), and let go once it's worked out, so only the
    /// answer (a number for each sound) is kept. Slow: seconds for a big library, and the fingerprints take a few
    /// hundred megabytes while it works (call it off the panel's thread).
    pub fn read_like(&self) {
        let started = std::time::Instant::now();
        let like = match &self.from {
            Some((sounds_db, analysed)) => match fingerprints(sounds_db, *analysed) {
                Ok(index) => Like::Ready(mo_app::background::on_pool(|| self.list.like_favourites_in(&index))),
                Err(why) => Like::Not(why),
            },
            None => Like::Not("No sound library to read".into()),
        };
        let said = match &like {
            Like::Not(why) => format!("Sounds like your favourites not worked out: {why}"),
            _ => "Worked out the sounds like your favourites".to_owned(),
        };
        crate::log::took("Library", &said, started);
        *lock(&self.like) = like;
    }
}

/// The sounds' fingerprints in the app's own sound library beside `sounds_db`, made into the index "Sounds like this"
/// searches, as the app makes it.
fn fingerprints(sounds_db: &Path, analysed: usize) -> Result<SoundIndex, String> {
    let copy = mo_sounds::compact::copy_of(sounds_db);
    let db = match mo_sounds::SoundsDb::open_read_only(&copy) {
        Ok(db) if db.is_copy() => db,
        Ok(_) => return Err("the sound library isn't one this plugin can read".into()),
        Err(e) => return Err(format!("couldn't open the sound library: {e}")),
    };
    let (features, clap) = db.fingerprints(analysed).map_err(|e| format!("couldn't read the fingerprints: {e}"))?;
    drop(db);
    Ok(mo_app::background::on_pool(|| SoundIndex::from_gathered(features, clap, analysed)))
}

/// What the panel asks of the list: the app's own filters (with the search box's words), and the order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub filters: Filters,
    pub order: Order,
    /// Picks the shuffled order: a new one each time the panel opens or Shuffled is picked again.
    pub seed: u64,
}

impl Asked {
    /// Whether anything narrows the list: a filter, or words in the search box.
    pub fn any(&self) -> bool {
        self.filters.any() || !self.filters.search.trim().is_empty()
    }

    /// The filter column as the app's Sounds page shows it: each choice with how many sounds it would show
    /// (`counts`, once a list has been worked out), over `library`'s types and tags (none until it's read).
    /// The first `type_lines` types are lines of their own (as many as the column has room for), the rest in the
    /// More types menu.
    pub fn chips(&self, library: Option<&Library>, counts: Option<&SoundCounts>, type_lines: usize) -> SoundChips {
        let (types, tags) = library.map_or((&[][..], &[][..]), |l| (&l.types[..], &l.tags[..]));
        sounds::chips_showing(&self.filters, counts, types, tags, type_lines)
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
    // One read at a time: a second asker (another copy of the plugin, or a window made again) waits for the first
    // and takes what it read.
    static READING: Mutex<()> = Mutex::new(());
    let _reading = lock(&READING);
    if let Some(library) = lock(&SHARED).upgrade() {
        return Read::Ready(library);
    }
    let started = std::time::Instant::now();
    let read = read_from(&mo_app::paths::data_dir().join("sounds.db"));
    match &read {
        Read::Ready(library) => {
            *lock(&SHARED) = Arc::downgrade(library);
            crate::log::took("Library", &format!("Read {} sounds", library.list.len()), started);
        }
        Read::Not(why) => crate::log::took("Library", &format!("Not read: {why}"), started),
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
        Ok(sounds) => {
            let analysed = mo_sounds::count_clap_could_hear(&sounds, mo_sounds::compact::grow::new_since(&copy));
            let mut library = Library::new(sounds);
            library.from = Some((sounds_db.to_path_buf(), analysed));
            Read::Ready(Arc::new(library))
        }
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
    let started = std::time::Instant::now();
    match mo_sounds::SoundsDb::open_read_only(&copy).and_then(|db| db.waves(ids)) {
        Ok(found) => {
            crate::log::took_if_slow("Waveforms", &format!("Read {} of {} asked for", found.len(), ids.len()), started);
            found
        }
        Err(e) => {
            crate::log::took("Waveforms", &format!("Couldn't read {} from the library: {e}", ids.len()), started);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn files_in_a_temp_folder_stay_out() {
        // FL Studio's blank render, an older scan's closed hat, stays out; a folder of his own called Temp is his.
        let mut sounds = vec![
            Sound::new(1, r"C:\Users\silki\AppData\Local\Temp\Image-Line\a.wav"),
            Sound::new(2, r"J:\Samples\Temp\Kick.wav"),
        ];
        sounds[0].sound_type = mo_sounds::Word::new("Closed hat");
        let library = Library::new(sounds);
        assert_eq!(library.list.len(), 1);
        assert_eq!(library.list.sound(0).map(|s| s.id), Some(2));
    }

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
        let mut kick = by_name.clone();
        kick.filters.search = "KICK".into();
        assert_eq!(names(&lib, &kick), ["Kick A.wav", "sub kick.wav"]);
        assert!(kick.any());
        let mut typed = by_name.clone();
        typed.filters.pick(sounds::TYPE, "Kick");
        typed.filters.pick(sounds::TYPE, "snare");
        assert_eq!(names(&lib, &typed), ["Kick A.wav", "snare B.wav"]);
        typed.filters.pick(sounds::TYPE, "KICK");
        assert_eq!(names(&lib, &typed), ["snare B.wav"]);
        let mut liked = by_name.clone();
        liked.filters.toggle("favourites");
        assert_eq!(names(&lib, &liked), ["Kick A.wav"]);
        // Most liked: the favourite first.
        assert_eq!(names(&lib, &Asked { order: Order::MostLiked, ..by_name.clone() })[0], "Kick A.wav");
        assert_eq!(lib.types.len(), 3);
    }

    #[test]
    fn the_filter_column_counts_what_each_choice_would_show() {
        let lib = library();
        let mut kick = Asked::default();
        kick.filters.search = "kick".into();
        // Before the library is read: no types, the keys and lengths without numbers.
        let before = kick.chips(None, None, sounds::TYPE_CHIPS);
        assert!(before.types.chips.is_empty() && before.types.more.is_empty());
        assert_eq!(before.tempo.words, "Any tempo");
        let (_, counts) = lib.view(&kick);
        let chips = kick.chips(Some(&lib), Some(&counts), sounds::TYPE_CHIPS);
        let count = |kind: &str| chips.types.chips.iter().find(|c| c.text == kind).map(|c| c.count.clone());
        assert_eq!(
            (count("Kick"), count("Bass"), count("Snare")),
            (Some("1".into()), Some("1".into()), Some("0".into()))
        );
        kick.filters.pick(sounds::TYPE, "Kick");
        assert!(kick.chips(Some(&lib), Some(&counts), 1).types.chips.iter().any(|c| c.text == "Kick" && c.on));
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
