//! The Backup section of the Space page (Solomon liked the mock-up, 1 Nov 2026): the backup drive and its last run,
//! How much (Full, Slim, Essentials), the format (plain folders or one zip per song), the list of folders (each whole
//! or slimmed), and what goes and what's left out, and why.
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
use mo_backup::pack::{Packed, Packer, pack_names};
use mo_backup::plan::{self, Folders, LeftOut, Level, Plan, Rules, SongInfo, Used};
use mo_formats::case;
use mo_formats::paths::{file_name, stem};
use mo_library::Library;
use mo_library::scan::is_inside;
use mo_library::songs::{Song, group_songs, marked};
use mo_space::size::{FileEntry, size_text};
use mo_space::song::{CompactSettings, DEFAULT_KEEP_BACKUPS, Elsewhere, SongSave, SongSpaceReport, measure};
use rayon::prelude::*;

/// The settings file's name, in the app's folder.
pub const SETTINGS_FILE: &str = "backup.txt";

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

    /// Writes `backup.txt`: under a temporary name first, then renamed, so a crash part way leaves the old one whole.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let part = path.with_extension("txt.mo-part");
        fs::write(&part, self.to_text())?;
        fs::rename(&part, path).inspect_err(|_| {
            let _ = fs::remove_file(&part);
        })
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

    /// Writes `backup-songs.txt`: under a temporary name first, then renamed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let part = path.with_extension("txt.mo-part");
        fs::write(&part, self.to_text())?;
        fs::rename(&part, path).inspect_err(|_| {
            let _ = fs::remove_file(&part);
        })
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
    /// The whole folders.
    pub always: Vec<PathBuf>,
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

/// Walks the folders on the list, finds the songs saved in them and the samples they use (wherever those are), and
/// makes the plan. Reads only. Slow: on the app's low-priority pool, never the window's thread.
pub fn prepare(library: &Library, settings: &Settings, stop: &AtomicBool) -> Result<Prepared, String> {
    let backup = settings.drive.clone().ok_or("Pick a backup drive first")?;
    let gathered = folders::gather(&settings.folders, stop).map_err(|_| "Stopped".to_owned())?;
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
    let mut have: HashSet<String> = files.iter().map(|f| case::key(&f.path.to_string_lossy())).collect();
    let all_songs = group_songs(&saves, &choices);
    // FL Studio save names more than one song has ("Beat", "Untitled"): its shared Backup folder can't say whose an
    // autosave of those is, so measuring leaves them alone.
    let mut name_songs: HashMap<String, usize> = HashMap::new();
    for song in &all_songs {
        let names: HashSet<String> = song.saves().map(|v| case::key(stem(&v.path.replace('\\', "/")))).collect();
        for n in names {
            *name_songs.entry(n).or_default() += 1;
        }
    }
    let shared_names: HashSet<String> = name_songs.into_iter().filter(|(_, n)| *n > 1).map(|(k, _)| k).collect();
    let used_names: HashSet<String> = samples.values().flatten().map(|p| case::key(file_name(p))).collect();
    let used_by_any = |names: &[String]| -> HashSet<String> {
        names.iter().map(|n| case::key(n)).filter(|k| used_names.contains(k)).collect()
    };
    let elsewhere =
        Elsewhere { sample_folders: &sample_folders, used_by_any: &used_by_any, shared_names: &shared_names };
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
    let measured: Vec<Measured> = songs
        .par_iter()
        .map(|song| {
            // The song's own saves, or (when only copies of them are in the folders) the copies.
            let own = song.saves().any(|v| have.contains(&case::key(&v.path)));
            let picked: Vec<&mo_library::Save> =
                if own { song.saves().collect() } else { song.copies.iter().collect() };
            let saves: Vec<SongSave> = picked
                .iter()
                .map(|v| SongSave {
                    path: v.path.clone(),
                    size: u64::try_from(v.size).unwrap_or(0),
                    modified: v.modified,
                })
                .collect();
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
            let report = measure(&saves, &roots, &used_text, compact, Some(&elsewhere), stop).map_err(|_| ())?;
            // Saves you mark as milestones by their name always go.
            let milestones = picked.iter().filter(|v| marked(&v.name)).map(|v| v.path.clone()).collect();
            Ok(Measured { saves, report, milestones, used, id: song_id(song) })
        })
        .collect::<Result<_, ()>>()
        .map_err(|()| "Stopped".to_owned())?;

    // The samples the songs use, wherever they are: one outside the folders is added, so a song's backup is whole.
    for m in &measured {
        for p in &m.used {
            let k = case::key(&p.to_string_lossy());
            if have.contains(&k) {
                continue;
            }
            if let Ok(meta) = fs::metadata(p)
                && meta.is_file()
            {
                have.insert(k);
                files.push(FileEntry {
                    path: p.clone(),
                    size: meta.len(),
                    modified: meta
                        .modified()
                        .map(|t| chrono::DateTime::<Local>::from(t).naive_local())
                        .unwrap_or_default(),
                });
            }
        }
    }
    let all_bytes = files.iter().map(|f| f.size).fold(0, u64::saturating_add);
    let used = Used::new(samples.values().flatten().map(String::as_str));
    let infos: Vec<SongInfo<'_>> =
        measured.iter().map(|m| SongInfo { report: &m.report, saves: &m.saves, milestones: &m.milestones }).collect();
    let rules = Rules { level: settings.level, ..Rules::default() };
    let plan = plan::plan(files, &infos, &used, Folders { backup: &backup, always: &gathered.always }, rules);

    // Each song's files that go: its saves, the samples it uses, and (when it has a folder to itself) the rest of it.
    // Files in whole folders stay out of the zips: they go as plain folders.
    let going: HashMap<String, &FileEntry> =
        plan.copy.iter().map(|f| (case::key(&f.path.to_string_lossy()), f)).collect();
    let always: Vec<String> = gathered.always.iter().map(|w| w.to_string_lossy().into_owned()).collect();
    let in_whole = |p: &Path| {
        let p = p.to_string_lossy();
        always.iter().any(|w| is_inside(&p, w))
    };
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
    let newest = measured
        .iter()
        .filter_map(|m| m.saves.iter().max_by_key(|s| s.modified).map(|s| (m.id, PathBuf::from(&s.path), s.modified)))
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
                    && !in_whole(&f.path)
                    && seen.insert(k)
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
        always: gathered.always,
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
}

