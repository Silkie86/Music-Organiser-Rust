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
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mo_formats::{PluginRef, case, paths};
use mo_library::plugin_writes;
use mo_library::read::{Installed, read_save};
use mo_library::samples::SampleLocator;
use mo_library::scan::{self, KnownFile};
use mo_library::song_fields::{self, Change, SaveRow};
use mo_library::song_writes::{self, Batch, Missing, Put, Written};
use mo_library::songs;
use mo_library::watch::Changes;
use mo_library::{Library, LibraryError, Save, SaveDetail, SongChoices};
use rayon::prelude::*;

use crate::background::{on_bulk_pool, on_pool};
use crate::shell;
use crate::songs::{ASK_DOUBTS, DOUBTS_ACROSS_FOLDERS, RUN_GAP, Songs, run_gap};

/// The details of the saves read here, by their ids, shared with the song panel's reader.
pub type LiveDetails = Arc<Mutex<HashMap<i64, SaveDetail>>>;

/// The DAWs the scanner reads, so a song from one the library has no save of yet still opens in yours.
const DAWS: [&str; 3] = ["Bitwig Studio", "FL Studio", "Studio One"];

/// How long the folders a sample locator has read are trusted: saves a minute apart share them, later ones look again.
const SAMPLES_FRESH: Duration = Duration::from_secs(60);

/// How many times a save that couldn't be read (still being written, or locked) is tried again.
const TRIES: u8 = 3;

/// Everything the Songs list is made from, kept so it can be made again when a save changes.
pub struct Live {
    saves: Vec<Save>,
    choices: SongChoices,
    tags: Vec<String>,
    gap_months: u32,
    asking: Option<bool>,
    daw_exes: HashMap<String, String>,
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
    /// Unchanged saves to read again because a plugin marked installed in them isn't found now
    /// ([`plugin_writes::Rechecked::to_read`], review P-2): their own read says whether it's missing.
    reread: HashSet<i64>,
    /// Those read again, to take off the library's note once they're kept ([`plugin_writes::read_again_kept`]).
    reread_put: HashSet<i64>,
}

/// Where reading again the saves whose "muted" sample marks were lost is (review finding A-1): looked for once a start,
/// at the catch-up, read with it, and noted in the library once they're all read again and kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Mend {
    /// Not looked for yet this time.
    #[default]
    NotLooked,
    /// The saves still to be read again (by id, unchanged or not); those that couldn't be read this time (in a folder
    /// that couldn't be read, a drive that's away): noted as owed, and only they are read again at the next start
    /// (review finding G-1); and whether one couldn't be kept: then nothing is noted, and they're all read again at
    /// the next start.
    Reading { ids: HashSet<i64>, owed: HashSet<i64>, missed: bool },
    /// Nothing (more) to do this time.
    Done,
}

impl Mend {
    fn has(&self, id: i64) -> bool {
        matches!(self, Mend::Reading { ids, .. } if ids.contains(&id))
    }

    /// `id` was read again (`read`), or couldn't be.
    fn tried(&mut self, id: i64, read: bool) {
        if let Mend::Reading { ids, owed, .. } = self
            && ids.remove(&id)
            && !read
        {
            owed.insert(id);
        }
    }

