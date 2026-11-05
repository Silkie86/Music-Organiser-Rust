//! The Space page's copies of the same sound: information only, as its question round agreed (question 4). How many
//! sounds your sample folders hold more than once, even renamed, converted to another format or trimmed, as the old
//! app's duplicate finder found them, and the room the copies that are the same sound take. [`mo_sounds::duplicates`]
//! groups them, the one to keep first, as the old app's Duplicates page did. Nothing here moves, hides or removes
//! anything: samples never move or go (rule 2), and copies only ever hide in the app.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use mo_formats::paths;
use mo_sounds::Sound;
pub use mo_sounds::duplicates::Finds;
use mo_sounds::duplicates::{self, DuplicateGroup};
use mo_sounds::usage::Usage;
use mo_space::size::size_text;

use crate::drag_out::count_of;
use crate::shell::{folder_of, wrappable_path};
use crate::songs::thousands;
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
const WHAT_THEY_ARE: &str =
    "Sounds your sample folders hold more than once, even renamed, converted to another format or trimmed.";

/// What the card says of where the copies come from, what its room is, and that nothing goes from here.
const NOTE: &str = "As the old app's duplicate finder found them. The room is what the copies that are the same sound \
    take, hidden ones too (hiding leaves the file where it is); very similar ones may differ on purpose, so they don't \
    count. Nothing goes from here: your samples stay where they are.";

/// What a hidden file's line says.
const HIDDEN: &str = "Hidden in the app";

/// One file of a sound with copies: where it is, its size, whether it's hidden in the app, and why it's the one to
/// keep or how it's a copy ("Exact copy, renamed").
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
    /// The one to keep's name, and its id for its link to the Sounds page (none when it's hidden: that page doesn't
    /// list it).
    pub name: String,
    pub id: Option<i64>,
    /// Its files, the one to keep first: the first [`FILES`] of them, and how many more there are.
    pub files: Vec<File>,
    pub more: usize,
    /// Of its copies: how many are the same sound, only very similar, and hidden (of either; the one to keep is
    /// hidden only when every file is).
    pub same: usize,
    pub similar: usize,
    pub hidden: usize,
    /// The room its copies that are the same sound take.
    pub bytes: u64,
}

/// What a look found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// How many sounds have copies; of their copies, how many are the same sound, only very similar, and hidden (of
    /// either).
    pub sounds: usize,
    pub same: usize,
    pub similar: usize,
    pub hidden: usize,
    /// The room the copies that are the same sound take, hidden ones too: hiding leaves the file where it is.
    pub bytes: u64,
    /// The sounds whose copies take the most room, the first [`LISTED`].
    pub listed: Vec<Copied>,
}

/// Groups the copies `finds` holds of `sounds` (the Sounds page's list, which leaves hidden copies out: `finds` has
/// those) as the old app did, the one to keep first (`usage`: how your songs use each sound, once that's known; one
/// they use is the one to keep), and what the page shows of them. Goes through the whole list once, and every copy the
/// finder found: call it away from the window's thread.
pub fn look(sounds: &[Sound], usage: Option<&HashMap<i64, Usage>>, finds: &Finds) -> Found {
    // The sounds with copies, found once: the grouping and the card only ever look up these.
    let copied: HashSet<i64> =
        finds.matches.iter().flat_map(|m| [m.a, m.b]).chain(finds.exact.iter().map(|e| e.0)).collect();
    let by_id: HashMap<i64, &Sound> =
        sounds.iter().chain(&finds.hidden).filter(|s| copied.contains(&s.id)).map(|s| (s.id, s)).collect();
    let none = HashMap::new();
    let groups =
        duplicates::groups(by_id.values().copied(), usage.unwrap_or(&none), &finds.matches, &finds.exact, &finds.sizes);
    let hidden: HashSet<i64> = finds.hidden.iter().map(|s| s.id).collect();
    found_of(&groups, &by_id, &hidden)
}

/// What the page shows of `groups` (biggest saving first): every count, and the first [`LISTED`] with their files,
/// each sound found in `by_id`. `hidden`: the sounds you've hidden.
fn found_of(groups: &[DuplicateGroup], by_id: &HashMap<i64, &Sound>, hidden: &HashSet<i64>) -> Found {
    let listed = groups.get(..LISTED.min(groups.len())).unwrap_or_default();
    let mut found = Found { sounds: groups.len(), ..Found::default() };
    for g in groups {
        let (same, similar, hid) = counts_of(g, hidden);
        found.same += same;
        found.similar += similar;
        found.hidden += hid;
        found.bytes = found.bytes.saturating_add(bytes(g.spare_bytes));
    }
    found.listed = listed.iter().map(|g| copied(g, by_id, hidden)).collect();
    found
}

