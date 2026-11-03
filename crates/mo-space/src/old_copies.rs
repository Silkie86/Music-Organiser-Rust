//! The old copies moves leave beside each moved folder's link, and the other folders moves leave behind, each with its
//! size: what the Space page reminds you of every time, for as long as any is there (Solomon, 3 Nov 2026: "should
//! always remind about old copies", rather than only after two weeks). The C# app's card over the Plugins list
//! ("2 old copies left after moving folders").
//!
//! The moves and folders left behind come from the app's records. An old copy no move was kept for (the app stopped
//! between swapping the folder for its link and keeping the move, say) is found too, beside its link, in the folders
//! the app hands in: it's taken in as a move, so it goes the same guarded way as any other once you say. Nothing here
//! removes anything: [`crate::folder_move::remove_old_copy`] and [`crate::after_move::remove_old_home`] do that.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::NaiveDateTime;
use mo_formats::paths::file_name;

use crate::after_move::{OldHome, kept_beside, old_home_stays, real_place};
use crate::folder_move::{MovedFolder, OLD_SUFFIX, csharp_time, leads_into, old_copy_stays};
use crate::size::{self, Stopped, is_link, size_text};
use crate::{in_places, is_dir, key, not_there, parent_of, real_folder, real_path, text};

/// Something a move left for you to delete once its plugin opens fine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leftover {
    /// Where it is, as it really is (an old copy inside a moved folder is on the other drive).
    pub path: String,
    /// How big it is now.
    pub bytes: u64,
    /// What it is.
    pub what: Left,
    /// Where its files are now (the copy on the other drive), when that's known.
    pub now_in: String,
    /// When the folder moved, when that's known.
    pub since: Option<NaiveDateTime>,
    /// Why it can't go yet, when it can't. One that can still goes only once nothing is only in it, or you're sure.
    pub stays: Option<String>,
}

/// What a [`Leftover`] is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Left {
    /// The old copy of the moved folder at this place in the moves handed in.
    OldCopy(usize),
    /// The folder left behind at this place in the ones handed in.
    Behind(usize),
    /// Files kept aside that aren't in the folders left behind yet (a try that stopped part way kept them:
    /// [`crate::after_move::kept_beside`]), for the app to add to them.
    Kept(OldHome),
    /// An old copy beside a link that no move was kept for: the move as it's found, for the app to keep, so the old
    /// copy goes as any other does. Its time is when it was found; the plugin it's for isn't known.
    NotKept(MovedFolder),
    /// A folder called "… (old copy)" that no move was kept for, with no link beside it to a copy: it stays, for you to
    /// look at.
    Unknown,
}

/// Everything moves left for you to delete, biggest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reminder {
    pub leftovers: Vec<Leftover>,
}

impl Reminder {
    /// Whether there's nothing to remind you of.
    pub fn is_empty(&self) -> bool {
        self.leftovers.is_empty()
    }

    /// How much room they take together.
    pub fn bytes(&self) -> u64 {
        self.leftovers.iter().fold(0, |sum, l| sum.saturating_add(l.bytes))
    }

    /// What the reminder says: "1 old copy left after a move (12.3 GB)", "3 old copies left after moving folders
    /// (64.2 GB)"; nothing when there's none.
    pub fn title(&self) -> String {
        let size = size_text(self.bytes());
        match self.leftovers.len() {
            0 => String::new(),
            1 => format!("1 old copy left after a move ({size})"),
            n => format!("{n} old copies left after moving folders ({size})"),
        }
    }
}

/// Every old copy and folder left behind that's still there, each measured now, biggest first, with why one can't go
/// yet. `moved` and `homes` are the app's records. `look_in` is the folders the plugins' data folders are in (where
/// the app found them), to find old copies no move was kept for; the folder each moved folder is in is looked in too.
/// Reads and measures every one: call it away from the window's thread.
pub fn remind(
    moved: &[MovedFolder],
    homes: &[OldHome],
    look_in: &[String],
    stop: &AtomicBool,
) -> Result<Reminder, Stopped> {
    let mut leftovers: Vec<Leftover> = Vec::new();
    let measure = |path: &str| size::measure(Path::new(path), stop).map(|m| m.bytes);
    for (i, m) in moved.iter().enumerate() {
        // Gone, or a link now (it frees nothing).
        if !m.has_old_copy() {
            continue;
        }
        leftovers.push(Leftover {
            path: real_place(&m.old_copy, moved),
            bytes: measure(&m.old_copy)?,
            what: Left::OldCopy(i),
            now_in: m.to.clone(),
            since: moved_at(&m.moved),
            stays: old_copy_stays(m, moved),
        });
    }
    for (i, h) in homes.iter().enumerate() {
        if !real_folder(&h.path) {
            continue;
        }
        leftovers.push(Leftover {
            path: h.path.clone(),
            bytes: measure(&h.path)?,
            what: Left::Behind(i),
            now_in: h.copy.clone(),
            since: None,
            stays: old_home_stays(h, moved),
        });
    }
    for h in kept_beside(moved, homes, stop) {
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let stays = old_home_stays(&h, moved);
        leftovers.push(Leftover {
            path: h.path.clone(),
            bytes: h.bytes,
            what: Left::Kept(h),
            now_in: String::new(),
            since: None,
            stays,
        });
    }
    for found in not_kept(moved, homes, look_in, &leftovers, stop)? {
        leftovers.push(found);
    }
    leftovers.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| key(&a.path).cmp(&key(&b.path))));
    Ok(Reminder { leftovers })
}

