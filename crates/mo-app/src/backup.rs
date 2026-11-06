//! The Backup page (Solomon liked the mock-up, 1 Nov 2026; a page of its own in the rail): the backup drive and its
//! last run, How much (Full, Slim, Essentials), the format (plain folders or one zip per song), the list of folders
//! (each whole or slimmed), and what goes and what's left out, and why.
//!
//! Three parts, none of them touching a window:
//! - [`Settings`]: your choices, kept in `backup.txt` in the app's folder (a file of its own, so the C# app's
//!   databases stay read-only).
//! - [`prepare`]: walks the folders, reads which songs and samples are in them from the library, and makes the plan.
//!   Slow: run it on the app's low-priority pool.
//! - [`run`]: copies the plan (or packs each song), then [`view`] turns it all into the lines the page shows.
//!
//! Your music folders are only ever read; the backup drive is only written to by `mo-backup`, with its checks.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chrono::{Local, NaiveDateTime};
use mo_backup::copy::{self, Outcome};
use mo_backup::folders::{self, Folder, Mode};
use mo_backup::pack::{Packed, Packer, lists, owners, pack_names};
use mo_backup::plan::{self, Folders, LeftOut, Level, Plan, Rules, SongInfo, Used};
use mo_formats::case;
use mo_library::Library;
use mo_library::songs::{Song, file_id, group_songs, marked};
use mo_space::size::{FileEntry, size_text};
use mo_space::song::{CompactSettings, DEFAULT_KEEP_BACKUPS, SongSave, SongSpaceReport};
use rayon::prelude::*;

use crate::song_room::{Others, song_saves};

/// The settings file's name, in the app's folder.
pub const SETTINGS_FILE: &str = "backup.txt";

/// The ending of a settings file's temporary name, renamed once it's whole, so a crash part way leaves the old one
/// whole. [`crate::app_settings::write_checked`] uses `<name>.mo-part`; [`write_whole`] uses one of each save's own,
/// `<name>.<process>-<count>.mo-part` ([`is_whole_part_of`]), which the next save of the same file clears. The start-up
/// sweep (rule 28) clears what a crash leaves of each file on [`WRITTEN_WHOLE`] and
/// [`crate::app_settings::WRITTEN_CHECKED`], by the name its writer gives it, since some (a data move's) may never be
/// saved again.
pub const PART_ENDING: &str = ".mo-part";

/// Every file written through [`write_whole`] in the data folder: the Backup page's two, a data folder move's notes in
/// the folder it moved to ([`crate::data_move`]), and the Plugins page's moves under way
/// ([`crate::plugins::moves_under_way`]). A new caller adds its file's name here (or to
/// [`WRITTEN_WHOLE_BESIDE_FIRST`]), so the start-up sweep clears what a crash leaves of it.
pub const WRITTEN_WHOLE: [&str; 5] = [
    SETTINGS_FILE,
    SONGS_FILE,
    crate::paths::MOVED_IN_FILE,
    crate::data_move::MOVING_IN_FILE,
    crate::plugins::moves_under_way::FILE_NAME,
];

/// Every file written through [`write_whole`] beside the first data folder (`%LOCALAPPDATA%`): a data folder move's
/// pointer and its "ready to switch" file.
pub const WRITTEN_WHOLE_BESIDE_FIRST: [&str; 2] = [crate::paths::POINTER_FILE, crate::data_move::PENDING_FILE];

/// The two formats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// Plain folders you can open without the app; only what changed is copied.
    #[default]
    Folders,
    /// One zip per song, holding every sample it uses. Whole folders, and files that are no song's (presets, MIDI,
    /// samples no song uses), still go as plain folders.
    Zips,
}

/// How the last run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastRun {
    pub when: NaiveDateTime,
    /// Everything went and was checked.
    pub ok: bool,
    /// One line on what happened.
    pub line: String,
}

/// Your choices on the backup screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub drive: Option<PathBuf>,
    pub level: Level,
    pub format: Format,
    pub folders: Vec<Folder>,
    pub last_run: Option<LastRun>,
}

impl Settings {
    /// Reads `backup.txt`; none yet (or one that can't be read) gives the defaults. A line that doesn't make sense
    /// is skipped.
    pub fn load(path: &Path) -> Settings {
        let Ok(text) = fs::read_to_string(path) else { return Settings::default() };
        let mut s = Settings::default();
        let mut folder_lines = String::new();
        for line in text.lines() {
            let mut parts = line.splitn(2, '\t');
            let (Some(key), Some(value)) = (parts.next(), parts.next()) else { continue };
            match key {
                "drive" if !value.is_empty() => s.drive = Some(PathBuf::from(value)),
                "level" => {
                    s.level = match value {
                        "full" => Level::Full,
                        "essentials" => Level::Essentials,
                        _ => Level::Slim,
                    }
                }
                "format" => s.format = if value == "zips" { Format::Zips } else { Format::Folders },
                "folder" => {
                    folder_lines.push_str(value);
                    folder_lines.push('\n');
                }
                "last" => {
                    let mut p = value.splitn(3, '\t');
                    if let (Some(when), Some(ok), Some(line)) = (p.next(), p.next(), p.next())
                        && let Ok(when) = NaiveDateTime::parse_from_str(when, "%Y-%m-%d %H:%M:%S")
                    {
                        s.last_run = Some(LastRun { when, ok: ok == "1", line: line.to_owned() });
                    }
                }
                _ => {}
            }
        }
        s.folders = folders::from_text(&folder_lines);
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(d) = &self.drive {
            out.push_str(&format!("drive\t{}\n", d.display()));
        }
        let level = match self.level {
            Level::Full => "full",
            Level::Slim => "slim",
            Level::Essentials => "essentials",
        };
        out.push_str(&format!("level\t{level}\n"));
        out.push_str(if self.format == Format::Zips { "format\tzips\n" } else { "format\tfolders\n" });
        for line in folders::to_text(&self.folders).lines() {
            out.push_str(&format!("folder\t{line}\n"));
        }
        if let Some(r) = &self.last_run {
            let line = r.line.replace(['\t', '\n', '\r'], " ");
            out.push_str(&format!("last\t{}\t{}\t{line}\n", r.when.format("%Y-%m-%d %H:%M:%S"), u8::from(r.ok)));
        }
        out
    }

    /// Writes `backup.txt` the safe way ([`write_whole`]), so a crash or power cut part way leaves the old one whole.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_whole(path, &self.to_text())
    }

    /// Adds a folder (slimmed at first). One already on the list isn't added twice.
    pub fn add_folder(&mut self, path: PathBuf) {
        let k = case::key(&path.to_string_lossy());
        if !self.folders.iter().any(|f| case::key(&f.path.to_string_lossy()) == k) {
            self.folders.push(Folder { path, mode: Mode::Slimmed });
        }
    }

    /// Takes a folder off the list (nothing is deleted anywhere).
    pub fn remove_folder(&mut self, index: usize) {
        if index < self.folders.len() {
            self.folders.remove(index);
        }
    }

    pub fn set_mode(&mut self, index: usize, mode: Mode) {
        if let Some(f) = self.folders.get_mut(index) {
            f.mode = mode;
        }
    }
}

