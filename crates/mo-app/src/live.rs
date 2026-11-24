//! Keeping the Songs list up to date while the app is open, as the C# app's folder watch does: a project saved, added,
//! moved or deleted in your project folders is read (or marked missing) a moment later, without walking every folder
//! again. When the folder watch can't name every file, the folders it names are walked with the scanner instead.
//!
//! Once the library is the app's own `songs.db`, what's read here is kept there too ([`Live::keep`], in
//! [`mo_library::song_writes`]), so the next start lists it without reading the files again. Before that (the app
//! still reading the C# app's `library.db`, which it never writes) it's held in memory only. A save read again keeps
//! its id; a new one gets an id after the library's. The song panel takes the details of either from here rather than
//! from the library.
//!
//! At the start, once the list is up, every project folder is walked once ([`Live::catch_up`]): saves made or changed
//! while the app was closed are read, and ones gone are marked missing, as the C# app's scan at its start did.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mo_formats::dates::RealDates;
use mo_formats::{PluginRef, case, paths};
use mo_library::read::{Installed, read_save};
use mo_library::samples::SampleLocator;
use mo_library::scan::{self, KnownFile};
use mo_library::song_fields::{self, Change, KeepCopy, SaveRow};
use mo_library::song_writes::{self, Batch, Missing, Put, Written};
use mo_library::songs;
use mo_library::watch::Changes;
use mo_library::{Library, LibraryError, Save, SaveDetail, SongChoices};
use mo_library::{plugin_writes, sample_writes};
use rayon::prelude::*;

use crate::background::{on_bulk_pool, on_pool};
use crate::songs::{ASK_DOUBTS, DOUBTS_ACROSS_FOLDERS, RUN_GAP, Songs, run_gap, shown_tags};

/// The details of the saves read here, by their ids, shared with the song panel's reader.
pub type LiveDetails = Arc<Mutex<HashMap<i64, SaveDetail>>>;

/// How long the folders a sample locator has read are trusted: saves a minute apart share them, later ones look again.
const SAMPLES_FRESH: Duration = Duration::from_secs(60);

/// How many times a save that couldn't be read (still being written, or locked) is tried again.
const TRIES: u8 = 3;

/// How long after songs.db refused something for longer than a moment it's tried again (review SG-12): a look again at
/// which plugins are installed, or the note that the muted samples are mended. Not at every pass of the folder watch.
const REFUSED_AGAIN_AFTER: Duration = Duration::from_secs(60);

/// How long a folder found away (on a drive that isn't plugged in, a network place that can't be reached, or busy or
/// denied) isn't looked in again for the saves in it waiting to be read again (review SG-6): a network place that's
/// gone can take seconds to answer each look.
const AWAY_AGAIN_AFTER: Duration = Duration::from_secs(60);

/// Everything the Songs list is made from, kept so it can be made again when a save changes.
pub struct Live {
    saves: Vec<Save>,
    choices: SongChoices,
    tags: Vec<String>,
    gap_months: u32,
    asking: Option<bool>,
    roots: Vec<String>,
    /// The export folder Open in Bitwig writes its copies into, as it was when the list was loaded: never listed, even
    /// inside a project folder, as Presets leaves them out (review finding A-7).
    export: Option<String>,
    installed: Installed,
    next_id: i64,
    details: LiveDetails,
    /// The folders read for the last saves' samples, and when.
    samples: Option<(Instant, Arc<SampleLocator>)>,
    /// The Sounds page's database, where missing samples are looked for last; none in tests.
    sounds_db: Option<PathBuf>,
    /// Saves whose read failed, to try again, with how many tries are left.
    retry: HashMap<String, u8>,
    /// Where the list came from when it was loaded, for the log (`Library::saves_and_source`).
    source: Option<String>,
    /// The app's own `songs.db`, where what's read is kept ([`Live::with_writes`]); none while it's held here only.
    writes: Option<PathBuf>,
    /// What's still to be kept there: saves read, and saves gone missing (by id).
    to_put: BTreeSet<i64>,
    to_mark: BTreeSet<i64>,
    /// The path the library keeps each listed save under, by id ([`Put::known_as`]): not the list's while a save that
    /// moved isn't kept yet (the library refused it, say). One the library has never had isn't in it.
    kept_as: HashMap<i64, String>,
    /// Reading again the saves whose "muted" sample marks were lost ([`song_writes::to_mend`]).
    mend: Mend,
    /// How many plugin measures had been kept when the installed plugins were last read
    /// ([`plugin_writes::measures_kept`]).
    measures_seen: u64,
    /// When a look again at which plugins are installed that failed may be tried again ([`REFUSED_AGAIN_AFTER`]).
    recheck_after: Option<Instant>,
    /// How many scans had changed the Sounds library when the samples marked missing were last looked for
    /// ([`crate::sound_scan::sounds_changed`]), and when a look that failed may be tried again.
    sounds_seen: u64,
    samples_after: Option<Instant>,
    /// When the note that the muted samples are mended, refused, may be tried again ([`REFUSED_AGAIN_AFTER`]).
    note_after: Option<Instant>,
    /// Folders found away as saves in them waited to be read again, by key, and when ([`AWAY_AGAIN_AFTER`]).
    away: HashMap<String, Instant>,
    /// Whether the log has said that songs.db has some saves under other ids by now ([`Live::give`]): once a run.
    said_renumbered: bool,
    /// Unchanged saves to read again because a plugin marked installed in them isn't found now
    /// ([`plugin_writes::Rechecked::to_read`], review P-2): their own read says whether it's missing.
    reread: HashSet<i64>,
    /// Those read again, to take off the library's note once they're kept ([`plugin_writes::read_again_kept`]).
    reread_put: HashSet<i64>,
    /// Notes that a song is shown from a save that moved before the library had it ([`Batch::moved_notes`], review
    /// SS-1): kept with the next batch.
    moved_notes: Vec<(String, String)>,
    /// Set as the app closes ([`Live::stopper`]): a walk of the project folders, or a batch of saves being read, stops
    /// part way (review SS-3).
    stop: Arc<AtomicBool>,
}

/// Where reading again the saves whose "muted" sample marks were lost is (review finding A-1): looked for once a start,
/// at the catch-up, read with it, and noted in the library once they're all read again and kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Mend {
    /// Not looked for yet this time.
    #[default]
    NotLooked,
    /// The saves still to be read again (by id, unchanged or not); those that couldn't be read this time (in a folder
    /// that couldn't be read, a drive that's away, not on the list or not found by the walk): noted as owed, and only
    /// they are read again at the next start (review finding G-1; for the Bitwig saves, the whole-app review of 19 Nov:
    /// one not read again can't say which multisample copies it uses); and those read again: one of them the library
    /// then won't take is owed too, not every save read again at the next start.
    Reading { ids: HashSet<i64>, owed: HashSet<i64>, read: HashSet<i64> },
    /// Nothing (more) to do this time.
    Done,
}

impl Mend {
    fn has(&self, id: i64) -> bool {
        matches!(self, Mend::Reading { ids, .. } if ids.contains(&id))
    }

    /// `id` was read again (`read`), or couldn't be.
    fn tried(&mut self, id: i64, read: bool) {
        if let Mend::Reading { ids, owed, read: done } = self
            && ids.remove(&id)
        {
            if read {
                done.insert(id)
            } else {
                owed.insert(id)
            };
        }
    }

    /// Every save has been read again and kept, or couldn't be read: to be noted with the next batch, with those owed
    /// ([`Mend::owed`]).
    fn to_note(&self) -> bool {
        matches!(self, Mend::Reading { ids, .. } if ids.is_empty())
    }

    /// The saves that couldn't be read this time, by id, in order.
    fn owed(&self) -> Vec<i64> {
        let Mend::Reading { owed, .. } = self else { return Vec::new() };
        let mut owed: Vec<i64> = owed.iter().copied().collect();
        owed.sort_unstable();
        owed
    }
}

/// What [`Live::update`] did, for its card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Updated {
    /// The saves read again or for the first time, by name.
    pub read: Vec<String>,
    /// The saves found in a new folder, by name.
    pub moved: Vec<String>,
    /// How many saves' files have gone (moved somewhere the app doesn't watch, or deleted).
    pub gone: usize,
    /// How many unchanged saves were read again for their "muted" sample marks ([`song_writes::to_mend`]): kept, but
    /// not on the card.
    pub mended: usize,
    /// How many saves' count of missing plugins changed when their plugins were looked at again
    /// ([`plugin_writes::recheck_installed`]): kept already, not on the card.
    pub rechecked: usize,
    /// Whether it stopped part way as the app closed ([`Live::stopper`]): what's left is read at the next start.
    pub stopped: bool,
}

impl Updated {
    /// This and `more` together.
    pub fn and(mut self, more: Updated) -> Updated {
        self.read.extend(more.read);
        self.moved.extend(more.moved);
        self.gone += more.gone;
        self.mended += more.mended;
        self.rechecked += more.rechecked;
        self.stopped |= more.stopped;
        self
    }

    pub fn is_empty(&self) -> bool {
        !self.for_card() && self.mended == 0 && self.rechecked == 0
    }

    /// Whether there's anything to say on the card: a save read, moved or gone.
    pub fn for_card(&self) -> bool {
        !self.read.is_empty() || !self.moved.is_empty() || self.gone > 0
    }

    /// The card's title and line, as the C# app words them: "Updated Night Drive_15", "Saved in its folder, so the
    /// app read it again".
    pub fn card(&self) -> (String, String) {
        let (r, m, g) = (self.read.len(), self.moved.len(), self.gone);
        let title = match (r + m + g, self.read.first().or(self.moved.first())) {
            (1, Some(name)) => format!("Updated {name}"),
            (1, None) => "Updated 1 project".to_owned(),
            (n, _) => format!("Updated {n} projects"),
        };
        let line = match (r, m, g) {
            (1, 0, 0) => "Saved in its folder, so the app read it again".to_owned(),
            (_, 0, 0) => "Saved in their folders, so the app read them again".to_owned(),
            (0, 1, 0) => "Found in its new folder".to_owned(),
            (0, 0, 1) => "Missing: moved or deleted".to_owned(),
            _ => [(r, "saved"), (m, "moved"), (g, "missing")]
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, what)| format!("{n} {what}"))
                .collect::<Vec<_>>()
                .join(", "),
        };
        (title, line)
    }
}

/// Whether `path` is in the export folder `export` (Open in Bitwig's copies): the Songs list leaves them out, even when
/// the folder is inside a project folder. A folder changed in Settings counts from the next start.
fn in_export(path: &str, export: Option<&str>) -> bool {
    export.is_some_and(|e| scan::is_inside(path, e))
}

/// Which of the saves the library keeps the Songs page lists: not Fix's copies still waiting for Keep (until Keep, a
/// song opens its original), not a DAW's autosave copies the C# app's scan let in ("sly_17 (autosaved at 4h10).flp")
/// or saves in a folder the scan leaves out (a project folder inside one was taken off, FL's Backup), and not Open in
/// Bitwig's copies in the export folder. The Sounds page counts the songs a sound is used in from the same saves.
pub struct Listed {
    pending: mo_library::PathSet,
    roots: Vec<String>,
    export: Option<String>,
}

impl Listed {
    /// Reads what decides it. Reads `library`, so never on the window's thread.
    pub fn read(library: &Library) -> Result<Listed, LibraryError> {
        let pending = library.pending_fixes()?;
        let roots = library.roots()?;
        let export = crate::bitwig_convert::export_left_out(library)?.map(|e| e.display().to_string());
        Ok(Listed { pending, roots, export })
    }

    /// Whether the Songs page lists the save at `path`.
    pub fn has(&self, path: &str) -> bool {
        (self.pending.is_empty() || !self.pending.contains(path))
            && !scan::is_ignored(path, &self.roots)
            && !in_export(path, self.export.as_deref())
    }
}

/// What a change given to a song goes to ([`Live::given_to`]).
struct Given {
    /// The saves, by id: the song's, its copies too.
    ids: Vec<i64>,
    /// The changes, in order.
    changes: Vec<Change>,
    /// The copies the song is shown from, to keep with them (review SS-1).
    keep: Vec<KeepCopy>,
}

/// Of `song`'s files found in other folders too, the copy it's shown from and the file's other places, for each that
/// another of its copies would take over from once `changes` are made to them all ([`songs::shown_over`]), unless
/// `choices` notes it already ([`SongChoices::is_kept`], review SS-1). Most songs have no copies, and none; nor does a
/// copy that stays shown anyway, so nothing is kept that can be worked out (rule 23).
fn copies_to_keep(song: &songs::Song, choices: &SongChoices, changes: &[Change]) -> Vec<KeepCopy> {
    if song.copies.is_empty() {
        return Vec::new();
    }
    let after = |s: &Save| {
        let mut s = s.clone();
        for change in changes {
            change.apply(&mut s);
        }
        s
    };
    let mut keep = Vec::new();
    for shown in song.saves() {
        let Some(file) = songs::file_id(shown) else { continue };
        let copies: Vec<&Save> = song.copies.iter().filter(|c| songs::file_id(c).as_ref() == Some(&file)).collect();
        if copies.is_empty() {
            continue;
        }
        let copy_noted = copies.iter().any(|c| choices.is_kept(&c.path));
        if !copy_noted && choices.is_kept(&shown.path) {
            continue;
        }
        let shown_after = after(shown);
        if !copy_noted && copies.iter().all(|c| songs::shown_over(&shown_after, &after(c))) {
            continue;
        }
        keep.push(KeepCopy { shown: shown.path.clone(), copies: copies.iter().map(|c| c.path.clone()).collect() });
    }
    keep
}

