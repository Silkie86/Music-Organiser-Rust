//! The Space page's songs (phase 4, part 5): what compacting your songs could free, and the older saves thinning could
//! take out, across every song in the library. As the page's mock-up has them, two lines under Biggest wins first:
//! "Songs that can be compacted" (the same save twice, autosaves past the newest few, caches the DAW makes again) and
//! "Older saves from the middle of a sitting". Each song is measured by mo-space's `song::measure` (through
//! [`crate::song_room`], as the backup measures them), on the app's low-priority pool; this says every word of the two
//! lines and their cards, with all that doesn't change as you tick worked out once, on the look's thread ([`Shown`]).
//!
//! Remove (part 5b) sends the old autosaves and caches you tick to the Recycle Bin, each checked again just before it
//! goes, and Put back returns them for as long as they're there (the page's question 5). The library lists neither,
//! so it isn't written to. Identical saves, older saves and audio no save seems to use stay read-only for now.
//!
//! The save a song opens always stays (see [`crate::song_room::Others::measure`]), and saves the library says have
//! gone, or that no reader knows, aren't measured.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chrono::{Datelike, NaiveDate};
use mo_formats::paths::file_name;
use mo_library::scan::{is_inside, is_project_file};
use mo_library::songs::group_songs;
use mo_library::{Library, LibraryError, Save, Song, SongChoices};
use mo_space::Stopped;
use mo_space::clean::{self, Bin, Done, Guard, Problem, Refused, Removed, What};
use mo_space::key;
use mo_space::prints::Prints;
use mo_space::size::size_text;
use mo_space::song::{CompactItem, CompactKind, CompactSettings, SongSpaceReport};
use rayon::prelude::*;

use crate::drag_out::count_of;
use crate::song_room::{Others, keep_prints, song_saves};
use crate::songs::thousands;
use crate::space::{Card, Line, Opens, Part, and_list};
use crate::space_bin::BIN;
use crate::space_removing::{
    self, ADMIN, Looked, Noun, Removable, back_words, clauses, freed_words, result_figures, sent_words, stayed,
};

/// The Songs that can be compacted line's id, and its clean-up's kind in what the app put in the Recycle Bin.
pub const COMPACT: &str = "compact-songs";

/// The Older saves from the middle of a sitting line's id.
pub const THIN: &str = "thin-saves";

/// What the compact line says while the first look is on.
pub const LOOKING: &str = "Measuring your songs…";

/// What the page says under Biggest wins first when a look found nothing in any song.
pub const NONE_FOUND: &str = "Your songs have nothing to compact. ";

/// What the older saves card says comes next.
pub const NEXT: &str = "Nothing is removed yet. Sending what you pick to the Recycle Bin, with Put back, comes next.";

/// How many songs the compact card lists under each kind before it adds up the rest in one line.
pub const SONGS_LISTED: usize = 10;

/// How many songs the thinning card lists before it adds up the rest in one line.
pub const THIN_LISTED: usize = 30;

/// The activity card's title while they're sent to the bin.
pub const REMOVING: &str = if cfg!(windows) {
    "Sending old autosaves and caches to the Recycle Bin"
} else {
    "Sending old autosaves and caches to the Trash"
};

/// Why every ticked one stays when Remove couldn't read your songs from the library, to measure them again.
pub const CANT_CHECK: &str =
    "Your songs couldn't be read from the library to check it again (the log says why), so it stays.";

/// What a kind's line says when the card can't send it to the bin yet: identical saves (the library lists them) and
/// audio no save seems to use (you listen to each first).
pub const NOT_YET: &str = if cfg!(windows) {
    "These can't go to the Recycle Bin from here yet."
} else {
    "These can't go to the Trash from here yet."
};

/// Why one picked stays when the card can't send its kind to the bin yet.
pub const NOT_YET_STAYS: &str = if cfg!(windows) {
    "This kind can't go to the Recycle Bin from here yet, so it stays."
} else {
    "This kind can't go to the Trash from here yet, so it stays."
};

/// What the compact card says always stays.
const STAYS: &str = "The save each song opens stays where it is, and so does every sample.";

/// The compact card's kind and title.
const KIND: &str = "COMPACT SONGS";
const TITLE: &str = "Songs that can be compacted";

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
/// folder to itself, everything in that folder. `prints` are the fingerprints of saves the last looks read (see
/// [`crate::song_room`]): a save unchanged since isn't read again. Spread over the pool it's called in (rule 21): call
/// it on the app's low-priority pool, never the window's thread.
pub fn look(
    from: &FromLibrary,
    settings: CompactSettings,
    prints: Option<&Prints>,
    stop: &AtomicBool,
) -> Result<Found, Stopped> {
    let songs = group_songs(&from.saves, &from.choices);
    let others = Others::new(&songs, &from.samples, from.sample_folders.clone());
    let measured: Result<Vec<Option<SongRoom>>, Stopped> =
        songs.par_iter().map(|song| measure_song(song, from, &others, settings, prints, stop)).collect();
    // One with a project folder away (a drive asleep or unplugged) isn't whole: its songs' prints are kept for when
    // it's back.
    keep_prints(prints, measured.is_ok() && from.roots.iter().all(|r| Path::new(r).is_dir()));
    Ok(Found { songs: measured?.into_iter().flatten().collect(), settings })
}

/// A song's saves still on disk that a reader knows: a save that's gone can't be compacted, and one no reader knows
/// isn't a project the app can vouch for. The first is the save the song opens (saves come that way), or when that
/// one isn't a project the app knows, its highest-numbered save that is.
fn present(song: &Song) -> Vec<&Save> {
    song.saves().filter(|v| !v.file_missing && is_project_file(&v.path)).collect()
}

/// One song, or None when it has nothing to free (or no save of it is on disk).
fn measure_song(
    song: &Song,
    from: &FromLibrary,
    others: &Others,
    settings: CompactSettings,
    prints: Option<&Prints>,
    stop: &AtomicBool,
) -> Result<Option<SongRoom>, Stopped> {
    let present = present(song);
    let Some(opens) = present.first() else { return Ok(None) };
    let used: Vec<String> =
        song.saves().chain(&song.copies).flat_map(|v| from.samples.get(&v.id).into_iter().flatten().cloned()).collect();
    let saves = song_saves(present.iter().copied());
    let report = others.measure(&saves, Some(&opens.path), &from.roots, &used, settings, prints, stop)?;
    Ok((!report.items.is_empty()).then(|| SongRoom {
        name: song.name.clone(),
        opens: opens.path.clone(),
        saves: present.len(),
        report,
    }))
}

// ---- What Remove takes ------------------------------------------------------------------------------------------

/// One thing of a song's the compact card lists, and the song it's of: its name, and the save it opens, which its
/// line shows in its folder. It carries both, so how Remove went still shows once the next look has found something
/// else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SongItem {
    pub song: Arc<str>,
    pub opens: Arc<str>,
    pub item: CompactItem,
}

/// Old autosaves and caches can go, each ticked out of the box (as in the C# app).
impl Removable for SongItem {
    fn path(&self) -> &str {
        &self.item.path
    }
    fn bytes(&self) -> u64 {
        self.item.bytes
    }
    fn can_go(&self) -> bool {
        can_go(self.item.kind)
    }
}

/// Whether Remove takes this kind yet: old autosaves and caches, which the library doesn't list (it leaves out the
/// DAWs' autosave folders, as the C# app's scan did). Identical saves wait, as the library lists them, and so does
/// audio no save seems to use, which you listen to first.
fn can_go(kind: CompactKind) -> bool {
    matches!(kind, CompactKind::Backup | CompactKind::Cache)
}

/// How removing your songs' old autosaves and caches goes while the app runs: the ones you unticked, and where Remove
/// has got to.
pub type Removing = space_removing::Removing<SongItem>;

/// Where removing them has got to.
pub type Stage = space_removing::Stage<SongItem>;

/// A removal of them that ran.
pub type Sent = space_removing::Sent<SongItem>;

