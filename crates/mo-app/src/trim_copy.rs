//! Trimmed copies for dragging into a DAW: with Trim silence on, a sound dragged out of the app or the plugin goes as a
//! copy without its quiet start and end (`mo_audio::trim`), so the DAW gets what you heard (Solomon, 12 Nov 2026).
//!
//! The original is never changed or moved (rule 2). Copies go in `Trimmed sounds` in the export folder (Settings, or
//! "Music Organiser Exports" in Documents), beside the app's other exports: they're yours, as DAW projects go on using
//! them, so the app never tidies them away and a data-folder move leaves them where they are. One copy for each sound
//! and level, named by `mo_sounds::trimmed::copy_name` (so the Sounds page still counts a project using the copy as
//! using the original): the next drag reuses it, and an original changed since gets it made again.
//!
//! Loops go as they are: a loop's silence is part of its groove and its bar length. So do sounds with too little
//! silence to hear, long ones, and ones that go over full scale (a 24-bit copy would clip them).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// The export folder's folder for trimmed copies.
pub const FOLDER: &str = "Trimmed sounds";

/// The longest sound a copy is made of: a longer one is dragged as it is (it's read whole, and in the plugin that's
/// in the DAW's own memory).
const MOST_SECONDS: f64 = 5.0 * 60.0;

/// The least silence worth a copy, start and end together: less (an MP3's encoder delay, a few ms of padding) can't be
/// heard, and the original goes.
const LEAST_CUT_SECONDS: f64 = 0.020;

/// How many "goes as it is" answers are remembered for this run, so dragging the same long or loud sound again doesn't
/// read it whole again (forgotten all at once when full).
const REMEMBERED: usize = 4_096;

/// What to drag for the sound at `original` with Trim silence at `db` below full scale (`None`: off), and what the
/// log should hear, if anything: the trimmed copy, or the original when no copy is wanted or can be made. `is_loop`
/// is what the library knows of it. Reads the library's settings and maybe the whole file: never on the window's
/// thread.
pub fn to_drag(original: &Path, db: Option<i32>, is_loop: bool) -> (PathBuf, Option<String>) {
    let Some(db) = db.filter(|_| !is_loop) else { return (original.to_path_buf(), None) };
    let folder = match folder_now() {
        Ok(folder) => folder,
        Err(why) => return (original.to_path_buf(), Some(format!("Dragged the whole file, untrimmed: {why}"))),
    };
    match for_drag(original, db, &folder) {
        Made::Copy(copy) => (copy, None),
        Made::AsItIs(why) => {
            (original.to_path_buf(), why.map(|why| format!("Dragged the whole file, untrimmed: {why}")))
        }
    }
}

/// How long the copies' folder, once looked up, is used for drags before it's looked up again (a change in Settings
/// reaches drags within this; the song library isn't opened twice for each drag in between).
const FOLDER_KEPT: std::time::Duration = std::time::Duration::from_secs(30);

/// The copies' folder this run, and when it was looked up.
static FOLDER_NOW: Mutex<Option<(std::time::Instant, Result<PathBuf, String>)>> = Mutex::new(None);

/// [`folder_in`] for the data folder in use, looked up again at most every [`FOLDER_KEPT`].
fn folder_now() -> Result<PathBuf, String> {
    let mut now = FOLDER_NOW.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match &*now {
        Some((at, folder)) if at.elapsed() < FOLDER_KEPT => folder.clone(),
        _ => {
            let folder = folder_in(&crate::paths::data_dir());
            *now = Some((std::time::Instant::now(), folder.clone()));
            folder
        }
    }
}

/// Where trimmed copies go for the data folder `data`: [`FOLDER`] in the export folder set in Settings, read from the
/// app's own song library (songs.db once it's the app's own, library.db before), as every other export reads it
/// (DF-2, 14 Nov review: it read library.db, which the archive step takes away). Opens the library: off the window's
/// thread.
pub fn folder_in(data: &Path) -> Result<PathBuf, String> {
    let library = mo_library::songs_db::to_read(data);
    crate::bitwig_convert::export_folder_in(&library).map(|exports| exports.join(FOLDER))
}

/// Whether `name` is a copy whose making was cut short, in [`FOLDER`]: written under this program's own name first
/// (`<copy>.<pid>.wav`, and `write_wav`'s `<copy>.<pid>.wav.part` beside it) and renamed after (DF-5, for the
/// start-up sweep). Never a copy itself.
pub fn is_half_made(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".wav.part").or_else(|| name.strip_suffix(".wav")) else { return false };
    let Some((copy, pid)) = rest.rsplit_once('.') else { return false };
    !pid.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && mo_sounds::trimmed::mark_in(&format!("{copy}.wav")).is_some()
}

/// What [`for_drag`] gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Made {
    /// The trimmed copy, made now or before.
    Copy(PathBuf),
    /// Drag the original: nothing to trim (`None`), or why it can't or shouldn't be trimmed.
    AsItIs(Option<String>),
}

