//! The old copies moves leave beside each moved folder's link, and the other folders moves leave behind, each with its
//! size: what the Space page reminds you of every time, for as long as any is there (Solomon, 3 Nov 2026: "should
//! always remind about old copies", rather than only after two weeks). The C# app's card over the Plugins list
//! ("2 old copies left after moving folders").
//!
//! The moves and folders left behind come from the app's records. An old copy no move was kept for (the app stopped
//! between swapping the folder for its link and keeping the move, say) is found too, beside its link, in the folders
//! the app hands in: it's taken in as a move, but it stays until the app keeps that move, as once it's gone nothing
//! would show the link was a move's. Each one that can go says what's only in it (by sizes and times), so the page can
//! show that list before anything goes. [`remove`] sends the ones you pick, together, each checked again first;
//! [`crate::folder_move::remove_old_copy`] and [`crate::after_move::remove_old_home`] send one.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::NaiveDateTime;
use mo_formats::paths::file_name;
use rayon::prelude::*;

use crate::admin::real_spot;
use crate::after_move::{OldHome, kept_beside, old_home_check, old_home_stays, only_in_old_home, real_place};
use crate::clean::{Bin, Done, Problem, Sending, What};
use crate::folder_move::{
    MovedFolder, OLD_SUFFIX, csharp_time, leads_into, old_copy_check, old_copy_stays, only_in_old_copy,
};
use crate::plugin_space::SpaceKind;
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
    /// What's only in it as the look found it, as paths inside it: not in the copy its files went to, or newer here.
    /// Sizes and times only: what the files hold is compared just before it goes. `None` when there's nothing to check
    /// it against (files kept aside, or a folder left behind whose record doesn't say where its files went), and for
    /// one that stays anyway.
    pub only: Option<Vec<String>>,
}

impl Leftover {
    /// Whether it can go: nothing keeps it, and the look found nothing only in it (or nothing to check it against: then
    /// it goes once you're sure).
    pub fn can_go(&self) -> bool {
        self.stays.is_none() && self.only.as_ref().is_none_or(Vec::is_empty)
    }
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

    /// The ones that can go ([`Leftover::can_go`]).
    pub fn can_go(&self) -> impl Iterator<Item = &Leftover> {
        self.leftovers.iter().filter(|l| l.can_go())
    }

    /// How much room the ones that can go take together: what deleting them frees. One that stays isn't counted (the
    /// plugin may be using it), nor one with something only in it.
    pub fn bytes(&self) -> u64 {
        self.can_go().fold(0, |sum, l| sum.saturating_add(l.bytes))
    }

    /// What the reminder says, counting what can go and its room: "1 old copy left after a move (12.3 GB)", "3 old
    /// copies left after moving folders (64.2 GB), and 1 that can't go yet", "1 old copy left after a move can't go
    /// yet"; nothing when there's none.
    pub fn title(&self) -> String {
        let go = self.can_go().count();
        let staying = self.leftovers.len() - go;
        let left = |n: usize| match n {
            1 => "1 old copy left after a move".to_owned(),
            n => format!("{n} old copies left after moving folders"),
        };
        match (go, staying) {
            (0, 0) => String::new(),
            (0, n) => format!("{} can't go yet", left(n)),
            (n, 0) => format!("{} ({})", left(n), size_text(self.bytes())),
            (n, m) => format!("{} ({}), and {m} that can't go yet", left(n), size_text(self.bytes())),
        }
    }
}

