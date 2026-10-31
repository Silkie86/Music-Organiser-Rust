//! What a backup copies, and what it leaves out and why: the backup list, before anything is copied.
//!
//! The caller walks your music folders with [`mo_space::size::files`] (which leaves links alone, so a folder moved to
//! another drive is backed up once, from where it really is: put the moved folder's new place in the list of folders),
//! measures each song with [`mo_space::song::measure`], and gathers every sample path your projects use. [`plan`] then
//! sorts each file by the [`Level`] you pick:
//!
//! - **Full:** everything goes, every save (caches too).
//! - **Essentials:** each song's saves are only its [`milestones`] (any size); everything else as Slim. Measure songs
//!   with `keep_backups` 0 for it, so every autosave is on the Compact list and stays out.
//! - **Slim** (the first choice):
//!
//! - **Left out, whatever its size:** a save that's byte for byte the same as another save of the song, a DAW
//!   autosave older than the newest few, and caches the DAW makes again (Studio One's Cache folder).
//! - **Left out when big:** a save from the middle of a sitting (the first save, your newest and the last save of each
//!   sitting always go), or with [`BigSaves::NewestOnly`] every big save but the newest; and audio no project uses.
//! - **Everything else goes:** small saves (they're cheap and keep a song's history), every sample a project uses,
//!   audio beside a save or in a song's own folder (bounces, stems, recordings), anything in a folder you mark
//!   "always", sample libraries (unless you choose to leave unused ones out), presets, MIDI and the rest.
//!
//! Your newest save of each song, and any save you mark as a milestone, always goes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use mo_formats::case;
use mo_formats::textscan::is_audio_path;
use mo_space::key;
use mo_space::size::{FileEntry, is_sound};
use mo_space::song::{CompactKind, SongSave, SongSpaceReport};

/// 1 MB, as Explorer counts it.
const MB: u64 = 1 << 20;

/// Sound files at least this big that no project uses stay out (Settings can change it).
pub const DEFAULT_BIG_UNUSED: u64 = 10 * MB;

/// Saves at least this big are "big saves": only the ones worth keeping go (Settings can change it).
pub const DEFAULT_BIG_SAVE: u64 = 5 * MB;

/// Which of a song's big saves go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BigSaves {
    /// The first, the newest and the last save of each sitting (as Thin keeps them).
    #[default]
    SittingEnds,
    /// Only the newest.
    NewestOnly,
}

/// How much of your music a backup takes (Solomon, 31 Oct 2026: a choice on the backup screen).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    /// Everything in your folders, every save.
    Full,
    /// Lean: big unused audio and big mid-sitting saves stay out (the module's notes list every rule).
    #[default]
    Slim,
    /// Only each song's milestone saves (any size); every other file as Slim. [`BigSaves`] and the copy rule don't
    /// apply to milestones: each one goes.
    Essentials,
}

/// How lean the backup is: the level is picked on the backup screen and remembered; the sizes are in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Full, Slim or Essentials.
    pub level: Level,
    /// Sound files at least this big that no project uses stay out.
    pub big_unused: u64,
    /// Saves at least this big are big saves.
    pub big_save: u64,
    pub big_saves: BigSaves,
    /// Also leave out big sample-library files (Kontakt, SoundFonts, REX...) no project names. Off at first: projects
    /// use these through a plugin, so a project's sample list never names them.
    pub leave_out_unused_libraries: bool,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules {
            level: Level::default(),
            big_unused: DEFAULT_BIG_UNUSED,
            big_save: DEFAULT_BIG_SAVE,
            big_saves: BigSaves::default(),
            leave_out_unused_libraries: false,
        }
    }
}

