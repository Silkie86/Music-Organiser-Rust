//! Keeping the Songs list up to date while the app is open, as the C# app's folder watch does: a project saved, added,
//! moved or deleted in your project folders is read (or marked missing) a moment later, without walking every folder
//! again. When the folder watch can't name every file, the folders it names are walked with the scanner instead.
//!
//! Until the library is trusted the app doesn't write it (CLAUDE.md), so what's read here is held in memory: the list
//! shows it, and the C# app's next scan puts the same saves in the library. Saves read here get ids below zero, so
//! the song panel knows to take their details from here rather than from the library.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mo_formats::case;
use mo_library::read::{Installed, read_save};
use mo_library::samples::SampleLocator;
use mo_library::scan::{self, KnownFile};
use mo_library::watch::Changes;
use mo_library::{Library, LibraryError, Save, SaveDetail, SongChoices};

use crate::shell;
use crate::songs::{ASK_DOUBTS, DOUBTS_ACROSS_FOLDERS, Songs};

/// The details of the saves read here, by their ids (below zero), shared with the song panel's reader.
pub type LiveDetails = Arc<Mutex<HashMap<i64, SaveDetail>>>;

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
}

/// What [`Live::update`] did, for its card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Updated {
    /// The saves read again or for the first time, by name.
    pub read: Vec<String>,
    /// How many saves' files have moved or gone.
    pub gone: usize,
}

impl Updated {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.gone == 0
    }

    /// The card's title and line, as the C# app words them: "Updated Night Drive_15", "Saved in its folder, so the
    /// app read it again".
    pub fn card(&self) -> (String, String) {
        let n = self.read.len() + self.gone;
        let title = match (n, self.read.first()) {
            (1, Some(name)) => format!("Updated {name}"),
            _ => format!("Updated {n} projects"),
        };
        let line = match (self.read.len(), self.gone) {
            (1, 0) => "Saved in its folder, so the app read it again".to_owned(),
            (_, 0) => "Saved in their folders, so the app read them again".to_owned(),
            (0, g) => format!("{g} moved or deleted"),
            (r, g) => format!("{r} saved, {g} moved or deleted"),
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
        if !pending.is_empty() {
            saves.retain(|s| !pending.contains(&s.path));
        }
        // As the C# app: asked unless Settings turned it off; across folders only when turned on.
        let asking = (library.setting(ASK_DOUBTS)?.as_deref() != Some("0"))
            .then(|| library.setting(DOUBTS_ACROSS_FOLDERS).map(|v| v.as_deref() == Some("1")))
            .transpose()?;
        let mut daw_exes = HashMap::new();
        let daws: std::collections::BTreeSet<String> = saves.iter().map(|s| s.daw.clone()).collect();
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
            next_id: -1,
            details,
        })
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

    /// Takes in what the folder watch saw: new and changed project files are read, gone ones marked missing, and
    /// folders it couldn't name each file in are walked with the scanner. Asks the disk, so never on the window's
    /// thread.
    pub fn update(&mut self, changes: &Changes) -> Updated {
        let mut paths: Vec<String> = changes.files.clone();
        if !changes.overflowed.is_empty() {
            let found = scan::find_projects(&changes.overflowed);
            let known: Vec<KnownFile> = self
                .saves
                .iter()
                .map(|s| KnownFile {
                    id: s.id,
                    path: s.path.clone(),
                    size: s.size,
                    mtime: s.mtime,
                    missing: s.file_missing,
                    reread: false,
                })
                .collect();
            let plan = scan::plan(&found, &known, &changes.overflowed, &|_| false, false);
            paths.extend(plan.read.into_iter().map(|f| f.path));
            let gone: std::collections::HashSet<i64> = plan.gone.into_iter().collect();
            paths.extend(self.saves.iter().filter(|s| gone.contains(&s.id)).map(|s| s.path.clone()));
        }
        let mut done = Updated::default();
        let samples = SampleLocator::default();
        let mut seen = std::collections::HashSet::new();
        let mut index: HashMap<String, usize> =
            self.saves.iter().enumerate().map(|(i, s)| (case::key(&s.path), i)).collect();
        for path in paths {
            if !seen.insert(case::key(&path)) || !scan::is_project_file(&path) || scan::is_ignored(&path, &self.roots) {
                continue;
            }
            let at = index.get(&case::key(&path)).copied();
            let Ok(meta) = std::fs::metadata(&path) else {
                // Gone: it stays listed, marked missing, as in the C# app.
                if let Some(s) = at.and_then(|i| self.saves.get_mut(i)).filter(|s| !s.file_missing) {
                    s.file_missing = true;
                    done.gone += 1;
                }
                continue;
            };
            if let Some(s) = at.and_then(|i| self.saves.get(i))
                && !s.file_missing
                && s.mtime == scan::mtime(&meta)
                && s.size == i64::try_from(meta.len()).unwrap_or(i64::MAX)
            {
                continue;
            }
            // Still being written, or locked: the next change gets it.
            let Some(mut detail) = read_save(&path, &self.installed, &samples) else { continue };
            if let Some(old) = at.and_then(|i| self.saves.get(i)) {
                keep_your_fields(&mut detail.save, old);
            }
            detail.save.id = self.next_id;
            self.next_id -= 1;
            done.read.push(detail.save.name.clone());
            let save = detail.save.clone();
            if let Ok(mut d) = self.details.lock() {
                d.insert(save.id, detail);
            }
            match at.and_then(|i| self.saves.get_mut(i)) {
                Some(s) => *s = save,
                None => {
                    index.insert(case::key(&save.path), self.saves.len());
                    self.saves.push(save);
                }
            }
        }
        done
    }
}

/// A save read again keeps what you gave it, and what the library worked out that isn't read here yet (its key
/// from the notes): they belong to the save, not to the file's bytes.
fn keep_your_fields(new: &mut Save, old: &Save) {
    new.status = old.status.clone();
    new.rating = old.rating;
    new.favourite = old.favourite;
    new.musical_key = old.musical_key.clone();
    new.notes = old.notes.clone();
    new.tags = old.tags.clone();
    new.last_opened = old.last_opened;
    new.detected_key = old.detected_key.clone();
    new.note_count = old.note_count;
    new.length_bars = old.length_bars;
    new.created = new.created.or(old.created);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_says_what_changed() {
        let one = Updated { read: vec!["Night Drive_15".into()], gone: 0 };
        assert_eq!(
            one.card(),
            ("Updated Night Drive_15".into(), "Saved in its folder, so the app read it again".into())
        );
        let some = Updated { read: vec!["a".into(), "b".into()], gone: 1 };
        assert_eq!(some.card(), ("Updated 3 projects".into(), "2 saved, 1 moved or deleted".into()));
        let gone = Updated { read: vec![], gone: 1 };
        assert_eq!(gone.card().1, "1 moved or deleted");
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
}