/// Writes a file whole or not at all: the text goes to a temporary name of this save's own beside it (two copies of
/// the app saving at once never share one), is flushed to the disk, then renamed over the old file. A crash or power
/// cut part way leaves the old file whole, and the temporary one never stays behind on an error.
///
/// Temporary files of the same file that a crash left (ten minutes old or more: a save takes far less, even on a busy
/// disk, so another copy of the app's live one is left alone) are cleared first,
/// so they never pile up.
///
/// Its file's name must be on [`WRITTEN_WHOLE`] or [`WRITTEN_WHOLE_BESIDE_FIRST`] (rule 28).
pub fn write_whole(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    clear_left_parts(path, &name);
    let mut tries = 0;
    let (mut f, part) = loop {
        tries += 1;
        let n = SAVES.fetch_add(1, Ordering::Relaxed);
        let part = path.with_file_name(part_name(&name, std::process::id(), n));
        match fs::File::create_new(&part) {
            Ok(f) => break (f, part),
            // Left by an earlier app with the same process number: the next name.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 100 => {}
            Err(e) => return Err(e),
        }
    };
    let written = (|| {
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&part, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&part);
    }
    written
}

/// The temporary name [`write_whole`] gives the file named `name` in the app with process id `process`, for its
/// save number `count`: `<name>.<process>-<count>.mo-part`.
fn part_name(name: &str, process: u32, count: u64) -> String {
    format!("{name}.{process}-{count}{PART_ENDING}")
}

/// Whether `name` is a temporary name [`write_whole`] gives the file named `file`: `<file>.<process>-<count>.mo-part`,
/// the process id and the count both digits only ([`part_name`]). Nothing else: not `<file>.mo-part`, nor
/// `<file>.1-2.mo-part.bak`, nor `<file>.old.mo-part`.
pub(crate) fn is_whole_part_of(name: &str, file: &str) -> bool {
    name.strip_prefix(file)
        .and_then(|rest| rest.strip_suffix(PART_ENDING))
        .and_then(|middle| middle.strip_prefix('.'))
        .and_then(|middle| middle.split_once('-'))
        .is_some_and(|(process, count)| crate::tidy::is_number(process) && crate::tidy::is_number(count))
}

/// Removes temporary files of `path` ([`is_whole_part_of`], or `<name>.mo-part`, as older versions wrote these) ten
/// minutes old or more.
fn clear_left_parts(path: &Path, name: &str) {
    let Some(folder) = path.parent() else { return };
    let Ok(dir) = fs::read_dir(folder) else { return };
    for entry in dir.flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        if !(is_whole_part_of(&other, name) || other.strip_prefix(name) == Some(PART_ENDING)) {
            continue;
        }
        let old = entry.metadata().ok().filter(fs::Metadata::is_file).and_then(|m| m.modified().ok());
        if old.and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= std::time::Duration::from_secs(10 * 60)) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// A song's id on the backup: its first save's id, which stays the same as new saves come. Its zip is known by it,
/// and so is its line in [`BackedUp`].
pub fn song_id(song: &Song) -> u64 {
    let id = song.saves().chain(&song.copies).map(|v| v.id).min().unwrap_or(0);
    u64::try_from(id).unwrap_or(0)
}

/// The file that says when each song last reached the backup drive, in the app's folder.
pub const SONGS_FILE: &str = "backup-songs.txt";

/// When each song's newest save last reached the backup drive, for the song panel's "Backed up" line. Kept by the
/// app (not read from the drive), so the line shows while the drive is unplugged. A new drive starts it again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackedUp {
    /// The drive the songs went to.
    pub drive: Option<PathBuf>,
    /// By song id: the time of the newest save that went, and when it went.
    pub songs: HashMap<u64, (NaiveDateTime, NaiveDateTime)>,
}

const TIME: &str = "%Y-%m-%d %H:%M:%S%.f";

impl BackedUp {
    /// Reads `backup-songs.txt`; none yet (or one that can't be read) is empty. A line that doesn't make sense is
    /// skipped.
    pub fn load(path: &Path) -> BackedUp {
        let Ok(text) = fs::read_to_string(path) else { return BackedUp::default() };
        let mut b = BackedUp::default();
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("drive"), Some(d), None) if !d.is_empty() => b.drive = Some(PathBuf::from(d)),
                (Some(id), Some(save), Some(when)) => {
                    if let (Ok(id), Ok(save), Ok(when)) = (
                        id.parse(),
                        NaiveDateTime::parse_from_str(save, TIME),
                        NaiveDateTime::parse_from_str(when, TIME),
                    ) {
                        b.songs.insert(id, (save, when));
                    }
                }
                _ => {}
            }
        }
        b
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(d) = &self.drive {
            out.push_str(&format!("drive\t{}\n", d.display()));
        }
        let mut ids: Vec<&u64> = self.songs.keys().collect();
        ids.sort();
        for id in ids {
            let (save, when) = self.songs[id];
            out.push_str(&format!("{id}\t{}\t{}\n", save.format(TIME), when.format(TIME)));
        }
        out
    }

    /// Writes `backup-songs.txt` the safe way ([`write_whole`]).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_whole(path, &self.to_text())
    }

    /// Notes the songs a run on `drive` took. Songs it didn't reach keep their old lines; a different drive starts
    /// the list again, since what went to the old one isn't on the new one.
    pub fn add(&mut self, drive: &Path, went: &[(u64, NaiveDateTime)], when: NaiveDateTime) {
        if !self.on(Some(drive)) {
            self.songs.clear();
            self.drive = Some(drive.to_path_buf());
        }
        for (id, save) in went {
            self.songs.insert(*id, (*save, when));
        }
    }

    /// Whether the list is for this drive.
    fn on(&self, drive: Option<&Path>) -> bool {
        match (&self.drive, drive) {
            (Some(a), Some(b)) => case::key(&a.to_string_lossy()) == case::key(&b.to_string_lossy()),
            _ => false,
        }
    }

    /// The song panel's line: "Backed up today 23:40", "Newest save not backed up yet · last backed up 12 Oct
    /// 23:40" or "Not backed up yet". Empty when no backup drive is picked, so the panel stays quiet until you set
    /// one up. `id` is the song's ([`song_id`]) and `newest` the time of its newest save.
    pub fn song_line(&self, drive: Option<&Path>, id: u64, newest: NaiveDateTime, now: NaiveDateTime) -> String {
        if drive.is_none() {
            return String::new();
        }
        match self.songs.get(&id).filter(|_| self.on(drive)) {
            None => "Not backed up yet".to_owned(),
            Some((save, when)) if *save >= newest => format!("Backed up {}", when_text(*when, now)),
            Some((_, when)) => format!("Newest save not backed up yet · last backed up {}", when_text(*when, now)),
        }
    }
}

/// One song's files, for a zip of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SongFiles {
    pub title: String,
    pub id: u64,
    pub files: Vec<PathBuf>,
}

