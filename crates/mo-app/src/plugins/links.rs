//! The Plugins page's broken links (the whole-app review of 19 Nov, PM-2; spec 3.6: Move "warns on broken or replaced
//! links" and has Link again). A folder moved to another drive is reached through the link left in its place, and that
//! can stop working: the drive comes back under another letter (another USB stick took its letter), or isn't plugged
//! in, or the folder on it was moved or renamed, so the link leads nowhere (it's lost); or a plugin update put a real
//! folder back where the link was (replaced), or a later move of the folder around it left a second copy there
//! (doubled), so the plugin saves where its copy doesn't get it. The page looks for them at each read ([`look`]) and
//! names each one ([`text`]). Link again ([`link_again`]) points a lost link at its copy where it's found now, and for
//! a replaced or doubled one copies what's new or different across, sets the folder aside and puts the link back. The
//! folder set aside is kept in the library's folders left behind (Solomon's OK of 21 Nov 2026), so the Space page
//! reminds you of it until it goes to the Recycle Bin. Nothing here deletes anything: the folders the mo-space steps
//! set aside stay.

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_space::after_move::{self, Elsewhere, OldHome};
use mo_space::folder_move::{CantMove, Drives, MovedFolder};
use mo_space::{key, size};

/// What the library keeps that the look goes by, as read just before: its moved folders, its folders left behind,
/// and your folders (project, sample and MIDI folders). A lost copy is never looked for in a folder left behind, nor in
/// one of yours ([`after_move::found_on_another_drive`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Records {
    pub moved: Vec<MovedFolder>,
    pub homes: Vec<OldHome>,
    pub yours: Vec<String>,
}

/// What's wrong with a moved folder's link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// Its copy is where the library says, but the link doesn't lead there.
    LeadsElsewhere,
    /// Its copy isn't where the library says, but a folder of the same name is at the same place on another drive:
    /// its drive may have come back under another letter.
    OnAnotherDrive(Elsewhere),
    /// Its copy isn't where the library says, nor found on another drive: its drive isn't plugged in (`drive_away`,
    /// only told on Windows), or is but the folder isn't there (moved or renamed).
    CopyAway { drive_away: bool },
    /// An update put a real folder back where its link was.
    Replaced,
    /// A second copy is where its link was, inside this folder, which moved after it.
    Doubled(String),
}

/// A moved folder whose link doesn't work, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broken {
    pub moved: MovedFolder,
    pub trouble: Trouble,
}

impl Broken {
    /// Whether Link again can put it right: not while its copy is nowhere to be found.
    pub fn can_link_again(&self) -> bool {
        !matches!(self.trouble, Trouble::CopyAway { .. })
    }
}

/// The moved folders whose link doesn't work, as the library keeps them (`records`). A lost one's copy is looked for
/// on the computer's own drives, asked of `drives` only then ([`after_move::local_drives`]). Lost ones first, as the
/// library keeps them. Looks at each one on the disk: call it away from the window's thread.
pub fn look(records: &Records, drives: &dyn Fn() -> Vec<String>) -> Vec<Broken> {
    let moved = &records.moved;
    let asked: OnceCell<Vec<String>> = OnceCell::new();
    let mut broken: Vec<Broken> = after_move::lost(moved)
        .into_iter()
        .map(|m| {
            let elsewhere = || {
                let drives = asked.get_or_init(drives);
                after_move::found_on_another_drive(m, moved, &records.homes, &records.yours, drives)
            };
            let trouble = if is_dir(&m.to) {
                Trouble::LeadsElsewhere
            } else if let Some(found) = elsewhere() {
                Trouble::OnAnotherDrive(found)
            } else {
                Trouble::CopyAway { drive_away: after_move::drive_start(&m.to).is_none_or(|d| !is_dir(&d)) }
            };
            Broken { moved: m.clone(), trouble }
        })
        .collect();
    broken.extend(
        after_move::replaced(moved).into_iter().map(|m| Broken { moved: m.clone(), trouble: Trouble::Replaced }),
    );
    broken.extend(
        after_move::doubled(moved)
            .into_iter()
            .map(|(m, inside)| Broken { moved: m.clone(), trouble: Trouble::Doubled(inside) }),
    );
    broken
}

fn is_dir(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// How many the page's line names; the rest are counted.
const NAMED: usize = 3;

/// The drive a path is on, as Windows names it ("E:"), or "its drive".
fn drive_name(path: &str) -> String {
    after_move::drive_start(path)
        .map(|d| d.trim_end_matches(['/', '\\']).to_owned())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "its drive".to_owned())
}

