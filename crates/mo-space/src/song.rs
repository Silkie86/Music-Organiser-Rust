//! A song's room on disk, and what could go to free some without losing any music: the C# app's `SongSpace` (the song
//! panel's Space tab, Compact and Thin older saves).
//!
//! Nothing here removes anything: [`measure`] only lists. What you pick from the list goes through
//! [`crate::clean`], which checks every line again and only ever sends things to the Recycle Bin.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{NaiveDateTime, TimeDelta};
use mo_formats::case;
use mo_formats::paths::{extension, file_name, stem};
use mo_formats::textscan::is_audio_path;
use mo_library::scan::{daw_of, is_autosave_dir, is_autosave_file, is_inside, is_project_file};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::key;
use crate::size::{self, Stopped};

/// What a song's space can be freed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompactKind {
    /// A save that's byte for byte the same as another save in the same folder.
    SameSave,
    /// A DAW's own autosave copy, older than the newest few.
    Backup,
    /// Files the DAW makes again by itself when the song opens (Studio One's Cache folder).
    Cache,
    /// Audio in the song's folder that no save seems to use: never ticked for you; you listen and pick each one.
    UnusedAudio,
    /// A save from the middle of a sitting: one of a burst of saves made close together. Only picked by hand: the
    /// first save, your newest and the last save of each sitting always stay.
    ThinnedSave,
}

/// Something in a song's folder that could go: one file, or (for a cache) one folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactItem {
    pub kind: CompactKind,
    pub path: String,
    pub bytes: u64,
    /// When the save or autosave was written (local time).
    pub modified: Option<NaiveDateTime>,
    /// For a save that's a copy: the save that stays in its place.
    pub same_as: Option<String>,
}

impl CompactItem {
    /// Removable without losing anything. Unused audio might be music (an old bounce), so it's only ever picked by
    /// hand.
    pub fn can_remove(&self) -> bool {
        self.kind != CompactKind::UnusedAudio
    }

    /// Whether the list ticks it for you: copies, old autosaves and caches. Thinned saves and unused audio wait for you
    /// to pick them.
    pub fn ticked_at_first(&self) -> bool {
        matches!(self.kind, CompactKind::SameSave | CompactKind::Backup | CompactKind::Cache)
    }

    /// The line's note, as the C# app writes it.
    pub fn note(&self) -> String {
        match self.kind {
            CompactKind::SameSave => format!("Same as {}", file_name(self.same_as.as_deref().unwrap_or_default())),
            CompactKind::ThinnedSave => {
                format!("Saved {}", self.modified.map(|m| m.format("%-d %b %Y %H:%M").to_string()).unwrap_or_default())
            }
            CompactKind::Backup => {
                format!(
                    "Autosave from {}",
                    self.modified.map(|m| m.format("%-d %b %Y").to_string()).unwrap_or_default()
                )
            }
            CompactKind::Cache => "Studio One makes these again when the song opens".to_owned(),
            CompactKind::UnusedAudio => file_name(&self.path).to_owned(),
        }
    }
}

/// Where a song's space goes, and what could be freed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongSpaceReport {
    /// Everything below added up.
    pub bytes: u64,
    /// Whether the song has its folder to itself (then everything in it counts as the song's).
    pub own_folder: bool,
    /// The folder of the newest save.
    pub folder: String,
    pub saves: u64,
    pub backups: u64,
    pub caches: u64,
    pub audio: u64,
    pub other: u64,
    pub items: Vec<CompactItem>,
}

impl SongSpaceReport {
    /// What Compact frees: copies, old autosaves and caches (thinning and unused audio are extra, picked by hand).
    pub fn freeable(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.can_remove() && i.kind != CompactKind::ThinnedSave)
            .map(|i| i.bytes)
            .fold(0, u64::saturating_add)
    }

    /// What thinning older saves could free on top, if you pick it.
    pub fn thinnable(&self) -> u64 {
        self.items.iter().filter(|i| i.kind == CompactKind::ThinnedSave).map(|i| i.bytes).fold(0, u64::saturating_add)
    }
}

