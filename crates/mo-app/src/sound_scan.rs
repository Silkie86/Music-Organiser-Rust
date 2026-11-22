//! Scanning the sample folders for new, changed, moved and gone sounds, and keeping what's found in the app's own sound
//! library (stage 2 of "the sound library grows again", review finding X1-own1). Since the C# app retired, nothing
//! did: new samples never showed on the Sounds page.
//!
//! As the C# app's `SoundOrganiser.Scan`: the folders are walked ([`scan::find_sounds`]); only files new or changed by
//! their size and time are listened to; renamed and moved ones keep everything you gave them; a folder on an unplugged
//! drive keeps its sounds ([`scan::plan`]). It all runs off the window's thread, on the bulk pool at low priority
//! (rule 21), a few sounds at a time, each few kept in one short write ([`grow`]).
//!
//! A file that couldn't be listened to is kept with why and left alone until it changes, or until the reader learns
//! to read more ([`mo_audio::READS`]): then the next scan tries just those again, once, and notes it in the library.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use mo_library::scan::{Found, FoundFile, count_inside, is_inside};
use mo_sounds::compact::{self, grow};
use mo_sounds::scan::{self, ScanPlan};
use mo_sounds::{KnownSound, SoundsDb, analyse};
use rayon::prelude::*;

use crate::background;
use crate::backup::thousands;

/// How many sounds are listened to before what was found is kept: each write stays short, and a stop (the app closed)
/// loses no more than this.
const AT_A_TIME: usize = 64;

/// The most of the library a scan forgets at once: past it (and past [`FEW_GONE`] sounds), the sounds that look gone
/// are left, as something is more likely wrong with the folders than half the samples deleted.
const MOST_GONE: f64 = 0.5;

/// So many sounds gone are forgotten however few the library has.
const FEW_GONE: usize = 100;

/// What a scan did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scanned {
    /// How many sound files are in the sample folders.
    pub found: usize,
    /// What changed in the library.
    pub kept: grow::Kept,
    /// Of the files listened to, how many couldn't be (kept with why, as the C# app kept them).
    pub unreadable: usize,
    /// Of those, how many were new files: they aren't counted as new sounds found.
    pub unheard_new: usize,
    /// How many unchanged sounds that couldn't be listened to before were tried again, as the reader has learnt to
    /// read more since ([`mo_audio::READS`]). They count in [`grow::Kept::updated`], but not in `unreadable`: that
    /// they couldn't be read was known.
    pub tried_again: usize,
    /// Of those, how many could be listened to this time.
    pub heard_now: usize,
    /// Why the library couldn't note that its unread sounds were tried, when it couldn't: they're tried again at the
    /// next scan, and nothing else is lost.
    pub unnoted: Option<String>,
    /// How many sounds looked gone but weren't forgotten: too many at once ([`MOST_GONE`]).
    pub not_forgotten: usize,
    /// How long it took.
    pub took: Duration,
    /// Why it stopped part way, when it did: what was kept before stays kept, and the rest is tried again at the next
    /// scan.
    pub stopped: Option<String>,
    /// The files listened to and kept (new and changed sounds), for the page to read only their fingerprints, not
    /// every one again (review finding S-5).
    pub listened: Vec<String>,
}