/// Why a file stays out of the backup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeftOut {
    /// Byte for byte the same as another save of the song, which goes.
    SameSave,
    /// A DAW autosave older than the newest few.
    OldAutosave,
    /// The DAW makes it again when the song opens.
    Cache,
    /// A big save from the middle of a sitting.
    MidSittingSave,
    /// A big save older than the newest ([`BigSaves::NewestOnly`]).
    OlderBigSave,
    /// A big sound file no project uses.
    UnusedSound,
    /// A save that isn't one of the song's milestones ([`Level::Essentials`]).
    NotMilestone,
}

impl LeftOut {
    /// The line's note on the backup card.
    pub fn note(self) -> &'static str {
        match self {
            LeftOut::SameSave => "Same as another save of the song",
            LeftOut::OldAutosave => "Older autosave",
            LeftOut::Cache => "The DAW makes this again",
            LeftOut::MidSittingSave => "Big save from the middle of a sitting",
            LeftOut::OlderBigSave => "Older big save",
            LeftOut::UnusedSound => "Big sound no project uses",
            LeftOut::NotMilestone => "Not one of the song's milestones",
        }
    }
}

/// A file left out, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub file: FileEntry,
    pub why: LeftOut,
}

/// The backup list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// What goes, in the order the walk met it.
    pub copy: Vec<FileEntry>,
    /// What stays out, biggest first.
    pub left_out: Vec<Skipped>,
}

impl Plan {
    /// The room the files that go take (before identical files are stored once).
    pub fn copy_bytes(&self) -> u64 {
        self.copy.iter().map(|f| f.size).fold(0, u64::saturating_add)
    }

    /// The room left out.
    pub fn left_out_bytes(&self) -> u64 {
        self.left_out.iter().map(|s| s.file.size).fold(0, u64::saturating_add)
    }
}

/// Every sample path your projects use. Projects keep paths as they were on the PC that saved them, so (as the Space
/// page does) a file counts as used when its full path matches, or when any project names a file of the same name.
#[derive(Clone, Debug, Default)]
pub struct Used {
    paths: HashSet<String>,
    names: HashSet<String>,
}

impl Used {
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a str>) -> Used {
        let mut used = Used::default();
        for p in paths {
            used.paths.insert(key(p));
            used.names.insert(case::key(file_name(p)));
        }
        used
    }

    /// Whether any project uses this file.
    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(&key(path)) || self.names.contains(&case::key(file_name(path)))
    }
}

/// One song as the backup sees it: its Compact list (from [`mo_space::song::measure`], with sitting thinning on) and
/// its saves.
pub struct SongInfo<'a> {
    pub report: &'a SongSpaceReport,
    pub saves: &'a [SongSave],
    /// More milestone saves on top of those found here (ones you've marked, ones where the song changed a lot).
    pub milestones: &'a [String],
}

/// A song's milestone saves, as keys: its first and newest, the last save of each day it was worked on, and any the
/// caller adds (`more`). Days follow sittings (saves less than [`mo_space::song::DEFAULT_SITTING_HOURS`] apart): a
/// sitting that runs past midnight counts for the day it began, so only its last save is a milestone, never one from
/// its middle.
pub fn milestones(saves: &[SongSave], more: &[String]) -> HashSet<String> {
    let mut out: HashSet<String> = more.iter().map(|m| key(m)).collect();
    if let Some(n) = newest_save(saves) {
        out.insert(key(&n.path));
    }
    let mut ordered: Vec<&SongSave> = saves.iter().collect();
    // Stable: saves written at the same moment keep their order, so the first listed counts as the first.
    ordered.sort_by_key(|s| s.modified);
    if let Some(first) = ordered.first() {
        out.insert(key(&first.path));
    }
    let gap = chrono::TimeDelta::hours(i64::from(mo_space::song::DEFAULT_SITTING_HOURS));
    // The last save of each sitting, with the day the sitting began.
    let mut ends: Vec<(chrono::NaiveDate, &SongSave)> = Vec::new();
    let mut began = ordered.first().map(|s| s.modified.date());
    for (i, s) in ordered.iter().enumerate() {
        let next = ordered.get(i + 1);
        if next.is_none_or(|n| n.modified - s.modified >= gap) {
            if let Some(day) = began {
                ends.push((day, s));
            }
            began = next.map(|n| n.modified.date());
        }
    }
    let mut last_of_day: HashMap<chrono::NaiveDate, &SongSave> = HashMap::new();
    for (day, s) in ends {
        last_of_day.insert(day, s);
    }
    out.extend(last_of_day.values().map(|s| key(&s.path)));
    out
}

