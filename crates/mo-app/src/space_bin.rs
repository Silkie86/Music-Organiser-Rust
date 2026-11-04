//! The Space page's Recycle Bin line and card (the Trash on a Mac or Linux): what the app's clean-ups put there that's
//! still there, as [`crate::binned`] keeps it, with each clean-up's Put back for as long as it's there (the page's
//! question 5: "every clean-up result has a Put back button"). The app never empties the bin itself (Solomon, 3 Nov
//! 2026: "leave it to me with a warning"): this is the warning, under Also taking room, as the page's mock-up has it.

use std::collections::HashMap;

use mo_formats::paths;
use mo_space::clean::Problem;
use mo_space::size::size_text;

use crate::binned::{Binned, Kept};
use crate::songs::thousands;
use crate::space::{Card, Line, Part};
use crate::space_spare::SPARE_COPIES;

/// The line's id.
pub const RECYCLE_BIN: &str = "recycle-bin";

/// What the bin is called here.
pub const BIN: &str = if cfg!(windows) { "Recycle Bin" } else { "Trash" };

/// Whether the app can put things back from the bin here: a Mac's Trash only lets Finder do it (its Put Back), so
/// there the page says to.
pub const CAN_PUT_BACK: bool =
    cfg!(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))));

/// The card's lead, as the mock-up has it.
pub const LEAD: &str = "Everything the app removes goes here first. Empty it yourself once you're sure: the app never \
    does.";

/// The words of a clean-up's Put back link on the card.
pub const PUT_BACK: &str = "Put back";

/// Put back from the page: which clean-up is being put back, and what couldn't come back of each one put back since the
/// app started (by its kind and when it ran). One at a time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PutBacks {
    pub going: Option<(String, i64)>,
    pub problems: HashMap<(String, i64), Vec<Problem>>,
}

impl PutBacks {
    /// A put back starts: none other can until it's [`done`](Self::done).
    pub fn start(&mut self, kind: &str, when: i64) {
        self.going = Some((kind.to_owned(), when));
    }

    /// It's done: what couldn't come back, and why (none when all of it did).
    pub fn done(&mut self, kind: &str, when: i64, problems: Vec<Problem>) {
        if self.going.as_ref().is_some_and(|(k, w)| k == kind && *w == when) {
            self.going = None;
        }
        self.problems.insert((kind.to_owned(), when), problems);
    }
}

/// The Recycle Bin line under Also taking room: what of what the app removed is still in the bin, and the room it
/// takes. None until the bin has been looked in, and when nothing the app put there is.
pub fn line(kept: Option<&Kept>) -> Option<Line> {
    let in_bin = &kept?.in_bin;
    if in_bin.is_empty() {
        return None;
    }
    let why = if in_bin.looked {
        "What the app removed is here until you empty it yourself"
    } else {
        "What the app removed, unless you've emptied it since"
    };
    Some(Line { id: RECYCLE_BIN, title: BIN, why: why.to_owned(), size: size_text(in_bin.bytes) })
}

/// A clean-up's name, as its line on the page has it.
pub fn kind_title(kind: &str) -> &'static str {
    match kind {
        SPARE_COPIES => "Spare plugin copies",
        _ => "A clean-up",
    }
}

/// The activity card's title while a clean-up is put back.
pub fn putting_back_title(kind: &str) -> &'static str {
    match kind {
        SPARE_COPIES => "Putting back spare plugin copies",
        _ => "Putting back what a clean-up removed",
    }
}

/// Why nothing came back when Put back stopped with an error (logged as a crash).
pub fn put_back_crashed() -> String {
    format!("Put back stopped with an error (the log says which), so it may still be in the {BIN}.")
}

/// What a clean-up sent, as a count: "10 plugin copies", "1 file or folder".
fn things(b: &Binned) -> String {
    let n = b.done.removed.len();
    match b.kind.as_str() {
        SPARE_COPIES if n == 1 => "1 plugin copy".to_owned(),
        SPARE_COPIES => format!("{} plugin copies", thousands(n)),
        _ if n == 1 => "1 file or folder".to_owned(),
        _ => format!("{} files and folders", thousands(n)),
    }
}