/// Scans `folders` and keeps what's new, changed, moved or gone in the app's own sound library beside `sounds_db`.
/// `None` when there's nothing to scan: no folders, or no library that's the app's own (one still following
/// `sounds.db` is made from it instead). `progress` is told how many files have been listened to, of how many, before
/// the first and after each few; it's called on this thread. Waits for the work, so never on the window's thread.
///
/// An error when nothing could be done (the library couldn't be read); a write that fails once some are kept stops the
/// scan with what was kept ([`Scanned::stopped`]), for the page to take it.
pub fn scan(sounds_db: &Path, folders: &[String], progress: &dyn Fn(usize, usize)) -> Result<Option<Scanned>, String> {
    let began = Instant::now();
    let copy = compact::copy_of(sounds_db);
    if folders.is_empty() || compact::own_since(&copy).is_none() {
        return Ok(None);
    }
    // Settings shows how far it is, folder by folder, whatever happens to it ([`state`]).
    let mut told = Told::new(sounds_db, folders);
    let found = background::on_bulk_pool(|| scan::find_sounds(folders));
    let known = match SoundsDb::open_read_only(&copy).and_then(|db| db.known_files()) {
        Ok(known) => known,
        Err(e) => {
            told.ended(Some(e.to_string()));
            return Err(e.to_string());
        }
    };
    let ScanPlan { mut listen, moves, forget } = scan::plan(&found, &known, folders);
    let mut scanned = Scanned { found: found.files.len(), ..Scanned::default() };
    let again = try_again(&copy, &mut listen, &found, &moves, folders);
    told.found(&found, &listen);
    let forget = if forget.len() > FEW_GONE && forget.len() as f64 > known.len() as f64 * MOST_GONE {
        scanned.not_forgotten = forget.len();
        Vec::new()
    } else {
        forget
    };
    let to_do = ToDo { listen: &listen, moves: &moves, forget: &forget, again: again.as_ref().map(|a| &a.added) };
    let progress = |done: usize, of: usize, files: &[FoundFile], listened: &[grow::Listened]| {
        told.listened(files, listened);
        progress(done, of);
    };
    match keep_all(sounds_db, &known, &to_do, &mut scanned, &progress) {
        // Noted only once every one was tried: a scan stopped part way, or one with an unread sound on an unplugged
        // drive, tries them again at the next. A note that can't be written costs only that.
        Ok(()) => {
            if again.is_some_and(|a| a.waiting == 0) {
                scanned.unnoted = grow::note_unread_tried(sounds_db, scan::READS).err().map(|e| e.to_string());
            }
        }
        Err(why) => scanned.stopped = Some(why),
    }
    scanned.took = began.elapsed();
    told.ended(scanned.stopped.clone());
    // Sounds joined, moved or went: the Songs list looks again for the samples it has marked missing.
    if scanned.kept.any() {
        SOUNDS_CHANGED.fetch_add(1, Ordering::Relaxed);
    }
    Ok(Some(scanned))
}

/// How many scans in this run have changed the sound library ([`sounds_changed`]).
static SOUNDS_CHANGED: AtomicU64 = AtomicU64::new(0);

/// How many scans in this run have kept new, changed, moved or gone sounds: when it goes up, the Songs list looks
/// again for the samples it has marked missing (a sample folder added in Settings may hold them).
pub fn sounds_changed() -> u64 {
    SOUNDS_CHANGED.load(Ordering::Relaxed)
}

/// Where scanning one sample folder is, or was at the last scan, for Settings' folder rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderScan {
    /// The folder, as the list has it.
    pub folder: String,
    /// How many sound files are in it (and the folders in it).
    pub files: usize,
    /// Of those, how many are new or changed, to listen to this scan.
    pub to_listen: usize,
    /// Of those, how many have been listened to so far.
    pub listened: usize,
    /// Of those, how many couldn't be.
    pub unreadable: usize,
    /// It couldn't be read (a drive unplugged, a folder gone): its sounds were kept as they were.
    pub away: bool,
}

/// Where scanning the sample folders is: the scan going now, or the last one this run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanState {
    /// The scan hasn't ended yet.
    pub running: bool,
    /// It's still looking for sound files: [`ScanState::folders`] is empty until it's done.
    pub looking: bool,
    /// Each sample folder it scans.
    pub folders: Vec<FolderScan>,
    /// When it started, and when it ended.
    pub started: NaiveDateTime,
    pub ended: Option<NaiveDateTime>,
    /// Why it stopped part way, or couldn't be done, when it did.
    pub stopped: Option<String>,
}

impl ScanState {
    /// This folder's part of it, by the list's spelling.
    pub fn folder(&self, folder: &str) -> Option<&FolderScan> {
        self.folders.iter().find(|f| f.folder == folder)
    }
}

/// The scan state of each sound library scanned this run (the app has one; tests make many), newest last.
static STATES: Mutex<Vec<(PathBuf, ScanState)>> = Mutex::new(Vec::new());

