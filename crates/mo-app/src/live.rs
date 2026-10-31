//! Keeping the Songs list up to date while the app is open, as the C# app's folder watch does: a project saved, added,
//! moved or deleted in your project folders is read (or marked missing) a moment later, without walking every folder
//! again. When the folder watch can't name every file, the folders it names are walked with the scanner instead.
//!
//! Until the library is trusted the app doesn't write it (CLAUDE.md), so what's read here is held in memory: the list
//! shows it, and the C# app's next scan puts the same saves in the library. A save read again keeps its id; a new one
//! gets an id after the library's. The song panel takes the details of either from here rather than from the library.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mo_formats::{case, paths};
use mo_library::read::{Installed, read_save};
use mo_library::samples::SampleLocator;
use mo_library::scan::{self, KnownFile};
use mo_library::songs;
use mo_library::watch::Changes;
use mo_library::{Library, LibraryError, Save, SaveDetail, SongChoices};
use rayon::prelude::*;

use crate::background::on_pool;
use crate::shell;
use crate::songs::{ASK_DOUBTS, DOUBTS_ACROSS_FOLDERS, Songs};

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
    installed: Installed,
    next_id: i64,
    details: LiveDetails,
    /// The folders read for the last saves' samples, and when.
    samples: Option<(Instant, Arc<SampleLocator>)>,
    /// The Sounds page's database, where missing samples are looked for last; none in tests.
    sounds_db: Option<PathBuf>,
    /// Saves whose read failed, to try again, with how many tries are left.
    retry: HashMap<String, u8>,
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
}

impl Updated {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.moved.is_empty() && self.gone == 0
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

impl Live {
    /// Reads what the list is made from. Fix's copies still waiting for Keep or Delete are left out: until Keep, a
    /// song opens its original. `gap_months` splits a song's saves into runs.
    pub fn load(library: &Library, gap_months: u32, details: LiveDetails) -> Result<Live, LibraryError> {
        let pending = library.pending_fixes()?;
        let mut saves = library.saves()?;
        // After every row of the library, Fix's copies left out below included, so no two saves share an id.
        let next_id = saves.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        if !pending.is_empty() {
            saves.retain(|s| !pending.contains(&s.path));
        }
        // As the C# app: asked unless Settings turned it off; across folders only when turned on.
        let asking = (library.setting(ASK_DOUBTS)?.as_deref() != Some("0"))
            .then(|| library.setting(DOUBTS_ACROSS_FOLDERS).map(|v| v.as_deref() == Some("1")))
            .transpose()?;
        let mut daw_exes = HashMap::new();
        let daws: BTreeSet<String> = saves.iter().map(|s| s.daw.clone()).chain(DAWS.map(String::from)).collect();
        for daw in daws {
            if let Some(exe) = library.setting(&shell::daw_exe_key(&daw))?.filter(|e| !e.trim().is_empty()) {
                daw_exes.insert(daw, exe);
            }
        }
        Ok(Live {
            saves,
            choices: library.song_choices()?,
            tags: library.all_tags()?.into_iter().map(|(t, _)| t).collect(),
            gap_months,
            asking,
            daw_exes,
            roots: library.roots()?,
            installed: library.installed()?,
            next_id,
            details,
            samples: None,
            sounds_db: None,
            retry: HashMap::new(),
        })
    }

    /// Looks for missing samples in the Sounds library at `sounds_db` too (rule 13).
    pub fn with_sounds(mut self, sounds_db: PathBuf) -> Live {
        self.sounds_db = Some(sounds_db);
        self
    }

    /// The project folders, for the folder watch.
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// The list, grouped into songs.
    pub fn songs(&self) -> Songs {
        let mut songs = Songs::from_saves(&self.saves, &self.choices, self.tags.clone(), self.gap_months, self.asking);
        songs.daw_exes = self.daw_exes.clone();
        songs
    }

    /// The saves whose read failed last time, to read again a moment later; None when there are none.
    pub fn retries(&mut self) -> Option<Changes> {
        let files: Vec<String> = self.retry.keys().cloned().collect();
        (!files.is_empty()).then_some(Changes { files, overflowed: Vec::new() })
    }

    /// Takes in what the folder watch saw: new and changed project files are read, gone ones marked missing, and
    /// folders it couldn't name each file in are walked with the scanner. Asks the disk, so never on the window's
    /// thread; the walking and reading are spread over the app's low-priority threads.
    pub fn update(&mut self, changes: &Changes) -> Updated {
        let mut done = Updated::default();
        let mut paths: Vec<String> = changes.files.clone();
        if !changes.overflowed.is_empty() {
            self.walk(&changes.overflowed, &mut paths, &mut done);
        }
        let mut seen = HashSet::new();
        paths.retain(|p| seen.insert(case::key(p)) && scan::is_project_file(p) && !scan::is_ignored(p, &self.roots));
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
                        marked.insert(i);
                        done.gone += 1;
                    }
                }
            }
        }
        here.retain(|(path, at, meta)| {
            let unchanged = at.and_then(|i| self.saves.get(i)).is_some_and(|s| {
                !s.file_missing
                    && s.error.is_none()
                    && s.mtime == scan::mtime(meta)
                    && s.size == i64::try_from(meta.len()).unwrap_or(i64::MAX)
            });
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
                    } else {
                        done.read.push(detail.save.name.clone());
                    }
                    self.saves[i] = detail.save.clone();
                }
                None => {
                    detail.save.id = self.next_id;
                    self.next_id += 1;
                    self.inherit(&mut detail.save);
                    add_hardware_tags(&mut detail.save);
                    index.insert(case::key(&detail.save.path), self.saves.len());
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
        let found = on_pool(|| scan::find_projects(&walk));
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
                // A missing save whose file is back is read again, even unchanged.
                reread: s.file_missing,
            })
            .collect();
        let plan = scan::plan(&found, &known, &self.roots, &|_| false, false);
        paths.extend(plan.read.into_iter().map(|f| f.path));
        let gone: HashSet<i64> = plan.gone.into_iter().collect();
        paths.extend(self.saves.iter().filter(|s| gone.contains(&s.id)).map(|s| s.path.clone()));
        if !plan.dropped.is_empty() {
            let dropped: HashSet<i64> = plan.dropped.into_iter().collect();
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
            installed: Installed::default(),
            next_id: 100,
            details: LiveDetails::default(),
            samples: None,
            sounds_db: None,
            retry: HashMap::new(),
        }
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
    fn the_card_says_what_changed() {
        let one = Updated { read: vec!["Night Drive_15".into()], ..Updated::default() };
        assert_eq!(
            one.card(),
            ("Updated Night Drive_15".into(), "Saved in its folder, so the app read it again".into())
        );
        let some = Updated { read: vec!["a".into(), "b".into()], moved: vec!["c".into()], gone: 1 };
        assert_eq!(some.card(), ("Updated 4 projects".into(), "2 saved, 1 moved, 1 missing".into()));
        let gone = Updated { gone: 1, ..Updated::default() };
        assert_eq!(gone.card(), ("Updated 1 project".into(), "Missing: moved or deleted".into()));
        let moved = Updated { moved: vec!["Night_11".into()], ..Updated::default() };
        assert_eq!(moved.card(), ("Updated Night_11".into(), "Found in its new folder".into()));
        assert!(Updated::default().is_empty());
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
}