    /// Every save has been read again and kept, or couldn't be read: to be noted with the next batch, with those owed
    /// ([`Mend::owed`]).
    fn to_note(&self) -> bool {
        matches!(self, Mend::Reading { ids, missed: false, .. } if ids.is_empty())
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
}

impl Updated {
    /// This and `more` together.
    pub fn and(mut self, more: Updated) -> Updated {
        self.read.extend(more.read);
        self.moved.extend(more.moved);
        self.gone += more.gone;
        self.mended += more.mended;
        self.rechecked += more.rechecked;
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

impl Live {
    /// Reads what the list is made from. Fix's copies still waiting for Keep or Delete are left out: until Keep, a
    /// song opens its original. Runs of saves split at the gap picked in Settings (`version_gap_months`, shared with
    /// the C# app), or at `gap_months` when none is picked.
    pub fn load(library: &Library, gap_months: u32, details: LiveDetails) -> Result<Live, LibraryError> {
        let pending = library.pending_fixes()?;
        let (mut saves, source) = library.saves_and_source()?;
        // After every row of the library, Fix's copies left out below included, so no two saves share an id.
        let next_id = saves.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        if !pending.is_empty() {
            saves.retain(|s| !pending.contains(&s.path));
        }
        // A DAW's autosave copies the C# app's scan let in ("sly_17 (autosaved at 4h10).flp") stay hidden, as do saves
        // in a folder the scan leaves out (a project folder inside one was taken off): the library keeps them.
        let roots = library.roots()?;
        let export = crate::bitwig_convert::export_left_out(library)?.map(|e| e.display().to_string());
        saves.retain(|s| !scan::is_ignored(&s.path, &roots) && !in_export(&s.path, export.as_deref()));
        // As the C# app: asked unless Settings turned it off; across folders only when turned on.
        let asking = (library.setting(ASK_DOUBTS)?.as_deref() != Some("0"))
            .then(|| library.setting(DOUBTS_ACROSS_FOLDERS).map(|v| v.as_deref() == Some("1")))
            .transpose()?;
        let gap_months = run_gap(library.setting(RUN_GAP)?.as_deref(), gap_months);
        let mut daw_exes = HashMap::new();
        let daws: BTreeSet<String> = saves.iter().map(|s| s.daw.clone()).chain(DAWS.map(String::from)).collect();
        for daw in daws {
            if let Some(exe) = library.setting(&shell::daw_exe_key(&daw))?.filter(|e| !e.trim().is_empty()) {
                daw_exes.insert(daw, exe);
            }
        }
        let kept_as = saves.iter().map(|s| (s.id, s.path.clone())).collect();
        Ok(Live {
            saves,
            choices: library.song_choices()?,
            tags: library.all_tags()?.into_iter().map(|(t, _)| t).collect(),
            gap_months,
            asking,
            daw_exes,
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
            reread: HashSet::new(),
            reread_put: HashSet::new(),
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

    /// Gives every save in `save_ids` (all the saves of one song, rule 14) `change`: kept in the library
    /// ([`mo_library::song_fields::set`]: all of them or none) and then made to the saves held here, so the list shows
    /// what's kept. Call it on the thread that owns this list, as [`Live::update`] and [`Live::keep`] are: then no
    /// save can be read, or kept, between the two. It waits for another write to the library, up to five seconds.
    ///
    /// A save the library hasn't taken yet (just read, or refused) is changed here only, and is kept with the change
    /// when the library takes it. While the list isn't kept in a library of the app's own ([`Live::writes`]) the change
    /// is made here only, and lasts until the app closes.
    ///
    /// If the library can't keep it nothing is changed, here or there. Whether anything held here is different for
    /// it: then [`Live::songs`] gives the list anew.
    pub fn give(&mut self, save_ids: &[i64], change: &Change) -> Result<bool, LibraryError> {
        if let Some(songs_db) = &self.writes {
            // By id and the path the library keeps each under: an id alone may be another save's there by now.
            let rows: Vec<SaveRow> = save_ids
                .iter()
                .filter_map(|id| self.kept_as.get(id).map(|path| SaveRow { id: *id, path: path.clone() }))
                .collect();
            song_fields::set(songs_db, &rows, change)?;
        } else if let Err(why) = change.checked() {
            return Err(LibraryError::Write { path: PathBuf::new(), why });
        }
        Ok(self.gave(save_ids, change))
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
        let mut songs = Songs::from_saves(&self.saves, &self.choices, self.tags.clone(), self.gap_months, self.asking);
        songs.daw_exes = self.daw_exes.clone();
        songs
    }

    /// The saves whose read failed last time, to read again a moment later; None when there are none.
    /// Also Some (with nothing in it) when the Plugins page measured your plugins since the last look, so
    /// [`Live::update`] looks again at which are installed while no project file changes (review G-3).
    pub fn retries(&mut self) -> Option<Changes> {
        let files: Vec<String> = self.retry.keys().cloned().collect();
        (!files.is_empty() || self.measures_seen != plugin_writes::measures_kept())
            .then_some(Changes { files, ..Changes::default() })
    }

    /// Walks every project folder once, as the C# app's scan did at its start: saves made or changed while the app was
    /// closed are read, and ones whose file has gone are marked missing. Only what differs from the library (by size
    /// and date) is read. Asks the disk, so never on the window's thread.
    ///
    /// The first time, saves whose "muted" sample marks the app lost are read again too, unchanged or not
    /// ([`song_writes::to_mend`]); once they're kept, the library notes it ([`Live::keep`]) and they aren't read again.
    pub fn catch_up(&mut self) -> Updated {
        self.look_for_mends();
        let rechecked = self.recheck_installed();
        let roots = self.roots.clone();
        if roots.is_empty() {
            self.mend = Mend::Done;
            return Updated { rechecked, ..Updated::default() };
        }
        let mut done = self.update(&Changes { overflowed: roots, ..Changes::default() });
        done.rechecked += rechecked;
        // One the walk didn't find (gone, so marked missing, or outside the project folders now) has nothing to read
        // again: only those waiting to be tried again are still to come. One in a folder that couldn't be read is
        // owed already ([`Live::walk`]).
        if let Mend::Reading { ids, .. } = &mut self.mend {
            let waiting: HashSet<String> = self.retry.keys().map(|p| case::key(p)).collect();
            let waiting: HashSet<i64> =
                self.saves.iter().filter(|s| waiting.contains(&case::key(&s.path))).map(|s| s.id).collect();
            ids.retain(|id| waiting.contains(id));
        }
        done
    }

    /// Looks again at whether every save's plugins are installed, when the installed plugins or the rules changed
    /// since the last look ([`plugin_writes::recheck_installed`]), and shows the new counts; says how many saves
    /// changed. Saves read from now on use the installed plugins as they are now. One that can't be looked at is
    /// looked at again at the next start.
    fn recheck_installed(&mut self) -> usize {
        // Taken before the look, so a measure kept meanwhile is looked at next time; only kept once the look worked,
        // so one that failed is tried again at the next batch.
        let measures = plugin_writes::measures_kept();
        let Some(songs_db) = &self.writes else {
            self.measures_seen = measures;
            return 0;
        };
        let r = match plugin_writes::recheck_installed(songs_db) {
            Ok(r) => {
                self.measures_seen = measures;
                r
            }
            Err(e) => {
                crate::log::warning("Songs", &format!("{e}; which plugins are installed is looked at again later"));
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
                let listed: HashSet<i64> = self.saves.iter().map(|s| s.id).collect();
                let ids = ids.into_iter().filter(|id| listed.contains(id)).collect();
                self.mend = Mend::Reading { ids, owed: HashSet::new(), missed: false };
            }
            Ok(None) => {}
            Err(e) => crate::log::warning("Songs", &format!("{e}; the muted samples are mended at the next start")),
        }
    }

    /// Whether the note that the muted samples were mended is still to be kept, though nothing else may be
    /// ([`Live::keep`]).
    pub fn note_to_keep(&self) -> bool {
        self.mend.to_note()
    }

    /// Keeps what's been read since the last time in the app's own `songs.db` ([`Live::with_writes`]): `None` when
    /// there's nothing to keep, or nowhere to keep it. If the library is busy (another copy of the app holding it), it's
    /// all kept the next time, and meanwhile the list shows it as before. A save the library refuses is kept on its
    /// own, so it can't hold up the rest; one that can't be kept at all is held here only this time
    /// ([`Kept::not_kept`]). Writes to the disk, so never on the window's thread.
    pub fn keep(&mut self) -> Option<Result<Kept, LibraryError>> {
        let songs_db = self.writes.clone()?;
        let mut batch = self.batch();
        batch.mended = self.mend.to_note();
        batch.owed = if batch.mended { self.mend.owed() } else { Vec::new() };
        let put: Vec<(i64, String)> =
            batch.put.iter().map(|p| (p.detail.save.id, p.detail.save.path.clone())).collect();
        if batch.is_empty() {
            self.to_put.clear();
            self.to_mark.clear();
            return None;
        }
        let kept = match song_writes::write(&songs_db, &batch) {
            Ok(written) => Kept { written, not_kept: Vec::new() },
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
        if kept.written.mended {
            self.mend = Mend::Done;
        } else if let Mend::Reading { missed, .. } = &mut self.mend
            && !kept.not_kept.is_empty()
        {
            // One the library wouldn't take may be one of them.
            *missed = true;
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
        let mut done = Updated::default();
        // The Plugins page measured your plugins since: saves are judged against what it found from now on (review G-3).
        if self.measures_seen != plugin_writes::measures_kept() {
            done.rechecked += self.recheck_installed();
        }
        let mut paths: Vec<String> = changes.files.clone();
        // Saves naming a plugin that isn't found now are read again, changed or not (review P-2).
        if !self.reread.is_empty() {
            paths.extend(
                self.saves.iter().filter(|s| !s.file_missing && self.reread.contains(&s.id)).map(|s| s.path.clone()),
            );
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
        let read: Vec<(String, Option<usize>, Option<SaveDetail>)> = on_pool(|| {
            here.into_par_iter()
                .map(|(path, at, _)| {
                    let detail = read_save(&path, installed, &samples);
                    (path, at, detail)
                })
                .collect()
        });
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
                    self.inherit(&mut detail.save);
                    add_hardware_tags(&mut detail.save);
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
        let found = on_bulk_pool(|| scan::find_projects(&walk));
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
    /// for that save's hardware aren't carried over. As the C# library's InheritFromEarlierSave.
    fn inherit(&self, new: &mut Save) {
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
        let before = candidates
            .iter()
            .filter(|s| songs::split(&s.name).1 < number)
            .max_by_key(|s| (songs::split(&s.name).1, s.mtime));
        let Some(from) = before.or_else(|| candidates.iter().max_by_key(|s| s.mtime)) else { return };
        copy_song_fields(new, from);
        let hardware: HashSet<String> = from.hardware.iter().map(|h| case::to_lower(h)).collect();
        for tag in from.tags.iter().filter(|t| !hardware.contains(t.as_str())) {
            if !new.tags.contains(tag) {
                new.tags.push(tag.clone());
            }
        }
    }
}

/// What [`Live::keep`] kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub written: Written,
    /// Saves the library wouldn't take, by path, with why: the list shows them this time, as read, and they're tried
    /// again when they're next read.
    pub not_kept: Vec<(String, String)>,
}

/// Keeps a batch the library refused as a whole a save at a time, so one it won't take doesn't hold up the rest. Busy
/// stops it: everything is tried again the next time (what's written already is the same written again).
fn one_by_one(songs_db: &std::path::Path, batch: Batch) -> Result<Kept, LibraryError> {
    let mut kept = Kept::default();
    let Batch { put, missing, mended, owed } = batch;
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
    // The note that the muted samples were mended goes last, on its own, and only if the library took every save (a
    // save it refused may be one of them): if it isn't kept, they're mended again at the next start.
    if mended && kept.not_kept.is_empty() {
        match song_writes::write(songs_db, &Batch { mended: true, owed, ..Batch::default() }) {
            Ok(w) => kept.written.mended = w.mended,
            Err(e) if e.is_busy() => return Err(e),
            Err(_) => {}
        }
    }
    Ok(kept)
}

/// A save read again keeps what you gave it, and what the library worked out that isn't read here yet (its key
/// from the notes): they belong to the save, not to the file's bytes.
fn keep_your_fields(new: &mut Save, old: &Save) {
    copy_song_fields(new, old);
    new.tags = old.tags.clone();
    add_hardware_tags(new);
    new.last_opened = old.last_opened;
    new.detected_key = old.detected_key.clone();
    new.note_count = old.note_count;
    new.length_bars = old.length_bars;
    new.created = new.created.or(old.created);
}

/// Tags for the hardware a save uses, put on as it's read, as the C# library does (and, as there, never taken off).
fn add_hardware_tags(save: &mut Save) {
    for hw in save.hardware.iter().map(|h| case::to_lower(h)) {
        if !save.tags.contains(&hw) {
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
            daw_exes: HashMap::new(),
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
            reread: HashSet::new(),
            reread_put: HashSet::new(),
        }
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
        assert!(live.give(&[3], &Change::Rating(2)).unwrap());
        assert!(live.give(&[3], &Change::Rating(9)).is_err());
        assert_eq!(live.saves[2].rating, 2);
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

    #[test]
    fn the_card_says_what_changed() {
        let one = Updated { read: vec!["Night Drive_15".into()], ..Updated::default() };
        assert_eq!(
            one.card(),
            ("Updated Night Drive_15".into(), "Saved in its folder, so the app read it again".into())
        );
        let some =
            Updated { read: vec!["a".into(), "b".into()], moved: vec!["c".into()], gone: 1, mended: 7, rechecked: 0 };
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
        // Given to both: kept for the one the library has, held for the other, which is kept with it when it's taken.
        assert!(live.give(&ids, &Change::Status("Mixing".into())).unwrap());
        assert!(live.give(&ids, &Change::Rating(4)).unwrap());
        assert!(live.give(&ids, &Change::AddTag("Dark".into())).unwrap());
        assert!(!live.give(&ids, &Change::Rating(4)).unwrap(), "the same again changes nothing");
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
        assert!(live.give(&ids, &Change::Rating(9)).is_err());
        assert!(live.saves.iter().all(|s| s.rating == 4));
        // The library busy with another write: said so, and nothing changed here either.
        let holder = rusqlite::Connection::open(&songs).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
        assert!(live.give(&ids, &Change::Favourite(true)).unwrap_err().is_busy());
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
        // Nothing changed, but the FL Studio save the app kept is read again, and that's not on the card.
        let mut live = open();
        assert_eq!(live.saves.len(), 3);
        let caught = live.catch_up();
        assert_eq!((caught.read.len(), caught.moved.len(), caught.gone, caught.mended), (0, 0, 0, 1));
        assert!(!caught.for_card());
        assert!(live.note_to_keep());
        let kept = live.keep().unwrap().unwrap();
        assert_eq!((kept.written.put, kept.written.mended), (1, true));
        assert_eq!(live.mend, Mend::Done);
        assert!(live.keep().is_none(), "kept once");
        // Noted: the next start reads nothing again.
        let mut next = open();
        assert!(next.catch_up().is_empty());
        assert!(next.keep().is_none());
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