/// What [`prepare`] found.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub plan: Plan,
    /// The songs in the folders, with the files of each that go (for the zip format).
    pub songs: Vec<SongFiles>,
    /// Folders on the list that aren't there now.
    pub missing: Vec<PathBuf>,
    /// Everything in the folders (and the samples outside them the songs use), before anything is left out.
    pub all_bytes: u64,
    /// Each folder on the list's size, in the list's order; `None` for one that isn't there.
    pub folder_bytes: Vec<Option<u64>>,
    /// The folder list and level it was made for: a plan for other choices isn't shown.
    pub made_for: (Vec<Folder>, Level),
    /// Each song's id and newest save (its place and time), for the song panel's "Backed up" line.
    pub newest: Vec<(u64, PathBuf, NaiveDateTime)>,
}

/// A sample path as a project stores it, made whole: a full path as it is, a relative one from the save's folder.
/// Just a file name can't be placed: `None`.
fn sample_file(sample: &str, save_folder: &str) -> Option<PathBuf> {
    let p = Path::new(sample);
    if p.is_absolute() || sample.get(1..3) == Some(":\\") || sample.starts_with("\\\\") {
        Some(p.to_path_buf())
    } else if sample.contains(['/', '\\']) {
        Some(Path::new(save_folder).join(sample))
    } else {
        None
    }
}

/// The save `song` opens, among the saves measured: its own (`own`), or when only copies of its saves are in the folders,
/// the copy of that save (none when the library can't tell which copy it is).
fn opens_of(song: &Song, own: bool) -> Option<&str> {
    if own {
        return Some(&song.latest.path);
    }
    let id = file_id(&song.latest)?;
    song.copies.iter().find(|c| file_id(c).is_some_and(|c| c == id)).map(|c| c.path.as_str())
}

/// What the library knows of each save's size, for finding big changes. A save the reader couldn't read says
/// nothing (its counts would read as everything taken away). Bars are counted as four beats.
fn shapes(saves: &[&mo_library::Save]) -> Vec<plan::SaveShape> {
    saves
        .iter()
        .filter(|v| v.error.is_none())
        .map(|v| plan::SaveShape {
            path: v.path.clone(),
            modified: v.modified,
            tracks: u32::try_from(v.track_count).ok(),
            plugins: v.plugin_count,
            samples: v.sample_count,
            length_beats: (v.length_bars > 0).then(|| f64::from(v.length_bars) * 4.0),
        })
        .collect()
}

/// Walks the folders on the list, finds the songs saved in them and the samples they use (wherever those are), and
/// makes the plan. Reads only. Slow: on the app's low-priority pool, never the window's thread.
pub fn prepare(library: &Library, settings: &Settings, stop: &AtomicBool) -> Result<Prepared, String> {
    let backup = settings.drive.clone().ok_or("Pick a backup drive first")?;
    let gathered = folders::gather(&settings.folders, &backup, stop).map_err(|_| "Stopped".to_owned())?;
    let saves = library.saves().map_err(|e| e.to_string())?;
    let choices = library.song_choices().map_err(|e| e.to_string())?;
    let samples = library.sample_paths().map_err(|e| e.to_string())?;
    let roots = library.roots().map_err(|e| e.to_string())?;
    let sample_folders = library.list_setting("sample_folders").map_err(|e| e.to_string())?;

    // Each folder's size, in one pass over the files, matching folders as the walk did.
    let mut folder_bytes: Vec<Option<u64>> =
        settings.folders.iter().map(|f| (!gathered.missing.contains(&f.path)).then_some(0)).collect();
    for e in &gathered.files {
        for (f, bytes) in settings.folders.iter().zip(folder_bytes.iter_mut()) {
            if let Some(b) = bytes
                && folders::within(&e.path, &f.path)
            {
                *b = b.saturating_add(e.size);
            }
        }
    }
    let mut files = gathered.files;
    let have: HashSet<String> = files.iter().map(|f| case::key(&f.path.to_string_lossy())).collect();
    let all_songs = group_songs(&saves, &choices);
    let others = Others::new(&all_songs, &samples, sample_folders);
    let compact = CompactSettings {
        // Essentials leaves every autosave out: all of them go on the Compact list.
        keep_backups: if settings.level == Level::Essentials { 0 } else { DEFAULT_KEEP_BACKUPS },
        ..CompactSettings::default()
    };
    // Only songs with a save in the folders are this backup's (a song there only as a copy, too).
    let songs: Vec<_> = all_songs
        .into_iter()
        .filter(|s| s.saves().chain(&s.copies).any(|v| have.contains(&case::key(&v.path))))
        .collect();

    struct Measured {
        saves: Vec<SongSave>,
        report: SongSpaceReport,
        milestones: Vec<String>,
        used: Vec<PathBuf>,
        id: u64,
    }
    let mut measured: Vec<Measured> = songs
        .par_iter()
        .map(|song| {
            // The song's own saves, or (when only copies of them are in the folders) the copies.
            let own = song.saves().any(|v| have.contains(&case::key(&v.path)));
            let picked: Vec<&mo_library::Save> =
                if own { song.saves().collect() } else { song.copies.iter().collect() };
            let saves = song_saves(picked.iter().copied());
            let opens = opens_of(song, own);
            let mut used: Vec<PathBuf> = Vec::new();
            let mut used_text: Vec<String> = Vec::new();
            for v in song.saves().chain(&song.copies) {
                for s in samples.get(&v.id).into_iter().flatten() {
                    used_text.push(s.clone());
                    if let Some(p) = sample_file(s, &v.folder) {
                        used.push(p);
                    }
                }
            }
            let report = others.measure(&saves, opens, &roots, &used_text, compact, stop).map_err(|_| ())?;
            // Saves you mark as milestones by their name always go, and so do saves where the song changed a lot, and
            // the save the song opens (its highest-numbered one: the newest by time, which always goes, can be another).
            let mut milestones: Vec<String> =
                picked.iter().filter(|v| marked(&v.name)).map(|v| v.path.clone()).collect();
            milestones.extend(plan::big_changes(&shapes(&picked)));
            milestones.extend(opens.map(str::to_owned));
            Ok(Measured { saves, report, milestones, used, id: song_id(song) })
        })
        .collect::<Result<_, ()>>()
        .map_err(|()| "Stopped".to_owned())?;

    // The samples the songs use, wherever they are: one outside the folders is added, so a song's backup is whole.
    // Each is looked up once (a sample on a drive that isn't there is asked about once, not once per save that names
    // it). One reached through a link (a folder Space moved, a project still naming its old place) that is already
    // in the folders where it really is goes once, from there: the song's list names that place instead.
    // Keyed as mo-space keys paths, so a real place (with no `\\?\` in front on Windows) matches the walk's spelling.
    let mut by_key: HashMap<String, usize> =
        files.iter().enumerate().map(|(i, f)| (mo_space::key(&f.path.to_string_lossy()), i)).collect();
    let mut not_there: HashSet<String> = HashSet::new();
    // A listed folder that is a link (walked under its listed spelling): where it really is, so a project naming
    // that real place finds the walked file.
    let linked: Vec<(String, String)> = settings
        .folders
        .iter()
        .filter_map(|f| {
            let listed = mo_space::key(&f.path.to_string_lossy());
            let real = mo_space::key(&mo_space::real_path(&f.path.to_string_lossy())?);
            (real != listed).then_some((real, listed))
        })
        .collect();
    let through_listed = |k: &str| {
        linked.iter().find_map(|(real, listed)| {
            let rest = k.strip_prefix(real.as_str())?;
            rest.starts_with('/').then(|| format!("{listed}{rest}"))
        })
    };
    for m in &mut measured {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Stopped".to_owned());
        }
        let mut seen: HashSet<String> = HashSet::new();
        m.used.retain(|p| seen.insert(mo_space::key(&p.to_string_lossy())));
        for p in &mut m.used {
            let k = mo_space::key(&p.to_string_lossy());
            // Named as the plan names it, so the song's own list finds it (whatever slashes the project used).
            if let Some(&i) = by_key.get(&k).or_else(|| through_listed(&k).and_then(|l| by_key.get(&l))) {
                *p = files[i].path.clone();
                continue;
            }
            if not_there.contains(&k) {
                continue;
            }
            let Some(meta) = fs::metadata(&*p).ok().filter(fs::Metadata::is_file) else {
                not_there.insert(k);
                continue;
            };
            let real = mo_space::real_path(&p.to_string_lossy()).map(|r| mo_space::key(&r));
            if let Some(&i) = real.as_ref().and_then(|r| by_key.get(r)) {
                *p = files[i].path.clone();
                continue;
            }
            // Known by where it really is too, so another save naming that place finds this one.
            if let Some(real) = real {
                by_key.insert(real, files.len());
            }
            by_key.insert(k, files.len());
            files.push(FileEntry {
                path: p.clone(),
                size: meta.len(),
                modified: meta.modified().map(|t| chrono::DateTime::<Local>::from(t).naive_local()).unwrap_or_default(),
            });
        }
    }
    let all_bytes = files.iter().map(|f| f.size).fold(0, u64::saturating_add);
    let used = Used::new(samples.values().flatten().map(String::as_str));
    let infos: Vec<SongInfo<'_>> =
        measured.iter().map(|m| SongInfo { report: &m.report, saves: &m.saves, milestones: &m.milestones }).collect();
    let rules = Rules { level: settings.level, ..Rules::default() };
    // A file follows the nearest folder on the list it's in (a Slimmed one inside a Whole one keeps its choice).
    let listed = plan::Listed::new(&settings.folders);
    let plan = plan::plan(files, &infos, &used, Folders { backup: &backup, listed: &listed }, rules);

    // Each song's files that go: its saves, the samples it uses, and (when it has a folder to itself) the rest of it.
    // Files in whole folders stay out of the zips: they go as plain folders.
    let going: HashMap<String, &FileEntry> =
        plan.copy.iter().map(|f| (case::key(&f.path.to_string_lossy()), f)).collect();
    // The files in each song's own folder, found in one pass over the plan: each file's folders, looked up.
    let own_folders: HashMap<String, usize> = measured
        .iter()
        .enumerate()
        .filter(|(_, m)| m.report.own_folder)
        .map(|(i, m)| (case::key(m.report.folder.trim_end_matches(['\\', '/'])), i))
        .collect();
    let mut in_own: Vec<Vec<&Path>> = vec![Vec::new(); measured.len()];
    if !own_folders.is_empty() {
        for f in &plan.copy {
            if let Some(i) = f.path.ancestors().skip(1).find_map(|a| own_folders.get(&case::key(&a.to_string_lossy())))
            {
                in_own[*i].push(&f.path);
            }
        }
    }
    // Picked as the plan picks it (the first of two saved at the same moment), so the song's line follows the plan.
    let newest = measured
        .iter()
        .filter_map(|m| plan::newest_save(&m.saves).map(|s| (m.id, PathBuf::from(&s.path), s.modified)))
        .collect();
    let song_files = songs
        .iter()
        .zip(&measured)
        .zip(&in_own)
        .map(|((song, m), own)| {
            let mut list: Vec<PathBuf> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            let mut take = |p: &Path| {
                let k = case::key(&p.to_string_lossy());
                if let Some(f) = going.get(&k)
                    && seen.insert(k)
                    && !listed.whole(&f.path)
                {
                    list.push(f.path.clone());
                }
            };
            for s in &m.saves {
                take(Path::new(&s.path));
            }
            for p in &m.used {
                take(p);
            }
            for p in own {
                take(p);
            }
            SongFiles { title: song.name.clone(), id: m.id, files: list }
        })
        .filter(|s| !s.files.is_empty())
        .collect();
    Ok(Prepared {
        plan,
        songs: song_files,
        missing: gathered.missing,
        all_bytes,
        folder_bytes,
        made_for: (settings.folders.clone(), settings.level),
        newest,
    })
}

