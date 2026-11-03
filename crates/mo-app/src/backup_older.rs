//! The Backup page's Older versions card (Solomon, 3 Nov 2026: older versions are kept for ever, with a way to remove
//! them). When a backup replaces a small file you changed, the copy it replaced is kept on the backup drive, in Older
//! versions (`mo_backup::older`). The card says how many there are and the room they take, and its menu removes the
//! ones kept more than a year, 6, 3 or 1 month ago, or all of them: first it asks, with the exact numbers, and only
//! a yes removes them.
//!
//! Words only: `mo-ui` reads the drive and removes on its low-priority threads.

use std::sync::Arc;

use chrono::{Months, NaiveDateTime, NaiveTime};
use mo_backup::older::{Kept, Removed, Run, Why, freed};
use mo_space::size::size_text;

use crate::songs::{plural, thousands};

/// What's known of the older versions on the backup drive.
#[derive(Clone, Copy, Debug)]
pub enum KeptState<'a> {
    /// Not read yet.
    Waiting,
    Ready(&'a Kept),
    /// The drive isn't plugged in, or its Older versions couldn't be read: why, in words.
    Failed(&'a str),
}

/// One line of the Remove older versions menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What the page hands back when it's picked.
    pub id: &'static str,
    pub what: String,
    /// Off when nothing is that old.
    pub enabled: bool,
}

/// The menu's choices: how long ago the older versions were kept, in months (0 for all of them).
const CHOICES: [(&str, u32, &str); 5] = [
    ("older-12", 12, "More than a year old"),
    ("older-6", 6, "More than 6 months old"),
    ("older-3", 3, "More than 3 months old"),
    ("older-1", 1, "More than a month old"),
    ("older-all", 0, "All of them"),
];

/// Everything the Older versions card shows, in words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    /// The room they take ("842 MB"); empty when there are none, or they can't be seen.
    pub size: String,
    /// "in 1,204 older versions", beside the size.
    pub count: String,
    pub note: String,
    /// The Remove menu; empty when there's nothing to remove.
    pub choices: Vec<Choice>,
}

/// The runs a choice takes: those that began before the day it gives (`None`: every run). The start of that day, so a
/// run from earlier on it doesn't go while the question says "kept before" it.
fn cutoff(months: u32, now: NaiveDateTime) -> Option<NaiveDateTime> {
    // A date too far back to work out: nothing is that old.
    (months > 0).then(|| {
        now.checked_sub_months(Months::new(months))
            .map_or(NaiveDateTime::MIN, |day| day.date().and_time(NaiveTime::MIN))
    })
}

/// How many older versions a set of runs holds, and the room removing them gives back (a file the backup shares with
/// another counts only when every name of it goes).
fn count(runs: &[&Run]) -> (usize, u64) {
    let files = runs.iter().map(|r| r.versions.len()).sum();
    (files, freed(runs.iter().flat_map(|r| &r.versions)))
}

/// The card's words. `drive_picked` is whether a backup drive is picked at all.
pub fn lines(drive_picked: bool, state: KeptState<'_>, now: NaiveDateTime) -> Lines {
    let note = |text: &str| Lines { note: text.to_owned(), ..Lines::default() };
    if !drive_picked {
        return note("Pick a backup drive to see the older versions it keeps.");
    }
    let kept = match state {
        KeptState::Waiting => return note("Looking at the older versions on the backup drive…"),
        KeptState::Failed(why) => return note(why),
        KeptState::Ready(kept) => kept,
    };
    let files = kept.files();
    if files == 0 {
        return note(
            "None yet. When a backup replaces a small file you changed, the copy it replaced is kept here, for ever.",
        );
    }
    let runs: Vec<&Run> = kept.runs.iter().filter(|r| !r.versions.is_empty()).collect();
    let first = runs.first().map(|r| r.when.format("%-d %b %Y").to_string()).unwrap_or_default();
    let from = if runs.len() == 1 {
        format!("From one backup, on {first}.")
    } else {
        format!("From {} backups, the first on {first}.", thousands(runs.len()))
    };
    let choices = CHOICES
        .iter()
        .map(|&(id, months, what)| {
            let (files, bytes) = count(&kept.before(cutoff(months, now)));
            let size = if files == 0 { "none".to_owned() } else { size_text(bytes) };
            Choice { id, what: format!("{what} · {size}"), enabled: files > 0 }
        })
        .collect();
    Lines {
        size: size_text(kept.bytes()),
        count: format!("in {}", plural(files, "older version")),
        note: format!("{from} Kept for ever, so a file you saved over by mistake can still be found."),
        choices,
    }
}

/// What a choice from the menu would remove, and the question asked before it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub question: String,
    /// The listing it was asked about (a newer one doesn't change what goes), and the time it cuts at.
    kept: Arc<Kept>,
    cut: Option<NaiveDateTime>,
}