/// Copies the plan to the drive (or, for the zip format, packs each song and copies the whole folders), and says how
/// it went. Never on the window's thread. A run that stops part way still says which songs went before it stopped.
pub fn run(prepared: &Prepared, settings: &Settings, stop: &AtomicBool, progress: &dyn Fn(Progress)) -> Ran {
    let mut there: HashSet<String> = HashSet::new();
    let last = run_on(prepared, settings, stop, progress, &mut there);
    let went = prepared
        .newest
        .iter()
        .filter(|(_, path, _)| there.contains(&case::key(&path.to_string_lossy())))
        .map(|(id, _, time)| (*id, *time))
        .collect();
    Ran { last, went }
}

/// The run itself; `there` gathers the files now safely on the drive.
fn run_on(
    prepared: &Prepared,
    settings: &Settings,
    stop: &AtomicBool,
    progress: &dyn Fn(Progress),
    there: &mut HashSet<String>,
) -> LastRun {
    let when = Local::now().naive_local();
    let Some(backup) = settings.drive.as_deref() else {
        return LastRun { when, ok: false, line: "No backup drive picked".to_owned() };
    };
    let options = copy::Options::new(when.format("%Y-%m-%d %H.%M").to_string());
    let copy_list: Vec<FileEntry> = match settings.format {
        Format::Folders => prepared.plan.copy.clone(),
        // Everything that isn't in a song's zip still goes, as plain folders: whole folders, and files no song owns
        // (presets, MIDI packs, samples no song in the folders uses).
        Format::Zips => {
            let zipped: HashSet<String> =
                prepared.songs.iter().flat_map(|s| &s.files).map(|p| case::key(&p.to_string_lossy())).collect();
            prepared
                .plan
                .copy
                .iter()
                .filter(|f| !zipped.contains(&case::key(&f.path.to_string_lossy())))
                .cloned()
                .collect()
        }
    };
    let mut record_note = None;
    let mut problems: Vec<String> = Vec::new();
    let mut new_or_changed = 0usize;
    let mut checked = 0usize;
    if !copy_list.is_empty() {
        let bytes = copy_list.iter().map(|f| f.size).fold(0, u64::saturating_add);
        progress(Progress::Copying { files: copy_list.len(), bytes });
        match copy::run(&copy_list, backup, &options, stop) {
            Err(_) => return LastRun { when, ok: false, line: "The backup drive isn't there".to_owned() },
            Ok(report) => {
                for l in &report.lines {
                    match &l.outcome {
                        Outcome::New | Outcome::Updated { .. } => {
                            new_or_changed += 1;
                            checked += 1;
                            there.insert(case::key(&l.source.to_string_lossy()));
                        }
                        Outcome::UpToDate => {
                            checked += 1;
                            there.insert(case::key(&l.source.to_string_lossy()));
                        }
                        Outcome::Failed(why) => problems.push(format!("{}: {why}", l.source.display())),
                        Outcome::NotReached => {}
                    }
                }
                if report.stopped {
                    return LastRun {
                        when,
                        ok: false,
                        line: format!("Stopped · {} files checked", thousands(checked)),
                    };
                }
                if report.full {
                    return LastRun { when, ok: false, line: "The backup drive is full".to_owned() };
                }
                if report.gone {
                    return LastRun { when, ok: false, line: "The backup drive went away part way".to_owned() };
                }
                record_note = report.record_not_saved;
            }
        }
    }
    if settings.format == Format::Zips {
        let Ok(packer) = Packer::new(backup) else {
            return LastRun { when, ok: false, line: "The backup drive isn't there".to_owned() };
        };
        let names = pack_names(&prepared.songs.iter().map(|s| (s.title.as_str(), s.id)).collect::<Vec<_>>());
        for (i, (song, name)) in prepared.songs.iter().zip(&names).enumerate() {
            progress(Progress::Packing { done: i, of: prepared.songs.len() });
            let packed = packer.pack_song(&song.files, name, song.id, stop);
            if packed.is_ok() {
                there.extend(song.files.iter().map(|p| case::key(&p.to_string_lossy())));
            }
            match packed {
                Ok(Packed::Written { .. }) => {
                    new_or_changed += 1;
                    checked += 1;
                }
                Ok(Packed::UpToDate) => checked += 1,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    return LastRun { when, ok: false, line: format!("Stopped · {} checked", thousands(checked)) };
                }
                Err(e) if e.kind() == io::ErrorKind::StorageFull => {
                    return LastRun { when, ok: false, line: "The backup drive is full".to_owned() };
                }
                Err(_) if !backup.is_dir() => {
                    return LastRun { when, ok: false, line: "The backup drive went away part way".to_owned() };
                }
                Err(e) => problems.push(format!("{}: {e}", song.title)),
            }
        }
    }
    let mut line = format!("{} checked · {} new or changed", thousands(checked), thousands(new_or_changed));
    if !problems.is_empty() {
        line.push_str(&format!(" · {} couldn't go", thousands(problems.len())));
    }
    if !prepared.missing.is_empty() {
        line.push_str(&format!(" · {} folders not plugged in", prepared.missing.len()));
    }
    if record_note.is_some() {
        // Everything was still copied and checked; the next run just reads every file again.
        line.push_str(" · the backup's own list couldn't be saved, so next time takes longer");
    }
    LastRun { when, ok: problems.is_empty() && prepared.missing.is_empty(), line }
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
    let (drive_fill, drive_line) = match room {
        Some(r) if r.total > 0 => {
            ((r.total.saturating_sub(r.free) as f64 / r.total as f64) as f32, drive_line(r.free, r.total))
        }
        _ => (0.0, String::new()),
    };
    PageLines {
        drive: settings.drive.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
        drive_fill,
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
            "Open it in Explorer without the app. Only new or changed files are copied; small replaced files keep older versions."
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
        assert_eq!(l.folders[1].size, "Not plugged in");
        assert_eq!(l.left_out[0].0, "Big unused sounds");
        assert_eq!(l.left_out[0].1, "Big sound no project uses · 1 file");
        // A plan made for another folder list isn't shown.
        s.add_folder(PathBuf::from("/more"));
        assert_eq!(page_lines(&s, room, PlanState::Ready(&prepared), now).goes, "");
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