/// Of a group's copies (not the one to keep): how many are the same sound as the one to keep, only very similar, and
/// hidden.
fn counts_of(g: &DuplicateGroup, hidden: &HashSet<i64>) -> (usize, usize, usize) {
    let copies = || g.members.iter().filter(|m| !m.keep);
    (g.same_copies(), copies().filter(|m| !m.same).count(), copies().filter(|m| hidden.contains(&m.id)).count())
}

fn copied(g: &DuplicateGroup, by_id: &HashMap<i64, &Sound>, hidden: &HashSet<i64>) -> Copied {
    let keep = g.members.first();
    let (same, similar, hid) = counts_of(g, hidden);
    let files = g.members.iter().take(FILES).filter_map(|m| {
        let s = by_id.get(&m.id)?;
        Some(File { path: s.path.clone(), bytes: bytes(m.size), hidden: hidden.contains(&m.id), why: m.why.clone() })
    });
    Copied {
        name: keep.and_then(|m| by_id.get(&m.id)).map(|s| s.name().to_owned()).unwrap_or_default(),
        id: keep.filter(|m| !hidden.contains(&m.id)).map(|m| m.id),
        files: files.collect(),
        more: g.members.len().saturating_sub(FILES),
        same,
        similar,
        hidden: hid,
        bytes: bytes(g.spare_bytes),
    }
}

/// A size the library keeps (none known, or damaged, below 0: none).
fn bytes(size: i64) -> u64 {
    u64::try_from(size).unwrap_or(0)
}

