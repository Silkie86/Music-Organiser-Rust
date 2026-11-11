//! The MIDI page's "Fits my song" list: the parts of the library that go with the song being worked on, the best
//! first, each with how far to move it. Its screen logic and every word it shows; no window in it, so all of it is
//! tested here.
//!
//! The song is one of the few saved last ([`song_choices`]: the first is the one being worked on), read from its
//! newest save as it plays (`mo_midi::playing`). Looking for tunes, what's fitted to is its chords and bass; looking
//! for chords, its melodies; over the bars where the most of them play together ([`fitted`]). The page's own filters
//! stay in force ([`tried`]): a kind chip narrows the list to basslines, the search box to a pack. The song's own
//! parts are never offered.
//!
//! The slow work is the page's to run off the window's thread, newest wins (rule 12). [`search`] is all of it in
//! one call: reading the song, the index's slots, the song's own parts, then [`tried`] and [`look`]. What it reads
//! comes through [`Reads`] (the page's index on disk; made-up ones in the tests and the screenshots), and what's slow
//! to make it keeps in a [`Kept`] for the next search: the song until its save is written again, the slots and the
//! song's own parts until the index changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use mo_formats::NoteEvent;
use mo_formats::dates::RealDates;
use mo_library::songs::thousands;
use mo_midi::fit::for_song::{Own, Pick, Tried};
use mo_midi::fit::{Found, Target, Wants};
use mo_midi::playing::{self, Song};
use mo_midi::store::SlotBook;
use mo_midi::{Catalogue, Role};

use crate::midi::{self as page, Filters};

/// The most lines the list shows: the parts that fit best.
pub const MOST: usize = 200;

/// How many of the songs saved last the page offers to fit to.
pub const SONGS_OFFERED: usize = 8;

/// Said under the toolbar while a search runs.
pub const LOOKING: &str = "Looking for parts that fit…";

/// A song the page offers to fit to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SongChoice {
    pub name: String,
    /// What the page's menu calls it: its name, with its folder after it where two of the songs offered share a name.
    /// No two of the songs offered have the same.
    pub label: String,
    /// Its newest save that can be read: the file the song is read from.
    pub save: String,
    /// When that save was written (Unix seconds), as the song library last saw it.
    pub mtime: i64,
    /// Every save of the song, as the song library groups them (the ones you joined to it by hand too, and the same
    /// files found somewhere else): whatever the MIDI library has from any of them is the song's own.
    pub saves: Vec<String>,
}

impl SongChoice {
    /// The song's own parts in the catalogue, which a search for parts that fit it leaves out. Keep it while the
    /// catalogue and the choice stay.
    pub fn own(&self, cat: &Catalogue) -> Own {
        Own::of(cat, self.saves.iter().map(String::as_str))
    }

    /// This song among the songs offered `now`, which were made after it was: the one that has the save this one is
    /// read from, else one that shares another save with it (that save gone, or split off by hand). By its saves,
    /// never by its label: a second song of its name changes that. None when it isn't offered any more.
    pub fn among<'a>(&self, now: &'a [SongChoice]) -> Option<&'a SongChoice> {
        let shares = |s: &&SongChoice| s.saves.iter().any(|save| self.saves.contains(save));
        now.iter().find(|s| s.saves.contains(&self.save)).or_else(|| now.iter().find(shares))
    }
}

/// The songs saved last, the newest first: the first is the song being worked on. `songs`: every song, as the Songs
/// page groups them.
pub fn song_choices<'a>(songs: impl IntoIterator<Item = &'a mo_library::Song>, dates: &RealDates) -> Vec<SongChoice> {
    let offered = playing::worked_on(songs, dates, SONGS_OFFERED);
    let shared = |name: &str| offered.iter().filter(|(song, _)| song.name == name).count() > 1;
    let mut out: Vec<SongChoice> = Vec::new();
    for (song, save) in &offered {
        // Two songs of one name are told apart by the folder each is in, and by a number if that's the same too.
        let folder = mo_formats::paths::file_name(save.folder.trim_end_matches(['\\', '/']));
        let named = if shared(&song.name) && !folder.is_empty() {
            format!("{} · {folder}", song.name)
        } else {
            song.name.clone()
        };
        let mut label = named.clone();
        let mut n = 1;
        while out.iter().any(|c| c.label == label) {
            n += 1;
            label = format!("{named} ({n})");
        }
        out.push(SongChoice {
            name: song.name.clone(),
            label,
            save: save.path.clone(),
            mtime: save.mtime,
            saves: song.saves().chain(&song.copies).map(|s| s.path.clone()).collect(),
        });
    }
    out
}

/// The switch's words for what's looked for.
pub fn wants_words(wants: Wants) -> &'static str {
    match wants {
        Wants::Tune => "To play over its chords",
        Wants::Chords => "Chords for its melody",
    }
}

/// What of a song the library's parts are fitted to.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub wants: Wants,
    pub pick: Pick,
    pub target: Target,
}

/// What of `song` is fitted to, nothing picked by hand. `wanted`: what the page's switch asks for; none (the switch
/// hasn't been touched): parts to play over the song's chords when it has any that can be read, else chords for its
/// melody. Err: why nothing can be fitted, in plain words.
pub fn fitted(song: &Song, wanted: Option<Wants>) -> Result<Fitted, String> {
    let pitched = |p: &playing::Playing| !p.drums && p.role != Role::Drums && !p.notes.is_empty();
    if !song.parts.iter().any(pitched) {
        return Err("This song has no notes to fit to yet (drums aside).".to_owned());
    }
    let (chords, melody) = (Pick::usual(song, Wants::Tune), Pick::usual(song, Wants::Chords));
    // What the parts are decides what's looked for (a melody alone wants chords), so the two always agree.
    let of = |pick: &Option<Pick>| {
        let pick = pick.clone()?;
        let target = pick.target(song)?;
        Some(Fitted { wants: target.wants(), pick, target })
    };
    let (over, under) = (of(&chords), of(&melody));
    // The other way is only pointed to when it would work.
    let (can_over, can_under) = (over.is_some(), under.is_some());
    let fitted = match wanted {
        Some(Wants::Tune) => over,
        Some(Wants::Chords) => under,
        None => over.or(under),
    };
    if let Some(fitted) = fitted {
        return Ok(fitted);
    }
    // Why not: nothing of the kind in the song, or nothing that could be read where it plays.
    let no_chords = match &chords {
        Some(pick) => format!("No chords could be read in {} of this song.", bars_words(&pick.bars)),
        None => "No chords or bassline found in this song to play over.".to_owned(),
    };
    let no_melody = match &melody {
        Some(pick) => format!("The melody couldn't be read in {} of this song.", bars_words(&pick.bars)),
        None => "No melody found in this song to put chords under.".to_owned(),
    };
    Err(match wanted {
        Some(Wants::Tune) if can_under => format!("{no_chords} It has a melody: look for chords for that."),
        Some(Wants::Tune) => no_chords,
        Some(Wants::Chords) if can_over => format!("{no_melody} It has chords: look for parts to play over them."),
        Some(Wants::Chords) => no_melody,
        None if chords.is_none() && melody.is_none() => {
            "No chords, bassline or melody found in this song (drums and samples aside).".to_owned()
        }
        None if chords.is_some() => no_chords,
        None => no_melody,
    })
}