/// Counts every change to [`STATES`], so a page can tell when to show it again ([`state_turn`]).
static STATE_TURN: AtomicU64 = AtomicU64::new(0);

/// Where scanning the sample folders into the library beside `sounds_db` is: `None` before its first scan this run.
pub fn state(sounds_db: &Path) -> Option<ScanState> {
    let states = STATES.lock().unwrap_or_else(PoisonError::into_inner);
    states.iter().find(|(db, _)| db == sounds_db).map(|(_, s)| s.clone())
}

/// [`state`] of the library scanned last this run, whichever it is: the app only ever scans its own.
pub fn latest_state() -> Option<ScanState> {
    STATES.lock().unwrap_or_else(PoisonError::into_inner).last().map(|(_, s)| s.clone())
}

/// Goes up each time a scan's state changes: a page showing it reads it again only then.
pub fn state_turn() -> u64 {
    STATE_TURN.load(Ordering::Relaxed)
}

/// Puts `state` in for the library beside `sounds_db`, as a scan would: for the screenshots, which scan nothing.
pub fn show_state(sounds_db: &Path, state: ScanState) {
    let mut states = STATES.lock().unwrap_or_else(PoisonError::into_inner);
    states.retain(|(db, _)| db != sounds_db);
    states.push((sounds_db.to_path_buf(), state));
    STATE_TURN.fetch_add(1, Ordering::Relaxed);
}

/// Tells [`STATES`] how a scan is going, folder by folder.
struct Told {
    sounds_db: PathBuf,
    folders: Vec<String>,
    /// Whether it was told the scan ended: if it's let go before (a scan that tripped up), it says so itself.
    ended: bool,
}

impl Drop for Told {
    fn drop(&mut self) {
        if !self.ended {
            self.ended(Some("It stopped on something the app didn't expect (the log says what).".to_owned()));
        }
    }
}

impl Told {
    /// A scan of `folders` started: looking for sound files.
    fn new(sounds_db: &Path, folders: &[String]) -> Told {
        let told = Told { sounds_db: sounds_db.to_path_buf(), folders: folders.to_vec(), ended: false };
        let started = Local::now().naive_local();
        told.change(|s| *s = ScanState { running: true, looking: true, started, ..ScanState::default() });
        told
    }

    fn change(&self, change: impl FnOnce(&mut ScanState)) {
        let mut states = STATES.lock().unwrap_or_else(PoisonError::into_inner);
        let at = match states.iter().position(|(db, _)| *db == self.sounds_db) {
            Some(at) => at,
            None => {
                states.push((self.sounds_db.clone(), ScanState::default()));
                states.len() - 1
            }
        };
        // Newest last, for [`latest_state`].
        let mut this = states.remove(at);
        change(&mut this.1);
        states.push(this);
        STATE_TURN.fetch_add(1, Ordering::Relaxed);
    }

    /// The sound files are found, and `listen` are the ones to listen to: counted for each folder (a file in a folder
    /// inside another listed one counts in both).
    fn found(&self, found: &Found, listen: &[FoundFile]) {
        let (files, to_listen) = background::on_bulk_pool(|| {
            (count_inside(&found.files, &self.folders), count_inside(listen, &self.folders))
        });
        let folders: Vec<FolderScan> = self
            .folders
            .iter()
            .zip(files.into_iter().zip(to_listen))
            .map(|(folder, (files, to_listen))| FolderScan {
                folder: folder.clone(),
                files,
                to_listen,
                away: found.unreadable.iter().any(|u| is_inside(folder, u)),
                ..FolderScan::default()
            })
            .collect();
        self.change(|s| {
            s.looking = false;
            s.folders = folders;
        });
    }

    /// These `files` were listened to, with what was heard.
    fn listened(&self, files: &[FoundFile], listened: &[grow::Listened]) {
        self.change(|s| {
            for f in &mut s.folders {
                for (file, heard) in files.iter().zip(listened) {
                    if is_inside(&file.path, &f.folder) {
                        f.listened += 1;
                        f.unreadable += usize::from(heard.analysed.is_err());
                    }
                }
            }
        });
    }