/// Folders the backup treats in a special way.
#[derive(Clone, Copy, Debug)]
pub struct Folders<'a> {
    /// Where the backup goes. Files already in it are never planned again, so a backup inside one of your music
    /// folders doesn't copy itself into itself.
    pub backup: &'a Path,
    /// Folders whose files always go, whatever their size and whether a project uses them (your bounces and exports
    /// folder, say).
    pub always: &'a [PathBuf],
}

/// Sorts the walked files into what goes and what's left out. A file met twice (two folders in the list that overlap)
/// goes once.
pub fn plan(files: Vec<FileEntry>, songs: &[SongInfo<'_>], used: &Used, folders: Folders<'_>, rules: Rules) -> Plan {
    let mut newest: HashSet<String> = HashSet::new();
    let mut saves: HashSet<String> = HashSet::new();
    // Audio beside a save, or anywhere in a song's own folder, is the song's (a bounce, a stem, a recording): it goes.
    let mut beside_saves: HashSet<String> = HashSet::new();
    let mut song_folders: HashSet<String> = HashSet::new();
    let mut milestone_saves: HashSet<String> = HashSet::new();
    for song in songs {
        if rules.level == Level::Essentials {
            milestone_saves.extend(milestones(song.saves, song.milestones));
        } else {
            // Saves you mark always go, whatever the level.
            milestone_saves.extend(song.milestones.iter().map(|m| key(m)));
        }
        if let Some(n) = newest_save(song.saves) {
            newest.insert(key(&n.path));
        }
        for s in song.saves {
            saves.insert(key(&s.path));
            beside_saves.insert(parent_key(&key(&s.path)).to_owned());
        }
        if song.report.own_folder && !song.report.folder.is_empty() {
            song_folders.insert(key(&song.report.folder));
        }
    }
    song_folders.extend(folders.always.iter().map(|f| key(&f.to_string_lossy())));
    let mut compact: HashMap<String, CompactKind> = HashMap::new();
    let mut caches: HashSet<String> = HashSet::new();
    for item in songs.iter().flat_map(|s| &s.report.items) {
        let k = key(&item.path);
        match item.kind {
            CompactKind::Cache => {
                caches.insert(k);
            }
            // Your newest save of a song always goes, whatever another song's list says of it.
            _ if newest.contains(&k) => {}
            kind => {
                compact.insert(k, kind);
            }
        }
    }
    let ctx = Context { compact, caches, newest, saves, milestone_saves, beside_saves, song_folders, used, rules };
    let backup = key(&folders.backup.to_string_lossy());

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Plan::default();
    for file in files {
        let path = file.path.to_string_lossy().into_owned();
        let k = key(&path);
        if is_inside(&k, &backup) {
            continue;
        }
        // Windows paths ignore letter case; on a Mac or Linux "a.mid" and "A.MID" can be two files.
        let same = if cfg!(windows) { k.clone() } else { path.clone() };
        if !seen.insert(same) {
            continue;
        }
        match ctx.left_out(&k, &path, file.size) {
            Some(why) => out.left_out.push(Skipped { file, why }),
            None => out.copy.push(file),
        }
    }
    out.left_out.sort_by(|a, b| b.file.size.cmp(&a.file.size).then_with(|| a.file.path.cmp(&b.file.path)));
    out
}

struct Context<'a> {
    compact: HashMap<String, CompactKind>,
    caches: HashSet<String>,
    newest: HashSet<String>,
    saves: HashSet<String>,
    milestone_saves: HashSet<String>,
    beside_saves: HashSet<String>,
    song_folders: HashSet<String>,
    used: &'a Used,
    rules: Rules,
}