/// Where a run is, for the page's progress line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Looking,
    Copying { files: usize, bytes: u64 },
    Packing { done: usize, of: usize },
}

impl Progress {
    /// How far along, 0 to 1, when that's known.
    pub fn done(&self) -> f32 {
        match self {
            Progress::Packing { done, of } if *of > 0 => (*done as f64 / *of as f64) as f32,
            _ => 0.0,
        }
    }

    pub fn line(&self) -> String {
        match self {
            Progress::Looking => "Looking through your folders and songs…".to_owned(),
            Progress::Copying { files, bytes } => {
                format!(
                    "Backing up {} files ({}), each checked · you can keep working",
                    thousands(*files),
                    size_text(*bytes)
                )
            }
            Progress::Packing { done, of } => format!("Packing songs: {} of {}", thousands(*done), thousands(*of)),
        }
    }
}

/// How a run went, and the songs whose newest save reached the drive in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    pub last: LastRun,
    /// Each song's id and the time of its newest save that went (or was already there and checked).
    pub went: Vec<(u64, NaiveDateTime)>,
    /// How many replaced files were kept in Older versions (the Older versions card is read again only when some were).
    pub kept_older: usize,
}

impl Ran {
    /// A run that broke part way, at `when`: nothing counts as gone, so the next run checks everything again.
    pub fn broke(when: NaiveDateTime) -> Ran {
        let line = "The backup stopped part way on something it didn't expect (the log says what). The next run checks it all again."
            .to_owned();
        Ran { last: LastRun { when, ok: false, line }, went: Vec::new(), kept_older: 0 }
    }
}

/// Copies the plan to the drive (or, for the zip format, packs each song and copies the whole folders), and says how
/// it went. Never on the window's thread. A run that stops part way still says which songs went before it stopped.
pub fn run(prepared: &Prepared, settings: &Settings, stop: &AtomicBool, progress: &dyn Fn(Progress)) -> Ran {
    let mut there: HashSet<String> = HashSet::new();
    let mut kept_older = 0;
    let last = run_on(prepared, settings, stop, progress, &mut there, &mut kept_older);
    let went = prepared
        .newest
        .iter()
        .filter(|(_, path, _)| there.contains(&case::key(&path.to_string_lossy())))
        .map(|(id, _, time)| (*id, *time))
        .collect();
    Ran { last, went, kept_older }
}

/// The most of a run's problems the log names one by one.
const LOGGED_PROBLEMS: usize = 50;