    /// The scan ended, done or stopped part way (`stopped`).
    fn ended(&mut self, stopped: Option<String>) {
        self.ended = true;
        let ended = Local::now().naive_local();
        self.change(|s| {
            s.running = false;
            s.looking = false;
            s.ended = Some(ended);
            s.stopped = stopped;
        });
    }
}

/// When the reader has learnt to read more since the sounds that couldn't be listened to were last tried
/// ([`scan::READS`]), adds just those to `listen`, to be tried again, once. Sounds read fine before aren't touched
/// (their size and time say they're the same). `None` when there's no need, or the library can't say just now: the
/// scan goes on without, and the next one asks again.
fn try_again(
    copy: &Path,
    listen: &mut Vec<FoundFile>,
    found: &Found,
    moves: &[(String, String)],
    folders: &[String],
) -> Option<scan::Again> {
    if grow::unread_tried(copy).ok()?.unwrap_or(0) >= scan::READS {
        return None;
    }
    let unread = SoundsDb::open_read_only(copy).and_then(|db| db.unread_files()).ok()?;
    // One renamed or moved since is tried under its new name.
    let now_at: HashMap<String, &String> = moves.iter().map(|(from, to)| (from.to_ascii_lowercase(), to)).collect();
    let unread: Vec<String> =
        unread.into_iter().map(|p| now_at.get(&p.to_ascii_lowercase()).map_or(p, |to| (*to).clone())).collect();
    Some(scan::again(listen, found, &unread, folders))
}

/// What a scan has to do: [`ScanPlan`]'s parts, and the unread sounds tried again among `listen`, if any (in lower
/// case, as [`scan::Again::added`] has them).
struct ToDo<'a> {
    listen: &'a [FoundFile],
    moves: &'a [(String, String)],
    forget: &'a [String],
    again: Option<&'a HashSet<String>>,
}

/// Follows the moved and gone sounds, then listens to the new and changed ones a few at a time, each few kept in one
/// write, counting what was kept in `scanned` as it goes.
fn keep_all(
    sounds_db: &Path,
    known: &[KnownSound],
    to_do: &ToDo<'_>,
    scanned: &mut Scanned,
    progress: &Progress<'_>,
) -> Result<(), String> {
    let ToDo { listen, moves, forget, again } = *to_do;
    let again = again.filter(|a| !a.is_empty());
    if !moves.is_empty() || !forget.is_empty() {
        let followed = grow::follow(sounds_db, moves, forget).map_err(|e| e.to_string())?;
        scanned.kept.moved = followed.moved;
        scanned.kept.forgotten = followed.forgotten;
    }
    if listen.is_empty() {
        return Ok(());
    }
    progress(0, listen.len(), &[], &[]);
    // The paths the library knows (letter case aside, as SQLite compares them), to tell a new file that couldn't be
    // listened to from a changed one: gathered only once there's one.
    let mut known_paths: Option<HashSet<String>> = None;
    let mut done = 0;
    for files in listen.chunks(AT_A_TIME) {
        let listened: Vec<grow::Listened> = background::on_bulk_pool(|| files.par_iter().map(listen_to).collect());
        let kept = grow::keep(sounds_db, &listened).map_err(|e| e.to_string())?;
        scanned.kept.added += kept.added;
        scanned.kept.updated += kept.updated;
        scanned.listened.extend(listened.iter().map(|l| l.path.clone()));
        for l in &listened {
            if again.is_some_and(|a| a.contains(&l.path.to_ascii_lowercase())) {
                scanned.tried_again += 1;
                scanned.heard_now += usize::from(l.analysed.is_ok());
                continue;
            }
            if l.analysed.is_ok() {
                continue;
            }
            scanned.unreadable += 1;
            let known = known_paths.get_or_insert_with(|| {
                known
                    .iter()
                    .map(|k| &k.path)
                    .chain(moves.iter().map(|(_, to)| to))
                    .map(|p| p.to_ascii_lowercase())
                    .collect()
            });
            if !known.contains(&l.path.to_ascii_lowercase()) {
                scanned.unheard_new += 1;
            }
        }
        done += files.len();
        progress(done, listen.len(), files, &listened);
    }
    Ok(())
}

