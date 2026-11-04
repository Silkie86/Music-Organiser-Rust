//! The Space page's old copies (phase 4, part 3): what moving a plugin's folder to another drive left behind. A move
//! copies the folder over, puts a link where it was and keeps the old copy beside the link, so you can check the plugin
//! still opens before the old copy goes. Solomon (3 Nov 2026): the page "should always remind about old copies", with
//! their size, for as long as any is there. `mo_space::old_copies` finds and measures them from the moves the app keeps
//! (library.db, shared with the C# app), with what's only in each; this reads those records and says every word of the
//! line and card, and keeps track of Remove: the ones you tick go to the Recycle Bin together, each checked again first,
//! and Put back returns them for as long as they're there. As the page's mock-up has it, one with something only in it
//! has no tick ("so it stays until you look"); one with nothing to check it against goes only once you tick it.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chrono::NaiveDateTime;
use mo_formats::paths;
use mo_space::Stopped;
use mo_space::after_move::{OldHome, parse_old_homes};
use mo_space::clean::{Bin, Done, Problem};
use mo_space::folder_move::{MovedFolder, parse_moved};
use mo_space::old_copies::{self, Left, Leftover, Reminder};
use mo_space::size::size_text;

use crate::shell::{folder_of, wrappable_path};
use crate::songs::thousands;
use crate::space::{Card, Line, Opens, Part, and_list};
use crate::space_bin::BIN;
use crate::space_removing::{
    self, ADMIN, Looked, Removable, back_words, freed_words, gone, result_figures, result_words, sent_words,
};

/// The library settings the moves are kept in, as the C# app keeps them: the moved folders, and the folders moves
/// left behind.
pub const MOVED_FOLDERS: &str = "space_moved_folders";
pub const OLD_HOMES: &str = "space_old_homes";

/// The old copies line's id.
pub const OLD_COPIES: &str = "old-copies";

/// The card's big figure when none of the old copies can go yet.
pub const NOTHING_YET: &str = "Nothing to free yet";

/// The activity card's title while Remove is on.
pub const REMOVING: &str = "Removing old copies";

/// Why every ticked one stays when Remove couldn't read the moves the app keeps, to check each one again.
pub const CANT_CHECK: &str = "The moves the app keeps couldn't be read to check it again (the log says why), so it \
    stays.";

/// What a look for old copies found: each one still there, biggest first, and the records it was found from (what a
/// [`Left`] counts into). The card's words come from them, and Remove finds each one again from them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub reminder: Reminder,
    pub moved: Vec<MovedFolder>,
    pub homes: Vec<OldHome>,
}

impl Looked for Found {
    type Item = Leftover;
    fn items(&self) -> &[Leftover] {
        &self.reminder.leftovers
    }
}

/// One goes once nothing keeps it and nothing is only in it. One with nothing to check it against isn't ticked out of
/// the box: it goes only once you've looked in it and tick it.
impl Removable for Leftover {
    fn path(&self) -> &str {
        &self.path
    }
    fn bytes(&self) -> u64 {
        self.bytes
    }
    fn can_go(&self) -> bool {
        Leftover::can_go(self)
    }
    fn ticked_at_first(&self) -> bool {
        Leftover::can_go(self) && self.only.is_some()
    }
}

/// How removing old copies goes while the app runs: the ticks you changed, and where Remove has got to.
pub type Removing = space_removing::Removing<Leftover>;

/// Where removing old copies has got to.
pub type Stage = space_removing::Stage<Leftover>;

/// A removal of old copies that ran.
pub type Sent = space_removing::Sent<Leftover>;

/// Looks for old copies: from the moves as the library keeps them (`moved` and `homes`, its [`MOVED_FOLDERS`] and
/// [`OLD_HOMES`] settings; none when there's no record or it can't be read), and in `look_in` (the folders plugins
/// keep their data in) for one no move was kept for. Reads and measures each one, and what's only in it: call it away
/// from the window's thread. Stopped, it says so and finds nothing.
pub fn look(moved: Option<&str>, homes: Option<&str>, look_in: &[String], stop: &AtomicBool) -> Result<Found, Stopped> {
    let moved = moved.map(parse_moved).unwrap_or_default();
    let homes = homes.map(parse_old_homes).unwrap_or_default();
    let reminder = old_copies::remind(&moved, &homes, look_in, stop)?;
    Ok(Found { reminder, moved, homes })
}

