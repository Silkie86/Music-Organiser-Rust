//! The Space page's old copies (phase 4, part 3): what moving a plugin's folder to another drive left behind. A move
//! copies the folder over, puts a link where it was and keeps the old copy beside the link, so you can check the plugin
//! still opens before the old copy goes. Solomon (3 Nov 2026): the page "should always remind about old copies", with
//! their size, for as long as any is there. `mo_space::old_copies` finds and measures them from the moves the app keeps
//! (library.db, shared with the C# app); this reads those records and says every word of the line and card. Nothing is
//! removed yet: sending the ones that can go to the Recycle Bin, with Put back, comes next.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chrono::NaiveDateTime;
use mo_formats::paths;
use mo_space::Stopped;
use mo_space::after_move::{OldHome, parse_old_homes};
use mo_space::folder_move::{MovedFolder, parse_moved};
use mo_space::old_copies::{self, Left, Leftover, Reminder};
use mo_space::size::size_text;

use crate::shell::{folder_of, wrappable_path};
use crate::space::{Card, Line, Opens, Part};

/// The library settings the moves are kept in, as the C# app keeps them: the moved folders, and the folders moves
/// left behind.
pub const MOVED_FOLDERS: &str = "space_moved_folders";
pub const OLD_HOMES: &str = "space_old_homes";

/// The old copies line's id.
pub const OLD_COPIES: &str = "old-copies";

/// The card's big figure when none of the old copies can go yet.
pub const NOTHING_YET: &str = "Nothing to free yet";

/// What the card says comes next.
pub const NEXT: &str = "Nothing is removed yet. Once you've opened a plugin from its new place and it works, its old \
    copy can go to the Recycle Bin, with Put back: that part comes next.";

/// What a look for old copies found: each one still there, biggest first, and the records it was found from (what a
/// [`Left`] counts into). The card's words come from them, and so will sending the old copies to the Recycle Bin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub reminder: Reminder,
    pub moved: Vec<MovedFolder>,
    pub homes: Vec<OldHome>,
}

/// Looks for old copies: from the moves as the library keeps them (`moved` and `homes`, its [`MOVED_FOLDERS`] and
/// [`OLD_HOMES`] settings; none when there's no record or it can't be read), and in `look_in` (the folders plugins
/// keep their data in) for one no move was kept for. Reads and measures each one: call it away from the window's
/// thread. Stopped, it says so and finds nothing.
pub fn look(moved: Option<&str>, homes: Option<&str>, look_in: &[String], stop: &AtomicBool) -> Result<Found, Stopped> {
    let moved = moved.map(parse_moved).unwrap_or_default();
    let homes = homes.map(parse_old_homes).unwrap_or_default();
    let reminder = old_copies::remind(&moved, &homes, look_in, stop)?;
    Ok(Found { reminder, moved, homes })
}