/// Sends the old autosaves and caches you picked to the Recycle Bin, each checked again first: the songs they're of
/// are measured again as they are now (`from`, read from the library again; `settings` as the look had them), and only
/// what that offers again goes on ([`still_offered`]); then the clean-up's guard checks each one again just before it
/// goes (it's in a project folder and not a sample folder, and it's what it was: an autosave the DAW made, or a Studio
/// One Cache folder; one that changed since stays). Saves always stay: each song's newest and the save it opens are
/// guarded too. What doesn't go says why. `stop` (Stop) leaves what hasn't gone yet where it is. Measures and changes
/// the disk, spread over the pool it's called in: call it on the app's low-priority pool, never the window's thread.
pub fn remove(
    picked: &[SongItem],
    from: &FromLibrary,
    settings: CompactSettings,
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    let songs = group_songs(&from.saves, &from.choices);
    let others = Others::new(&songs, &from.samples, from.sample_folders.clone());
    let Ok((going, problems)) = still_offered(picked, &songs, from, &others, settings, stop) else {
        return stayed(picked, Refused::Stopped.text());
    };
    let keep = kept_saves(&songs);
    let in_use = |name: &str| others.uses(name);
    let guard =
        Guard { project_roots: &from.roots, sample_folders: &from.sample_folders, keep: &keep, sample_in_use: &in_use };
    let plan = clean::preview(&going, &guard);
    let mut done = clean::apply(&plan, &guard, bin, stop);
    let refused = plan.refused.into_iter().map(|(item, why)| Problem { path: item.path, why: why.text().to_owned() });
    done.problems.splice(0..0, problems.into_iter().chain(refused));
    went_inside(&going, &mut done);
    done
}

/// What the clean-up passed over as inside a folder that went too (a line inside a cache folder that went, say) went
/// with that folder: `done` says so, so its line says it went (its room is in the folder's) and it comes back with it.
/// Usually there's none, so only what `done` says nothing of is looked for in what went.
fn went_inside(going: &[CompactItem], done: &mut Done) {
    let said: HashSet<&str> =
        done.removed.iter().map(|r| r.path.as_str()).chain(done.problems.iter().map(|p| p.path.as_str())).collect();
    let with: Vec<(String, String)> = going
        .iter()
        .filter(|i| !said.contains(i.path.as_str()))
        .filter_map(|i| {
            done.removed.iter().find(|r| is_inside(&i.path, &r.path)).map(|r| (i.path.clone(), r.path.clone()))
        })
        .collect();
    done.went_with.extend(with);
}

/// Of `picked`, what goes on to be checked and sent: what can go from here that measuring its song again (as `from`
/// has the songs now) still offers, as the same kind. A song is measured again when it has the name of a song picked
/// from, or any save of it is the save one opened (so a song saved again since is found). The rest stay, each with
/// why. Stopped, it says so.
fn still_offered(
    picked: &[SongItem],
    songs: &[Song],
    from: &FromLibrary,
    others: &Others,
    settings: CompactSettings,
    stop: &AtomicBool,
) -> Result<(Vec<CompactItem>, Vec<Problem>), Stopped> {
    let mut problems = Vec::new();
    let mut can: Vec<&SongItem> = Vec::new();
    for p in picked {
        if p.can_go() {
            can.push(p);
        } else {
            problems.push(Problem { path: p.item.path.clone(), why: NOT_YET_STAYS.to_owned() });
        }
    }
    let names: HashSet<&str> = can.iter().map(|p| &*p.song).collect();
    let opened: HashSet<String> = can.iter().map(|p| key(&p.opens)).collect();
    let again: Vec<&Song> = songs
        .iter()
        .filter(|s| names.contains(s.name.as_str()) || s.saves().any(|v| opened.contains(&key(&v.path))))
        .collect();
    // Every save read afresh, without the look's fingerprints: this is the last check before anything goes.
    let measured: Vec<Option<SongRoom>> = again
        .par_iter()
        .map(|song| measure_song(song, from, others, settings, None, stop))
        .collect::<Result<_, _>>()?;
    let offered: HashMap<String, &CompactItem> =
        measured.iter().flatten().flat_map(|s| s.report.items.iter().map(|i| (key(&i.path), i))).collect();
    let mut going = Vec::new();
    for p in can {
        match offered.get(&key(&p.item.path)) {
            Some(now) if now.kind == p.item.kind => going.push(as_measured_now(&p.item, now)),
            _ => problems.push(Problem { path: p.item.path.clone(), why: not_offered(&p.item) }),
        }
    }
    Ok((going, problems))
}

/// What goes on to the guard for one picked that measuring again offers again (`now`): an autosave as you saw it, so
/// the guard keeps one written again since (it compares a file's size and time); a cache folder at its size now, as
/// the guard doesn't compare a folder's size, and what it frees is what it holds now.
fn as_measured_now(picked: &CompactItem, now: &CompactItem) -> CompactItem {
    match picked.kind {
        CompactKind::Cache => CompactItem { bytes: now.bytes, ..picked.clone() },
        _ => picked.clone(),
    }
}

/// Why one picked stays when measuring its song again doesn't offer it now: it's gone; or it's one of its song's
/// newest few autosaves now, or its folder isn't the song's alone any more (another song saved into it, say).
fn not_offered(item: &CompactItem) -> String {
    if std::fs::symlink_metadata(&item.path).is_err() {
        return Refused::Gone.text().to_owned();
    }
    match item.kind {
        CompactKind::Cache => "Measured again just now, it isn't a cache of its song's any more, so it stays.",
        _ => "Measured again just now, it isn't one of its song's old autosaves any more, so it stays.",
    }
    .to_owned()
}

/// The saves that always stay, as [`key`]s: each song's newest (by time, as mo-space measures it: every one saved at
/// that moment) and the save it opens.
fn kept_saves(songs: &[Song]) -> HashSet<String> {
    let mut keep = HashSet::new();
    for song in songs {
        let present = present(song);
        keep.extend(present.first().map(|v| key(&v.path)));
        if let Some(newest) = present.iter().map(|v| v.modified).max() {
            keep.extend(present.iter().filter(|v| v.modified == newest).map(|v| key(&v.path)));
        }
    }
    keep
}

// ---- Words ------------------------------------------------------------------------------------------------------

/// "1 song", "1,200 songs".
fn songs_words(n: usize) -> String {
    count_of(n, "song", "songs")
}

/// "1 save", "46 saves".
fn saves_words(n: usize) -> String {
    count_of(n, "save", "saves")
}

/// How many things of a kind, as the Recycle Bin card and Remove's lines count them: "12 old autosaves".
fn kind_count(kind: CompactKind, n: usize) -> String {
    match kind {
        CompactKind::SameSave => count_of(n, "identical save", "identical saves"),
        CompactKind::ThinnedSave => count_of(n, "older save", "older saves"),
        CompactKind::Backup => count_of(n, "old autosave", "old autosaves"),
        CompactKind::Cache => count_of(n, "cache", "caches"),
        CompactKind::UnusedAudio => count_of(n, "audio file", "audio files"),
    }
}

/// How a removal's lines count what it sent: by its kind, when all of it is `one` kind ([`one_kind`]: "12 old
/// autosaves"); otherwise "13 autosaves and caches".
fn noun_of(one: Option<CompactKind>) -> impl Fn(usize) -> String {
    move |n| match one {
        Some(kind) => kind_count(kind, n),
        None => count_of(n, "autosave or cache", "autosaves and caches"),
    }
}

/// The kind all of `kinds` are, when they're all one.
fn one_kind(mut kinds: impl Iterator<Item = CompactKind>) -> Option<CompactKind> {
    let first = kinds.next();
    first.filter(|first| kinds.all(|k| k == *first))
}

/// What a removal of your songs' things sent, by kind, as the Recycle Bin card says it: "12 old autosaves and 1
/// cache".
pub fn sent_things(removed: &[Removed]) -> String {
    things_words(removed)
}

/// Things that went, counted by kind: "12 old autosaves and 1 cache".
fn things_words<'a>(removed: impl IntoIterator<Item = &'a Removed>) -> String {
    let mut counts: Vec<(CompactKind, usize)> = [
        CompactKind::SameSave,
        CompactKind::ThinnedSave,
        CompactKind::Backup,
        CompactKind::Cache,
        CompactKind::UnusedAudio,
    ]
    .map(|kind| (kind, 0))
    .to_vec();
    let mut other = 0;
    for r in removed {
        match counts.iter_mut().find(|(kind, _)| r.what == What::Song(*kind)) {
            Some((_, n)) => *n += 1,
            None => other += 1,
        }
    }
    let mut said: Vec<String> =
        counts.into_iter().filter(|(_, n)| *n > 0).map(|(kind, n)| kind_count(kind, n)).collect();
    if other > 0 {
        said.push(count_of(other, "file or folder", "files and folders"));
    }
    and_list(&said)
}