/// Every old copy and folder left behind that's still there, each measured now, biggest first, with why one can't go
/// yet, and what's only in each one that can. `moved` and `homes` are the app's records. `look_in` is the folders the
/// plugins' data folders are in (where the app found them), to find old copies no move was kept for; the folder each
/// moved folder is in is looked in too. Reads and measures every one, and the copies their files went to: call it away
/// from the window's thread.
pub fn remind(
    moved: &[MovedFolder],
    homes: &[OldHome],
    look_in: &[String],
    stop: &AtomicBool,
) -> Result<Reminder, Stopped> {
    let measure = |path: &str| size::measure(Path::new(path), stop).map(|m| m.bytes);
    // Each one, and the copy its files went to, is read on its own (rule 21). One that's gone, or a link now (it frees
    // nothing), isn't there to remind you of.
    let old_copies = moved.par_iter().enumerate().filter(|(_, m)| m.has_old_copy()).map(|(i, m)| {
        let stays = old_copy_stays(m, moved);
        let only = match stays {
            None => Some(only_in_old_copy(m, moved, stop).map_err(|_| Stopped)?),
            Some(_) => None,
        };
        Ok(Leftover {
            path: real_place(&m.old_copy, moved),
            bytes: measure(&m.old_copy)?,
            what: Left::OldCopy(i),
            now_in: m.to.clone(),
            since: moved_at(&m.moved),
            stays,
            only,
        })
    });
    let behind = homes.par_iter().enumerate().filter(|(_, h)| real_folder(&h.path)).map(|(i, h)| {
        let stays = old_home_stays(h, moved);
        let only = match stays {
            None => only_in_old_home(h, stop).map_err(|_| Stopped)?,
            Some(_) => None,
        };
        Ok(Leftover {
            path: h.path.clone(),
            bytes: measure(&h.path)?,
            what: Left::Behind(i),
            now_in: h.copy.clone(),
            since: None,
            stays,
            only,
        })
    });
    let mut leftovers = old_copies.chain(behind).collect::<Result<Vec<Leftover>, Stopped>>()?;
    for h in kept_beside(moved, homes, stop) {
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        // Kept aside, they say nothing of where their files went: there's nothing to check them against.
        let stays = old_home_stays(&h, moved);
        leftovers.push(Leftover {
            path: h.path.clone(),
            bytes: h.bytes,
            what: Left::Kept(h),
            now_in: String::new(),
            since: None,
            stays,
            only: None,
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
            only: None,
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
        let now_in = m.to.clone();
        found.push(Leftover { path, bytes, now_in, since: None, stays: None, only: None, what: Left::NotKept(m) });
    }
    // Checked against every move, the ones taken in too: an old copy can hold another's. One that could go stays
    // anyway, until the app keeps its move.
    let all: Vec<MovedFolder> = moved.iter().chain(&adopted).cloned().collect();
    for f in &mut found {
        if let Left::NotKept(m) = &f.what {
            f.stays = Some(old_copy_stays(m, &all).unwrap_or_else(|| not_kept_stays(&m.from)));
        }
    }
    Ok(found)
}

/// Why an old copy no move was kept for stays, even when it could go: once it's gone, nothing would show the app that
/// the folder beside it was moved (to remind you of its link, or to move it back).
pub fn not_kept_stays(from: &str) -> String {
    let name = file_name(from);
    format!(
        "No move was kept for it (the app found it beside {name}, a link to its copy), so it stays for now: without \
         it, the app couldn't tell {name} was moved. You can delete it yourself once the plugin opens fine."
    )
}

/// Why a picked old copy stays when the app's records don't have its move now.
pub const MOVE_GONE: &str = "Its move isn't in the app's records now (it may have moved back since), so it stays. \
    Measure again to see what's there.";

/// Why a picked folder left behind stays when the app's records don't list it now.
pub const HOME_GONE: &str = "It isn't in the app's records now, so it stays. Measure again to see what's there.";

/// Why a picked old copy stays when it isn't where it was when the page looked.
pub const MOVED_SINCE: &str = "It isn't where it was when the page looked (a folder around it moved since), so it \
    stays. Measure again to see where it is.";

/// Why a folder called "… (old copy)" that no move was kept for stays, when the look said nothing more.
const UNKNOWN: &str = "No move was kept for it, so the app can't tell whether anything is only in it. Look in it \
    before deleting it yourself.";

/// What goes: a move's old copy, or a folder left behind (or files kept aside), as the app's records have it.
#[derive(Clone, Copy)]
pub(crate) enum Going<'a> {
    /// The old copy of this move.
    OldCopy(&'a MovedFolder),
    /// This folder left behind, or files kept aside.
    Home(&'a OldHome),
}

impl Going<'_> {
    /// Why it can't go now ([`old_copy_check`], [`old_home_check`]): what keeps it, then, unless you're `sure`, what's
    /// only in it, what its files hold compared too (slow). `moved` is every moved folder.
    fn check(self, moved: &[MovedFolder], sure: bool, stop: &AtomicBool) -> Result<(), String> {
        match self {
            Going::OldCopy(m) => old_copy_check(m, moved, sure, stop),
            Going::Home(h) => old_home_check(h, moved, sure, stop),
        }
    }

    /// Whether its record says nothing of where its files went, so there's nothing to check it against.
    fn nothing_to_check(self) -> bool {
        matches!(self, Going::Home(h) if h.copy.is_empty())
    }

    fn kind(self) -> SpaceKind {
        match self {
            Going::OldCopy(m) => m.space_kind(),
            Going::Home(h) => h.space_kind(),
        }
    }
}

/// Sends one that was just checked (`going`, at `path`, `bytes` big) with the others `sending` sends. The folder itself
/// goes (it was checked not to be a link), not where a link would lead. One Windows guards waits for the administrator
/// step, and just before Windows asks, only what keeps it is checked again: what's only in it was, just now.
fn send_checked<'a>(
    sending: &mut Sending<'a>,
    (path, bytes): (&str, u64),
    going: Going<'a>,
    moved: &'a [MovedFolder],
    stop: &'a AtomicBool,
    done: &mut Done,
) {
    sending
        .send(path, real_spot(path), bytes, What::Plugin(going.kind()), done, move || going.check(moved, true, stop));
}

/// Checks one (`going`, at `path`, `bytes` big) and sends it to the Recycle Bin on its own, as [`remove`] does each one
/// it sends: [`crate::folder_move::remove_old_copy`] and [`crate::after_move::remove_old_home`]. `moved` is every moved
/// folder; unless you're `sure`, what's only in it keeps it.
pub(crate) fn send_one(
    path: &str,
    bytes: u64,
    going: Going<'_>,
    moved: &[MovedFolder],
    sure: bool,
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    let mut done = Done::default();
    if let Err(why) = going.check(moved, sure, stop) {
        done.problems.push(Problem { path: path.to_owned(), why });
        return done;
    }
    let mut sending = Sending::new(bin, stop);
    send_checked(&mut sending, (path, bytes), going, moved, stop, &mut done);
    sending.finish(&mut done);
    done
}

/// A picked one, found again in the records as they are now (`now`): an old copy by its move (`then`, the records the
/// look had, says which), a folder left behind by its place. Files kept aside aren't in the records yet, so they're as
/// the look found them, unless the records list them now. Why it stays when it can't be found there.
fn found_again<'a>(
    l: &'a Leftover,
    then: (&[MovedFolder], &[OldHome]),
    now: (&'a [MovedFolder], &'a [OldHome]),
) -> Result<Going<'a>, String> {
    let move_now =
        |m: &MovedFolder| now.0.iter().find(|n| key(&n.from) == key(&m.from) && key(&n.old_copy) == key(&m.old_copy));
    let home_now = |path: &str| now.1.iter().find(|n| key(&n.path) == key(path));
    let old_copy = |m: &'a MovedFolder| {
        // Where it really is now must be where the page showed it: that's what goes, and what its line says.
        if key(&real_place(&m.old_copy, now.0)) == key(&l.path) {
            Ok(Going::OldCopy(m))
        } else {
            Err(MOVED_SINCE.to_owned())
        }
    };
    match &l.what {
        Left::OldCopy(at) => then.0.get(*at).and_then(move_now).ok_or_else(|| MOVE_GONE.to_owned()).and_then(old_copy),
        // Kept since (by the C# app, say), it's a move like any other.
        Left::NotKept(m) => move_now(m).ok_or_else(|| not_kept_stays(&m.from)).and_then(old_copy),
        Left::Behind(at) => {
            then.1.get(*at).and_then(|h| home_now(&h.path)).map(Going::Home).ok_or_else(|| HOME_GONE.to_owned())
        }
        Left::Kept(h) => Ok(Going::Home(home_now(&h.path).unwrap_or(h))),
        Left::Unknown => Err(l.stays.clone().unwrap_or_else(|| UNKNOWN.to_owned())),
    }
}

