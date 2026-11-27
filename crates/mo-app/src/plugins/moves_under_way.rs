//! The Plugins page's moves under way, noted in the data folder before each copies (review E2-3): a move the app's
//! closing, a crash or the computer stopping cuts off part way leaves a half copy on the other drive (marked with
//! `mo_space::half_copy::MARKER`), and the move never reaches the library's moved folders, so nothing else knows where
//! to look for it. The next look ([`cut_off`]) finds each half copy in the folders these moves went to, for the page to
//! offer clearing; a note whose move left nothing there goes then (one whose folder can't be read now, its drive not
//! plugged in, stays for when it's back: review P-3). A move takes its own note out once the library's moved folders
//! keep it and its copy holds no marker ([`settled`]), and the file goes with the last one (rule 28). A move cut off
//! after its swap (the folder is a link to its copy, its old copy beside it) but before the library kept it is kept at
//! the next look instead ([`keep_swapped`]): its note says whose folder it was, and stays until it's kept. Notes are by
//! the folder moved and where it went, so a move again of the same folder to another place keeps where the first
//! went.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, PoisonError};

use mo_space::Stopped;
use mo_space::folder_move::{self, Drives, MovedFolder, SystemDrives};
use mo_space::half_copy::{self, Cleared, HalfCopy};
use mo_space::plugin_space::SpaceKind;
use serde::{Deserialize, Serialize};

/// The file, in the data folder. Written whole ([`crate::backup::write_whole`]), so it's on that list.
pub const FILE_NAME: &str = "plugin-moves-under-way.json";

/// One move under way: the folder being moved, and the folder picked on the other drive (its copy goes in there).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnderWay {
    pub from: String,
    pub destination: String,
    /// When it was noted (Unix seconds), or first looked at, for a note written before that (0).
    #[serde(default)]
    pub since: i64,
    /// When its folder was first found it couldn't be looked in (Unix seconds), 0 while it can: one that can't be
    /// looked in for [`UNREADABLE_FOR`] from then goes (rule 28), and the time starts again each time it's back. Not
    /// from when it was noted: a note kept for months beside its half copy went the first time its drive was found
    /// unplugged, and the half copy was never offered again (the whole-app review of 14 Nov, AP-11).
    #[serde(default)]
    pub away_since: i64,
    /// Whose folder it is, for its record in the library's moved folders should the move be cut off after its swap
    /// ([`swapped_unkept`]). None in a note written before 21 Nov 2026.
    #[serde(default)]
    pub whose: Option<Whose>,
}

/// Whose folder a move moves, as `mo_space::folder_move::move_folder` takes it: the plugin's name key, what the folder
/// holds, the other plugins it's also the folder of; and how big it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whose {
    pub key: String,
    pub kind: SpaceKind,
    pub shared_with: Vec<String>,
    pub bytes: u64,
}

/// How long a note whose folder can't be looked in (its drive not plugged in) is kept, for when it's back.
pub const UNREADABLE_FOR: i64 = 90 * 24 * 60 * 60;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The file in the app's data folder.
pub fn file() -> PathBuf {
    crate::paths::data_dir().join(FILE_NAME)
}

/// One change at a time to the file, from any thread.
static CHANGING: Mutex<()> = Mutex::new(());

/// The moves noted in `file`. None when it isn't there; one that can't be read is said in the log and counts as none:
/// all that's lost is finding a half copy (a move again of the same folder to the same place still finds its own).
pub fn under_way(file: &Path) -> Vec<UnderWay> {
    match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            crate::log::warning("Plugins", &format!("The moves under way in {} couldn't be read: {e}", file.display()));
            Vec::new()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            crate::log::warning("Plugins", &format!("The moves under way in {} couldn't be read: {e}", file.display()));
            Vec::new()
        }
    }
}