/// How a removal's lines count what it sent: those in the bin by kind (`in_bin`: "61 old autosaves and 1 cache"), the
/// rest by the kind of what went when it's all one ([`noun_of`]).
struct Counted {
    in_bin: String,
    went: Option<CompactKind>,
}

impl Counted {
    /// For what `done` says went, of which `in_bin` are in the bin.
    fn of<'a>(done: &Done, in_bin: impl IntoIterator<Item = &'a Removed>) -> Counted {
        let went = done.removed.iter().filter_map(|r| match r.what {
            What::Song(kind) => Some(kind),
            _ => None,
        });
        Counted { in_bin: things_words(in_bin), went: one_kind(went) }
    }
}

impl Noun for Counted {
    fn count(&self, n: usize) -> String {
        noun_of(self.went)(n)
    }
    /// Those in the bin, as counted when this was made: all of them.
    fn in_bin(&self, _n: usize) -> String {
        self.in_bin.clone()
    }
}

/// The activity card's line while Remove is on.
pub fn removing_words() -> String {
    format!("Each one is checked again just before it goes to the {BIN}.")
}

/// The activity card's line once Remove is done: how much was sent to the bin, what's in it, what went but wasn't found
/// there, and what stayed. Err when nothing went (the card then fails).
pub fn removed_words(done: &Done) -> Result<String, String> {
    let said: HashSet<&str> = done.problems.iter().map(|p| p.path.as_str()).collect();
    let in_bin = done.removed.iter().filter(|r| !said.contains(r.path.as_str()));
    space_removing::removed_words(done, &Counted::of(done, in_bin))
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

// ---- The compact line and card ----------------------------------------------------------------------------------

/// What one kind of Compact holds across your songs: how many things, their room, and each song's share, biggest
/// first, by where the song is in [`Found::songs`].
struct Kind {
    kind: CompactKind,
    count: usize,
    bytes: u64,
    songs: Vec<(usize, usize, u64)>,
}

/// The kinds Compact can free (identical saves, old autosaves, caches) biggest first, then audio no save seems to use,
/// which never goes unless you pick it; each only when some song has any.
fn kinds(found: &Found) -> Vec<Kind> {
    let of = |kind: CompactKind| {
        let mut songs: Vec<(usize, usize, u64)> = found
            .songs
            .iter()
            .enumerate()
            .filter_map(|(at, s)| {
                let (n, bytes) = s.items(kind).fold((0, 0u64), |(n, b), i| (n + 1, b.saturating_add(i.bytes)));
                (n > 0).then_some((at, n, bytes))
            })
            .collect();
        songs.sort_by_key(|(_, _, bytes)| std::cmp::Reverse(*bytes));
        let count = songs.iter().map(|(_, n, _)| n).sum();
        let bytes = songs.iter().fold(0u64, |sum, (_, _, b)| sum.saturating_add(*b));
        Kind { kind, count, bytes, songs }
    };
    let mut freeable: Vec<Kind> = [CompactKind::SameSave, CompactKind::Backup, CompactKind::Cache]
        .into_iter()
        .map(of)
        .filter(|k| k.count > 0)
        .collect();
    freeable.sort_by_key(|k| std::cmp::Reverse(k.bytes));
    freeable.extend(Some(of(CompactKind::UnusedAudio)).filter(|k| k.count > 0));
    freeable
}

/// The compact line, saying `why`, with `size`.
fn line(why: String, size: String) -> Line {
    Line { id: COMPACT, title: TITLE, why, size }
}

/// The Songs that can be compacted line under Biggest wins first, with what Compact would free (what the wins are
/// sorted by). While the first look is on (`looking`, nothing found yet) it says so; none when a look found nothing
/// in any song, or none has finished and none is on. Audio no save seems to use frees nothing until you pick it, so
/// with only that the line shows no size.
fn compact_line(found: Option<&Found>, looking: bool) -> Option<(u64, Line)> {
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

/// A kind's line on the compact card: "7 identical saves", what they are, and their room `bytes` (none when it's 0).
/// A kind Remove doesn't take yet says so.
fn kind_part(kind: CompactKind, count: usize, bytes: u64, keep: usize) -> Part {
    let (name, what) = match kind {
        CompactKind::SameSave => (
            count_of(count, "identical save", "identical saves"),
            "Byte for byte the same as a save that stays".to_owned(),
        ),
        CompactKind::Backup => (
            count_of(count, "old autosave", "old autosaves"),
            capitalised(&format!("{} of each song", past_newest(keep))),
        ),
        CompactKind::Cache => {
            ("Caches the DAW rebuilds".to_owned(), "Studio One makes them again when the song opens".to_owned())
        }
        CompactKind::UnusedAudio => (
            count_of(count, "audio file no save seems to use", "audio files no save seems to use"),
            "Not in the figure above: listen to each first, as none goes unless you pick it".to_owned(),
        ),
        // On a card of their own, not this one.
        CompactKind::ThinnedSave => (
            format!("{} from the middle of a sitting", saves_words(count)),
            "The last save of each sitting stays".to_owned(),
        ),
    };
    let status = if can_go(kind) { String::new() } else { NOT_YET.to_owned() };
    Part { name, what, size: if bytes == 0 { String::new() } else { size_text(bytes) }, status, ..Part::default() }
}

/// What a song holds of a kind, on its line under the kind: "3 saves", "12 autosaves", "Its Cache folder", or the
/// audio files by name (`names`, of `n`).
fn song_what<'a>(kind: CompactKind, n: usize, names: impl Iterator<Item = &'a str>) -> String {
    match kind {
        CompactKind::SameSave | CompactKind::ThinnedSave => saves_words(n),
        CompactKind::Backup => count_of(n, "autosave", "autosaves"),
        CompactKind::Cache => "Its Cache folder".to_owned(),
        CompactKind::UnusedAudio => {
            let names: Vec<&str> = names.take(2).collect();
            match n.saturating_sub(names.len()) {
                0 => and_list(&names),
                more => format!("{} and {more} more", names.join(", ")),
            }
        }
    }
}

/// A song's line, under its kind or on the thinning card: its name, what it holds and its size (none when `bytes` is
/// 0), with a link that shows the save it opens (`opens`, which always stays) in its folder.
fn song_part(opens: &str, (name, what): (String, String), bytes: u64, under: bool) -> Part {
    Part {
        name,
        what,
        size: if bytes == 0 { String::new() } else { size_text(bytes) },
        under,
        open: Some((Opens::InFolder, PathBuf::from(opens))),
        ..Part::default()
    }
}

/// The line adding up the songs past the first few: "And 40 more songs", with their room.
fn rest_part(songs: usize, bytes: u64, under: bool) -> Part {
    Part {
        name: format!("And {}", count_of(songs, "more song", "more songs")),
        size: if bytes == 0 { String::new() } else { size_text(bytes) },
        under,
        ..Part::default()
    }
}

/// A line of the compact card while you pick: its words, and which of [`Shown`]'s things its tick is for (all of a
/// kind's, one song's of a kind, or those of the songs past the first few); none for a kind Remove doesn't take yet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    part: Part,
    ticks: Option<Range<usize>>,
}

/// What the compact card lists, and its lines while you pick: each kind with its room (the one that frees most first,
/// audio no save seems to use last), and under it the songs it's in, biggest first, up to [`SONGS_LISTED`], then one
/// line adding up the rest. Each kind's things are listed song by song, so every line's tick is for a run of them.
fn compact_rows(found: &Found) -> (Vec<SongItem>, Vec<Row>) {
    let keep = found.settings.keep_backups;
    // Each song's name and the save it opens, shared by all its things.
    let heads: Vec<(Arc<str>, Arc<str>)> =
        found.songs.iter().map(|s| (Arc::from(s.name.as_str()), Arc::from(s.opens.as_str()))).collect();
    let (mut items, mut rows) = (Vec::new(), Vec::new());
    for k in kinds(found) {
        let ticks = can_go(k.kind);
        let start = items.len();
        let mut under = Vec::new();
        let mut rest: Option<(usize, usize, u64)> = None;
        let songs = k.songs.iter().filter_map(|&(at, n, bytes)| Some((found.songs.get(at)?, heads.get(at)?, n, bytes)));
        for (listed, (song, (name, opens), n, bytes)) in songs.enumerate() {
            let from = items.len();
            let of_song =
                song.items(k.kind).map(|i| SongItem { song: name.clone(), opens: opens.clone(), item: i.clone() });
            items.extend(of_song);
            if listed < SONGS_LISTED {
                let names = song.items(k.kind).map(|i| file_name(&i.path));
                let words = (song.name.clone(), song_what(k.kind, n, names));
                let part = song_part(&song.opens, words, bytes, true);
                under.push(Row { part, ticks: ticks.then_some(from..items.len()) });
            } else {
                let (_, songs, sum) = rest.get_or_insert((from, 0, 0));
                *songs += 1;
                *sum = sum.saturating_add(bytes);
            }
        }
        rows.push(Row { part: kind_part(k.kind, k.count, k.bytes, keep), ticks: ticks.then_some(start..items.len()) });
        rows.extend(under);
        if let Some((from, songs, bytes)) = rest {
            rows.push(Row { part: rest_part(songs, bytes, true), ticks: ticks.then_some(from..items.len()) });
        }
    }
    (items, rows)
}