/// One broken link, as the page's line names it.
fn sentence(b: &Broken) -> String {
    let (from, to) = (&b.moved.from, &b.moved.to);
    match &b.trouble {
        Trouble::LeadsElsewhere => {
            format!("The link at {from} no longer leads to its copy at {to}: Link again points it back there.")
        }
        Trouble::OnAnotherDrive(found) => format!(
            "The link at {from} leads to {to}, which isn't there now, but a folder of the same name is at {}: if \
             that's its drive under a new letter, Link again points the link there.",
            found.to
        ),
        Trouble::CopyAway { drive_away: true } => {
            format!("The link at {from} leads to {to}, but {} isn't plugged in.", drive_name(to))
        }
        Trouble::CopyAway { drive_away: false } => format!("The link at {from} leads to {to}, which isn't there now."),
        Trouble::Replaced => format!(
            "An update put a real folder back at {from}, where its link was, so the plugin uses that and its copy at \
             {to} is out of date."
        ),
        Trouble::Doubled(inside) => format!(
            "{from} is a second copy, left when {inside} moved, so the plugin uses that and its own copy at {to} is \
             out of date."
        ),
    }
}

/// The page's line about the moved folders whose link doesn't work: each named (the first few, the ones Link again
/// can put right first), what that does to the plugin, and what Link again does when it can do anything. Empty when
/// there are none.
pub fn text(broken: &[Broken]) -> String {
    if broken.is_empty() {
        return String::new();
    }
    let mut said: Vec<String> = vec![if broken.len() == 1 {
        "A moved folder's link needs a look.".to_owned()
    } else {
        format!("{} moved folders' links need a look.", broken.len())
    }];
    let (fixable, away): (Vec<&Broken>, Vec<&Broken>) = broken.iter().partition(|b| b.can_link_again());
    said.extend(fixable.iter().chain(&away).take(NAMED).map(|b| sentence(b)));
    if broken.len() > NAMED {
        said.push(format!("And {} more.", broken.len() - NAMED));
    }
    match broken.iter().filter(|b| matches!(b.trouble, Trouble::Replaced | Trouble::Doubled(_))).count() {
        0 => {}
        1 => said.push(
            "For the real folder, Link again copies its new files, and files that differ, to its copy (keeping the \
             copy's own versions beside it), sets it aside for you to remove on the Space page once the plugin opens \
             fine, and puts the link back. Close your DAW first."
                .to_owned(),
        ),
        _ => said.push(
            "For the real folders, Link again copies each one's new files, and files that differ, to its copy \
             (keeping the copy's own versions beside it), sets them aside for you to remove on the Space page once \
             the plugins open fine, and puts the links back. Close your DAW first."
                .to_owned(),
        ),
    }
    // Only Windows names a drive by a letter another drive can take ([`after_move::local_drives`]).
    let letters = cfg!(windows);
    match away.len() {
        0 => {}
        1 => said.push(format!(
            "Until that copy is back, the plugin can't find what's in it: plug its drive in, or put the folder back \
             where its link leads.{}",
            if letters { " If its drive comes back under another letter, this page offers Link again." } else { "" }
        )),
        _ => said.push(format!(
            "Until those copies are back, the plugins can't find what's in them: plug their drives in, or put each \
             folder back where its link leads.{}",
            if letters {
                " Any whose drive comes back under another letter, this page offers to link again."
            } else {
                ""
            }
        )),
    }
    said.join(" ")
}

/// What Link again did for one moved folder: where its link is, and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linked {
    pub from: String,
    pub said: Result<String, String>,
}

/// What Link again did, and what the library is to keep of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fixed {
    /// For each moved folder it looked at.
    pub done: Vec<Linked>,
    /// The moved folders whose copy is somewhere else now (its drive's letter changed), for the library's moved
    /// folders.
    pub moved: Vec<MovedFolder>,
    /// The folders left behind to keep: the ones linking again set aside (or kept from the copy), and the ones that
    /// moved with their drive, each with the path it was kept under before.
    pub homes: Vec<(OldHome, Option<String>)>,
}

/// How far Link again has got with a folder: the folder, then what's copied of how much.
pub type Progress<'a> = &'a (dyn Fn(&MovedFolder, u64, u64) + Sync);