impl Live {
    /// Reads what the list is made from. Fix's copies still waiting for Keep or Delete are left out: until Keep, a
    /// song opens its original. Runs of saves split at the gap picked in Settings (`version_gap_months`, shared with
    /// the C# app), or at `gap_months` when none is picked.
    pub fn load(library: &Library, gap_months: u32, details: LiveDetails) -> Result<Live, LibraryError> {
        let listed = Listed::read(library)?;
        let (mut saves, source) = library.saves_and_source()?;
        // After every row of the library, Fix's copies left out below included, so no two saves share an id.
        let next_id = saves.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        // The saves the list leaves out stay in the library.
        saves.retain(|s| listed.has(&s.path));
        let Listed { roots, export, .. } = listed;
        // As the C# app: asked unless Settings turned it off; across folders only when turned on.
        let asking = (library.setting(ASK_DOUBTS)?.as_deref() != Some("0"))
            .then(|| library.setting(DOUBTS_ACROSS_FOLDERS).map(|v| v.as_deref() == Some("1")))
            .transpose()?;
        let gap_months = run_gap(library.setting(RUN_GAP)?.as_deref(), gap_months);
        let kept_as = saves.iter().map(|s| (s.id, s.path.clone())).collect();
        Ok(Live {
            saves,
            choices: library.song_choices()?,
            tags: library.all_tags()?.into_iter().map(|(t, _)| t).collect(),
            gap_months,
            asking,
            roots,
            export,
            installed: library.installed()?,
            next_id,
            details,
            samples: None,
            sounds_db: None,
            retry: HashMap::new(),
            source,
            writes: None,
            to_put: BTreeSet::new(),
            to_mark: BTreeSet::new(),
            kept_as,
            mend: Mend::NotLooked,
            measures_seen: plugin_writes::measures_kept(),
            recheck_after: None,
            sounds_seen: 0,
            samples_after: None,
            note_after: None,
            away: HashMap::new(),
            said_renumbered: false,
            reread: HashSet::new(),
            reread_put: HashSet::new(),
            moved_notes: Vec::new(),
            stop: Arc::default(),
        })
    }

    /// Keeps what's read in `songs_db`, the library it was loaded from, once that's the app's own ([`Live::keep`]).
    pub fn with_writes(mut self, songs_db: PathBuf) -> Live {
        self.writes = Some(songs_db);
        self
    }

    /// Whether what's read is kept in the library ([`Live::with_writes`]).
    pub fn writes(&self) -> bool {
        self.writes.is_some()
    }

    /// Looks for missing samples in the Sounds library at `sounds_db` too (rule 13).
    pub fn with_sounds(mut self, sounds_db: PathBuf) -> Live {
        self.sounds_db = Some(sounds_db);
        self
    }