/// One save of the song, as the library lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SongSave {
    pub path: String,
    /// The size the library has; the file's own size is used when it's there.
    pub size: u64,
    /// When it was written (local time).
    pub modified: NaiveDateTime,
}

/// How many of the newest autosaves stay.
pub const DEFAULT_KEEP_BACKUPS: usize = 5;

/// How long without a save ends a sitting. Measured on Solomon's FL Studio saves (26 Oct 2026): half the gaps between a
/// song's saves are under 3 hours, then hardly any fall between 3 and 6 hours, so 3 hours parts one sitting from the
/// next.
pub const DEFAULT_SITTING_HOURS: u32 = 3;

/// Your choices for Compact (Settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactSettings {
    /// How many of the newest autosaves stay.
    pub keep_backups: usize,
    /// How long without a save ends a sitting, for thinning older saves; 0 doesn't thin.
    pub sitting_hours: u32,
}

impl Default for CompactSettings {
    fn default() -> CompactSettings {
        CompactSettings { keep_backups: DEFAULT_KEEP_BACKUPS, sitting_hours: DEFAULT_SITTING_HOURS }
    }
}

/// What the rest of the library says, so Compact never offers what another song needs.
pub struct Elsewhere<'a> {
    /// Your sample folders: audio in them is never offered.
    pub sample_folders: &'a [String],
    /// Which of these audio file names any project's samples have (by name, letter case aside), as [`key`]s.
    pub used_by_any: &'a (dyn Fn(&[String]) -> HashSet<String> + Sync),
    /// Names of the song's FL Studio saves that another song elsewhere has too ("Beat", "Untitled"), as [`key`]s: FL
    /// Studio's shared Backup folder doesn't say which song an autosave is of, so those autosaves are left alone.
    pub shared_names: &'a HashSet<String>,
}

