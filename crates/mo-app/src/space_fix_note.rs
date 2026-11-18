//! The app's note of the "(fixed)" Bitwig projects Fix wrote: each one's place, size and modified time as Fix left it
//! (the whole-app review of 14 Nov, FX-5). "Send every fixed Bitwig project to the Recycle Bin"
//! (`mo_space::bitwig::remove_fixed_projects`) sends only a "(fixed)" project still as Fix left it. One you've saved in
//! Bitwig since is your work (rule 3), so it stays, and the card names it (Solomon, 15 Nov 2026: "Skip ones I've saved
//! in"). Before, it went too.
//!
//! The note began when the app first ran with it ([`Note::since`]). A "(fixed)" project Fix wrote before then isn't on
//! it, so it goes only while Windows can tell it hasn't changed since it was made: it last changed within a minute of
//! being made ([`WRITING_TAKES`]), and before the note began. Saving in Bitwig doesn't change when a file was made on
//! Windows; on a Mac or Linux it may (a save can make a new file), so there one written before the note stays, as does
//! one whose drive doesn't say, or one copied from elsewhere (made after it last changed). One that isn't on the note
//! but changed after it began stays too. A file's entry stays when it goes to the Recycle Bin, so one put back (by the
//! app or by hand) is still as Fix left it; Fix writing the same name again takes its place. A damaged note begins
//! again as if it had always been kept ([`Note::since`] 0), so nothing it held goes until Fix writes it again. The note
//! keeps at most [`KEEP_AT_MOST`] (rule 28): past that, the ones Fix wrote longest ago go first (and stay on disk).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The file, in the data folder. Written whole ([`crate::backup::write_whole`]), so it's on that list.
pub const FILE_NAME: &str = "bitwig-fixed-projects.json";

/// How many "(fixed)" projects the note keeps at most (rule 28). A few hundred projects, fixed again and again under the
/// same names, keep it far below that. Each one Fix writes saves the whole note (a crash part way through Fix loses
/// none): at most half a megabyte, against the project Fix has just read, changed, written and read back.
pub const KEEP_AT_MOST: usize = 5_000;

/// How long Fix may take to write a "(fixed)" project, from making the file to its last byte: one written before the
/// note began and changed later than this after it was made has been saved in since. Opening a project in Bitwig and
/// saving it takes longer.
pub const WRITING_TAKES: Duration = Duration::from_secs(60);

/// How far a file's modified time may read from the time noted and still be the same: FAT keeps it to 2 seconds, and
/// a network drive may round it. Saving in Bitwig comes later than this, and the size must match too.
pub const SAME_TIME: Duration = Duration::from_secs(2);

/// Why a "(fixed)" project stays out of the Recycle Bin, from the note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stays {
    /// It has changed since Fix wrote it: you've saved in it, most likely. The card names it.
    Changed,
    /// The app has no note of Fix writing it as it is (written before the note began, or Fix's note of it was lost),
    /// so whether you've saved in it can't be told. The card names it, and says you can delete it yourself.
    NotNoted,
    /// When it last changed can't be read.
    NoTime,
    /// Free the space sent away the copies its save used: it's the one that plays now, so it's never binned here.
    Freed,
}

impl Stays {
    /// Why it stays, in plain words, as the log says it.
    pub fn why(self) -> &'static str {
        match self {
            Stays::Changed => "It has changed since Fix wrote it, so it's yours now: it stays.",
            Stays::NotNoted => {
                "The app has no record of Fix writing it as it is now, so whether you've saved in it can't be told: it \
                 stays."
            }
            Stays::NoTime => {
                "When it last changed can't be read, so whether you've saved in it can't be told: it stays."
            }
            Stays::Freed => "Its song's copies went to the Recycle Bin, so it's the one that plays now: it stays.",
        }
    }
}

/// What the app has noted of what Fix wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// When the note began (Unix seconds): from then on, each "(fixed)" project Fix writes is on it. 0 for one begun
    /// again after it was damaged.
    pub since: i64,
    /// What Fix wrote, the newest last.
    #[serde(default)]
    pub written: Vec<Written>,
}

/// A "(fixed)" project as Fix left it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    pub path: String,
    pub bytes: u64,
    /// When it last changed, in nanoseconds from 1970.
    pub modified: u64,
    /// Free the space sent away the copies its save used (Solomon, 17 Nov 2026): it stays out of the Recycle Bin
    /// whatever else is true of it.
    #[serde(default)]
    pub freed: bool,
}