/// Writes `moves` to `file`, or removes it when there are none.
fn write(file: &Path, moves: &[UnderWay]) -> std::io::Result<()> {
    if moves.is_empty() {
        return match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(moves).map_err(std::io::Error::other)?;
    crate::backup::write_whole(file, &text)
}

fn same(a: &str, b: &str) -> bool {
    mo_space::key(a) == mo_space::key(b)
}

/// Whether two notes are of the same move: the same folder into the same place, noted at the same time. Not by when
/// its folder went away ([`UnderWay::away_since`]), which another look may have noted meanwhile.
fn same_move(a: &UnderWay, b: &UnderWay) -> bool {
    same(&a.from, &b.from) && same(&a.destination, &b.destination) && a.since == b.since
}

/// Notes a move about to copy `from` (`whose` it is) into `destination` (once for each folder and place).
pub fn start(file: &Path, from: &str, destination: &str, whose: Whose) -> std::io::Result<()> {
    let _one = CHANGING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut moves = under_way(file);
    moves.retain(|m| !(same(&m.from, from) && same(&m.destination, destination)));
    moves.push(UnderWay {
        from: from.to_owned(),
        destination: destination.to_owned(),
        since: now(),
        away_since: 0,
        whose: Some(whose),
    });
    write(file, &moves)
}

/// Takes out the note of the move of `from` into `destination`: it finished and the library's moved folders keep it
/// (or it left nothing). A note of an earlier move of it to another place stays.
pub fn finished(file: &Path, from: &str, destination: &str) -> std::io::Result<()> {
    let _one = CHANGING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut moves = under_way(file);
    let before = moves.len();
    moves.retain(|m| !(same(&m.from, from) && same(&m.destination, destination)));
    if moves.len() == before {
        return Ok(());
    }
    write(file, &moves)
}

/// A move that finished and that the library's moved folders keep: its note goes, unless its copy (`copy`) still
/// holds its marker (taking it out gave up: a virus checker held the file, say). Then the note stays, so the next look
/// ([`cut_off`]) finds the marker and the page's Clear takes it out: nothing else ever looks at that copy again
/// (rule 28; the whole-app review of 19 Nov, PM-3). Gives whether the note went.
pub fn settled(file: &Path, from: &str, destination: &str, copy: &str) -> std::io::Result<bool> {
    if std::fs::symlink_metadata(Path::new(copy).join(half_copy::MARKER)).is_ok() {
        return Ok(false);
    }
    finished(file, from, destination).map(|()| true)
}

/// The moves noted in `file` that swapped their folder for its link but that the library's moved folders (`moved`)
/// don't keep, each with its record to keep: the app or the computer stopped between the swap and keeping the move,
/// or the swap's administrator step finished after the app had gone (the whole-app review of 19 Nov, PM-4). Only
/// while the folder is a link to its copy where the move went, and its old copy is beside it
/// (`mo_space::folder_move::swapped_unkept`); not for a move `running` now (it keeps its own). The record's time is
/// when the move started. Looks at the disk: call it away from the window's thread.
pub fn swapped_unkept(file: &Path, moved: &[MovedFolder], running: &[String]) -> Vec<(UnderWay, MovedFolder)> {
    under_way(file)
        .into_iter()
        .filter(|m| !running.iter().any(|r| same(r, &m.from)))
        .filter_map(|m| {
            let record = unkept_record(&m, moved)?;
            Some((m, record))
        })
        .collect()
}

/// The record to keep for a noted move that swapped its folder for its link but that the library's moved folders
/// (`moved`) don't keep ([`swapped_unkept`]), or `None`.
fn unkept_record(m: &UnderWay, moved: &[MovedFolder]) -> Option<MovedFolder> {
    if moved.iter().any(|k| same(&k.from, &m.from)) {
        return None;
    }
    let started = chrono::DateTime::from_timestamp(m.since, 0)
        .filter(|_| m.since > 0)
        .map_or_else(chrono::Local::now, |t| t.with_timezone(&chrono::Local));
    let (key, kind, shared_with, bytes) = match &m.whose {
        Some(w) => (w.key.as_str(), w.kind, w.shared_with.as_slice(), w.bytes),
        // A note from before notes said whose: kept all the same, as a move the C# app kept is.
        None => ("", SpaceKind::Sounds, &[][..], 0),
    };
    folder_move::swapped_unkept(&m.from, &m.destination, bytes, (key, kind, shared_with), started)
}

/// Keeps the moves noted in `file` that were cut off after their swap but that the library's moved folders (`moved`)
/// don't keep ([`swapped_unkept`]), as each move keeps itself once it's done: `keep` adds each to the library's moved
/// folders, and each kept one is added to `moved`. Then the marker the move would have taken out of its copy goes (only
/// that: the copy is in use), and then its note ([`settled`]). One `keep` can't keep now (the library is busy) stays
/// noted, and [`cut_off`] leaves its note too, for the next look. Gives the ones kept, for the page to say so. The
/// whole-app review of 19 Nov, PM-4. Writes through `keep` and takes out markers: call it away from the window's
/// thread.
pub fn keep_swapped(
    file: &Path,
    moved: &mut Vec<MovedFolder>,
    running: &[String],
    keep: &dyn Fn(&MovedFolder) -> Result<(), String>,
) -> Vec<MovedFolder> {
    // One look at a time keeps them: two at once would both keep the same move.
    static KEEPING: Mutex<()> = Mutex::new(());
    let _one = KEEPING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut kept = Vec::new();
    for (noted, record) in swapped_unkept(file, moved, running) {
        if let Err(why) = keep(&record) {
            crate::log::warning(
                "Plugins",
                &format!("Move: {} was cut off after its swap, and couldn't be kept yet: {why}", record.from),
            );
            continue;
        }
        crate::log::info(
            "Plugins",
            &format!("Move: {} to {} was cut off after its swap; kept now", record.from, record.to),
        );
        moved.push(record.clone());
        let copy = Path::new(&record.to);
        // With its move in `moved`, the copy counts as in use: only its marker can go, never the copy.
        if std::fs::symlink_metadata(copy.join(half_copy::MARKER)).is_ok() {
            let never = AtomicBool::new(false);
            match half_copy::clear(copy, moved, &SystemDrives, &never) {
                Ok(Cleared::Marker) => {
                    crate::log::info("Plugins", &format!("Took the marker out of {}: it's in use", record.to));
                }
                Ok(Cleared::Copy(_)) => {
                    crate::log::warning("Plugins", &format!("Cleared {} as a half copy", record.to))
                }
                Err(why) => crate::log::warning("Plugins", &format!("The marker in {} stays: {why}", record.to)),
            }
        }
        if let Err(e) = settled(file, &noted.from, &noted.destination, &record.to) {
            crate::log::warning("Plugins", &format!("Move: the move under way's note couldn't be taken out: {e}"));
        }
        kept.push(record);
    }
    kept
}

/// The half copies the moves noted in `file` left, in the folders they went to (and where each moved folder's copy is,
/// `moved`, as `half_copy::half_copies` looks), each the copy of a noted move. A note whose move left nothing in the
/// folder it went to goes, unless that move is `running` now (the folders being moved, in this run), was noted while
/// this looked, swapped its folder for its link but isn't kept yet ([`keep_swapped`] couldn't keep it: the whole-app
/// review of 19 Nov, PM-4; told by the link and its old copy alone, PG-1 of 26 Nov), or that folder couldn't be read
/// (its drive not plugged in, no rights: review P-3). Reads and measures them: call it away from the window's thread.
pub fn cut_off(
    file: &Path,
    moved: &[MovedFolder],
    running: &[String],
    drives: &dyn Drives,
    stop: &AtomicBool,
) -> Result<Vec<HalfCopy>, Stopped> {
    let noted = under_way(file);
    if noted.is_empty() {
        return Ok(Vec::new());
    }
    let places: Vec<String> = noted.iter().map(|m| m.destination.clone()).collect();
    let mut found = half_copy::half_copies(moved, &places, drives, stop)?;
    found.retain(|h| noted.iter().any(|m| same(&h.from, &m.from)));
    // Looked at before the lock: a place that can't be reached may take a while to say so.
    let unreadable: Vec<String> =
        noted.iter().filter(|m| !looked_in(&m.destination)).map(|m| mo_space::key(&m.destination)).collect();
    // Swapped for its link, but not kept yet: the look that keeps it needs its note. Only by what's there (a link,
    // its old copy beside it), not where the link leads: that check can fail on a link that works (Solomon's log of
    // 25 Nov), and then the note was dropped, the move recorded nowhere (the whole-app review of 26 Nov, PG-1).
    // [`keep_swapped`] asks it, and tries again at each look until it can keep the move.
    let swapped: Vec<String> = noted
        .iter()
        .filter(|m| !moved.iter().any(|k| same(&k.from, &m.from)) && folder_move::looks_swapped(&m.from))
        .map(|m| mo_space::key(&m.from))
        .collect();
    let now = now();
    let _one = CHANGING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut moves = under_way(file);
    let before = moves.clone();
    moves.retain_mut(|m| {
        // Noted while this looked (a move started, or started again): its folder wasn't looked in for it.
        if !noted.iter().any(|n| same_move(n, m)) {
            return true;
        }
        let away = unreadable.contains(&mo_space::key(&m.destination));
        if !away {
            m.away_since = 0;
        } else if m.away_since == 0 {
            m.away_since = now;
        }
        if running.iter().any(|r| same(r, &m.from))
            || swapped.contains(&mo_space::key(&m.from))
            || found.iter().any(|h| same(&h.from, &m.from) && mo_library::scan::is_inside(&h.path, &m.destination))
        {
            return true;
        }
        if away && now.saturating_sub(m.away_since) > UNREADABLE_FOR {
            crate::log::info(
                "Plugins",
                &format!("A cut-off move's note to {} goes: it couldn't be looked in for 3 months", m.destination),
            );
            return false;
        }
        away
    });
    for m in moves.iter_mut().filter(|m| m.since == 0) {
        m.since = now;
    }
    if moves != before
        && let Err(e) = write(file, &moves)
    {
        crate::log::warning("Plugins", &format!("The moves under way couldn't be written: {e}"));
    }
    Ok(found)
}

/// Whether the folder a move went to could be looked in now: it's there and can be read, or it isn't there and its
/// drive is (so nothing was left in it).
fn looked_in(destination: &str) -> bool {
    match std::fs::read_dir(destination) {
        Ok(_) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound && super::folder_fixes::drive_there(Path::new(destination)),
    }
}

/// What the page says of the half copies found: the copies cut off part way, how big, where; a finished move's
/// leftover marker (only that goes); and a copy that stays, and why. Before them, the moves cut off after their swap
/// that a look kept ([`keep_swapped`]). "" for none.
pub fn found_text(found: &[HalfCopy], kept: &[MovedFolder]) -> String {
    use mo_space::half_copy::State;
    let mut parts = Vec::new();
    for m in kept {
        parts.push(format!(
            "{} moved to {}, but the move wasn't noted at first. It's noted now, and its old copy was added to the \
             Space page.",
            m.from, m.to
        ));
    }
    let cut: Vec<&HalfCopy> = found.iter().filter(|h| h.state == State::CutOff).collect();
    match cut.as_slice() {
        [] => {}
        [one] => parts.push(format!(
            "A move was cut off part way (the app or the computer stopped): {} is a half copy of {} ({}). Clear it \
             removes it for good; {} wasn't changed.",
            one.path,
            mo_formats::paths::file_name(&one.from),
            mo_space::size_text(one.bytes),
            one.from
        )),
        many => parts.push(format!(
            "{} moves were cut off part way (the app or the computer stopped), leaving half copies ({}). Clear it \
             removes them for good; the folders they copy weren't changed.",
            many.len(),
            mo_space::size_text(many.iter().fold(0u64, |a, h| a.saturating_add(h.bytes)))
        )),
    }
    for h in found.iter().filter(|h| h.state == State::MarkerOnly) {
        parts.push(format!(
            "A finished move left its marker file in {}, which is in use: Clear it takes only that file out.",
            h.path
        ));
    }
    for h in found {
        if let State::Stays(why) = &h.state {
            parts.push(format!("A move cut off part way left {}, and it stays: {why}", h.path));
        }
    }
    parts.join(" ")
}

/// Whether any of the half copies found can be cleared (all of it, or only its leftover marker).
pub fn can_clear(found: &[HalfCopy]) -> bool {
    found.iter().any(|h| !matches!(h.state, mo_space::half_copy::State::Stays(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whose folder the moves in these tests move.
    fn kits() -> Whose {
        Whose { key: "kits".into(), kind: SpaceKind::Sounds, shared_with: vec!["drums".into()], bytes: 4 }
    }

    #[test]
    fn a_move_is_noted_once_and_the_file_goes_with_the_last() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-under-way-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(FILE_NAME);
        start(&file, "C:/Kits", "E:/", kits()).unwrap();
        start(&file, "c:\\kits\\", "e:/", kits()).unwrap();
        start(&file, "C:/Kits", "F:/", kits()).unwrap();
        start(&file, "C:/Other", "E:/", kits()).unwrap();
        let noted = under_way(&file);
        assert_eq!(noted.len(), 3, "the same folder to another place is noted too: {noted:?}");
        assert_eq!(noted[0].destination, "e:/");
        finished(&file, "C:/Kits", "F:/").unwrap();
        finished(&file, "C:/Nothing", "E:/").unwrap();
        assert_eq!(under_way(&file).len(), 2, "the move to E: is still noted");
        finished(&file, "C:/Kits", "E:/").unwrap();
        finished(&file, "C:/Other", "E:/").unwrap();
        assert!(!file.exists());
        // A damaged file counts as no moves.
        std::fs::write(&file, "not json").unwrap();
        assert!(under_way(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    /// A move of `d`/C/Kits into `d`/E cut off part way: a marked copy of some of its files in `d`/E/Kits. Returns
    /// the folder moved and where it went.
    fn half_copy_in(d: &Path) -> (PathBuf, PathBuf) {
        let _ = std::fs::remove_dir_all(d);
        let (from, to_dir) = (d.join("C").join("Kits"), d.join("E"));
        let copy = to_dir.join("Kits");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(from.join("a.wav"), b"kick").unwrap();
        std::fs::write(from.join("b.wav"), b"snare").unwrap();
        std::fs::write(copy.join("a.wav"), b"kick").unwrap();
        let marker = serde_json::json!({
            "From": s(&from), "To": s(&copy), "Real": "", "Settled": s(&copy),
            "Started": "2026-11-06T19:00:00.0000000+00:00", "InUse": false,
        });
        std::fs::write(copy.join(half_copy::MARKER), format!("A note.\n\n{marker}\n")).unwrap();
        (from, to_dir)
    }

    /// A move cut off part way: its half copy (a marked copy of some of its files) is found, its note stays, and the
    /// page offers clearing it.
    #[test]
    fn a_half_copy_a_cut_off_move_left_is_found_and_its_note_stays() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-half-{}", std::process::id()));
        let (from, to_dir) = half_copy_in(&d);
        let file = d.join(FILE_NAME);
        start(&file, &s(&from), &s(&to_dir), kits()).unwrap();
        start(&file, &s(&d.join("C").join("Other")), &s(&to_dir), kits()).unwrap();
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].state, mo_space::half_copy::State::CutOff, "{found:?}");
        let left = under_way(&file);
        assert_eq!((left.len(), &left[0].from, &left[0].destination), (1, &s(&from), &s(&to_dir)));
        assert!(can_clear(&found));
        assert!(found_text(&found, &[]).starts_with("A move was cut off part way"), "{}", found_text(&found, &[]));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A note whose move left no half copy goes at the next look, unless that move is running now.
    #[test]
    fn a_note_with_nothing_left_goes_unless_its_move_is_running() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-cut-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let to = d.join("E");
        std::fs::create_dir_all(to.join("Kits")).unwrap();
        let file = d.join(FILE_NAME);
        let to_s = to.to_string_lossy().into_owned();
        start(&file, "C:/Kits", &to_s, kits()).unwrap();
        start(&file, "C:/Running", &to_s, kits()).unwrap();
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &["C:/Running".to_owned()], &mo_space::folder_move::SystemDrives, &stop);
        assert!(found.unwrap().is_empty());
        let left = under_way(&file);
        assert_eq!((left.len(), left[0].from.as_str(), &left[0].destination), (1, "C:/Running", &to_s));
        assert_eq!(found_text(&[], &[]), "");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Review P-3: a note whose folder can't be read now (its drive isn't plugged in) stays for when it's back.
    #[test]
    fn a_note_on_a_drive_not_plugged_in_stays() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-unplugged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(FILE_NAME);
        let away = if cfg!(windows) { "W:\\mo-no-such-drive\\Dest" } else { "/mnt/mo-no-such-drive/Dest" };
        start(&file, "C:/Kits", away, kits()).unwrap();
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop);
        assert!(found.unwrap().is_empty());
        assert_eq!(under_way(&file).len(), 1);
        // Not for ever (rule 28): once it couldn't be looked in for 3 months, it goes.
        let old = now() - UNREADABLE_FOR - 1;
        let gone =
            UnderWay { from: "C:/Kits".into(), destination: away.into(), since: old, away_since: old, whose: None };
        write(&file, &[gone]).unwrap();
        cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert!(under_way(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A note is its move and when that started: one another look noted its folder away for meanwhile is still judged
    /// by this one; a move started again is another move, which this look couldn't have seen.
    #[test]
    fn a_note_is_its_move_and_when_it_started() {
        let note = UnderWay { from: "C:/Kits".into(), destination: "E:/".into(), since: 5, ..UnderWay::default() };
        assert!(same_move(&note, &UnderWay { away_since: 9, ..note.clone() }));
        assert!(!same_move(&note, &UnderWay { since: 6, ..note.clone() }));
        assert!(!same_move(&note, &UnderWay { destination: "F:/".into(), ..note.clone() }));
    }

    /// The whole-app review of 14 Nov, AP-11: the 3 months count from when its folder was first found it couldn't be
    /// looked in, not from the move. A note kept for months beside its half copy stays the first time its drive isn't
    /// plugged in, and the time away starts again once its folder can be looked in.
    #[test]
    fn the_months_away_count_from_when_the_drive_went_not_from_the_move() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-away-since-{}", std::process::id()));
        let (from, to_dir) = half_copy_in(&d);
        let file = d.join(FILE_NAME);
        let four_months_ago = now() - UNREADABLE_FOR - 30 * 24 * 60 * 60;
        let away = if cfg!(windows) { "W:\\mo-no-such-drive\\Dest" } else { "/mnt/mo-no-such-drive/Dest" };
        // Moved four months ago and kept since beside its half copy; now its drive isn't plugged in, the first time.
        let moved = UnderWay {
            from: "C:/Kits".into(),
            destination: away.into(),
            since: four_months_ago,
            ..UnderWay::default()
        };
        // And one away for four months, on a drive that's back now, its half copy there.
        let back = UnderWay {
            from: s(&from),
            destination: s(&to_dir),
            since: four_months_ago,
            away_since: four_months_ago,
            whose: None,
        };
        write(&file, &[moved, back]).unwrap();
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        let left = under_way(&file);
        assert_eq!(left.len(), 2, "both notes stay: {left:?}");
        assert!(now() - left[0].away_since < 60, "the time away starts now: {left:?}");
        assert_eq!((left[1].away_since, left[1].since), (0, four_months_ago), "back, so not away: {left:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A folder link, as a move makes (a junction on Windows: a symbolic link needs administrator rights there).
    fn link(at: &Path, to: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(to, at).unwrap();
        #[cfg(windows)]
        junction::create(to, at).unwrap();
    }

    /// The whole-app review of 19 Nov, PM-4: a move cut off after its swap (its folder a link to its copy, the old
    /// copy beside it) that the library's moved folders don't keep is kept at the next look, as whose it was and from
    /// when it started; not before its swap, nor one the library keeps or that's running now. A note from before notes
    /// said whose is kept all the same.
    #[test]
    fn a_move_cut_off_after_its_swap_is_kept() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-swapped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (from, to_dir) = (d.join("C").join("Kits"), d.join("E"));
        let (copy, aside) = (to_dir.join("Kits"), d.join("C").join("Kits (old copy)"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(from.join("a.wav"), b"kick").unwrap();
        std::fs::write(copy.join("a.wav"), b"kick").unwrap();
        let file = d.join(FILE_NAME);
        start(&file, &s(&from), &s(&to_dir), kits()).unwrap();
        assert!(swapped_unkept(&file, &[], &[]).is_empty(), "not swapped yet: the copy may be a half copy");
        // The swap: the folder set aside as its old copy, a link to the copy in its place.
        std::fs::rename(&from, &aside).unwrap();
        link(&from, &copy);
        let kept = swapped_unkept(&file, &[], &[]);
        assert_eq!(kept.len(), 1, "{kept:?}");
        let (note, record) = &kept[0];
        assert_eq!(note, &under_way(&file)[0]);
        let want = (s(&from), s(&copy), s(&aside), 4, "kits", 2, vec!["drums".to_owned()]);
        let got = (
            record.from.clone(),
            record.to.clone(),
            record.old_copy.clone(),
            record.bytes,
            record.key.as_str(),
            record.kind,
            record.shared_with.clone(),
        );
        assert_eq!(got, want);
        let when = chrono::DateTime::parse_from_rfc3339(&record.moved).unwrap();
        assert_eq!(when.timestamp(), note.since, "the record's time is when the move started: {}", record.moved);
        // Not one the library keeps, or that's running now (it keeps its own).
        assert!(swapped_unkept(&file, std::slice::from_ref(record), &[]).is_empty());
        assert!(swapped_unkept(&file, &[], &[s(&from)]).is_empty());
        // A note from before notes said whose: kept all the same, as a move the C# app kept is.
        let old = UnderWay { since: 0, whose: None, ..note.clone() };
        write(&file, &[old]).unwrap();
        let kept = swapped_unkept(&file, &[], &[]);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!((kept[0].1.key.as_str(), kept[0].1.kind, kept[0].1.bytes), ("", 2, 0));
        assert!(found_text(&[], &[kept[0].1.clone()]).contains("but the move wasn't noted at first"));
        // Its link gone (put back by hand, say): nothing to keep.
        std::fs::remove_file(&from).or_else(|_| std::fs::remove_dir(&from)).unwrap();
        assert!(swapped_unkept(&file, &[], &[]).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A move of `d`/C/`name` into `d`/E cut off after its swap: the folder set aside as its old copy, a link to its
    /// copy in its place, noted in `file`. Gives the folder moved and its copy.
    fn swapped_in(d: &Path, file: &Path, name: &str) -> (PathBuf, PathBuf) {
        let (from, to_dir) = (d.join("C").join(name), d.join("E"));
        let (copy, aside) = (to_dir.join(name), d.join("C").join(format!("{name} (old copy)")));
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(aside.join("a.wav"), b"kick").unwrap();
        std::fs::write(copy.join("a.wav"), b"kick").unwrap();
        link(&from, &copy);
        start(file, &s(&from), &s(&to_dir), kits()).unwrap();
        (from, copy)
    }

    /// The marker a move leaves in its copy while the copy is going into use.
    fn mark_in_use(from: &Path, copy: &Path) {
        let marker = serde_json::json!({
            "From": s(from), "To": s(copy), "Real": "", "Settled": s(copy),
            "Started": "2026-11-21T01:00:00.0000000+00:00", "InUse": true,
        });
        std::fs::write(copy.join(half_copy::MARKER), format!("A note.\n\n{marker}\n")).unwrap();
    }

    /// The whole-app review of 19 Nov, PM-4: a move cut off after its swap keeps its note until the library keeps it.
    /// A look whose keep failed (the library busy) doesn't drop it, though the copy holds no marker any more (the move
    /// took it out), so the next look still keeps the move; once kept, the note goes. A kept move's copy loses the
    /// marker the move would have taken out, and only that.
    #[test]
    fn a_move_cut_off_after_its_swap_stays_noted_until_its_kept() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-keep-swapped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let file = d.join(FILE_NAME);
        let (from, copy) = swapped_in(&d, &file, "Kits");
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(under_way(&file).len(), 1, "swapped but not kept: its note stays for the look that keeps it");
        // The library is busy: nothing kept, the note stays, and the next look's cut_off leaves it too.
        let tries = std::cell::Cell::new(0);
        let busy = |_: &MovedFolder| -> Result<(), String> {
            tries.set(tries.get() + 1);
            Err("database is locked".into())
        };
        let mut moved = Vec::new();
        assert!(keep_swapped(&file, &mut moved, &[], &busy).is_empty());
        assert_eq!((tries.get(), moved.len(), under_way(&file).len()), (1, 0, 1));
        cut_off(&file, &moved, &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert_eq!(under_way(&file).len(), 1);
        // Not while its move is running: that move keeps itself.
        let keeps = std::cell::RefCell::new(Vec::new());
        let keep = |m: &MovedFolder| -> Result<(), String> {
            keeps.borrow_mut().push(m.clone());
            Ok(())
        };
        assert!(keep_swapped(&file, &mut moved, &[s(&from)], &keep).is_empty());
        assert!(keeps.borrow().is_empty());
        // Kept: in `moved` now, and its note goes.
        let kept = keep_swapped(&file, &mut moved, &[], &keep);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(*keeps.borrow(), kept);
        assert_eq!(moved, kept);
        assert_eq!((kept[0].from.as_str(), kept[0].to.as_str()), (s(&from).as_str(), s(&copy).as_str()));
        assert!(under_way(&file).is_empty());
        // Another, whose copy still holds the marker going into use left: only the marker goes, then its note.
        let (pads, pads_copy) = swapped_in(&d, &file, "Pads");
        mark_in_use(&pads, &pads_copy);
        let kept = keep_swapped(&file, &mut moved, &[], &keep);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(!pads_copy.join(half_copy::MARKER).exists(), "the marker went");
        assert!(pads_copy.join("a.wav").exists(), "the copy stays: it's in use");
        assert!(under_way(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The whole-app review of 26 Nov, PG-1: a move cut off after its swap keeps its note while its folder is a link
    /// with its old copy beside it, even while the check of where the link leads fails (as it did on Solomon's PC on
    /// 25 Nov): the note is then the move's only record. Nothing is kept until that check works; then it is, and the
    /// note goes.
    #[test]
    fn a_swapped_moves_note_stays_while_its_link_cant_be_checked() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-unchecked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let file = d.join(FILE_NAME);
        let (from, copy) = swapped_in(&d, &file, "Kits");
        // The link leads somewhere the check doesn't take for its copy.
        let elsewhere = d.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::remove_file(&from).or_else(|_| std::fs::remove_dir(&from)).unwrap();
        link(&from, &elsewhere);
        assert!(unkept_record(&under_way(&file)[0], &[]).is_none(), "the check of where it leads fails");
        let stop = AtomicBool::new(false);
        for _ in 0..2 {
            cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
            assert_eq!(under_way(&file).len(), 1, "its note stays");
        }
        let keep = |_: &MovedFolder| -> Result<(), String> { Ok(()) };
        let mut moved = Vec::new();
        assert!(keep_swapped(&file, &mut moved, &[], &keep).is_empty(), "not kept while it can't be checked");
        assert_eq!(under_way(&file).len(), 1);
        // The check works again: kept, and the note goes.
        std::fs::remove_file(&from).or_else(|_| std::fs::remove_dir(&from)).unwrap();
        link(&from, &copy);
        assert_eq!(keep_swapped(&file, &mut moved, &[], &keep).len(), 1);
        assert!(under_way(&file).is_empty());
        // A note whose folder isn't a link any more (put back by hand) goes at the next look, as before.
        let (pads, _) = swapped_in(&d, &file, "Pads");
        std::fs::remove_file(&pads).or_else(|_| std::fs::remove_dir(&pads)).unwrap();
        std::fs::create_dir_all(&pads).unwrap();
        cut_off(&file, &moved, &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert!(under_way(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The whole-app review of 19 Nov, PM-3: a finished move's note stays while its copy still holds its marker (taking
    /// it out gave up), so the next look finds the marker and Clear takes it out; then the note goes.
    #[test]
    fn a_finished_moves_note_stays_while_its_copy_holds_its_marker() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-settled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let file = d.join(FILE_NAME);
        let (from, copy) = swapped_in(&d, &file, "Kits");
        let to_dir = s(&d.join("E"));
        mark_in_use(&from, &copy);
        assert!(!settled(&file, &s(&from), &to_dir, &s(&copy)).unwrap());
        assert_eq!(under_way(&file).len(), 1);
        // The look finds the marker, as a finished move's leftover in a copy in use.
        let record = unkept_record(&under_way(&file)[0], &[]).unwrap();
        let stop = AtomicBool::new(false);
        let moved = std::slice::from_ref(&record);
        let found = cut_off(&file, moved, &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].state, mo_space::half_copy::State::MarkerOnly, "{found:?}");
        assert_eq!(under_way(&file).len(), 1, "its note stays while the marker is there");
        std::fs::remove_file(copy.join(half_copy::MARKER)).unwrap();
        assert!(settled(&file, &s(&from), &to_dir, &s(&copy)).unwrap());
        assert!(under_way(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