/// The copies being made, one at a time in this program, and the "goes as it is" answers remembered by file (its
/// path, level, size and date).
static MAKING: Mutex<Option<HashMap<FileKey, Option<String>>>> = Mutex::new(None);

/// A file as [`MAKING`] remembers it: its path (as `mo_formats::case::key` has it), the level, its size and its date.
type FileKey = (String, i32, u64, Option<SystemTime>);

/// [`to_drag`]'s work, with copies kept in `folder`.
pub fn for_drag(original: &Path, db: i32, folder: &Path) -> Made {
    let meta = match std::fs::metadata(original) {
        Ok(m) => m,
        Err(e) => return Made::AsItIs(Some(format!("couldn't look at {}: {e}", original.display()))),
    };
    let path = original.to_string_lossy();
    let copy = folder.join(mo_sounds::trimmed::copy_name(&path, db));
    let changed = meta.modified().ok();
    // Made before, and the original hasn't changed since: a copy carries its original's date (stamped as it's made,
    // so an original dated ahead of now, as a pack's files can be, keeps its copy rather than having it made again at
    // every drag: DF-6). A copy made before copies were stamped is dated when it was made, after its original.
    let fresh = |copy: &Path| {
        std::fs::metadata(copy).is_ok_and(|c| c.is_file() && c.modified().ok().zip(changed).is_none_or(|(c, o)| c >= o))
    };
    if fresh(&copy) {
        return Made::Copy(copy);
    }
    let key = (mo_formats::case::key(&path), db, meta.len(), changed);
    let mut making = MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let remembered = making.get_or_insert_with(HashMap::new);
    if let Some(why) = remembered.get(&key) {
        return Made::AsItIs(why.clone());
    }
    // Another drag may have made it while this one waited.
    if fresh(&copy) {
        return Made::Copy(copy);
    }
    let made = make(original, db, &copy, folder, changed);
    if let Made::AsItIs(why) = &made {
        if remembered.len() >= REMEMBERED {
            remembered.clear();
        }
        remembered.insert(key, why.clone());
    }
    made
}

/// Reads `original` whole, trims it at `db` and writes it to `copy` in `folder`, dated `changed` (the original's
/// date), or says why it goes as it is.
fn make(original: &Path, db: i32, copy: &Path, folder: &Path, changed: Option<SystemTime>) -> Made {
    let as_it_is = |why: String| Made::AsItIs(Some(why));
    let mut audio = match mo_audio::read(original, MOST_SECONDS) {
        Ok(a) => a,
        Err(e) => return as_it_is(format!("couldn't read {}: {e}", original.display())),
    };
    if audio.sample_rate == 0 || audio.is_empty() {
        return Made::AsItIs(None);
    }
    let rate = f64::from(audio.sample_rate);
    // Read only so far (a long file; some files don't say how long they are, so the length read counts too).
    if audio.seconds() < audio.file_seconds * 0.99 || audio.seconds() + 1.0 / rate >= MOST_SECONDS {
        return as_it_is(format!("{} is longer than {} minutes", original.display(), MOST_SECONDS / 60.0));
    }
    // The file says it's a loop.
    if audio.embedded_is_loop == Some(true) {
        return Made::AsItIs(None);
    }
    let Some((start, end)) = mo_audio::trim::sound_span(&audio, mo_audio::trim::level_of_db(db)) else {
        return Made::AsItIs(None);
    };
    if ((start + audio.len() - end) as f64) < LEAST_CUT_SECONDS * rate {
        return Made::AsItIs(None);
    }
    if audio.channels.iter().flatten().any(|s| s.abs() > 1.0) {
        return as_it_is(format!("{} goes over full scale, and a copy would clip it", original.display()));
    }
    audio.keep(start, end);
    if let Err(e) = std::fs::create_dir_all(folder) {
        return as_it_is(format!("couldn't make {}: {e}", folder.display()));
    }
    // Written under a name of this program's own and then renamed, so two programs (the app and the plugin) making
    // the same copy never write into one file, and a half-made copy never takes the copy's name.
    let mine = copy.with_extension(format!("{}.wav", std::process::id()));
    let written = mo_audio::write_wav(&mine, &audio.channels, audio.sample_rate, 24)
        .map_err(|e| e.to_string())
        .and_then(|()| stamp(&mine, changed).map_err(|e| e.to_string()))
        .and_then(|()| std::fs::rename(&mine, copy).map_err(|e| e.to_string()));
    match written {
        Ok(()) => Made::Copy(copy.to_path_buf()),
        Err(e) => {
            let _ = std::fs::remove_file(&mine);
            as_it_is(format!("couldn't write {}: {e}", copy.display()))
        }
    }
}