/// What [`keep_all`] tells as it goes: how many files have been listened to, of how many, and the few just listened to
/// with what was heard.
type Progress<'a> = dyn Fn(usize, usize, &[FoundFile], &[grow::Listened]) + 'a;

/// Listens to one sound file. A file that can't be read, or that trips the reader up (a damaged file), is kept with
/// why, as the C# app kept it: it isn't listened to again until it changes.
fn listen_to(file: &FoundFile) -> grow::Listened {
    let path = Path::new(&file.path);
    let analysed = match mo_catch::catch(|| analyse::analyse_file(path)) {
        Ok(Ok(analysed)) => Ok(analysed),
        Ok(Err(e)) => Err(format!("Couldn't read the file: {e}")),
        Err(_) => Err("Couldn't read the file: it tripped the reader up (the file may be damaged)".to_owned()),
    };
    grow::Listened { path: file.path.clone(), size: file.size, mtime: file.mtime, analysed }
}

/// What the log says a scan did.
pub fn scanned_line(s: &Scanned) -> String {
    let k = &s.kept;
    let mut line = format!(
        "{} sound files in the sample folders: {} new, {} listened to again, {} moved, {} gone, in {:.1} s",
        thousands(s.found),
        thousands(k.added),
        thousands(k.updated),
        thousands(k.moved),
        thousands(k.forgotten),
        s.took.as_secs_f32()
    );
    if s.unreadable > 0 {
        line.push_str(&format!("; {} couldn't be listened to", thousands(s.unreadable)));
    }
    if s.tried_again > 0 {
        line.push_str(&format!(
            "; {} that couldn't be read before were tried again, {} can be read now",
            thousands(s.tried_again),
            thousands(s.heard_now)
        ));
    }
    if let Some(why) = &s.unnoted {
        line.push_str(&format!("; couldn't note that they were tried, so they will be again: {why}"));
    }
    if s.not_forgotten > 0 {
        line.push_str(&format!(
            "; {} that look gone were left, as that's over half the library",
            thousands(s.not_forgotten)
        ));
    }
    if let Some(why) = &s.stopped {
        line.push_str(&format!("; it stopped part way: {why}"));
    }
    line
}

/// What the card says once a scan that listened to sounds is done: a headline, and a line that's never empty.
pub fn scanned_card(s: &Scanned) -> (String, String) {
    let k = &s.kept;
    // A file that couldn't be listened to isn't a sound found.
    // Sounds tried again because the reader learnt more didn't change.
    let changed = k.updated.saturating_sub(s.tried_again);
    let title = match (k.added.saturating_sub(s.unheard_new), changed, s.tried_again) {
        (0, 0, 0) => "Couldn't listen to your new sound files".to_owned(),
        (0, 0, _) => "Tried the sounds that couldn't be read before".to_owned(),
        (0, _, _) => "Listened to your changed sounds".to_owned(),
        (1, _, _) => "Found a new sound".to_owned(),
        (n, _, _) => format!("Found {} new sounds", thousands(n)),
    };
    let mut detail = Vec::new();
    if changed > 0 {
        detail.push(format!("{} changed, listened to again", thousands(changed)));
    }
    if s.tried_again > 0 {
        detail.push(match s.heard_now {
            0 => format!("{} still can't be read", thousands(s.tried_again)),
            n if n == s.tried_again => format!("{} that couldn't be read before can be now", thousands(n)),
            n => format!("{} of {} that couldn't be read before can be now", thousands(n), thousands(s.tried_again)),
        });
    }
    if k.moved > 0 {
        detail.push(format!("{} moved", thousands(k.moved)));
    }
    if k.forgotten > 0 {
        detail.push(format!("{} gone", thousands(k.forgotten)));
    }
    if s.unreadable > 0 {
        detail.push(format!("{} couldn't be listened to", thousands(s.unreadable)));
    }
    // Never empty: the card would keep saying how far listening got.
    if detail.is_empty() {
        return (title, "They're in your sound library now.".to_owned());
    }
    (title, format!("{}.", detail.join(", ")))
}