/// "bars 9–16" (the first bar is 1); "bar 9" for one.
pub fn bars_words(bars: &std::ops::Range<i64>) -> String {
    let (first, last) = (bars.start + 1, bars.end.max(bars.start + 1));
    if last == first { format!("bar {first}") } else { format!("bars {first}–{last}") }
}

/// The picked parts' names: "Keys", "Keys and Sub", "Keys, Pad and Sub", "Keys, Pad and 3 more".
fn part_names(song: &Song, pick: &Pick) -> String {
    // A name once: two channels called "Keys" (a layered sound) are "Keys".
    let mut names: Vec<&str> = Vec::new();
    for name in pick.parts.iter().filter_map(|&at| song.parts.get(at)).map(|p| p.name.as_str()) {
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [first, second, third] => format!("{first}, {second} and {third}"),
        [first, second, rest @ ..] => format!("{first}, {second} and {} more", rest.len()),
    }
}

/// Said beside the song's name: what of it the list is fitted to. "To play over Keys and Sub, bars 9–16", "Chords
/// for Lead, bars 13–16".
pub fn fitted_text(song: &Song, fitted: &Fitted) -> String {
    let names = part_names(song, &fitted.pick);
    let bars = bars_words(&fitted.pick.bars);
    match (fitted.wants, names.is_empty()) {
        (Wants::Tune, false) => format!("To play over {names}, {bars}"),
        (Wants::Tune, true) => format!("To play over its chords, {bars}"),
        (Wants::Chords, false) => format!("Chords for {names}, {bars}"),
        (Wants::Chords, true) => format!("Chords for its melody, {bars}"),
    }
}

/// The parts tried for a song: every part the page's filters let through, without the song's `own`.
pub fn tried<'a>(cat: &'a Catalogue, own: &Own, filters: &'a Filters) -> Tried<'a> {
    mo_midi::fit::for_song::tried(cat, own, page::passing(cat, filters))
}

/// One line of the list: a part that fits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// Its place in the catalogue's entries.
    pub entry: usize,
    /// How far to move it, in semitones (-6 to 5; 0: as it is). The part moves, never the song.
    pub semitones: i32,
    /// How it sits moved so: about 0 (it clashes) to about 1 (as a tune written for those chords does).
    pub sits: f64,
}

impl Fit {
    fn of(found: &Found) -> Fit {
        let (semitones, sits) = found.sits.best();
        Fit { entry: found.at, semitones, sits }
    }

    /// How it sits, for a meter and for words: 0 to 100.
    pub fn percent(&self) -> u32 {
        (self.sits.clamp(0.0, 1.0) * 100.0).round() as u32
    }
}

/// How a part is moved to fit: "as it is", "1 semitone up", "3 semitones down".
pub fn move_words(semitones: i32) -> String {
    let way = if semitones > 0 { "up" } else { "down" };
    match semitones.unsigned_abs() {
        0 => "as it is".to_owned(),
        1 => format!("1 semitone {way}"),
        n => format!("{n} semitones {way}"),
    }
}

/// Said first in a line's details: "Fits 92% as it is", "Fits 88%, 2 semitones up".
pub fn fit_text(fit: &Fit) -> String {
    let comma = if fit.semitones == 0 { "" } else { "," };
    format!("Fits {}%{comma} {}", fit.percent(), move_words(fit.semitones))
}

/// What a search came to, for the list and the words about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Looked {
    /// The parts that fit, the best first; [`MOST`] at the most.
    pub fits: Vec<Fit>,
    /// How many parts the filters let through to be tried.
    pub tried: usize,
    /// Of those, how many were of a kind worth trying and had slots to go by (each set of notes once).
    pub looked: usize,
    /// How many had their notes read for the close look.
    pub close: usize,
    /// Left out: the index has no slots for them until the MIDI is read again.
    pub not_ready: usize,
    /// Left out: their notes couldn't be read.
    pub unread: usize,
    /// Stopped for a newer search: not the whole answer, and not to be shown.
    pub stopped: bool,
    /// How long it took, and how much of that was reading notes from the index.
    pub took: Duration,
    pub reading: Duration,
}

/// The search: the parts of `tried` that go best with what the song gave, [`MOST`] at the most. `book` has the
/// index's slots, `notes` reads one part's notes from it (by its row and note hash), and `stop` set ends it early (a
/// newer search is wanted). The first look and the judging run on `pool`; call it off the window's thread.
pub fn look(
    fitted: &Fitted,
    tried: &Tried,
    book: &SlotBook,
    notes: &mut dyn FnMut(i64, &str) -> Option<Vec<NoteEvent>>,
    pool: &rayon::ThreadPool,
    stop: &AtomicBool,
) -> Looked {
    let started = Instant::now();
    let mut reading = Duration::ZERO;
    let mut timed = |id: i64, hash: &str| {
        let from = Instant::now();
        let read = notes(id, hash);
        reading += from.elapsed();
        read
    };
    let search = tried.search(&fitted.target, book, &mut timed, MOST, pool, stop);
    Looked {
        fits: search.found.iter().map(Fit::of).collect(),
        tried: tried.len(),
        looked: search.looked,
        close: search.close,
        not_ready: search.not_ready,
        unread: search.unread,
        stopped: search.stopped,
        took: started.elapsed(),
        reading,
    }
}

fn parts(n: usize) -> String {
    if n == 1 { "1 part".to_owned() } else { format!("{} parts", thousands(n)) }
}

/// Under the toolbar: what the list holds. How many parts were tried is only said when the list was cut to its
/// length: the parts tried count every key's copy of a loop and the list has a loop once, so "28 of 60" would read
/// as 32 worse fits hidden when none is.
pub fn result_text(l: &Looked) -> String {
    match (l.fits.len(), l.looked) {
        (0, 0) if l.not_ready > 0 => {
            "Nothing can be fitted yet: read your MIDI again first (Read MIDI again), then it can.".to_owned()
        }
        (0, 0) => "No parts to try: the filters leave none of the right kind.".to_owned(),
        (0, looked) => format!("None of {} fits.", parts(looked)),
        (shown, looked) if shown >= MOST => {
            format!("The {} that fit best, of {} tried", thousands(shown), parts(looked))
        }
        (shown, _) => format!("{}, best fit first", parts(shown)),
    }
}

