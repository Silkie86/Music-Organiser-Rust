//! Measuring a song's room on disk with what the rest of the library says, so it never offers what another song needs.
//! Shared by the backup's plan ([`crate::backup::prepare`]) and the Space page's songs ([`crate::space_songs`]). Reads
//! nothing itself: mo-space's [`measure`] reads the song's folder.
//!
//! The save fingerprints a look and a backup plan keep between runs are each in a store of their own in the app's data
//! folder ([`SPACE_PRINTS`], [`BACKUP_PRINTS`]): each forgets, after a measure of every song that went to its end, the
//! saves it didn't ask about, so neither holds more than one measure's saves (rule 28), and one can't make the other
//! forget what it still needs.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;

use mo_formats::case;
use mo_formats::paths::{file_name, stem};
use mo_library::Save;
use mo_library::songs::Song;
use mo_space::key;
use mo_space::prints::Prints;
use mo_space::size::Stopped;
use mo_space::song::{BackupFolders, CompactSettings, Compare, Elsewhere, SongSave, SongSpaceReport, measure};

/// The store of the Space page's song look's save fingerprints, in the app's data folder.
pub const SPACE_PRINTS: &str = "space-song-prints.db";
/// The store of the backup plan's save fingerprints, in the app's data folder.
pub const BACKUP_PRINTS: &str = "backup-song-prints.db";

/// The save fingerprints a look or plan keeps between runs, from the store at `path` (made if it isn't there). One that
/// can't be opened says why in the log, under `area` (the page whose look or plan it is): the fingerprints are then
/// kept while the app runs only.
pub fn prints_in(path: &std::path::Path, area: &str) -> Prints {
    let mut prints = Prints::default();
    if let Err(e) = prints.remember_in(path) {
        crate::log::warning(area, &format!("Couldn't open {} to keep save fingerprints: {e}", path.display()));
    }
    prints
}

/// After a measure of every song that used `prints`: keeps what was read for next time, and after one that went to its
/// end (`whole`) forgets the saves it didn't ask about. That's so even with a drive away: its saves are read again when
/// it's back, and a drive gone for good doesn't keep its saves' prints for ever (rule 28). Problems go in the log
/// under `area`.
pub fn keep_prints(prints: Option<&Prints>, whole: bool, area: &str) {
    if let Some(Err(e)) = prints.map(|p| p.keep(whole)) {
        crate::log::warning(area, &format!("Couldn't keep the save fingerprints for next time: {e}"));
    }
}

/// What the other songs say: your sample folders, the save names more than one song has, and the name of every sample
/// any save uses.
pub struct Others {
    sample_folders: Vec<String>,
    /// FL Studio save names more than one song has ("Beat", "Untitled"): its shared Backup folder can't say whose an
    /// autosave of those is, so measuring leaves them alone.
    shared_names: HashSet<String>,
    /// Every sample's file name any save uses, as keys.
    used_names: HashSet<String>,
    /// FL Studio's Backup folders, each listed once for every song measured with these (the whole-app review of 6
    /// Nov, X2-1).
    backup_folders: BackupFolders,
}

impl Others {
    /// From every song in the library (`songs`), the sample paths each save uses (by save id) and your sample folders.
    pub fn new(songs: &[Song], samples: &HashMap<i64, Vec<String>>, sample_folders: Vec<String>) -> Others {
        let mut name_songs: HashMap<String, usize> = HashMap::new();
        for song in songs {
            let names: HashSet<String> = song.saves().map(|v| case::key(stem(&v.path.replace('\\', "/")))).collect();
            for n in names {
                *name_songs.entry(n).or_default() += 1;
            }
        }
        Others {
            sample_folders,
            shared_names: name_songs.into_iter().filter(|(_, n)| *n > 1).map(|(k, _)| k).collect(),
            used_names: samples.values().flatten().map(|p| case::key(file_name(p))).collect(),
            backup_folders: BackupFolders::default(),
        }
    }

    /// Whether any save uses a sample with this file name (letter case aside): what a clean-up's guard asks of audio
    /// before it goes.
    pub fn uses(&self, name: &str) -> bool {
        self.used_names.contains(&case::key(file_name(name)))
    }

    /// Measures one song's `saves` ([`measure`]): `roots` are your project folders, `used` every sample path the
    /// song's saves name. `opens` is the save the song opens (its highest-numbered one, as the Songs page has it): it's
    /// never on the list, even when by time alone it's from the middle of a sitting or a copy of a newer save (a copied
    /// folder gives every save a new time). `compare` says how the saves of the same size are compared ([`Compare`]).
    /// Reads the song's folder: call it away from the window's thread.
    #[allow(clippy::too_many_arguments)]
    pub fn measure(
        &self,
        saves: &[SongSave],
        opens: Option<&str>,
        roots: &[String],
        used: &[String],
        settings: CompactSettings,
        compare: Compare<'_>,
        stop: &AtomicBool,
    ) -> Result<SongSpaceReport, Stopped> {
        let used_by_any = |names: &[String]| -> HashSet<String> {
            names.iter().map(|n| case::key(n)).filter(|k| self.used_names.contains(k)).collect()
        };
        let elsewhere = Elsewhere {
            sample_folders: &self.sample_folders,
            used_by_any: &used_by_any,
            shared_names: &self.shared_names,
            backup_folders: &self.backup_folders,
            compare,
        };
        let mut report = measure(saves, roots, used, settings, Some(&elsewhere), stop)?;
        if let Some(opens) = opens.map(key) {
            report.items.retain(|i| key(&i.path) != opens);
        }
        Ok(report)
    }
}

/// Saves as mo-space measures them: where each is, the size the library has and when it was written.
pub fn song_saves<'a>(saves: impl IntoIterator<Item = &'a Save>) -> Vec<SongSave> {
    saves
        .into_iter()
        .map(|v| SongSave { path: v.path.clone(), size: u64::try_from(v.size).unwrap_or(0), modified: v.modified })
        .collect()
}