    /// Where the list came from when it was loaded ("from the app's copy"...), for the log.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// The project folders, for the folder watch.
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Gives the song `key` `change`, as you gave it on its card (rule 14: every save of it gets it, its copies in other
    /// folders too): kept in the library ([`mo_library::song_fields::set_all_keeping`]: all of it or none) and then
    /// made to the saves held here, so the list shows what's kept. Call it on the thread that owns this list, as
    /// [`Live::update`] and [`Live::keep`] are: then no save can be read, or kept, between the two. It waits for another
    /// write to the library, up to five seconds.
    ///
    /// The song's saves are those held here now (review SG-8): a save read since the card was shown is the song's too,
    /// and one the library numbered anew is found by its new id. `save_ids`, the saves the card was shown with, are
    /// given it only when no song has that key now. A tag put on goes with the tags the card showed (review SG-1), and
    /// one taken off leaves the others the card showed on every save (review SS-2). Of a file also found in another
    /// folder, the copy the song is shown from is kept with the change (review SS-1), so the change, which goes to the
    /// copies too, can't make another copy take over.
    ///
    /// A save the library hasn't taken yet (just read, or refused) is changed here only, and is kept with the change
    /// when the library takes it. While the list isn't kept in a library of the app's own ([`Live::writes`]) the change
    /// is made here only, and lasts until the app closes.
    ///
    /// If the library can't keep it nothing is changed, here or there. What it did ([`ChangeMade`]): whether anything
    /// held here is different for it, and whether it's held here only.
    pub fn give(&mut self, key: &str, save_ids: &[i64], change: &Change) -> Result<ChangeMade, LibraryError> {
        let Given { ids: save_ids, changes, keep } = self.given_to(key, save_ids, change);
        let held_only = if let Some(songs_db) = &self.writes {
            // By id and the path the library keeps each under: an id alone may be another save's there by now.
            let rows: Vec<SaveRow> = save_ids
                .iter()
                .filter_map(|id| self.kept_as.get(id).map(|path| SaveRow { id: *id, path: path.clone() }))
                .collect();
            let kept = song_fields::set_all_keeping(songs_db, &rows, &changes, &keep)?;
            // Saves the library has under other ids by now (another copy of the app kept them anew): changed here only,
            // and said, once a run (review SG-2). One it hasn't taken yet is kept with the change when it does.
            if !kept.unknown.is_empty() && !std::mem::replace(&mut self.said_renumbered, true) {
                crate::log::warning(
                    "Songs",
                    &format!(
                        "{} saves of the song aren't in songs.db as the list has them (another copy of the app may have \
                         kept them anew): what you gave them is shown until the app closes",
                        kept.unknown.len()
                    ),
                );
            }
            !kept.unknown.is_empty()
        } else if let Some(why) = changes.iter().find_map(|c| c.checked().err()) {
            return Err(LibraryError::Write { path: PathBuf::new(), why });
        } else {
            true
        };
        // The copy the song is shown from stays so, here too (kept in the library with the change, or, while it's held
        // here only, until the app closes).
        for k in &keep {
            self.choices.keep(&k.shown, k.copies.iter().map(String::as_str));
        }
        let mut differs = false;
        for change in &changes {
            differs |= self.gave(&save_ids, change);
        }
        // Nothing different is nothing held.
        Ok(ChangeMade { differs, held_only: held_only && differs })
    }

    /// The saves a change given to the song `key` goes to ([`Live::give`]), the changes it makes, and the copies the
    /// song is shown from: the song's saves and copies as they group now, else `save_ids`.
    ///
    /// A tag put on comes after the tags the song shows: while its newest save has none it shows an older save's, and
    /// the newest holding the new tag alone would hide them (review SG-1). A tag taken off leaves every save the other
    /// tags the song shows, so none of an older save's that it never showed can show through (review SS-2); when it
    /// was the last one shown, every tag the song's saves hold goes with it.
    fn given_to(&self, key: &str, save_ids: &[i64], change: &Change) -> Given {
        let real = RealDates::now();
        let grouped = songs::group_songs(&self.saves, &self.choices, &real);
        let Some(song) = grouped.iter().find(|s| s.key == key) else {
            return Given { ids: save_ids.to_vec(), changes: vec![change.clone()], keep: Vec::new() };
        };
        let ids: Vec<i64> = song.saves().chain(&song.copies).map(|s| s.id).collect();
        // The tags the song shows, to put on every save: not the hardware tags the newest save holds for hardware it
        // uses, which stay on it by themselves, so a save that doesn't use that hardware isn't tagged with it. One the
        // song shows from an older save goes on the newest too, or it would stop showing. One an older library holds
        // that couldn't be put on now (too long, say) stays on the save it's on.
        let held: HashSet<String> = song.latest.tags.iter().map(|t| song_fields::same_tag(t)).collect();
        let own_hardware: HashSet<String> =
            song.latest.hardware.iter().map(|h| song_fields::same_tag(h)).filter(|t| held.contains(t)).collect();
        let put_on = |tags: Vec<String>| {
            tags.into_iter()
                .filter(|t| !own_hardware.contains(&song_fields::same_tag(t)))
                .map(Change::AddTag)
                .filter(|c| c.checked().is_ok())
                .collect::<Vec<_>>()
        };
        let mut changes = Vec::new();
        match change.checked() {
            Ok(Change::AddTag(_)) => changes.extend(put_on(shown_tags(song, &real))),
            Ok(Change::RemoveTag(tag)) => {
                let others: Vec<String> =
                    shown_tags(song, &real).into_iter().filter(|t| song_fields::same_tag(t) != tag).collect();
                if others.is_empty() {
                    // Hardware tags too: one taken off isn't put back when its save is read again
                    // ([`keep_your_fields`]).
                    let held: BTreeSet<String> = song
                        .saves()
                        .chain(&song.copies)
                        .flat_map(|s| s.tags.iter().map(|t| song_fields::same_tag(t)))
                        .filter(|t| *t != tag && !t.is_empty())
                        .collect();
                    changes.extend(held.into_iter().map(Change::RemoveTag));
                } else {
                    changes.extend(put_on(others));
                }
            }
            _ => {}
        }
        changes.push(change.clone());
        let keep = copies_to_keep(song, &self.choices, &changes);
        Given { ids, changes, keep }
    }

    /// Makes `change` to the saves held here, the panel's copies of them too ([`Live::give`]).
    fn gave(&mut self, save_ids: &[i64], change: &Change) -> bool {
        let mut differs = false;
        for save in self.saves.iter_mut().filter(|s| save_ids.contains(&s.id)) {
            differs |= change.apply(save);
        }
        let mut details = self.details.lock().unwrap_or_else(PoisonError::into_inner);
        for id in save_ids {
            if let Some(detail) = details.get_mut(id) {
                differs |= change.apply(&mut detail.save);
            }
        }
        // The list of every tag gains a new one at its end (the least used). One taken off stays in it until the app
        // next starts: a save the list leaves out may still have it.
        if let Ok(Change::AddTag(tag)) = change.checked()
            && !self.tags.iter().any(|t| song_fields::same_tag(t) == tag)
        {
            self.tags.push(tag);
        }
        differs
    }

    /// The list, grouped into songs.
    pub fn songs(&self) -> Songs {
        Songs::from_saves(&self.saves, &self.choices, self.tags.clone(), self.gap_months, self.asking)
    }

    /// The saves whose read failed last time, to read again a moment later; None when there are none.
    /// Also Some (with nothing in it) when the Plugins page measured your plugins since the last look, so
    /// [`Live::update`] looks again at which are installed while no project file changes (review G-3).
    pub fn retries(&mut self) -> Option<Changes> {
        let files: Vec<String> = self.retry.keys().cloned().collect();
        (!files.is_empty()
            || (self.measures_seen != plugin_writes::measures_kept() && !self.recheck_waits())
            || self.samples_due())
        .then_some(Changes { files, ..Changes::default() })
    }

    /// Whether a Sounds scan changed the library since the samples marked missing were last looked for, and a look
    /// that failed isn't waiting.
    fn samples_due(&self) -> bool {
        self.sounds_seen != crate::sound_scan::sounds_changed()
            && self.samples_after.is_none_or(|at| Instant::now() >= at)
    }

    /// Whether a look again at which plugins are installed failed a moment ago, so it waits a while.
    fn recheck_waits(&self) -> bool {
        self.recheck_after.is_some_and(|at| Instant::now() < at)
    }

    /// The switch that stops [`Live::catch_up`] and [`Live::update`] part way, set as the app closes: what you gave
    /// songs on their cards waits behind them, and would be lost if the app ended first (review SS-3). A folder or a
    /// save being read is finished first; nothing more is read, and what's left is read at the next start. Never set
    /// back.
    pub fn stopper(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Walks every project folder once, as the C# app's scan did at its start: saves made or changed while the app was
    /// closed are read, and ones whose file has gone are marked missing. Only what differs from the library (by size
    /// and date) is read. Asks the disk, so never on the window's thread.
    ///
    /// The first time, saves whose "muted" sample marks the app lost are read again too, unchanged or not
    /// ([`song_writes::to_mend`]); once they're kept, the library notes it ([`Live::keep`]) and they aren't read again.
    ///
    /// As the app closes it stops part way ([`Live::stopper`]), and the next start's catch-up does it all again.
    pub fn catch_up(&mut self) -> Updated {
        if self.stopping() {
            return Updated { stopped: true, ..Updated::default() };
        }
        self.look_for_mends();
        let rechecked = self.recheck_installed();
        let roots = self.roots.clone();
        if roots.is_empty() {
            self.mend = Mend::Done;
            return Updated { rechecked: rechecked + self.recheck_samples(), ..Updated::default() };
        }
        let mut done = self.update(&Changes { overflowed: roots, ..Changes::default() });
        done.rechecked += rechecked;
        // The saves still to mend that it didn't get to aren't taken off.
        if done.stopped {
            return done;
        }
        // Samples marked missing, looked for again when the Sounds library changed since the last look (a folder
        // added and scanned, in this run or an earlier one): after the walk, so new and changed saves list first.
        done.rechecked += self.recheck_samples();
        // One the walk didn't find (gone, so marked missing, or outside the project folders now) can't be read again
        // now: owed (one marked missing is left off the owed list at the next start, `song_writes::to_mend`). Only
        // those waiting to be tried again are still to come. One in a folder that couldn't be read is owed already
        // ([`Live::walk`]).
        if let Mend::Reading { ids, owed, .. } = &mut self.mend {
            let waiting: HashSet<String> = self.retry.keys().map(|p| case::key(p)).collect();
            let waiting: HashSet<i64> =
                self.saves.iter().filter(|s| waiting.contains(&case::key(&s.path))).map(|s| s.id).collect();
            owed.extend(ids.iter().filter(|id| !waiting.contains(id)));
            ids.retain(|id| waiting.contains(id));
        }
        done
    }

    /// Looks again at whether every save's plugins are installed, when the installed plugins or the rules changed
    /// since the last look ([`plugin_writes::recheck_installed`]), and shows the new counts; says how many saves
    /// changed. Saves read from now on use the installed plugins as they are now. A look that fails is tried again a
    /// minute later ([`REFUSED_AGAIN_AFTER`]), or at the next start.
    fn recheck_installed(&mut self) -> usize {
        // Taken before the look, so a measure kept meanwhile is looked at next time; only kept once the look worked,
        // so one that failed is tried again.
        let measures = plugin_writes::measures_kept();
        let Some(songs_db) = &self.writes else {
            self.measures_seen = measures;
            return 0;
        };
        if self.recheck_waits() {
            return 0;
        }
        let r = match plugin_writes::recheck_installed(songs_db) {
            Ok(r) => {
                self.measures_seen = measures;
                self.recheck_after = None;
                r
            }
            Err(e) => {
                crate::log::warning(
                    "Songs",
                    &format!("{e}; which plugins are installed is looked at again in a minute"),
                );
                self.recheck_after = Some(Instant::now() + REFUSED_AGAIN_AFTER);
                return 0;
            }
        };
        if r.looked {
            crate::log::info(
                "Songs",
                &format!(
                    "Looked again at which plugins are installed: {} saves' missing plugins changed",
                    r.missing.len()
                ),
            );
        }
        self.installed = r.installed;
        if !r.to_read.is_empty() {
            let listed: HashSet<i64> = self.saves.iter().map(|s| s.id).collect();
            let before = self.reread.len();
            self.reread.extend(r.to_read.into_iter().filter(|id| listed.contains(id)));
            crate::log::info(
                "Songs",
                &format!(
                    "{} saves name a plugin that isn't found by its name or file now: read again to see if it's missing",
                    self.reread.len() - before
                ),
            );
        }
        let mut missing: HashMap<i64, u32> = r.missing.into_iter().collect();
        if r.looked {
            // Saves read but not kept yet were read against the installed plugins as they were: judged again too, so
            // keeping them doesn't put the old marks back.
            let mut held = self.details.lock().unwrap_or_else(PoisonError::into_inner);
            for d in held.values_mut() {
                let mut read_again = false;
                for p in d.plugins.iter_mut() {
                    let named = PluginRef {
                        name: p.name.clone(),
                        vendor: p.vendor.clone(),
                        kind: p.kind,
                        format: p.format.clone(),
                        path: p.path.clone(),
                        ..PluginRef::default()
                    };
                    let has = self.installed.has(&named);
                    if !p.installed {
                        p.installed = has;
                    } else if !has {
                        // Found by an id it no longer has, perhaps: its own read judges it again (review P-2).
                        read_again = true;
                    }
                }
                if read_again {
                    self.reread.insert(d.save.id);
                }
                let now = u32::try_from(d.plugins.iter().filter(|p| !p.installed).count()).unwrap_or(u32::MAX);
                if now != d.save.missing_plugins {
                    d.save.missing_plugins = now;
                    missing.insert(d.save.id, now);
                }
            }
        }
        let mut changed = 0;
        for s in &mut self.saves {
            if let Some(&n) = missing.get(&s.id) {
                s.missing_plugins = n;
                changed += 1;
            }
        }
        changed
    }

    /// Looks again for the samples marked missing, when the Sounds library has changed since the last look
    /// ([`sample_writes::recheck_found`]): a sample folder added in Settings and scanned may hold them, and a save is
    /// only read again when its file changes. Only the Sounds library is asked (it's all that changed since each
    /// sample was looked for), read only when there's a change and something missing. Shows the new counts; says how
    /// many saves changed. A look that fails (a library busy just then) is tried again a minute later, or at the next
    /// start.
    fn recheck_samples(&mut self) -> usize {
        // Taken before the look, so a scan that ends meanwhile is looked at next time.
        let seen = crate::sound_scan::sounds_changed();
        let (Some(songs_db), Some(sounds_db)) = (self.writes.clone(), self.sounds_db.clone()) else {
            self.sounds_seen = seen;
            return 0;
        };
        // The app's own sound library, whichever file the list was given when it loaded (the old app's, before the
        // app's own was made): its stamp and its sounds come from the same file.
        let own = mo_sounds::compact::copy_of(&sounds_db);
        let own = if sounds_db.file_name() == own.file_name() { sounds_db } else { own };
        let refused = |this: &mut Live, why: String| {
            crate::log::warning("Songs", &format!("{why}; the missing samples are looked for again in a minute"));
            this.samples_after = Some(Instant::now() + REFUSED_AGAIN_AFTER);
            0
        };
        let stamp = match mo_sounds::compact::grow::sounds_stamp(&own) {
            Ok(Some(stamp)) => stamp,
            // No sound library of the app's own: the old app's has nothing new to add.
            Ok(None) => {
                self.sounds_seen = seen;
                return 0;
            }
            Err(e) => return refused(self, format!("Couldn't read the Sounds library: {e}")),
        };
        let began = Instant::now();
        let stop = self.stop.clone();
        // Read only when it's needed, and fresh: the read [`crate::sound_library::shared`] keeps for a minute may be
        // from before the scan. One that couldn't read every sound isn't taken as the answer.
        let judge = || {
            let library = crate::sound_library::read_now(&own);
            if library.is_complete() {
                Ok(move |sample: &str, _project: &str| library.has_sample(sample))
            } else {
                Err("the Sounds library couldn't be read whole just now".to_owned())
            }
        };
        let r =
            on_bulk_pool(|| sample_writes::recheck_found(&songs_db, &stamp, judge, || stop.load(Ordering::Relaxed)));
        let r = match r {
            Ok(r) => r,
            Err(e) => return refused(self, e.to_string()),
        };
        self.sounds_seen = seen;
        self.samples_after = None;
        if !r.looked {
            return 0;
        }
        // The locator read before has the Sounds library as it was: the next save read takes a new one.
        self.samples = None;
        crate::log::info(
            "Songs",
            &format!(
                "Looked again for {} samples marked missing, as the Sounds library changed: {} found now, in {} saves, \
                 in {:.1} s{}",
                r.looked_for,
                r.found,
                r.missing.len(),
                began.elapsed().as_secs_f32(),
                if r.stopped { "; stopped as the app closed, so it looks again next time" } else { "" }
            ),
        );
        let mut missing: HashMap<i64, u32> = r.missing.into_iter().collect();
        // Saves read but not kept yet hold samples of their own (songs.db may have older ones for them), read against
        // the Sounds library as it was: looked at again too, so keeping them doesn't put the old marks back, and their
        // own count is the one shown. Judged away from the lock the song panel's reader shares.
        let held: Vec<(i64, Vec<String>)> = {
            let held = self.details.lock().unwrap_or_else(PoisonError::into_inner);
            held.values()
                .map(|d| (d.save.id, d.samples.iter().filter(|s| !s.found).map(|s| s.path.clone()).collect()))
                .collect()
        };
        if !held.is_empty() {
            let library = crate::sound_library::shared(&own);
            let found: HashMap<i64, HashSet<String>> = on_bulk_pool(|| {
                held.into_par_iter()
                    .map(|(id, samples)| (id, samples.into_iter().filter(|s| library.has_sample(s)).collect()))
                    .collect()
            });
            let mut details = self.details.lock().unwrap_or_else(PoisonError::into_inner);
            for d in details.values_mut() {
                if let Some(now) = found.get(&d.save.id).filter(|f| !f.is_empty()) {
                    for s in d.samples.iter_mut().filter(|s| now.contains(&s.path)) {
                        s.found = true;
                    }
                    // Missing ones first, then by path, as a save's read sorts them.
                    d.samples.sort_by_cached_key(|s| (s.found, case::to_lower(&s.path)));
                }
                let now = u32::try_from(d.samples.iter().filter(|s| !s.found).count()).unwrap_or(u32::MAX);
                d.save.missing_samples = now;
                missing.insert(d.save.id, now);
            }
        }
        let mut changed = 0;
        for s in &mut self.saves {
            if let Some(&n) = missing.get(&s.id)
                && s.missing_samples != n
            {
                s.missing_samples = n;
                changed += 1;
            }
        }
        changed
    }

    /// Asks the library once a start which saves to read again for their "muted" sample marks. One it can't tell is
    /// asked again at the next start.
    fn look_for_mends(&mut self) {
        if self.mend != Mend::NotLooked {
            return;
        }
        self.mend = Mend::Done;
        let Some(songs_db) = &self.writes else { return };
        match song_writes::to_mend(songs_db) {
            Ok(Some(ids)) => {
                // One not on the list (in a folder taken off the project folders, say) can't be read here: owed.
                let listed: HashSet<i64> = self.saves.iter().map(|s| s.id).collect();
                let (ids, owed) = ids.into_iter().partition(|id| listed.contains(id));
                self.mend = Mend::Reading { ids, owed, read: HashSet::new() };
            }
            Ok(None) => {}
            Err(e) => {
                crate::log::warning("Songs", &format!("{e}; the saves to read again once are read at the next start"))
            }
        }
    }

    /// Whether the note that the muted samples were mended is still to be kept, though nothing else may be
    /// ([`Live::keep`]): not for a minute after songs.db refused it (review SG-12).
    pub fn note_to_keep(&self) -> bool {
        self.mend.to_note() && self.note_after.is_none_or(|at| Instant::now() >= at)
    }

    /// Keeps what's been read since the last time in the app's own `songs.db` ([`Live::with_writes`]): `None` when
    /// there's nothing to keep, or nowhere to keep it. If the library is busy (another copy of the app holding it), it's
    /// all kept the next time, and meanwhile the list shows it as before. A save the library refuses is kept on its
    /// own, so it can't hold up the rest; one that can't be kept at all is held here only this time
    /// ([`Kept::not_kept`]). Writes to the disk, so never on the window's thread.
    pub fn keep(&mut self) -> Option<Result<Kept, LibraryError>> {
        let songs_db = self.writes.clone()?;
        let mut batch = self.batch();
        batch.mended = self.note_to_keep();
        batch.owed = if batch.mended { self.mend.owed() } else { Vec::new() };
        let put: Vec<(i64, String)> =
            batch.put.iter().map(|p| (p.detail.save.id, p.detail.save.path.clone())).collect();
        if batch.is_empty() {
            self.to_put.clear();
            self.to_mark.clear();
            return None;
        }
        let kept = match song_writes::write(&songs_db, &batch) {
            Ok(written) => Kept { written, ..Kept::default() },
            Err(e) if e.is_busy() => return Some(Err(e)),
            Err(_) => match one_by_one(&songs_db, batch) {
                Ok(kept) => kept,
                Err(e) => return Some(Err(e)),
            },
        };
        self.renumber(&kept.written.ids);
        // In the library now, where the song panel reads them; ones that couldn't be kept stay here.
        let held: HashSet<String> = kept.not_kept.iter().map(|(path, _)| case::key(path)).collect();
        self.note_kept(&put, &kept.written.ids, &held);
        let done: Vec<i64> = self
            .saves
            .iter()
            .filter(|s| !held.contains(&case::key(&s.path)))
            .map(|s| s.id)
            .filter(|id| self.to_put.contains(id) || kept.written.ids.iter().any(|&(_, k)| k == *id))
            .collect();
        let mut details = self.details.lock().unwrap_or_else(PoisonError::into_inner);
        for id in done {
            details.remove(&id);
        }
        drop(details);
        self.to_put.clear();
        self.to_mark.clear();
        self.moved_notes.clear();
        // Saves read again for a plugin that isn't found now, kept: off the library's note of them (review P-2). One
        // not taken off is only read again at the next start.
        let read_again: Vec<i64> = self
            .saves
            .iter()
            .filter(|s| self.reread_put.contains(&s.id) && !held.contains(&case::key(&s.path)))
            .map(|s| s.id)
            .collect();
        if !read_again.is_empty() {
            for id in &read_again {
                self.reread_put.remove(id);
            }
            if let Err(e) = plugin_writes::read_again_kept(&songs_db, &read_again) {
                crate::log::warning(
                    "Songs",
                    &format!("{e}; the saves read again for their plugins are read again next start"),
                );
            }
        }
        // A save read again that the library wouldn't take is owed, to be read again at the next start: in the batch
        // that noted the rest, added to the note's list.
        if let Mend::Reading { owed, read, .. } = &mut self.mend
            && !held.is_empty()
        {
            let refused: Vec<i64> = self
                .saves
                .iter()
                .filter(|s| read.contains(&s.id) && held.contains(&case::key(&s.path)))
                .map(|s| s.id)
                .collect();
            if kept.written.mended {
                if let Err(e) = song_writes::owe(&songs_db, &refused) {
                    crate::log::warning(
                        "Songs",
                        &format!("{e}; {} saves read again once that weren't kept aren't read again", refused.len()),
                    );
                }
            } else {
                for id in refused {
                    read.remove(&id);
                    owed.insert(id);
                }
            }
        }
        if kept.written.mended {
            self.mend = Mend::Done;
        } else if let Some(why) = &kept.not_noted {
            // Not a library busy for a moment: tried again at every pass it might fail at every pass, as long as the
            // app runs (review SG-12). So in a minute; if the app closes first, they're read again next start.
            crate::log::warning(
                "Songs",
                &format!("Couldn't note in songs.db that the muted samples are mended: {why}; tried again in a minute"),
            );
            self.note_after = Some(Instant::now() + REFUSED_AGAIN_AFTER);
        }
        Some(Ok(kept))
    }

    /// What's to be kept, each save as the list has it now, by the path the library keeps it under
    /// ([`Live::kept_as`]). A save the library has never had that's gone again is kept as it is now (missing), not
    /// marked.
    fn batch(&self) -> Batch {
        let details = self.details.lock().unwrap_or_else(PoisonError::into_inner);
        let mut batch = Batch::default();
        for s in &self.saves {
            let known_as = self.kept_as.get(&s.id);
            if self.to_put.contains(&s.id)
                && let Some(detail) = details.get(&s.id)
            {
                batch.put.push(Put {
                    detail: SaveDetail { save: s.clone(), ..detail.clone() },
                    known_as: known_as.cloned(),
                });
            }
            if self.to_mark.contains(&s.id)
                && let Some(path) = known_as
            {
                batch.missing.push(Missing { id: s.id, path: path.clone() });
            }
        }
        batch.moved_notes = self.moved_notes.clone();
        batch
    }

    /// Notes the paths the library keeps the saves just `put` under, as (id given, path), each by the id it kept it
    /// under (`ids`, as (given, kept)): the ones it wouldn't take (`held`, by [`case::key`]) apart. All at once, as one
    /// save's new id may be another's old one.
    fn note_kept(&mut self, put: &[(i64, String)], ids: &[(i64, i64)], held: &HashSet<String>) {
        let to: HashMap<i64, i64> = ids.iter().copied().collect();
        let kept: Vec<(i64, String)> = put
            .iter()
            .filter(|(_, path)| !held.contains(&case::key(path)))
            .map(|(given, path)| (to.get(given).copied().unwrap_or(*given), path.clone()))
            .collect();
        for given in put.iter().map(|(given, _)| given).filter(|given| to.contains_key(given)) {
            self.kept_as.remove(given);
        }
        self.kept_as.extend(kept);
    }

    /// Saves the library kept under another id than the one given here, as (given, kept): they go by it from now on.
    /// All at once, as one save's new id may be another's old one.
    fn renumber(&mut self, ids: &[(i64, i64)]) {
        if ids.is_empty() {
            return;
        }
        let to: HashMap<i64, i64> = ids.iter().copied().collect();
        for s in &mut self.saves {
            if let Some(&kept) = to.get(&s.id) {
                s.id = kept;
            }
        }
        let mut details = self.details.lock().unwrap_or_else(PoisonError::into_inner);
        let moved: Vec<(i64, SaveDetail)> = ids
            .iter()
            .filter_map(|&(given, kept)| {
                let mut detail = details.remove(&given)?;
                detail.save.id = kept;
                Some((kept, detail))
            })
            .collect();
        details.extend(moved);
        drop(details);
        let highest = ids.iter().map(|&(_, kept)| kept).max().unwrap_or(0);
        self.next_id = self.next_id.max(highest + 1);
    }

    /// Takes in what the folder watch saw: new and changed project files are read, gone ones marked missing, and
    /// folders it couldn't name each file in are walked with the scanner. Asks the disk, so never on the window's
    /// thread; the walking and reading are spread over the app's low-priority threads.
    pub fn update(&mut self, changes: &Changes) -> Updated {
        // The app is closing: nothing more is read ([`Live::stopper`]). A save to mend that isn't read stays to mend.
        if self.stopping() {
            return Updated { stopped: true, ..Updated::default() };
        }
        let mut done = Updated::default();
        // The Plugins page measured your plugins since: saves are judged against what it found from now on (review G-3).
        if self.measures_seen != plugin_writes::measures_kept() {
            done.rechecked += self.recheck_installed();
        }
        // A Sounds scan kept new sounds (a sample folder added, say): samples marked missing may be found now.
        if self.samples_due() {
            done.rechecked += self.recheck_samples();
        }
        let mut paths: Vec<String> = changes.files.clone();
        // Saves naming a plugin that isn't found now are read again, changed or not (review P-2). One in a folder
        // walked now is the walk's, which holds back one whose folder can't be read. Any other only once its folder
        // can be looked at (review SG-6): one on a drive that's away, or in a folder busy or denied, isn't marked
        // missing for it, and is read again when its folder is back. Each folder is looked at once a batch, and one
        // found away not again for a minute.
        if !self.reread.is_empty() {
            let walked = |path: &str| changes.overflowed.iter().any(|o| scan::is_inside(path, o));
            let now = Instant::now();
            self.away.retain(|_, since| now.duration_since(*since) < AWAY_AGAIN_AFTER);
            let mut folders: HashMap<String, bool> = HashMap::new();
            for s in &self.saves {
                // A folder that's away can take seconds to answer.
                if self.stopping() {
                    break;
                }
                if s.file_missing || !self.reread.contains(&s.id) || walked(&s.path) {
                    continue;
                }
                let folder = folder_key(&s.path);
                if self.away.contains_key(&folder) {
                    continue;
                }
                if *folders.entry(folder.clone()).or_insert_with(|| can_look_in(&s.path)) {
                    paths.push(s.path.clone());
                } else {
                    self.away.insert(folder, now);
                }
            }
            if self.stopping() {
                done.stopped = true;
                return done;
            }
        }
        // A path that went and may have been a folder: the saves listed inside it are looked at (the ones that went
        // are marked missing below), with no walk.
        if !changes.went.is_empty() {
            paths.extend(
                self.saves
                    .iter()
                    .filter(|s| !s.file_missing && changes.went.iter().any(|w| scan::is_inside(&s.path, w)))
                    .map(|s| s.path.clone()),
            );
        }
        if !changes.overflowed.is_empty() {
            self.walk(&changes.overflowed, &mut paths, &mut done);
            if self.stopping() {
                done.stopped = true;
                return done;
            }
        }
        let mut seen = HashSet::new();
        paths.retain(|p| {
            seen.insert(case::key(p))
                && scan::is_project_file(p)
                && !scan::is_ignored(p, &self.roots)
                && !in_export(p, self.export.as_deref())
        });
        let mut index: HashMap<String, usize> =
            self.saves.iter().enumerate().map(|(i, s)| (case::key(&s.path), i)).collect();

        // Gone files first, so a file moved in the same moment is found below as the save it was.
        let mut here = Vec::new();
        let mut marked = HashSet::new();
        for path in paths {
            // A file on a drive that's away can take seconds to answer.
            if self.stopping() {
                break;
            }
            let at = index.get(&case::key(&path)).copied();
            match std::fs::metadata(&path) {
                Ok(meta) => here.push((path, at, meta)),
                Err(_) => {
                    self.retry.remove(&path);
                    // It stays listed, marked missing, as in the C# app.
                    if let Some(i) = at
                        && let Some(s) = self.saves.get_mut(i).filter(|s| !s.file_missing)
                    {
                        s.file_missing = true;
                        self.to_mark.insert(s.id);
                        marked.insert(i);
                        done.gone += 1;
                    }
                }
            }
        }
        if self.stopping() {
            done.stopped = true;
            return done;
        }
        // Unchanged saves read again only to mend their muted samples, by path: not on the card.
        let mut mends = HashSet::new();
        here.retain(|(path, at, meta)| {
            let listed = at.and_then(|i| self.saves.get(i));
            let unchanged = listed.is_some_and(|s| {
                !s.file_missing
                    && s.error.is_none()
                    && s.mtime == scan::mtime(meta)
                    && s.size == i64::try_from(meta.len()).unwrap_or(i64::MAX)
            });
            if unchanged && listed.is_some_and(|s| self.mend.has(s.id) || self.reread.contains(&s.id)) {
                mends.insert(path.clone());
                return true;
            }
            // A new "(fixed)" copy is Fix's, waiting for Keep or Delete: left out, as when the list is loaded.
            !unchanged && !(at.is_none() && songs::is_fixed_copy(paths::stem(path)))
        });
        if here.is_empty() {
            return done;
        }

        let samples = self.sample_locator();
        let installed = &self.installed;
        let stop = &*self.stop;
        let read: Vec<(String, Option<usize>, Option<SaveDetail>)> = on_pool(|| {
            here.into_par_iter()
                .map(|(path, at, _)| {
                    // As the app closes, the saves not read yet are left for the next start; those read are kept.
                    let detail =
                        if stop.load(Ordering::Relaxed) { None } else { read_save(&path, installed, &samples) };
                    (path, at, detail)
                })
                .collect()
        });
        done.stopped = self.stopping();
        for (path, at, detail) in read {
            let Some(mut detail) = detail else { continue };
            // Still being written, or locked: tried again in a moment, and a good save read before stays meanwhile.
            if detail.save.error.is_some() {
                let tries = self.retry.entry(path.clone()).or_insert(TRIES);
                *tries = tries.saturating_sub(1);
                if *tries > 0 && at.is_none() {
                    continue;
                }
                if *tries == 0 {
                    self.retry.remove(&path);
                    // Can't be read: it keeps what the library has.
                    if let Some(s) = at.and_then(|i| self.saves.get(i)) {
                        self.mend.tried(s.id, false);
                        self.reread.remove(&s.id);
                    }
                }
                if at.and_then(|i| self.saves.get(i)).is_some_and(|s| s.error.is_none() && !s.file_missing) {
                    continue;
                }
            } else {
                self.retry.remove(&path);
            }
            let moved = at.is_none().then(|| self.moved_from(&detail.save)).flatten();
            match at.or(moved) {
                Some(i) => {
                    // Read again, or the same file moved here: it carries on as that save, with what you gave it.
                    let old = &self.saves[i];
                    keep_your_fields(&mut detail.save, old);
                    detail.save.id = old.id;
                    if moved.is_some() {
                        // Its song is still shown from it, if it was (review SS-1): `song_writes` moves the note in the
                        // library with its row, or with the next batch while the library hasn't got the save.
                        if self.writes.is_some()
                            && !self.kept_as.contains_key(&old.id)
                            && self.choices.is_kept(&old.path)
                        {
                            self.moved_notes.push((old.path.clone(), detail.save.path.clone()));
                        }
                        self.choices.moved(&old.path, &detail.save.path);
                        index.remove(&case::key(&old.path));
                        index.insert(case::key(&detail.save.path), i);
                        // Marked gone a moment ago: it moved, so it's counted once, as moved.
                        if marked.remove(&i) {
                            done.gone -= 1;
                        }
                        done.moved.push(detail.save.name.clone());
                    } else if mends.contains(&path) {
                        done.mended += 1;
                    } else {
                        done.read.push(detail.save.name.clone());
                    }
                    self.mend.tried(old.id, true);
                    if self.reread.remove(&old.id) {
                        self.reread_put.insert(old.id);
                    }
                    self.to_put.insert(old.id);
                    self.to_mark.remove(&old.id);
                    self.saves[i] = detail.save.clone();
                }
                None => {
                    detail.save.id = self.next_id;
                    self.next_id += 1;
                    let taken_off = self.inherit(&mut detail.save);
                    add_hardware_tags(&mut detail.save, &taken_off);
                    index.insert(case::key(&detail.save.path), self.saves.len());
                    self.to_put.insert(detail.save.id);
                    self.saves.push(detail.save.clone());
                    done.read.push(detail.save.name.clone());
                }
            }
            if let Ok(mut d) = self.details.lock() {
                d.insert(detail.save.id, detail);
            }
        }
        done
    }

    /// Walks the watched folders where too much changed to name each file, adding the files to read (new, changed,
    /// back again, or gone) to `paths`. Saves the scan now leaves out (an autosave, or in a folder it skips) go, as
    /// the C# scan drops them.
    fn walk(&mut self, overflowed: &[String], paths: &mut Vec<String>, done: &mut Updated) {
        // The watch folds a project folder inside another into the outer one: walk the inner ones too (one inside a
        // folder the outer walk skips is only found that way), and judge what's left out by the innermost.
        let mut walk: Vec<String> = overflowed.to_vec();
        walk.extend(self.roots.iter().filter(|r| overflowed.iter().any(|o| scan::is_inside(r, o))).cloned());
        // A walk waits on the disk (a cold drive takes seconds a level): on the bulk pool, so the pages' quick jobs on
        // the shared pool (search, filters, the galaxy's pictures) don't queue behind it (F-4, 2026-11-06 review).
        let stop = &*self.stop;
        let found = on_bulk_pool(|| scan::find_projects_until(&walk, &|| stop.load(Ordering::Relaxed)));
        // Stopped part way as the app closes: nothing is taken from a walk that didn't finish, so no save is marked
        // missing or dropped for it ([`Live::stopper`]).
        if self.stopping() {
            return;
        }
        let known: Vec<KnownFile> = self
            .saves
            .iter()
            .filter(|s| overflowed.iter().any(|o| scan::is_inside(&s.path, o)))
            .map(|s| KnownFile {
                id: s.id,
                path: s.path.clone(),
                size: s.size,
                mtime: s.mtime,
                missing: s.file_missing,
                // A missing save whose file is back is read again, even unchanged, as is one to mend.
                reread: s.file_missing || self.mend.has(s.id) || self.reread.contains(&s.id),
            })
            .collect();
        let plan = scan::plan(&found, &known, &self.roots, &|_| false, false);
        // A save to mend in a folder that couldn't be read (a drive that's away, a folder busy or denied) isn't read
        // this time, nor marked missing: it's owed, and mended at a later start (review finding G-1).
        for id in &plan.held {
            self.mend.tried(*id, false);
        }
        paths.extend(plan.read.into_iter().map(|f| f.path));
        let gone: HashSet<i64> = plan.gone.into_iter().collect();
        paths.extend(self.saves.iter().filter(|s| gone.contains(&s.id)).map(|s| s.path.clone()));
        if !plan.dropped.is_empty() {
            let dropped: HashSet<i64> = plan.dropped.into_iter().collect();
            // Off the list only: the library keeps them, with what you gave them (they're left off it at the next
            // start too).
            for id in &dropped {
                self.to_put.remove(id);
                self.to_mark.remove(id);
            }
            self.saves.retain(|s| !dropped.contains(&s.id));
            if let Ok(mut d) = self.details.lock() {
                d.retain(|id, _| !dropped.contains(id));
            }
            done.gone += dropped.len();
        }
    }

    /// The sample locator for this batch: the last one while its folders are fresh, so saving every minute doesn't
    /// read the project's and the DAW's folders every time.
    fn sample_locator(&mut self) -> Arc<SampleLocator> {
        match &self.samples {
            Some((at, locator)) if at.elapsed() < SAMPLES_FRESH => locator.clone(),
            _ => {
                let locator = Arc::new(crate::sound_library::locator(self.sounds_db.as_deref()));
                self.samples = Some((Instant::now(), locator.clone()));
                locator
            }
        }
    }

    /// The save marked missing that this new file is, moved: the one missing save of the same DAW with the same file
    /// name, size and modified time (a move keeps those). None when there's none, or more than one to choose from.
    /// As the C# library's MovedFrom.
    fn moved_from(&self, new: &Save) -> Option<usize> {
        let name = case::key(&new.name);
        let mut found = self.saves.iter().enumerate().filter(|(_, s)| {
            s.file_missing
                && s.daw == new.daw
                && s.size == new.size
                && s.mtime == new.mtime
                && case::key(&s.name) == name
        });
        let first = found.next().map(|(i, _)| i);
        if found.next().is_some() { None } else { first }
    }

    /// A new save of a song (Song_12 next to Song_11) takes over the status, rating, notes and tags you gave the song,
    /// from the save just before it: the highest number below its own, else the last one saved. Tags the scan put on
    /// for that save's hardware aren't carried over. As the C# library's InheritFromEarlierSave, but a save dated still
    /// to come, or with no date, isn't taken for the last one saved (mo_formats::dates): one with a real date is.
    ///
    /// Gives the hardware that save uses whose tag you took off it, so the new save isn't tagged for it either
    /// ([`add_hardware_tags`], review SS-2).
    fn inherit(&self, new: &mut Save) -> Vec<String> {
        let song = self.choices.key_of(&new.path, &new.folder, &new.daw, &new.name);
        let root = case::key(&songs::song_folder(&new.folder, &new.name));
        let number = songs::split(&new.name).1;
        let inside = |folder: &str| {
            let f = case::key(folder);
            f == root || f.strip_prefix(root.as_str()).is_some_and(|rest| rest.starts_with(['\\', '/']))
        };
        let candidates: Vec<&Save> = self
            .saves
            .iter()
            .filter(|s| s.daw == new.daw && s.id != new.id && inside(&s.folder))
            .filter(|s| self.choices.key_of(&s.path, &s.folder, &s.daw, &s.name) == song)
            .collect();
        let real = RealDates::now();
        let saved = |s: &Save| (real.seconds(s.mtime), s.mtime);
        let before = candidates
            .iter()
            .filter(|s| songs::split(&s.name).1 < number)
            .max_by_key(|s| (songs::split(&s.name).1, saved(s)));
        let Some(from) = before.or_else(|| candidates.iter().max_by_key(|s| saved(s))) else { return Vec::new() };
        copy_song_fields(new, from);
        let hardware: HashSet<String> = from.hardware.iter().map(|h| case::to_lower(h)).collect();
        for tag in from.tags.iter().filter(|t| !hardware.contains(t.as_str())) {
            if !new.tags.contains(tag) {
                new.tags.push(tag.clone());
            }
        }
        let tagged: HashSet<String> = from.tags.iter().map(|t| case::to_lower(t)).collect();
        hardware.into_iter().filter(|h| !tagged.contains(h)).collect()
    }
}

/// What a change given to a song did ([`Live::give`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangeMade {
    /// Whether anything held here is different for it: then [`Live::songs`] gives the list anew.
    pub differs: bool,
    /// Whether it's held here only, until the app closes, for some or all of the song's saves (review SG-2): the
    /// list isn't kept in a library of the app's own this time ([`Live::writes`]), or the library has some of the
    /// saves under other ids by now (another copy of the app kept them anew).
    pub held_only: bool,
}

/// What [`Live::keep`] kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub written: Written,
    /// Saves the library wouldn't take, by path, with why: the list shows them this time, as read, and they're tried
    /// again when they're next read.
    pub not_kept: Vec<(String, String)>,
    /// Why the note that the muted samples are mended couldn't be kept, when the library refused it for longer than a
    /// moment (busy is tried again with the next batch): it's tried again in a minute ([`REFUSED_AGAIN_AFTER`]).
    pub not_noted: Option<String>,
}