/// What was left out of a search, to say after [`result_text`]; "" when nothing was.
pub fn left_out_text(l: &Looked) -> String {
    let mut out = Vec::new();
    // With nothing looked at, `result_text` has said so already.
    if l.not_ready > 0 && l.looked > 0 {
        out.push(format!("{} left out until your MIDI is read again", parts(l.not_ready)));
    }
    if l.unread > 0 {
        out.push(format!("{} left out (notes couldn't be read)", parts(l.unread)));
    }
    out.join(" · ")
}

/// The log's line for a search: how much it looked at and how long it took.
pub fn log_line(l: &Looked) -> String {
    format!(
        "tried {}, first look {}, notes read {} in {:.2} s, found {}, {:.2} s in all{}",
        thousands(l.tried),
        thousands(l.looked),
        thousands(l.close),
        l.reading.as_secs_f64(),
        l.fits.len(),
        l.took.as_secs_f64(),
        if l.stopped { " (stopped for a newer search)" } else { "" }
    )
}

/// Under the toolbar once a search has ended: what of the song the list was fitted to (`to`, [`fitted_text`]), what
/// the list holds, and what was left out.
pub fn page_line(to: &str, l: &Looked) -> String {
    let said = [to.to_owned(), result_text(l), left_out_text(l)];
    said.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// Where a search reads from: the song's save and the MIDI index. The page gives the ones on disk; the tests and the
/// screenshots, made-up ones.
pub trait Reads {
    /// A number that's another once the save has been written again (its time on disk as it is now); 0 when that
    /// can't be told, and the song is then read again for every search.
    fn written(&mut self, save: &str) -> i64;
    /// The song, read from its save as it plays (`mo_midi::playing::read`). Err: why not, in plain words.
    fn song(&mut self, save: &str) -> Result<Song, String>;
    /// The index's slots for every part, for the first look (`MidiStore::slots`). Err: why not, in plain words.
    fn slots(&mut self) -> Result<SlotBook, String>;
    /// One part's notes, by its row in the index and its notes' fingerprint; none when they can't be read.
    fn notes(&mut self, id: i64, hash: &str) -> Option<Vec<NoteEvent>>;
}

/// What a search is asked.
pub struct Asked<'a> {
    pub song: &'a SongChoice,
    /// What the page's switch asks for; none while it hasn't been touched ([`fitted`]).
    pub wanted: Option<Wants>,
    /// The page's catalogue. What a search keeps is for this very one: another (the index read again) has it made
    /// again.
    pub cat: &'a Arc<Catalogue>,
    /// The page's filters, with Settings' smallest part.
    pub filters: &'a Filters,
    /// The page's count of the readings of the MIDI that made slots and no new catalogue: the slots kept are read
    /// again when it's another.
    pub slots_made: u64,
}

/// What a search came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// The list: what was looked for, what of the song it was fitted to ([`fitted_text`]) and the parts found.
    Fits { wants: Wants, to: String, looked: Looked },
    /// Nothing could be fitted, and why, in plain words.
    Nothing(String),
    /// Stopped for a newer search: nothing to show.
    Stopped,
}

/// What the list's place says instead of a list that has no lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Empty {
    pub title: &'static str,
    /// Why, in plain words.
    pub text: String,
    /// Whether fewer filters could give the list lines, so "Clear the filters" is worth offering (when any is on):
    /// not when the song or the index is why.
    pub filters: bool,
}

/// The title in the list's place when the search couldn't be made at all (the song, the index).
pub const COULDNT_LOOK: &str = "Couldn't look for parts that fit";

/// What the list's place says instead of a list that has no lines. None when there's a list (or the search was
/// stopped).
pub fn empty_words(answer: &Answer) -> Option<Empty> {
    match answer {
        Answer::Nothing(why) => Some(Empty { title: COULDNT_LOOK, text: why.clone(), filters: false }),
        Answer::Fits { looked, .. } if looked.fits.is_empty() => Some(Empty {
            title: "No parts fit",
            text: result_text(looked),
            // Parts the index has no slots for yet aren't the filters' doing.
            filters: looked.looked > 0 || looked.not_ready == 0,
        }),
        Answer::Fits { .. } | Answer::Stopped => None,
    }
}

/// What one search made that the next can use again: the song as read, what of it is fitted to, its own parts in the
/// catalogue and the index's slots. The page keeps one on its search's thread.
#[derive(Default)]
pub struct Kept {
    song: Option<KeptSong>,
    /// The song's own parts: in which catalogue, and for which saves.
    own: Option<(Weak<Catalogue>, Vec<String>, Own)>,
    /// The index's slots: read for which catalogue, at which count of slots made.
    book: Option<(Weak<Catalogue>, u64, SlotBook)>,
}

impl Kept {
    /// Whether nothing is kept: no search has read anything since it was made.
    pub fn is_empty(&self) -> bool {
        self.song.is_none() && self.own.is_none() && self.book.is_none()
    }
}

struct KeptSong {
    save: String,
    written: i64,
    song: Song,
    /// What of it is fitted to, for what was asked last.
    fitted: Option<(Option<Wants>, Result<Fitted, String>)>,
}