/// Link again for each moved folder that needs it and can have it ([`look`] again on `records`, as the library keeps
/// them just before; `drives` as for [`look`]). Lost links first: each pointed at its copy, the ones whose drive came
/// back under the same other letter together, so Windows asks its administrator question once for the links it guards
/// ([`after_move::repoint`]); then the ones inside those, which only their links lead to, a round at a time. Then each
/// replaced or doubled one: measured, its new and different files copied to its copy, set aside, and linked again
/// ([`after_move::link_again`]). A stop leaves the rest as they were. Changes the disk: call it away from the window's
/// thread, with the DAW closed.
pub fn link_again(
    records: &Records,
    drives: &dyn Fn() -> Vec<String>,
    disk: &dyn Drives,
    progress: Progress<'_>,
    stop: &AtomicBool,
) -> Fixed {
    // The drives are asked for once, and only if a lost copy is looked for.
    let asked: OnceCell<Vec<String>> = OnceCell::new();
    let drives_once = || asked.get_or_init(drives).clone();
    let mut fixed = Fixed::default();
    let mut now = records.clone();
    // The moved folders whose lost link was looked at: each once.
    let mut tried: HashSet<String> = HashSet::new();
    loop {
        // The lost ones, by the place their links are pointed from and to: one drive to another, or a copy's own
        // folder.
        let mut lost: BTreeMap<(String, String), Vec<MovedFolder>> = BTreeMap::new();
        for b in look(&now, &drives_once) {
            let between = match &b.trouble {
                Trouble::OnAnotherDrive(found) => (found.was_on.clone(), found.now_on.clone()),
                Trouble::LeadsElsewhere => (after_move::home_of(&b.moved), after_move::home_of(&b.moved)),
                _ => continue,
            };
            if tried.insert(key(&b.moved.from)) {
                lost.entry(between).or_default().push(b.moved);
            }
        }
        if lost.is_empty() {
            break;
        }
        for ((from, to), these) in lost {
            if stop.load(Ordering::Relaxed) {
                fixed.done.extend(these.iter().map(|m| stopped_before(&m.from)));
                continue;
            }
            let done = after_move::repoint(&from, &to, &these, &now.homes, disk.admin());
            for (was, m) in these.iter().zip(&done.moved) {
                match done.problems.iter().find(|p| key(&p.path) == key(&m.from)) {
                    Some(p) => fixed.done.push(Linked { from: m.from.clone(), said: Err(p.why.clone()) }),
                    None => {
                        fixed.done.push(Linked {
                            from: m.from.clone(),
                            said: Ok(format!("{} leads to its copy at {} again.", m.from, m.to)),
                        });
                        if key(&m.to) != key(&was.to) {
                            fixed.moved.push(m.clone());
                            if let Some(kept) = now.moved.iter_mut().find(|k| key(&k.from) == key(&m.from)) {
                                kept.to = m.to.clone();
                            }
                        }
                    }
                }
            }
            // The folders left behind that moved with the drive.
            for (before, after) in now.homes.iter().zip(&done.old_homes) {
                if key(&before.path) != key(&after.path) || key(&before.copy) != key(&after.copy) {
                    fixed.homes.push((after.clone(), Some(before.path.clone())));
                }
            }
            now.homes = done.old_homes;
        }
        // A stop leaves the ones inside them as they are, unlooked at.
        if stop.load(Ordering::Relaxed) {
            break;
        }
    }
    // Then the real folders where links were, as things are now.
    for b in look(&now, &drives_once) {
        let second = match &b.trouble {
            Trouble::Replaced => false,
            Trouble::Doubled(_) => true,
            _ => continue,
        };
        let m = &b.moved;
        if stop.load(Ordering::Relaxed) {
            fixed.done.push(stopped_before(&m.from));
            continue;
        }
        let Ok(measured) = size::measure(Path::new(&m.from), stop) else {
            fixed.done.push(stopped_before(&m.from));
            continue;
        };
        let bytes = measured.bytes;
        let each = |copied: u64| progress(m, copied, bytes);
        match after_move::link_again(m, &now.moved, bytes, second, disk, &each, stop) {
            Ok(linked) => {
                let mut said = format!(
                    "{} leads to its copy at {} again. The folder that was there is set aside at {}: remove it on the \
                     Space page once the plugin opens fine.",
                    m.from, m.to, linked.set_aside.path
                );
                if let Some(kept) = &linked.kept {
                    let files = if linked.replaced.len() == 1 { "file" } else { "files" };
                    said.push_str(&format!(
                        " The {} {files} it replaced in the copy {} kept in {}.",
                        linked.replaced.len(),
                        if linked.replaced.len() == 1 { "is" } else { "are" },
                        kept.path
                    ));
                }
                if !linked.links_left.is_empty() {
                    said.push_str(&format!(
                        " Some links were left as they were, for you to look at: {}.",
                        listed(&linked.links_left)
                    ));
                }
                fixed.homes.push((linked.set_aside, None));
                fixed.homes.extend(linked.kept.map(|k| (k, None)));
                fixed.done.push(Linked { from: m.from.clone(), said: Ok(said) });
            }
            Err(why) => fixed.done.push(Linked { from: m.from.clone(), said: Err(failed_text(&why)) }),
        }
    }
    fixed
}

/// What one Link again didn't get to says.
fn stopped_before(from: &str) -> Linked {
    Linked { from: from.to_owned(), said: Err("Link again stopped before this one: it's as it was.".to_owned()) }
}

/// Why linking again didn't happen: a move's words, without what's only true of a move.
fn failed_text(why: &CantMove) -> String {
    match why {
        CantMove::Stopped => "Stopped: it's as it was.".to_owned(),
        other => other.text().replace(" Nothing was moved.", " It's as it was."),
    }
}