impl Ask {
    /// Exactly what goes if you say yes, as the listing had it, the oldest run first.
    pub fn runs(&self) -> Vec<&Run> {
        self.kept.before(self.cut)
    }
}

/// The question for a menu choice (its `id`), or `None` for a choice that removes nothing (or isn't one).
pub fn ask(kept: &Arc<Kept>, id: &str, now: NaiveDateTime) -> Option<Ask> {
    let &(_, months, _) = CHOICES.iter().find(|(c, _, _)| *c == id)?;
    let cut = cutoff(months, now);
    let runs = kept.before(cut);
    let (files, bytes) = count(&runs);
    if files == 0 {
        return None;
    }
    let what = format!("{} ({})", plural(files, "older version"), size_text(bytes));
    let question = match cut {
        Some(c) => format!("Remove {what}, kept before {}?", c.format("%-d %b %Y")),
        None if files > 1 => format!("Remove all {what}?"),
        None => format!("Remove {what}?"),
    };
    Some(Ask {
        question: format!(
            "{question} They go for good, not to the Recycle Bin. Your music and the rest of the backup stay as they are."
        ),
        kept: Arc::clone(kept),
        cut,
    })
}

/// The page's progress line while they're removed.
pub fn removing_line(done: usize, of: usize) -> String {
    format!("Removing older versions: {} of {}", thousands(done.min(of)), thousands(of))
}

/// What a removal did, in one line for the card.
pub fn removed_line(r: &Removed) -> String {
    let mut line = if r.files == 0 {
        "Nothing removed".to_owned()
    } else {
        format!("Removed {} ({})", plural(r.files, "older version"), size_text(r.bytes))
    };
    if r.gone {
        line = format!("The backup drive went away part way · {line}");
    } else if r.stopped {
        line = format!("Stopped · {line}");
    }
    if !r.left.is_empty() {
        line.push_str(&format!(" · {} stayed: {}", thousands(r.left.len()), stayed_why(&r.left)));
    }
    line
}

