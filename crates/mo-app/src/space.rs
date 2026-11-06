//! The Space page's words, and the drives it shows (phase 4). Part 1: the drives your projects, samples, backup and
//! the app's own files are on, how full each one is, and what the app's own files take. Every word is decided here so
//! it can be tested without a window; mo-ui's `space.rs` reads the disks (never on the window's thread) and fills the
//! page. What can be freed comes a part at a time, as agreed in the page's question round (2026-11-03): Bitwig copies
//! first ([`crate::space_bitwig`], whose Fix writes new "(fixed)" projects), then old copies, spare plugin copies,
//! songs ([`crate::space_songs`]) and plugins. Nothing here changes a file.

use std::path::{Path, PathBuf};

use mo_space::app_files::{self, AppFilesPart, DriveSpace, VERY_LOW_SPACE};
use mo_space::size::size_text;

/// What one of your folders holds, as its drive's card names it, in the order the card lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Holds {
    Projects,
    Samples,
    Backup,
    AppFiles,
}

impl Holds {
    fn words(self) -> &'static str {
        match self {
            Holds::Projects => "projects",
            Holds::Samples => "samples",
            Holds::Backup => "the backup",
            Holds::AppFiles => "the app's files",
        }
    }
}

/// A drive one of your folders is on: its name, its room, and what of yours it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// "C:" on Windows; on a Mac or Linux, where the disk is mounted ("your disk" for the system's own).
    pub name: String,
    pub free: u64,
    pub total: u64,
    /// Whether its room could be read: an unplugged drive (a backup drive, say) still shows, as not plugged in.
    pub here: bool,
    pub holds: Vec<Holds>,
}

/// How full a drive is with `free` bytes left and the app's own files taking `app_bytes` on it (0 when they're
/// elsewhere): the one rule for Space's drive bars and Backup's drive line, so a drive looks the same on both.
pub fn fill_of(free: u64, app_bytes: u64) -> Fill {
    if free < VERY_LOW_SPACE {
        Fill::Almost
    } else if app_files::is_low(free, app_bytes) {
        Fill::Nearly
    } else {
        Fill::Roomy
    }
}

impl Fill {
    /// 0 roomy, 1 nearly full, 2 almost full: what the pages' `fill` numbers mean.
    pub fn index(self) -> i32 {
        match self {
            Fill::Roomy => 0,
            Fill::Nearly => 1,
            Fill::Almost => 2,
        }
    }
}

/// How full a drive is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    Roomy,
    /// Under 10 GB left, or, on the drive with the app's files, under what they take (a big scan can grow them by as
    /// much again).
    Nearly,
    /// Under 2 GB left: saving may already fail.
    Almost,
}

impl Drive {
    /// How full it is, with the app's own files taking `app_bytes` (they only count on their own drive).
    pub fn fill(&self, app_bytes: u64) -> Fill {
        let app = if self.holds.contains(&Holds::AppFiles) { app_bytes } else { 0 };
        if !self.here { Fill::Roomy } else { fill_of(self.free, app) }
    }

    /// How much of it is used, from 0 to 1.
    pub fn used(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.total.saturating_sub(self.free) as f64 / self.total as f64) as f32 }
    }
}

/// The folders the page looks at: each project folder the app scans and each sample folder (both from the library),
/// the backup drive, and the app's own folder. A folder named twice is looked at once.
pub fn folders(
    roots: &[String],
    samples: &[String],
    backup: Option<&Path>,
    app_folder: &Path,
) -> Vec<(Holds, PathBuf)> {
    let mut out: Vec<(Holds, PathBuf)> = Vec::new();
    let mut add = |holds: Holds, path: PathBuf| {
        if !path.as_os_str().is_empty() && !out.iter().any(|(h, p)| *h == holds && p == &path) {
            out.push((holds, path));
        }
    };
    roots.iter().for_each(|r| add(Holds::Projects, PathBuf::from(r)));
    samples.iter().for_each(|s| add(Holds::Samples, PathBuf::from(s)));
    if let Some(b) = backup {
        add(Holds::Backup, b.to_path_buf());
    }
    add(Holds::AppFiles, app_folder.to_path_buf());
    out
}