/// Sends the ones you picked (`picked`, from `found`) to the Recycle Bin together: each found again in the moves as
/// the library keeps them now (`now`: its [`MOVED_FOLDERS`] and [`OLD_HOMES`] settings, read just before; none when
/// there's no record), checked again, and compared file by file with its copy just before it goes
/// (`mo_space::old_copies::remove`). Ones Windows guards go together at the end, Windows asking once. Reads and changes
/// the disk, and waits for Windows' question: call it away from the window's thread.
pub fn remove(
    picked: &[Leftover],
    found: &Found,
    now: (Option<&str>, Option<&str>),
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    let moved = now.0.map(parse_moved).unwrap_or_default();
    let homes = now.1.map(parse_old_homes).unwrap_or_default();
    old_copies::remove(picked, (&found.moved, &found.homes), (&moved, &homes), bin, stop)
}

/// Every one picked stays, for `why`: what Remove says when it couldn't check them again (the moves couldn't be read,
/// say).
pub fn stayed(picked: &[Leftover], why: &str) -> Done {
    let problems = picked.iter().map(|l| Problem { path: l.path.clone(), why: why.to_owned() }).collect();
    Done { problems, ..Done::default() }
}

/// Why every ticked one is said to have stayed when Remove stopped with an error (logged as a crash): what went before
/// it stopped is in the bin, but which isn't known.
pub fn crashed_words() -> String {
    format!("Remove stopped with an error (the log says which). Anything that went before it stopped is in the {BIN}.")
}

/// "1 old copy", "3 old copies".
fn old_copies_words(n: usize) -> String {
    if n == 1 { "1 old copy".to_owned() } else { format!("{} old copies", thousands(n)) }
}

/// How many can go, and how many can't yet: "3 old copies can go, and 1 can't go yet".
fn count_words(found: &Reminder) -> String {
    let go = found.can_go().count();
    let staying = found.leftovers.len() - go;
    match (go, staying) {
        (0, 0) => String::new(),
        (0, m) => format!("{} can't go yet", old_copies_words(m)),
        (n, 0) => format!("{} can go", old_copies_words(n)),
        (n, m) => format!("{} can go, and {m} can't go yet", old_copies_words(n)),
    }
}

/// The old copies line under Biggest wins first, with the room the ones that can go would free (what the wins are
/// sorted by), once a look has found any. None when there are none, or no look has finished yet. With only ones that
/// can't go yet, it shows no size: they free nothing now. Once a removal's result is on show (`removing`), it says how
/// that went instead, freeing nothing more, until Measure again.
pub fn line(found: Option<&Found>, removing: &Removing) -> Option<(u64, Line)> {
    let line = |why: String, size: String| Line { id: OLD_COPIES, title: "Old copies after moving folders", why, size };
    match removing.stage() {
        Stage::Sent(sent) => return Some((0, line(sent_words(sent, &old_copies_words), freed_words(sent)))),
        Stage::PuttingBack(sent) => return Some((0, line("Putting them back…".to_owned(), freed_words(sent)))),
        Stage::PutBack(sent, problems) => return Some((0, line(back_words(sent, problems), String::new()))),
        Stage::Picking | Stage::Sending(_) => {}
    }
    let found = found.map(|f| &f.reminder).filter(|f| !f.is_empty())?;
    let bytes = found.bytes();
    let why = format!("Kept when plugin folders moved to another drive: {}", count_words(found));
    Some((bytes, line(why, if bytes == 0 { String::new() } else { size_text(bytes) })))
}

/// "26 Oct 2026".
fn day(at: NaiveDateTime) -> String {
    at.format("%-d %b %Y").to_string()
}

/// A part's path without a slash at its end, so its name and folder can be read from it.
fn trimmed(l: &Leftover) -> &str {
    l.path.trim_end_matches(['\\', '/'])
}

/// What a part's line says under its name: the folder it's in (or came from, once it's gone: `gone`), and when its
/// folder moved when that's known.
fn what_words(l: &Leftover, gone: bool) -> String {
    let folder = folder_of(trimmed(l));
    let since = l.since.map(day);
    let place = if gone { "From" } else { "In" };
    match (folder.is_empty(), since) {
        (false, Some(since)) => format!("{place} {}, moved {since}", wrappable_path(folder)),
        (false, None) => format!("{place} {}", wrappable_path(folder)),
        (true, Some(since)) => format!("Moved {since}"),
        (true, None) => String::new(),
    }
}