/// Dates the file at `path` `when` (nothing for `None`); a rename keeps the date.
fn stamp(path: &Path, when: Option<SystemTime>) -> std::io::Result<()> {
    match when {
        Some(when) => std::fs::File::options().write(true).open(path)?.set_modified(when),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-trim-copy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 1,000 frames a second, one channel: silent 0.2 s, loud 0.3 s, silent 0.5 s.
    fn quiet_loud_quiet(path: &Path) {
        let mut s = vec![0.0; 1_000];
        s[200..500].fill(0.5);
        mo_audio::write_wav(path, &[s], 1_000, 24).unwrap();
    }

    #[test]
    fn a_copy_is_trimmed_made_once_and_the_original_stays() {
        let dir = folder("once");
        let original = dir.join("Kick.wav");
        quiet_loud_quiet(&original);
        let before = std::fs::read(&original).unwrap();
        let copies = dir.join(FOLDER);
        let Made::Copy(copy) = for_drag(&original, -60, &copies) else { panic!("no copy") };
        assert_eq!(std::fs::read(&original).unwrap(), before, "the original is never changed");
        let name = copy.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, mo_sounds::trimmed::copy_name(&original.to_string_lossy(), -60));
        let audio = mo_audio::read(&copy, 60.0).unwrap();
        // From the first loud frame to the last, with its 10 ms tail.
        assert_eq!(audio.len(), 310);
        assert!(audio.channels[0][0] > 0.4);
        // Only the copy is left: nothing half-written beside it.
        assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 1);
        // The second drag drags the same copy; another level makes another.
        assert_eq!(for_drag(&original, -60, &copies), Made::Copy(copy.clone()));
        let Made::Copy(other) = for_drag(&original, -40, &copies) else { panic!("no copy") };
        assert_ne!(other, copy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn little_or_no_silence_goes_as_it_is_and_a_missing_file_says_why() {
        let dir = folder("nothing");
        let copies = dir.join(FOLDER);
        let loud = dir.join("Loud.wav");
        mo_audio::write_wav(&loud, &[vec![0.5; 500]], 1_000, 24).unwrap();
        assert_eq!(for_drag(&loud, -60, &copies), Made::AsItIs(None));
        // 5 ms of silence at the start: too little to hear.
        let mut s = vec![0.5; 500];
        s[..5].fill(0.0);
        let padded = dir.join("Padded.wav");
        mo_audio::write_wav(&padded, &[s], 1_000, 24).unwrap();
        assert_eq!(for_drag(&padded, -60, &copies), Made::AsItIs(None));
        assert!(!copies.exists(), "no folder made for nothing");
        assert!(matches!(for_drag(&dir.join("Gone.wav"), -60, &copies), Made::AsItIs(Some(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copies_go_to_the_export_folder_the_song_library_sets() {
        let dir = folder("library");
        let exports = dir.join("Exports on I");
        std::fs::create_dir_all(&exports).unwrap();
        // Only songs.db, as on his PC since library.db was put away: its export folder is the one used.
        let c = rusqlite::Connection::open(dir.join(mo_library::songs_db::FILE_NAME)).unwrap();
        c.execute_batch(
            "CREATE TABLE projects (id INTEGER PRIMARY KEY); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![crate::bitwig_convert::EXPORT_FOLDER_KEY, exports.to_string_lossy()],
        )
        .unwrap();
        drop(c);
        assert_eq!(folder_in(&dir), Ok(exports.join(FOLDER)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_original_dated_ahead_keeps_its_copy() {
        let dir = folder("ahead");
        let original = dir.join("Ahead.wav");
        quiet_loud_quiet(&original);
        let later = SystemTime::now() + std::time::Duration::from_secs(80 * 365 * 24 * 60 * 60);
        stamp(&original, Some(later)).unwrap();
        let copies = dir.join(FOLDER);
        let Made::Copy(copy) = for_drag(&original, -60, &copies) else { panic!("no copy") };
        assert_eq!(std::fs::metadata(&copy).unwrap().modified().unwrap(), later, "the copy carries its date");
        // Marked (and dated as made), so a copy made again would show.
        std::fs::write(&copy, b"kept").unwrap();
        stamp(&copy, Some(later)).unwrap();
        assert_eq!(for_drag(&original, -60, &copies), Made::Copy(copy.clone()));
        assert_eq!(std::fs::read(&copy).unwrap(), b"kept");
        // The original changed (dated ahead again, later still): made again.
        stamp(&original, Some(later + std::time::Duration::from_secs(60))).unwrap();
        assert_eq!(for_drag(&original, -60, &copies), Made::Copy(copy.clone()));
        assert_ne!(std::fs::read(&copy).unwrap(), b"kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn half_made_copies_are_told_from_copies() {
        let copy = mo_sounds::trimmed::copy_name("D:\\Samples\\Kick.wav", -60);
        let stem = copy.strip_suffix(".wav").unwrap();
        assert!(is_half_made(&format!("{stem}.4242.wav")));
        assert!(is_half_made(&format!("{stem}.4242.wav.part")));
        assert!(!is_half_made(&copy), "never a copy");
        assert!(!is_half_made("Kick.4242.wav"), "never a file of the user's");
        assert!(!is_half_made(&format!("{stem}.x42.wav")));
    }

    #[test]
    fn loops_and_trim_off_drag_the_original() {
        let original = Path::new("D:/Samples/Loop.wav");
        assert_eq!(to_drag(original, Some(-60), true), (original.to_path_buf(), None));
        assert_eq!(to_drag(original, None, false), (original.to_path_buf(), None));
    }
}