/// The file in the app's data folder.
pub fn file() -> PathBuf {
    crate::paths::data_dir().join(FILE_NAME)
}

/// What waits to be put on a note that couldn't be saved (its drive full, say), by the note's file: kept while the app
/// runs, put on with the next change, and counted by the bin button meanwhile.
struct Waiting {
    /// When each note begun in this run began, so one that couldn't be saved keeps its time.
    begun: Vec<(PathBuf, i64)>,
    /// What Fix wrote.
    written: Vec<(PathBuf, Written)>,
}

/// Notes that Free the space sent away the copies the save of the "(fixed)" project `fixed` used, in `file`: the
/// button for every fixed project leaves it then. One that isn't on the note yet is put on as it is now. What can't be
/// noted is said in the log and returned.
pub fn freed(file: &Path, fixed: &Path) -> Result<(), String> {
    let mut waiting = CHANGING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut note = load_or_begin(file, &mut waiting)?;
    match note.written.iter_mut().find(|w| same(&w.path, &fixed.to_string_lossy())) {
        Some(w) => w.freed = true,
        None => {
            let meta = fs::symlink_metadata(fixed).map_err(|e| e.to_string())?;
            let modified = meta.modified().ok().and_then(nanos).unwrap_or(0);
            note.written.push(Written {
                path: fixed.to_string_lossy().into_owned(),
                bytes: meta.len(),
                modified,
                freed: true,
            });
        }
    }
    serde_json::to_string(&note)
        .map_err(std::io::Error::other)
        .and_then(|text| crate::backup::write_whole(file, &text))
        .map_err(|e| {
            let why = format!("Couldn't note that {}'s copies went ({e})", fixed.display());
            crate::log::warning("Space", &why);
            why
        })
}

/// One change at a time to a note, from any thread, with what waits to be put on it.
static CHANGING: Mutex<Waiting> = Mutex::new(Waiting { begun: Vec::new(), written: Vec::new() });

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A time in nanoseconds from 1970; None before then, or past what 64 bits hold (the year 2554).
fn nanos(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().and_then(|d| u64::try_from(d.as_nanos()).ok())
}

/// Whether two times in nanoseconds are no further apart than `within`.
fn near(a: u64, b: u64, within: Duration) -> bool {
    u128::from(a.abs_diff(b)) <= within.as_nanos()
}

fn same(a: &str, b: &str) -> bool {
    mo_space::key(a) == mo_space::key(b)
}

/// What [`load`] found.
enum Loaded {
    Note(Note),
    Missing,
    /// Not a note (said in the log).
    Damaged,
}

/// The note in `file`. Err when it can't be read now.
fn load(file: &Path) -> Result<Loaded, String> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(match serde_json::from_str(&text) {
            Ok(note) => Loaded::Note(note),
            Err(e) => {
                crate::log::warning(
                    "Space",
                    &format!(
                        "The note of what Fix wrote, {}, is damaged ({e}), so a new one begins: the \"(fixed)\" \
                         projects Fix wrote before now stay out of the Recycle Bin",
                        file.display()
                    ),
                );
                Loaded::Damaged
            }
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded::Missing),
        Err(e) => Err(format!("The app's note of what Fix wrote can't be read now ({e}), so it stays.")),
    }
}

