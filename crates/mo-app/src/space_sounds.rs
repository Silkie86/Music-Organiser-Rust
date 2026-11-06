//! The Space page's copies of the same sound: information only, as its question round agreed (question 4). How many
//! sounds your sample folders hold more than once, as the Sounds page's Hide duplicates finds them: only sounds whose
//! audio is exactly the same ([`mo_sounds::copies`]), never ones that only sound alike (review E1-2, 6 Nov: the card
//! and the switch agree on what a copy is, which one to keep and which are hidden). The one to keep is the one that
//! page shows ([`SoundList::copy_groups`]). Nothing here moves, hides or removes anything: samples never move or go
//! (rule 2), and copies only ever hide in the app.

use std::path::PathBuf;

use mo_formats::paths;
use mo_space::size::size_text;

use crate::drag_out::count_of;
use crate::shell::{folder_of, wrappable_path};
use crate::songs::thousands;
use crate::sounds::{CopyGroup, KeptFor, SoundList};
use crate::space::{Card, Line, Opens, Part};

/// The line's id, and its card's.
pub const SOUND_COPIES: &str = "sound-copies";

/// What the line says while the sounds are read, or the first look counts their copies.
pub const LOOKING: &str = "Reading your sounds…";

/// How many sounds with copies the card lists, those whose copies take the most room first; the rest add up in one
/// line.
pub const LISTED: usize = 20;

/// How many of a sound's files the card lists under it, the one to keep first; the rest add up in one line.
pub const FILES: usize = 5;

const KIND: &str = "SOUND COPIES";
const TITLE: &str = "Copies of the same sound";

/// What the card says of what its copies are, before how many there are.
const WHAT_THEY_ARE: &str = "Sounds your sample folders hold more than once: the very same audio, even renamed or \
    saved in another file.";

/// What the card says of how copies are found, which one is kept, and that nothing goes from here.
pub const NOTE: &str = "Only sounds whose audio is exactly the same count, as the Sounds page's Hide duplicates finds \
    them: sounds that only sound alike never do. The one to keep is the one that page shows: the one your projects \
    use most, then a favourite, more stars, more plays. Nothing goes from here: your samples stay where they are.";

/// What a copy's line says while Hide duplicates is on.
pub const HIDDEN: &str = "Hidden by Hide duplicates";

/// Why a copy is one.
const SAME: &str = "The very same audio";

/// One file of a sound with copies: where it is, its size, whether Hide duplicates hides it, and why it's the one to
/// keep or that it's a copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub bytes: u64,
    pub hidden: bool,
    pub why: String,
}

/// A sound with copies, as the card lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    /// The one to keep's name, and its id for its link to the Sounds page.
    pub name: String,
    pub id: Option<i64>,
    /// Its files, the one to keep first: the first [`FILES`] of them, and how many more there are.
    pub files: Vec<File>,
    pub more: usize,
    /// How many copies it has (its files but the one to keep), and how many of them Hide duplicates hides.
    pub copies: usize,
    pub hidden: usize,
    /// The room its copies take.
    pub bytes: u64,
}

/// What a look found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// How many sounds have copies; how many copies there are, and how many of them Hide duplicates hides.
    pub sounds: usize,
    pub copies: usize,
    pub hidden: usize,
    /// The room the copies take, the ones Hide duplicates hides too: hiding leaves the file where it is. (Sounds hidden
    /// in the old app aren't on the Sounds page, so they aren't counted.)
    pub bytes: u64,
    /// The sounds whose copies take the most room, the first [`LISTED`].
    pub listed: Vec<Copied>,
}

/// What the card shows of the copies the Sounds page's `list` knows ([`SoundList::copy_groups`]; none before a look
/// has found them), with Hide duplicates on (`hiding`) or off. Goes through every group: call it away from the
/// window's thread.
pub fn look(list: &SoundList, hiding: bool) -> Found {
    let groups = list.copy_groups();
    let mut found = Found { sounds: groups.len(), ..Found::default() };
    for g in &groups {
        let copies = g.sounds.len() - 1;
        found.copies += copies;
        if hiding {
            found.hidden += copies;
        }
        found.bytes = found.bytes.saturating_add(g.spare_bytes());
    }
    found.listed = groups.iter().take(LISTED).map(|g| copied(list, g, hiding)).collect();
    found
}