/// What a tick on the compact card is for, as its part names it: not every file under it (a kind can have
/// thousands), but where they are in what [`Shown`] lists, after a mark no file's path can have. [`tick`] ticks each
/// of them.
const TICK: &str = "\u{0}songs:";

fn tick_key(range: &Range<usize>) -> PathBuf {
    PathBuf::from(format!("{TICK}{}-{}", range.start, range.end))
}

fn tick_range(path: &Path) -> Option<Range<usize>> {
    let (start, end) = path.to_str()?.strip_prefix(TICK)?.split_once('-')?;
    Some(start.parse().ok()?..end.parse().ok()?)
}

/// Ticks or unticks what the compact card's ticks at `paths` are for, while picking ([`Removing::tick`]): every one of
/// a kind, of a song or of the songs past the first few, as `shown` lists them.
pub fn tick(removing: &mut Removing, shown: Option<&Shown>, paths: &[PathBuf], ticked: bool) {
    let Some(shown) = shown else { return };
    let each: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| shown.items.get(tick_range(p)?))
        .flatten()
        .map(|i| PathBuf::from(&i.item.path))
        .collect();
    removing.tick(&each, ticked);
}

/// What the page shows of a look at your songs: what the compact card lists and its lines while you pick, its big
/// figures, the two lines under Biggest wins first, the older saves card and the note under the wins. Worked out once,
/// on the look's thread, as a library can have thousands of songs: the window's thread only adds the ticks (rule 12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// Each thing the compact card lists, in its order: kind by kind, and in each, song by song (biggest first).
    items: Vec<SongItem>,
    rows: Vec<Row>,
    /// Where those in Program Files are in `items` (Windows guards them): Remove sends them together at the end, so
    /// Windows asks once.
    guarded: Vec<usize>,
    /// What Compact frees, in how many songs, and the choices they were measured with (how many of each song's newest
    /// autosaves stay): Remove measures them again with the same.
    bytes: u64,
    songs: usize,
    settings: CompactSettings,
    compact: Option<(u64, Line)>,
    thin: Option<(u64, Line)>,
    thin_card: Card,
    pub none_note: &'static str,
}

impl Shown {
    /// What `found` shows. `today` says which year goes without saying on the thinning card.
    pub fn of(found: &Found, today: NaiveDate) -> Shown {
        Shown::with(found, today, &mo_space::plugin_space::is_install_folder)
    }

    /// The same, with `guarded` saying which places Windows guards.
    fn with(found: &Found, today: NaiveDate, guarded: &dyn Fn(&str) -> bool) -> Shown {
        let (items, rows) = compact_rows(found);
        let guarded = items.iter().enumerate().filter(|(_, i)| i.can_go() && guarded(&i.item.path)).map(|(n, _)| n);
        Shown {
            guarded: guarded.collect(),
            items,
            rows,
            bytes: found.compact_bytes(),
            songs: found.songs.iter().filter(|s| s.report.freeable() > 0).count(),
            settings: found.settings,
            compact: compact_line(Some(found), false),
            thin: thin_line(Some(found)),
            thin_card: thin_card_of(Some(found), today),
            none_note: none_note(Some(found)),
        }
    }
}

impl Shown {
    /// The choices the songs were measured with, for Remove to measure them again with ([`remove`]).
    pub fn settings(&self) -> CompactSettings {
        self.settings
    }
}

impl Looked for Shown {
    type Item = SongItem;
    fn items(&self) -> &[SongItem] {
        &self.items
    }
}

/// The two lines under Biggest wins first, compact then thin: what the last look found (`shown`, which stays on show
/// while the next one looks), or while the first look is on (`looking`), that it's measuring. Once a removal's result
/// is on show (`removing`), the compact line says how that went instead, freeing nothing more, until Measure again.
pub fn lines(shown: Option<&Shown>, looking: bool, removing: &Removing) -> [Option<(u64, Line)>; 2] {
    let compact = match removing.stage() {
        Stage::Sent(sent) => {
            Some((0, line(sent_words(sent, &Counted::of(sent.done(), sent.binned())), freed_words(sent))))
        }
        Stage::PuttingBack(sent) => Some((0, line("Putting them back…".to_owned(), freed_words(sent)))),
        Stage::PutBack(sent, problems) => Some((0, line(back_words(sent, problems), String::new()))),
        Stage::Picking | Stage::Sending(_) => match shown {
            Some(s) => s.compact.clone(),
            None => compact_line(None, looking),
        },
    };
    [compact, shown.and_then(|s| s.thin.clone())]
}

/// The Songs that can be compacted card. While picking (and while Remove is on): what Compact frees, then each kind
/// with its room, biggest first (audio no save seems to use last, as it isn't counted), and under each the songs it's
/// in, biggest first, each with a link that shows the save it opens in its folder. Old autosaves and caches have
/// ticks, all ticked out of the box: a kind's tick is for all of its, a song's for its own, and the line adding up the
/// songs past the first few for theirs. Once a removal has run, the card shows how it went instead ([`Removing`]),
/// until Measure again. `can_put_back` says whether the app can put things back here (a Mac's Trash doesn't let it).
pub fn compact_card(shown: Option<&Shown>, removing: &Removing, can_put_back: bool) -> Card {
    let keep = shown.map_or(CompactSettings::default(), |s| s.settings).keep_backups;
    match removing.stage() {
        Stage::Sent(sent) | Stage::PuttingBack(sent) | Stage::PutBack(sent, _) => {
            result_card(sent, removing.stage(), can_put_back, keep)
        }
        Stage::Picking | Stage::Sending(_) => picking_card(shown, removing, can_put_back, keep),
    }
}

/// The compact card's lead.
fn lead(keep: usize) -> String {
    format!("Nothing that holds music: the same save twice, {}, and caches your DAW makes again.", past_newest(keep))
}

fn picking_card(shown: Option<&Shown>, removing: &Removing, can_put_back: bool, keep: usize) -> Card {
    let picking = *removing.stage() == Stage::Picking;
    let items = shown.map(|s| s.items.as_slice()).unwrap_or_default();
    let rows = shown.map(|s| s.rows.as_slice()).unwrap_or_default();
    let parts: Vec<Part> = rows
        .iter()
        .map(|row| {
            let mut part = row.part.clone();
            if let Some(range) = row.ticks.as_ref().filter(|_| picking) {
                let under = items.get(range.clone()).unwrap_or_default();
                let ticked = under.iter().filter(|i| removing.ticked(i)).count();
                part.tick = Some((vec![tick_key(range)], ticked == under.len()));
                part.partly = ticked > 0 && ticked < under.len();
            }
            part
        })
        .collect();
    let can_go = items.iter().any(Removable::can_go);
    let guarded = shown.is_some_and(|s| s.guarded.iter().any(|&n| s.items.get(n).is_some_and(|i| removing.ticked(i))));
    let note = match removing.stage() {
        Stage::Sending(_) => format!("Checking each ticked one again, then sending it to the {BIN}…"),
        _ if can_go => remove_note(keep, can_put_back, guarded),
        _ if !parts.is_empty() => STAYS.to_owned(),
        _ => String::new(),
    };
    Card {
        kind: KIND,
        title: TITLE,
        big: match shown {
            None => "Measuring…".to_owned(),
            Some(s) if s.bytes == 0 => "Nothing to free".to_owned(),
            Some(s) => size_text(s.bytes),
        },
        big_note: match shown {
            Some(s) if s.songs > 0 => format!("in {}", songs_words(s.songs)),
            _ => String::new(),
        },
        lead: lead(keep),
        ticks: can_go,
        parts,
        note,
        room: String::new(),
        warning: String::new(),
    }
}