/// The note in `file`, begun if it isn't there (when this run first began it, for one that couldn't be saved), with
/// what waits to be put on it. Saved when that changed it; what still can't be saved waits on (said in the log). Call
/// it holding [`CHANGING`].
fn load_or_begin(file: &Path, waiting: &mut Waiting) -> Result<Note, String> {
    let (mut note, mut changed) = match load(file)? {
        Loaded::Note(note) => (note, false),
        Loaded::Missing => {
            let since = match waiting.begun.iter().find(|(f, _)| f == file) {
                Some(&(_, since)) => since,
                None => {
                    let since = now();
                    waiting.begun.push((file.to_owned(), since));
                    since
                }
            };
            (Note { since, written: Vec::new() }, true)
        }
        // Whatever it held is lost, so it's as if it had always been kept: one that isn't on it now stays.
        Loaded::Damaged => (Note { since: 0, written: Vec::new() }, true),
    };
    for (_, w) in waiting.written.iter().filter(|(f, _)| f == file) {
        note.written.retain(|n| !same(&n.path, &w.path));
        note.written.push(w.clone());
        changed = true;
    }
    if note.written.len() > KEEP_AT_MOST {
        note.written.drain(..note.written.len() - KEEP_AT_MOST);
    }
    if changed {
        let saved = serde_json::to_string(&note)
            .map_err(std::io::Error::other)
            .and_then(|text| crate::backup::write_whole(file, &text));
        match saved {
            Ok(()) => waiting.written.retain(|(f, _)| f != file),
            Err(e) => crate::log::warning(
                "Space",
                &format!(
                    "The note of what Fix wrote couldn't be saved in {} ({e}): kept while the app runs",
                    file.display()
                ),
            ),
        }
    }
    Ok(note)
}

/// Begins the note in `file` if it isn't there yet. As the app starts ([`begin_soon`]): a "(fixed)" project changed
/// after that and not on the note stays.
pub fn begin(file: &Path) {
    let mut waiting = CHANGING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Err(why) = load_or_begin(file, &mut waiting) {
        crate::log::warning("Space", &why);
    }
}

/// [`begin`] in the data folder, away from the window's thread.
pub fn begin_soon() {
    if let Err(e) = crate::background::spawn("Fix note", || begin(&file())) {
        crate::log::warning("Space", &format!("Couldn't begin the note of what Fix writes: {e}"));
    }
}

/// Notes the "(fixed)" project Fix has just written, `fixed`, as it is now (in place of one noted before under that
/// name), in `file`. What can't be noted is said in the log (it then stays out of the Recycle Bin, as one the app has
/// no record of); one that can't be saved is kept while the app runs.
pub fn wrote(file: &Path, fixed: &Path) {
    let written = fs::symlink_metadata(fixed)
        .map_err(|e| e.to_string())
        .and_then(|m| Ok((m.len(), m.modified().map_err(|e| e.to_string())?)))
        .and_then(|(bytes, modified)| {
            let modified = nanos(modified).ok_or("its modified time is out of range")?;
            Ok(Written { path: fixed.to_string_lossy().into_owned(), bytes, modified, freed: false })
        });
    let written = match written {
        Ok(written) => written,
        Err(e) => {
            crate::log::warning("Space", &format!("Couldn't note what Fix wrote, {} ({e})", fixed.display()));
            return;
        }
    };
    let mut waiting = CHANGING.lock().unwrap_or_else(PoisonError::into_inner);
    waiting.written.retain(|(f, w)| !(f == file && same(&w.path, &written.path)));
    waiting.written.push((file.to_owned(), written));
    if let Err(why) = load_or_begin(file, &mut waiting) {
        crate::log::warning("Space", &format!("Couldn't note what Fix wrote, {}: {why}", fixed.display()));
    }
}

/// The note as it was when the bin button was pressed.
#[derive(Clone, Debug)]
pub struct Noted {
    /// When it began, in nanoseconds from 1970.
    since: u64,
    /// Size and modified time, by the path as Space compares paths (`mo_space::key`).
    by_path: HashMap<String, (u64, u64)>,
    /// The ones Free the space sent the copies of, by the path as Space compares paths.
    freed: std::collections::HashSet<String>,
}

/// The note in `file`, for the bin button: begun now if it isn't there yet, with what Fix wrote this run that couldn't
/// be saved on it. Err when it can't be read now, and why every "(fixed)" project stays then.
fn read(file: &Path) -> Result<Noted, String> {
    let note = {
        let mut waiting = CHANGING.lock().unwrap_or_else(PoisonError::into_inner);
        load_or_begin(file, &mut waiting)?
    };
    Ok(Noted::from(&note))
}

impl From<&Note> for Noted {
    fn from(note: &Note) -> Self {
        Noted {
            since: u64::try_from(note.since).unwrap_or(0).saturating_mul(1_000_000_000),
            by_path: note.written.iter().map(|w| (mo_space::key(&w.path), (w.bytes, w.modified))).collect(),
            freed: note.written.iter().filter(|w| w.freed).map(|w| mo_space::key(&w.path)).collect(),
        }
    }
}