/// Keeps a batch the library refused as a whole a save at a time, so one it won't take doesn't hold up the rest. Busy
/// stops it: everything is tried again the next time (what's written already is the same written again).
fn one_by_one(songs_db: &std::path::Path, batch: Batch) -> Result<Kept, LibraryError> {
    let mut kept = Kept::default();
    let Batch { put, missing, mended, owed, moved_notes } = batch;
    let alone = put
        .into_iter()
        .map(|p| (p.detail.save.path.clone(), Batch { put: vec![p], ..Batch::default() }))
        .chain(missing.into_iter().map(|m| (m.path.clone(), Batch { missing: vec![m], ..Batch::default() })));
    for (path, one) in alone {
        match song_writes::write(songs_db, &one) {
            Ok(w) => {
                kept.written.put += w.put;
                kept.written.missing += w.missing;
                kept.written.ids.extend(w.ids);
            }
            Err(e) if e.is_busy() => return Err(e),
            Err(e) => kept.not_kept.push((path, e.to_string())),
        }
    }
    // The notes that moved with saves the library hadn't got, for the saves it took, on their own: one that isn't kept
    // stays where it was.
    let moved_notes: Vec<(String, String)> = moved_notes
        .into_iter()
        .filter(|(_, to)| !kept.not_kept.iter().any(|(path, _)| case::eq_ignore_case(path, to)))
        .collect();
    if !moved_notes.is_empty()
        && let Err(e) = song_writes::write(songs_db, &Batch { moved_notes, ..Batch::default() })
    {
        if e.is_busy() {
            return Err(e);
        }
        crate::log::warning("Songs", &format!("Couldn't move in songs.db which copy a song is shown from: {e}"));
    }
    // The note that the muted samples were mended goes last, on its own, and only if the library took every save (a
    // save it refused may be one of them): if it isn't kept, they're mended again at the next start.
    if mended && kept.not_kept.is_empty() {
        match song_writes::write(songs_db, &Batch { mended: true, owed, ..Batch::default() }) {
            Ok(w) => kept.written.mended = w.mended,
            Err(e) if e.is_busy() => return Err(e),
            Err(e) => kept.not_noted = Some(e.to_string()),
        }
    }
    Ok(kept)
}