fn copied(list: &SoundList, g: &CopyGroup, hiding: bool) -> Copied {
    let keep = g.sounds.first().and_then(|&(i, _)| list.sound(i));
    let copies = g.sounds.len() - 1;
    let files = g.sounds.iter().take(FILES).enumerate().filter_map(|(n, &(i, bytes))| {
        let s = list.sound(i)?;
        let why = if n == 0 { keep_why(list, g) } else { SAME.to_owned() };
        Some(File { path: s.path.clone(), bytes, hidden: hiding && n > 0, why })
    });
    Copied {
        name: keep.map(|s| s.name().to_owned()).unwrap_or_default(),
        id: keep.map(|s| s.id),
        files: files.collect(),
        more: g.sounds.len().saturating_sub(FILES),
        copies,
        hidden: if hiding { copies } else { 0 },
        bytes: g.spare_bytes(),
    }
}

/// Why the first of `g` is the one to keep, against the next: the first thing [`SoundList::copy_groups`]' order
/// goes by that tells them apart.
fn keep_why(list: &SoundList, g: &CopyGroup) -> String {
    let (Some(&(a, _)), Some(&(b, _))) = (g.sounds.first(), g.sounds.get(1)) else { return String::new() };
    let reason = match list.why_kept(a, b) {
        KeptFor::Used => format!("used in {}", count_of(list.used_in(a).unwrap_or(0) as usize, "project", "projects")),
        KeptFor::Favourite => "a favourite".to_owned(),
        KeptFor::Stars => "more stars".to_owned(),
        KeptFor::Plays => "played more".to_owned(),
        KeptFor::Tags => "more tags".to_owned(),
        KeptFor::Older => "the oldest file".to_owned(),
        KeptFor::Path => "first by its folder".to_owned(),
    };
    format!("Best to keep: {reason}")
}

/// "2 copies, 1 hidden by Hide duplicates", leaving out what there's none of.
fn kinds(copies: usize, hidden: usize) -> String {
    let mut said = count_of(copies, "copy", "copies");
    if hidden > 0 {
        said.push_str(&format!(", {} hidden by Hide duplicates", thousands(hidden)));
    }
    said
}

/// "1,234 sounds have copies: 2,345 copies, 2,345 hidden by Hide duplicates".
fn counts(f: &Found) -> String {
    format!("{}: {}", count_of(f.sounds, "sound has copies", "sounds have copies"), kinds(f.copies, f.hidden))
}

/// The line under Also taking room: how many sounds have copies and what's in them, with the room the copies take.
/// While the sounds are read or the first look counts their copies (`looking`, nothing found yet) it says so; none
/// when a look found no copies, or none has finished and none is on.
pub fn line(found: Option<&Found>, looking: bool) -> Option<Line> {
    let line = |why: String, size: String| Line { id: SOUND_COPIES, title: TITLE, why, size };
    match found {
        Some(f) if f.sounds > 0 => Some(line(counts(f), size_text(f.bytes))),
        None if looking => Some(line(LOOKING.to_owned(), "Measuring…".to_owned())),
        _ => None,
    }
}