/// The run itself; `there` gathers the files now safely on the drive, and `kept_older` counts the replaced files kept
/// in Older versions.
fn run_on(
    prepared: &Prepared,
    settings: &Settings,
    stop: &AtomicBool,
    progress: &dyn Fn(Progress),
    there: &mut HashSet<String>,
    kept_older: &mut usize,
) -> LastRun {
    let when = Local::now().naive_local();
    let Some(backup) = settings.drive.as_deref() else {
        return LastRun { when, ok: false, line: "No backup drive picked".to_owned() };
    };
    // This run's older versions go in a folder named after the minute it began, as the Older versions card reads them.
    let options = copy::Options::new(mo_backup::older::stamp(when));
    // Borrowed for plain folders (rule 24: borrow, don't copy); only the zip format's shorter list is its own.
    let copy_list: std::borrow::Cow<'_, [FileEntry]> = match settings.format {
        Format::Folders => std::borrow::Cow::Borrowed(&prepared.plan.copy),
        // Everything that isn't in a song's zip still goes, as plain folders: whole folders, and files no song owns
        // (presets, MIDI packs, samples no song in the folders uses).
        Format::Zips => {
            let zipped: HashSet<String> =
                prepared.songs.iter().flat_map(|s| &s.files).map(|p| case::key(&p.to_string_lossy())).collect();
            std::borrow::Cow::Owned(
                prepared
                    .plan
                    .copy
                    .iter()
                    .filter(|f| !zipped.contains(&case::key(&f.path.to_string_lossy())))
                    .cloned()
                    .collect(),
            )
        }
    };
    let mut tally = Tally { there, kept_older, checked: 0, new_or_changed: 0, problems: Vec::new(), record_note: None };
    // Files gone from your folders that are damaged in a song's zip.
    let mut damaged = 0usize;
    let mut plain_songs = 0usize;
    if !copy_list.is_empty()
        && let Some(end) = tally.copy(&copy_list, backup, &options, stop, progress)
    {
        return LastRun { when, ok: false, line: end };
    }
    if settings.format == Format::Zips {
        let Ok(packer) = Packer::new(backup) else {
            return LastRun { when, ok: false, line: "The backup drive isn't there".to_owned() };
        };
        // The zips already there, so no song takes the name of another's (one gone from your folders, say).
        let on_backup = match lists(backup, stop) {
            Ok(found) => {
                for zip in found.iter().filter(|l| l.song.is_none()) {
                    crate::log::warning(
                        "Backup",
                        &format!(
                            "{} isn't one of the app's song zips, or is damaged: it's left as it is, and no song is \
                             packed in its place",
                            zip.zip.display()
                        ),
                    );
                }
                owners(&found)
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                return LastRun { when, ok: false, line: format!("Stopped · {} checked", thousands(tally.checked)) };
            }
            Err(e) => {
                // Each zip is still only packed over when it's the song's own.
                crate::log::warning("Backup", &format!("Couldn't read the song zips already on the backup: {e}"));
                HashMap::new()
            }
        };
        // Songs whose zip couldn't be made: their files go as plain folders instead, so none is left out of the run.
        let mut unzipped: Vec<&SongFiles> = Vec::new();
        let titles: Vec<(&str, u64)> = prepared.songs.iter().map(|s| (s.title.as_str(), s.id)).collect();
        let names = pack_names(&titles, &on_backup);
        for (i, (song, name)) in prepared.songs.iter().zip(&names).enumerate() {
            progress(Progress::Packing { done: i, of: prepared.songs.len() });
            let packed = packer.pack_song(&song.files, name, song.id, stop);
            if packed.is_ok() {
                tally.there.extend(song.files.iter().map(|p| case::key(&p.to_string_lossy())));
            }
            match packed {
                Ok(Packed::Written { damaged: found, .. }) => {
                    tally.new_or_changed += 1;
                    tally.checked += 1;
                    for d in &found {
                        let what = match d.kept {
                            true => "is damaged in the song's zip: it's kept as it was, but may not open",
                            false => "was on the song's zip's list but isn't in the zip, so there's nothing to keep",
                        };
                        crate::log::warning(
                            "Backup",
                            &format!("{}: {}, gone from your folders, {what}", song.title, d.inside),
                        );
                    }
                    damaged += found.len();
                }
                Ok(Packed::UpToDate { .. }) => tally.checked += 1,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    return LastRun {
                        when,
                        ok: false,
                        line: format!("Stopped · {} checked", thousands(tally.checked)),
                    };
                }
                Err(e) if e.kind() == io::ErrorKind::StorageFull => {
                    return LastRun { when, ok: false, line: "The backup drive is full".to_owned() };
                }
                Err(_) if !backup.is_dir() => {
                    return LastRun { when, ok: false, line: "The backup drive went away part way".to_owned() };
                }
                Err(e) => {
                    crate::log::warning(
                        "Backup",
                        &format!("{}: its zip couldn't be made ({e}), so its files go as plain folders", song.title),
                    );
                    unzipped.push(song);
                }
            }
        }
        if !unzipped.is_empty() {
            // Each file of theirs still in your folders, as the plan has it; one renamed or deleted since the plan
            // was made can't go, and says so.
            let wanted: HashSet<String> =
                unzipped.iter().flat_map(|s| &s.files).map(|p| case::key(&p.to_string_lossy())).collect();
            let (list, vanished): (Vec<FileEntry>, Vec<FileEntry>) = prepared
                .plan
                .copy
                .iter()
                .filter(|f| wanted.contains(&case::key(&f.path.to_string_lossy())))
                .cloned()
                .partition(|f| f.path.is_file());
            for f in &vanished {
                tally.problems.push(format!("{}: not there now (renamed or deleted since the plan)", f.path.display()));
            }
            plain_songs = unzipped.len();
            if !list.is_empty()
                && let Some(end) = tally.copy(&list, backup, &options, stop, progress)
            {
                return LastRun { when, ok: false, line: end };
            }
        }
    }
    let Tally { checked, new_or_changed, problems, record_note, .. } = tally;
    let mut line = format!("{} checked · {} new or changed", thousands(checked), thousands(new_or_changed));
    if plain_songs > 0 {
        line.push_str(&format!(
            " · {} went as plain folders (their zips couldn't be made: see the log)",
            crate::songs::plural(plain_songs, "song")
        ));
    }
    if !problems.is_empty() {
        line.push_str(&format!(" · {} couldn't go", thousands(problems.len())));
        // Why, in the app's log.
        for why in problems.iter().take(LOGGED_PROBLEMS) {
            crate::log::warning("Backup", &format!("Couldn't go: {why}"));
        }
        if problems.len() > LOGGED_PROBLEMS {
            crate::log::warning("Backup", &format!("…and {} more couldn't go", problems.len() - LOGGED_PROBLEMS));
        }
    }
    if damaged > 0 {
        let verb = if damaged == 1 { "is" } else { "are" };
        line.push_str(&format!(
            " · {} gone from your folders {verb} damaged in a song zip",
            crate::songs::plural(damaged, "file")
        ));
    }
    if !prepared.missing.is_empty() {
        line.push_str(&format!(" · {} folders not plugged in", prepared.missing.len()));
    }
    if record_note.is_some() {
        // Everything was still copied and checked; the next run just reads every file again.
        line.push_str(" · the backup's own list couldn't be saved, so next time takes longer");
    }
    // A song that went as plain folders is safe, but not as you asked: the run isn't all right.
    LastRun { when, ok: problems.is_empty() && prepared.missing.is_empty() && damaged == 0 && plain_songs == 0, line }
}