/// What looking for a folder's drive found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// The drive is there. Folders whose `key` is the same are on one drive; `room` has the name it's shown by.
    Here { key: String, room: DriveSpace },
    /// The drive isn't there (unplugged, or a share that's offline): the name it's shown by, which is also its key.
    Away(String),
}

/// The drives `folders` are on, each once, in the order of their names, with what each holds. `look` finds a folder's
/// drive ([`look`] is the real one; tests give their own, so no test reads a real drive).
pub fn drives(folders: &[(Holds, PathBuf)], look: impl Fn(&Path) -> Found) -> Vec<Drive> {
    let mut out: Vec<(String, Drive)> = Vec::new();
    for (holds, folder) in folders {
        let (key, name, room) = match look(folder) {
            Found::Here { key, room } => (key, room.drive.clone(), Some(room)),
            Found::Away(name) => (name.clone(), name, None),
        };
        let at = match out.iter().position(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            Some(at) => at,
            None => {
                out.push((key, Drive { name, free: 0, total: 0, here: false, holds: Vec::new() }));
                out.len() - 1
            }
        };
        let drive = &mut out[at].1;
        if !drive.here
            && let Some(r) = room
        {
            (drive.free, drive.total, drive.here) = (r.free, r.total, true);
        }
        if !drive.holds.contains(holds) {
            drive.holds.push(*holds);
        }
    }
    let mut out: Vec<Drive> = out.into_iter().map(|(_, d)| d).collect();
    for d in &mut out {
        d.holds.sort();
    }
    out.sort_by_key(|d| d.name.to_ascii_lowercase());
    out
}

/// A folder's drive and its room, for [`drives`]. Asks the disk, so never on the window's thread (rule 12): a drive
/// that has gone to sleep can take a few seconds to answer.
///
/// A folder that has gone (moved or deleted) is looked for through the nearest folder above it that is still there,
/// so its drive still shows with its room. Its drive is away only when nothing above it is there (the drive letter
/// itself has gone), or only the folder other drives are mounted in (`/Volumes` on a Mac).
pub fn look(path: &Path) -> Found {
    let there = path.ancestors().find(|a| a.exists());
    let room = there.filter(|t| *t == path || !holds_mounts(t)).and_then(app_files::drive_space);
    match (there, room) {
        (Some(there), Some(room)) => here(there, room),
        _ => Found::Away(away_name(path, there)),
    }
}

/// On Windows a drive is known by its letter (or share), which `drive_space` gives.
#[cfg(not(unix))]
fn here(_there: &Path, room: DriveSpace) -> Found {
    Found::Here { key: room.drive.clone(), room }
}

/// On a Mac or Linux every disk would be "your disk": each is told apart by its device and named after where it's
/// mounted instead.
#[cfg(unix)]
fn here(there: &Path, room: DriveSpace) -> Found {
    match mounted_as(there) {
        Some((device, drive)) => Found::Here { key: format!("device {device}"), room: DriveSpace { drive, ..room } },
        None => Found::Here { key: room.drive.clone(), room },
    }
}

/// The device a folder is on, and the disk's name from where it's mounted: "Samples" for /Volumes/Samples, "your
/// disk" for the system's own. On a Mac the system's disk is reached through folders at the top (/Users,
/// /Applications), which count as "your disk" too.
#[cfg(unix)]
fn mounted_as(path: &Path) -> Option<(u64, String)> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize(path).ok()?;
    let device = std::fs::metadata(&path).ok()?.dev();
    let mut top = path.as_path();
    while let Some(up) = top.parent() {
        if std::fs::metadata(up).ok()?.dev() != device {
            break;
        }
        top = up;
    }
    let systems = cfg!(target_os = "macos") && top.parent() == Some(Path::new("/"));
    let name = match top.file_name() {
        Some(name) if !systems => name.to_string_lossy().into_owned(),
        _ => "your disk".to_owned(),
    };
    Some((device, name))
}