impl Context<'_> {
    fn left_out(&self, k: &str, path: &str, size: u64) -> Option<LeftOut> {
        let rules = self.rules;
        if rules.level == Level::Full || self.milestone_saves.contains(k) {
            return None;
        }
        // Essentials: a song's saves are its milestones, whatever their size (a milestone that's a byte copy of
        // another save still goes: the save it copies may not be one).
        if rules.level == Level::Essentials && self.saves.contains(k) {
            return Some(LeftOut::NotMilestone);
        }
        let big_save = size >= rules.big_save;
        match self.compact.get(k) {
            Some(CompactKind::SameSave) => return Some(LeftOut::SameSave),
            Some(CompactKind::Backup) => return Some(LeftOut::OldAutosave),
            Some(CompactKind::ThinnedSave) if big_save => return Some(LeftOut::MidSittingSave),
            _ => {}
        }
        if folders_above(k).any(|f| self.caches.contains(f)) {
            return Some(LeftOut::Cache);
        }
        let is_save = self.saves.contains(k);
        if is_save {
            if rules.big_saves == BigSaves::NewestOnly && big_save && !self.newest.contains(k) {
                return Some(LeftOut::OlderBigSave);
            }
            return None;
        }
        // Plain audio, and (only when you turn it on) sample-library files, which a project uses through a plugin, so
        // no project's sample list names them.
        let sound = is_audio_path(path) || (rules.leave_out_unused_libraries && is_sound(path, 0));
        if size >= rules.big_unused
            && sound
            && !self.beside_saves.contains(parent_key(k))
            && !folders_above(k).any(|f| self.song_folders.contains(f))
            && !self.used.contains(path)
        {
            return Some(LeftOut::UnusedSound);
        }
        None
    }
}

/// The newest save; of two saved at the same moment, the first listed (as the Space page picks it).
fn newest_save(saves: &[SongSave]) -> Option<&SongSave> {
    saves.iter().fold(None, |best: Option<&SongSave>, s| match best {
        Some(b) if b.modified >= s.modified => Some(b),
        _ => Some(s),
    })
}

/// Every folder a path key is in, nearest first, as keys.
fn folders_above(k: &str) -> impl Iterator<Item = &str> {
    k.char_indices().rev().filter(|&(_, c)| c == '/').map(move |(i, _)| if i == 0 { "/" } else { &k[..i] })
}

/// The folder a path key is in.
fn parent_key(k: &str) -> &str {
    folders_above(k).next().unwrap_or("")
}

/// Whether a path key is inside (or is) a folder key.
fn is_inside(path: &str, folder: &str) -> bool {
    folder != "/"
        && !folder.is_empty()
        && path.strip_prefix(folder).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Where a file goes on the backup drive: the backup folder, then the drive letter as a folder, then the same path
/// (`I:\Fl Studio Data\x.flp` goes to `<backup>\I\Fl Studio Data\x.flp`). A network path goes under `Network\server`.
/// `None` for a path that isn't absolute.
pub fn place_on_backup(backup: &Path, path: &Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};
    let mut out = backup.to_path_buf();
    let mut rooted = false;
    // On Windows a path is only whole with its drive (`\x` is "on whichever drive is current").
    let mut drive = !cfg!(windows);
    for c in path.components() {
        match c {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                    drive = true;
                    out.push((d as char).to_ascii_uppercase().to_string());
                }
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    drive = true;
                    out.push("Network");
                    out.push(server);
                    out.push(share);
                }
                _ => return None,
            },
            Component::RootDir => rooted = true,
            Component::Normal(n) => out.push(n),
            // A backup never climbs out of its folder.
            Component::CurDir | Component::ParentDir => return None,
        }
    }
    (rooted && drive).then_some(out)
}