/// One search, start to end: the parts of the catalogue that go best with the song asked for. Reads the song when
/// it isn't the one kept or its save has been written since, the slots and the song's own parts when the catalogue
/// is another than the one they were made for (the slots also when a reading made some: `asked.slots_made`), then
/// tries every part the filters let through ([`tried`], [`look`]). `stop` set ends it early. The first look and the
/// judging run on `pool`; call it off the window's thread.
pub fn search(
    kept: &mut Kept,
    reads: &mut dyn Reads,
    asked: &Asked,
    pool: &rayon::ThreadPool,
    stop: &AtomicBool,
) -> Answer {
    let stopped = || stop.load(Ordering::Relaxed);
    let save = &asked.song.save;
    let written = reads.written(save);
    if !kept.song.as_ref().is_some_and(|k| written != 0 && k.written == written && k.save == *save) {
        kept.song = None;
        if stopped() {
            return Answer::Stopped;
        }
        match reads.song(save) {
            Ok(song) => kept.song = Some(KeptSong { save: save.clone(), written, song, fitted: None }),
            // Not kept: a save the DAW was writing just then reads the next time.
            Err(why) => return Answer::Nothing(format!("{} couldn't be read. {why}", asked.song.name)),
        }
    }
    let Some(song) = kept.song.as_mut() else { return Answer::Stopped };
    if song.fitted.as_ref().is_none_or(|(wanted, _)| *wanted != asked.wanted) {
        song.fitted = Some((asked.wanted, fitted(&song.song, asked.wanted)));
    }
    let fitted = match &song.fitted {
        Some((_, Ok(fitted))) => fitted,
        Some((_, Err(why))) => return Answer::Nothing(why.clone()),
        None => return Answer::Stopped,
    };
    // The very catalogue asked about, not one like it: a kept weak hold lets no other take its place in memory, so
    // the same place is the same catalogue.
    let cat = Arc::downgrade(asked.cat);
    if kept.book.as_ref().is_none_or(|(of, made, _)| !of.ptr_eq(&cat) || *made != asked.slots_made) {
        kept.book = None;
        if stopped() {
            return Answer::Stopped;
        }
        match reads.slots() {
            Ok(book) => kept.book = Some((cat.clone(), asked.slots_made, book)),
            Err(why) => return Answer::Nothing(format!("The MIDI index couldn't be read. {why}")),
        }
    }
    if kept.own.as_ref().is_none_or(|(of, saves, _)| !of.ptr_eq(&cat) || *saves != asked.song.saves) {
        kept.own = None;
        if stopped() {
            return Answer::Stopped;
        }
        kept.own = Some((cat, asked.song.saves.clone(), asked.song.own(asked.cat)));
    }
    let (Some((_, _, book)), Some((_, _, own))) = (&kept.book, &kept.own) else { return Answer::Stopped };
    let tried = tried(asked.cat, own, asked.filters);
    let looked = look(fitted, &tried, book, &mut |id, hash| reads.notes(id, hash), pool, stop);
    if looked.stopped {
        return Answer::Stopped;
    }
    Answer::Fits { wants: fitted.wants, to: fitted_text(&song.song, fitted), looked }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::midi::tests::phrase;
    use mo_formats::NoteAttribute;
    use mo_formats::arrangement::{Arrangement, LaneKind, PreviewClip, PreviewLane, PreviewNote};
    use mo_midi::fit::slots_of;
    use mo_midi::{BAR, PPQ, StoredPhrase};

    fn note(start: i64, length: i64, key: u8) -> NoteEvent {
        NoteEvent {
            start,
            length,
            key,
            velocity: 40_000,
            muted: false,
            step: false,
            attribute: NoteAttribute::default(),
        }
    }

    /// Am7, Dm7, G7, Cmaj7, a bar each.
    fn chords() -> Vec<NoteEvent> {
        [[57, 60, 64, 67], [50, 60, 65, 69], [55, 59, 62, 65], [48, 59, 64, 67]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| keys.iter().map(move |&k| note(bar * BAR, BAR, k)))
            .collect()
    }

    /// A tune on those chords' thirds, sevenths and roots, moved up by `up` semitones.
    fn tune(up: u8) -> Vec<NoteEvent> {
        [[72, 76, 79], [77, 72, 74], [71, 77, 74], [76, 71, 72]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| {
                [
                    note(bar * BAR, PPQ, keys[0] + up),
                    note(bar * BAR + PPQ, PPQ, keys[1] + up),
                    note(bar * BAR + 2 * PPQ, 2 * PPQ, keys[2] + up),
                ]
            })
            .collect()
    }

    /// The same rhythm climbing by half steps: whichever way it's moved, most of it is off the chords.
    fn clashing() -> Vec<NoteEvent> {
        tune(0).into_iter().zip(60u8..).map(|(n, key)| NoteEvent { key, ..n }).collect()
    }

    fn lane(name: &str, at_bar: i64, notes: &[NoteEvent]) -> PreviewLane {
        let beats = |ticks: i64| ticks as f64 / PPQ as f64;
        let notes: Vec<PreviewNote> = notes
            .iter()
            .map(|n| PreviewNote {
                start: beats(n.start),
                length: beats(n.length),
                key: n.key,
                velocity: n.velocity,
                ..PreviewNote::default()
            })
            .collect();
        let clip =
            PreviewClip { name: name.into(), start: beats(at_bar * BAR), length: 16.0, notes, ..Default::default() };
        PreviewLane { clips: vec![clip], ..PreviewLane::new(name, LaneKind::Instrument) }
    }

    fn song_of(lanes: Vec<PreviewLane>) -> Song {
        playing::of(&Arrangement {
            view: "Test",
            tempo: 120.0,
            beats_per_bar: 4,
            lanes,
            markers: Vec::new(),
            notes: Vec::new(),
        })
    }

    /// Chords from bar 9, and a lead over them from bar 13.
    fn song() -> Song {
        song_of(vec![lane("Keys", 8, &chords()), lane("Lead", 12, &tune(0))])
    }

    /// A made-up index: its parts, their slots and their notes.
    #[derive(Default)]
    struct Index {
        phrases: Vec<StoredPhrase>,
        book: SlotBook,
        notes: HashMap<String, Vec<NoteEvent>>,
    }

    impl Index {
        fn add(&mut self, hash: &str, path: &str, role: Role, notes: Vec<NoteEvent>) {
            self.phrases.push(StoredPhrase {
                id: self.phrases.len() as i64,
                notes: notes.len() as i32,
                lowest: notes.iter().map(|n| n.key).min().unwrap_or(0),
                highest: notes.iter().map(|n| n.key).max().unwrap_or(0),
                shape: mo_midi::analyser::shape_of(&notes),
                cut: Some(false),
                ..phrase(path, hash, role, hash, 1_760_000_000)
            });
            self.book.add(hash, &slots_of(&notes));
            self.notes.insert(hash.into(), notes);
        }

        fn look(&self, fitted: &Fitted, tried: &Tried) -> Looked {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
            let mut notes = |_: i64, hash: &str| self.notes.get(hash).cloned();
            look(fitted, tried, &self.book, &mut notes, &pool, &AtomicBool::new(false))
        }
    }

    const SAVE: &str = "C:/Songs/Didums/Didums_70.flp";

    /// The song as the page offers it: its saves are what the song library says they are.
    fn choice() -> SongChoice {
        SongChoice {
            name: "Didums".into(),
            label: "Didums".into(),
            save: SAVE.into(),
            mtime: 1_760_000_000,
            saves: vec![SAVE.into()],
        }
    }

    /// The song's own chords and lead, a pack's tune that fits a tone up (and a two-note scrap of it), one that
    /// clashes, and a bassline.
    fn library() -> Index {
        let mut index = Index::default();
        index.add("song chords", SAVE, Role::Chords, chords());
        index.add("song lead", SAVE, Role::Melody, tune(0));
        let mut fits: Vec<NoteEvent> = tune(2).into_iter().map(|n| NoteEvent { key: n.key - 12, ..n }).collect();
        fits.truncate(10);
        index.add("fits a tone down", "D:/Packs/Leads/Lead 01.mid", Role::Melody, fits.clone());
        index.add("scrap", "D:/Packs/Leads/Scrap.mid", Role::Melody, fits[..2].to_vec());
        index.add("clashes", "D:/Packs/Leads/Lead 02.mid", Role::Melody, clashing());
        let bass: Vec<NoteEvent> =
            [33, 38, 31, 36].iter().zip(0i64..).map(|(&k, bar)| note(bar * BAR, BAR, k)).collect();
        index.add("bassline", "D:/Packs/Bass/Bass 01.mid", Role::Bass, bass);
        index
    }

    #[test]
    fn a_song_is_fitted_to_by_its_chords_or_its_melody() {
        let song = song();
        // Nothing asked: parts to play over its chords, where they play.
        let over = fitted(&song, None).unwrap();
        assert_eq!((over.wants, over.pick.parts.clone(), over.pick.bars.clone()), (Wants::Tune, vec![0], 8..12));
        assert_eq!(fitted_text(&song, &over), "To play over Keys, bars 9–12");
        assert_eq!(fitted(&song, Some(Wants::Tune)).unwrap(), over);
        // Asked for chords: fitted to the lead.
        let under = fitted(&song, Some(Wants::Chords)).unwrap();
        assert_eq!((under.wants, under.pick.parts.clone(), under.pick.bars.clone()), (Wants::Chords, vec![1], 12..16));
        assert_eq!(fitted_text(&song, &under), "Chords for Lead, bars 13–16");

        // A song with a melody alone: chords for it when nothing's asked, and why not when a tune is.
        let lead_only = song_of(vec![lane("Lead", 0, &tune(0))]);
        assert_eq!(fitted(&lead_only, None).unwrap().wants, Wants::Chords);
        assert_eq!(
            fitted(&lead_only, Some(Wants::Tune)).unwrap_err(),
            "No chords or bassline found in this song to play over. It has a melody: look for chords for that."
        );
        // And the other way round.
        let keys_only = song_of(vec![lane("Keys", 0, &chords())]);
        assert_eq!(fitted(&keys_only, None).unwrap().wants, Wants::Tune);
        assert_eq!(
            fitted(&keys_only, Some(Wants::Chords)).unwrap_err(),
            "No melody found in this song to put chords under. It has chords: look for parts to play over them."
        );
        // A song of samples alone: a sampler's notes aren't pitches the project names, so nothing is fitted to them
        // unless they're picked by hand.
        let mut samples = keys_only.clone();
        samples.parts[0].sample = true;
        assert_eq!(
            fitted(&samples, None).unwrap_err(),
            "No chords, bassline or melody found in this song (drums and samples aside)."
        );
        // No notes at all.
        assert_eq!(
            fitted(&song_of(Vec::new()), None).unwrap_err(),
            "This song has no notes to fit to yet (drums aside)."
        );
    }

    #[test]
    fn chords_that_cant_be_read_fall_back_to_the_melody_and_say_which_couldnt() {
        // However it comes about that a part's notes give nothing to read (here they're all muted, which a song
        // read from a save never has): the chords are there, but no target can be made of them.
        let mut song = song();
        song.parts[0].notes.iter_mut().for_each(|n| n.muted = true);
        // Nothing asked: chords for the melody, which can be read.
        let under = fitted(&song, None).unwrap();
        assert_eq!((under.wants, under.pick.parts.clone()), (Wants::Chords, vec![1]));
        // Asked to play over the chords: why not, and what there is.
        assert_eq!(
            fitted(&song, Some(Wants::Tune)).unwrap_err(),
            "No chords could be read in bars 9–12 of this song. It has a melody: look for chords for that."
        );
        // The melody unreadable too: the words say it's the melody when chords for it were asked for, and don't
        // point to the other way, which wouldn't work either.
        song.parts[1].notes.iter_mut().for_each(|n| n.muted = true);
        assert_eq!(
            fitted(&song, Some(Wants::Chords)).unwrap_err(),
            "The melody couldn't be read in bars 13–16 of this song."
        );
        assert_eq!(fitted(&song, Some(Wants::Tune)).unwrap_err(), "No chords could be read in bars 9–12 of this song.");
        assert_eq!(fitted(&song, None).unwrap_err(), "No chords could be read in bars 9–12 of this song.");
        // Only the melody unreadable: the chords are pointed to.
        song.parts[0].notes.iter_mut().for_each(|n| n.muted = false);
        assert_eq!(
            fitted(&song, Some(Wants::Chords)).unwrap_err(),
            "The melody couldn't be read in bars 13–16 of this song. It has chords: look for parts to play over them."
        );
    }

    #[test]
    fn the_words_for_bars_parts_and_moves() {
        assert_eq!((bars_words(&(0..1)), bars_words(&(8..16))), ("bar 1".to_owned(), "bars 9–16".to_owned()));
        let named = |names: &[&str]| {
            let song = song_of(names.iter().map(|n| lane(n, 0, &chords())).collect());
            part_names(&song, &Pick { parts: (0..names.len()).collect(), bars: 0..4 })
        };
        assert_eq!(named(&["Keys"]), "Keys");
        assert_eq!(named(&["Keys", "Sub"]), "Keys and Sub");
        assert_eq!(named(&["Keys", "Pad", "Sub"]), "Keys, Pad and Sub");
        assert_eq!(named(&["Keys", "Pad", "Sub", "Arp", "Pluck"]), "Keys, Pad and 3 more");
        // Two channels of one name are named once.
        assert_eq!(named(&["Keys", "Keys", "Sub", "Keys"]), "Keys and Sub");
        assert_eq!(
            [0, 1, -1, 5, -6].map(move_words),
            ["as it is", "1 semitone up", "1 semitone down", "5 semitones up", "6 semitones down"]
        );
        let fit = |semitones, sits| Fit { entry: 0, semitones, sits };
        assert_eq!(fit_text(&fit(0, 0.917)), "Fits 92% as it is");
        assert_eq!(fit_text(&fit(-2, 0.88)), "Fits 88%, 2 semitones down");
        // The score isn't held to 0 to 1; the words are.
        assert_eq!((fit(0, 1.07).percent(), fit(0, -0.2).percent()), (100, 0));
        assert_eq!(
            (wants_words(Wants::Tune), wants_words(Wants::Chords)),
            ("To play over its chords", "Chords for its melody")
        );
    }

    #[test]
    fn the_list_is_the_parts_that_fit_best_without_the_songs_own_or_what_the_filters_hide() {
        let index = library();
        let cat = Catalogue::new(index.phrases.clone());
        let song = song();
        let over = fitted(&song, None).unwrap();
        let own = choice().own(&cat);
        assert_eq!(own.len(), 2);
        let place = |hash: &str| cat.find(hash).unwrap();

        // Settings' smallest part hides the scrap, so it isn't tried; nor are the song's own two.
        let filters = Filters { smallest: 3, ..Filters::default() };
        let all = tried(&cat, &own, &filters);
        assert_eq!(all.len(), 3);
        let looked = index.look(&over, &all);
        assert_eq!((looked.tried, looked.looked, looked.close, looked.stopped), (3, 3, 3, false));
        // The tune that fits comes first, with the move that makes it fit; the one that clashes sits well below.
        assert_eq!(looked.fits.len(), 3);
        let best = looked.fits[0];
        assert_eq!((best.entry, best.semitones), (place("fits a tone down"), -2));
        assert!(best.percent() > 80, "{best:?}");
        let clashes = looked.fits.iter().find(|f| f.entry == place("clashes")).unwrap();
        assert!(clashes.sits < best.sits - 0.3, "{clashes:?}");
        assert_eq!(result_text(&looked), "3 parts, best fit first");
        assert_eq!(left_out_text(&looked), "");
        assert!(log_line(&looked).starts_with("tried 3, first look 3, notes read 3 in "), "{}", log_line(&looked));

        // A kind chip narrows what's tried, as it narrows the page's own list.
        let basslines = Filters { smallest: 3, roles: vec![Role::Bass], ..Filters::default() };
        let looked = index.look(&over, &tried(&cat, &own, &basslines));
        assert_eq!(looked.fits.iter().map(|f| f.entry).collect::<Vec<_>>(), [place("bassline")]);
        assert_eq!(result_text(&looked), "1 part, best fit first");
        // And so do the words searched for.
        let words = Filters { text: "lead 02".into(), ..Filters::default() };
        let looked = index.look(&over, &tried(&cat, &own, &words));
        assert_eq!(looked.fits.iter().map(|f| f.entry).collect::<Vec<_>>(), [place("clashes")]);
        // And the years picked on the year strip (the made-up files are all of 2025).
        let years = |from, to| Filters { years: Some((from, to)), ..Filters::default() };
        assert_eq!((tried(&cat, &own, &years(2025, 2025)).len(), tried(&cat, &own, &years(2001, 2024)).len()), (4, 0));

        // Chords for the song's lead: the library here has none but the song's own.
        let under = fitted(&song, Some(Wants::Chords)).unwrap();
        let looked = index.look(&under, &all);
        assert!(looked.fits.is_empty());
        assert_eq!(result_text(&looked), "No parts to try: the filters leave none of the right kind.");
    }

    #[test]
    fn what_a_search_left_out_is_said() {
        let index = library();
        let cat = Catalogue::new(index.phrases.clone());
        let song = song();
        let over = fitted(&song, None).unwrap();
        let filters = Filters::default();
        let all = tried(&cat, &Own::default(), &filters);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();

        // An index read before slots were kept has none: nothing can be fitted until it's read again.
        let mut notes = |_: i64, hash: &str| index.notes.get(hash).cloned();
        let none = look(&over, &all, &SlotBook::default(), &mut notes, &pool, &AtomicBool::new(false));
        assert_eq!((none.looked, none.not_ready), (0, 6));
        assert_eq!(
            result_text(&none),
            "Nothing can be fitted yet: read your MIDI again first (Read MIDI again), then it can."
        );
        assert_eq!(left_out_text(&none), "");

        // Some without slots, and one whose notes can't be read: both said after the list's line.
        let mut book = SlotBook::default();
        for hash in ["song chords", "song lead", "fits a tone down", "clashes"] {
            book.add(hash, &slots_of(&index.notes[hash]));
        }
        let mut notes = |_: i64, hash: &str| index.notes.get(hash).filter(|_| hash != "clashes").cloned();
        let some = look(&over, &all, &book, &mut notes, &pool, &AtomicBool::new(false));
        assert_eq!((some.looked, some.not_ready, some.unread, some.fits.len()), (4, 2, 1, 3));
        // Fewer lines than parts looked at doesn't mean worse fits were hidden: no "of 4" unless the list was cut.
        assert_eq!(result_text(&some), "3 parts, best fit first");
        assert_eq!(
            left_out_text(&some),
            "2 parts left out until your MIDI is read again · 1 part left out (notes couldn't be read)"
        );

        // Stopped for a newer search: said in the log, and flagged so the page doesn't show it.
        let mut notes = |_: i64, hash: &str| index.notes.get(hash).cloned();
        let stopped = look(&over, &all, &index.book, &mut notes, &pool, &AtomicBool::new(true));
        assert!(stopped.stopped && stopped.fits.is_empty());
        assert!(log_line(&stopped).ends_with("(stopped for a newer search)"));

        // Nothing fits: every part tried hardly plays where the chords do.
        let nothing = Looked { looked: 1_234, ..Looked::default() };
        assert_eq!(result_text(&nothing), "None of 1,234 parts fits.");
        // A list cut to its length says of how many.
        let fit = Fit { entry: 0, semitones: 0, sits: 1.0 };
        let cut = Looked { fits: vec![fit; MOST], looked: 212_345, ..Looked::default() };
        assert_eq!(result_text(&cut), "The 200 that fit best, of 212,345 parts tried");
    }

    #[test]
    fn the_songs_offered_are_the_ones_saved_last_each_with_all_its_saves() {
        let save = |path: &str, mtime: i64| mo_library::Save {
            path: path.to_owned(),
            name: mo_formats::paths::stem(mo_formats::paths::file_name(path)).to_owned(),
            daw: "FL Studio".to_owned(),
            folder: "C:\\Songs".to_owned(),
            mtime,
            ..mo_library::Save::default()
        };
        let now = 1_800_000_000;
        let saves = vec![
            save("C:\\Songs\\Night drive_12.flp", now - 500),
            save("C:\\Songs\\Night drive_11.flp", now - 100),
            save("C:\\Songs\\Dawn.flp", now - 300),
        ];
        let dates = RealDates::at(now);
        let songs = mo_library::songs::group_songs(&saves, &mo_library::SongChoices::default(), &dates);
        let offered = song_choices(&songs, &dates);
        // The song saved last first, read from the save written last, with every save it has.
        assert_eq!(offered.len(), 2);
        assert_eq!((offered[0].save.as_str(), offered[0].mtime), ("C:\\Songs\\Night drive_11.flp", now - 100));
        let mut all = offered[0].saves.clone();
        all.sort();
        assert_eq!(all, ["C:\\Songs\\Night drive_11.flp", "C:\\Songs\\Night drive_12.flp"]);
        assert_eq!(
            (offered[1].name.as_str(), offered[1].saves.as_slice()),
            ("Dawn", ["C:\\Songs\\Dawn.flp".to_owned()].as_slice())
        );
        // No songs, nothing to offer.
        assert!(song_choices(&[], &RealDates::now()).is_empty());
        // Each is called by its name in the menu.
        assert_eq!(offered.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["Night drive", "Dawn"]);

        // Two songs of one name are told apart by their folders, and by a number where those are the same too.
        // (Each in a folder of its own is a song of its own.)
        let in_own_folder = |path: &str, mtime: i64| mo_library::Save {
            folder: path.rsplit_once('\\').map(|(folder, _)| folder.to_owned()).unwrap_or_default(),
            ..save(path, mtime)
        };
        let saves = vec![
            in_own_folder("C:\\Songs\\Beats\\Idea.flp", now - 100),
            in_own_folder("D:\\Old\\Beats\\Idea.flp", now - 200),
            in_own_folder("C:\\Songs\\Trap\\Idea.flp", now - 300),
            in_own_folder("C:\\Songs\\Dawn.flp", now - 400),
        ];
        let songs = mo_library::songs::group_songs(&saves, &mo_library::SongChoices::default(), &dates);
        let offered = song_choices(&songs, &dates);
        let labels: Vec<&str> = offered.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Idea · Beats", "Idea · Beats (2)", "Idea · Trap", "Dawn"]);

        // A song is found again among the songs offered later by its saves, not by what the menu calls it. Dawn gets
        // a new save and a namesake in another folder: its label is another, and it's still the song.
        let dawn = offered[3].clone();
        let mut later = saves.clone();
        later.push(in_own_folder("C:\\Songs\\Dawn 2.flp", now - 50));
        later.push(in_own_folder("D:\\Old\\Dawn.flp", now - 60));
        let choices = mo_library::SongChoices::default();
        let songs = mo_library::songs::group_songs(&later, &choices, &dates);
        let offered = song_choices(&songs, &dates);
        let found = dawn.among(&offered).unwrap();
        assert_eq!((found.label.as_str(), found.save.as_str()), ("Dawn · Songs", "C:\\Songs\\Dawn 2.flp"));
        assert_ne!(found.label, dawn.label);
        // Its namesake isn't it, though the label Dawn had would now be read as either.
        let namesake = offered.iter().find(|c| c.save == "D:\\Old\\Dawn.flp").unwrap();
        assert_eq!(namesake.label, "Dawn · Old");
        assert_eq!(namesake.among(std::slice::from_ref(&dawn)), None);
        // The save it was read from gone (deleted, or split off by hand): the song is the one with its other saves.
        let was = SongChoice { save: "C:\\Songs\\Dawn 3.flp".into(), ..found.clone() };
        assert_eq!(was.among(&offered), Some(found));
        // A song that isn't offered any more isn't found (it's the first offered now, with the save written last).
        assert_eq!((offered[0].save.as_str(), dawn.among(&offered[1..])), ("C:\\Songs\\Dawn 2.flp", None));
    }

    /// A made-up song and index to search, counting what's read.
    struct Fake<'a> {
        index: &'a Index,
        song: Result<Song, String>,
        written: i64,
        songs_read: usize,
        slots_read: usize,
    }

    impl<'a> Fake<'a> {
        fn new(index: &'a Index) -> Fake<'a> {
            Fake { index, song: Ok(song()), written: 7, songs_read: 0, slots_read: 0 }
        }
    }

    impl Reads for Fake<'_> {
        fn written(&mut self, _: &str) -> i64 {
            self.written
        }

        fn song(&mut self, _: &str) -> Result<Song, String> {
            self.songs_read += 1;
            self.song.clone()
        }

        fn slots(&mut self) -> Result<SlotBook, String> {
            self.slots_read += 1;
            let mut book = SlotBook::default();
            for (hash, notes) in &self.index.notes {
                book.add(hash, &slots_of(notes));
            }
            Ok(book)
        }

        fn notes(&mut self, _: i64, hash: &str) -> Option<Vec<NoteEvent>> {
            self.index.notes.get(hash).cloned()
        }
    }

    #[test]
    fn a_search_keeps_the_song_its_own_parts_and_the_slots_until_they_change() {
        let index = library();
        let cat = Arc::new(Catalogue::new(index.phrases.clone()));
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let filters = Filters { smallest: 3, ..Filters::default() };
        let song = choice();
        let go = AtomicBool::new(false);
        let mut kept = Kept::default();
        assert!(kept.is_empty());
        let mut fake = Fake::new(&index);
        let ask = |wanted, slots_made| Asked { song: &song, wanted, cat: &cat, filters: &filters, slots_made };
        let entries = |answer: &Answer| match answer {
            Answer::Fits { looked, .. } => looked.fits.iter().map(|f| f.entry).collect::<Vec<_>>(),
            other => panic!("no list: {other:?}"),
        };
        let place = |hash: &str| cat.find(hash).unwrap();

        // The first search reads the song and the slots, and gives the list with what it was fitted to.
        let first = search(&mut kept, &mut fake, &ask(None, 1), &pool, &go);
        let Answer::Fits { wants, to, looked } = &first else { panic!("{first:?}") };
        assert_eq!((*wants, to.as_str()), (Wants::Tune, "To play over Keys, bars 9–12"));
        assert_eq!((looked.tried, looked.fits[0].entry, looked.fits[0].semitones), (3, place("fits a tone down"), -2));
        assert_eq!(page_line(to, looked), "To play over Keys, bars 9–12 · 3 parts, best fit first");
        assert_eq!(empty_words(&first), None);
        assert_eq!((fake.songs_read, fake.slots_read), (1, 1));
        assert!(!kept.is_empty());

        // The same again, and the other way: nothing is read again.
        assert_eq!(entries(&search(&mut kept, &mut fake, &ask(None, 1), &pool, &go)), entries(&first));
        let under = search(&mut kept, &mut fake, &ask(Some(Wants::Chords), 1), &pool, &go);
        let Answer::Fits { wants, to, looked } = &under else { panic!("{under:?}") };
        assert_eq!((*wants, to.as_str(), looked.fits.len()), (Wants::Chords, "Chords for Lead, bars 13–16", 0));
        // No lines: the list's place says why, and so does the line under the toolbar.
        let none = "No parts to try: the filters leave none of the right kind.";
        assert_eq!(empty_words(&under), Some(Empty { title: "No parts fit", text: none.to_owned(), filters: true }));
        assert_eq!(page_line(to, looked), format!("Chords for Lead, bars 13–16 · {none}"));
        assert_eq!((fake.songs_read, fake.slots_read), (1, 1));

        // The save written again: the song is read again, the slots aren't.
        fake.written = 8;
        search(&mut kept, &mut fake, &ask(None, 1), &pool, &go);
        assert_eq!((fake.songs_read, fake.slots_read), (2, 1));
        // A reading of the MIDI made slots and no new catalogue: the slots are read again.
        search(&mut kept, &mut fake, &ask(None, 2), &pool, &go);
        assert_eq!((fake.songs_read, fake.slots_read), (2, 2));
        // The index read again: another catalogue, whatever the page counts. Its slots are read again, and the
        // song's own parts are found again: here every part is at another place, and the song's own are still the
        // ones left out.
        let turned = Arc::new(Catalogue::new(index.phrases.iter().rev().cloned().collect()));
        assert_ne!(turned.find("song lead"), cat.find("song lead"));
        let again = search(&mut kept, &mut fake, &Asked { cat: &turned, ..ask(None, 2) }, &pool, &go);
        assert_eq!((fake.songs_read, fake.slots_read), (2, 3));
        let mut found: Vec<&str> =
            entries(&again).into_iter().map(|at| turned.entries[at].phrase.hash.as_str()).collect();
        found.sort_unstable();
        assert_eq!(found, ["bassline", "clashes", "fits a tone down"]);
        // And back to the first catalogue: made again for it, not taken from what was kept for the other.
        let back = search(&mut kept, &mut fake, &ask(None, 2), &pool, &go);
        assert_eq!((fake.songs_read, fake.slots_read), (2, 4));
        assert_eq!(entries(&back), entries(&first));
        // Another song's saves: the own parts are that song's. A song with none of these saves leaves nothing out,
        // so the first song's lead is offered too.
        let other = SongChoice { saves: vec!["C:/Songs/Other/Other.flp".into()], ..choice() };
        let with_own = search(&mut kept, &mut fake, &Asked { song: &other, ..ask(None, 2) }, &pool, &go);
        assert!(entries(&with_own).contains(&place("song lead")), "{with_own:?}");
        assert!(!entries(&search(&mut kept, &mut fake, &ask(None, 2), &pool, &go)).contains(&place("song lead")));
        // Neither read the slots again: they're the index's, whichever the song.
        assert_eq!(fake.slots_read, 4);

        // Another save with the same time on disk is another song to read.
        let before = fake.songs_read;
        let elsewhere = SongChoice { save: "C:/Songs/Didums/Didums_71.flp".into(), ..choice() };
        search(&mut kept, &mut fake, &Asked { song: &elsewhere, ..ask(None, 2) }, &pool, &go);
        assert_eq!(fake.songs_read, before + 1);
        // A save whose time can't be told is read for every search.
        fake.written = 0;
        let before = fake.songs_read;
        search(&mut kept, &mut fake, &ask(None, 2), &pool, &go);
        search(&mut kept, &mut fake, &ask(None, 2), &pool, &go);
        assert_eq!(fake.songs_read, before + 2);
    }

    #[test]
    fn a_search_says_why_when_theres_nothing_to_fit_and_stops_when_told() {
        let index = library();
        let cat = Arc::new(Catalogue::new(index.phrases.clone()));
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let filters = Filters::default();
        let song = choice();
        let go = AtomicBool::new(false);
        let ask = |wanted| Asked { song: &song, wanted, cat: &cat, filters: &filters, slots_made: 0 };

        // The save can't be read: said with the song's name, and tried again the next time (the DAW may have been
        // writing it).
        let mut kept = Kept::default();
        let mut fake = Fake::new(&index);
        fake.song = Err("It couldn't be opened, maybe because another program had it open.".into());
        let unread = search(&mut kept, &mut fake, &ask(None), &pool, &go);
        let why = "Didums couldn't be read. It couldn't be opened, maybe because another program had it open.";
        assert_eq!(unread, Answer::Nothing(why.to_owned()));
        // The filters have nothing to do with it: clearing them isn't offered.
        assert_eq!(empty_words(&unread), Some(Empty { title: COULDNT_LOOK, text: why.to_owned(), filters: false }));
        assert_eq!((fake.songs_read, fake.slots_read), (1, 0));
        assert!(kept.is_empty());
        fake.song = Ok(song_of(vec![lane("Lead", 0, &tune(0))]));
        // Read now. It has a melody alone: asked for parts to play over its chords, the answer is why not.
        let no_chords = search(&mut kept, &mut fake, &ask(Some(Wants::Tune)), &pool, &go);
        assert_eq!(
            no_chords,
            Answer::Nothing(
                "No chords or bassline found in this song to play over. It has a melody: look for chords for that."
                    .to_owned()
            )
        );
        assert_eq!((fake.songs_read, fake.slots_read), (2, 0));

        // Told to stop before it starts: nothing is read, and there's nothing to show.
        let mut kept = Kept::default();
        let mut fake = Fake::new(&index);
        let stopped = search(&mut kept, &mut fake, &ask(None), &pool, &AtomicBool::new(true));
        assert_eq!((stopped.clone(), empty_words(&stopped)), (Answer::Stopped, None));
        assert_eq!((fake.songs_read, fake.slots_read), (0, 0));
        // Stopped with everything kept already: the search itself stops, and still nothing shows.
        search(&mut kept, &mut fake, &ask(None), &pool, &go);
        assert_eq!(search(&mut kept, &mut fake, &ask(None), &pool, &AtomicBool::new(true)), Answer::Stopped);

        // No lines: fewer filters could give some, unless what's missing is the index's slots (the MIDI has to be
        // read again first, whatever the filters).
        let fits = |looked| Answer::Fits { wants: Wants::Tune, to: String::new(), looked };
        let words = |looked| empty_words(&fits(looked)).map(|e| (e.title, e.text, e.filters));
        assert_eq!(
            words(Looked { looked: 12, not_ready: 3, ..Looked::default() }),
            Some(("No parts fit", "None of 12 parts fits.".to_owned(), true))
        );
        let read_again = "Nothing can be fitted yet: read your MIDI again first (Read MIDI again), then it can.";
        assert_eq!(
            words(Looked { not_ready: 3, ..Looked::default() }),
            Some(("No parts fit", read_again.to_owned(), false))
        );
    }
}