/// What a folder left behind is, from the note its record keeps ([`OldHome::note`], the same words in the C# app's
/// records and the Rust app's).
fn behind_words(note: &str) -> String {
    match note {
        "its old place" => "The old place of the folder your moved plugin folders live in.".to_owned(),
        "left by an update" => {
            "A plugin update made it where the link was. Its files have joined the moved copy.".to_owned()
        }
        "a second copy" => "A second copy that a later move made where the link was.".to_owned(),
        "replaced by linking again" => "The files that linking again replaced.".to_owned(),
        "the old copy's newer files, from moving back" => {
            "The old copy's newer files, kept when its folder moved back.".to_owned()
        }
        "" => "Left behind by a move.".to_owned(),
        other => format!("Left behind by a move: {other}."),
    }
}

/// The first few of what's only in one, and how many more: "a.fxp", "a.fxp, b.fxp, c.fxp and 12 more".
fn only_words(only: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut said: Vec<String> = only.iter().take(SHOWN).map(|p| wrappable_path(p)).collect();
    if only.len() > SHOWN {
        said.push(format!("{} more", thousands(only.len() - SHOWN)));
    }
    and_list(&said)
}

/// What a part's status line says: why it can't go yet (a problem), or what's only in it, which keeps it until you've
/// looked (a problem too); otherwise what it is, when it isn't a move's old copy, where its files are now, and whether
/// anything is only in it (or that there's nothing to check it against, so it goes only once you tick it).
fn status_words(l: &Leftover, homes: &[OldHome]) -> (String, bool) {
    if let Some(why) = &l.stays {
        return (why.clone(), true);
    }
    let there = if l.now_in.is_empty() { "where its files went".to_owned() } else { wrappable_path(&l.now_in) };
    if let Some(only) = l.only.as_ref().filter(|only| !only.is_empty()) {
        let (verb, what_to_do) = match only.len() {
            1 => ("is", "copy it over there if you want to keep it, or delete it"),
            _ => ("are", "copy the ones you want to keep over there and delete the others"),
        };
        return (
            format!(
                "{} {verb} only here, or newer here than in {there}, so it stays until nothing is: {what_to_do}, then \
                 Measure again.",
                only_words(only)
            ),
            true,
        );
    }
    let what = match &l.what {
        Left::OldCopy(_) | Left::Unknown => None,
        Left::NotKept(_) => Some("No move was kept for it: the app found it beside its link.".to_owned()),
        Left::Behind(at) => Some(behind_words(homes.get(*at).map_or("", |h| h.note.as_str()))),
        Left::Kept(h) => Some(format!("{} A try that stopped part way left them here.", behind_words(&h.note))),
    };
    let now_in = (!l.now_in.is_empty()).then(|| format!("Its files are now in {there}."));
    let checked = match &l.only {
        Some(_) => "Nothing is only here.",
        None => "There's nothing to check it against, so it isn't ticked: tick it once you've looked in it.",
    };
    (what.into_iter().chain(now_in).chain([checked.to_owned()]).collect::<Vec<_>>().join(" "), false)
}

/// The card's lead.
const LEAD: &str = "When a plugin's folder moved to another drive, its old copy stayed where it was, so you could check \
    the plugin still opens from its new place. Once it does, the old copy is room you can free.";

/// What the card says about Remove, above it: what it does, the checks, and where they go (`can_put_back`: the app can
/// put them back here, which a Mac's Trash doesn't let it). Ones in Program Files (`guarded`: Windows guards them) go
/// together, Windows asking once, and come back together the same way.
fn remove_note(can_put_back: bool, guarded: bool) -> String {
    let checks = format!(
        "Remove sends the ticked ones to the {BIN}, each compared file by file with its copy just before it goes: one \
         with anything only in it, or that the plugin could need now, stays."
    );
    match (can_put_back, guarded) {
        (true, false) => format!("{checks} Put back returns them until you empty the {BIN}."),
        (true, true) => {
            format!("{checks} {ADMIN} Put back returns them until you empty the {BIN}, with Windows asking once again.")
        }
        (false, _) => format!("{checks} Until you empty the {BIN}, Finder's Put Back returns them."),
    }
}

/// The old copies card. While picking (and while Remove is on): the room the ones that can go take, then each one
/// biggest first, with where it is, when its folder moved, what it is and where its files are now, whether anything is
/// only in it (or why it can't go yet), its tick, and a link that shows it in its folder. Every one that can go is
/// ticked out of the box but one with nothing to check it against. `guarded` says whether Windows guards a place (the
/// app passes `mo_space::plugin_space::is_install_folder`): the card says Windows asks once for those. Once a removal
/// has run, the card shows how it went instead ([`Removing`]), until Measure again.
pub fn card(found: Option<&Found>, removing: &Removing, can_put_back: bool, guarded: impl Fn(&str) -> bool) -> Card {
    match removing.stage() {
        Stage::Sent(sent) | Stage::PuttingBack(sent) | Stage::PutBack(sent, _) => {
            result_card(sent, removing.stage(), can_put_back)
        }
        Stage::Picking | Stage::Sending(_) => picking_card(found, removing, can_put_back, guarded),
    }
}