/// What the card says about Remove, above it: what it does, the checks, what always stays, and where they go
/// (`can_put_back`: the app can put them back here, which a Mac's Trash doesn't let it). Those in Program Files
/// (`guarded`: Windows guards them) go together, Windows asking once, and come back together the same way.
fn remove_note(keep: usize, can_put_back: bool, guarded: bool) -> String {
    let newest = match keep {
        0 => String::new(),
        1 => ", and so does the newest autosave of each song".to_owned(),
        n => format!(", and so do the newest {n} autosaves of each song"),
    };
    let checks = format!(
        "The ticked autosaves and caches go to the {BIN}, each checked again just before it goes: one that changed \
         since stays{newest}."
    );
    let back = match (can_put_back, guarded) {
        (true, false) => format!("Put back returns them until you empty the {BIN}."),
        (true, true) => {
            format!("{ADMIN} Put back returns them until you empty the {BIN}, with Windows asking once again.")
        }
        (false, _) => format!("Until you empty the {BIN}, Finder's Put Back returns them."),
    };
    format!("{checks} {back} {STAYS}")
}

/// How one thing Remove took went, as its song's line counts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Went<'a> {
    /// It stayed, and why ("" when nothing was said).
    Stayed(&'a str),
    /// It went, but wasn't found in the bin: why.
    Lost(&'a str),
    InBin,
    PuttingBack,
    Back,
    /// It couldn't come back, and why.
    Stuck(&'a str),
}

/// How `item` went, once Remove has run (`back`: what Put back said couldn't come back, by where each was).
fn went<'a>(item: &SongItem, sent: &'a Sent, stage: &Stage, back: &HashMap<&'a str, &'a str>) -> Went<'a> {
    match (sent.went(item), sent.why(item)) {
        (false, why) => Went::Stayed(why.unwrap_or_default()),
        (true, Some(why)) => Went::Lost(why),
        (true, None) => match stage {
            Stage::PuttingBack(_) => Went::PuttingBack,
            Stage::PutBack(..) => back.get(item.path()).map_or(Went::Back, |why| Went::Stuck(why)),
            _ => Went::InBin,
        },
    }
}

/// How the things on one line of the card went: how many were picked, the room those that went freed, and how many
/// went which way.
#[derive(Default)]
struct Tally<'a> {
    picked: usize,
    freed: u64,
    in_bin: usize,
    putting_back: usize,
    back: usize,
    stayed: Vec<&'a str>,
    lost: Vec<&'a str>,
    stuck: Vec<&'a str>,
}

impl<'a> Tally<'a> {
    /// One more, that went `went`, freeing `freed` ([`Sent::freed`]).
    fn add(&mut self, freed: u64, went: Went<'a>) {
        self.picked += 1;
        self.freed = self.freed.saturating_add(freed);
        match went {
            Went::Stayed(why) => self.stayed.push(why),
            Went::Lost(why) => self.lost.push(why),
            Went::InBin => self.in_bin += 1,
            Went::PuttingBack => self.putting_back += 1,
            Went::Back => self.back += 1,
            Went::Stuck(why) => self.stuck.push(why),
        }
    }

    /// What its line says of how they went, and whether that's a problem: as for one when they all went the same way
    /// ("In the Recycle Bin."), otherwise how many went which way, what went well first ("10 in the Recycle Bin, and 2
    /// stayed: It changed since you looked, so it stays.").
    fn words(&self) -> (String, bool) {
        let n = self.picked;
        let all = |count: usize| n > 0 && count == n;
        let one_why = |whys: &[&'a str]| match whys {
            [first, rest @ ..] if !first.is_empty() && rest.iter().all(|w| w == first) => Some(*first),
            _ => None,
        };
        if all(self.in_bin) {
            return (format!("In the {BIN}."), false);
        }
        if all(self.putting_back) {
            return ("Putting back…".to_owned(), false);
        }
        if all(self.back) {
            return ((if n == 1 { "Back where it was." } else { "Back where they were." }).to_owned(), false);
        }
        for whys in [&self.stayed, &self.lost] {
            if let Some(why) = one_why(whys).filter(|_| all(whys.len())) {
                return (why.to_owned(), true);
            }
        }
        if let Some(why) = one_why(&self.stuck).filter(|_| all(self.stuck.len())) {
            return (format!("{} couldn't come back: {why}", if n == 1 { "It" } else { "They" }), true);
        }
        let mut said = Vec::new();
        if self.in_bin > 0 {
            said.push(format!("{} in the {BIN}", thousands(self.in_bin)));
        }
        if self.back > 0 {
            let were = if self.back == 1 { "it was" } else { "they were" };
            said.push(format!("{} back where {were}", thousands(self.back)));
        }
        if self.putting_back > 0 {
            said.push(format!("{} being put back", thousands(self.putting_back)));
        }
        let why_words = |whys: &[&'a str], what: &str| match one_why(whys) {
            Some(why) => format!("{} {what}: {why}", thousands(whys.len())),
            None => format!("{} {what} (the log says why)", thousands(whys.len())),
        };
        if !self.stayed.is_empty() {
            said.push(why_words(&self.stayed, "stayed"));
        }
        if !self.lost.is_empty() {
            let wasnt = if self.lost.len() == 1 { "wasn't" } else { "weren't" };
            said.push(format!("{} went but {wasnt} found in the {BIN}", thousands(self.lost.len())));
        }
        if !self.stuck.is_empty() {
            said.push(why_words(&self.stuck, "couldn't come back"));
        }
        let mut words = clauses(&said);
        if !words.is_empty() && !words.ends_with('.') {
            words.push('.');
        }
        (words, !(self.stayed.is_empty() && self.lost.is_empty() && self.stuck.is_empty()))
    }
}

/// The card once Remove has run: what was freed, what went and what stayed (each kind picked from, with the room it
/// freed, and under it its songs, the first few each with how its things went, then the rest in one line), and what
/// Put back does; then, once Put back has run, what came back.
fn result_card(sent: &Sent, stage: &Stage, can_put_back: bool, keep: usize) -> Card {
    let mut back: HashMap<&str, &str> = match stage {
        Stage::PutBack(_, problems) => problems.iter().map(|p| (p.path.as_str(), p.why.as_str())).collect(),
        _ => HashMap::new(),
    };
    // One that went inside a folder comes back with it, or doesn't.
    for (inner, outer) in &sent.done().went_with {
        if let Some(why) = back.get(outer.as_str()).copied() {
            back.entry(inner.as_str()).or_insert(why);
        }
    }
    let mut parts = Vec::new();
    // What was picked comes in the card's order: kind by kind, and in each, song by song.
    for of_kind in sent.picked.chunk_by(|a, b| a.item.kind == b.item.kind) {
        let Some(kind) = of_kind.first().map(|i| i.item.kind) else { continue };
        let (mut all, mut rest, mut rest_songs, mut under) = (Tally::default(), Tally::default(), 0, Vec::new());
        for (listed, of_song) in of_kind.chunk_by(|a, b| a.opens == b.opens && a.song == b.song).enumerate() {
            let Some(first) = of_song.first() else { continue };
            let mut song = Tally::default();
            for item in of_song {
                let (went, freed) = (went(item, sent, stage, &back), sent.freed(item));
                song.add(freed, went);
                all.add(freed, went);
                if listed >= SONGS_LISTED {
                    rest.add(freed, went);
                }
            }
            if listed < SONGS_LISTED {
                let names = of_song.iter().map(|i| file_name(&i.item.path));
                let words = (first.song.to_string(), song_what(kind, of_song.len(), names));
                let (status, problem) = song.words();
                under.push(Part { status, problem, ..song_part(&first.opens, words, song.freed, true) });
            } else {
                rest_songs += 1;
            }
        }
        parts.push(Part { status: String::new(), ..kind_part(kind, all.picked, all.freed, keep) });
        parts.extend(under);
        if rest_songs > 0 {
            let (status, problem) = rest.words();
            parts.push(Part { status, problem, ..rest_part(rest_songs, rest.freed, true) });
        }
    }
    let (big, big_note, note) = result_figures(sent, stage, can_put_back, &Counted::of(sent.done(), sent.binned()));
    Card {
        kind: KIND,
        title: TITLE,
        big,
        big_note,
        lead: lead(keep),
        ticks: false,
        parts,
        note,
        room: String::new(),
        warning: String::new(),
    }
}