/// What a run's copying has done so far.
struct Tally<'a> {
    /// The files now safely on the drive.
    there: &'a mut HashSet<String>,
    /// Replaced files kept in Older versions.
    kept_older: &'a mut usize,
    checked: usize,
    new_or_changed: usize,
    problems: Vec<String>,
    /// Why the backup's record couldn't be saved.
    record_note: Option<String>,
}

impl Tally<'_> {
    /// Copies a list as plain folders and counts what happened. Gives the line to end the run with when it stopped,
    /// or the drive filled up or went away.
    fn copy(
        &mut self,
        list: &[FileEntry],
        backup: &Path,
        options: &copy::Options,
        stop: &AtomicBool,
        progress: &dyn Fn(Progress),
    ) -> Option<String> {
        let bytes = list.iter().map(|f| f.size).fold(0, u64::saturating_add);
        progress(Progress::Copying { files: list.len(), bytes });
        let Ok(report) = copy::run(list, backup, options, stop) else {
            return Some("The backup drive isn't there".to_owned());
        };
        for l in &report.lines {
            if l.outcome == (Outcome::Updated { kept_older: true }) {
                *self.kept_older += 1;
            }
            match &l.outcome {
                Outcome::New | Outcome::Updated { .. } => {
                    self.new_or_changed += 1;
                    self.checked += 1;
                    self.there.insert(case::key(&l.source.to_string_lossy()));
                }
                Outcome::UpToDate => {
                    self.checked += 1;
                    self.there.insert(case::key(&l.source.to_string_lossy()));
                }
                Outcome::Failed(why) => self.problems.push(format!("{}: {why}", l.source.display())),
                Outcome::NotReached => {}
            }
        }
        if report.stopped {
            return Some(format!("Stopped · {} files checked", thousands(self.checked)));
        }
        if report.full {
            return Some("The backup drive is full".to_owned());
        }
        if report.gone {
            return Some("The backup drive went away part way".to_owned());
        }
        // The latest copy's word on the record is the one that counts: it holds everything before it too.
        self.record_note = report.record_not_saved;
        None
    }
}

/// Your DAWs' project folders (the library's roots) and your sample folders, for "Add my DAW and sample folders".
pub fn library_folders(library: &Library) -> Result<Vec<PathBuf>, mo_library::LibraryError> {
    let mut out: Vec<PathBuf> = library.roots()?.into_iter().map(PathBuf::from).collect();
    out.extend(library.list_setting("sample_folders")?.into_iter().map(PathBuf::from));
    Ok(out)
}

/// Where working out what goes has got to.
#[derive(Clone, Copy, Debug)]
pub enum PlanState<'a> {
    /// Nothing to work out (no drive or no folders yet).
    Waiting,
    Working,
    Ready(&'a Prepared),
    Failed(&'a str),
}

/// The backup drive's room, as the drive card shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriveRoom {
    pub free: u64,
    pub total: u64,
}

/// One folder's line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderLine {
    pub path: String,
    pub size: String,
    /// 0 Whole, 1 Slimmed: the order of the switch.
    pub mode: usize,
}

/// Everything the Backup section shows, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct PageLines {
    pub drive: String,
    pub drive_fill: f32,
    /// How full the backup drive is, by Space's rule (`space::fill_of`): 0 roomy, 1 nearly full, 2 almost full.
    pub drive_state: i32,
    pub drive_line: String,
    pub last_line: String,
    /// 0 Full, 1 Slim, 2 Essentials: the order of the switch.
    pub level: usize,
    pub level_note: String,
    /// 0 Plain folders, 1 Zip per song.
    pub format: usize,
    pub format_note: String,
    pub folders: Vec<FolderLine>,
    /// Empty until worked out; then `plan_note` isn't shown.
    pub goes: String,
    pub of_all: String,
    pub goes_part: f32,
    /// Why, the note under it, and its size.
    pub left_out: Vec<(String, String, String)>,
    pub plan_note: String,
}

/// The words a left-out reason is shown with on the card.
pub fn left_out_title(why: LeftOut) -> &'static str {
    match why {
        LeftOut::SameSave => "Same save twice",
        LeftOut::OldAutosave => "Old autosaves",
        LeftOut::Cache => "Caches",
        LeftOut::MidSittingSave => "Mid-sitting saves",
        LeftOut::OlderBigSave => "Older big saves",
        LeftOut::UnusedSound => "Big unused sounds",
        LeftOut::NotMilestone => "Not a milestone",
    }
}

/// Makes the section's words from the settings, the drive's room and the plan. `prepared` is only used when it was
/// made for the same folder list.
pub fn page_lines(settings: &Settings, room: Option<DriveRoom>, plan: PlanState<'_>, now: NaiveDateTime) -> PageLines {
    let ready = match plan {
        PlanState::Ready(p)
            if p.made_for.0 == settings.folders
                && p.made_for.1 == settings.level
                && p.folder_bytes.len() == settings.folders.len() =>
        {
            Some(p)
        }
        _ => None,
    };
    let folders = settings
        .folders
        .iter()
        .enumerate()
        .map(|(i, f)| FolderLine {
            path: f.path.display().to_string(),
            size: match ready.map(|p| p.folder_bytes[i]) {
                Some(Some(b)) => size_text(b),
                Some(None) => "Not plugged in".to_owned(),
                None => String::new(),
            },
            mode: usize::from(f.mode == Mode::Slimmed),
        })
        .collect();
    let (goes, of_all, goes_part, left_out) = match ready {
        Some(p) => {
            let goes = p.plan.copy_bytes();
            let part = if p.all_bytes == 0 { 0.0 } else { (goes as f64 / p.all_bytes as f64) as f32 };
            let lines = left_out_lines(&p.plan)
                .into_iter()
                .map(|l| {
                    let note = format!("{} · {}", l.why.note(), crate::song_panel::count_of(l.files, "file"));
                    (left_out_title(l.why).to_owned(), note, size_text(l.bytes))
                })
                .collect();
            (size_text(goes), size_text(p.all_bytes), part, lines)
        }
        None => (String::new(), String::new(), 0.0, Vec::new()),
    };
    let plan_note = match plan {
        _ if settings.folders.is_empty() => "Add a folder to see what goes".to_owned(),
        _ if settings.drive.is_none() => "Pick a backup drive to see what goes".to_owned(),
        PlanState::Failed(why) => format!("Couldn't work out what goes: {why}"),
        _ => "Working out what goes…".to_owned(),
    };
    let (drive_fill, drive_state, drive_line) = match room {
        Some(r) if r.total > 0 => (
            (r.total.saturating_sub(r.free) as f64 / r.total as f64) as f32,
            // The app keeps its own files on its own drive, not the backup drive.
            crate::space::fill_of(r.free, 0).index(),
            drive_line(r.free, r.total),
        ),
        _ => (0.0, 0, String::new()),
    };
    PageLines {
        drive: settings.drive.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
        drive_fill,
        drive_state,
        drive_line,
        last_line: last_run_line(settings.last_run.as_ref(), now),
        level: match settings.level {
            Level::Full => 0,
            Level::Slim => 1,
            Level::Essentials => 2,
        },
        level_note: level_note(settings.level).to_owned(),
        format: usize::from(settings.format == Format::Zips),
        format_note: format_note(settings.format).to_owned(),
        folders,
        goes,
        of_all,
        goes_part,
        left_out,
        plan_note,
    }
}