/// Whether a folder is where other drives are mounted (/Volumes on a Mac; /media, /media/you, /run/media/you and
/// /mnt on Linux): a folder below it that isn't there means its drive isn't.
fn holds_mounts(folder: &Path) -> bool {
    if !cfg!(unix) {
        return false;
    }
    let up = folder.parent();
    [Path::new("/Volumes"), Path::new("/media"), Path::new("/mnt"), Path::new("/run/media")].contains(&folder)
        || up == Some(Path::new("/media"))
        || up == Some(Path::new("/run/media"))
}

/// The name of a drive that isn't there, from the folder's path: "E:" for E:\Backup, \\NAS\Music for a share, and on
/// a Mac or Linux the drive's own folder ("Samples" for /Volumes/Samples/Loops) below `there`, the nearest folder
/// above it that is.
fn away_name(path: &Path, there: Option<&Path>) -> String {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    if let (Some(letter), Some(':')) = (chars.next(), chars.next())
        && letter.is_ascii_alphabetic()
    {
        return format!("{}:", letter.to_ascii_uppercase());
    }
    if let Some(share) = text.strip_prefix(r"\\") {
        let mut parts = share.split('\\').filter(|p| !p.is_empty());
        if let (Some(server), Some(name)) = (parts.next(), parts.next()) {
            return format!(r"\\{server}\{name}");
        }
    }
    there
        .and_then(|t| path.strip_prefix(t).ok())
        .and_then(|below| below.components().next())
        .map_or_else(|| text.into_owned(), |c| c.as_os_str().to_string_lossy().into_owned())
}

/// A drive card's room line: "38 GB free of 500 GB".
pub fn room_line(d: &Drive) -> String {
    if d.here { format!("{} free of {}", size_text(d.free), size_text(d.total)) } else { "Not plugged in".to_owned() }
}

/// Things as a sentence lists them: "A", "A and B", "A, B and C" ("" for none).
pub fn and_list<S: AsRef<str>>(items: &[S]) -> String {
    match items {
        [] => String::new(),
        [one] => one.as_ref().to_owned(),
        [rest @ .., last] => {
            let rest: Vec<&str> = rest.iter().map(AsRef::as_ref).collect();
            format!("{} and {}", rest.join(", "), last.as_ref())
        }
    }
}

/// What of yours a drive holds: "Projects, samples and the app's files".
pub fn holds_line(d: &Drive) -> String {
    let words: Vec<&str> = d.holds.iter().map(|h| h.words()).collect();
    let joined = and_list(&words);
    let mut chars = joined.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Why a drive that's nearly full needs room, or "" when it has room. On the app's own drive it says what a full one
/// stops: the library saving what scans find (the C# app's warning).
pub fn fill_note(d: &Drive, app_bytes: u64) -> String {
    let left = size_text(d.free);
    let library = d.holds.contains(&Holds::AppFiles);
    let why = if library { " Once it's full, the library can't save what scans find." } else { "" };
    match d.fill(app_bytes) {
        Fill::Roomy => String::new(),
        Fill::Nearly => format!("Nearly full: {left} left.{why} Free some room soon."),
        Fill::Almost => format!("Almost full: {left} left.{why} Free some room now."),
    }
}

/// The page's summary beside its title: the fullest drive when one is nearly full, else the room left on them all.
pub fn summary(drives: &[Drive], app_bytes: u64) -> String {
    let short = drives.iter().filter(|d| d.fill(app_bytes) != Fill::Roomy).min_by_key(|d| d.free);
    if let Some(d) = short {
        let how = if d.fill(app_bytes) == Fill::Almost { "almost" } else { "nearly" };
        return format!("{} is {how} full: {} left", d.name, size_text(d.free));
    }
    let here: Vec<&Drive> = drives.iter().filter(|d| d.here).collect();
    let free = here.iter().fold(0u64, |sum, d| sum.saturating_add(d.free));
    match here.as_slice() {
        [] => String::new(),
        [one] => format!("{} free on {}", size_text(free), one.name),
        many => format!("{} free on {} drives", size_text(free), many.len()),
    }
}

/// A line under Biggest wins first or Also taking room: what it is, why it's there, and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub id: &'static str,
    pub title: &'static str,
    pub why: String,
    pub size: String,
}