fn picking_card(
    found: Option<&Found>,
    removing: &Removing,
    can_put_back: bool,
    guarded: impl Fn(&str) -> bool,
) -> Card {
    let picking = *removing.stage() == Stage::Picking;
    let reminder = found.map(|f| &f.reminder);
    let homes = found.map(|f| f.homes.as_slice()).unwrap_or_default();
    let leftovers = reminder.map(|r| r.leftovers.as_slice()).unwrap_or_default();
    let parts = leftovers
        .iter()
        .map(|l| {
            let (status, problem) = status_words(l, homes);
            Part {
                name: paths::file_name(trimmed(l)).to_owned(),
                what: what_words(l, false),
                size: size_text(l.bytes),
                tick: (picking && l.can_go()).then(|| (vec![PathBuf::from(&l.path)], removing.ticked(l))),
                status,
                problem,
                open: Some((Opens::InFolder, PathBuf::from(&l.path))),
                ..Part::default()
            }
        })
        .collect();
    let can_go = leftovers.iter().any(Leftover::can_go);
    let note = match removing.stage() {
        Stage::Sending(_) => format!("Checking each ticked one again, then sending it to the {BIN}…"),
        _ if can_go => remove_note(can_put_back, removing.each_ticked(found).any(|l| guarded(&l.path))),
        _ => String::new(),
    };
    Card {
        kind: "OLD COPIES",
        title: "Old copies after moving folders",
        // Only ones that can't go yet free nothing now: said in words, as the line shows no size.
        big: match reminder.map(Reminder::bytes) {
            None => "Measuring…".to_owned(),
            Some(0) if !leftovers.is_empty() => NOTHING_YET.to_owned(),
            Some(bytes) => size_text(bytes),
        },
        big_note: reminder.map(count_words).unwrap_or_default(),
        lead: LEAD.to_owned(),
        ticks: can_go,
        parts,
        note,
        room: String::new(),
        warning: String::new(),
    }
}

/// The card once Remove has run: what was freed, what went and what stayed (each one picked, with how it went), and
/// what Put back does; then, once Put back has run, what came back.
fn result_card(sent: &Sent, stage: &Stage, can_put_back: bool) -> Card {
    let parts = sent
        .picked
        .iter()
        .map(|l| {
            let (status, problem) = result_words(l, sent, stage);
            // Where it is, with a link that shows it there, only while it's there: not once it went, until it's back.
            let gone = gone(l, sent, stage);
            Part {
                name: paths::file_name(trimmed(l)).to_owned(),
                what: what_words(l, gone),
                size: size_text(l.bytes),
                status,
                problem,
                open: (!gone).then(|| (Opens::InFolder, PathBuf::from(&l.path))),
                ..Part::default()
            }
        })
        .collect();
    let (big, big_note, note) = result_figures(sent, stage, can_put_back, &old_copies_words);
    Card {
        kind: "OLD COPIES",
        title: "Old copies after moving folders",
        big,
        big_note,
        lead: LEAD.to_owned(),
        ticks: false,
        parts,
        note,
        room: String::new(),
        warning: String::new(),
    }
}

/// The activity card's line while Remove is on.
pub fn removing_words() -> String {
    format!("Each one is compared with its copy again just before it goes to the {BIN}.")
}