impl Noted {
    /// Whether the "(fixed)" project at `path` is still as Fix left it, from its size, when it last changed and when it
    /// was made (None where that can't be trusted: [`Check`] gives it on Windows only). Err says why it stays.
    pub fn judge(
        &self,
        path: &Path,
        bytes: u64,
        modified: Option<SystemTime>,
        made: Option<SystemTime>,
    ) -> Result<(), Stays> {
        if self.freed.contains(&mo_space::key(&path.to_string_lossy())) {
            return Err(Stays::Freed);
        }
        let modified = modified.and_then(nanos).ok_or(Stays::NoTime)?;
        if let Some(&(was_bytes, was_modified)) = self.by_path.get(&mo_space::key(&path.to_string_lossy())) {
            return if bytes == was_bytes && near(modified, was_modified, SAME_TIME) {
                Ok(())
            } else {
                Err(Stays::Changed)
            };
        }
        // Not on the note: one written before it began may go, while it last changed before then and as it was made.
        if modified >= self.since {
            return Err(Stays::NotNoted);
        }
        match made.and_then(nanos) {
            Some(made) if near(modified, made, WRITING_TAKES) => Ok(()),
            Some(made) if modified > made => Err(Stays::Changed),
            // Copied from elsewhere (made after it last changed), or when it was made isn't known.
            _ => Err(Stays::NotNoted),
        }
    }
}

/// The bin button's check of each "(fixed)" project against the note (as `mo_space::bitwig::AsFixLeft` asks), keeping
/// why each one that stays does, for the card.
pub struct Check {
    noted: Result<Noted, String>,
    stayed: Mutex<HashMap<String, Stays>>,
}

impl Check {
    /// Reads the note in `file` (begun now if it isn't there yet), once for the whole bin press: Fix can't write
    /// meanwhile. Reads the disk: call it away from the window's thread.
    pub fn new(file: &Path) -> Self {
        Check { noted: read(file), stayed: Mutex::new(HashMap::new()) }
    }

    /// Whether the "(fixed)" project at `path`, with this metadata, is still as Fix left it. Err says why it stays.
    pub fn as_fix_left(&self, path: &Path, meta: &fs::Metadata) -> Result<(), String> {
        let noted = self.noted.as_ref().map_err(Clone::clone)?;
        let made = if cfg!(windows) { meta.created().ok() } else { None };
        noted.judge(path, meta.len(), meta.modified().ok(), made).map_err(|stays| {
            let mut stayed = self.stayed.lock().unwrap_or_else(PoisonError::into_inner);
            stayed.insert(mo_space::key(&path.to_string_lossy()), stays);
            stays.why().to_owned()
        })
    }