/// Biggest wins first: the lines that free room, each with the room it frees, biggest first. Lines that free the same
/// keep their order.
pub fn biggest_first(mut lines: Vec<(u64, Line)>) -> Vec<Line> {
    lines.sort_by_key(|(bytes, _)| std::cmp::Reverse(*bytes));
    lines.into_iter().map(|(_, line)| line).collect()
}

/// What the page says under Biggest wins first when no look found anything and none says why.
pub const NOTHING_TO_FREE: &str = "Nothing to free right now.";

/// What the page says under Biggest wins first: why a kind has no line when a look found none of it (each kind's own
/// sentence, ending in a space; "" for one with nothing to say), and with no `lines` at all, that there's nothing to
/// free when no kind says why; "" when there's nothing to say (the page then shows no note).
pub fn wins_note<S: AsRef<str>>(nones: &[S], lines: usize) -> String {
    let note = nones.iter().map(AsRef::as_ref).collect::<String>().trim_end().to_owned();
    if note.is_empty() && lines == 0 { NOTHING_TO_FREE.to_owned() } else { note }
}

/// The App files line's id.
pub const APP_FILES: &str = "app-files";

/// The App files line: what the app's own folder holds and its size, "Measuring…" until it's measured (`parts`
/// `None`), on the drive named `drive`.
pub fn app_files_line(parts: Option<&[AppFilesPart]>, drive: Option<&str>) -> Line {
    let on = drive.map(|d| format!(", on {d}")).unwrap_or_default();
    Line {
        id: APP_FILES,
        title: "App files",
        why: format!("The libraries, search models, logs and notes{on}"),
        size: parts.map_or_else(|| "Measuring…".to_owned(), |p| size_text(app_bytes(p))),
    }
}

/// What the app's own files take in all.
pub fn app_bytes(parts: &[AppFilesPart]) -> u64 {
    parts.iter().fold(0u64, |sum, p| sum.saturating_add(p.bytes))
}

/// What a part's link that opens something does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opens {
    /// Opens a fixed project in Bitwig.
    InBitwig,
    /// Shows it (an old copy, a spare plugin copy) in its folder.
    InFolder,
    /// Opens the folder itself (the one a format's spare copies are all in).
    Folder,
}

impl Opens {
    /// The link's words.
    pub fn words(self) -> &'static str {
        match self {
            Opens::InBitwig => "Open in Bitwig",
            Opens::InFolder => "Show in folder",
            Opens::Folder => "Open folder",
        }
    }
}

/// One part on a card: its name, what it holds and its size; and for a part you can act on, its tick, what happened to
/// it, a file it opens, a link that shows more, one that puts back what a clean-up sent to the Recycle Bin, and one
/// that shows it on another page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub what: String,
    pub size: String,
    /// Listed under the part above it (a save under its project), a step in.
    pub under: bool,
    /// What its tick is for (a save, a spare copy, or every copy of a format that can go) and whether it's ticked;
    /// None when it has no tick.
    pub tick: Option<(Vec<PathBuf>, bool)>,
    /// Some of what its tick is for are ticked, not all: its tick shows a dash, and ticking it ticks them all.
    pub partly: bool,
    /// What happened to it, or what to know about it; "" for nothing. `problem` shows it as a problem.
    pub status: String,
    pub problem: bool,
    /// Its link that opens something: what it does, and the file or folder it's for (a fixed project, opened in
    /// Bitwig; an old copy, shown in its folder).
    pub open: Option<(Opens, PathBuf)>,
    /// Its link that shows or hides more under it: its words, and what it's for (a project's folder).
    pub more: Option<(String, PathBuf)>,
    /// Its Put back link, for a clean-up whose files are in the Recycle Bin: which clean-up (its kind, and when it ran).
    pub put_back: Option<(String, i64)>,
    /// Its link to the Plugins page: one plugin's card there, or every plugin no song has used lately.
    pub plugins: Option<OnPlugins>,
    /// Its link to the Sounds page: the sound it shows there, by id.
    pub sound: Option<i64>,
}

/// The words of a part's link to the Sounds page.
pub const ON_SOUNDS: &str = "Show on Sounds page";