/// The level at a place on the How much switch.
pub fn level_at(i: usize) -> Level {
    match i {
        0 => Level::Full,
        2 => Level::Essentials,
        _ => Level::Slim,
    }
}

/// One line of "Left out, and why".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftOutLine {
    pub why: LeftOut,
    pub files: usize,
    pub bytes: u64,
}

/// The plan's left-out files by reason, biggest first.
pub fn left_out_lines(plan: &Plan) -> Vec<LeftOutLine> {
    let mut by: Vec<LeftOutLine> = Vec::new();
    for s in &plan.left_out {
        match by.iter_mut().find(|l| l.why == s.why) {
            Some(l) => {
                l.files += 1;
                l.bytes = l.bytes.saturating_add(s.file.size);
            }
            None => by.push(LeftOutLine { why: s.why, files: 1, bytes: s.file.size }),
        }
    }
    by.sort_by_key(|l| std::cmp::Reverse(l.bytes));
    by
}

/// "1,204".
pub fn thousands(n: usize) -> String {
    crate::songs::thousands(n)
}

/// The words under How much.
pub fn level_note(level: Level) -> &'static str {
    match level {
        Level::Full => "Everything in your folders, every save. The biggest backup.",
        Level::Slim => {
            "Leaves out big audio no project uses and big saves from the middle of a sitting. Every small save goes."
        }
        Level::Essentials => {
            "Only each song's milestones: the last save of each day, big changes and saves you mark. Plus every sample they use."
        }
    }
}

/// The words under Format.
pub fn format_note(format: Format) -> &'static str {
    match format {
        Format::Folders => {
            "Open it in Explorer without the app. Only new or changed files are copied, and the small files a backup replaces are kept as older versions."
        }
        Format::Zips => {
            "One zip per song in Songs, with every sample it uses. Easy to move or send. Whole folders, and files that aren't any song's, go as plain folders."
        }
    }
}

/// The drive card's second line: how full the drive is.
pub fn drive_line(free: u64, total: u64) -> String {
    format!("{} used of {}", size_text(total.saturating_sub(free)), size_text(total))
}

/// The drive card's last line.
pub fn last_run_line(last: Option<&LastRun>, now: NaiveDateTime) -> String {
    match last {
        None => "Not backed up yet".to_owned(),
        Some(r) => format!("Last backup {} · {}", when_text(r.when, now), r.line),
    }
}