    /// Why each one that stayed did, by its path as Space compares paths (`mo_space::key`).
    pub fn stayed(&self) -> HashMap<String, Stays> {
        self.stayed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mo-app-fix-note-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn at(secs: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn at_ms(ms: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_millis(ms))
    }

    const BEGAN: u64 = 1_763_200_000;

    fn noted(since: u64, written: &[(&str, u64, u64)]) -> Noted {
        let written = written
            .iter()
            .map(|&(path, bytes, secs)| Written {
                path: path.to_owned(),
                bytes,
                modified: secs * 1_000_000_000,
                freed: false,
            })
            .collect();
        Noted::from(&Note { since: since as i64, written })
    }

    fn loaded(file: &Path) -> Note {
        match load(file).unwrap() {
            Loaded::Note(note) => note,
            _ => panic!("no note in {}", file.display()),
        }
    }

    #[test]
    fn one_on_the_note_goes_only_while_its_as_fix_left_it() {
        let noted = noted(BEGAN, &[("K:/Projects/Garage/Garage 2 (fixed).bwproject", 900, BEGAN + 60)]);
        let garage = Path::new(r"k:\projects\garage\Garage 2 (fixed).bwproject");
        // As Fix left it (the path as Space compares paths): it goes, whenever it was made.
        assert_eq!(noted.judge(garage, 900, at(BEGAN + 60), None), Ok(()));
        assert_eq!(noted.judge(garage, 900, at(BEGAN + 60), at(BEGAN - 3600)), Ok(()));
        // Its time read rounded (FAT keeps 2 seconds) is still the same.
        assert_eq!(noted.judge(garage, 900, at_ms((BEGAN + 62) * 1000), None), Ok(()));
        assert_eq!(noted.judge(garage, 900, at_ms((BEGAN + 58) * 1000), None), Ok(()));
        // Saved in since: a later time, or a new size.
        assert_eq!(noted.judge(garage, 900, at_ms((BEGAN + 62) * 1000 + 1), None), Err(Stays::Changed));
        assert_eq!(noted.judge(garage, 901, at(BEGAN + 60), None), Err(Stays::Changed));
        // When it last changed can't be read.
        assert_eq!(noted.judge(garage, 900, None, None), Err(Stays::NoTime));
    }

    #[test]
    fn one_written_before_the_note_began_goes_only_while_it_last_changed_as_it_was_made() {
        let before = noted(BEGAN, &[]);
        let old = Path::new("K:/Projects/Night/Night (fixed).bwproject");
        let fixed = BEGAN - 86_400;
        // Fix made it and wrote its last byte a moment later; on a FAT drive it's made once it's written.
        assert_eq!(before.judge(old, 900, at(fixed), at(fixed - 2)), Ok(()));
        assert_eq!(before.judge(old, 900, at(fixed), at(fixed + 2)), Ok(()));
        let took = WRITING_TAKES.as_secs();
        assert_eq!(before.judge(old, 900, at(fixed + took), at(fixed)), Ok(()));
        // Saved in after it was made (still before the note began).
        assert_eq!(before.judge(old, 900, at(fixed + took + 1), at(fixed)), Err(Stays::Changed));
        // Copied from another drive (made after it last changed), or when it was made isn't known: can't be told.
        assert_eq!(before.judge(old, 900, at(fixed), at(fixed + took + 1)), Err(Stays::NotNoted));
        assert_eq!(before.judge(old, 900, at(fixed), None), Err(Stays::NotNoted));
        // Changed after the note began, yet not on it.
        assert_eq!(before.judge(old, 900, at(BEGAN), at(BEGAN)), Err(Stays::NotNoted));
        assert_eq!(before.judge(old, 900, at(BEGAN + 3600), at(BEGAN + 3600)), Err(Stays::NotNoted));
        // A note begun again after damage holds nothing written before.
        assert_eq!(noted(0, &[]).judge(old, 900, at(fixed), at(fixed)), Err(Stays::NotNoted));
    }

    /// On a Mac or Linux a save can make a new file, so when one was made isn't trusted there: one written before the
    /// note began stays. Windows' is (here, made as it was written: it goes).
    #[test]
    fn one_written_before_the_note_began_stays_where_when_it_was_made_cant_be_trusted() {
        let d = folder("made");
        let file = d.join(FILE_NAME);
        // A note that begins an hour from now: the project below was written before it.
        let since = now() + 3600;
        crate::backup::write_whole(&file, &serde_json::to_string(&Note { since, written: Vec::new() }).unwrap())
            .unwrap();
        let fixed = d.join("Old (fixed).bwproject");
        fs::write(&fixed, b"BtWg").unwrap();
        let meta = fs::metadata(&fixed).unwrap();
        let expected = if cfg!(windows) { Ok(()) } else { Err(Stays::NotNoted.why().to_owned()) };
        assert_eq!(Check::new(&file).as_fix_left(&fixed, &meta), expected);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn what_fix_wrote_is_noted_as_it_is_and_the_note_begins_once() {
        let d = folder("wrote");
        let file = d.join(FILE_NAME);
        let fixed = d.join("Garage 2 (fixed).bwproject");
        fs::write(&fixed, b"BtWg fixed").unwrap();
        let before = now();
        begin(&file);
        let note = loaded(&file);
        assert!(note.since >= before && note.written.is_empty(), "{note:?}");
        // Beginning again changes nothing.
        let since = note.since - 10;
        crate::backup::write_whole(&file, &serde_json::to_string(&Note { since, written: Vec::new() }).unwrap())
            .unwrap();
        begin(&file);
        assert_eq!(loaded(&file).since, since);

        wrote(&file, &fixed);
        let meta = fs::metadata(&fixed).unwrap();
        let check = Check::new(&file);
        assert_eq!(check.noted.as_ref().unwrap().since, since as u64 * 1_000_000_000);
        assert_eq!(check.as_fix_left(&fixed, &meta), Ok(()));
        let note = loaded(&file);
        assert_eq!(note.written.len(), 1);
        assert_eq!((note.written[0].bytes, note.written[0].modified), (10, nanos(meta.modified().unwrap()).unwrap()));

        // Saved in since: it's no longer as Fix left it, and the check keeps why.
        fs::write(&fixed, b"BtWg saved in Bitwig").unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        fs::File::options().write(true).open(&fixed).unwrap().set_modified(later).unwrap();
        let meta = fs::metadata(&fixed).unwrap();
        assert_eq!(check.as_fix_left(&fixed, &meta), Err(Stays::Changed.why().to_owned()));
        assert_eq!(check.stayed(), HashMap::from([(mo_space::key(&fixed.to_string_lossy()), Stays::Changed)]));
        // Fix writing that name again takes the old one's place.
        wrote(&file, &fixed);
        assert_eq!(loaded(&file).written.len(), 1);
        assert_eq!(Check::new(&file).as_fix_left(&fixed, &meta), Ok(()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_note_begins_again_holding_nothing_and_one_that_cant_be_saved_keeps_its_time() {
        let d = folder("damaged");
        let file = d.join(FILE_NAME);
        fs::write(&file, "not json").unwrap();
        let read = read(&file).unwrap();
        assert!(read.since == 0 && read.by_path.is_empty());
        assert_eq!(loaded(&file).since, 0, "the new one is saved");

        // A note whose folder isn't there can't be saved: it keeps the time it began (here, as if long ago in this
        // run), and what Fix wrote is still counted while the app runs.
        let away = d.join("gone").join(FILE_NAME);
        begin(&away);
        assert!(!away.exists());
        CHANGING.lock().unwrap().begun.iter_mut().find(|(f, _)| f == &away).unwrap().1 = 1_000;
        let began = super::read(&away).unwrap().since;
        assert_eq!(began, 1_000 * 1_000_000_000);
        let fixed = d.join("Song (fixed).bwproject");
        fs::write(&fixed, b"BtWg").unwrap();
        wrote(&away, &fixed);
        let meta = fs::metadata(&fixed).unwrap();
        let check = Check::new(&away);
        assert_eq!(check.noted.as_ref().unwrap().since, began, "it began once");
        assert_eq!(check.as_fix_left(&fixed, &meta), Ok(()));
        // Once the note can be saved, it's put on it.
        fs::create_dir_all(away.parent().unwrap()).unwrap();
        begin(&away);
        let note = loaded(&away);
        assert_eq!((note.since as u64 * 1_000_000_000, note.written.len()), (began, 1));
        assert!(!CHANGING.lock().unwrap().written.iter().any(|(f, _)| f == &away), "nothing waits any more");
        // One that can't be read at all (a folder in its place) is why each stays.
        let folder_note = d.join("folder");
        fs::create_dir_all(&folder_note).unwrap();
        let check = Check::new(&folder_note);
        let why = check.as_fix_left(&fixed, &meta).unwrap_err();
        assert!(why.starts_with("The app's note of what Fix wrote can't be read now"), "{why}");
        assert!(check.stayed().is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_note_keeps_the_newest_it_may() {
        let d = folder("keep");
        let file = d.join(FILE_NAME);
        let written: Vec<Written> = (0..KEEP_AT_MOST + 2)
            .map(|n| Written {
                path: format!("K:/P/{n} (fixed).bwproject"),
                bytes: 1,
                modified: n as u64,
                freed: false,
            })
            .collect();
        let since = BEGAN as i64;
        crate::backup::write_whole(&file, &serde_json::to_string(&Note { since, written }).unwrap()).unwrap();
        let fixed = d.join("New (fixed).bwproject");
        fs::write(&fixed, b"BtWg").unwrap();
        wrote(&file, &fixed);
        let note = loaded(&file);
        assert_eq!(note.written.len(), KEEP_AT_MOST);
        assert_eq!(note.written[0].path, "K:/P/3 (fixed).bwproject", "the oldest go first");
        assert_eq!(note.written.last().unwrap().path, fixed.to_string_lossy());
        // What a crash part way through a save leaves of it, the start-up sweep clears (rule 28).
        assert!(crate::backup::WRITTEN_WHOLE.contains(&FILE_NAME));
        let _ = fs::remove_dir_all(&d);
    }
}