/// What a part's link to the Plugins page shows there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnPlugins {
    /// One plugin's card, by its line's name key and own (the DAW of a DAW's own device), and its name (what that page
    /// looks for when it's no longer there by those).
    Plugin { key: String, own: String, name: String },
    /// Every plugin no song has used lately (the Plugins page's "Big and unused").
    Unused,
}

impl OnPlugins {
    /// The link's words.
    pub fn words(&self) -> &'static str {
        match self {
            OnPlugins::Plugin { .. } => "Show on Plugins page",
            OnPlugins::Unused => "Show them on Plugins page",
        }
    }
}

/// The picked line's card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub kind: &'static str,
    pub title: &'static str,
    /// Its size in big figures, and what they mean.
    pub big: String,
    pub big_note: String,
    pub lead: String,
    /// Whether its parts can be ticked: each keeps room for its tick, so their names line up.
    pub ticks: bool,
    /// Each part, biggest first.
    pub parts: Vec<Part>,
    /// What to know about the parts and what the buttons do, shown above them; "" when there's nothing more to say.
    pub note: String,
    /// The drive's room, in a sentence.
    pub room: String,
    /// Why the drive needs room, when it's nearly full.
    pub warning: String,
}

/// The App files card: each part of the app's own folder (each database with the files SQLite keeps beside it, the
/// search models, the logs...) and the room on its drive.
pub fn app_files_card(parts: Option<&[AppFilesPart]>, drive: Option<&Drive>) -> Card {
    let bytes = parts.map(app_bytes);
    Card {
        kind: "APP FILES",
        title: "The app's own files",
        big: bytes.map_or_else(|| "Measuring…".to_owned(), size_text),
        big_note: match (bytes, drive) {
            (Some(_), Some(d)) => format!("in all, on {}", d.name),
            (Some(_), None) => "in all".to_owned(),
            (None, _) => String::new(),
        },
        lead: "The app's own folder: the libraries every scan fills, the search models, the logs and your F12 notes."
            .to_owned(),
        ticks: false,
        parts: parts
            .unwrap_or_default()
            .iter()
            .map(|p| Part {
                name: p.name.to_owned(),
                what: p.what.to_owned(),
                size: size_text(p.bytes),
                ..Part::default()
            })
            .collect(),
        note: String::new(),
        room: drive
            .filter(|d| d.here)
            .map(|d| format!("{} has {} free of {}.", d.name, size_text(d.free), size_text(d.total)))
            .unwrap_or_default(),
        warning: drive.map(|d| fill_note(d, bytes.unwrap_or(0))).unwrap_or_default(),
    }
}