/// Why the older versions that stayed stayed, in a few words: each reason with its count when there's more than one.
fn stayed_why(left: &[(std::path::PathBuf, Why)]) -> String {
    let changed = left.iter().filter(|(_, w)| *w == Why::Changed).count();
    let folder = left.iter().filter(|(_, w)| *w == Why::Folder).count();
    let refused = left.len() - changed - folder;
    let reasons = [
        (changed, "changed since they were listed"),
        (folder, "in a folder that isn't as it was"),
        (refused, "couldn't be removed (open in another program?)"),
    ];
    let reasons: Vec<&(usize, &str)> = reasons.iter().filter(|(n, _)| *n > 0).collect();
    match reasons.as_slice() {
        [(_, why)] => (*why).to_owned(),
        many => many.iter().map(|(n, why)| format!("{} {why}", thousands(*n))).collect::<Vec<_>>().join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_backup::older::{Shared, Version};
    use std::path::PathBuf;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    fn run(when: &str, sizes: &[u64]) -> Run {
        let when = at(when);
        Run {
            stamp: mo_backup::older::stamp(when),
            when,
            versions: sizes
                .iter()
                .enumerate()
                .map(|(i, &size)| Version {
                    path: PathBuf::from(format!("/b/{i}")),
                    size,
                    modified: None,
                    shared: None,
                })
                .collect(),
        }
    }

    fn kept() -> Kept {
        Kept {
            runs: vec![
                run("2025-06-01 10:00", &[1000, 2000]),
                run("2026-02-10 09:00", &[]),
                run("2026-09-20 22:40", &[500]),
                run("2026-11-02 23:10", &[250, 250]),
            ],
            others: 1,
        }
    }

    #[test]
    fn the_card_says_what_is_kept_and_what_each_choice_would_free() {
        let now = at("2026-11-03 14:40");
        let k = kept();
        let l = lines(true, KeptState::Ready(&k), now);
        assert_eq!(l.size, "3.91 KB");
        assert_eq!(l.count, "in 5 older versions");
        assert!(l.note.starts_with("From 3 backups, the first on 1 Jun 2025. Kept for ever"), "{}", l.note);
        let menu: Vec<(&str, bool)> = l.choices.iter().map(|c| (c.what.as_str(), c.enabled)).collect();
        assert_eq!(
            menu,
            [
                ("More than a year old · 2.93 KB", true),
                ("More than 6 months old · 2.93 KB", true),
                ("More than 3 months old · 2.93 KB", true),
                ("More than a month old · 3.42 KB", true),
                ("All of them · 3.91 KB", true),
            ]
        );
        // Kept within the last month only: the older choices can't be picked.
        let recent = Kept { runs: vec![run("2026-11-01 10:00", &[10])], others: 0 };
        let l = lines(true, KeptState::Ready(&recent), now);
        assert!(l.note.starts_with("From one backup, on 1 Nov 2026."));
        assert_eq!(l.choices.iter().filter(|c| c.enabled).count(), 1);
        assert_eq!(l.choices[0].what, "More than a year old · none");
    }

    #[test]
    fn the_card_says_why_when_there_is_nothing_to_show() {
        let now = at("2026-11-03 14:40");
        assert!(lines(false, KeptState::Waiting, now).note.starts_with("Pick a backup drive"));
        assert!(lines(true, KeptState::Waiting, now).note.starts_with("Looking"));
        let failed = lines(true, KeptState::Failed("The backup drive isn't plugged in"), now);
        assert_eq!(failed.note, "The backup drive isn't plugged in");
        let none = lines(true, KeptState::Ready(&Kept::default()), now);
        assert!(none.note.starts_with("None yet") && none.size.is_empty() && none.choices.is_empty());
    }

    #[test]
    fn asking_names_exactly_what_goes() {
        let now = at("2026-11-03 14:40");
        let k = Arc::new(kept());
        let a = ask(&k, "older-6", now).unwrap();
        assert!(a.question.starts_with("Remove 2 older versions (2.93 KB), kept before 3 May 2026? They go for good"));
        // The empty February run goes along, so its folder is tidied away; September and November stay.
        let stamps: Vec<&str> = a.runs().iter().map(|r| r.stamp.as_str()).collect();
        assert_eq!(stamps, ["2025-06-01 10.00", "2026-02-10 09.00"]);
        let all = ask(&k, "older-all", now).unwrap();
        assert!(all.question.starts_with("Remove all 5 older versions (3.91 KB)?"));
        assert_eq!(all.runs().len(), 4);
        let one = Arc::new(Kept { runs: vec![run("2026-01-01 10:00", &[10])], others: 0 });
        assert!(ask(&one, "older-all", now).unwrap().question.starts_with("Remove 1 older version (10 bytes)?"));
        // Nothing that old, or not a choice: nothing to ask.
        assert_eq!(ask(&one, "older-12", now), None);
        assert_eq!(ask(&k, "everything", now), None);
    }

    #[test]
    fn kept_before_a_day_means_before_that_day_began() {
        // Six months before 14:40 on 3 Nov is 3 May: a run from 09:15 that morning isn't "kept before 3 May".
        let now = at("2026-11-03 14:40");
        let k =
            Arc::new(Kept { runs: vec![run("2026-05-02 23:59", &[10]), run("2026-05-03 09:15", &[20])], others: 0 });
        let a = ask(&k, "older-6", now).unwrap();
        assert!(a.question.starts_with("Remove 1 older version (10 bytes), kept before 3 May 2026?"), "{}", a.question);
        assert_eq!(a.runs().len(), 1);
    }

    #[test]
    fn shared_bytes_count_once_and_only_when_every_name_goes() {
        let now = at("2026-11-03 14:40");
        let shared = |names| Some(Shared { id: (1, 7), names });
        let mut old = run("2025-01-01 10:00", &[4000, 500]);
        // Shares its bytes with the backup's current copy of the file: removing it frees nothing.
        old.versions[0].shared = shared(2);
        let k = Arc::new(Kept { runs: vec![old], others: 0 });
        let l = lines(true, KeptState::Ready(&k), now);
        assert_eq!((l.size.as_str(), l.count.as_str()), ("500 bytes", "in 2 older versions"));
        assert!(ask(&k, "older-all", now).unwrap().question.starts_with("Remove all 2 older versions (500 bytes)?"));
    }

    #[test]
    fn a_removal_says_what_it_did() {
        let done = Removed { files: 412, bytes: 230 << 20, ..Removed::default() };
        assert_eq!(removed_line(&done), "Removed 412 older versions (230 MB)");
        let left = Removed { files: 1, bytes: 10, left: vec![(PathBuf::from("/b/x"), Why::Changed)], ..done.clone() };
        assert_eq!(
            removed_line(&left),
            "Removed 1 older version (10 bytes) · 1 stayed: changed since they were listed"
        );
        let mixed = Removed {
            left: vec![
                (PathBuf::from("/b/x"), Why::Changed),
                (PathBuf::from("/b/y"), Why::Refused("in use".into())),
                (PathBuf::from("/b/z"), Why::Refused("in use".into())),
            ],
            ..done.clone()
        };
        assert!(
            removed_line(&mixed).ends_with(
                "· 3 stayed: 1 changed since they were listed, 2 couldn't be removed (open in another program?)"
            ),
            "{}",
            removed_line(&mixed)
        );
        let stopped = Removed { stopped: true, ..done.clone() };
        assert!(removed_line(&stopped).starts_with("Stopped · Removed 412"));
        let gone = Removed { gone: true, files: 0, bytes: 0, ..Removed::default() };
        assert_eq!(removed_line(&gone), "The backup drive went away part way · Nothing removed");
        assert_eq!(removing_line(120, 412), "Removing older versions: 120 of 412");
    }
}