/// The activity card's line once Remove is done: what was freed, what's in the bin, what went but wasn't found there,
/// and what stayed. Err when nothing went (the card then fails).
pub fn removed_words(done: &Done) -> Result<String, String> {
    space_removing::removed_words(done, &old_copies_words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space_bin::{PutBacks, yourself_words};
    use mo_space::clean::{Removed, What};
    use mo_space::plugin_space::SpaceKind;

    const GB: u64 = 1 << 30;

    const SPITFIRE: &str = r"C:\Program Files\Spitfire (old copy)";
    const ARTURIA: &str = r"C:\ProgramData\Arturia (old copy)";
    const UHE: &str = r"C:\ProgramData\u-he (old copy)";
    const KEPT: &str = r"I:\Plugins\STEAM (kept from before)";

    /// Text as it reads, without the marks that let a path wrap after its slashes.
    fn read(text: &str) -> String {
        text.replace('\u{200B}', "")
    }

    fn left(path: &str, bytes: u64, stays: Option<&str>) -> Leftover {
        Leftover {
            path: path.to_owned(),
            bytes,
            what: Left::OldCopy(0),
            now_in: r"I:\Plugins\Spitfire".to_owned(),
            since: NaiveDateTime::parse_from_str("2026-10-26 14:03:12", "%Y-%m-%d %H:%M:%S").ok(),
            stays: stays.map(str::to_owned),
            only: stays.is_none().then(Vec::new),
        }
    }

    fn found(leftovers: Vec<Leftover>) -> Found {
        Found { reminder: Reminder { leftovers }, ..Found::default() }
    }

    fn sample() -> Found {
        found(vec![
            left(SPITFIRE, 60 * GB, None),
            left(ARTURIA, 4 * GB, Some("Arturia beside it isn't a link any more.")),
        ])
    }

    /// One of each: one that can go, one with presets saved since, one with nothing to check it against, and one that
    /// can't go yet.
    fn every_kind() -> Found {
        let saved = Leftover {
            only: Some(vec!["Pads/a.h2p".to_owned(), "Pads/b.h2p".to_owned()]),
            now_in: r"I:\Plugins\u-he".to_owned(),
            ..left(UHE, 2 * GB, None)
        };
        let home = OldHome {
            path: KEPT.to_owned(),
            bytes: GB,
            note: "replaced by linking again".to_owned(),
            copy: String::new(),
            kind: 2,
        };
        let kept =
            Leftover { what: Left::Kept(home), now_in: String::new(), since: None, only: None, ..left(KEPT, GB, None) };
        let mut f = sample();
        f.reminder.leftovers.insert(1, saved);
        f.reminder.leftovers.insert(3, kept);
        f
    }

    /// The card while picking, with nothing in a place Windows guards.
    fn picking(found: Option<&Found>, removing: &Removing) -> Card {
        card(found, removing, true, |_| false)
    }

    fn removed(path: &str, bytes: u64) -> Removed {
        Removed {
            path: path.to_owned(),
            real: path.to_owned(),
            bytes,
            what: What::Plugin(SpaceKind::Sounds),
            when: 1,
            bin_id: Some("1".to_owned()),
            binned: None,
            binned_zone: None,
            stamp: None,
        }
    }

    #[test]
    fn the_line_shows_the_room_the_ones_that_can_go_free() {
        let (bytes, line) = line(Some(&sample()), &Removing::default()).unwrap();
        assert_eq!(bytes, 60 * GB);
        assert_eq!(line.id, OLD_COPIES);
        assert_eq!(line.why, "Kept when plugin folders moved to another drive: 1 old copy can go, and 1 can't go yet");
        assert_eq!(line.size, "60 GB");
        // None found, or not looked yet: no line.
        assert_eq!(super::line(Some(&Found::default()), &Removing::default()), None);
        assert_eq!(super::line(None, &Removing::default()), None);
        // Only ones that can't go yet: no room to free, so no size.
        let staying = found(vec![sample().reminder.leftovers[1].clone()]);
        let (bytes, line) = super::line(Some(&staying), &Removing::default()).unwrap();
        assert_eq!((bytes, line.size.as_str()), (0, ""));
        assert!(line.why.ends_with(": 1 old copy can't go yet"));
        // One with something only in it can't go yet either; one with nothing to check it against can, once ticked.
        let (bytes, line) = super::line(Some(&every_kind()), &Removing::default()).unwrap();
        assert_eq!(bytes, 61 * GB);
        assert!(line.why.ends_with(": 2 old copies can go, and 2 can't go yet"), "{}", line.why);
    }

    #[test]
    fn the_card_lists_each_old_copy_with_where_it_is_and_why_one_stays() {
        let card = picking(Some(&sample()), &Removing::default());
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("60 GB", "1 old copy can go, and 1 can't go yet"));
        let spitfire = &card.parts[0];
        assert_eq!(
            (spitfire.name.as_str(), read(&spitfire.what).as_str(), spitfire.size.as_str()),
            ("Spitfire (old copy)", r"In C:\Program Files, moved 26 Oct 2026", "60 GB")
        );
        assert_eq!(
            (read(&spitfire.status).as_str(), spitfire.problem),
            (r"Its files are now in I:\Plugins\Spitfire. Nothing is only here.", false)
        );
        assert_eq!(spitfire.open, Some((Opens::InFolder, PathBuf::from(SPITFIRE))));
        let arturia = &card.parts[1];
        assert_eq!((arturia.status.as_str(), arturia.problem), ("Arturia beside it isn't a link any more.", true));
        // The one that can go is ticked out of the box; the one that can't has no tick.
        assert!(card.ticks);
        assert_eq!(spitfire.tick, Some((vec![PathBuf::from(SPITFIRE)], true)));
        assert_eq!(arturia.tick, None);
        assert_eq!(card.note, remove_note(true, false));
        // Only ones that can't go yet: nothing to free now, said in words rather than "0 bytes", and nothing to tick.
        let staying = found(vec![sample().reminder.leftovers[1].clone()]);
        let card = picking(Some(&staying), &Removing::default());
        assert_eq!(card.big, NOTHING_YET);
        assert!(!card.ticks && card.note.is_empty());
        // Before a look has finished.
        let first = picking(None, &Removing::default());
        assert_eq!(first.big, "Measuring…");
        assert!(first.parts.is_empty() && first.note.is_empty());
    }

    #[test]
    fn one_with_something_only_in_it_stays_until_you_look_and_one_that_cant_be_checked_waits_for_your_tick() {
        let found = every_kind();
        let mut removing = Removing::default();
        let card = picking(Some(&found), &removing);
        let names: Vec<&str> = card.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Spitfire (old copy)", "u-he (old copy)", "Arturia (old copy)", "STEAM (kept from before)"]);
        // Presets saved since the move: named, no tick, as the mock-up has it.
        let uhe = &card.parts[1];
        assert_eq!(uhe.tick, None);
        assert!(uhe.problem);
        assert_eq!(
            read(&uhe.status),
            r"Pads/a.h2p and Pads/b.h2p are only here, or newer here than in I:\Plugins\u-he, so it stays until nothing is: copy the ones you want to keep over there and delete the others, then Measure again."
        );
        // Kept aside, with nothing to check it against: a tick, but not ticked until you tick it.
        let kept = &card.parts[3];
        assert_eq!(kept.tick, Some((vec![PathBuf::from(KEPT)], false)));
        assert!(!kept.problem);
        assert!(
            kept.status.starts_with("The files that linking again replaced. A try that stopped part way left them")
        );
        assert!(
            kept.status.ends_with(
                "There's nothing to check it against, so it isn't ticked: tick it once you've looked in it."
            )
        );
        // Remove frees what's ticked: Spitfire, then the kept files too once you tick them.
        let button = |r: &Removing| r.remove_button(Some(&found), false, &PutBacks::default());
        assert_eq!(button(&removing), Some(("Remove 60 GB".to_owned(), true)));
        removing.tick(&[PathBuf::from(KEPT)], true);
        assert_eq!(button(&removing), Some(("Remove 61 GB".to_owned(), true)));
        let picked: Vec<String> = removing.ticked_copies(Some(&found)).into_iter().map(|l| l.path).collect();
        assert_eq!(picked, [SPITFIRE, KEPT]);
        // Nothing can be ticked into going that can't go.
        removing.tick(&[PathBuf::from(UHE), PathBuf::from(ARTURIA)], true);
        assert_eq!(removing.ticked_copies(Some(&found)).len(), 2);
        // Waiting while a look is on or something else goes; nothing ticked, nothing to press.
        assert_eq!(
            removing.remove_button(Some(&found), true, &PutBacks::default()),
            Some(("Remove 61 GB".to_owned(), false))
        );
        removing.tick(&[PathBuf::from(SPITFIRE), PathBuf::from(KEPT)], false);
        assert_eq!(button(&removing), Some(("Nothing ticked".to_owned(), false)));
    }

    #[test]
    fn the_note_says_windows_asks_once_for_the_ticked_ones_it_guards() {
        let guarded = |path: &str| path.starts_with(r"C:\Program Files");
        let card = card(Some(&sample()), &Removing::default(), true, guarded);
        assert!(card.note.contains(ADMIN) && card.note.ends_with("with Windows asking once again."), "{}", card.note);
        // Unticked, it isn't going: nothing to ask.
        let mut removing = Removing::default();
        removing.tick(&[PathBuf::from(SPITFIRE)], false);
        assert_eq!(super::card(Some(&sample()), &removing, true, guarded).note, remove_note(true, false));
        // Where the app can't put things back (a Mac), Finder's Put Back is the way.
        assert!(
            picking_card(Some(&sample()), &Removing::default(), false, |_| false)
                .note
                .ends_with("Finder's Put Back returns them.")
        );
    }

    #[test]
    fn once_remove_has_run_the_card_says_how_each_went_and_put_back_brings_them_back() {
        let found = every_kind();
        let mut removing = Removing::default();
        removing.tick(&[PathBuf::from(KEPT)], true);
        let picked = removing.ticked_copies(Some(&found));
        removing.start(picked);
        let sending = picking(Some(&found), &removing);
        assert!(sending.note.starts_with("Checking each ticked one again"));
        assert!(sending.parts.iter().all(|p| p.tick.is_none()));
        assert_eq!(
            removing.remove_button(Some(&found), false, &PutBacks::default()),
            Some(("Removing…".to_owned(), false))
        );
        // Spitfire went; the kept files changed since, so they stayed.
        let why = "Some of it isn't where its files went, is newer here, or isn't the same there: x. Look before deleting it.";
        let done = Done {
            removed: vec![removed(SPITFIRE, 60 * GB)],
            problems: vec![Problem { path: KEPT.to_owned(), why: why.to_owned() }],
            went_with: Vec::new(),
        };
        removing.sent(7, done.clone());
        let card = super::card(Some(&found), &removing, true, |_| false);
        assert_eq!(
            (card.big.as_str(), card.big_note.as_str()),
            ("Freed 60 GB", format!("1 old copy is in the {BIN}, and 1 stayed").as_str())
        );
        assert!(!card.ticks);
        let spitfire = &card.parts[0];
        assert_eq!(
            (read(&spitfire.what).as_str(), spitfire.status.as_str()),
            (r"From C:\Program Files, moved 26 Oct 2026", format!("In the {BIN}.").as_str())
        );
        assert_eq!(spitfire.open, None);
        assert_eq!((card.parts[1].status.as_str(), card.parts[1].problem), (why, true));
        assert!(card.note.starts_with(&format!("They're in the {BIN}. Put back returns each one to where it was")));
        let (bytes, line) = super::line(Some(&found), &removing).unwrap();
        assert_eq!(
            (bytes, line.why.as_str(), line.size.as_str()),
            (0, format!("1 old copy is in the {BIN}, and 1 stayed").as_str(), "Freed 60 GB")
        );
        assert_eq!(removing.put_back_button(true, &PutBacks::default()), Some(("Put back".to_owned(), true)));
        assert_eq!(removing.to_put_back().map(|s| s.coming_back().removed.len()), Some(1));
        // Put back: it's back where it was, and shown there again.
        removing.putting_back(7);
        assert_eq!(super::card(Some(&found), &removing, true, |_| false).big_note, "Putting them back…");
        removing.put_back(7, Vec::new());
        let card = super::card(Some(&found), &removing, true, |_| false);
        assert_eq!(
            (card.big.as_str(), card.big_note.as_str(), card.note.as_str()),
            ("Put back", "It's back where it was", "")
        );
        assert_eq!((card.parts[0].status.as_str(), card.parts[0].open.is_some()), ("Back where it was.", true));
        // One that couldn't come back says why, and what to do.
        let mut stuck = Removing::default();
        stuck.start(vec![found.reminder.leftovers[0].clone()]);
        stuck.sent(8, Done { removed: vec![removed(SPITFIRE, 60 * GB)], ..Done::default() });
        stuck.put_back(
            8,
            vec![Problem { path: SPITFIRE.to_owned(), why: "It isn't in the Recycle Bin any more.".to_owned() }],
        );
        let card = super::card(Some(&found), &stuck, true, |_| false);
        assert_eq!(card.parts[0].status, "It couldn't come back: It isn't in the Recycle Bin any more.");
        assert_eq!(card.note, yourself_words());
        // Measure again goes back to picking.
        stuck.again();
        assert!(!stuck.shows_result());
    }

    #[test]
    fn the_activity_card_counts_old_copies() {
        let done = Done { removed: vec![removed(SPITFIRE, 60 * GB), removed(UHE, 2 * GB)], ..Done::default() };
        assert_eq!(removed_words(&done), Ok(format!("Freed 62 GB: 2 old copies are in the {BIN}.")));
        let none = stayed(&sample().reminder.leftovers, CANT_CHECK);
        assert_eq!(none.problems.len(), 2);
        assert_eq!(removed_words(&none), Err("None could go: the Space page says why.".to_owned()));
    }

    #[test]
    fn a_folder_left_behind_says_what_it_is() {
        let home = |note: &str, copy: &str| OldHome {
            path: r"C:\ProgramData\Plugin Data".to_owned(),
            bytes: GB,
            note: note.to_owned(),
            copy: copy.to_owned(),
            kind: 2,
        };
        let behind = |at: usize, now_in: &str| Leftover {
            what: Left::Behind(at),
            now_in: now_in.to_owned(),
            since: None,
            ..left(r"C:\ProgramData\Plugin Data", GB, None)
        };
        let mut f = found(vec![behind(0, ""), behind(1, r"I:\Plugins\Spitfire")]);
        f.homes = vec![home("its old place", ""), home("left by an update", r"I:\Plugins\Spitfire")];
        let card = picking(Some(&f), &Removing::default());
        assert_eq!(read(&card.parts[0].what), r"In C:\ProgramData");
        assert_eq!(
            card.parts[0].status,
            "The old place of the folder your moved plugin folders live in. Nothing is only here."
        );
        assert_eq!(
            read(&card.parts[1].status),
            r"A plugin update made it where the link was. Its files have joined the moved copy. Its files are now in I:\Plugins\Spitfire. Nothing is only here."
        );
        // Files a try kept aside, and an old copy no move was kept for.
        let kept = Leftover {
            what: Left::Kept(home("replaced by linking again", "")),
            now_in: String::new(),
            only: None,
            ..behind(0, "")
        };
        let found_beside = MovedFolder {
            from: "Kontakt".to_owned(),
            to: r"I:\Plugins\Spitfire".to_owned(),
            old_copy: "Kontakt (old copy)".to_owned(),
            bytes: GB,
            moved: String::new(),
            key: String::new(),
            kind: 2,
            shared_with: Vec::new(),
        };
        let not_kept =
            Leftover { what: Left::NotKept(found_beside), since: None, ..left("Kontakt (old copy)", GB, None) };
        let card = picking(Some(&found(vec![kept, not_kept])), &Removing::default());
        assert_eq!(
            card.parts[0].status,
            "The files that linking again replaced. A try that stopped part way left them here. There's nothing to \
             check it against, so it isn't ticked: tick it once you've looked in it."
        );
        assert_eq!(
            read(&card.parts[1].status),
            r"No move was kept for it: the app found it beside its link. Its files are now in I:\Plugins\Spitfire. Nothing is only here."
        );
        // A bare name is in no folder, so its line says nothing.
        assert_eq!((card.parts[1].name.as_str(), card.parts[1].what.as_str()), ("Kontakt (old copy)", ""));
        // A note the app doesn't know is said as it is.
        assert_eq!(behind_words("kept by hand"), "Left behind by a move: kept by hand.");
    }

    #[test]
    fn what_is_only_in_one_is_said_in_a_few_names() {
        let names = |n: usize| (0..n).map(|i| format!("{i}.fxp")).collect::<Vec<_>>();
        assert_eq!(only_words(&names(1)), "0.fxp");
        assert_eq!(only_words(&names(3)), "0.fxp, 1.fxp and 2.fxp");
        assert_eq!(only_words(&names(15)), "0.fxp, 1.fxp, 2.fxp and 12 more");
        let one = Leftover { only: Some(names(1)), ..left(UHE, GB, None) };
        let (status, problem) = status_words(&one, &[]);
        assert!(problem);
        assert_eq!(
            read(&status),
            r"0.fxp is only here, or newer here than in I:\Plugins\Spitfire, so it stays until nothing is: copy it over there if you want to keep it, or delete it, then Measure again."
        );
    }

    #[test]
    fn counts_read_as_a_sentence() {
        let with = |go: usize, staying: usize| {
            let mut leftovers = vec![left("A", GB, None); go];
            leftovers.extend(vec![left("B", GB, Some("why")); staying]);
            count_words(&Reminder { leftovers })
        };
        assert_eq!(with(1, 0), "1 old copy can go");
        assert_eq!(with(3, 0), "3 old copies can go");
        assert_eq!(with(0, 2), "2 old copies can't go yet");
        assert_eq!(with(2, 3), "2 old copies can go, and 3 can't go yet");
        assert_eq!(with(0, 0), "");
    }

    #[test]
    fn with_no_moves_kept_and_nowhere_to_look_it_finds_none() {
        let stop = AtomicBool::new(false);
        assert_eq!(look(None, None, &[], &stop), Ok(Found::default()));
        // A record that can't be read counts as none.
        assert_eq!(look(Some("not json"), Some("[{"), &[], &stop), Ok(Found::default()));
    }
}