/// The drive the app's own files are on, once looked at.
pub fn app_drive(drives: &[Drive]) -> Option<&Drive> {
    drives.iter().find(|d| d.holds.contains(&Holds::AppFiles))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    /// Made-up drives for made-up folders: C: nearly full, D: roomy, E: unplugged.
    fn fake(path: &Path) -> Found {
        let text = path.to_string_lossy();
        let (drive, free, total) = match text.get(..2) {
            Some("C:") => ("C:", 8 * GB, 500 * GB),
            Some("D:" | "d:") => ("D:", 1200 * GB, 3600 * GB),
            _ => return Found::Away(away_name(path, None)),
        };
        Found::Here { key: drive.to_owned(), room: DriveSpace { drive: drive.to_owned(), free, total } }
    }

    fn sample_drives() -> Vec<Drive> {
        let roots = [r"D:\Music\FL".to_owned(), r"d:\Music\Bitwig".to_owned()];
        let samples = [r"D:\Samples".to_owned(), r"C:\Users\me\Splice".to_owned()];
        let folders =
            folders(&roots, &samples, Some(Path::new(r"E:\Backup")), Path::new(r"C:\Users\me\AppData\Local\MO"));
        drives(&folders, fake)
    }

    #[test]
    fn each_drive_once_with_what_it_holds() {
        let d = sample_drives();
        let names: Vec<&str> = d.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["C:", "D:", "E:"]);
        assert_eq!(d[0].holds, [Holds::Samples, Holds::AppFiles]);
        assert_eq!(holds_line(&d[0]), "Samples and the app's files");
        assert_eq!(holds_line(&d[1]), "Projects and samples");
        // The backup drive isn't plugged in: it still shows, by the letter in its path.
        assert!(!d[2].here);
        assert_eq!(room_line(&d[2]), "Not plugged in");
        assert_eq!(holds_line(&d[2]), "The backup");
        assert_eq!(room_line(&d[1]), "1.17 TB free of 3.52 TB");
        assert!((d[1].used() - 2.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn two_disks_with_one_name_stay_two_and_one_disk_stays_one() {
        // A Mac or Linux names disks after where they're mounted, so two can share a name: their devices differ.
        let disk = |key: &str, free: u64| Found::Here {
            key: key.to_owned(),
            room: DriveSpace { drive: "Samples".to_owned(), free: free * GB, total: 1000 * GB },
        };
        let folders = [
            (Holds::Samples, PathBuf::from("/media/me/Samples")),
            (Holds::Projects, PathBuf::from("/mnt/nas/Samples")),
            (Holds::Projects, PathBuf::from("/media/me/Samples/Songs")),
        ];
        let d = drives(&folders, |p| if p.starts_with("/mnt") { disk("device 2", 300) } else { disk("device 1", 100) });
        assert_eq!(d.len(), 2);
        assert_eq!(d.iter().map(|d| d.free / GB).collect::<Vec<_>>(), [100, 300]);
        assert_eq!(d[0].holds, [Holds::Projects, Holds::Samples]);
    }

    #[test]
    fn a_drive_that_is_away_is_named_from_the_path() {
        assert_eq!(away_name(Path::new(r"e:\Backup\Music"), None), "E:");
        assert_eq!(away_name(Path::new(r"\\NAS\Music\Projects"), None), r"\\NAS\Music");
        assert_eq!(away_name(Path::new("/Volumes/Samples/Loops"), Some(Path::new("/Volumes"))), "Samples");
        assert_eq!(away_name(Path::new("somewhere"), None), "somewhere");
    }

    #[test]
    fn a_folder_that_has_gone_still_shows_on_its_drive() {
        // This crate's own folder is there; a folder in it that isn't is on the same drive, not "Not plugged in".
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let gone = here.join("no such folder").join("deeper");
        let (Found::Here { key: a, .. }, Found::Here { key: b, .. }) = (look(here), look(&gone)) else {
            panic!("the crate's own drive should be there");
        };
        assert_eq!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn the_folders_drives_are_mounted_in_are_known() {
        assert!(holds_mounts(Path::new("/Volumes")));
        assert!(holds_mounts(Path::new("/media/me")));
        assert!(holds_mounts(Path::new("/run/media/me")));
        assert!(!holds_mounts(Path::new("/home/me")));
        assert!(!holds_mounts(Path::new("/")));
    }

    #[test]
    fn a_folder_named_twice_counts_once() {
        let f = folders(&[r"D:\a".to_owned(), r"D:\a".to_owned()], &[], None, Path::new(r"C:\mo"));
        assert_eq!(f, [(Holds::Projects, PathBuf::from(r"D:\a")), (Holds::AppFiles, PathBuf::from(r"C:\mo"))]);
    }

    #[test]
    fn nearly_full_says_what_a_full_drive_stops() {
        let d = sample_drives();
        let app = 3 * GB;
        assert_eq!(d[0].fill(app), Fill::Nearly);
        assert_eq!(
            fill_note(&d[0], app),
            "Nearly full: 8 GB left. Once it's full, the library can't save what scans find. Free some room soon."
        );
        assert_eq!(d[1].fill(app), Fill::Roomy);
        assert_eq!(fill_note(&d[1], app), "");
        assert_eq!(summary(&d, app), "C: is nearly full: 8 GB left");
        // Under 2 GB it's almost full; a drive without the app's files says only how much is left.
        let almost = Drive { name: "K:".into(), free: GB, total: 2000 * GB, here: true, holds: vec![Holds::Projects] };
        assert_eq!(almost.fill(app), Fill::Almost);
        assert_eq!(fill_note(&almost, app), "Almost full: 1 GB left. Free some room now.");
        assert_eq!(summary(&[almost], app), "K: is almost full: 1 GB left");
    }

    #[test]
    fn the_apps_own_drive_is_short_once_its_files_outgrow_what_is_left() {
        // 20 GB left counts as room, until the app's files take more than that.
        let c = Drive { name: "C:".into(), free: 20 * GB, total: 500 * GB, here: true, holds: vec![Holds::AppFiles] };
        assert_eq!(c.fill(GB), Fill::Roomy);
        assert_eq!(c.fill(30 * GB), Fill::Nearly);
        // The same room on a drive without them is fine.
        let d = Drive { holds: vec![Holds::Samples], ..c };
        assert_eq!(d.fill(30 * GB), Fill::Roomy);
    }

    #[test]
    fn with_room_everywhere_the_summary_adds_it_up() {
        let d = Drive { name: "D:".into(), free: 600 * GB, total: 2000 * GB, here: true, holds: vec![Holds::Samples] };
        let k = Drive { name: "K:".into(), free: 300 * GB, ..d.clone() };
        assert_eq!(summary(&[d.clone(), k], GB), "900 GB free on 2 drives");
        assert_eq!(summary(std::slice::from_ref(&d), GB), "600 GB free on D:");
        assert_eq!(summary(&[], GB), "");
    }

    #[test]
    fn app_files_line_and_card() {
        let parts = [
            AppFilesPart { name: "Your library", what: "Projects, plugins, samples, tags and ratings", bytes: 6 * GB },
            AppFilesPart { name: "Logs", what: "What the app did", bytes: 100 << 20 },
        ];
        assert_eq!(app_files_line(None, Some("C:")).size, "Measuring…");
        let line = app_files_line(Some(&parts), Some("C:"));
        assert_eq!(
            (line.why.as_str(), line.size.as_str()),
            ("The libraries, search models, logs and notes, on C:", "6.1 GB")
        );
        let d = sample_drives();
        let card = app_files_card(Some(&parts), app_drive(&d));
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("6.1 GB", "in all, on C:"));
        let logs = &card.parts[1];
        assert_eq!(
            (logs.name.as_str(), logs.what.as_str(), logs.size.as_str()),
            ("Logs", "What the app did", "100 MB")
        );
        assert!(!card.ticks && logs.tick.is_none());
        assert_eq!(card.room, "C: has 8 GB free of 500 GB.");
        assert!(card.warning.starts_with("Nearly full: 8 GB left."));
        let measuring = app_files_card(None, None);
        assert_eq!((measuring.big.as_str(), measuring.big_note.as_str()), ("Measuring…", ""));
        assert!(measuring.parts.is_empty() && measuring.room.is_empty());
    }

    #[test]
    fn the_biggest_win_comes_first() {
        let line = |id: &'static str| Line { id, title: "", why: String::new(), size: String::new() };
        let lines = biggest_first(vec![(2 * GB, line("small")), (60 * GB, line("big")), (2 * GB, line("also small"))]);
        let ids: Vec<&str> = lines.iter().map(|l| l.id).collect();
        // The two that free the same stay in the order they came.
        assert_eq!(ids, ["big", "small", "also small"]);
    }

    #[test]
    fn a_list_reads_as_a_sentence() {
        assert_eq!(and_list::<&str>(&[]), "");
        assert_eq!(and_list(&["VST3"]), "VST3");
        assert_eq!(and_list(&["VST3", "CLAP"]), "VST3 and CLAP");
        assert_eq!(and_list(&["VST2".to_owned(), "VST3".to_owned(), "CLAP".to_owned()]), "VST2, VST3 and CLAP");
    }

    #[test]
    fn the_note_under_the_wins_says_why_a_kind_has_no_line() {
        assert_eq!(wins_note::<&str>(&[], 2), "");
        assert_eq!(wins_note(&["", ""], 1), "");
        let nones = ["", "No spare plugin copies were found. ", "Your songs have nothing to compact. "];
        assert_eq!(wins_note(&nones, 1), "No spare plugin copies were found. Your songs have nothing to compact.");
        assert_eq!(wins_note(&nones, 0), "No spare plugin copies were found. Your songs have nothing to compact.");
        // No line and no kind says why: the page still says something.
        assert_eq!(wins_note(&["", ""], 0), NOTHING_TO_FREE);
    }
}