/// When a move was, from the time the app keeps with it (the C# app's way: `2026-10-26T14:03:12.1234567+01:00`, or an
/// older record's without the zone), as the time it was here.
fn moved_at(when: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(when)
        .map(|t| t.with_timezone(&chrono::Local).naive_local())
        .or_else(|_| NaiveDateTime::parse_from_str(when, "%Y-%m-%dT%H:%M:%S%.f"))
        .ok()
}

/// The folders called "… (old copy)" in `look_in` (and in the folder each moved folder is in) that aren't listed yet:
/// beside a link to a folder, taken in as a move; otherwise left as they are, saying why.
fn not_kept(
    moved: &[MovedFolder],
    homes: &[OldHome],
    look_in: &[String],
    listed: &[Leftover],
    stop: &AtomicBool,
) -> Result<Vec<Leftover>, Stopped> {
    let mut places: Vec<String> = look_in.to_vec();
    places.extend(moved.iter().filter_map(|m| parent_of(&m.from)));
    // Every place known already, as written and as it really is.
    let mut known: Vec<String> = Vec::new();
    for path in listed.iter().map(|l| l.path.as_str()).chain(moved.iter().map(|m| m.old_copy.as_str())) {
        know(path, &mut known);
    }
    for h in homes {
        know(&h.path, &mut known);
    }
    let mut found: Vec<Leftover> = Vec::new();
    let mut adopted: Vec<MovedFolder> = Vec::new();
    for entry in in_places(places, stop)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = text(&entry.path());
        let Some(base) = old_copy_of(&path, &name) else { continue };
        // A real folder (an old copy that's a link frees nothing), not listed yet, and not a recorded move's.
        if !entry.file_type().is_ok_and(|t| t.is_dir())
            || known.contains(&key(&path))
            || real_path(&path).is_some_and(|r| known.contains(&key(&r)))
            || moved.iter().any(|m| key(&m.from) == key(&base))
        {
            continue;
        }
        know(&path, &mut known);
        let bytes = size::measure(Path::new(&path), stop)?.bytes;
        let unknown = |why: String| Leftover {
            path: path.clone(),
            bytes,
            what: Left::Unknown,
            now_in: String::new(),
            since: None,
            stays: Some(why),
        };
        let Some(to) = (is_link(Path::new(&base)) && is_dir(&base)).then(|| real_path(&base)).flatten() else {
            found.push(unknown(unknown_stays(&base)));
            continue;
        };
        let m = MovedFolder {
            from: base,
            to,
            old_copy: path.clone(),
            bytes,
            moved: csharp_time(chrono::Local::now()),
            key: String::new(),
            kind: 2,
            shared_with: Vec::new(),
        };
        // Its link leads into it (pointed back at it once the other drive went, say): it's what the plugin uses.
        if leads_into(&m) {
            let name = file_name(&m.from);
            found.push(unknown(format!(
                "{name} beside it is a link that leads into it, so it's what the plugin uses: it stays."
            )));
            continue;
        }
        adopted.push(m.clone());
        found.push(Leftover { path, bytes, now_in: m.to.clone(), since: None, stays: None, what: Left::NotKept(m) });
    }
    // Checked against every move, the ones taken in too: an old copy can hold another's.
    let all: Vec<MovedFolder> = moved.iter().chain(&adopted).cloned().collect();
    for f in &mut found {
        if let Left::NotKept(m) = &f.what {
            f.stays = old_copy_stays(m, &all);
        }
    }
    Ok(found)
}

/// Adds a place to those known, as written and as it really is.
fn know(path: &str, known: &mut Vec<String>) {
    known.push(key(path));
    if let Some(real) = real_path(path) {
        known.push(key(&real));
    }
}

/// The folder an old copy is the old copy of ("STEAM" beside "STEAM (old copy)"), if its name says it's one.
fn old_copy_of(path: &str, name: &str) -> Option<String> {
    let cut = name.len().checked_sub(OLD_SUFFIX.len())?;
    if cut == 0 || !name.is_char_boundary(cut) || !name[cut..].eq_ignore_ascii_case(OLD_SUFFIX) {
        return None;
    }
    let folder = Path::new(path).parent()?;
    Some(text(&folder.join(&name[..cut])))
}

/// Why an old copy with no move kept for it, and no link beside it to a copy, stays.
fn unknown_stays(base: &str) -> String {
    let name = file_name(base);
    if not_there(Path::new(base)) {
        return format!(
            "There's no {name} beside it: it may be {name} itself, renamed when a move was cut off as it swapped the \
             folder for its link. If so, rename it back to \"{name}\". Otherwise look in it before deleting it \
             yourself."
        );
    }
    format!(
        "No move was kept for it, and {name} beside it isn't a link to a copy, so the app can't tell whether anything \
         is only in it. Look in it before deleting it yourself."
    )
}