/// "today 23:40", "yesterday 09:05", "Saturday 23:40", "12 Oct 23:40".
pub fn when_text(when: NaiveDateTime, now: NaiveDateTime) -> String {
    let days = (now.date() - when.date()).num_days();
    let time = when.format("%H:%M");
    match days {
        0 => format!("today {time}"),
        1 => format!("yesterday {time}"),
        2..=6 => format!("{} {time}", when.format("%A")),
        _ => format!("{} {time}", when.format("%-d %b")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_temporary_names_write_whole_gives_are_its_parts() {
        assert!(is_whole_part_of(&part_name(SETTINGS_FILE, 4120, 0), SETTINGS_FILE));
        assert!(is_whole_part_of(&part_name(SONGS_FILE, u32::MAX, u64::MAX), SONGS_FILE));
        assert!(is_whole_part_of("backup.txt.1-22.mo-part", SETTINGS_FILE));
        for not in [
            "backup.txt",
            "backup.txt.mo-part",
            "backup.txt.mo-part.bak",
            "backup.txt.1-2.mo-part.bak",
            "backup.txt.old.mo-part",
            "backup.txt.4120.mo-part",
            "backup.txt.4120-.mo-part",
            "backup.txt.-0.mo-part",
            "backup.txt.41a-0.mo-part",
            "backup.txt.1-2-3.mo-part",
            "backup.txt.+1-2.mo-part",
            "backup.txt4120-0.mo-part",
            "my backup.txt.1-2.mo-part",
            "backup-songs.txt.1-2.mo-part",
            "backup.txt.1-2.mo-part-3",
        ] {
            assert!(!is_whole_part_of(not, SETTINGS_FILE), "{not}");
        }
    }

    #[test]
    fn settings_keep_through_their_file() {
        let s = Settings {
            drive: Some(PathBuf::from(r"E:\Music Backup")),
            level: Level::Essentials,
            format: Format::Zips,
            folders: vec![
                Folder { path: PathBuf::from(r"I:\Samples"), mode: Mode::Slimmed },
                Folder { path: PathBuf::from(r"K:\Bitwig Studio\Library"), mode: Mode::Whole },
            ],
            last_run: Some(LastRun {
                when: NaiveDateTime::parse_from_str("2026-11-01 03:40:00", "%Y-%m-%d %H:%M:%S").unwrap_or_default(),
                ok: true,
                line: "3,881 checked\t· 12 new".to_owned(),
            }),
        };
        let dir = std::env::temp_dir().join(format!("mo-backup-settings-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap_or_default();
        let path = dir.join(SETTINGS_FILE);
        s.save(&path).unwrap_or_default();
        let back = Settings::load(&path);
        assert_eq!(back.drive, s.drive);
        assert_eq!(back.level, Level::Essentials);
        assert_eq!(back.format, Format::Zips);
        assert_eq!(back.folders, s.folders);
        assert_eq!(back.last_run.map(|r| r.line), Some("3,881 checked · 12 new".to_owned()));
        assert_eq!(Settings::load(&dir.join("none.txt")), Settings::default());
    }

    #[test]
    fn folders_are_added_once_and_removed_by_place() {
        let mut s = Settings::default();
        s.add_folder(PathBuf::from("/music"));
        s.add_folder(PathBuf::from("/music"));
        s.add_folder(PathBuf::from("/samples"));
        assert_eq!(s.folders.len(), 2);
        s.set_mode(1, Mode::Whole);
        assert_eq!(s.folders[1].mode, Mode::Whole);
        s.remove_folder(0);
        s.remove_folder(9);
        assert_eq!(s.folders.len(), 1);
    }

    #[test]
    fn sample_paths_are_made_whole_where_they_can_be() {
        assert_eq!(sample_file(r"I:\Samples\Kick.wav", r"I:\Song"), Some(PathBuf::from(r"I:\Samples\Kick.wav")));
        assert_eq!(sample_file("/s/Kick.wav", "/song"), Some(PathBuf::from("/s/Kick.wav")));
        assert_eq!(sample_file("Samples/Kick.wav", "/song"), Some(Path::new("/song").join("Samples/Kick.wav")));
        assert_eq!(sample_file("Kick.wav", "/song"), None);
    }

    #[test]
    fn the_page_says_what_to_do_next_and_what_goes() {
        let now = NaiveDateTime::default();
        let mut s = Settings::default();
        let empty = page_lines(&s, None, PlanState::Waiting, now);
        assert_eq!(empty.plan_note, "Add a folder to see what goes");
        assert_eq!((empty.level, empty.format), (1, 0));
        s.add_folder(PathBuf::from("/music"));
        s.add_folder(PathBuf::from("/unplugged"));
        assert_eq!(page_lines(&s, None, PlanState::Waiting, now).plan_note, "Pick a backup drive to see what goes");
        s.drive = Some(PathBuf::from("/backup"));
        let file = |p: &str, size| FileEntry { path: PathBuf::from(p), size, modified: now };
        let prepared = Prepared {
            plan: Plan {
                copy: vec![file("/music/a.flp", 300)],
                left_out: vec![plan::Skipped { file: file("/music/big.wav", 700), why: LeftOut::UnusedSound }],
            },
            all_bytes: 1000,
            folder_bytes: vec![Some(1000), None],
            made_for: (s.folders.clone(), s.level),
            ..Prepared::default()
        };
        let room = Some(DriveRoom { free: 25, total: 100 });
        let l = page_lines(&s, room, PlanState::Ready(&prepared), now);
        assert_eq!(l.goes, "300 bytes");
        assert!((l.goes_part - 0.3).abs() < 1e-6 && (l.drive_fill - 0.75).abs() < 1e-6);
        // 25 bytes left is under Space's "almost full" line, so the drive line is red there and here (X3-16).
        assert_eq!(l.drive_state, 2);
        let roomy = Some(DriveRoom { free: 500 * 1024 * 1024 * 1024, total: 1000 * 1024 * 1024 * 1024 });
        assert_eq!(page_lines(&s, roomy, PlanState::Ready(&prepared), now).drive_state, 0);
        assert_eq!(l.folders[1].size, "Not plugged in");
        assert_eq!(l.left_out[0].0, "Big unused sounds");
        assert_eq!(l.left_out[0].1, "Big sound no project uses · 1 file");
        // A plan made for another folder list isn't shown.
        s.add_folder(PathBuf::from("/more"));
        assert_eq!(page_lines(&s, room, PlanState::Ready(&prepared), now).goes, "");
    }

    #[test]
    fn the_save_a_song_opens_is_its_own_or_the_copy_of_it() {
        // Named as the library names them on Windows, whatever the slashes mean here.
        let save = |folder: &str, name: &str, size: i64| mo_library::Save {
            path: format!(r"{folder}\{name}.flp"),
            name: name.to_owned(),
            daw: "FL Studio".to_owned(),
            size,
            note_count: 10,
            length_bars: 8,
            track_count: 3,
            ..mo_library::Save::default()
        };
        let song = Song {
            name: "Night Drive".to_owned(),
            latest: save(r"D:\Songs", "Night Drive_3", 300),
            older: vec![save(r"D:\Songs", "Night Drive_2", 200)],
            key: "night drive".to_owned(),
            copies: vec![save(r"E:\Copy", "Night Drive_2", 200), save(r"E:\Copy", "Night Drive_3", 300)],
        };
        assert_eq!(opens_of(&song, true), Some(r"D:\Songs\Night Drive_3.flp"));
        assert_eq!(opens_of(&song, false), Some(r"E:\Copy\Night Drive_3.flp"));
        // A save the library can't tell apart from others has no copy it can name.
        let unknown = Song { latest: mo_library::Save { note_count: -1, ..song.latest.clone() }, ..song };
        assert_eq!(opens_of(&unknown, false), None);
    }

    #[test]
    fn big_changes_come_from_what_the_library_knows() {
        let t = |h: u32| chrono::NaiveDate::from_ymd_opt(2026, 3, 14).unwrap().and_hms_opt(h, 0, 0).unwrap();
        let save = |n: u32, tracks: i32, bars: i32| mo_library::Save {
            path: format!("/s/{n}.flp"),
            modified: t(n),
            track_count: tracks,
            length_bars: bars,
            plugin_count: 4,
            sample_count: 10,
            ..mo_library::Save::default()
        };
        let a = save(1, 4, 16);
        let b = save(2, 9, 16); // five tracks more: big
        let c = save(3, -1, 40); // tracks unknown; 24 bars longer: big
        let mut broken = save(4, 0, -1);
        broken.plugin_count = 0;
        broken.error = Some("damaged".into()); // couldn't be read: says nothing
        let found = plan::big_changes(&shapes(&[&a, &b, &c, &broken]));
        assert_eq!(found, ["/s/2.flp", "/s/3.flp"]);
    }

    #[test]
    fn each_song_says_when_it_last_went() {
        let t = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").unwrap();
        let now = t("2026-11-01 12:00:00");
        let drive = Path::new("E:\\Backup");
        let mut b = BackedUp::default();
        b.add(drive, &[(7, t("2026-10-31 22:15:03.250")), (9, t("2026-10-20 10:00:00"))], t("2026-11-01 09:30:00"));
        let file = std::env::temp_dir().join(format!("mo-backup-songs-{}.txt", std::process::id()));
        b.save(&file).unwrap();
        let back = BackedUp::load(&file);
        let _ = fs::remove_file(&file);
        assert_eq!(back, b, "the list keeps through its file, to the part of a second");

        let d = Some(drive);
        assert_eq!(b.song_line(d, 7, t("2026-10-31 22:15:03.250"), now), "Backed up today 09:30");
        assert_eq!(
            b.song_line(d, 7, t("2026-11-01 11:00:00"), now),
            "Newest save not backed up yet · last backed up today 09:30"
        );
        assert_eq!(b.song_line(d, 8, now, now), "Not backed up yet");
        assert_eq!(b.song_line(None, 7, now, now), "", "quiet until a backup drive is picked");
        assert_eq!(b.song_line(Some(Path::new("F:\\Other")), 7, now, now), "Not backed up yet");

        // A later run that didn't reach song 9 keeps its line; a new drive starts again.
        b.add(drive, &[(7, t("2026-11-01 11:00:00"))], t("2026-11-01 11:30:00"));
        assert_eq!(b.songs.len(), 2);
        b.add(Path::new("F:\\Other"), &[(9, t("2026-10-20 10:00:00"))], t("2026-11-01 11:45:00"));
        assert_eq!(b.songs.len(), 1);
    }

    #[test]
    fn times_read_as_people_say_them() {
        let at = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap_or_default();
        let now = at("2026-11-01 10:00:00");
        assert_eq!(when_text(at("2026-11-01 08:05:00"), now), "today 08:05");
        assert_eq!(when_text(at("2026-10-31 23:40:00"), now), "yesterday 23:40");
        assert_eq!(when_text(at("2026-10-28 23:40:00"), now), "Wednesday 23:40");
        assert_eq!(when_text(at("2026-10-12 23:40:00"), now), "12 Oct 23:40");
        assert_eq!(last_run_line(None, now), "Not backed up yet");
    }
}
