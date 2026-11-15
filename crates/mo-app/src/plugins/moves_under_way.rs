//! The Plugins page's moves under way, noted in the data folder before each copies (review E2-3): a move the app's
//! closing, a crash or the computer stopping cuts off part way leaves a half copy on the other drive (marked with
//! `mo_space::half_copy::MARKER`), and the move never reaches the library's moved folders, so nothing else knows where
//! to look for it. The next look ([`cut_off`]) finds each half copy in the folders these moves went to, for the page to
//! offer clearing; a note whose move left nothing there goes then (one whose folder can't be read now, its drive not
//! plugged in, stays for when it's back: review P-3). A move that finishes takes its own note out, and the file goes
//! with the last one (rule 28). Notes are by the folder moved and where it went, so a move again of the same folder to
//! another place keeps where the first went.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use mo_space::Stopped;
use mo_space::folder_move::{Drives, MovedFolder};
use mo_space::half_copy::{self, HalfCopy};
use serde::{Deserialize, Serialize};

/// The file, in the data folder. Written whole ([`crate::backup::write_whole`]), so it's on that list.
pub const FILE_NAME: &str = "plugin-moves-under-way.json";

/// One move under way: the folder being moved, and the folder picked on the other drive (its copy goes in there).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Notes a move about to copy `from` into `destination` (once for each folder and place).
pub fn start(file: &Path, from: &str, destination: &str) -> std::io::Result<()> {
    let _one = CHANGING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut moves = under_way(file);
    moves.retain(|m| !(same(&m.from, from) && same(&m.destination, destination)));
    moves.push(UnderWay { from: from.to_owned(), destination: destination.to_owned(), since: now(), away_since: 0 });
    write(file, &moves)
}

/// Takes out the note of the move of `from` into `destination`: it finished, so its copy is in use (or it left
/// nothing). A note of an earlier move of it to another place stays.
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

/// The half copies the moves noted in `file` left, in the folders they went to (and where each moved folder's copy is,
/// `moved`, as `half_copy::half_copies` looks), each the copy of a noted move. A note whose move left nothing in the
/// folder it went to goes, unless that move is `running` now (the folders being moved, in this run), was noted while
/// this looked, or that folder couldn't be read (its drive not plugged in, no rights: review P-3). Reads and measures
/// them: call it away from the window's thread.
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
/// leftover marker (only that goes); and a copy that stays, and why. "" for none.
pub fn found_text(found: &[HalfCopy]) -> String {
    use mo_space::half_copy::State;
    let mut parts = Vec::new();
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

    #[test]
    fn a_move_is_noted_once_and_the_file_goes_with_the_last() {
        let d = std::env::temp_dir().join(format!("mo-app-moves-under-way-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join(FILE_NAME);
        start(&file, "C:/Kits", "E:/").unwrap();
        start(&file, "c:\\kits\\", "e:/").unwrap();
        start(&file, "C:/Kits", "F:/").unwrap();
        start(&file, "C:/Other", "E:/").unwrap();
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
        start(&file, &s(&from), &s(&to_dir)).unwrap();
        start(&file, &s(&d.join("C").join("Other")), &s(&to_dir)).unwrap();
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].state, mo_space::half_copy::State::CutOff, "{found:?}");
        let left = under_way(&file);
        assert_eq!((left.len(), &left[0].from, &left[0].destination), (1, &s(&from), &s(&to_dir)));
        assert!(can_clear(&found));
        assert!(found_text(&found).starts_with("A move was cut off part way"), "{}", found_text(&found));
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
        start(&file, "C:/Kits", &to_s).unwrap();
        start(&file, "C:/Running", &to_s).unwrap();
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &["C:/Running".to_owned()], &mo_space::folder_move::SystemDrives, &stop);
        assert!(found.unwrap().is_empty());
        let left = under_way(&file);
        assert_eq!((left.len(), left[0].from.as_str(), &left[0].destination), (1, "C:/Running", &to_s));
        assert_eq!(found_text(&[]), "");
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
        start(&file, "C:/Kits", away).unwrap();
        let stop = AtomicBool::new(false);
        let found = cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop);
        assert!(found.unwrap().is_empty());
        assert_eq!(under_way(&file).len(), 1);
        // Not for ever (rule 28): once it couldn't be looked in for 3 months, it goes.
        let old = now() - UNREADABLE_FOR - 1;
        let gone = UnderWay { from: "C:/Kits".into(), destination: away.into(), since: old, away_since: old };
        write(&file, &[gone]).unwrap();
        cut_off(&file, &[], &[], &mo_space::folder_move::SystemDrives, &stop).unwrap();
        assert!(under_way(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A note is its move and when that started: one another look noted its folder away for meanwhile is still judged
    /// by this one; a move started again is another move, which this look couldn't have seen.
    #[test]
    fn a_note_is_its_move_and_when_it_started() {
        let note = UnderWay { from: "C:/Kits".into(), destination: "E:/".into(), since: 5, away_since: 0 };
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
        let moved =
            UnderWay { from: "C:/Kits".into(), destination: away.into(), since: four_months_ago, away_since: 0 };
        // And one away for four months, on a drive that's back now, its half copy there.
        let back =
            UnderWay { from: s(&from), destination: s(&to_dir), since: four_months_ago, away_since: four_months_ago };
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
}