/// Sends the old copies and folders left behind you picked (`picked`, as [`remind`] found them: their [`Left`]s count
/// into `then`, the records the look had) to the Recycle Bin, together. Each is found again in the app's records as
/// they are now (`now`), and checked again: it goes only while nothing keeps it
/// ([`crate::folder_move::old_copy_stays`], [`crate::after_move::old_home_stays`]) and nothing is only in it, what its
/// files hold compared too. One with nothing to check it against goes as it is: the page ticks those only when you do,
/// once you've looked in it. One Windows guards waits, and all of those go in one administrator step at the end, Windows
/// asking once, each checked again just before. Reads and changes the disk, and waits for Windows' question: call it
/// away from the window's thread. [`crate::clean::undo`] puts them back.
pub fn remove(
    picked: &[Leftover],
    then: (&[MovedFolder], &[OldHome]),
    now: (&[MovedFolder], &[OldHome]),
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    let moved = now.0;
    let mut done = Done::default();
    let mut sending = Sending::new(bin, stop);
    for l in picked {
        let checked = found_again(l, then, now).and_then(|going| {
            // One with nothing to check it against goes as it is only when the look had nothing either: then the page
            // ticked it only once you did. One the look could check has been ticked for you: it stays.
            let sure = l.only.is_none() && going.nothing_to_check();
            going.check(moved, sure, stop).map(|()| going)
        });
        match checked {
            Ok(going) => send_checked(&mut sending, (&l.path, l.bytes), going, moved, stop, &mut done),
            Err(why) => done.problems.push(Problem { path: l.path.clone(), why }),
        }
    }
    sending.finish(&mut done);
    done
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
