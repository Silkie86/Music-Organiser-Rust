//! The Space page's songs (phase 4, part 5, read-only): what compacting your songs could free, and the older saves
//! thinning could take out, across every song in the library. As the page's mock-up has them, two lines under Biggest
//! wins first: "Songs that can be compacted" (the same save twice, autosaves past the newest few, caches the DAW makes
//! again) and "Older saves from the middle of a sitting". Each song is measured by mo-space's `song::measure` (through
//! [`crate::song_room`], as the backup measures them), on the app's low-priority pool; this says every word of the two
//! lines and their cards, worked out once on the look's thread ([`Shown`]). Nothing is removed yet: sending what you
//! pick to the Recycle Bin, with Put back, comes next.
//!
//! The save a song opens always stays (see [`crate::song_room::Others::measure`]), and saves the library says have
//! gone, or that no reader knows, aren't measured.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chrono::{Datelike, NaiveDate};
use mo_formats::paths::file_name;
use mo_library::scan::is_project_file;
use mo_library::songs::group_songs;
use mo_library::{Library, LibraryError, Save, Song, SongChoices};
use mo_space::Stopped;
use mo_space::size::size_text;
use mo_space::song::{CompactItem, CompactKind, CompactSettings, SongSpaceReport};
use rayon::prelude::*;

use crate::drag_out::count_of;
use crate::song_room::{Others, song_saves};
use crate::space::{Card, Line, Opens, Part, and_list};

/// The Songs that can be compacted line's id.
pub const COMPACT: &str = "compact-songs";

/// The Older saves from the middle of a sitting line's id.
pub const THIN: &str = "thin-saves";

/// What the compact line says while the first look is on.
pub const LOOKING: &str = "Measuring your songs…";

/// What the page says under Biggest wins first when a look found nothing in any song.
pub const NONE_FOUND: &str = "Your songs have nothing to compact. ";

/// What the cards say comes next.
pub const NEXT: &str = "Nothing is removed yet. Sending what you pick to the Recycle Bin, with Put back, comes next.";

/// How many songs the compact card lists under each kind before it adds up the rest in one line.
pub const SONGS_LISTED: usize = 10;

/// How many songs the thinning card lists before it adds up the rest in one line.
pub const THIN_LISTED: usize = 30;

/// What the look reads from the library (read-only), in one go so the library is closed again before any song is
/// measured.
#[derive(Clone, Debug, Default)]
pub struct FromLibrary {
    pub saves: Vec<Save>,
    pub choices: SongChoices,
    /// The sample paths each save uses, by save id.
    pub samples: HashMap<i64, Vec<String>>,
    /// Your project folders.
    pub roots: Vec<String>,
    pub sample_folders: Vec<String>,
}

impl FromLibrary {
    /// Reads it all from `lib`.
    pub fn read(lib: &Library) -> Result<FromLibrary, LibraryError> {
        Ok(FromLibrary {
            saves: lib.saves()?,
            choices: lib.song_choices()?,
            samples: lib.sample_paths()?,
            roots: lib.roots()?,
            sample_folders: lib.list_setting("sample_folders")?,
        })
    }
}

/// A song with something to free: its name, the save it opens, how many of its saves were measured, and what
/// `measure` found (the save it opens left out).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SongRoom {
    pub name: String,
    pub opens: String,
    pub saves: usize,
    pub report: SongSpaceReport,
}

impl SongRoom {
    fn items(&self, kind: CompactKind) -> impl Iterator<Item = &CompactItem> {
        self.report.items.iter().filter(move |i| i.kind == kind)
    }
}

/// What a look found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The songs with something to free, in the library's order.
    pub songs: Vec<SongRoom>,
    /// The choices they were measured with.
    pub settings: CompactSettings,
}

impl Found {
    /// What Compact frees in all: identical saves, old autosaves and caches.
    pub fn compact_bytes(&self) -> u64 {
        self.songs.iter().fold(0u64, |sum, s| sum.saturating_add(s.report.freeable()))
    }

    /// What thinning older saves could free on top.
    pub fn thin_bytes(&self) -> u64 {
        self.songs.iter().fold(0u64, |sum, s| sum.saturating_add(s.report.thinnable()))
    }
}