/// The first few of a list, and how many more.
fn listed(paths: &[String]) -> String {
    let first: Vec<&str> = paths.iter().take(NAMED).map(String::as_str).collect();
    match paths.len() - first.len() {
        0 => first.join(", "),
        more => format!("{} and {more} more", first.join(", ")),
    }
}

/// The moved folders the library didn't keep: [`Fixed::moved`] from `first` on (keeping stops at the first it can't
/// keep), and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotKept {
    pub first: usize,
    pub why: String,
}

/// What Link again says once it's over, with how keeping it in the library went: `moved_kept` for the moved folders
/// whose copy is somewhere else now, and `homes_kept` for the folders left behind ([`Fixed`]).
pub fn done_text(fixed: &Fixed, moved_kept: &Result<(), NotKept>, homes_kept: &Result<(), String>) -> String {
    let mut said: Vec<String> = fixed
        .done
        .iter()
        .map(|d| match &d.said {
            Ok(s) => s.clone(),
            Err(why) => format!("{}: {why}", d.from),
        })
        .collect();
    if said.is_empty() {
        said.push("Nothing needed linking again by the time it looked.".to_owned());
    }
    if let Err(not) = moved_kept {
        let froms: Vec<String> = fixed.moved.iter().skip(not.first).map(|m| m.from.clone()).collect();
        said.push(format!(
            "But the library couldn't keep where the copy is now for {} ({}), so this page shows the link as needing \
             a look again: Link again then tries once more.",
            listed(&froms),
            not.why
        ));
    }
    if let Err(why) = homes_kept {
        let paths: Vec<String> = fixed.homes.iter().map(|(h, _)| h.path.clone()).collect();
        said.push(format!(
            "But the library couldn't keep its folders left behind ({why}), so the Space page won't remind you of {}: \
             remove them yourself once the plugin opens fine.",
            listed(&paths)
        ));
    }
    said.join(" ")
}

/// Whether Link again went well for every folder it looked at, and was kept.
pub fn all_done(fixed: &Fixed, moved_kept: &Result<(), NotKept>, homes_kept: &Result<(), String>) -> bool {
    fixed.done.iter().all(|d| d.said.is_ok()) && moved_kept.is_ok() && homes_kept.is_ok()
}

/// The moved folders as the library's moved folders keep them (the C# app's shape), one JSON object each, for
/// `mo_library::moves::add_moved_folder`.
pub fn moved_entries(fixed: &Fixed) -> Result<Vec<String>, String> {
    fixed.moved.iter().map(|m| serde_json::to_string(m).map_err(|e| e.to_string())).collect()
}

/// The folders left behind as the library keeps them (the C# app's shape), each with the path it was kept under
/// before, for `mo_library::moves::keep_old_homes`.
pub fn home_entries(fixed: &Fixed) -> Result<Vec<(String, Option<String>)>, String> {
    fixed
        .homes
        .iter()
        .map(|(h, before)| serde_json::to_string(h).map(|j| (j, before.clone())).map_err(|e| e.to_string()))
        .collect()
}