// ---- The older saves card ---------------------------------------------------------------------------------------

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
fn thin_card_of(found: Option<&Found>, today: NaiveDate) -> Card {
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
    let mut parts: Vec<Part> = songs
        .iter()
        .take(THIN_LISTED)
        .map(|(s, n, bytes)| {
            let days: BTreeSet<NaiveDate> =
                s.items(CompactKind::ThinnedSave).filter_map(|i| i.modified).map(|m| m.date()).collect();
            let stay = count_of(s.saves.saturating_sub(*n), "save stays", "saves stay");
            let what = format!("{}; {stay}", sittings_words(&days, today.year()));
            song_part(&s.opens, (format!("{}: {}", s.name, saves_words(*n)), what), *bytes, false)
        })
        .collect();
    let rest = songs.get(THIN_LISTED..).unwrap_or_default();
    if !rest.is_empty() {
        parts.push(rest_part(rest.len(), rest.iter().fold(0u64, |sum, (_, _, b)| sum.saturating_add(*b)), false));
    }
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
            "Saves made less than {hours} hours apart while you worked. The first save, the last save of each \
             sitting and the save each song opens always stay, and so do saves a copy wrote all at once. None goes \
             unless you pick it."
        ),
        ticks: false,
        note: if parts.is_empty() { String::new() } else { NEXT.to_owned() },
        parts,
        room: String::new(),
        warning: String::new(),
    }
}