/// Measures every song in the library: its saves still on disk, the DAW's autosaves of it and, when the song has a
/// folder to itself, everything in that folder. Spread over the pool it's called in (rule 21): call it on the app's
/// low-priority pool, never the window's thread.
pub fn look(from: &FromLibrary, settings: CompactSettings, stop: &AtomicBool) -> Result<Found, Stopped> {
    let songs = group_songs(&from.saves, &from.choices);
    let others = Others::new(&songs, &from.samples, from.sample_folders.clone());
    let measured: Vec<Option<SongRoom>> =
        songs.par_iter().map(|song| measure_song(song, from, &others, settings, stop)).collect::<Result<_, _>>()?;
    Ok(Found { songs: measured.into_iter().flatten().collect(), settings })
}

/// One song, or None when it has nothing to free (or no save of it is on disk).
fn measure_song(
    song: &Song,
    from: &FromLibrary,
    others: &Others,
    settings: CompactSettings,
    stop: &AtomicBool,
) -> Result<Option<SongRoom>, Stopped> {
    // A save that's gone can't be compacted, and one no reader knows isn't a project the app can vouch for.
    let present: Vec<&Save> = song.saves().filter(|v| !v.file_missing && is_project_file(&v.path)).collect();
    // The first is the save the song opens (saves come that way), or when that one isn't a project the app knows,
    // its highest-numbered save that is.
    let Some(opens) = present.first() else { return Ok(None) };
    let used: Vec<String> =
        song.saves().chain(&song.copies).flat_map(|v| from.samples.get(&v.id).into_iter().flatten().cloned()).collect();
    let saves = song_saves(present.iter().copied());
    let report = others.measure(&saves, Some(&opens.path), &from.roots, &used, settings, stop)?;
    Ok((!report.items.is_empty()).then(|| SongRoom {
        name: song.name.clone(),
        opens: opens.path.clone(),
        saves: present.len(),
        report,
    }))
}

/// "1 song", "1,200 songs".
fn songs_words(n: usize) -> String {
    count_of(n, "song", "songs")
}

/// "1 save", "46 saves".
fn saves_words(n: usize) -> String {
    count_of(n, "save", "saves")
}

/// What one kind of Compact holds across your songs: how many things, their room, and each song's share, biggest
/// first.
struct Kind<'a> {
    kind: CompactKind,
    count: usize,
    bytes: u64,
    songs: Vec<(&'a SongRoom, usize, u64)>,
}

/// The kinds Compact can free (identical saves, old autosaves, caches) biggest first, then audio no save seems to use,
/// which never goes unless you pick it; each only when some song has any.
fn kinds(found: &Found) -> Vec<Kind<'_>> {
    let of = |kind: CompactKind| {
        let mut songs: Vec<(&SongRoom, usize, u64)> = found
            .songs
            .iter()
            .filter_map(|s| {
                let (n, bytes) = s.items(kind).fold((0, 0u64), |(n, b), i| (n + 1, b.saturating_add(i.bytes)));
                (n > 0).then_some((s, n, bytes))
            })
            .collect();
        songs.sort_by_key(|(_, _, bytes)| std::cmp::Reverse(*bytes));
        let count = songs.iter().map(|(_, n, _)| n).sum();
        let bytes = songs.iter().fold(0u64, |sum, (_, _, b)| sum.saturating_add(*b));
        Kind { kind, count, bytes, songs }
    };
    let mut freeable: Vec<Kind<'_>> = [CompactKind::SameSave, CompactKind::Backup, CompactKind::Cache]
        .into_iter()
        .map(of)
        .filter(|k| k.count > 0)
        .collect();
    freeable.sort_by_key(|k| std::cmp::Reverse(k.bytes));
    freeable.extend(Some(of(CompactKind::UnusedAudio)).filter(|k| k.count > 0));
    freeable
}

/// "autosaves past the newest 5", as the cards say what of the autosaves goes.
fn past_newest(keep: usize) -> String {
    match keep {
        0 => "every autosave".to_owned(),
        1 => "autosaves past the newest one".to_owned(),
        n => format!("autosaves past the newest {n}"),
    }
}