/// What the card says when a scan that listened to sounds stopped part way ([`Scanned::stopped`]).
pub fn stopped_card(s: &Scanned) -> (String, String) {
    let why = s.stopped.as_deref().unwrap_or_default().trim_end_matches('.');
    let k = &s.kept;
    if k.added + k.updated == 0 {
        return ("Couldn't keep your new sounds".to_owned(), format!("{why}. They're tried again at the next start."));
    }
    (
        "Couldn't keep all your new sounds".to_owned(),
        format!(
            "{} were kept, then: {why}. The rest are tried again at the next start.",
            thousands(k.added + k.updated)
        ),
    )
}

/// What the card says when sounds that look gone were left ([`MOST_GONE`]).
pub fn not_forgotten_line(s: &Scanned) -> String {
    format!(
        "{} sounds look gone from your sample folders, over half your library, so they weren't forgotten. If a drive \
         or folder moved, check the sample folders in Settings.",
        thousands(s.not_forgotten)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_a_scan_did() {
        let s = Scanned {
            found: 453_885,
            kept: grow::Kept { added: 1_204, updated: 3, moved: 12, forgotten: 2 },
            unreadable: 2,
            unheard_new: 1,
            tried_again: 0,
            heard_now: 0,
            unnoted: None,
            not_forgotten: 0,
            took: Duration::from_millis(61_250),
            stopped: None,
            listened: Vec::new(),
        };
        assert_eq!(
            scanned_line(&s),
            "453,885 sound files in the sample folders: 1,204 new, 3 listened to again, 12 moved, 2 gone, in 61.2 s; \
             2 couldn't be listened to"
        );
        assert_eq!(
            scanned_card(&s),
            (
                "Found 1,203 new sounds".to_owned(),
                "3 changed, listened to again, 12 moved, 2 gone, 2 couldn't be listened to.".to_owned()
            )
        );
        let one = Scanned { kept: grow::Kept { added: 1, ..grow::Kept::default() }, ..Scanned::default() };
        assert_eq!(
            scanned_card(&one),
            ("Found a new sound".to_owned(), "They're in your sound library now.".to_owned())
        );
        let left = Scanned { not_forgotten: 230_000, ..Scanned::default() };
        assert!(not_forgotten_line(&left).starts_with("230,000 sounds look gone"));
        assert!(scanned_line(&left).ends_with("230,000 that look gone were left, as that's over half the library"));
        // Only new files that couldn't be listened to.
        let unheard = Scanned {
            kept: grow::Kept { added: 2, ..grow::Kept::default() },
            unreadable: 2,
            unheard_new: 2,
            ..Scanned::default()
        };
        assert_eq!(scanned_card(&unheard).0, "Couldn't listen to your new sound files");
        // Only sounds tried again with a reader that reads more.
        let again = Scanned {
            kept: grow::Kept { updated: 40, ..grow::Kept::default() },
            tried_again: 40,
            heard_now: 37,
            ..Scanned::default()
        };
        assert_eq!(
            scanned_card(&again),
            (
                "Tried the sounds that couldn't be read before".to_owned(),
                "37 of 40 that couldn't be read before can be now.".to_owned()
            )
        );
        assert!(
            scanned_line(&again).ends_with("; 40 that couldn't be read before were tried again, 37 can be read now")
        );
        let none = Scanned { heard_now: 0, ..again.clone() };
        assert_eq!(scanned_card(&none).1, "40 still can't be read.");
        // Stopped part way, and before anything was kept.
        let stopped = Scanned {
            kept: grow::Kept { added: 128, ..grow::Kept::default() },
            stopped: Some("There isn't room on the disk.".to_owned()),
            ..Scanned::default()
        };
        assert_eq!(
            stopped_card(&stopped),
            (
                "Couldn't keep all your new sounds".to_owned(),
                "128 were kept, then: There isn't room on the disk. The rest are tried again at the next start."
                    .to_owned()
            )
        );
        assert!(scanned_line(&stopped).ends_with("; it stopped part way: There isn't room on the disk."));
        let none = Scanned { stopped: Some("busy".to_owned()), ..Scanned::default() };
        assert_eq!(stopped_card(&none).0, "Couldn't keep your new sounds");
    }
}