/// The Older saves from the middle of a sitting card: what the last look found, or that it's measuring.
pub fn thin_card(shown: Option<&Shown>) -> Card {
    match shown {
        Some(s) => s.thin_card.clone(),
        // With no song measured yet, there's no day to say.
        None => thin_card_of(None, NaiveDate::default()),
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDateTime;
    use mo_space::clean::Done;

    use super::*;
    use crate::space_bin::PutBacks;

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

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 4).unwrap_or_default()
    }

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

    /// `n` songs, each with one old autosave, the first the biggest; the second song has a Studio One cache too.
    fn autosaves(n: usize) -> Found {
        let mut songs: Vec<SongRoom> = (0..n)
            .map(|at| {
                let path = format!(r"D:\Projects\Song {at}\auto-backups\a.bwproject");
                song(&format!("Song {at}"), 3, vec![item(CompactKind::Backup, &path, (100 - at as u64) * MB, None)])
            })
            .collect();
        if let Some(second) = songs.get_mut(1) {
            second.report.items.push(item(CompactKind::Cache, r"D:\Projects\Song 1\Cache", 30 * MB, None));
        }
        Found { songs, ..Found::default() }
    }

    /// The card's lines as (name, what, size, under, status).
    fn rows(card: &Card) -> Vec<(&str, &str, &str, bool, &str)> {
        card.parts
            .iter()
            .map(|p| (p.name.as_str(), p.what.as_str(), p.size.as_str(), p.under, p.status.as_str()))
            .collect()
    }

    /// Each line's tick: whether it has one, it's ticked and it shows a dash.
    fn ticks(card: &Card) -> Vec<Option<(bool, bool)>> {
        card.parts.iter().map(|p| p.tick.as_ref().map(|(_, ticked)| (*ticked, p.partly))).collect()
    }

    /// What the tick on the card's line at `at` is for.
    fn tick_of(card: &Card, at: usize) -> Vec<PathBuf> {
        card.parts.get(at).and_then(|p| p.tick.clone()).map(|(paths, _)| paths).unwrap_or_default()
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
        let shown = Shown::of(&sample(), today());
        let card = compact_card(Some(&shown), &Removing::default(), true);
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("380 MB", "in 1 song"));
        assert_eq!(
            card.lead,
            "Nothing that holds music: the same save twice, autosaves past the newest 5, and caches your DAW makes again."
        );
        assert_eq!(
            rows(&card),
            [
                ("1 identical save", "Byte for byte the same as a save that stays", "300 MB", false, NOT_YET),
                ("Night Drive", "1 save", "300 MB", true, ""),
                ("2 old autosaves", "Autosaves past the newest 5 of each song", "80 MB", false, ""),
                ("Night Drive", "2 autosaves", "80 MB", true, ""),
                (
                    "4 audio files no save seems to use",
                    "Not in the figure above: listen to each first, as none goes unless you pick it",
                    "930 MB",
                    false,
                    NOT_YET
                ),
                ("Night Drive", "Old bounce.wav", "900 MB", true, ""),
                ("Old Idea", "Take 1.wav, Take 2.wav and 1 more", "30 MB", true, ""),
            ]
        );
        // Only the old autosaves have ticks, ticked out of the box: not the identical saves, nor the audio.
        assert_eq!(ticks(&card), [None, None, Some((true, false)), Some((true, false)), None, None, None]);
        assert!(card.ticks);
        // Each song's line shows the save it opens in its folder.
        assert_eq!(
            card.parts.get(1).and_then(|p| p.open.clone()),
            Some((Opens::InFolder, PathBuf::from(r"D:\Projects\Night Drive\Night Drive_5.bwproject")))
        );
        assert_eq!(card.note, remove_note(5, true, false));
        let first = compact_card(None, &Removing::default(), true);
        assert_eq!((first.big.as_str(), first.big_note.as_str()), ("Measuring…", ""));
        assert!(first.parts.is_empty() && first.note.is_empty() && !first.ticks);
        let none = compact_card(Some(&Shown::of(&Found::default(), today())), &Removing::default(), true);
        assert_eq!(none.big, "Nothing to free");
        // Keeping no autosaves says so.
        let every = Found { settings: CompactSettings { keep_backups: 0, ..CompactSettings::default() }, ..sample() };
        let card = compact_card(Some(&Shown::of(&every, today())), &Removing::default(), true);
        assert!(card.parts.iter().any(|p| p.what == "Every autosave of each song"));
        // With nothing Remove takes, the note says only what stays.
        let audio = Found { songs: vec![sample().songs.remove(2)], ..Found::default() };
        let card = compact_card(Some(&Shown::of(&audio, today())), &Removing::default(), true);
        assert_eq!((card.note.as_str(), card.ticks), (STAYS, false));
    }

    #[test]
    fn a_long_list_adds_up_the_songs_past_the_first_few() {
        let shown = Shown::of(&autosaves(SONGS_LISTED + 2), today());
        let card = compact_card(Some(&shown), &Removing::default(), true);
        // The old autosaves (with their songs, then the rest), then the cache.
        assert_eq!(card.parts.len(), 1 + SONGS_LISTED + 1 + 2);
        let last = card.parts.get(SONGS_LISTED + 1).cloned().unwrap_or_default();
        assert_eq!((last.name.as_str(), last.size.as_str(), last.under), ("And 2 more songs", "179 MB", true));
        assert_eq!(last.open, None);
        assert_eq!(last.tick.map(|(_, ticked)| ticked), Some(true));
    }

    #[test]
    fn a_tick_is_for_every_one_under_its_line() {
        let shown = Shown::of(&autosaves(SONGS_LISTED + 2), today());
        let mut removing = Removing::default();
        let card = compact_card(Some(&shown), &removing, true);
        let all = shown.items.iter().filter(|i| i.item.kind == CompactKind::Backup).count();
        assert_eq!(all, SONGS_LISTED + 2);
        // A tick names where its things are, not each file: a kind can have thousands.
        assert_eq!(tick_of(&card, 0).len(), 1);

        // Song 1's autosave unticked: its kind's tick shows a dash, and Remove frees the rest.
        tick(&mut removing, Some(&shown), &tick_of(&card, 2), false);
        let card = compact_card(Some(&shown), &removing, true);
        assert_eq!(ticks(&card).first().copied().flatten(), Some((false, true)));
        assert_eq!(ticks(&card).get(2).copied().flatten(), Some((false, false)));
        // The songs past the first few, unticked together.
        tick(&mut removing, Some(&shown), &tick_of(&card, SONGS_LISTED + 1), false);
        let picked = removing.ticked_copies(Some(&shown));
        let names: Vec<&str> = picked.iter().filter(|i| i.item.kind == CompactKind::Backup).map(|i| &*i.song).collect();
        assert_eq!(names.len(), all - 3);
        assert!(!names.contains(&"Song 1") && !names.contains(&"Song 10") && !names.contains(&"Song 11"));
        // Song 1's cache is still ticked: its own line's tick.
        assert!(picked.iter().any(|i| i.item.kind == CompactKind::Cache));
        let free: u64 = picked.iter().map(|i| i.item.bytes).sum();
        assert_eq!(
            removing.remove_button(Some(&shown), false, &PutBacks::default()),
            Some((space_removing::send_words(free), true))
        );
        // A file's own path on a tick isn't one of the card's: nothing changes.
        let before = removing.clone();
        tick(&mut removing, Some(&shown), &[PathBuf::from(r"D:\Projects\Song 0\auto-backups\a.bwproject")], false);
        assert_eq!(removing, before);
        // The kind's tick ticks them all again.
        tick(&mut removing, Some(&shown), &tick_of(&card, 0), true);
        assert_eq!(removing.ticked_copies(Some(&shown)).len(), all + 1);

        // While Remove is on there are no ticks, and the note says what it's doing.
        removing.start(removing.ticked_copies(Some(&shown)));
        let card = compact_card(Some(&shown), &removing, true);
        assert!(card.parts.iter().all(|p| p.tick.is_none()));
        assert_eq!(card.note, format!("Checking each ticked one again, then sending it to the {BIN}…"));
        assert_eq!(
            removing.remove_button(Some(&shown), false, &PutBacks::default()),
            Some((space_removing::sending_words(), false))
        );
    }

    #[test]
    fn the_note_says_what_remove_does_what_stays_and_how_they_come_back() {
        assert_eq!(
            remove_note(5, true, false),
            format!(
                "The ticked autosaves and caches go to the {BIN}, each checked again just before it goes: one that \
                 changed since stays, and so do the newest 5 autosaves of each song. Put back returns them until you \
                 empty the {BIN}. {STAYS}"
            )
        );
        assert!(remove_note(1, true, false).contains("stays, and so does the newest autosave of each song."));
        assert!(remove_note(0, true, false).contains("one that changed since stays. Put back"));
        assert!(remove_note(5, false, false).contains("Finder's Put Back returns them."));
        // Ticked ones in Program Files go together, Windows asking once; unticked, they don't count.
        let mut found = autosaves(2);
        if let Some(first) = found.songs.first_mut().and_then(|s| s.report.items.first_mut()) {
            first.path = r"C:\Program Files\Image-Line\Data\Projects\Backup\Song 0 (autosave).flp".to_owned();
        }
        let shown = Shown::with(&found, today(), &|p| p.starts_with(r"C:\Program Files"));
        let mut removing = Removing::default();
        assert!(compact_card(Some(&shown), &removing, true).note.contains(ADMIN));
        let card = compact_card(Some(&shown), &removing, true);
        tick(&mut removing, Some(&shown), &tick_of(&card, 1), false);
        assert!(!compact_card(Some(&shown), &removing, true).note.contains(ADMIN));
    }

    /// One thing of `found`'s, as the bin keeps it once it went.
    fn binned(i: &SongItem) -> Removed {
        Removed {
            path: i.item.path.clone(),
            real: i.item.path.clone(),
            bytes: i.item.bytes,
            what: What::Song(i.item.kind),
            when: 1_793_805_600,
            bin_id: Some(String::new()),
            binned: None,
            binned_zone: None,
            stamp: None,
        }
    }

    #[test]
    fn once_remove_has_run_the_card_says_how_each_song_went_and_put_back_brings_them_back() {
        const WHEN: i64 = 1_793_805_600;
        let mut found = autosaves(SONGS_LISTED + 2);
        // Song 0 has three old autosaves.
        if let Some(first) = found.songs.first_mut() {
            for name in ["b", "c"] {
                let path = format!(r"D:\Projects\Song 0\auto-backups\{name}.bwproject");
                first.report.items.push(item(CompactKind::Backup, &path, 10 * MB, None));
            }
        }
        let shown = Shown::of(&found, today());
        let mut removing = Removing::default();
        let picked = removing.ticked_copies(Some(&shown));
        removing.start(picked.clone());
        // All went but one of Song 0's, which changed since the look.
        let changed = r"D:\Projects\Song 0\auto-backups\c.bwproject";
        let why = Refused::Changed.text();
        let done = Done {
            removed: picked.iter().filter(|i| i.item.path != changed).map(binned).collect(),
            problems: vec![Problem { path: changed.to_owned(), why: why.to_owned() }],
            went_with: Vec::new(),
        };
        removing.sent(WHEN, done);
        let card = compact_card(Some(&shown), &removing, true);
        let freed: u64 = picked.iter().filter(|i| i.item.path != changed).map(|i| i.item.bytes).sum();
        assert_eq!(card.big, format!("Sent {}", size_text(freed)));
        // What's in the bin is counted by kind; what stayed only by how many.
        let in_bin = picked.len() - 2;
        assert_eq!(card.big_note, format!("{in_bin} old autosaves and 1 cache are in the {BIN}, and 1 stayed"));
        let rows = rows(&card);
        assert_eq!(rows.first().map(|r| (r.0, r.4)), Some(("14 old autosaves", "")));
        let in_bin = format!("In the {BIN}.");
        assert_eq!(
            rows.get(1).copied(),
            Some(("Song 0", "3 autosaves", "110 MB", true, &*format!("2 in the {BIN}, and 1 stayed: {why}")))
        );
        assert_eq!(rows.get(2).map(|r| (r.0, r.4)), Some(("Song 1", in_bin.as_str())));
        assert_eq!(rows.get(SONGS_LISTED + 1).map(|r| (r.0, r.2, r.4)), Some(("And 2 more songs", "179 MB", &*in_bin)));
        assert_eq!(rows.get(SONGS_LISTED + 3).map(|r| (r.0, r.1, r.4)), Some(("Song 1", "Its Cache folder", &*in_bin)));
        assert!(card.parts.get(1).is_some_and(|p| p.problem) && card.parts.get(2).is_some_and(|p| !p.problem));
        assert!(card.parts.iter().all(|p| p.tick.is_none()) && !card.ticks);
        let [line, _] = lines(Some(&shown), false, &removing);
        let line = line.map(|(bytes, l)| (bytes, l.why, l.size)).unwrap_or_default();
        assert_eq!(line, (0, card.big_note.clone(), card.big.clone()));

        // While it's put back, and once it's back but for one that couldn't come back.
        removing.putting_back(WHEN);
        let card = compact_card(Some(&shown), &removing, true);
        assert_eq!(card.parts.get(2).map(|p| p.status.as_str()), Some("Putting back…"));
        assert_eq!(card.parts.get(1).map(|p| p.status.clone()), Some(format!("2 being put back, and 1 stayed: {why}")));
        let stuck = r"D:\Projects\Song 1\Cache";
        let gone = "It isn't in the Recycle Bin any more.";
        removing.put_back(WHEN, vec![Problem { path: stuck.to_owned(), why: gone.to_owned() }]);
        let card = compact_card(Some(&shown), &removing, true);
        assert_eq!(card.parts.get(2).map(|p| p.status.as_str()), Some("Back where it was."));
        assert_eq!(
            card.parts.get(1).map(|p| p.status.clone()),
            Some(format!("2 back where they were, and 1 stayed: {why}"))
        );
        let cache = card.parts.get(SONGS_LISTED + 3).map(|p| (p.status.clone(), p.problem));
        assert_eq!(cache, Some((format!("It couldn't come back: {gone}"), true)));
        let [line, _] = lines(Some(&shown), false, &removing);
        assert_eq!(
            line.map(|(_, l)| l.why),
            Some(format!("{} are back where they were, and 1 couldn't come back", picked.len() - 2))
        );
    }

    #[test]
    fn each_songs_newest_saves_and_the_one_it_opens_are_kept_from_remove() {
        let save = |name: &str, modified: NaiveDateTime| Save {
            path: format!(r"D:\Projects\Night Drive\{name}.bwproject"),
            name: name.to_owned(),
            modified,
            ..Save::default()
        };
        // It opens its second save; its third and fourth were saved at the same moment, the newest; its fifth is gone
        // (so isn't the newest), and its sixth isn't a project the app knows.
        let song = Song {
            name: "Night Drive".to_owned(),
            latest: save("Night Drive_2", at(10, "10:00")),
            older: vec![
                save("Night Drive_3", at(12, "18:00")),
                save("Night Drive_4", at(12, "18:00")),
                save("Night Drive_1", at(9, "09:00")),
                Save { file_missing: true, ..save("Night Drive_5", at(13, "10:00")) },
                Save { path: r"D:\Projects\Night Drive\Night Drive_6.txt".to_owned(), ..save("x", at(14, "10:00")) },
            ],
            key: String::new(),
            copies: Vec::new(),
        };
        let kept = kept_saves(&[song]);
        let want: HashSet<String> = ["Night Drive_2", "Night Drive_3", "Night Drive_4"]
            .iter()
            .map(|n| key(&format!(r"D:\Projects\Night Drive\{n}.bwproject")))
            .collect();
        assert_eq!(kept, want);
    }

    #[test]
    fn what_went_inside_a_folder_that_went_went_with_it_and_says_what_the_folder_does() {
        let cache = item(CompactKind::Cache, r"D:\Projects\Song\Cache", 30 * MB, None);
        let inside = item(CompactKind::Backup, r"D:\Projects\Song\Cache\History\a.song", MB, None);
        let stayed = item(CompactKind::Backup, r"D:\Projects\Song\History\b.song", MB, None);
        let of_song = |item: &CompactItem| SongItem {
            song: Arc::from("Song"),
            opens: Arc::from(r"D:\Projects\Song\Song.song"),
            item: item.clone(),
        };
        let mut done = Done {
            removed: vec![binned(&of_song(&cache))],
            problems: vec![Problem { path: stayed.path.clone(), why: "It stays.".into() }],
            went_with: Vec::new(),
        };
        went_inside(&[cache.clone(), inside.clone(), stayed.clone()], &mut done);
        assert_eq!(done.went_with, [(inside.path.clone(), cache.path.clone())]);
        // It went (its room is in the folder's), is in the bin with it, and comes back with it.
        let picked: Vec<SongItem> = [&cache, &inside, &stayed].into_iter().map(of_song).collect();
        let sent = Sent::new(picked.clone(), 0, done.clone());
        let [folder, within, other] = [0, 1, 2].map(|n| picked.get(n).cloned().unwrap_or_else(|| of_song(&cache)));
        assert!(sent.went(&within) && sent.in_bin(&within) && !sent.went(&other));
        assert_eq!((sent.freed(&folder), sent.freed(&within), sent.freed(&other)), (30 * MB, 0, 0));
        assert_eq!(sent.stayed(), 1);
        assert_eq!(sent.coming_back().went_with, done.went_with);
        // When the folder isn't found in the bin, nor is what's in it.
        let mut lost = done;
        lost.problems.push(Problem { path: cache.path.clone(), why: clean::NOT_IN_BIN.into() });
        let sent = Sent::new(picked, 0, lost);
        assert!(sent.went(&within) && !sent.in_bin(&within));
        assert_eq!(sent.why(&within), Some(clean::NOT_IN_BIN));
    }

    #[test]
    fn a_lines_words_say_how_its_things_went() {
        let tally = |went: &[Went<'static>]| {
            let mut t = Tally::default();
            for w in went {
                t.add(MB, *w);
            }
            t.words()
        };
        assert_eq!(tally(&[Went::InBin, Went::InBin]), (format!("In the {BIN}."), false));
        assert_eq!(tally(&[Went::Back]), ("Back where it was.".into(), false));
        assert_eq!(tally(&[Went::Back, Went::Back]), ("Back where they were.".into(), false));
        assert_eq!(tally(&[Went::Stayed("It isn't there any more.")]), ("It isn't there any more.".into(), true));
        assert_eq!(tally(&[Went::Lost("Not found.")]), ("Not found.".into(), true));
        assert_eq!(tally(&[Went::Stuck("Gone.")]), ("It couldn't come back: Gone.".into(), true));
        assert_eq!(
            tally(&[Went::Stuck("Gone."), Went::Stuck("Gone.")]),
            ("They couldn't come back: Gone.".into(), true)
        );
        // Mixed: how many went which way, what went well first; different reasons are in the log.
        assert_eq!(
            tally(&[Went::InBin, Went::Stayed("A."), Went::Stayed("B."), Went::Lost("Not found.")]),
            (format!("1 in the {BIN}, 2 stayed (the log says why), and 1 went but wasn't found in the {BIN}."), true)
        );
        assert_eq!(
            tally(&[Went::Back, Went::Back, Went::Stuck("Gone.")]),
            ("2 back where they were, and 1 couldn't come back: Gone.".into(), true)
        );
        assert_eq!(tally(&[]), (String::new(), false));
    }

    #[test]
    fn what_went_is_counted_by_its_kind() {
        let shown = Shown::of(&autosaves(3), today());
        let removed: Vec<Removed> = shown.items.iter().map(binned).collect();
        assert_eq!(sent_things(&removed), "3 old autosaves and 1 cache");
        assert_eq!(sent_things(removed.get(..1).unwrap_or_default()), "1 old autosave");
        let done = Done { removed, ..Done::default() };
        assert_eq!(
            removed_words(&done),
            Ok(format!("Sent {}: 3 old autosaves and 1 cache are in the {BIN}.", size_text(done.bytes())))
        );
        // One that went but wasn't found in the bin is counted on its own.
        let first = done.removed.first().map(|r| r.path.clone()).unwrap_or_default();
        let lost = Done { problems: vec![Problem { path: first, why: "Not found.".to_owned() }], ..done.clone() };
        assert_eq!(
            removed_words(&lost),
            Ok(format!(
                "Sent {}: 2 old autosaves and 1 cache are in the {BIN}, and 1 went but wasn't found there.",
                size_text(done.bytes())
            ))
        );
        let caches = Done {
            removed: done.removed.iter().filter(|r| r.what == What::Song(CompactKind::Cache)).cloned().collect(),
            ..Done::default()
        };
        assert_eq!(removed_words(&caches), Ok(format!("Sent 30 MB: 1 cache is in the {BIN}.")));
        assert_eq!(removed_words(&Done::default()), Err("Nothing was removed.".to_owned()));
    }

    #[test]
    fn the_thin_card_lists_each_song_with_its_sittings_and_what_stays() {
        let card = thin_card_of(Some(&sample()), today());
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("45 MB", "2 saves in 1 song"));
        assert!(card.lead.starts_with("Saves made less than 3 hours apart while you worked."));
        assert!(card.lead.contains("and so do saves a copy wrote all at once."));
        let rows: Vec<(&str, &str, &str, bool)> =
            card.parts.iter().map(|p| (p.name.as_str(), p.what.as_str(), p.size.as_str(), p.under)).collect();
        assert_eq!(rows, [("Bounce: 2 saves", "Sittings on 3 and 9 Oct; 4 saves stay", "45 MB", false)]);
        assert_eq!(card.note, NEXT);
        let none = thin_card_of(Some(&Found::default()), NaiveDate::default());
        assert!(none.parts.is_empty() && none.note.is_empty() && none.big == "Nothing to free");
    }

    #[test]
    fn what_shows_is_worked_out_once_and_says_it_is_measuring_until_a_look_finishes() {
        let found = sample();
        let shown = Shown::of(&found, today());
        let removing = Removing::default();
        // What the last look found stays on show while the next one looks.
        assert_eq!(lines(Some(&shown), true, &removing), [compact_line(Some(&found), false), thin_line(Some(&found))]);
        assert_eq!(thin_card(Some(&shown)), thin_card_of(Some(&found), today()));
        assert_eq!(shown.none_note, "");
        // Before any look has finished, the compact line says one is measuring while it is, and the cards wait for it.
        let [compact, thin] = lines(None, true, &removing);
        assert_eq!((compact.map(|(_, l)| l.why), thin), (Some(LOOKING.to_owned()), None));
        assert_eq!(lines(None, false, &removing), [None, None]);
        let waiting = (compact_card(None, &removing, true).big, thin_card(None).big);
        assert_eq!(waiting, ("Measuring…".to_owned(), "Measuring…".to_owned()));
        let none = Shown::of(&Found::default(), today());
        assert_eq!((none.compact, none.thin, none.none_note), (None, None, NONE_FOUND));
        // Saves from the middle of a sitting have their own words, should they ever be a kind on a card.
        assert_eq!(kind_part(CompactKind::ThinnedSave, 2, MB, 5).name, "2 saves from the middle of a sitting");
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