/// "2 the same sound, 1 only very similar, 1 hidden", as the old app said it, leaving out what there's none of.
fn kinds(same: usize, similar: usize, hidden: usize) -> String {
    let said: Vec<String> = [(same, "the same sound"), (similar, "only very similar"), (hidden, "hidden")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{} {what}", thousands(n)))
        .collect();
    said.join(", ")
}

/// "1,234 sounds have copies: 2,345 the same sound, 456 only very similar, 12 hidden".
fn counts(f: &Found) -> String {
    format!("{} copies: {}", count_of(f.sounds, "sound has", "sounds have"), kinds(f.same, f.similar, f.hidden))
}

/// The line under Also taking room: how many sounds have copies and what's in them, with the room the copies that are
/// the same sound take. While the sounds are read or the first look counts their copies (`looking`, nothing found
/// yet) it says so; none when a look found no copies, or none has finished and none is on.
pub fn line(found: Option<&Found>, looking: bool) -> Option<Line> {
    let line = |why: String, size: String| Line { id: SOUND_COPIES, title: TITLE, why, size };
    match found {
        Some(f) if f.sounds > 0 => Some(line(counts(f), size_text(f.bytes))),
        None if looking => Some(line(LOOKING.to_owned(), "Measuring…".to_owned())),
        _ => None,
    }
}

/// The card: the room the copies that are the same sound take, how many sounds have copies and what they are, then
/// the sounds whose copies take the most room, the first [`LISTED`], each with a link that shows it on the Sounds page
/// and its files under it, the one to keep first (the first [`FILES`]), each with why, where it is and a link that
/// shows it in its folder. The rest add up in one line.
pub fn card(found: Option<&Found>, looking: bool) -> Card {
    let mut parts = Vec::new();
    if let Some(f) = found {
        for c in &f.listed {
            parts.push(Part {
                name: c.name.clone(),
                what: format!(
                    "{}: {}",
                    count_of(c.same + c.similar, "copy", "copies"),
                    kinds(c.same, c.similar, c.hidden)
                ),
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
            Some(f) if f.same > 0 => format!("in {}", count_of(f.same, "copy", "copies")),
            _ => String::new(),
        },
        lead: match found {
            Some(f) if f.sounds > 0 => format!("{WHAT_THEY_ARE} {}.", counts(f)),
            Some(_) => "The old app's duplicate finder found no copies of the same sound.".to_owned(),
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

/// A file's line under its sound: its name, why it's kept or how it's a copy and the folder it's in, its size, whether
/// it's hidden in the app, and a link that shows it in its folder.
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
    use mo_sounds::{SoundAnalysis, SoundMatch};

    use super::*;

    fn sound(id: i64, path: &str, seconds: f64) -> Sound {
        let mut s = Sound::new(id, path);
        s.analysis = Some(SoundAnalysis { seconds, sample_rate: 44_100, peak_db: -1.0, ..SoundAnalysis::default() });
        s
    }

    fn matched(a: i64, b: i64, kind: &str, apart: f64) -> SoundMatch {
        SoundMatch { a, b, kind: kind.to_owned(), score: 1.0, offset: 0.0, apart: Some(apart), typical: Some(0.2) }
    }

    /// Kicks: one your songs use, an exact copy of it, one you've hidden, and an MP3 close enough to be the same sound;
    /// and a snare with a trimmed copy, only very similar.
    fn library() -> (Vec<Sound>, Finds) {
        let sounds = vec![
            sound(1, r"D:\Samples\Kicks\Kick.wav", 1.0),
            sound(2, r"D:\Packs\Drums\Kick copy.wav", 1.0),
            sound(3, r"D:\Old\Kick.mp3", 1.02),
            sound(4, r"D:\Samples\Snares\Snare.wav", 2.0),
            sound(5, r"D:\Samples\Snares\Snare short.wav", 0.5),
        ];
        let mut hidden = sound(6, r"E:\Backup\Kick (2).wav", 1.0);
        hidden.hidden = true;
        let finds = Finds {
            matches: vec![matched(1, 3, "Converted", 0.5), matched(4, 5, "Trimmed", 0.2)],
            exact: vec![(1, "AB".to_owned()), (2, "AB".to_owned()), (6, "AB".to_owned())],
            sizes: HashMap::from([
                (1, 1_000_000),
                (2, 1_000_000),
                (3, 400_000),
                (4, 3_000_000),
                (5, 800_000),
                (6, 1_000_000),
            ]),
            hidden: vec![hidden],
        };
        (sounds, finds)
    }

    /// Your songs use the first kick.
    fn usage() -> HashMap<i64, Usage> {
        HashMap::from([(1, Usage { projects: 3, ..Usage::default() })])
    }

    #[test]
    fn copies_are_counted_with_the_hidden_ones_and_the_one_your_songs_use_kept() {
        let (sounds, finds) = library();
        let found = look(&sounds, Some(&usage()), &finds);
        // The kicks: the one your songs use is kept; the exact copies (one hidden) and the MP3 are the same sound and
        // count, 2.4 MB in all. The snare's trimmed copy is only very similar: it doesn't count.
        assert_eq!((found.sounds, found.same, found.similar, found.hidden), (2, 3, 1, 1));
        assert_eq!(found.bytes, 2_400_000);
        let kick = &found.listed[0];
        assert_eq!((kick.name.as_str(), kick.id), ("Kick", Some(1)));
        let files: Vec<(&str, bool, &str)> =
            kick.files.iter().map(|f| (f.path.as_str(), f.hidden, f.why.as_str())).collect();
        assert_eq!(
            files,
            [
                (r"D:\Samples\Kicks\Kick.wav", false, "Best to keep: 3 of your projects use this one"),
                (r"D:\Packs\Drums\Kick copy.wav", false, "Exact copy, renamed"),
                (r"E:\Backup\Kick (2).wav", true, "Exact copy, renamed"),
                (r"D:\Old\Kick.mp3", false, "Same sound, converted to MP3"),
            ]
        );
        assert_eq!((kick.same, kick.similar, kick.hidden, kick.bytes, kick.more), (3, 0, 1, 2_400_000, 0));
        let snare = &found.listed[1];
        assert_eq!((snare.name.as_str(), snare.same, snare.similar, snare.bytes), ("Snare", 0, 1, 0));

        // Before your songs' use is known, the best of the files is kept: the counts stay.
        let found = look(&sounds, None, &finds);
        let keep = &found.listed[0].files[0];
        assert_eq!(
            (keep.path.as_str(), keep.why.as_str()),
            (r"D:\Packs\Drums\Kick copy.wav", "Best to keep: full quality, not compressed")
        );
        assert_eq!((found.sounds, found.same, found.similar, found.hidden, found.bytes), (2, 3, 1, 1, 2_400_000));
        // With every file of a sound hidden, the kept one has no link to the Sounds page, which doesn't list it.
        let hidden = sounds.into_iter().map(|s| Sound { hidden: true, ..s }).chain(finds.hidden.clone()).collect();
        let found = look(&[], None, &Finds { hidden, ..finds });
        // Only the copies count as hidden, as they're what the counts break down: 3 kicks and the snare's.
        assert_eq!((found.listed[0].id, found.hidden, found.listed[0].hidden), (None, 4, 3));
    }

    #[test]
    fn the_line_says_how_many_sounds_have_copies_and_their_room() {
        let (sounds, finds) = library();
        let found = look(&sounds, None, &finds);
        let line = line(Some(&found), false).unwrap();
        assert_eq!(line.id, SOUND_COPIES);
        assert_eq!(line.why, "2 sounds have copies: 3 the same sound, 1 only very similar, 1 hidden");
        assert_eq!(line.size, "2.29 MB");
        // While the sounds are read, and before the first look has found anything, it says so.
        assert_eq!(super::line(None, true).map(|l| l.why), Some(LOOKING.to_owned()));
        // No line when none are found, or nothing is known and nothing is looking.
        assert_eq!(super::line(Some(&Found::default()), true), None);
        assert_eq!(super::line(None, false), None);
        let one = Found { sounds: 1, similar: 1, ..Found::default() };
        assert_eq!(super::line(Some(&one), false).unwrap().why, "1 sound has copies: 1 only very similar");
    }

    #[test]
    fn the_card_lists_the_sounds_whose_copies_take_most_with_their_files() {
        let (sounds, finds) = library();
        let found = look(&sounds, Some(&usage()), &finds);
        let card = card(Some(&found), false);
        assert_eq!(card.big, "2.29 MB");
        assert_eq!(card.big_note, "in 3 copies");
        assert_eq!(
            card.lead,
            "Sounds your sample folders hold more than once, even renamed, converted to another format or trimmed. 2 \
             sounds have copies: 3 the same sound, 1 only very similar, 1 hidden."
        );
        assert_eq!(card.note, NOTE);
        let rows: Vec<(&str, &str, &str, bool)> =
            card.parts.iter().map(|p| (p.name.as_str(), p.what.as_str(), p.size.as_str(), p.under)).collect();
        assert_eq!(
            rows[..3],
            [
                ("Kick", "3 copies: 3 the same sound, 1 hidden", "2.29 MB", false),
                (
                    "Kick.wav",
                    "Best to keep: 3 of your projects use this one · in D:\\\u{200B}Samples\\\u{200B}Kicks",
                    "977 KB",
                    true
                ),
                ("Kick copy.wav", "Exact copy, renamed · in D:\\\u{200B}Packs\\\u{200B}Drums", "977 KB", true),
            ]
        );
        // The kept one's link shows it on the Sounds page; each file's shows it in its folder; a hidden file says so.
        assert_eq!(card.parts[0].sound, Some(1));
        assert_eq!(card.parts[1].open, Some((Opens::InFolder, PathBuf::from(r"D:\Samples\Kicks\Kick.wav"))));
        assert_eq!((card.parts[3].name.as_str(), card.parts[3].status.as_str()), ("Kick (2).wav", HIDDEN));
        assert_eq!(card.parts[4].what, "Same sound, converted to MP3 · in D:\\\u{200B}Old");
        // The snare has nothing that counts: no size.
        assert_eq!(
            (card.parts[5].name.as_str(), card.parts[5].what.as_str(), card.parts[5].size.as_str()),
            ("Snare", "1 copy: 1 only very similar", "")
        );
        assert_eq!(card.parts.len(), 8);
        assert!(card.parts.iter().all(|p| p.tick.is_none()), "nothing goes from here");
        // While looking, before anything's found.
        let looking = super::card(None, true);
        assert_eq!((looking.big.as_str(), looking.lead.as_str()), ("Measuring…", LOOKING));
        assert!(looking.parts.is_empty() && looking.note.is_empty());
    }

    #[test]
    fn a_long_list_shows_the_biggest_and_adds_up_the_rest() {
        // 22 sounds with an exact copy each, the bigger the later; and the biggest has 6 more copies.
        let mut sounds = Vec::new();
        let mut finds = Finds::default();
        for n in 0..22i64 {
            for copy in 0..if n == 21 { 8 } else { 2 } {
                let id = n * 100 + copy;
                sounds.push(sound(id, &format!(r"D:\Samples\Sound {n} ({copy}).wav"), 1.0));
                finds.exact.push((id, format!("{n:04}")));
                finds.sizes.insert(id, (n + 1) * 1000);
            }
        }
        let found = look(&sounds, None, &finds);
        assert_eq!((found.sounds, found.same, found.listed.len()), (22, 21 + 7, LISTED));
        let biggest = &found.listed[0];
        assert_eq!((biggest.files.len(), biggest.more, biggest.bytes), (FILES, 3, 7 * 22_000));
        let card = card(Some(&found), false);
        // The biggest's sixth to eighth files add up in a line under it.
        assert_eq!((card.parts[FILES + 1].name.as_str(), card.parts[FILES + 1].under), ("And 3 more files", true));
        // The two smallest add up at the end: 1 KB and 2 KB of copies.
        let last = card.parts.last().unwrap();
        assert_eq!(
            (last.name.as_str(), last.size.as_str(), last.under),
            ("And 2 more sounds with copies", "2.93 KB", false)
        );
        assert_eq!(found.bytes, 7 * 22_000 + (1..=21).map(|n| n * 1000).sum::<u64>());
    }
}