/// The card: the room the copies take, how many sounds have copies, then the sounds whose copies take the most room,
/// the first [`LISTED`], each with a link that shows it on the Sounds page and its files under it, the one to keep
/// first (the first [`FILES`]), each with why, where it is and a link that shows it in its folder. The rest add up in
/// one line.
pub fn card(found: Option<&Found>, looking: bool) -> Card {
    let mut parts = Vec::new();
    if let Some(f) = found {
        for c in &f.listed {
            parts.push(Part {
                name: c.name.clone(),
                what: kinds(c.copies, c.hidden),
                size: size_or_none(c.bytes),
                sound: c.id,
                ..Part::default()
            });
            parts.extend(c.files.iter().map(file_part));
            if c.more > 0 {
                parts.push(Part {
                    name: format!("And {}", count_of(c.more, "more file", "more files")),
                    under: true,
                    ..Part::default()
                });
            }
        }
        let rest = f.sounds.saturating_sub(f.listed.len());
        if rest > 0 {
            let listed = f.listed.iter().fold(0u64, |sum, c| sum.saturating_add(c.bytes));
            parts.push(Part {
                name: format!("And {}", count_of(rest, "more sound with copies", "more sounds with copies")),
                size: size_or_none(f.bytes.saturating_sub(listed)),
                ..Part::default()
            });
        }
    }
    Card {
        kind: KIND,
        title: TITLE,
        big: match found {
            None if looking => "Measuring…".to_owned(),
            None => String::new(),
            Some(f) => size_text(f.bytes),
        },
        big_note: match found {
            Some(f) if f.copies > 0 => format!("in {}", count_of(f.copies, "copy", "copies")),
            _ => String::new(),
        },
        lead: match found {
            Some(f) if f.sounds > 0 => format!("{WHAT_THEY_ARE} {}.", counts(f)),
            Some(_) => "No two of your sounds have exactly the same audio.".to_owned(),
            None if looking => LOOKING.to_owned(),
            None => String::new(),
        },
        ticks: false,
        parts,
        note: if found.is_some_and(|f| f.sounds > 0) { NOTE.to_owned() } else { String::new() },
        room: String::new(),
        warning: String::new(),
    }
}

/// A size as a part shows it: none for nothing.
fn size_or_none(bytes: u64) -> String {
    if bytes == 0 { String::new() } else { size_text(bytes) }
}