/// "1 old copy", "3 old copies".
fn old_copies_words(n: usize) -> String {
    if n == 1 { "1 old copy".to_owned() } else { format!("{n} old copies") }
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
/// can't go yet, it shows no size: they free nothing now.
pub fn line(found: Option<&Found>) -> Option<(u64, Line)> {
    let found = found.map(|f| &f.reminder).filter(|f| !f.is_empty())?;
    let bytes = found.bytes();
    let line = Line {
        id: OLD_COPIES,
        title: "Old copies after moving folders",
        why: format!("Kept when plugin folders moved to another drive: {}", count_words(found)),
        size: if bytes == 0 { String::new() } else { size_text(bytes) },
    };
    Some((bytes, line))
}

/// "26 Oct 2026".
fn day(at: NaiveDateTime) -> String {
    at.format("%-d %b %Y").to_string()
}

/// A part's path without a slash at its end, so its name and folder can be read from it.
fn trimmed(l: &Leftover) -> &str {
    l.path.trim_end_matches(['\\', '/'])
}

/// What a part's line says under its name: the folder it's in, and when its folder moved when that's known.
fn what_words(l: &Leftover) -> String {
    let folder = folder_of(trimmed(l));
    let since = l.since.map(day);
    match (folder.is_empty(), since) {
        (false, Some(since)) => format!("In {}, moved {since}", wrappable_path(folder)),
        (false, None) => format!("In {}", wrappable_path(folder)),
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

/// What a part's status line says: why it can't go yet (a problem); or what it is, when it isn't a move's old copy,
/// and where its files are now.
fn status_words(l: &Leftover, homes: &[OldHome]) -> (String, bool) {
    if let Some(why) = &l.stays {
        return (why.clone(), true);
    }
    let what = match &l.what {
        Left::OldCopy(_) | Left::Unknown => None,
        Left::NotKept(_) => Some("No move was kept for it: the app found it beside its link.".to_owned()),
        Left::Behind(at) => Some(behind_words(homes.get(*at).map_or("", |h| h.note.as_str()))),
        Left::Kept(h) => Some(format!("{} A try that stopped part way left them here.", behind_words(&h.note))),
    };
    let now_in = (!l.now_in.is_empty()).then(|| format!("Its files are now in {}.", wrappable_path(&l.now_in)));
    (what.into_iter().chain(now_in).collect::<Vec<_>>().join(" "), false)
}

/// The old copies card: the room the ones that can go take, then each one biggest first, with where it is, when its
/// folder moved, what it is and where its files are now (or why it can't go yet), and a link that shows it in its
/// folder.
pub fn card(found: Option<&Found>) -> Card {
    let reminder = found.map(|f| &f.reminder);
    let homes = found.map(|f| f.homes.as_slice()).unwrap_or_default();
    let leftovers = reminder.map(|r| r.leftovers.as_slice()).unwrap_or_default();
    let parts = leftovers
        .iter()
        .map(|l| {
            let (status, problem) = status_words(l, homes);
            Part {
                name: paths::file_name(trimmed(l)).to_owned(),
                what: what_words(l),
                size: size_text(l.bytes),
                status,
                problem,
                open: Some((Opens::InFolder, PathBuf::from(&l.path))),
                ..Part::default()
            }
        })
        .collect();
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
        lead:
            "When a plugin's folder moved to another drive, its old copy stayed where it was, so you could check the \
               plugin still opens from its new place. Once it does, the old copy is room you can free."
                .to_owned(),
        ticks: false,
        parts,
        note: if leftovers.is_empty() { String::new() } else { NEXT.to_owned() },
        room: String::new(),
        warning: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

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
        }
    }

    fn found(leftovers: Vec<Leftover>) -> Found {
        Found { reminder: Reminder { leftovers }, ..Found::default() }
    }

    fn sample() -> Found {
        found(vec![
            left(r"C:\Program Files\Spitfire (old copy)", 60 * GB, None),
            left(r"C:\ProgramData\Arturia (old copy)", 4 * GB, Some("Arturia beside it isn't a link any more.")),
        ])
    }

    #[test]
    fn the_line_shows_the_room_the_ones_that_can_go_free() {
        let (bytes, line) = line(Some(&sample())).unwrap();
        assert_eq!(bytes, 60 * GB);
        assert_eq!(line.id, OLD_COPIES);
        assert_eq!(line.why, "Kept when plugin folders moved to another drive: 1 old copy can go, and 1 can't go yet");
        assert_eq!(line.size, "60 GB");
        // None found, or not looked yet: no line.
        assert_eq!(super::line(Some(&Found::default())), None);
        assert_eq!(super::line(None), None);
        // Only ones that can't go yet: no room to free, so no size.
        let staying = found(vec![sample().reminder.leftovers[1].clone()]);
        let (bytes, line) = super::line(Some(&staying)).unwrap();
        assert_eq!((bytes, line.size.as_str()), (0, ""));
        assert!(line.why.ends_with(": 1 old copy can't go yet"));
    }

    #[test]
    fn the_card_lists_each_old_copy_with_where_it_is_and_why_one_stays() {
        let card = card(Some(&sample()));
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("60 GB", "1 old copy can go, and 1 can't go yet"));
        let spitfire = &card.parts[0];
        assert_eq!(
            (spitfire.name.as_str(), read(&spitfire.what).as_str(), spitfire.size.as_str()),
            ("Spitfire (old copy)", r"In C:\Program Files, moved 26 Oct 2026", "60 GB")
        );
        assert_eq!(
            (read(&spitfire.status).as_str(), spitfire.problem),
            (r"Its files are now in I:\Plugins\Spitfire.", false)
        );
        assert_eq!(spitfire.open, Some((Opens::InFolder, PathBuf::from(r"C:\Program Files\Spitfire (old copy)"))));
        let arturia = &card.parts[1];
        assert_eq!((arturia.status.as_str(), arturia.problem), ("Arturia beside it isn't a link any more.", true));
        assert!(!card.ticks && card.parts.iter().all(|p| p.tick.is_none()));
        assert_eq!(card.note, NEXT);
        // Only ones that can't go yet: nothing to free now, said in words rather than "0 bytes".
        let staying = found(vec![sample().reminder.leftovers[1].clone()]);
        assert_eq!(super::card(Some(&staying)).big, NOTHING_YET);
        // Before a look has finished.
        let first = super::card(None);
        assert_eq!(first.big, "Measuring…");
        assert!(first.parts.is_empty() && first.note.is_empty());
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
        let card = card(Some(&f));
        assert_eq!(read(&card.parts[0].what), r"In C:\ProgramData");
        assert_eq!(card.parts[0].status, "The old place of the folder your moved plugin folders live in.");
        assert_eq!(
            read(&card.parts[1].status),
            r"A plugin update made it where the link was. Its files have joined the moved copy. Its files are now in I:\Plugins\Spitfire."
        );
        // Files a try kept aside, and an old copy no move was kept for.
        let kept = Leftover {
            what: Left::Kept(home("replaced by linking again", "")),
            now_in: String::new(),
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
        let card = super::card(Some(&found(vec![kept, not_kept])));
        assert_eq!(
            card.parts[0].status,
            "The files that linking again replaced. A try that stopped part way left them here."
        );
        assert_eq!(
            read(&card.parts[1].status),
            r"No move was kept for it: the app found it beside its link. Its files are now in I:\Plugins\Spitfire."
        );
        // A bare name is in no folder, so its line says nothing.
        assert_eq!((card.parts[1].name.as_str(), card.parts[1].what.as_str()), ("Kontakt (old copy)", ""));
        // A note the app doesn't know is said as it is.
        assert_eq!(behind_words("kept by hand"), "Left behind by a move: kept by hand.");
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