/// Measures a song: its saves, the DAW's autosaves of it, and (when the song has a folder to itself) everything in that
/// folder. Then lists what can go without losing any music: saves that are exact copies of another, autosaves beyond
/// the newest few, caches the DAW rebuilds, and (picked by hand only) saves from the middle of a sitting and audio no
/// project uses. Your newest save is never on the list, and neither is any sample in your sample folders.
///
/// `roots` are your project folders (a song never has one of these to itself). `used_audio` is every sample path the
/// song's saves use: projects store paths as they were on the PC that saved them, so a file counts as used when any
/// save names a file of that name. Reads every file of the song's folder: call it away from the window's thread.
pub fn measure(
    saves: &[SongSave],
    roots: &[String],
    used_audio: &[String],
    settings: CompactSettings,
    elsewhere: Option<&Elsewhere<'_>>,
    stop: &AtomicBool,
) -> Result<SongSpaceReport, Stopped> {
    let Some(newest) = newest(saves) else { return Ok(SongSpaceReport::default()) };
    let folder = parent(&newest.path);
    let save_keys: HashSet<String> = saves.iter().map(|s| key(&s.path)).collect();
    let own = owns_folder(&folder, &save_keys, roots, stop)?;

    let mut report = SongSpaceReport { own_folder: own, folder: folder.clone(), ..SongSpaceReport::default() };
    let mut backups: Vec<(String, u64, NaiveDateTime)> = Vec::new();
    let mut unused_audio: Vec<(String, u64)> = Vec::new();
    let mut cache: Option<String> = None;
    let mut seen: HashSet<String> = HashSet::new();
    let used_full: HashSet<String> = used_audio.iter().map(|u| key(u)).collect();
    let used_names: HashSet<String> = used_audio.iter().map(|u| case::key(file_name(u))).collect();

    if own {
        let dir = Path::new(&folder);
        let studio_one = has_studio_one_song(dir);
        for f in size::files(dir, stop)? {
            let path = f.path.to_string_lossy().into_owned();
            let path_key = key(&path);
            seen.insert(path_key.clone());
            let rel = relative_parts(dir, &f.path);
            let (folders, name) = match rel.split_last() {
                Some((name, folders)) => (folders, name.as_str()),
                None => (&[][..], ""),
            };
            if save_keys.contains(&path_key) {
                report.saves += f.size;
            } else if daw_of(&path).is_some() && (folders.iter().any(|d| is_autosave_dir(d)) || is_autosave_file(name))
            {
                // Only project files are autosaves: audio or anything else kept in a "Backups" folder is yours.
                report.backups += f.size;
                backups.push((path, f.size, f.modified));
            } else if studio_one && folders.first().is_some_and(|d| case::eq_ignore_case(d, "Cache")) {
                report.caches += f.size;
                cache = Some(dir.join(folders.first().map_or("Cache", |d| d.as_str())).to_string_lossy().into_owned());
            } else if is_audio_path(&path) {
                report.audio += f.size;
                if !used_full.contains(&path_key) && !used_names.contains(&case::key(name)) {
                    unused_audio.push((path, f.size));
                }
            } else {
                report.other += f.size;
            }
        }
    }
    // Saves outside the folder (or all of them, when the song shares its folder) count on their own.
    for s in saves {
        if seen.insert(key(&s.path)) {
            report.saves += fs::metadata(&s.path).map_or(s.size, |m| m.len());
        }
    }

    // Audio in your sample folders, or that any other project uses by name, isn't the song's to offer.
    if let Some(elsewhere) = elsewhere
        && !unused_audio.is_empty()
    {
        unused_audio.retain(|(p, _)| !elsewhere.sample_folders.iter().any(|f| is_inside(p, f)));
        let mut names: Vec<String> = Vec::new();
        let mut name_keys: HashSet<String> = HashSet::new();
        for (p, _) in &unused_audio {
            if name_keys.insert(case::key(file_name(p))) {
                names.push(file_name(p).to_owned());
            }
        }
        let used = (elsewhere.used_by_any)(&names);
        unused_audio.retain(|(p, _)| !used.contains(&case::key(file_name(p))));
    }

    // FL Studio keeps its autosaves in one Backup folder for all songs (Projects\Backup), named after the song.
    for b in fl_backups(saves, roots, elsewhere.map(|e| e.shared_names)) {
        if seen.insert(key(&b.0)) {
            report.backups += b.1;
            backups.push(b);
        }
    }

    if stop.load(Ordering::Relaxed) {
        return Err(Stopped);
    }
    let same = same_saves(saves, newest, stop)?;
    // Neither a copy nor the save it's a copy of is thinned: removing both would lose what they held (the last save
    // of a sitting, kept as a later identical save).
    let copies: HashSet<String> =
        same.iter().flat_map(|i| [key(&i.path), key(i.same_as.as_deref().unwrap_or_default())]).collect();
    report.items.extend(same);
    for s in thin(saves, settings.sitting_hours) {
        if !copies.contains(&key(&s.path)) {
            report.items.push(CompactItem {
                kind: CompactKind::ThinnedSave,
                bytes: fs::metadata(&s.path).map_or(s.size, |m| m.len()),
                path: s.path.clone(),
                modified: Some(s.modified),
                same_as: None,
            });
        }
    }
    // The newest few autosaves stay: they're what you'd reach for after a crash.
    backups.sort_by_key(|b| std::cmp::Reverse(b.2));
    for (path, bytes, modified) in backups.into_iter().skip(settings.keep_backups) {
        report.items.push(CompactItem {
            kind: CompactKind::Backup,
            path,
            bytes,
            modified: Some(modified),
            same_as: None,
        });
    }
    if let Some(cache) = cache
        && report.caches > 0
    {
        report.items.push(CompactItem {
            kind: CompactKind::Cache,
            path: cache,
            bytes: report.caches,
            modified: None,
            same_as: None,
        });
    }
    // One line each: you listen to each and pick (never ticked for you).
    unused_audio.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (path, bytes) in unused_audio {
        report.items.push(CompactItem { kind: CompactKind::UnusedAudio, path, bytes, modified: None, same_as: None });
    }
    report.bytes = report.saves + report.backups + report.caches + report.audio + report.other;
    Ok(report)
}

/// The newest save; of two saved at the same moment, the first listed (as .NET's `MaxBy`).
fn newest(saves: &[SongSave]) -> Option<&SongSave> {
    saves.iter().fold(None, |best: Option<&SongSave>, s| match best {
        Some(b) if b.modified >= s.modified => Some(b),
        _ => Some(s),
    })
}