/// When a clean-up ran, in this computer's time: "4 Nov 2026, 15:20".
pub fn when_words(when: i64) -> String {
    chrono::DateTime::from_timestamp(when, 0)
        .map(|at| at.with_timezone(&chrono::Local).format("%-d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

/// What couldn't come back, in a line: the file and why for one; how many, and why when they all say the same.
pub fn problems_words(problems: &[Problem]) -> String {
    let name = |p: &Problem| paths::file_name(p.path.trim_end_matches(['\\', '/'])).to_owned();
    match problems {
        [] => String::new(),
        [one] => format!("{} couldn't come back: {}", name(one), one.why),
        [first, rest @ ..] if rest.iter().all(|p| p.why == first.why) => {
            format!("{} files or folders couldn't come back: {}", thousands(problems.len()), first.why)
        }
        many => format!("{} couldn't come back, for the reasons the log gives.", thousands(many.len())),
    }
}

/// What to do about what couldn't come back, while it's still in the bin.
pub fn yourself_words() -> String {
    let admin = if cfg!(windows) { " (Windows asks there for administrator rights where it needs them)" } else { "" };
    if CAN_PUT_BACK {
        format!("What couldn't come back is still in the {BIN}: open it to put it back yourself{admin}.")
    } else {
        format!("Put it back from the {BIN}: select it there and choose Put Back.")
    }
}

/// The Recycle Bin card: what the app's clean-ups put there that's still there, newest first, each with its Put back
/// (`can_put_back` says whether the app can here: [`CAN_PUT_BACK`]), how far a put back is, and what couldn't come
/// back. Only one thing goes or comes back at a time: while a clean-up or a put back is on (`cleaning`, `put_backs`),
/// no Put back can start. "Measuring…" until the bin has been looked in.
pub fn card(kept: Option<&Kept>, put_backs: &PutBacks, can_put_back: bool, cleaning: bool) -> Card {
    let list = kept.map(|k| k.list.as_slice()).unwrap_or_default();
    let mut parts = Vec::new();
    for b in list.iter().rev() {
        let key = (b.kind.clone(), b.when);
        let going = put_backs.going.as_ref() == Some(&key);
        let problems = put_backs.problems.get(&key).map(Vec::as_slice).unwrap_or_default();
        let (status, problem) = if going {
            ("Putting back…".to_owned(), false)
        } else {
            (problems_words(problems), !problems.is_empty())
        };
        parts.push(Part {
            name: kind_title(&b.kind).to_owned(),
            what: format!("{}, removed {}", things(b), when_words(b.when)),
            size: size_text(b.bytes()),
            status,
            problem,
            put_back: can_start(put_backs, can_put_back, cleaning).then_some(key),
            ..Part::default()
        });
    }
    let in_bin = kept.map(|k| &k.in_bin);
    let any_problem = parts.iter().any(|p| p.problem);
    let note = match in_bin {
        Some(b) if !b.is_empty() => {
            let mut note = b.note();
            if !can_put_back || any_problem {
                note.push(' ');
                note.push_str(&yourself_words());
            }
            if kept.is_some_and(|k| !k.noted) {
                note.push(' ');
                note.push_str(&unkept_words());
            }
            note
        }
        _ => String::new(),
    };
    Card {
        kind: if cfg!(windows) { "RECYCLE BIN" } else { "TRASH" },
        title: BIN,
        big: match in_bin {
            None => "Measuring…".to_owned(),
            Some(b) if b.is_empty() => "Nothing".to_owned(),
            Some(b) => size_text(b.bytes),
        },
        big_note: match in_bin {
            Some(b) if !b.is_empty() && b.looked => "the app put there, still in it".to_owned(),
            Some(b) if !b.is_empty() => "the app has put there".to_owned(),
            Some(_) => "of what the app removed is still there".to_owned(),
            None => String::new(),
        },
        lead: LEAD.to_owned(),
        ticks: false,
        parts,
        note,
        room: String::new(),
        warning: String::new(),
    }
}

/// Whether a put back can start from the card now: not where the app can't put things back (`can_put_back`), nor
/// while a clean-up (`cleaning`) or another put back is on: one thing goes or comes back at a time.
fn can_start(put_backs: &PutBacks, can_put_back: bool, cleaning: bool) -> bool {
    can_put_back && !cleaning && put_backs.going.is_none()
}

/// The newest clean-up the card can put back now (Put back's key there), by its kind and when it ran: the one at the
/// top of the card, when its Put back can be pressed ([`card`]).
pub fn newest_put_back(
    kept: Option<&Kept>,
    put_backs: &PutBacks,
    can_put_back: bool,
    cleaning: bool,
) -> Option<(String, i64)> {
    if !can_start(put_backs, can_put_back, cleaning) {
        return None;
    }
    kept?.list.last().map(|b| (b.kind.clone(), b.when))
}

/// What the card says when the list couldn't be kept in the app's folder (the disk full, say).
fn unkept_words() -> String {
    format!(
        "This list couldn't be saved in the app's folder (the log says why), so once the app closes, put these back \
         from the {BIN} yourself."
    )
}

/// The activity card's line once a put back is done, from how many were to come back and what couldn't, and why: Err
/// when none could (the card then fails). It says why itself, as the Recycle Bin card closes once nothing of the app's
/// is left in the bin.
pub fn put_back_words(went: usize, problems: &[Problem]) -> Result<String, String> {
    let back = went.saturating_sub(problems.len());
    match (back, problems.len()) {
        (_, 0) if went == 1 => Ok("It's back where it was.".to_owned()),
        (_, 0) => Ok(format!("All {} are back where they were.", thousands(went))),
        (0, _) => Err(problems_words(problems)),
        (1, _) => Ok(format!("1 is back where it was. {}", problems_words(problems))),
        (b, _) => Ok(format!("{} are back where they were. {}", thousands(b), problems_words(problems))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_space::clean::{Done, Removed, What};
    use mo_space::in_bin::InBin;
    use mo_space::plugin_space::SpaceKind;

    const MB: u64 = 1 << 20;

    fn binned(when: i64, paths: &[&str]) -> Binned {
        let removed = paths
            .iter()
            .map(|p| Removed {
                path: (*p).to_owned(),
                real: (*p).to_owned(),
                bytes: 100 * MB,
                what: What::Plugin(SpaceKind::Program),
                when,
                bin_id: None,
                binned: None,
                binned_zone: None,
                stamp: None,
            })
            .collect();
        Binned { kind: SPARE_COPIES.to_owned(), when, done: Done { removed, ..Done::default() } }
    }

    fn kept(list: Vec<Binned>, looked: bool) -> Kept {
        let bytes = list.iter().map(Binned::bytes).sum();
        let items = list.iter().map(|b| b.done.removed.len()).sum();
        Kept { seq: 1, list, in_bin: InBin { items, bytes, since: Some(1), drives: Vec::new(), looked }, noted: true }
    }

    #[test]
    fn the_line_says_what_the_app_put_in_the_bin() {
        assert_eq!(line(None), None);
        assert_eq!(line(Some(&Kept::default())), None);
        let k = kept(vec![binned(1, &[r"C:\A.aaxplugin", r"C:\B.aaxplugin"])], true);
        let l = line(Some(&k)).unwrap();
        assert_eq!((l.id, l.title, l.size.as_str()), (RECYCLE_BIN, BIN, "200 MB"));
        assert_eq!(l.why, "What the app removed is here until you empty it yourself");
        // A bin that can't be looked in may have been emptied since.
        let mac = kept(k.list.clone(), false);
        assert_eq!(line(Some(&mac)).unwrap().why, "What the app removed, unless you've emptied it since");
    }

    #[test]
    fn the_card_lists_each_clean_up_newest_first_with_its_put_back() {
        let k =
            kept(vec![binned(1_762_000_000, &[r"C:\A.aaxplugin"]), binned(1_762_100_000, &[r"C:\B", r"C:\C"])], true);
        let card = card(Some(&k), &PutBacks::default(), true, false);
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("300 MB", "the app put there, still in it"));
        assert_eq!(card.lead, LEAD);
        let names: Vec<(&str, &str)> = card.parts.iter().map(|p| (p.name.as_str(), p.size.as_str())).collect();
        assert_eq!(names, [("Spare plugin copies", "200 MB"), ("Spare plugin copies", "100 MB")]);
        assert!(card.parts[0].what.starts_with("2 plugin copies, removed "));
        assert!(card.parts[1].what.starts_with("1 plugin copy, removed "));
        assert_eq!(card.parts[0].put_back, Some((SPARE_COPIES.to_owned(), 1_762_100_000)));
        // Put back's key puts back the one at the top.
        assert_eq!(newest_put_back(Some(&k), &PutBacks::default(), true, false), card.parts[0].put_back);
        assert_eq!(newest_put_back(None, &PutBacks::default(), true, false), None);
        assert!(card.note.starts_with("3 files and folders"));
        // While one is put back, it says so, and no other can start.
        let mut put_backs = PutBacks::default();
        put_backs.start(SPARE_COPIES, 1_762_100_000);
        let going = super::card(Some(&k), &put_backs, true, false);
        assert_eq!(going.parts[0].status, "Putting back…");
        assert!(going.parts.iter().all(|p| p.put_back.is_none()));
        assert_eq!(newest_put_back(Some(&k), &put_backs, true, false), None);
        // Once it's done, what couldn't come back says why, and what to do.
        let stuck = Problem { path: r"C:\B".to_owned(), why: "It couldn't be put back.".to_owned() };
        put_backs.done(SPARE_COPIES, 1_762_100_000, vec![stuck]);
        let done = super::card(Some(&k), &put_backs, true, false);
        assert_eq!(
            (done.parts[0].status.as_str(), done.parts[0].problem),
            ("B couldn't come back: It couldn't be put back.", true)
        );
        assert!(done.parts[0].put_back.is_some());
        assert!(done.note.ends_with(&yourself_words()));
        // Where the app can't put things back (a Mac), no part has Put back, and the note says how.
        let mac = super::card(Some(&k), &PutBacks::default(), false, false);
        assert!(mac.parts.iter().all(|p| p.put_back.is_none()));
        assert_eq!(newest_put_back(Some(&k), &PutBacks::default(), false, false), None);
        assert!(mac.note.ends_with(&yourself_words()));
        // While a clean-up is on, nothing can be put back until it's done.
        let cleaning = super::card(Some(&k), &PutBacks::default(), true, true);
        assert!(cleaning.parts.iter().all(|p| p.put_back.is_none()));
        assert_eq!(newest_put_back(Some(&k), &PutBacks::default(), true, true), None);
        assert!(!cleaning.note.contains(&yourself_words()));
        // A list that couldn't be saved says what that means.
        let unkept = Kept { noted: false, ..k.clone() };
        assert!(super::card(Some(&unkept), &PutBacks::default(), true, false).note.ends_with(&unkept_words()));
    }

    #[test]
    fn before_a_look_and_with_nothing_there() {
        let first = card(None, &PutBacks::default(), true, false);
        assert_eq!((first.big.as_str(), first.big_note.as_str()), ("Measuring…", ""));
        assert!(first.parts.is_empty() && first.note.is_empty());
        let none = card(Some(&Kept::default()), &PutBacks::default(), true, false);
        assert_eq!(none.big, "Nothing");
        assert!(none.parts.is_empty() && none.note.is_empty());
    }

    #[test]
    fn what_couldnt_come_back_reads_as_a_sentence() {
        let p = |path: &str, why: &str| Problem { path: path.to_owned(), why: why.to_owned() };
        assert_eq!(problems_words(&[]), "");
        assert_eq!(
            problems_words(&[p(r"C:\VST\Serum_x64.dll", "It isn't in the Recycle Bin any more.")]),
            "Serum_x64.dll couldn't come back: It isn't in the Recycle Bin any more."
        );
        let same = [p("a", "It couldn't be put back."), p("b", "It couldn't be put back.")];
        assert_eq!(problems_words(&same), "2 files or folders couldn't come back: It couldn't be put back.");
        let mixed = [p("a", "One."), p("b", "Two.")];
        assert_eq!(problems_words(&mixed), "2 couldn't come back, for the reasons the log gives.");
    }

    #[test]
    fn the_activity_card_says_how_put_back_went() {
        let p = |path: &str, why: &str| Problem { path: path.to_owned(), why: why.to_owned() };
        let stuck = "It couldn't be put back; it's still in the Recycle Bin.";
        assert_eq!(put_back_words(1, &[]), Ok("It's back where it was.".to_owned()));
        assert_eq!(put_back_words(10, &[]), Ok("All 10 are back where they were.".to_owned()));
        assert_eq!(
            put_back_words(1, &[p(r"C:\VST\Serum_x64.dll", stuck)]),
            Err(format!("Serum_x64.dll couldn't come back: {stuck}"))
        );
        assert_eq!(
            put_back_words(2, &[p("a", stuck), p("b", stuck)]),
            Err(format!("2 files or folders couldn't come back: {stuck}"))
        );
        assert_eq!(
            put_back_words(3, &[p("a", "One."), p("b", "Two.")]),
            Ok("1 is back where it was. 2 couldn't come back, for the reasons the log gives.".to_owned())
        );
        assert_eq!(
            put_back_words(3, &[p("Diva.vst3", stuck)]),
            Ok(format!("2 are back where they were. Diva.vst3 couldn't come back: {stuck}"))
        );
        assert!(put_back_crashed().starts_with("Put back stopped with an error"));
        assert_eq!(putting_back_title(SPARE_COPIES), "Putting back spare plugin copies");
    }
}