/// Whether the folder of the file at `path` can be looked at now, to tell whether the file is there: it can be looked
/// in, or it has gone from a drive that's there. Not while its drive is away (not plugged in, a network place that
/// can't be reached) or the folder is busy or denied: a file that isn't found then may be back when they are.
fn can_look_in(path: &str) -> bool {
    let Some(folder) = std::path::Path::new(path).parent() else { return true };
    match std::fs::read_dir(folder) {
        Ok(_) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound && crate::plugins::folder_fixes::drive_there(folder),
    }
}

/// The folder of the file at `path`, as a key ([`case::key`]).
fn folder_key(path: &str) -> String {
    case::key(&std::path::Path::new(path).parent().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
}

/// A save read again keeps what you gave it, and what the library worked out that isn't read here yet (its key
/// from the notes): they belong to the save, not to the file's bytes.
fn keep_your_fields(new: &mut Save, old: &Save) {
    copy_song_fields(new, old);
    new.tags = old.tags.clone();
    add_hardware_tags(new, &old.hardware);
    new.last_opened = old.last_opened;
    new.detected_key = old.detected_key.clone();
    new.note_count = old.note_count;
    new.length_bars = old.length_bars;
    new.created = new.created.or(old.created);
}

/// Tags for the hardware a save uses, put on as it's read, as the C# library does; the app never takes one off. Not for
/// hardware in `before`: what a save read again used before, or what the save before a new one used with its tag
/// taken off ([`Live::inherit`]). So a tag you took off stays off (review SS-2), where the C# library put it back at
/// the next read.
fn add_hardware_tags(save: &mut Save, before: &[String]) {
    let before: HashSet<String> = before.iter().map(|h| case::to_lower(h)).collect();
    for hw in save.hardware.iter().map(|h| case::to_lower(h)) {
        if !before.contains(&hw) && !save.tags.contains(&hw) {
            save.tags.push(hw);
        }
    }
}

/// What you gave a song: its status, rating, favourite, key and notes.
fn copy_song_fields(new: &mut Save, from: &Save) {
    new.status = from.status.clone();
    new.rating = from.rating;
    new.favourite = from.favourite;
    new.musical_key = from.musical_key.clone();
    new.notes = from.notes.clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(saves: Vec<Save>) -> Live {
        Live {
            saves,
            choices: SongChoices::default(),
            tags: Vec::new(),
            gap_months: 6,
            asking: None,
            roots: Vec::new(),
            export: None,
            installed: Installed::default(),
            next_id: 100,
            details: LiveDetails::default(),
            samples: None,
            sounds_db: None,
            retry: HashMap::new(),
            source: None,
            writes: None,
            to_put: BTreeSet::new(),
            to_mark: BTreeSet::new(),
            kept_as: HashMap::new(),
            mend: Mend::NotLooked,
            measures_seen: plugin_writes::measures_kept(),
            recheck_after: None,
            sounds_seen: 0,
            samples_after: None,
            note_after: None,
            away: HashMap::new(),
            said_renumbered: false,
            reread: HashSet::new(),
            reread_put: HashSet::new(),
            moved_notes: Vec::new(),
            stop: Arc::default(),
        }
    }

    #[test]
    fn the_list_leaves_out_fix_copies_autosaves_skipped_folders_and_exports() {
        let dir = std::env::temp_dir().join(format!("mo-live-listed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.db");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT, name TEXT, daw TEXT);
                 INSERT INTO roots VALUES ('C:\\Music');
                 INSERT INTO settings VALUES ('fix_check:1', 'C:\\Music\\Night_2 (fixed).flp'),
                     ('{}', 'D:\\Exports');",
                crate::bitwig_convert::EXPORT_FOLDER_KEY
            ))
            .unwrap();
        let listed = Listed::read(&Library::open_read_only(&path).unwrap()).unwrap();
        assert!(listed.has(r"C:\Music\Night_2.flp"));
        // Fix's copy waiting for Keep (whatever the letters' case), an FL backup copy, a save in FL's Backup folder,
        // and Open in Bitwig's copy in the export folder.
        assert!(!listed.has(r"c:\music\night_2 (FIXED).flp"));
        assert!(!listed.has(r"C:\Music\Night_2 (overwritten at 4h10).flp"));
        assert!(!listed.has(r"C:\Music\Backup\Night_2.flp"));
        assert!(!listed.has(r"D:\Exports\Night (Bitwig)\Night.bwproject"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_kept_in_the_library_is_made_to_the_saves_held_here_too() {
        let mut eleven = save(1, "C:\\Music\\Night", "Night_11");
        eleven.tags = vec!["dark".into()];
        let mut live = live(vec![eleven, save(2, "C:\\Music\\Night", "Night_12"), save(3, "C:\\Music\\Day", "Day_1")]);
        live.tags = vec!["dark".into()];
        assert!(live.gave(&[1, 2], &Change::Rating(4)));
        assert!(!live.gave(&[1, 2], &Change::Rating(4)), "the same again changes nothing");
        assert!(live.gave(&[1, 2], &Change::AddTag("Techno".into())));
        assert_eq!(live.saves.iter().map(|s| s.rating).collect::<Vec<_>>(), [4, 4, 0]);
        assert_eq!(live.saves[1].tags, ["techno"]);
        assert_eq!(live.tags, ["dark", "techno"]);
        // Stars and hearts leave the list of every tag alone; so does a tag taken off (a hidden save may have it).
        assert!(live.gave(&[1, 2], &Change::RemoveTag("dark".into())));
        assert!(live.saves[0].tags == ["techno"] && live.tags == ["dark", "techno"]);
        // The panel's copy of a save changes with it.
        let panel = SaveDetail { save: live.saves[0].clone(), ..SaveDetail::default() };
        live.details.lock().unwrap().insert(1, panel);
        assert!(live.gave(&[1], &Change::Status("Finished".into())));
        assert_eq!(live.details.lock().unwrap()[&1].save.status, "Finished");
        // A save that isn't held here (hidden, or gone from the list) is no trouble.
        assert!(!live.gave(&[77], &Change::Favourite(true)));
        // With no library of the app's own to keep it in, a change is made here only; one that can't be kept, nowhere.
        let day = live.songs().rows.into_iter().find(|r| r.song.latest.id == 3).unwrap().song.key;
        assert_eq!(live.give(&day, &[3], &Change::Rating(2)).unwrap(), ChangeMade { differs: true, held_only: true });
        assert!(live.give(&day, &[3], &Change::Rating(9)).is_err());
        assert_eq!(live.saves[2].rating, 2);
    }

    #[test]
    fn what_you_give_a_song_goes_to_its_saves_as_they_are_now_with_the_tags_it_shows() {
        let mut eleven = save(1, "C:\\Music\\Night", "Night_11");
        eleven.tags = vec!["dark".into(), "vocal".into()];
        let mut live = live(vec![save(2, "C:\\Music\\Night", "Night_12"), eleven, save(3, "C:\\Music\\Day", "Day_1")]);
        let night = live.songs().rows.into_iter().find(|r| r.song.latest.id == 2).unwrap().song.key;
        let card = |live: &Live| live.songs().rows.into_iter().find(|r| r.song.key == night).unwrap();
        // The newest save has no tags, so the card shows the older save's.
        assert_eq!(card(&live).tags, ["dark", "vocal"]);
        // A tag put on keeps them on the card, the new one after them (review SG-1); the same tag again changes nothing.
        assert!(live.give(&night, &[2, 1], &Change::AddTag("Mix".into())).unwrap().differs);
        assert_eq!(card(&live).tags, ["dark", "vocal", "mix"]);
        // Nothing different is nothing held, though there's no library of the app's own here (review SG-2).
        assert_eq!(live.give(&night, &[2, 1], &Change::AddTag("DARK".into())).unwrap(), ChangeMade::default());
        assert!(live.saves.iter().all(|s| s.id == 3 || s.tags == ["dark", "vocal", "mix"]));
        // A save read since the card was shown (Night_13, with the song's status as it was), and one the library
        // numbered anew, get what's given too: the song's saves are those held now (review SG-8).
        live.saves.push(save(4, "C:\\Music\\Night", "Night_13"));
        live.saves[0].id = 22;
        assert!(live.give(&night, &[2, 1], &Change::Status("Finished".into())).unwrap().differs);
        assert_eq!(card(&live).status, "Finished");
        let night_saves: Vec<(i64, &str)> =
            live.saves.iter().filter(|s| s.id != 3).map(|s| (s.id, s.status.as_str())).collect();
        assert_eq!(night_saves, [(22, "Finished"), (1, "Finished"), (4, "Finished")]);
        // Another song is left alone; a song no longer found by its key gets it on the saves the card named.
        assert_eq!(live.saves[2].status, mo_library::STATUSES[0]);
        assert!(live.give("a song that's gone", &[3], &Change::Rating(2)).unwrap().differs);
        assert_eq!(live.saves[2].rating, 2);
    }

    /// Review SS-1: a song whose file the library also found in another folder first keeps showing the copy it shows,
    /// with what only that copy held, after a change given to it, which goes to both.
    #[test]
    fn a_change_given_to_a_song_with_a_copy_keeps_the_copy_it_shows() {
        let counted = |s: Save| Save { note_count: 50, track_count: 4, ..s };
        let found_first = counted(save(1, "D:\\Old\\dub", "Didums_3"));
        let used = Save { rating: 4, notes: "the bass".into(), ..counted(save(9, "E:\\Music\\dub", "Didums_3")) };
        let mut live = live(vec![found_first, used]);
        let shown = |live: &Live| {
            let rows = live.songs().rows;
            assert_eq!(rows.len(), 1, "one song, its file in two folders");
            let song = &rows[0].song;
            (
                song.latest.id,
                song.latest.rating,
                song.latest.notes.clone(),
                song.latest.status.clone(),
                song.copies.len(),
            )
        };
        let key = live.songs().rows[0].song.key.clone();
        assert_eq!(shown(&live), (9, 4, "the bass".to_owned(), mo_library::STATUSES[0].to_owned(), 1));
        // Both copies get the status, so both are organised: the one shown stays.
        assert!(live.give(&key, &[9], &Change::Status("Mixing".into())).unwrap().differs);
        assert!(live.saves.iter().all(|s| s.status == "Mixing"), "rule 14: the copy gets it too");
        assert_eq!(shown(&live), (9, 4, "the bass".to_owned(), "Mixing".to_owned(), 1));
        assert!(live.choices.is_kept("E:\\Music\\dub\\Didums_3.flp"));
        // And so it does when the stars it was shown for go.
        assert!(live.give(&key, &[9], &Change::Rating(0)).unwrap().differs);
        assert_eq!(shown(&live).0, 9);
        // Noted once: a change finds it noted already.
        assert!(live.given_to(&key, &[9], &Change::Rating(2)).keep.is_empty());
        // The song's copy found first, organised, stays shown after a change anyway: nothing is noted for it (rule 23).
        let first = Save { rating: 2, ..counted(save(1, "D:\\Old\\dub", "Didums_3")) };
        let mut live = super::tests::live(vec![first, counted(save(9, "E:\\Music\\dub", "Didums_3"))]);
        assert_eq!(shown(&live).0, 1);
        assert!(live.give(&key, &[1], &Change::Status("Mixing".into())).unwrap().differs);
        assert_eq!(shown(&live).0, 1);
        assert!(!live.choices.is_kept("D:\\Old\\dub\\Didums_3.flp"));
        // Unless the change takes away what kept it shown, and the copy has something of its own (a status the C# app
        // gave it): then it's noted.
        let first = Save { rating: 2, ..counted(save(1, "D:\\Old\\dub", "Didums_3")) };
        let later = Save { status: "Arranging".into(), ..counted(save(9, "E:\\Music\\dub", "Didums_3")) };
        let mut live = super::tests::live(vec![first, later]);
        assert_eq!(shown(&live).0, 1);
        let given = live.given_to(&key, &[1], &Change::Rating(0));
        assert_eq!(
            given.keep,
            [KeepCopy {
                shown: "D:\\Old\\dub\\Didums_3.flp".into(),
                copies: vec!["E:\\Music\\dub\\Didums_3.flp".into()]
            }]
        );
        assert!(live.give(&key, &[1], &Change::Rating(0)).unwrap().differs);
        assert_eq!(shown(&live).0, 1);
    }

    /// Review SS-2: a tag you took off a save for hardware it uses isn't put back when the save is read again; one for
    /// hardware it starts using is put on.
    #[test]
    fn a_hardware_tag_taken_off_stays_off_when_its_save_is_read_again() {
        let old = Save { hardware: vec!["Digitakt".into()], tags: vec!["dark".into()], ..Save::default() };
        let mut again = Save { hardware: vec!["Digitakt".into()], ..Save::default() };
        keep_your_fields(&mut again, &old);
        assert_eq!(again.tags, ["dark"]);
        let mut more = Save { hardware: vec!["Digitakt".into(), "Analog Rytm".into()], ..Save::default() };
        keep_your_fields(&mut more, &old);
        assert_eq!(more.tags, ["dark", "analog rytm"]);
        // A save read for the first time gets one for each.
        let mut new = Save { hardware: vec!["Digitakt".into()], ..Save::default() };
        add_hardware_tags(&mut new, &[]);
        assert_eq!(new.tags, ["digitakt"]);
        // The song's next save, using the same hardware: not for one taken off the save before it, and one for each
        // other.
        let before =
            Save { hardware: vec!["Digitakt".into()], tags: vec!["dark".into()], ..save(1, "C:\\M", "Night_1") };
        let next = || Save { hardware: vec!["Digitakt".into(), "Analog Rytm".into()], ..save(2, "C:\\M", "Night_2") };
        let mut new = next();
        let taken_off = live(vec![before.clone()]).inherit(&mut new);
        add_hardware_tags(&mut new, &taken_off);
        assert_eq!(new.tags, ["dark", "analog rytm"]);
        // Still on the save before it, it goes on the next.
        let before = Save { tags: vec!["dark".into(), "digitakt".into()], ..before };
        let mut new = next();
        let taken_off = live(vec![before]).inherit(&mut new);
        add_hardware_tags(&mut new, &taken_off);
        assert_eq!(new.tags, ["dark", "digitakt", "analog rytm"]);
    }

    /// Review SS-2: a tag put on a song goes on every save with the tags the song shows, but not the newest save's own
    /// hardware tags, which stay on it by themselves.
    #[test]
    fn a_tag_put_on_doesnt_tag_older_saves_with_the_newest_s_hardware() {
        let newest = Save {
            hardware: vec!["Digitakt".into()],
            tags: vec!["dark".into(), "digitakt".into()],
            ..save(2, "C:\\Music\\Night", "Night_2")
        };
        let mut live = live(vec![newest, save(1, "C:\\Music\\Night", "Night_1")]);
        let key = live.songs().rows[0].song.key.clone();
        assert!(live.give(&key, &[2], &Change::AddTag("live".into())).unwrap().differs);
        assert_eq!(live.saves[0].tags, ["dark", "digitakt", "live"]);
        assert_eq!(live.saves[1].tags, ["dark", "live"]);
        assert_eq!(live.songs().rows[0].tags, ["dark", "digitakt", "live"]);
        // A hardware tag the song shows from an older save (the newest has none) goes on the newest too: it would
        // stop showing otherwise.
        let older = || Save { tags: vec!["digitakt".into(), "dark".into()], ..save(1, "C:\\Music\\Day", "Day_1") };
        let newest = || Save { hardware: vec!["Digitakt".into()], ..save(2, "C:\\Music\\Day", "Day_2") };
        let mut put_on = self::live(vec![newest(), older()]);
        let key = put_on.songs().rows[0].song.key.clone();
        assert_eq!(put_on.songs().rows[0].tags, ["digitakt", "dark"]);
        assert!(put_on.give(&key, &[2], &Change::AddTag("live".into())).unwrap().differs);
        assert_eq!(put_on.songs().rows[0].tags, ["digitakt", "dark", "live"]);
        let mut taken_off = self::live(vec![newest(), older()]);
        assert!(taken_off.give(&key, &[2], &Change::RemoveTag("dark".into())).unwrap().differs);
        assert_eq!(taken_off.songs().rows[0].tags, ["digitakt"]);
    }

    /// Review SS-2: taking off the last tag a song shows leaves it with none, rather than an older save's it never
    /// showed; taking one off of several leaves the others.
    #[test]
    fn a_tag_taken_off_leaves_the_tags_the_song_showed() {
        let tagged = |id: i64, name: &str, tags: &[&str]| Save {
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            ..save(id, "C:\\Music\\Didums", name)
        };
        let saves =
            || vec![tagged(3, "Didums_3", &[]), tagged(2, "Didums_2", &["techno"]), tagged(1, "Didums_1", &["dub"])];
        let mut more = saves();
        more[1].tags.push("house".into());
        let (mut one, mut two) = (live(saves()), live(more));
        let card = |live: &Live| live.songs().rows.remove(0).tags;
        let key = one.songs().rows[0].song.key.clone();
        // The newest save has none, so the card shows Didums_2's.
        assert_eq!(card(&one), ["techno"]);
        assert!(one.give(&key, &[3], &Change::RemoveTag("Techno".into())).unwrap().differs);
        assert!(card(&one).is_empty(), "Didums_1's tag doesn't show through");
        assert!(one.saves.iter().all(|s| s.tags.is_empty()));
        // With two shown, the other stays on the card (on every save, so nothing older shows through either).
        assert_eq!(card(&two), ["techno", "house"]);
        assert!(two.give(&key, &[3], &Change::RemoveTag("techno".into())).unwrap().differs);
        assert_eq!(card(&two), ["house"]);
        assert_eq!(two.saves[0].tags, ["house"]);
    }

    /// Review SG-6: a save named to read again for its plugins, on a drive that's away, stays as it was, walked or not,
    /// and is read again once its drive is back; its folder isn't looked in again for a minute. One whose folder is
    /// there and file isn't has gone, and so has one whose folder has gone from a drive that's there.
    #[test]
    fn a_save_to_read_again_on_a_drive_thats_away_isnt_marked_missing() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // A drive that isn't plugged in.
        let away = if cfg!(windows) {
            let free = ('D'..='Z').rev().find(|l| !std::path::Path::new(&format!("{l}:\\")).exists()).unwrap();
            PathBuf::from(format!("{free}:\\Songs"))
        } else {
            PathBuf::from("/mnt/no drive plugged in for the test/Songs")
        };
        let at = |folder: &std::path::Path, name: &str| folder.join(format!("{name}.flp")).display().to_string();
        let mut away_save = save(1, "x", "Night_1");
        away_save.path = at(&away, "Night_1");
        let mut gone_save = save(2, "x", "Day_1");
        gone_save.path = at(here, "Day_1 is no file here");
        let mut folder_gone_save = save(3, "x", "Dusk_1");
        folder_gone_save.path = at(&here.join("no such folder"), "Dusk_1");
        let mut live = live(vec![away_save, gone_save, folder_gone_save]);
        live.roots = vec![away.display().to_string()];
        live.reread.insert(1);
        // Walked (the catch-up at the start): the walk holds it back, and it's left to the walk.
        assert_eq!(live.catch_up().gone, 0);
        assert!(!live.saves[0].file_missing);
        assert!(live.away.is_empty(), "its folder isn't looked in twice");
        // Not walked (a batch from the folder watch): left for later too; the other two have gone.
        live.reread.extend([2, 3]);
        assert_eq!(live.update(&Changes::default()).gone, 2);
        assert!(!live.saves[0].file_missing && live.saves[1].file_missing && live.saves[2].file_missing);
        assert!(live.reread.contains(&1), "read again once its drive is back");
        assert!(live.to_mark.contains(&2) && live.to_mark.contains(&3) && !live.to_mark.contains(&1));
        assert_eq!(
            live.away.keys().collect::<Vec<_>>(),
            [&folder_key(&live.saves[0].path)],
            "not looked in for a minute"
        );
        // A folder found away a moment ago isn't looked in again, so a save there that has gone isn't marked yet;
        // once the minute has passed it's looked in, and the save is marked.
        let mut dawn = save(4, "x", "Dawn_1");
        dawn.path = at(here, "Dawn_1 is no file here");
        let dawn_folder = folder_key(&dawn.path);
        live.saves.push(dawn);
        live.reread.insert(4);
        live.away.insert(dawn_folder.clone(), Instant::now());
        assert_eq!(live.update(&Changes::default()).gone, 0);
        assert!(!live.saves[3].file_missing);
        let a_minute_ago = Instant::now().checked_sub(AWAY_AGAIN_AFTER).expect("the clock has run a minute");
        live.away.insert(dawn_folder, a_minute_ago);
        assert_eq!(live.update(&Changes::default()).gone, 1);
        assert!(live.saves[3].file_missing);
    }

    fn save(id: i64, folder: &str, name: &str) -> Save {
        Save {
            id,
            path: format!("{folder}\\{name}.flp"),
            name: name.into(),
            daw: "FL Studio".into(),
            folder: folder.into(),
            size: 1000,
            mtime: 1_700_000_000 + id,
            ..Save::default()
        }
    }

    #[test]
    fn a_plugin_measure_wakes_the_watch_with_no_file_changed() {
        // Review G-3 (and of #406): the Songs watch only calls update when it has something, so a measure kept while
        // no project file changes is one too; looked at once, then not again.
        let mut l = live(Vec::new());
        assert!(l.retries().is_none());
        l.measures_seen = plugin_writes::measures_kept().wrapping_add(1);
        let woke = l.retries().expect("a measure to look at");
        assert!(woke.files.is_empty());
        l.update(&woke);
        assert!(l.retries().is_none());
    }

    /// Review SG-12: what songs.db refuses (not busy for a moment, which is tried again soon), the note that the
    /// muted samples are mended or a look again at which plugins are installed, isn't asked again at every pass.
    #[test]
    fn what_songs_db_refuses_isnt_asked_again_at_every_pass() {
        let mut l = live(Vec::new());
        l.writes = Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("no such songs.db"));
        // The note alone to keep, refused: tried again in a minute, not at every pass.
        l.mend = Mend::Reading { ids: HashSet::new(), owed: HashSet::new(), read: HashSet::new() };
        assert!(l.note_to_keep());
        let kept = l.keep().unwrap().unwrap();
        assert!(kept.not_noted.is_some() && !kept.written.mended, "{kept:?}");
        assert!(!l.note_to_keep());
        assert!(l.keep().is_none());
        l.note_after = Some(Instant::now());
        assert!(l.note_to_keep(), "and then tried again");
        // A measure kept and the look again failing: the watch isn't woken for it again until a minute has passed.
        l.measures_seen = plugin_writes::measures_kept().wrapping_add(1);
        let woke = l.retries().expect("a measure to look at");
        l.update(&woke);
        assert!(l.retries().is_none(), "it waits a minute");
        l.recheck_after = Some(Instant::now());
        assert!(l.retries().is_some(), "and then tries again");
    }

    #[test]
    fn the_card_says_what_changed() {
        let one = Updated { read: vec!["Night Drive_15".into()], ..Updated::default() };
        assert_eq!(
            one.card(),
            ("Updated Night Drive_15".into(), "Saved in its folder, so the app read it again".into())
        );
        let some = Updated {
            read: vec!["a".into(), "b".into()],
            moved: vec!["c".into()],
            gone: 1,
            mended: 7,
            ..Updated::default()
        };
        assert_eq!(some.card(), ("Updated 4 projects".into(), "2 saved, 1 moved, 1 missing".into()));
        let gone = Updated { gone: 1, ..Updated::default() };
        assert_eq!(gone.card(), ("Updated 1 project".into(), "Missing: moved or deleted".into()));
        let moved = Updated { moved: vec!["Night_11".into()], ..Updated::default() };
        assert_eq!(moved.card(), ("Updated Night_11".into(), "Found in its new folder".into()));
        assert!(Updated::default().is_empty());
        // Saves read again only to mend their muted samples are kept, but not on the card.
        let mended = Updated { mended: 3, ..Updated::default() };
        assert!(!mended.is_empty() && !mended.for_card());
        assert!(some.for_card());
    }

    #[test]
    fn a_save_read_again_keeps_your_fields() {
        let old = Save {
            status: "Mixing".into(),
            rating: 4,
            favourite: true,
            tags: vec!["dark".into()],
            note_count: 512,
            ..Save::default()
        };
        let mut new = Save { tempo: Some(140.0), ..Save::default() };
        keep_your_fields(&mut new, &old);
        assert_eq!((new.status.as_str(), new.rating, new.favourite, new.note_count), ("Mixing", 4, true, 512));
        assert_eq!((new.tags.len(), new.tempo), (1, Some(140.0)));
    }

    #[test]
    fn a_new_save_takes_over_the_song_s_fields() {
        let mut eleven = save(1, "C:\\Music\\Night", "Night_11");
        eleven.status = "Mixing".into();
        eleven.rating = 3;
        eleven.tags = vec!["dark".into(), "digitakt".into()];
        eleven.hardware = vec!["Digitakt".into()];
        let mut ten = save(2, "C:\\Music\\Night", "Night_10");
        ten.rating = 1;
        let other = Save { rating: 5, ..save(3, "C:\\Music\\Night", "Day_40") };
        let live = live(vec![eleven, ten, other]);
        let mut twelve = save(100, "C:\\Music\\Night", "Night_12");
        live.inherit(&mut twelve);
        assert_eq!((twelve.status.as_str(), twelve.rating), ("Mixing", 3));
        assert_eq!(twelve.tags, vec!["dark".to_owned()]);
    }

    #[test]
    fn a_save_dated_still_to_come_isnt_taken_for_the_last_one_saved() {
        // 1 Jan 2100: no clue to when the save was written (mo_formats::dates).
        const TO_COME: i64 = 4_102_444_800;
        let real = Save { rating: 3, ..save(1, "C:\\Music\\Night", "Night_5") };
        let to_come = Save { rating: 5, mtime: TO_COME, ..save(2, "C:\\Music\\Night", "Night 5") };
        let live = live(vec![real, to_come]);
        // Of the two saves numbered just below Night_6, the one with a real date.
        let mut six = save(100, "C:\\Music\\Night", "Night_6");
        live.inherit(&mut six);
        assert_eq!(six.rating, 3);
        // Night_1 has none numbered below it, so it takes over from the last one saved: the newest with a real date.
        let mut one = save(101, "C:\\Music\\Night", "Night_1");
        live.inherit(&mut one);
        assert_eq!(one.rating, 3);
    }

    #[test]
    fn a_moved_file_is_the_missing_save_it_was() {
        let mut old = save(7, "C:\\Music\\Old", "Night_11");
        old.file_missing = true;
        let live = live(vec![old.clone(), save(8, "C:\\Music\\Old", "Night_10")]);
        let moved = Save { path: "D:\\Night_11.flp".into(), folder: "D:".into(), ..old.clone() };
        assert_eq!(live.moved_from(&moved), Some(0));
        assert_eq!(live.moved_from(&Save { size: 2, ..moved.clone() }), None);
        // Two to choose from: neither.
        let twice = self::live(vec![old.clone(), Save { id: 9, ..old }]);
        assert_eq!(twice.moved_from(&moved), None);
    }

    /// The smallest FL Studio project there is: its header and no events.
    fn tiny_flp() -> Vec<u8> {
        let mut b = b"FLhd".to_vec();
        b.extend(6i32.to_le_bytes());
        b.extend(0i16.to_le_bytes());
        b.extend(3u16.to_le_bytes());
        b.extend(96u16.to_le_bytes());
        b.extend(b"FLdt");
        b.extend(0i32.to_le_bytes());
        b
    }

    #[test]
    fn what_you_give_a_song_is_kept_for_all_its_saves_and_stays_when_they_re_read_again() {
        let dir = std::env::temp_dir().join(format!("mo-live-give-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        std::fs::create_dir_all(&projects).unwrap();
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO roots VALUES ('{}');",
                projects.display()
            ))
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        let open = || {
            let lib = Library::open_read_only(&songs).unwrap();
            Live::load(&lib, 6, LiveDetails::default()).unwrap().with_writes(songs.clone())
        };
        std::fs::write(projects.join("Night_1.flp"), tiny_flp()).unwrap();
        let mut live = open();
        live.catch_up();
        live.keep().unwrap().unwrap();
        // A second save, read but not kept in the library yet.
        std::fs::write(projects.join("Night_2.flp"), tiny_flp()).unwrap();
        live.catch_up();
        let ids: Vec<i64> = live.saves.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 2);
        let night = live.songs().rows.remove(0).song.key;
        // Given to both: kept for the one the library has, held for the other, which is kept with it when it's taken.
        let kept_now = ChangeMade { differs: true, held_only: false };
        assert_eq!(live.give(&night, &ids, &Change::Status("Mixing".into())).unwrap(), kept_now);
        assert_eq!(live.give(&night, &ids, &Change::Rating(4)).unwrap(), kept_now);
        assert_eq!(live.give(&night, &ids, &Change::AddTag("Dark".into())).unwrap(), kept_now);
        assert!(!live.give(&night, &ids, &Change::Rating(4)).unwrap().differs, "the same again changes nothing");
        let kept = |songs: &std::path::Path| -> Vec<(String, i32, Vec<String>)> {
            let mut saves = Library::open_read_only(songs).unwrap().saves().unwrap();
            saves.sort_by_key(|s| s.id);
            saves.into_iter().map(|s| (s.status, s.rating, s.tags)).collect()
        };
        let given = ("Mixing".to_owned(), 4, vec!["dark".to_owned()]);
        assert_eq!(kept(&songs), std::slice::from_ref(&given));
        live.keep().unwrap().unwrap();
        assert_eq!(kept(&songs), [given.clone(), given.clone()]);
        // One that can't be kept changes nothing, here or in the library.
        assert!(live.give(&night, &ids, &Change::Rating(9)).is_err());
        assert!(live.saves.iter().all(|s| s.rating == 4));
        // The library busy with another write: said so, and nothing changed here either.
        let holder = rusqlite::Connection::open(&songs).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
        assert!(live.give(&night, &ids, &Change::Favourite(true)).unwrap_err().is_busy());
        holder.execute_batch("ROLLBACK;").unwrap();
        drop(holder);
        assert!(live.saves.iter().all(|s| !s.favourite));
        // A save changed on disk and read again keeps it, and a new save of the song starts with it.
        std::fs::write(projects.join("Night_1.flp"), [tiny_flp(), vec![0]].concat()).unwrap();
        std::fs::write(projects.join("Night_3.flp"), tiny_flp()).unwrap();
        live.catch_up();
        live.keep().unwrap().unwrap();
        assert_eq!(kept(&songs), [given.clone(), given.clone(), given.clone()]);
        // And the next start reads it all back.
        let next = open();
        assert!(next.saves.iter().all(|s| (s.status.as_str(), s.rating) == ("Mixing", 4) && s.tags == ["dark"]));
        drop(next);
        // Another copy of the app kept a save anew, under another id: given here only, and said (review SG-2).
        let other = rusqlite::Connection::open(&songs).unwrap();
        let anew = "PRAGMA foreign_keys = OFF; UPDATE projects SET id = id + 100 WHERE path LIKE '%Night_1.flp';";
        other.execute_batch(anew).unwrap();
        assert_eq!(other.changes(), 1);
        let made = live.give(&night, &ids, &Change::Rating(5)).unwrap();
        assert_eq!(made, ChangeMade { differs: true, held_only: true });
        let rating = |name: &str| -> i64 {
            let path = format!("%{name}.flp");
            other.query_row("SELECT rating FROM projects WHERE path LIKE ?1", [path], |r| r.get(0)).unwrap()
        };
        assert_eq!([rating("Night_1"), rating("Night_2"), rating("Night_3")], [4, 5, 5]);
        drop(other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review SS-1, in songs.db: the copy a song was shown from when it was given a change is noted with the change,
    /// and the next start shows it again.
    #[test]
    fn the_copy_a_song_was_shown_from_is_kept_in_songs_db_with_a_change() {
        let dir = std::env::temp_dir().join(format!("mo-live-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        for folder in ["Old", "Music"] {
            std::fs::create_dir_all(projects.join(folder)).unwrap();
        }
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO roots VALUES ('{}');",
                projects.display()
            ))
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        let open = || {
            let lib = Library::open_read_only(&songs).unwrap();
            Live::load(&lib, 6, LiveDetails::default()).unwrap().with_writes(songs.clone())
        };
        // The same file in two folders: one song, shown from one of them, the other its copy.
        for folder in ["Old", "Music"] {
            std::fs::write(projects.join(folder).join("Night.flp"), tiny_flp()).unwrap();
        }
        let mut live = open();
        live.catch_up();
        live.keep().unwrap().unwrap();
        let shown = |live: &Live| {
            let rows = live.songs().rows;
            assert_eq!(rows.len(), 1, "one song, its file in two folders");
            assert_eq!(rows[0].song.copies.len(), 1);
            (rows[0].song.latest.id, rows[0].song.latest.path.clone())
        };
        // Stars only the copy the library found second has (given before this app, say): it's the one shown. (These
        // tiny saves have no notes to count; a real one's are, and two files are only told to be one by them.)
        let ids: Vec<i64> = live.saves.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 2);
        let second = *ids.iter().max().unwrap();
        drop(live);
        let db = rusqlite::Connection::open(&songs).unwrap();
        db.execute_batch("UPDATE projects SET note_count = 0;").unwrap();
        db.execute("UPDATE projects SET rating = 4 WHERE id = ?1", [second]).unwrap();
        let mut live = open();
        let (id, path) = shown(&live);
        assert_eq!(id, second);
        // A status given to the song goes to both copies, which leaves them as organised as each other; the one shown
        // stays, in songs.db too.
        let key = live.songs().rows[0].song.key.clone();
        assert!(live.give(&key, &ids, &Change::Status("Mixing".into())).unwrap().differs);
        assert_eq!(shown(&live).0, second);
        let kept: Vec<(String, String)> = db
            .prepare("SELECT a, b FROM song_choices WHERE kind = 'kept'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kept, [(case::to_lower(&path), String::new())]);
        assert_eq!(shown(&open()).0, second, "and at the next start");
        // A second change finds it noted already, and adds nothing.
        assert!(live.given_to(&key, &ids, &Change::Rating(0)).keep.is_empty());
        assert!(live.give(&key, &ids, &Change::Rating(0)).unwrap().differs);
        let kept_rows = |db: &rusqlite::Connection| -> Vec<String> {
            let mut st = db.prepare("SELECT a FROM song_choices WHERE kind = 'kept'").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(kept_rows(&db), [case::to_lower(&path)]);
        // The copy shown moves to another folder: the note goes with it, here and in songs.db.
        let moved = projects.join("Moved").join("Night.flp");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::rename(&path, &moved).unwrap();
        let moved = moved.to_string_lossy().into_owned();
        live.catch_up();
        assert_eq!(shown(&live), (second, moved.clone()));
        live.keep().unwrap().unwrap();
        assert_eq!(kept_rows(&db), [case::to_lower(&moved)]);
        assert_eq!(shown(&open()), (second, moved), "and at the next start");
        // Without the note, the copy the library found first would take over.
        db.execute_batch("DELETE FROM song_choices WHERE kind = 'kept';").unwrap();
        assert_ne!(shown(&open()).0, second);
        drop((live, db));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review SS-1: a save noted as the copy its song is shown from that moves before the library has it (refused, or
    /// songs.db busy a moment) takes its note with it when it's kept.
    #[test]
    fn a_noted_save_the_library_hasnt_got_takes_its_note_when_it_moves() {
        let dir = std::env::temp_dir().join(format!("mo-live-kept-later-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        for folder in ["Old", "Music", "Moved"] {
            std::fs::create_dir_all(projects.join(folder)).unwrap();
        }
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO roots VALUES ('{}');",
                projects.display()
            ))
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        let old = projects.join("Old").join("Night.flp").to_string_lossy().into_owned();
        std::fs::write(&old, tiny_flp()).unwrap();
        let lib = Library::open_read_only(&songs).unwrap();
        let mut live = Live::load(&lib, 6, LiveDetails::default()).unwrap().with_writes(songs.clone());
        live.catch_up();
        live.keep().unwrap().unwrap();
        // A copy is read, and noted as the one shown, before the library has it.
        let copy = projects.join("Music").join("Night.flp").to_string_lossy().into_owned();
        std::fs::write(&copy, tiny_flp()).unwrap();
        live.catch_up();
        let id = live.saves.iter().find(|s| s.path == copy).unwrap().id;
        assert!(!live.kept_as.contains_key(&id));
        let db = rusqlite::Connection::open(&songs).unwrap();
        db.execute("INSERT INTO song_choices (kind, a, b) VALUES ('kept', ?1, '')", [case::to_lower(&copy)]).unwrap();
        live.choices.keep(&copy, [old.as_str()]);
        // It moves, and then the library takes it.
        let moved = projects.join("Moved").join("Night.flp").to_string_lossy().into_owned();
        std::fs::rename(&copy, &moved).unwrap();
        assert_eq!(live.catch_up().moved.len(), 1);
        assert!(live.choices.is_kept(&moved));
        live.keep().unwrap().unwrap();
        let kept: Vec<String> = db
            .prepare("SELECT a FROM song_choices WHERE kind = 'kept'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kept, [case::to_lower(&moved)]);
        assert!(live.moved_notes.is_empty(), "kept, so not again");
        drop((live, lib, db));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_s_read_is_kept_in_songs_db_and_not_read_again_at_the_next_start() {
        let dir = std::env::temp_dir().join(format!("mo-live-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        std::fs::create_dir_all(&projects).unwrap();
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO roots VALUES ('{}');",
                projects.display()
            ))
            .unwrap();
        // Before songs.db is the app's own: read, and held here only.
        let old = Library::open_read_only(&dir.join("library.db")).unwrap();
        std::fs::write(projects.join("Night_1.flp"), tiny_flp()).unwrap();
        let mut held = Live::load(&old, 6, LiveDetails::default()).unwrap();
        assert!(!held.writes());
        assert_eq!(held.catch_up().read, vec!["Night_1".to_owned()]);
        assert!(held.keep().is_none());
        drop(old);
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        let open = || {
            let lib = Library::open_read_only(&songs).unwrap();
            Live::load(&lib, 6, LiveDetails::default()).unwrap().with_writes(songs.clone())
        };
        // The app's own: what's read is kept.
        let mut live = open();
        assert_eq!(live.catch_up().read, vec!["Night_1".to_owned()]);
        let kept = live.keep().unwrap().unwrap();
        assert_eq!((kept.written.put, kept.written.missing), (1, 0));
        assert!(live.keep().is_none(), "kept once");
        // The next start lists it, and has nothing to read.
        let mut next = open();
        assert_eq!(next.saves.len(), 1);
        assert!(next.catch_up().is_empty());
        // Named to read again (a plugin marked installed in it isn't found now, review P-2): read unchanged, not on
        // the card, then not again.
        next.reread.insert(next.saves[0].id);
        assert_eq!(next.update(&Changes::default()).mended, 1);
        assert!(next.reread.is_empty());
        assert!(next.update(&Changes::default()).is_empty());
        next.keep();
        // Gone while the app was closed: missing, and kept so.
        std::fs::remove_file(projects.join("Night_1.flp")).unwrap();
        assert_eq!(next.catch_up().gone, 1);
        assert_eq!(next.keep().unwrap().unwrap().written.missing, 1);
        assert!(open().saves[0].file_missing);
        // A save in a folder the scan leaves out (an inner project folder taken off): kept in the library with what you
        // gave it, and left off the list.
        let backup = projects.join("Backup").join("Night_0.flp").display().to_string();
        let save = Save { path: backup, name: "Night_0".into(), rating: 5, ..save(50, "x", "y") };
        let hidden = SaveDetail { save, ..SaveDetail::default() };
        song_writes::write(&songs, &Batch { put: vec![Put { detail: hidden, known_as: None }], ..Batch::default() })
            .unwrap();
        assert_eq!(Library::open_read_only(&songs).unwrap().saves().unwrap().len(), 2);
        assert_eq!(open().saves.len(), 1);
        // The export folder inside the project folder: Open in Bitwig's copies there are never read in as songs, and
        // one the library got before is left off the list.
        let exports = projects.join("Exports");
        std::fs::create_dir_all(exports.join("Night_2")).unwrap();
        std::fs::write(projects.join("Night_2.flp"), tiny_flp()).unwrap();
        std::fs::write(exports.join("Night_2").join("Night_2.flp"), tiny_flp()).unwrap();
        let copy = exports.join("Night_0.flp").display().to_string();
        let exported = Save { path: copy, name: "Night_0".into(), ..self::save(51, "x", "y") };
        let put = Put { detail: SaveDetail { save: exported, ..SaveDetail::default() }, known_as: None };
        song_writes::write(&songs, &Batch { put: vec![put], ..Batch::default() }).unwrap();
        rusqlite::Connection::open(&songs)
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                [crate::bitwig_convert::EXPORT_FOLDER_KEY, &exports.display().to_string()],
            )
            .unwrap();
        let mut with_exports = open();
        assert_eq!(with_exports.saves.len(), 1);
        assert_eq!(with_exports.catch_up().read, vec!["Night_2".to_owned()]);
        let changed = exports.join("Night_2").join("Night_2.flp").display().to_string();
        assert!(with_exports.update(&Changes { files: vec![changed], ..Changes::default() }).is_empty());
        // The export folder set to the project folder itself: the saves there are yours, so none is left out.
        rusqlite::Connection::open(&songs)
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                [crate::bitwig_convert::EXPORT_FOLDER_KEY, &projects.display().to_string()],
            )
            .unwrap();
        assert_eq!(open().saves.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_that_lost_their_muted_samples_are_read_again_once() {
        let dir = std::env::temp_dir().join(format!("mo-live-mend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        std::fs::create_dir_all(&projects).unwrap();
        // Three saves, as they are on disk: two FL Studio ones and a Bitwig one.
        let mut rows = Vec::new();
        for (id, file, daw) in
            [(1, "Night_1.flp", "FL Studio"), (2, "Night_2.flp", "FL Studio"), (3, "Day_1.bwproject", "Bitwig Studio")]
        {
            let path = projects.join(file);
            std::fs::write(&path, tiny_flp()).unwrap();
            let meta = std::fs::metadata(&path).unwrap();
            let name = paths::stem(&path.display().to_string()).to_owned();
            rows.push(format!(
                "({id}, '{}', '{name}', '{daw}', '{}', {}, {})",
                path.display(),
                projects.display(),
                meta.len(),
                scan::mtime(&meta)
            ));
        }
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO roots VALUES ('{}');
                 INSERT INTO projects (id, path, name, daw, folder, size, mtime) VALUES {};",
                projects.display(),
                rows.join(", ")
            ))
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        // The C# app read Night_1 and Day_1 before they last changed, and the app kept them since (losing Night_1's
        // muted samples); Night_2 is as the C# app read it.
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch("UPDATE projects SET mtime = mtime - 1000 WHERE id IN (1, 3)")
            .unwrap();
        let open = || {
            let lib = Library::open_read_only(&songs).unwrap();
            Live::load(&lib, 6, LiveDetails::default()).unwrap().with_writes(songs.clone())
        };
        // Nothing changed, but the FL Studio save the app kept is read again, and so is every Bitwig save once (to
        // find the multisamples it uses from outside its folder, review FS-5); that's not on the card.
        let mut live = open();
        assert_eq!(live.saves.len(), 3);
        let caught = live.catch_up();
        assert_eq!((caught.read.len(), caught.moved.len(), caught.gone, caught.mended), (0, 0, 0, 2));
        assert!(!caught.for_card());
        assert!(live.note_to_keep());
        let kept = live.keep().unwrap().unwrap();
        assert_eq!((kept.written.put, kept.written.mended), (2, true));
        assert_eq!(live.mend, Mend::Done);
        assert!(live.keep().is_none(), "kept once");
        // Noted: the next start reads nothing again.
        let mut next = open();
        assert!(next.catch_up().is_empty());
        assert!(next.keep().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review SS-3: as the app closes, the catch-up stops part way, so what you gave songs on their cards doesn't wait
    /// behind it: nothing more is read, no save is taken for gone, and the saves still to mend stay to mend. The next
    /// start's catch-up does it all.
    #[test]
    fn a_catch_up_stopped_as_the_app_closes_leaves_the_rest_for_the_next_start() {
        let dir = std::env::temp_dir().join(format!("mo-live-stopped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        std::fs::create_dir_all(&projects).unwrap();
        // A save that changed since it was read, one whose file has gone, and a new one.
        let night = projects.join("Night_1.flp");
        std::fs::write(&night, tiny_flp()).unwrap();
        std::fs::write(projects.join("Day_1.flp"), tiny_flp()).unwrap();
        let meta = std::fs::metadata(&night).unwrap();
        let (size, mtime) = (meta.len(), scan::mtime(&meta));
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO roots VALUES ('{projects}');
                 INSERT INTO projects (id, path, name, daw, folder, size, mtime) VALUES
                     (1, '{night}', 'Night_1', 'FL Studio', '{projects}', {size}, {old}),
                     (2, '{gone}', 'Gone_1', 'FL Studio', '{projects}', {size}, {mtime});",
                projects = projects.display(),
                night = night.display(),
                gone = projects.join("Gone_1.flp").display(),
                old = mtime - 1000,
            ))
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        // The C# app's library has them as they were before: so they're read again for their muted samples too.
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch("UPDATE projects SET mtime = mtime - 1")
            .unwrap();
        let open = || {
            let lib = Library::open_read_only(&songs).unwrap();
            Live::load(&lib, 6, LiveDetails::default()).unwrap().with_writes(songs.clone())
        };
        let mut live = open();
        live.look_for_mends();
        assert_eq!(live.mend, Mend::Reading { ids: [1, 2].into(), owed: HashSet::new(), read: HashSet::new() });
        live.stopper().store(true, Ordering::SeqCst);
        let caught = live.catch_up();
        assert_eq!((caught.read.len(), caught.moved.len(), caught.gone, caught.mended), (0, 0, 0, 0));
        assert!(caught.stopped);
        assert_eq!(live.mend, Mend::Reading { ids: [1, 2].into(), owed: HashSet::new(), read: HashSet::new() });
        assert!(!live.note_to_keep());
        assert!(live.keep().is_none(), "nothing to keep");
        // A batch from the folder watch reads nothing either.
        let day = projects.join("Day_1.flp").display().to_string();
        let updated = live.update(&Changes { files: vec![day], ..Changes::default() });
        assert!(updated.stopped && updated.read.is_empty());
        // A mend that's done isn't thrown away by a stop: it's still noted, with the saves read for it.
        live.mend = Mend::Reading { ids: HashSet::new(), owed: HashSet::new(), read: HashSet::new() };
        assert!(live.update(&Changes::default()).stopped);
        assert!(live.note_to_keep());
        // The next start catches up with all of it.
        let mut live = open();
        let caught = live.catch_up();
        assert!(!caught.stopped);
        assert_eq!((caught.read, caught.gone), (vec!["Day_1".to_owned(), "Night_1".to_owned()], 1));
        assert!(live.note_to_keep());
        let kept = live.keep().unwrap().unwrap();
        assert_eq!((kept.written.put, kept.written.missing, kept.written.mended), (2, 1, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A save to mend on a drive that's away isn't read: it's noted as owed, and only it is read again at a later
    /// start, once the drive is back (review finding G-1). The one read stays read: a drive away for good doesn't
    /// have every save read again at every start.
    #[test]
    fn a_save_to_mend_on_a_drive_thats_away_waits_for_it() {
        let dir = std::env::temp_dir().join(format!("mo-live-mend-away-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        let away = dir.join("Away");
        std::fs::create_dir_all(&projects).unwrap();
        // Two FL Studio saves the app kept since the C# app read them (they changed since): one here, one on a
        // drive that's away now.
        let here = projects.join("Night_1.flp");
        std::fs::write(&here, tiny_flp()).unwrap();
        let meta = std::fs::metadata(&here).unwrap();
        let (size, mtime) = (meta.len(), scan::mtime(&meta));
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE roots (path TEXT PRIMARY KEY);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO roots VALUES ('{projects}'), ('{away}');
                 INSERT INTO projects (id, path, name, daw, folder, size, mtime) VALUES
                     (1, '{here}', 'Night_1', 'FL Studio', '{projects}', {size}, {mtime}),
                     (2, '{gone}', 'Away_1', 'FL Studio', '{away}', {size}, {mtime});",
                projects = projects.display(),
                away = away.display(),
                here = here.display(),
                gone = away.join("Away_1.flp").display(),
            ))
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch("UPDATE projects SET mtime = mtime - 1000")
            .unwrap();
        let open = || {
            let lib = Library::open_read_only(&songs).unwrap();
            Live::load(&lib, 6, LiveDetails::default()).unwrap().with_writes(songs.clone())
        };
        let mut ids = mo_library::song_writes::to_mend(&songs).unwrap().unwrap();
        ids.sort_unstable();
        assert_eq!(ids, [1, 2]);
        // The one here is read again; the one away is neither read nor marked missing, and it's owed.
        let mut live = open();
        let caught = live.catch_up();
        assert_eq!((caught.read.len(), caught.gone, caught.mended), (0, 0, 1));
        assert!(live.note_to_keep());
        let kept = live.keep().unwrap().unwrap();
        assert_eq!((kept.written.put, kept.written.mended), (1, true));
        assert_eq!(mo_library::song_writes::to_mend(&songs).unwrap(), Some(vec![2]));
        // Still away at the next start: still owed, and nothing else read again.
        let mut live = open();
        assert_eq!(live.catch_up().mended, 0);
        assert!(live.note_to_keep());
        assert!(live.keep().unwrap().unwrap().written.mended);
        assert_eq!(mo_library::song_writes::to_mend(&songs).unwrap(), Some(vec![2]));
        // The drive is back, the save as it was: it's read again, and nothing is owed.
        std::fs::create_dir_all(&away).unwrap();
        std::fs::write(away.join("Away_1.flp"), tiny_flp()).unwrap();
        let back = std::fs::File::options().write(true).open(away.join("Away_1.flp")).unwrap();
        back.set_modified(meta.modified().unwrap()).unwrap();
        drop(back);
        let mut live = open();
        let caught = live.catch_up();
        assert_eq!(caught.mended, 1);
        let kept = live.keep().unwrap().unwrap();
        assert_eq!((kept.written.put, kept.written.mended), (1, true));
        assert_eq!(mo_library::song_writes::to_mend(&songs).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_the_library_changed_go_over_all_at_once() {
        let mut live = live(vec![save(101, "C:\\a", "A_1"), save(102, "C:\\a", "B_1")]);
        for id in [101, 102] {
            let save = live.saves.iter().find(|s| s.id == id).unwrap().clone();
            live.details.lock().unwrap().insert(id, SaveDetail { save, ..SaveDetail::default() });
        }
        // 101 was taken meanwhile: A is kept as 102, so B as 103.
        live.renumber(&[(101, 102), (102, 103)]);
        let ids: Vec<(String, i64)> = live.saves.iter().map(|s| (s.name.clone(), s.id)).collect();
        assert_eq!(ids, vec![("A_1".to_owned(), 102), ("B_1".to_owned(), 103)]);
        let details = live.details.lock().unwrap();
        assert_eq!(details[&102].save.name, "A_1");
        assert_eq!(details[&103].save.name, "B_1");
        assert_eq!(live.next_id, 104);
    }

    #[test]
    fn a_save_the_library_refuses_doesnt_hold_up_the_rest() {
        let dir = std::env::temp_dir().join(format!("mo-live-refused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);",
            )
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        // Something the library won't take, whatever's tried.
        rusqlite::Connection::open(&songs)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER refuse BEFORE INSERT ON projects WHEN NEW.name = 'Bad_1'
                 BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )
            .unwrap();
        let mut live = live(vec![save(1, "C:\\a", "Good_1"), save(2, "C:\\a", "Bad_1")]);
        live.writes = Some(songs.clone());
        for s in live.saves.clone() {
            live.to_put.insert(s.id);
            live.details.lock().unwrap().insert(s.id, SaveDetail { save: s, ..SaveDetail::default() });
        }
        let kept = live.keep().unwrap().unwrap();
        assert_eq!(kept.written.put, 1);
        assert_eq!(kept.not_kept.len(), 1);
        assert!(kept.not_kept[0].0.ends_with("Bad_1.flp") && kept.not_kept[0].1.contains("refused"), "{kept:?}");
        // The good one is in the library (its details read from there now); the other is held here, and not tried
        // again until it's read again.
        let lib = Library::open_read_only(&songs).unwrap();
        assert_eq!(lib.saves().unwrap().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Good_1"]);
        assert!(!live.details.lock().unwrap().contains_key(&1) && live.details.lock().unwrap().contains_key(&2));
        assert!(live.keep().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_that_moves_again_after_the_library_refused_its_move_stays_one_song() {
        let dir = std::env::temp_dir().join(format!("mo-live-moved-twice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);",
            )
            .unwrap();
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
        assert!(mo_library::songs_db::at_start(&dir, "test").is_songs_db());
        let mut live = live(vec![save(1, "C:\\a", "Dusk_1")]);
        live.writes = Some(songs.clone());
        // Read, kept, then moved twice: the library won't take the first move.
        let moved = |live: &mut Live, folder: &str| {
            let s = &mut live.saves[0];
            s.folder = folder.into();
            s.path = format!("{folder}\\Dusk_1.flp");
            live.to_put.insert(s.id);
            live.details.lock().unwrap().insert(s.id, SaveDetail { save: s.clone(), ..SaveDetail::default() });
            live.keep().unwrap().unwrap()
        };
        assert_eq!(moved(&mut live, "C:\\a").written.put, 1);
        rusqlite::Connection::open(&songs)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER refuse BEFORE UPDATE ON projects WHEN NEW.folder = 'C:\\b'
                 BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )
            .unwrap();
        assert_eq!(moved(&mut live, "C:\\b").not_kept.len(), 1);
        // Still where it was in the library, which goes on being asked for it there: the same row moves, not a new one.
        let kept = moved(&mut live, "C:\\c");
        assert_eq!((kept.written.put, kept.not_kept.len()), (1, 0), "{kept:?}");
        let rows: Vec<(i64, String)> =
            Library::open_read_only(&songs).unwrap().saves().unwrap().into_iter().map(|s| (s.id, s.path)).collect();
        assert_eq!(rows, [(1, "C:\\c\\Dusk_1.flp".to_owned())]);
        // Gone from there: marked missing in the library, at the path it has there.
        live.saves[0].file_missing = true;
        live.to_mark.insert(1);
        assert_eq!(live.keep().unwrap().unwrap().written.missing, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