/// Whether a folder holds a Studio One song (a `.song` file right in it).
pub(crate) fn has_studio_one_song(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_type().is_ok_and(|t| t.is_file())
                && case::eq_ignore_case(extension(&e.file_name().to_string_lossy()), ".song")
        })
    })
}

/// The folder a path is in, or "" for none.
pub(crate) fn parent(path: &str) -> String {
    Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The names from `dir` down to `path`: its folders, then its own name.
fn relative_parts(dir: &Path, path: &Path) -> Vec<String> {
    path.strip_prefix(dir)
        .map(|rel| rel.iter().map(|p| p.to_string_lossy().into_owned()).collect())
        .unwrap_or_else(|_| vec![file_name(&path.to_string_lossy()).to_owned()])
}

/// The saves thinning would remove (Solomon's F12 note, 26 Oct: "thin the older saves so the first one and the most
/// important ones are kept, the ones after a flurry of saves"). Saves less than `sitting_hours` apart make one
/// sitting; the last save of each sitting stays, as do the very first save and the newest (the one [`measure`] keeps,
/// even when another was written at the same moment).
pub fn thin(saves: &[SongSave], sitting_hours: u32) -> Vec<&SongSave> {
    if sitting_hours == 0 || saves.len() < 3 {
        return Vec::new();
    }
    let newest = newest(saves).map(|n| key(&n.path)).unwrap_or_default();
    let mut ordered: Vec<&SongSave> = saves.iter().collect();
    // Stable, as .NET's OrderBy: saves written at the same moment keep their order.
    ordered.sort_by_key(|s| s.modified);
    let gap = TimeDelta::hours(i64::from(sitting_hours));
    ordered
        .windows(2)
        .skip(1)
        .filter_map(|w| match w {
            [a, b] if b.modified - a.modified < gap && key(&a.path) != newest => Some(*a),
            _ => None,
        })
        .collect()
}

/// Whether the folder is the song's own: none of your project folders, and every project file in it, or in a folder in
/// it, is one of the song's saves (autosave folders aside). Bitwig and Studio One give each song a folder; FL Studio
/// songs often share one, and another song kept in a folder inside this one has its own autosaves there.
pub fn owns_folder(
    folder: &str,
    save_keys: &HashSet<String>,
    roots: &[String],
    stop: &AtomicBool,
) -> Result<bool, Stopped> {
    // A link to another place isn't walked, so it can't be shown to hold only the song's saves; nor can a folder that
    // can't be read.
    let real_folder = fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    if folder.is_empty() || !real_folder || fs::read_dir(folder).is_err() {
        return Ok(false);
    }
    // A project folder, or one holding it.
    if roots.iter().any(|r| is_inside(r, folder)) {
        return Ok(false);
    }
    // Every file, OneDrive's online-only ones too: another song's save takes no room here, but it's still there.
    let dir = Path::new(folder);
    let others = size::walk(dir, true, stop, |path, _| {
        let p = path.to_string_lossy();
        if !is_project_file(&p) || save_keys.contains(&key(&p)) {
            return None;
        }
        let rel = relative_parts(dir, path);
        let in_autosaves = rel.split_last().is_some_and(|(_, folders)| folders.iter().any(|d| is_autosave_dir(d)));
        (!in_autosaves).then_some(())
    })?;
    Ok(others.is_empty())
}

/// FL Studio's autosaves of the song: "Song (autosave).flp" and "Song (overwritten at 2213h05).flp" in a Backup folder
/// beside the song or in a folder above it, up to your project folder. Names another song has too (`shared`) are left
/// out: the autosaves could be that song's.
fn fl_backups(
    saves: &[SongSave],
    roots: &[String],
    shared: Option<&HashSet<String>>,
) -> Vec<(String, u64, NaiveDateTime)> {
    let stems: HashSet<String> = saves
        .iter()
        .filter(|s| case::eq_ignore_case(extension(file_name(&s.path)), ".flp"))
        .map(|s| case::key(stem(&s.path.replace('\\', "/"))))
        .filter(|n| shared.is_none_or(|shared| !shared.contains(n)))
        .collect();
    if stems.is_empty() {
        return Vec::new();
    }
    let mut starts: Vec<String> = Vec::new();
    let mut start_keys: HashSet<String> = HashSet::new();
    for s in saves {
        let dir = parent(&s.path);
        if start_keys.insert(key(&dir)) {
            starts.push(dir);
        }
    }
    let mut backup_dirs: Vec<PathBuf> = Vec::new();
    let mut backup_keys: HashSet<String> = HashSet::new();
    for start in starts {
        let mut dir = Some(PathBuf::from(start));
        while let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
            let b = d.join("Backup");
            if b.is_dir() && backup_keys.insert(key(&b.to_string_lossy())) {
                backup_dirs.push(b);
            }
            let here = d.to_string_lossy();
            // Up to the project folder.
            if roots.iter().any(|r| is_inside(r, &here)) || backup_dirs.len() > 4 {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    let mut found = Vec::new();
    for b in backup_dirs {
        let Ok(entries) = fs::read_dir(&b) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !case::eq_ignore_case(extension(&name), ".flp") || !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let song = case::key(stem(&name));
            let cut = song.find(" (AUTOSAVE)").or_else(|| song.find(" (OVERWRITTEN AT "));
            if let Some(cut) = cut.filter(|&c| c > 0)
                && song.get(..cut).is_some_and(|s| stems.contains(s))
            {
                found.push((e.path().to_string_lossy().into_owned(), meta.len(), size::modified_of(&meta)));
            }
        }
    }
    found
}

/// Saves that are exact copies of another save in the same folder (a DAW saving with nothing changed). Of each set
/// the newest stays, so the save you'd open is never on the list. Only files of the same size are read, to compare
/// them, each once and at the same time.
fn same_saves(saves: &[SongSave], newest: &SongSave, stop: &AtomicBool) -> Result<Vec<CompactItem>, Stopped> {
    // By folder and size, in the order the saves come. Links aren't compared (a link to the save beside it would be
    // "the same"), and nor are OneDrive files that are only online (reading one downloads it).
    let mut sets: Vec<((String, u64), Vec<&SongSave>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for s in saves {
        let Ok(meta) = fs::symlink_metadata(&s.path) else { continue };
        if !meta.is_file() || size::is_online_only(&meta) || !seen.insert(key(&s.path)) {
            continue;
        }
        let set_key = (key(&parent(&s.path)), meta.len());
        match sets.iter_mut().find(|(k, _)| *k == set_key) {
            Some((_, set)) => set.push(s),
            None => sets.push((set_key, vec![s])),
        }
    }
    let mut items = Vec::new();
    for ((_, bytes), set) in sets.into_iter().filter(|(_, set)| set.len() > 1) {
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let hashes: Vec<Option<[u8; 32]>> = set.par_iter().map(|s| size::file_hash(Path::new(&s.path), stop)).collect();
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let mut groups: Vec<([u8; 32], Vec<&SongSave>)> = Vec::new();
        for (s, h) in set.iter().zip(hashes) {
            let Some(h) = h else { continue };
            match groups.iter_mut().find(|(g, _)| *g == h) {
                Some((_, group)) => group.push(s),
                None => groups.push((h, vec![s])),
            }
        }
        for (_, same) in groups.into_iter().filter(|(_, g)| g.len() > 1) {
            let keep = if same.iter().any(|s| key(&s.path) == key(&newest.path)) {
                newest
            } else {
                match newest_of(&same) {
                    Some(k) => k,
                    None => continue,
                }
            };
            for s in same.iter().filter(|s| key(&s.path) != key(&keep.path)) {
                items.push(CompactItem {
                    kind: CompactKind::SameSave,
                    path: s.path.clone(),
                    bytes,
                    modified: Some(s.modified),
                    same_as: Some(keep.path.clone()),
                });
            }
        }
    }
    Ok(items)
}

fn newest_of<'a>(saves: &[&'a SongSave]) -> Option<&'a SongSave> {
    saves.iter().fold(None, |best: Option<&SongSave>, s| match best {
        Some(b) if b.modified >= s.modified => Some(b),
        _ => Some(s),
    })
}