/// The library's folders left behind, read strictly: a value that isn't a list of them is an error, never none (Link
/// again would keep what it sets aside beside a list it couldn't read).
pub fn homes_now(json: Option<&str>) -> Result<Vec<OldHome>, String> {
    match json.map(str::trim).filter(|j| !j.is_empty()) {
        None => Ok(Vec::new()),
        Some(j) => serde_json::from_str::<Vec<OldHome>>(j)
            .map_err(|_| "The library's list of folders left behind couldn't be read.".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use mo_space::folder_move::move_folder;
    use mo_space::plugin_space::SpaceKind;

    use super::*;

    /// Every folder is on its own drive.
    struct OtherDrive;

    impl Drives for OtherDrive {
        fn same(&self, _a: &Path, _b: &Path) -> bool {
            false
        }
        fn free(&self, _path: &Path) -> Option<u64> {
            None
        }
    }

    /// A folder of this test's own, cleared first.
    fn place(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    /// Moves `from` into `to` as the Plugins page's Move does, leaving a link in its place.
    fn move_it(from: &Path, to: &Path, all: &[MovedFolder]) -> MovedFolder {
        let stop = AtomicBool::new(false);
        let kind = ("arcade", SpaceKind::Sounds, &[][..]);
        move_folder(&s(from), &s(to), 1, kind, &OtherDrive, all, &[], &|_| {}, &stop).unwrap()
    }

    /// Takes a link away, not what it leads to (Windows takes a junction away as a folder).
    fn unlink(at: &Path) {
        if fs::remove_file(at).is_err() {
            fs::remove_dir(at).unwrap();
        }
    }

    fn is_link(p: &Path) -> bool {
        fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
    }

    fn nothing(_: &MovedFolder, _: u64, _: u64) {}

    fn broken(from: &str, trouble: Trouble) -> Broken {
        let moved = MovedFolder {
            from: from.to_owned(),
            to: format!("E:\\Plugin Data\\{from}"),
            old_copy: String::new(),
            bytes: 1,
            moved: String::new(),
            key: String::new(),
            kind: 2,
            shared_with: Vec::new(),
        };
        Broken { moved, trouble }
    }

    /// The library's records with only moved folders.
    fn records(moved: &[MovedFolder]) -> Records {
        Records { moved: moved.to_vec(), ..Records::default() }
    }

    /// The records once what Link again did is kept: each moved folder whose copy is somewhere else now replaces the
    /// one for the same folder, as `mo_library::moves::add_moved_folder` does.
    fn kept(before: &[MovedFolder], fixed: &Fixed) -> Records {
        let moved = before
            .iter()
            .map(|m| fixed.moved.iter().find(|n| key(&n.from) == key(&m.from)).unwrap_or(m).clone())
            .collect::<Vec<_>>();
        records(&moved)
    }

    /// The whole-app review of 19 Nov, PM-2: each kind of broken link is named, with what Link again does, and only
    /// what it can do.
    #[test]
    fn each_broken_link_is_named_with_what_link_again_does() {
        assert_eq!(text(&[]), "");
        let elsewhere = Elsewhere { was_on: "E:\\".into(), now_on: "F:\\".into(), to: "F:\\Plugin Data\\A".into() };
        let one = text(&[broken("A", Trouble::OnAnotherDrive(elsewhere))]);
        assert!(one.starts_with("A moved folder's link needs a look."), "{one}");
        // Nothing says it's the same drive: you're asked to look.
        assert!(one.contains("a folder of the same name is at F:\\Plugin Data\\A: if that's its drive"), "{one}");
        assert!(one.ends_with("Link again points the link there."), "{one}");
        let back = text(&[broken("A", Trouble::LeadsElsewhere)]);
        assert!(back.ends_with("no longer leads to its copy at E:\\Plugin Data\\A: Link again points it back there."));
        let away = text(&[broken("A", Trouble::CopyAway { drive_away: true })]);
        // The drive by its letter on Windows (a Mac or Linux reads "E:\\" as no drive at all).
        let drive = if cfg!(windows) { "E:" } else { "its drive" };
        assert!(away.contains(&format!("but {drive} isn't plugged in")) && !away.contains("Link again "), "{away}");
        assert!(away.contains("put the folder back where its link leads."), "{away}");
        // Only Windows names a drive by a letter another drive can take.
        let offered = away.contains("If its drive comes back under another letter, this page offers Link again.");
        assert_eq!(offered, cfg!(windows), "{away}");
        let gone = text(&[broken("A", Trouble::CopyAway { drive_away: false })]);
        assert!(gone.contains("leads to E:\\Plugin Data\\A, which isn't there now. Until that copy"), "{gone}");
        let update = text(&[broken("A", Trouble::Replaced)]);
        assert!(update.contains("An update put a real folder back at A"), "{update}");
        assert!(update.contains("For the real folder, Link again copies its new files, and files that differ"));
        assert!(update.contains("sets it aside") && update.ends_with("Close your DAW first."), "{update}");
        let all = text(&[
            broken("D", Trouble::CopyAway { drive_away: true }),
            broken("E", Trouble::CopyAway { drive_away: true }),
            broken("A", Trouble::LeadsElsewhere),
            broken("B", Trouble::Replaced),
            broken("C", Trouble::Doubled("Native Instruments".into())),
        ]);
        assert!(all.starts_with("5 moved folders' links need a look."), "{all}");
        // The ones Link again can put right are named first.
        assert!(all.contains("C is a second copy, left when Native Instruments moved"), "{all}");
        assert!(all.contains("And 2 more.") && !all.contains("D leads"), "{all}");
        assert!(all.contains("For the real folders, Link again copies each one's new files"), "{all}");
        assert!(all.contains("Until those copies are back"), "{all}");
        assert!(!broken("D", Trouble::CopyAway { drive_away: true }).can_link_again());
        assert!(broken("D", Trouble::Replaced).can_link_again());
    }

    /// The whole-app review of 19 Nov, PM-2: a moved folder's drive comes back under another letter; the link is
    /// found lost, its copy on the other drive, and Link again points it there, the folder left behind on that drive
    /// following. A link that leads somewhere else is pointed back at its copy.
    #[test]
    fn a_lost_link_is_found_and_linked_again() {
        let dir = place("mo-app-links-lost");
        let (c, e, f) = (dir.join("C"), dir.join("E"), dir.join("F"));
        write(&c, "Arcade/Pad.db", "pad");
        write(&c, "Kontakt/a.nki", "a");
        let data = e.join("Plugin Data");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&f).unwrap();
        let arcade = move_it(&c.join("Arcade"), &data, &[]);
        let kontakt = move_it(&c.join("Kontakt"), &data, std::slice::from_ref(&arcade));
        let moved = vec![arcade.clone(), kontakt.clone()];
        // The drives are asked for only when a copy is looked for on them.
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let drives = || {
            asked.fetch_add(1, Ordering::Relaxed);
            vec![s(&f)]
        };
        assert!(look(&records(&moved), &drives).is_empty());
        assert_eq!(asked.load(Ordering::Relaxed), 0);

        // Kontakt's link leads somewhere else: Link again points it back.
        unlink(&c.join("Kontakt"));
        fs::create_dir_all(dir.join("Elsewhere")).unwrap();
        #[cfg(windows)]
        junction::create(dir.join("Elsewhere"), c.join("Kontakt")).unwrap();
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(dir.join("Elsewhere"), c.join("Kontakt")).unwrap();
        let found = look(&records(&moved), &drives);
        assert_eq!(found, [Broken { moved: kontakt.clone(), trouble: Trouble::LeadsElsewhere }]);
        let stop = AtomicBool::new(false);
        let fixed = link_again(&records(&moved), &drives, &OtherDrive, &nothing, &stop);
        assert!(fixed.moved.is_empty() && fixed.homes.is_empty(), "{fixed:?}");
        assert_eq!(fixed.done.len(), 1);
        assert!(fixed.done[0].said.as_ref().is_ok_and(|said| said.contains("again")), "{fixed:?}");
        assert_eq!(fs::read_to_string(c.join("Kontakt/a.nki")).unwrap(), "a");
        assert!(look(&records(&moved), &drives).is_empty());
        assert_eq!(asked.load(Ordering::Relaxed), 0);

        // The drive is away: both are lost, and nowhere to be found.
        let start = after_move::drive_start(&arcade.to).unwrap();
        fs::rename(&e, dir.join("E away")).unwrap();
        let found = look(&records(&moved), &drives);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|b| b.trouble == Trouble::CopyAway { drive_away: false }), "{found:?}");
        // Asked once for both.
        assert_eq!(asked.load(Ordering::Relaxed), 1);
        assert!(link_again(&records(&moved), &drives, &OtherDrive, &nothing, &stop).done.is_empty());

        // It comes back as F: with a folder left behind beside Arcade's copy. Link again points both links there,
        // and the folder left behind follows.
        let on_f = |p: &str| f.join(Path::new(p).strip_prefix(&start).unwrap());
        fs::create_dir_all(on_f(&s(&data)).parent().unwrap()).unwrap();
        fs::rename(dir.join("E away").join("Plugin Data"), on_f(&s(&data))).unwrap();
        let left = data.join("Arcade (kept from before)");
        fs::create_dir_all(on_f(&s(&left))).unwrap();
        let home = OldHome {
            path: s(&left),
            bytes: 1,
            note: "replaced by linking again".into(),
            copy: String::new(),
            kind: 2,
        };
        let now = Records { homes: vec![home.clone()], ..records(&moved) };
        let found = look(&now, &drives);
        assert!(found.iter().all(|b| matches!(&b.trouble, Trouble::OnAnotherDrive(e) if e.now_on == s(&f))));
        assert!(text(&found).contains("if that's its drive under a new letter"), "{}", text(&found));
        // Not where one of your folders is.
        let yours = Records { yours: vec![s(&on_f(&s(&data)))], ..now.clone() };
        assert!(look(&yours, &drives).iter().all(|b| b.trouble == Trouble::CopyAway { drive_away: false }));
        let fixed = link_again(&now, &drives, &OtherDrive, &nothing, &stop);
        assert!(fixed.done.iter().all(|d| d.said.is_ok()), "{fixed:?}");
        assert_eq!(fixed.moved.len(), 2);
        assert_eq!(fixed.moved[0].to, s(&on_f(&arcade.to)));
        assert_eq!(fixed.homes.len(), 1);
        assert_eq!(fixed.homes[0].0.path, s(&on_f(&s(&left))));
        assert_eq!(fixed.homes[0].1.as_deref(), Some(home.path.as_str()));
        // It didn't say where its files went, and still doesn't.
        assert_eq!(fixed.homes[0].0.copy, "");
        assert_eq!(fs::read_to_string(c.join("Arcade/Pad.db")).unwrap(), "pad");
        assert!(look(&kept(&moved, &fixed), &drives).is_empty());
        // What's kept in the library, in the C# app's shape.
        assert!(moved_entries(&fixed).unwrap()[0].contains("\"To\""));
        assert!(home_entries(&fixed).unwrap()[0].0.contains("\"Path\""));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Asks Windows' administrator question of no one, and stops Link again once it's been asked for (once a group of
    /// lost links has been pointed at their copies).
    struct StopAfterOne<'a>(&'a AtomicBool);

    impl Drives for StopAfterOne<'_> {
        fn same(&self, _a: &Path, _b: &Path) -> bool {
            false
        }
        fn free(&self, _path: &Path) -> Option<u64> {
            None
        }
        fn admin(&self) -> &dyn mo_space::admin::Admin {
            self.0.store(true, Ordering::Relaxed);
            &mo_space::admin::NoAdmin
        }
    }

    /// The whole-app review of 19 Nov, PM-2 (the branch's own review): two drives that came back under other letters,
    /// and a moved folder inside another (Reaktor 6 first, then Native Instruments around it), whose link only that
    /// one's link leads to. A stop after the first drive's links leaves the other's as they were; pressed again, the
    /// rest are linked again in one go, the inner one once the outer one's link leads somewhere again.
    #[test]
    fn links_inside_a_moved_folder_and_on_two_drives_are_linked_again_in_one_go() {
        let dir = place("mo-app-links-rounds");
        let (c, e, g) = (dir.join("C"), dir.join("E"), dir.join("G"));
        let (h, y) = (dir.join("H"), dir.join("Y"));
        write(&c, "NI/Reaktor 6/r.ens", "r");
        write(&c, "NI/k.nki", "k");
        write(&c, "Arcade/Pad.db", "pad");
        for d in [&e, &g] {
            fs::create_dir_all(d.join("Plugin Data")).unwrap();
        }
        let mut moved = vec![move_it(&c.join("NI").join("Reaktor 6"), &e.join("Plugin Data"), &[])];
        moved.push(move_it(&c.join("NI"), &e.join("Plugin Data"), &moved));
        moved.push(move_it(&c.join("Arcade"), &g.join("Plugin Data"), &moved));
        let drives = || vec![s(&h), s(&y)];
        assert!(look(&records(&moved), &drives).is_empty());

        // E comes back as Y, and G as H.
        let start = after_move::drive_start(&moved[0].to).unwrap();
        for (was, now) in [(&e, &y), (&g, &h)] {
            let there = now.join(was.strip_prefix(&start).unwrap());
            fs::create_dir_all(there.parent().unwrap()).unwrap();
            fs::rename(was, &there).unwrap();
        }
        // Reaktor 6's link is behind Native Instruments' lost one: only the outer two are seen.
        let found: Vec<String> = look(&records(&moved), &drives).into_iter().map(|b| b.moved.from).collect();
        assert_eq!(found, [moved[1].from.clone(), moved[2].from.clone()]);

        // A stop once H's links are done leaves Y's as they were.
        let stop = AtomicBool::new(false);
        let fixed = link_again(&records(&moved), &drives, &StopAfterOne(&stop), &nothing, &stop);
        assert_eq!(fixed.done.len(), 2, "{fixed:?}");
        assert_eq!(fixed.done[0].from, moved[2].from);
        assert!(fixed.done[0].said.is_ok(), "{fixed:?}");
        assert_eq!(fixed.done[1].said, Err("Link again stopped before this one: it's as it was.".to_owned()));
        assert_eq!(fixed.moved.len(), 1);
        assert_eq!(fs::read_to_string(c.join("Arcade/Pad.db")).unwrap(), "pad");
        assert!(fs::metadata(c.join("NI/k.nki")).is_err());
        let part_way = kept(&moved, &fixed);

        // Pressed again: Native Instruments' link, then Reaktor 6's, now that the one around it leads somewhere.
        let stop = AtomicBool::new(false);
        let fixed = link_again(&part_way, &drives, &OtherDrive, &nothing, &stop);
        assert!(fixed.done.iter().all(|d| d.said.is_ok()), "{fixed:?}");
        let froms: Vec<&str> = fixed.moved.iter().map(|m| m.from.as_str()).collect();
        assert_eq!(froms, [moved[1].from.as_str(), moved[0].from.as_str()]);
        assert_eq!(fs::read_to_string(c.join("NI/k.nki")).unwrap(), "k");
        assert_eq!(fs::read_to_string(c.join("NI/Reaktor 6/r.ens")).unwrap(), "r");
        assert!(look(&kept(&part_way.moved, &fixed), &drives).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The whole-app review of 19 Nov, PM-2: an update put a real folder back where the link was. Link again copies
    /// its newer files to the copy, sets it aside for Space, and puts the link back; a stop leaves it as it was. A lost
    /// link beside it is put right in the same go.
    #[test]
    fn a_folder_an_update_made_where_the_link_was_is_linked_again() {
        let dir = place("mo-app-links-update");
        let (c, e) = (dir.join("C"), dir.join("E"));
        write(&c, "Arcade/Pad.db", "pad");
        write(&c, "Kontakt/a.nki", "a");
        fs::create_dir_all(&e).unwrap();
        let arcade = move_it(&c.join("Arcade"), &e, &[]);
        let kontakt = move_it(&c.join("Kontakt"), &e, std::slice::from_ref(&arcade));
        let moved = vec![arcade.clone()];
        let none = Vec::new;
        unlink(&c.join("Arcade"));
        write(&c, "Arcade/Pad.db", "pad 2");
        write(&c, "Arcade/Update.db", "new");
        assert_eq!(look(&records(&moved), &none), [Broken { moved: arcade.clone(), trouble: Trouble::Replaced }]);

        let stopped = AtomicBool::new(true);
        let fixed = link_again(&records(&moved), &none, &OtherDrive, &nothing, &stopped);
        assert!(fixed.done[0].said.as_ref().is_err_and(|why| why.starts_with("Link again stopped before")));
        assert!(fixed.homes.is_empty() && !is_link(&c.join("Arcade")));

        // Kontakt's link leads somewhere else meanwhile.
        unlink(&c.join("Kontakt"));
        fs::create_dir_all(dir.join("Elsewhere")).unwrap();
        #[cfg(windows)]
        junction::create(dir.join("Elsewhere"), c.join("Kontakt")).unwrap();
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(dir.join("Elsewhere"), c.join("Kontakt")).unwrap();
        let both = vec![arcade.clone(), kontakt.clone()];
        let stop = AtomicBool::new(false);
        let fixed = link_again(&records(&both), &none, &OtherDrive, &nothing, &stop);
        assert_eq!(fixed.done.len(), 2, "{fixed:?}");
        assert_eq!(fixed.done[0].from, kontakt.from);
        assert!(fixed.done[0].said.is_ok(), "{fixed:?}");
        assert_eq!(fs::read_to_string(c.join("Kontakt/a.nki")).unwrap(), "a");
        let said = fixed.done[1].said.as_ref().unwrap();
        assert!(said.contains("set aside at") && said.contains("The 1 file it replaced"), "{said}");
        assert!(is_link(&c.join("Arcade")));
        assert_eq!(fs::read_to_string(c.join("Arcade/Update.db")).unwrap(), "new");
        assert_eq!(fs::read_to_string(c.join("Arcade/Pad.db")).unwrap(), "pad 2");
        // The folder set aside, and the copy's own Pad.db kept beside it, for the Space page.
        assert_eq!(fixed.homes.len(), 2);
        assert_eq!(fixed.homes[0].0.path, s(&c.join("Arcade (after update)")));
        assert_eq!(fixed.homes[0].0.copy, arcade.to);
        assert!(fixed.homes[1].0.path.ends_with("Arcade (kept from before)"));
        assert!(fixed.homes.iter().all(|(_, before)| before.is_none()));
        assert!(look(&records(&both), &none).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_link_again_says_names_what_the_library_couldnt_keep() {
        let fixed = Fixed {
            done: vec![
                Linked { from: "C:\\A".into(), said: Ok("C:\\A leads to its copy at F:\\A again.".into()) },
                Linked { from: "C:\\B".into(), said: Ok("C:\\B leads to its copy at F:\\B again.".into()) },
                Linked { from: "C:\\C".into(), said: Err("Windows said no.".into()) },
            ],
            moved: vec![broken("C:\\A", Trouble::LeadsElsewhere).moved, broken("C:\\B", Trouble::LeadsElsewhere).moved],
            homes: vec![(
                OldHome { path: "F:\\Old".into(), bytes: 1, note: String::new(), copy: String::new(), kind: 2 },
                None,
            )],
        };
        let (all_kept, ok) = (Ok(()), Ok(()));
        let said = done_text(&fixed, &all_kept, &ok);
        assert!(said.ends_with("again. C:\\C: Windows said no."), "{said}");
        assert!(!all_done(&fixed, &all_kept, &ok));
        // Keeping stopped at the second: the first was kept.
        let busy = Err(NotKept { first: 1, why: "the library is busy".into() });
        let said = done_text(&fixed, &busy, &ok);
        assert!(said.contains("couldn't keep where the copy is now for C:\\B (the library is busy)"), "{said}");
        let homes = Err("the library is busy".to_owned());
        let said = done_text(&fixed, &all_kept, &homes);
        assert!(said.contains("couldn't keep its folders left behind (the library is busy)"), "{said}");
        assert!(said.contains("won't remind you of F:\\Old"), "{said}");
        let nothing_done = done_text(&Fixed::default(), &all_kept, &ok);
        assert_eq!(nothing_done, "Nothing needed linking again by the time it looked.");
        assert!(homes_now(None).unwrap().is_empty());
        assert!(homes_now(Some("oops")).is_err());
        assert_eq!(homes_now(Some(r#"[{"Path":"C:\\A","Bytes":1}]"#)).unwrap()[0].path, "C:\\A");
    }
}