/// A file's line under its sound: its name, why it's kept or that it's a copy and the folder it's in, its size,
/// whether Hide duplicates hides it, and a link that shows it in its folder.
fn file_part(f: &File) -> Part {
    let folder = wrappable_path(folder_of(&f.path));
    Part {
        name: paths::file_name(&f.path).to_owned(),
        what: if f.why.is_empty() { format!("In {folder}") } else { format!("{} · in {folder}", f.why) },
        size: size_or_none(f.bytes),
        under: true,
        status: if f.hidden { HIDDEN.to_owned() } else { String::new() },
        open: Some((Opens::InFolder, PathBuf::from(&f.path))),
        ..Part::default()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use mo_sounds::Sound;
    use mo_sounds::copies::SameAudio;
    use mo_sounds::usage::Usage;

    use super::*;

    /// Kicks: three files of the very same audio, the second one your songs use; and two snares, the same too.
    fn library() -> SoundList {
        let sounds = vec![
            Sound::new(1, r"D:\Samples\Kicks\Kick.wav"),
            Sound::new(2, r"D:\Packs\Drums\Kick copy.wav"),
            Sound::new(3, r"E:\Backup\Kick (2).wav"),
            Sound::new(4, r"D:\Samples\Snares\Snare.wav"),
            Sound::new(5, r"D:\Samples\Snares\Snare again.wav"),
            Sound::new(6, r"D:\Samples\Hat.wav"),
        ];
        let mut list = SoundList::new(sounds);
        list.set_used(HashMap::from([(2, Usage { projects: 3, ..Usage::default() })]));
        list.set_copies(&SameAudio {
            groups: vec![vec![1, 2, 3], vec![4, 5]],
            sizes: HashMap::from([(1, 1_000_000), (2, 1_000_000), (3, 1_000_000), (4, 200_000), (5, 200_000)]),
        });
        list
    }

    #[test]
    fn copies_are_the_ones_hide_duplicates_finds_and_the_one_it_shows_is_kept() {
        let list = library();
        let found = look(&list, false);
        assert_eq!((found.sounds, found.copies, found.hidden, found.bytes), (2, 3, 0, 2_200_000));
        let kick = &found.listed[0];
        assert_eq!((kick.name.as_str(), kick.id, kick.copies, kick.bytes), ("Kick copy", Some(2), 2, 2_000_000));
        let files: Vec<(&str, bool, &str)> =
            kick.files.iter().map(|f| (f.path.as_str(), f.hidden, f.why.as_str())).collect();
        assert_eq!(
            files,
            [
                (r"D:\Packs\Drums\Kick copy.wav", false, "Best to keep: used in 3 projects"),
                (r"D:\Samples\Kicks\Kick.wav", false, SAME),
                (r"E:\Backup\Kick (2).wav", false, SAME),
            ]
        );
        // Hide duplicates on: its copies count as hidden, the one to keep doesn't.
        let on = look(&list, true);
        assert_eq!((on.hidden, on.listed[0].hidden, on.listed[1].hidden), (3, 2, 1));
        assert_eq!(on.listed[0].files.iter().map(|f| f.hidden).collect::<Vec<_>>(), [false, true, true]);
        // Nothing found yet: nothing counted.
        assert_eq!(look(&SoundList::new(vec![Sound::new(1, "a.wav")]), true), Found::default());
    }

    #[test]
    fn the_line_and_card_say_how_many_sounds_have_copies_and_their_room() {
        let found = look(&library(), true);
        let line = super::line(Some(&found), false).unwrap();
        assert_eq!(line.id, SOUND_COPIES);
        assert_eq!(line.why, "2 sounds have copies: 3 copies, 3 hidden by Hide duplicates");
        assert_eq!(line.size, "2.1 MB");
        assert_eq!(super::line(None, true).map(|l| l.why), Some(LOOKING.to_owned()));
        assert_eq!(super::line(Some(&Found::default()), true), None);
        assert_eq!(super::line(None, false), None);
        let card = card(Some(&found), false);
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("2.1 MB", "in 3 copies"));
        assert_eq!(card.note, NOTE);
        assert_eq!(card.parts[0].sound, Some(2));
        assert_eq!(card.parts[0].what, "2 copies, 2 hidden by Hide duplicates");
        assert_eq!((card.parts[2].name.as_str(), card.parts[2].status.as_str()), ("Kick.wav", HIDDEN));
        assert_eq!(card.parts[1].status, "");
        assert_eq!(card.parts.len(), 7);
        assert!(card.parts.iter().all(|p| p.tick.is_none()), "nothing goes from here");
        let none = super::card(Some(&Found::default()), false);
        assert_eq!(none.lead, "No two of your sounds have exactly the same audio.");
        let looking = super::card(None, true);
        assert_eq!((looking.big.as_str(), looking.lead.as_str()), ("Measuring…", LOOKING));
    }

    #[test]
    fn a_long_list_shows_the_biggest_and_adds_up_the_rest() {
        // 22 sounds with copies: sound n has n + 1 files of n thousand bytes each, the 22nd eight.
        let mut sounds = Vec::new();
        let mut groups = Vec::new();
        let mut sizes = HashMap::new();
        let mut id = 0;
        for n in 1..=22u64 {
            let files = if n == 22 { 8 } else { 2 };
            let mut g = Vec::new();
            for f in 0..files {
                id += 1;
                sounds.push(Sound::new(id, &format!(r"D:\S\{n}\{f}.wav")));
                sizes.insert(id, if n == 22 { 22_000 } else { n * 1000 });
                g.push(id);
            }
            groups.push(g);
        }
        let mut list = SoundList::new(sounds);
        list.set_copies(&SameAudio { groups, sizes });
        let found = look(&list, false);
        assert_eq!((found.sounds, found.copies, found.listed.len()), (22, 21 + 7, LISTED));
        let biggest = &found.listed[0];
        assert_eq!((biggest.files.len(), biggest.more, biggest.bytes), (FILES, 3, 7 * 22_000));
        let card = card(Some(&found), false);
        assert_eq!((card.parts[FILES + 1].name.as_str(), card.parts[FILES + 1].under), ("And 3 more files", true));
        assert_eq!(card.parts.last().map(|p| p.name.as_str()), Some("And 2 more sounds with copies"));
        assert_eq!(found.bytes, 7 * 22_000 + (1..=21).map(|n| n * 1000).sum::<u64>());
    }
}
