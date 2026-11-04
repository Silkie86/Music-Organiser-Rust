//! Measuring a song's room on disk with what the rest of the library says, so it never offers what another song needs.
//! Shared by the backup's plan ([`crate::backup::prepare`]) and the Space page's songs ([`crate::space_songs`]). Reads
//! nothing itself: mo-space's [`measure`] reads the song's folder.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;

use mo_formats::case;
use mo_formats::paths::{file_name, stem};
use mo_library::Save;
use mo_library::songs::Song;
use mo_space::key;
use mo_space::size::Stopped;
use mo_space::song::{CompactSettings, Elsewhere, SongSave, SongSpaceReport, measure};

/// What the other songs say: your sample folders, the save names more than one song has, and the name of every sample
/// any save uses.
pub struct Others {
    sample_folders: Vec<String>,
    /// FL Studio save names more than one song has ("Beat", "Untitled"): its shared Backup folder can't say whose an
    /// autosave of those is, so measuring leaves them alone.
    shared_names: HashSet<String>,
    /// Every sample's file name any save uses, as keys.
    used_names: HashSet<String>,
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
        }
    }

    /// Measures one song's `saves` ([`measure`]): `roots` are your project folders, `used` every sample path the
    /// song's saves name. `opens` is the save the song opens (its highest-numbered one, as the Songs page has it): it's
    /// never on the list, even when by time alone it's from the middle of a sitting or a copy of a newer save (a copied
    /// folder gives every save a new time). Reads the song's folder: call it away from the window's thread.
    pub fn measure(
        &self,
        saves: &[SongSave],
        opens: Option<&str>,
        roots: &[String],
        used: &[String],
        settings: CompactSettings,
        stop: &AtomicBool,
    ) -> Result<SongSpaceReport, Stopped> {
        let used_by_any = |names: &[String]| -> HashSet<String> {
            names.iter().map(|n| case::key(n)).filter(|k| self.used_names.contains(k)).collect()
        };
        let elsewhere = Elsewhere {
            sample_folders: &self.sample_folders,
            used_by_any: &used_by_any,
            shared_names: &self.shared_names,
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