/// A kind's name on the compact line: "identical saves".
fn kind_line_words(kind: CompactKind) -> &'static str {
    match kind {
        CompactKind::SameSave => "identical saves",
        CompactKind::Backup => "old autosaves",
        CompactKind::Cache => "caches the DAW rebuilds",
        CompactKind::UnusedAudio => "audio files no save seems to use",
        CompactKind::ThinnedSave => "saves from the middle of a sitting",
    }
}

/// The first letter in capitals.
fn capitalised(words: &str) -> String {
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The Songs that can be compacted line under Biggest wins first, with what Compact would free (what the wins are
/// sorted by). While the first look is on (`looking`, nothing found yet) it says so; none when a look found nothing
/// in any song, or none has finished and none is on. Audio no save seems to use frees nothing until you pick it, so
/// with only that the line shows no size.
fn compact_line(found: Option<&Found>, looking: bool) -> Option<(u64, Line)> {
    let line = |why: String, size: String| Line { id: COMPACT, title: "Songs that can be compacted", why, size };
    let Some(found) = found else {
        return looking.then(|| (0, line(LOOKING.to_owned(), "Measuring…".to_owned())));
    };
    let kinds = kinds(found);
    if kinds.is_empty() {
        return None;
    }
    // Named in the same order whatever their sizes, so the line reads the same from one look to the next.
    let freeable: Vec<&'static str> = [CompactKind::SameSave, CompactKind::Backup, CompactKind::Cache]
        .into_iter()
        .filter(|kind| kinds.iter().any(|k| k.kind == *kind))
        .map(kind_line_words)
        .collect();
    let (named, songs) = if freeable.is_empty() {
        (vec![kind_line_words(CompactKind::UnusedAudio)], kinds.iter().map(|k| k.songs.len()).sum())
    } else {
        (freeable, found.songs.iter().filter(|s| s.report.freeable() > 0).count())
    };
    let bytes = found.compact_bytes();
    let why = format!("{}, in {}", capitalised(&and_list(&named)), songs_words(songs));
    Some((bytes, line(why, if bytes == 0 { String::new() } else { size_text(bytes) })))
}

/// The Older saves from the middle of a sitting line, with what thinning would free; none when no song has any.
fn thin_line(found: Option<&Found>) -> Option<(u64, Line)> {
    let found = found?;
    let (saves, songs) = found.songs.iter().fold((0, 0), |(saves, songs), s| {
        let n = s.items(CompactKind::ThinnedSave).count();
        (saves + n, songs + usize::from(n > 0))
    });
    if saves == 0 {
        return None;
    }
    let bytes = found.thin_bytes();
    Some((
        bytes,
        Line {
            id: THIN,
            title: "Older saves from the middle of a sitting",
            why: format!("{} in {}; the last save of each sitting stays", saves_words(saves), songs_words(songs)),
            size: size_text(bytes),
        },
    ))
}

/// What the page says under Biggest wins first about songs: that a look found nothing to compact, or nothing.
fn none_note(found: Option<&Found>) -> &'static str {
    match found {
        Some(f) if f.songs.is_empty() => NONE_FOUND,
        _ => "",
    }
}

/// A kind's part on the compact card: "7 identical saves", and what they are.
fn kind_part(k: &Kind<'_>, keep: usize) -> Part {
    let (name, what) = match k.kind {
        CompactKind::SameSave => (
            count_of(k.count, "identical save", "identical saves"),
            "Byte for byte the same as a save that stays".to_owned(),
        ),
        CompactKind::Backup => (
            count_of(k.count, "old autosave", "old autosaves"),
            capitalised(&format!("{} of each song", past_newest(keep))),
        ),
        CompactKind::Cache => {
            ("Caches the DAW rebuilds".to_owned(), "Studio One makes them again when the song opens".to_owned())
        }
        CompactKind::UnusedAudio => (
            count_of(k.count, "audio file no save seems to use", "audio files no save seems to use"),
            "Not in the figure above: listen to each first, as none goes unless you pick it".to_owned(),
        ),
        // On a card of their own, not this one.
        CompactKind::ThinnedSave => (
            format!("{} from the middle of a sitting", saves_words(k.count)),
            "The last save of each sitting stays".to_owned(),
        ),
    };
    Part { name, what, size: size_text(k.bytes), ..Part::default() }
}

/// What a song holds of a kind, on its line under the kind: "3 saves", "12 autosaves", "Its Cache folder", or the
/// audio files by name.
fn song_what(song: &SongRoom, kind: CompactKind, n: usize) -> String {
    match kind {
        CompactKind::SameSave | CompactKind::ThinnedSave => saves_words(n),
        CompactKind::Backup => count_of(n, "autosave", "autosaves"),
        CompactKind::Cache => "Its Cache folder".to_owned(),
        CompactKind::UnusedAudio => {
            let names: Vec<&str> = song.items(kind).map(|i| file_name(&i.path)).take(2).collect();
            match n.saturating_sub(names.len()) {
                0 => and_list(&names),
                more => format!("{} and {more} more", names.join(", ")),
            }
        }
    }
}

/// A song's line, under its kind or on the thinning card: its name, what it holds and its size, with a link that shows
/// the save it opens in its folder.
fn song_part(song: &SongRoom, (name, what): (String, String), bytes: u64, under: bool) -> Part {
    Part {
        name,
        what,
        size: size_text(bytes),
        under,
        open: Some((Opens::InFolder, PathBuf::from(&song.opens))),
        ..Part::default()
    }
}

/// Up to `listed` songs' lines (biggest first, as `songs` comes, each worded by `words`), then one adding up the rest:
/// "And 40 more songs".
fn listed_parts<'a>(
    songs: &[(&'a SongRoom, usize, u64)],
    listed: usize,
    under: bool,
    words: impl Fn(&'a SongRoom, usize) -> (String, String),
) -> Vec<Part> {
    let mut parts: Vec<Part> =
        songs.iter().take(listed).map(|(s, n, bytes)| song_part(s, words(s, *n), *bytes, under)).collect();
    let rest = songs.get(listed..).unwrap_or_default();
    if !rest.is_empty() {
        parts.push(Part {
            name: format!("And {}", count_of(rest.len(), "more song", "more songs")),
            size: size_text(rest.iter().fold(0u64, |sum, (_, _, b)| sum.saturating_add(*b))),
            under,
            ..Part::default()
        });
    }
    parts
}

/// The Songs that can be compacted card: each kind with its room, biggest first (audio no save seems to use last, as
/// it isn't counted), and under each the songs it's in, biggest first.
fn compact_card(found: Option<&Found>) -> Card {
    let keep = found.map_or(CompactSettings::default().keep_backups, |f| f.settings.keep_backups);
    let kinds = found.map(kinds).unwrap_or_default();
    let bytes = found.map(Found::compact_bytes);
    let songs = found.map_or(0, |f| f.songs.iter().filter(|s| s.report.freeable() > 0).count());
    let mut parts = Vec::new();
    for k in &kinds {
        parts.push(kind_part(k, keep));
        parts.extend(listed_parts(&k.songs, SONGS_LISTED, true, |s, n| (s.name.clone(), song_what(s, k.kind, n))));
    }
    Card {
        kind: "COMPACT SONGS",
        title: "Songs that can be compacted",
        big: match bytes {
            None => "Measuring…".to_owned(),
            Some(0) => "Nothing to free".to_owned(),
            Some(b) => size_text(b),
        },
        big_note: if songs == 0 { String::new() } else { format!("in {}", songs_words(songs)) },
        lead: format!(
            "Nothing that holds music: the same save twice, {}, and caches your DAW makes again.",
            past_newest(keep)
        ),
        ticks: false,
        note: if parts.is_empty() {
            String::new()
        } else {
            format!("The save each song opens stays where it is, and so does every sample. {NEXT}")
        },
        parts,
        room: String::new(),
        warning: String::new(),
    }
}

/// "3 Oct", or "3 Oct 2025" in another year than `this_year`.
fn day_words(day: NaiveDate, this_year: i32) -> String {
    if day.year() == this_year { day.format("%-d %b").to_string() } else { day.format("%-d %b %Y").to_string() }
}

/// The days of a song's sittings that have saves thinning would take: "A sitting on 3 Oct", "Sittings on 3, 4 and 9
/// Oct", "Sittings on 14 days, 3 Oct to 9 Nov" ("2 to 28 Oct" in one month).
fn sittings_words(days: &BTreeSet<NaiveDate>, this_year: i32) -> String {
    let days: Vec<NaiveDate> = days.iter().copied().collect();
    match days.as_slice() {
        [] => String::new(),
        [one] => format!("A sitting on {}", day_words(*one, this_year)),
        [_, _] | [_, _, _] => {
            // Days in the same month say it once, after the last: "3, 4 and 9 Oct".
            let words: Vec<String> = days
                .iter()
                .enumerate()
                .map(|(i, d)| match days.get(i + 1) {
                    Some(next) if (next.year(), next.month()) == (d.year(), d.month()) => d.day().to_string(),
                    _ => day_words(*d, this_year),
                })
                .collect();
            format!("Sittings on {}", and_list(&words))
        }
        [first, .., last] => {
            let from = if (first.year(), first.month()) == (last.year(), last.month()) {
                first.day().to_string()
            } else {
                day_words(*first, this_year)
            };
            format!("Sittings on {} days, {from} to {}", days.len(), day_words(*last, this_year))
        }
    }
}

/// The Older saves from the middle of a sitting card: each song with saves thinning would take, biggest first, with
/// the days of those sittings and how many saves stay. `today` says which year goes without saying.
fn thin_card(found: Option<&Found>, today: NaiveDate) -> Card {
    let hours = found.map_or(CompactSettings::default().sitting_hours, |f| f.settings.sitting_hours);
    let mut songs: Vec<(&SongRoom, usize, u64)> = found
        .map(|f| {
            f.songs
                .iter()
                .filter_map(|s| {
                    let n = s.items(CompactKind::ThinnedSave).count();
                    (n > 0).then(|| (s, n, s.report.thinnable()))
                })
                .collect()
        })
        .unwrap_or_default();
    songs.sort_by_key(|(_, _, bytes)| std::cmp::Reverse(*bytes));
    let saves: usize = songs.iter().map(|(_, n, _)| n).sum();
    let parts = listed_parts(&songs, THIN_LISTED, false, |s, n| {
        let days: BTreeSet<NaiveDate> =
            s.items(CompactKind::ThinnedSave).filter_map(|i| i.modified).map(|m| m.date()).collect();
        let stay = count_of(s.saves.saturating_sub(n), "save stays", "saves stay");
        (format!("{}: {}", s.name, saves_words(n)), format!("{}; {stay}", sittings_words(&days, today.year())))
    });
    Card {
        kind: "THIN OLDER SAVES",
        title: "Older saves from the middle of a sitting",
        big: match found {
            None => "Measuring…".to_owned(),
            Some(_) if songs.is_empty() => "Nothing to free".to_owned(),
            Some(f) => size_text(f.thin_bytes()),
        },
        big_note: if songs.is_empty() {
            String::new()
        } else {
            format!("{} in {}", saves_words(saves), songs_words(songs.len()))
        },
        lead: format!(
            "Saves made less than {hours} hours apart while you worked. The last save of each sitting, the first save \
             and the save each song opens always stay. None goes unless you pick it."
        ),
        ticks: false,
        note: if parts.is_empty() { String::new() } else { NEXT.to_owned() },
        parts,
        room: String::new(),
        warning: String::new(),
    }
}

/// What the page shows of a look at your songs: its two lines under Biggest wins first, their cards and the note under
/// the wins. Worked out once, on the look's thread, as a library can have thousands of songs: the window's thread only
/// puts it on show (rule 12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub compact: Option<(u64, Line)>,
    pub thin: Option<(u64, Line)>,
    pub compact_card: Card,
    pub thin_card: Card,
    pub none_note: &'static str,
}

impl Shown {
    /// What `found` shows. `today` says which year goes without saying on the thinning card.
    pub fn of(found: &Found, today: NaiveDate) -> Shown {
        Shown {
            compact: compact_line(Some(found), false),
            thin: thin_line(Some(found)),
            compact_card: compact_card(Some(found)),
            thin_card: thin_card(Some(found), today),
            none_note: none_note(Some(found)),
        }
    }
}

/// The two lines under Biggest wins first, compact then thin: what the last look found (`shown`, which stays on show
/// while the next one looks), or while the first look is on (`looking`), that it's measuring.
pub fn lines(shown: Option<&Shown>, looking: bool) -> [Option<(u64, Line)>; 2] {
    match shown {
        Some(s) => [s.compact.clone(), s.thin.clone()],
        None => [compact_line(None, looking), None],
    }
}

/// The picked line's card ([`COMPACT`] or [`THIN`]): what the last look found, or that it's measuring.
pub fn card(shown: Option<&Shown>, picked: &str) -> Card {
    match (shown, picked == COMPACT) {
        (Some(s), true) => s.compact_card.clone(),
        (Some(s), false) => s.thin_card.clone(),
        (None, true) => compact_card(None),
        // With no song measured yet, there's no day to say.
        (None, false) => thin_card(None, NaiveDate::default()),
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDateTime;

    use super::*;

    fn at(day: u32, time: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(&format!("2026-10-{day:02} {time}"), "%Y-%m-%d %H:%M").unwrap_or_default()
    }

    fn item(kind: CompactKind, path: &str, bytes: u64, modified: Option<NaiveDateTime>) -> CompactItem {
        CompactItem { kind, path: path.to_owned(), bytes, modified, same_as: None }
    }

    fn song(name: &str, saves: usize, items: Vec<CompactItem>) -> SongRoom {
        SongRoom {
            name: name.to_owned(),
            opens: format!(r"D:\Projects\{name}\{name}_{saves}.bwproject"),
            saves,
            report: SongSpaceReport { items, ..SongSpaceReport::default() },
        }
    }

    const MB: u64 = 1024 * 1024;

    /// Night Drive has a copy, two old autosaves and an old bounce; Bounce has two saves thinning would take; Old Idea
    /// has only audio no save uses.
    fn sample() -> Found {
        let night = song(
            "Night Drive",
            5,
            vec![
                item(CompactKind::SameSave, r"D:\Projects\Night Drive\Night Drive_2.bwproject", 300 * MB, None),
                item(CompactKind::Backup, r"D:\Projects\Night Drive\auto-backups\a.bwproject", 40 * MB, None),
                item(CompactKind::Backup, r"D:\Projects\Night Drive\auto-backups\b.bwproject", 40 * MB, None),
                item(CompactKind::UnusedAudio, r"D:\Projects\Night Drive\Old bounce.wav", 900 * MB, None),
            ],
        );
        let bounce = song(
            "Bounce",
            6,
            vec![
                item(CompactKind::ThinnedSave, r"D:\Projects\Bounce\Bounce_2.bwproject", 20 * MB, Some(at(3, "10:10"))),
                item(CompactKind::ThinnedSave, r"D:\Projects\Bounce\Bounce_4.bwproject", 25 * MB, Some(at(9, "22:40"))),
            ],
        );
        let idea = song(
            "Old Idea",
            1,
            vec![
                item(CompactKind::UnusedAudio, r"D:\Projects\Old Idea\Take 1.wav", 10 * MB, None),
                item(CompactKind::UnusedAudio, r"D:\Projects\Old Idea\Take 2.wav", 10 * MB, None),
                item(CompactKind::UnusedAudio, r"D:\Projects\Old Idea\Take 3.wav", 10 * MB, None),
            ],
        );
        Found { songs: vec![night, bounce, idea], settings: CompactSettings::default() }
    }

    #[test]
    fn the_compact_line_frees_copies_autosaves_and_caches_and_says_while_the_first_look_is_on() {
        let found = sample();
        let (bytes, line) = compact_line(Some(&found), false).unwrap_or_else(|| panic!("no line"));
        // The old bounces aren't counted: they go only once you pick them.
        assert_eq!((bytes, line.id), (380 * MB, COMPACT));
        assert_eq!((line.why.as_str(), line.size.as_str()), ("Identical saves and old autosaves, in 1 song", "380 MB"));
        let (bytes, first) = compact_line(None, true).unwrap_or_else(|| panic!("no line"));
        assert_eq!((bytes, first.why.as_str(), first.size.as_str()), (0, LOOKING, "Measuring…"));
        // With only audio no save uses, it frees nothing until you pick some: no size.
        let audio = Found { songs: vec![sample().songs.remove(2)], ..Found::default() };
        let (bytes, line) = compact_line(Some(&audio), false).unwrap_or_else(|| panic!("no line"));
        assert_eq!(
            (bytes, line.why.as_str(), line.size.as_str()),
            (0, "Audio files no save seems to use, in 1 song", "")
        );
        // None found, or not looked yet: no line, and the note under the wins says why.
        assert_eq!(compact_line(Some(&Found::default()), false), None);
        assert_eq!(compact_line(None, false), None);
        assert_eq!(none_note(Some(&Found::default())), NONE_FOUND);
        assert_eq!(none_note(Some(&found)), "");
        assert_eq!(none_note(None), "");
    }

    #[test]
    fn the_thin_line_counts_the_saves_from_the_middle_of_a_sitting() {
        let (bytes, line) = thin_line(Some(&sample())).unwrap_or_else(|| panic!("no line"));
        assert_eq!((bytes, line.id, line.size.as_str()), (45 * MB, THIN, "45 MB"));
        assert_eq!(line.why, "2 saves in 1 song; the last save of each sitting stays");
        let none = Found { songs: vec![sample().songs.remove(0)], ..Found::default() };
        assert_eq!(thin_line(Some(&none)), None);
        assert_eq!(thin_line(None), None);
    }

    #[test]
    fn the_compact_card_lists_each_kind_biggest_first_with_its_songs_under_it() {
        let card = compact_card(Some(&sample()));
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("380 MB", "in 1 song"));
        assert_eq!(
            card.lead,
            "Nothing that holds music: the same save twice, autosaves past the newest 5, and caches your DAW makes again."
        );
        let rows: Vec<(&str, &str, &str, bool)> =
            card.parts.iter().map(|p| (p.name.as_str(), p.what.as_str(), p.size.as_str(), p.under)).collect();
        assert_eq!(
            rows,
            [
                ("1 identical save", "Byte for byte the same as a save that stays", "300 MB", false),
                ("Night Drive", "1 save", "300 MB", true),
                ("2 old autosaves", "Autosaves past the newest 5 of each song", "80 MB", false),
                ("Night Drive", "2 autosaves", "80 MB", true),
                (
                    "4 audio files no save seems to use",
                    "Not in the figure above: listen to each first, as none goes unless you pick it",
                    "930 MB",
                    false
                ),
                ("Night Drive", "Old bounce.wav", "900 MB", true),
                ("Old Idea", "Take 1.wav, Take 2.wav and 1 more", "30 MB", true),
            ]
        );
        // Each song's line shows the save it opens in its folder.
        assert_eq!(
            card.parts.get(1).and_then(|p| p.open.clone()),
            Some((Opens::InFolder, PathBuf::from(r"D:\Projects\Night Drive\Night Drive_5.bwproject")))
        );
        assert!(!card.ticks && card.note.ends_with(NEXT));
        let first = compact_card(None);
        assert_eq!((first.big.as_str(), first.big_note.as_str()), ("Measuring…", ""));
        assert!(first.parts.is_empty() && first.note.is_empty());
        let none = compact_card(Some(&Found::default()));
        assert_eq!(none.big, "Nothing to free");
        // Keeping no autosaves says so.
        let every = Found { settings: CompactSettings { keep_backups: 0, ..CompactSettings::default() }, ..sample() };
        assert!(compact_card(Some(&every)).parts.iter().any(|p| p.what == "Every autosave of each song"));
    }

    #[test]
    fn a_long_list_adds_up_the_songs_past_the_first_few() {
        let songs: Vec<SongRoom> = (0..SONGS_LISTED + 2)
            .map(|n| {
                let path = format!(r"D:\Projects\Song {n}\auto-backups\a.bwproject");
                song(&format!("Song {n}"), 3, vec![item(CompactKind::Backup, &path, (100 - n as u64) * MB, None)])
            })
            .collect();
        let card = compact_card(Some(&Found { songs, ..Found::default() }));
        assert_eq!(card.parts.len(), 1 + SONGS_LISTED + 1);
        let last = card.parts.last().cloned().unwrap_or_default();
        assert_eq!((last.name.as_str(), last.size.as_str(), last.under), ("And 2 more songs", "179 MB", true));
        assert_eq!(last.open, None);
    }

    #[test]
    fn the_thin_card_lists_each_song_with_its_sittings_and_what_stays() {
        let card = thin_card(Some(&sample()), NaiveDate::from_ymd_opt(2026, 11, 4).unwrap_or_default());
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("45 MB", "2 saves in 1 song"));
        assert!(card.lead.starts_with("Saves made less than 3 hours apart while you worked."));
        let rows: Vec<(&str, &str, &str, bool)> =
            card.parts.iter().map(|p| (p.name.as_str(), p.what.as_str(), p.size.as_str(), p.under)).collect();
        assert_eq!(rows, [("Bounce: 2 saves", "Sittings on 3 and 9 Oct; 4 saves stay", "45 MB", false)]);
        assert_eq!(card.note, NEXT);
        let none = thin_card(Some(&Found::default()), NaiveDate::default());
        assert!(none.parts.is_empty() && none.note.is_empty() && none.big == "Nothing to free");
    }

    #[test]
    fn what_shows_is_worked_out_once_and_says_it_is_measuring_until_a_look_finishes() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap_or_default();
        let found = sample();
        let shown = Shown::of(&found, today);
        // What the last look found stays on show while the next one looks.
        assert_eq!(lines(Some(&shown), true), [compact_line(Some(&found), false), thin_line(Some(&found))]);
        assert_eq!(card(Some(&shown), COMPACT), compact_card(Some(&found)));
        assert_eq!(card(Some(&shown), THIN), thin_card(Some(&found), today));
        assert_eq!(shown.none_note, "");
        // Before any look has finished, the compact line says one is measuring while it is, and the cards wait for it.
        let [compact, thin] = lines(None, true);
        assert_eq!((compact.map(|(_, l)| l.why), thin), (Some(LOOKING.to_owned()), None));
        assert_eq!(lines(None, false), [None, None]);
        assert_eq!((card(None, COMPACT).big.as_str(), card(None, THIN).big.as_str()), ("Measuring…", "Measuring…"));
        let none = Shown::of(&Found::default(), today);
        assert_eq!((none.compact, none.thin, none.none_note), (None, None, NONE_FOUND));
        // Saves from the middle of a sitting have their own words, should they ever be a kind on a card.
        let thinned = Kind { kind: CompactKind::ThinnedSave, count: 2, bytes: MB, songs: Vec::new() };
        assert_eq!(kind_part(&thinned, 5).name, "2 saves from the middle of a sitting");
    }

    #[test]
    fn sittings_say_their_days_shortly() {
        let day = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d).unwrap_or_default();
        let words = |days: &[NaiveDate]| sittings_words(&days.iter().copied().collect(), 2026);
        assert_eq!(words(&[day(2026, 10, 3)]), "A sitting on 3 Oct");
        assert_eq!(words(&[day(2026, 10, 3), day(2026, 10, 4), day(2026, 10, 9)]), "Sittings on 3, 4 and 9 Oct");
        assert_eq!(words(&[day(2026, 9, 28), day(2026, 10, 3), day(2026, 10, 9)]), "Sittings on 28 Sep, 3 and 9 Oct");
        assert_eq!(words(&[day(2025, 12, 30), day(2026, 1, 2)]), "Sittings on 30 Dec 2025 and 2 Jan");
        let many: Vec<NaiveDate> = (3..=9).map(|d| day(2026, 10, d)).chain([day(2026, 11, 9)]).collect();
        assert_eq!(words(&many), "Sittings on 8 days, 3 Oct to 9 Nov");
        assert_eq!(words(&many[..7]), "Sittings on 7 days, 3 to 9 Oct");
        assert_eq!(words(&[]), "");
    }
}
