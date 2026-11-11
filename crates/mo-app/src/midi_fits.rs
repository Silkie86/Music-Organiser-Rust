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
//!
//! A part picked in the list is the part as it fits: [`moved`] gives its notes moved by what its line says, which is
//! what the page then plays, drags out and sends to a DAW ([`moved_name`] names it so there), and [`card_text`] says
//! so on its card. The move is two things added up ([`Fit::moved`]): the semitones that put the part in the song's
//! key, and the octaves that put it where its kind of part plays beside the song (the search's,
//! `mo_midi::fit::octave`). The octave is a first guess: the card's Octave up and Octave down change it
//! ([`Fit::octave_step`]), and what you chose is remembered for those notes ([`YourOctaves`], and the index's
//! `your_octaves` for the next time), wherever they're fitted again.
//!
//! A part found can be heard with the song ([`SongBars`]): the parts it was fitted to, over the bars it was fitted
//! to, go round beside it at the song's tempo, as the two were judged and as the part will sound dropped on the first
//! of those bars in the DAW. [`with_song_line`] says so on the card.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use mo_formats::NoteEvent;
use mo_formats::dates::RealDates;
use mo_library::songs::thousands;
use mo_midi::fit::for_song::{Own, Pick, Tried};
use mo_midi::fit::octave::{self, MOST_OCTAVES};
use mo_midi::fit::{Found, Target, Wants};
use mo_midi::playing::{self, Song};
use mo_midi::store::SlotBook;
use mo_midi::{Catalogue, Role, StoredPhrase};

use crate::midi::{self as page, Filters};

/// The most lines the list shows: the parts that fit best.
pub const MOST: usize = 200;

/// How many of the songs saved last the page offers to fit to.
pub const SONGS_OFFERED: usize = 8;

/// Said under the toolbar while a search runs.
pub const LOOKING: &str = "Looking for parts that fit…";

/// The setting for the page's "Play with the song" chip: "0" when you've turned it off; on until then.
pub const WITH_SONG_KEY: &str = "midi_fits_with_song";

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
    /// The same parts and bars to hear a part found with.
    pub with: Arc<SongBars>,
}

/// What of a song a part found is heard with (the page's "Play with the song"): the parts the list is fitted to, over
/// the bars it's fitted to, as the song has them. They go round as picked, every bar of them (one the parts rest in
/// too), and the part found goes round beside them by its own length from the first of those bars
/// ([`SongBars::round`]): as the part sounds dropped on that bar in the DAW. The search judges the two going round
/// by the same rule over the same clip ([`mo_midi::analyser::clip_length`]), so what's heard here is what was judged
/// (`docs/harmony/together-experiments.md`, "What's judged is what's dropped").
#[derive(Clone, Debug, PartialEq)]
pub struct SongBars {
    /// What plays: each picked part that sounds in the bars (never drums), in the order they're picked, which is the
    /// order the page's line names them in ([`fitted_text`]).
    pub voices: Vec<SongVoice>,
    /// How long the bars are, and one bar of the song, in ticks ([`mo_midi::PPQ`] to a beat).
    pub length: i64,
    pub bar: i64,
    /// The song's tempo, when its save gives one a part can play at.
    pub tempo: Option<f64>,
    /// What was looked for (parts over these, or chords under them), the names of the parts that play ("Keys and
    /// Sub"; "" when none has a name) and the bars, counted from 0: for the card's words ([`with_song_line`]).
    pub wants: Wants,
    pub names: String,
    pub bars: std::ops::Range<i64>,
}

/// One of a song's parts in the bars a list is fitted to ([`SongBars`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SongVoice {
    pub name: String,
    /// The General MIDI sound it's heard with, matched as a library part's is: by its name, the instrument its name
    /// or its plugin's says, then its kind.
    pub sound: mo_midi::gm::GmSound,
    /// What it plays in the bars, the first of them at tick 0 ([`Song::stretch`]).
    pub notes: Vec<NoteEvent>,
}

impl SongBars {
    /// The picked parts of `song` over the picked bars, as [`Target::of_song`] takes them: each part once, in the
    /// order they're picked, no drums, and none that rests through the bars (so the card names what's heard, which
    /// is the page's line less any part that rests).
    pub fn of(song: &Song, pick: &Pick, wants: Wants) -> SongBars {
        let mut places: Vec<usize> = Vec::with_capacity(pick.parts.len());
        for &at in &pick.parts {
            if !places.contains(&at) {
                places.push(at);
            }
        }
        let voices: Vec<SongVoice> = places
            .into_iter()
            .filter_map(|at| {
                let part = song.parts.get(at).filter(|p| !p.drums && p.role != Role::Drums)?;
                let notes = song.stretch(at, &pick.bars);
                if notes.is_empty() {
                    return None;
                }
                // The part is pitched: a name that says drums (an "808" bassline's) doesn't put it on the drum kit.
                let family = mo_midi::instruments::guess([part.name.as_str(), part.plugin.as_str()], false, None)
                    .map(|guess| guess.family)
                    .filter(|family| mo_midi::gm::for_family(*family).is_some());
                let sound = mo_midi::gm::sound_for(part.role, false, None, &[&part.name], family);
                Some(SongVoice { name: part.name.clone(), sound, notes })
            })
            .collect();
        SongBars {
            length: pick.bars.end.saturating_sub(pick.bars.start).max(0).saturating_mul(song.bar()),
            bar: song.bar(),
            tempo: page::usable_tempo(Some(song.tempo)),
            wants,
            names: names_words(voices.iter().map(|v| v.name.as_str())),
            bars: pick.bars.clone(),
            voices,
        }
    }

    /// The tempo the two play at: the song's, else the one a part with none plays at ([`page::PLAY_TEMPO`]).
    pub fn bpm(&self) -> f64 {
        self.tempo.unwrap_or(page::PLAY_TEMPO)
    }

    /// How long a part whose loop is `part` ticks plays with these bars before both are back at their starts, in
    /// ticks: until they meet again, or once through the longer when that's past 32 bars (the rule
    /// `mo_harmony::sit` hears two loops by, [`mo_harmony::round_of`]). `most` is the longest that can be made into
    /// sound (`mo_audio::gm::longest_ticks`: two minutes, which a slow song's 32 bars are over). What's longer than
    /// that is held to as many times through the longer of the two as fit, or to as many whole bars when not even
    /// one does: where it goes round is always a bar line, so the beat never jumps.
    pub fn round(&self, part: i64, most: i64) -> i64 {
        let (part, own, bar) = (part.max(1), self.length.max(1), self.bar.max(1));
        let round = mo_harmony::round_of(part, own, bar);
        if round <= most {
            return round;
        }
        let longer = part.max(own);
        match most / longer {
            turns if turns >= 1 => turns.saturating_mul(longer),
            _ => (most / bar).max(1).saturating_mul(bar),
        }
    }
}

/// The card's sound line for a part found that plays with the song: its sound, what of `song` it's heard with, and
/// whose tempo they play at. "Saw Lead · for its kind of part · with Keys and Sub from Night drive (bars
/// 9–16), at the song's 128 BPM".
pub fn with_song_line(sound: &mo_midi::gm::GmSound, song: &str, with: &SongBars) -> String {
    let bars = bars_words(&with.bars);
    let heard = match (with.names.is_empty(), with.wants) {
        (false, _) => format!("with {} from {song} ({bars})", with.names),
        (true, Wants::Tune) => format!("with {song}'s chords ({bars})"),
        (true, Wants::Chords) => format!("with {song}'s melody ({bars})"),
    };
    let bpm = (with.bpm() * 10.0).round() / 10.0;
    let tempo = match with.tempo {
        Some(_) => format!("at the song's {bpm} BPM"),
        None => format!("at {bpm} BPM, as the song gives no tempo"),
    };
    format!("{} · {heard}, {tempo}", page::sound_line(sound, None))
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
        let with = Arc::new(SongBars::of(song, &pick, target.wants()));
        Some(Fitted { wants: target.wants(), pick, target, with })
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
    names_words(pick.parts.iter().filter_map(|&at| song.parts.get(at)).map(|p| p.name.as_str()))
}

/// Names in a row, as [`part_names`] says them.
fn names_words<'a>(all: impl Iterator<Item = &'a str>) -> String {
    // A name once: two channels called "Keys" (a layered sound) are "Keys".
    let mut names: Vec<&str> = Vec::new();
    for name in all {
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
    /// How far to move it into the song's key, in semitones (-6 to 5; 0: it's in key as it is). The part moves, never
    /// the song.
    pub semitones: i32,
    /// How many octaves the app would move it as well, to where its kind of part plays beside the bars of the song
    /// it's fitted to (-3 to 3; up for more than 0; `mo_midi::fit::octave`): a bassline saved two octaves up comes
    /// down.
    pub octaves: i32,
    /// The octaves you chose for its notes, from how they're saved, when you have ([`YourOctaves`]): they go in place
    /// of the app's ([`Fit::octaves_now`]).
    pub yours: Option<i32>,
    /// The lowest and the highest key it plays as it's saved, strays aside (`octave::edges`): no octave takes them off
    /// the keyboard.
    pub edges: (u8, u8),
    /// How it sits moved so: about 0 (it clashes) to about 1 (as a tune written for those chords does).
    pub sits: f64,
}

impl Fit {
    fn of(found: &Found) -> Fit {
        let (semitones, sits) = found.sits.best();
        Fit { entry: found.at, semitones, octaves: found.octaves, yours: None, edges: found.edges, sits }
    }

    /// The octaves it's moved by: the ones you chose for its notes when you have, else the app's. Yours are held as
    /// the app's are: 3 either way at the most, and never so many that the part, in the song's key, leaves the
    /// keyboard (an octave chosen beside a song in another key may not fit beside this one).
    pub fn octaves_now(&self) -> i32 {
        match self.yours {
            Some(yours) => octave::on_keyboard(self.edges, self.semitones, yours.clamp(-MOST_OCTAVES, MOST_OCTAVES)),
            None => self.octaves,
        }
    }

    /// How far it's moved in all, in semitones, up for more than 0: into the key and to the octave. What its line
    /// says ([`move_words`]) and what [`moved`] does to its notes.
    pub fn moved(&self) -> i32 {
        self.semitones + 12 * self.octaves_now()
    }

    /// The octaves it would be moved by one further up (`by` more than 0) or down: what's then remembered as yours.
    /// None when it can't go: it's 3 that way already, or the part would leave the keyboard.
    pub fn octave_step(&self, by: i32) -> Option<i32> {
        let wanted = self.octaves_now() + by.signum();
        let stays = octave::on_keyboard(self.edges, self.semitones, wanted) == wanted;
        (wanted.abs() <= MOST_OCTAVES && stays).then_some(wanted)
    }

    /// How it sits, for a meter and for words: 0 to 100.
    pub fn percent(&self) -> u32 {
        (self.sits.clamp(0.0, 1.0) * 100.0).round() as u32
    }
}

/// How a part is moved to fit, all of it in one go (`semitones`: [`Fit::moved`]), so it can be done by hand as said
/// and never points two ways: "as it is", "1 semitone up", "9 semitones down" (3 up for the key and an octave down),
/// "an octave up", "2 octaves down", "an octave and 3 semitones up".
pub fn move_words(semitones: i32) -> String {
    let way = if semitones > 0 { "up" } else { "down" };
    let (octaves, rest) = (semitones.unsigned_abs() / 12, semitones.unsigned_abs() % 12);
    let octaves = match octaves {
        0 => None,
        1 => Some("an octave".to_owned()),
        n => Some(format!("{n} octaves")),
    };
    let rest = match rest {
        0 => None,
        1 => Some("1 semitone".to_owned()),
        n => Some(format!("{n} semitones")),
    };
    match (octaves, rest) {
        (None, None) => "as it is".to_owned(),
        (Some(far), None) | (None, Some(far)) => format!("{far} {way}"),
        (Some(octaves), Some(rest)) => format!("{octaves} and {rest} {way}"),
    }
}

/// Said first in a line's details: "Fits 92% as it is", "Fits 88%, 2 semitones up", "Fits 71%, an octave down".
pub fn fit_text(fit: &Fit) -> String {
    let comma = if fit.moved() == 0 { "" } else { "," };
    format!("Fits {}%{comma} {}", fit.percent(), move_words(fit.moved()))
}

/// Said on the card of a part moved by `semitones` in all ([`Fit::moved`]) to fit `song`: how it's moved, that the
/// octave is one you chose when it is (`yours`: the card then offers the app's back), and that what's done with it is
/// moved so. "Moved 3 semitones up to fit Night drive: it plays, drags and goes to your DAW that way."
pub fn card_text(semitones: i32, yours: bool, song: &str) -> String {
    let chosen = if yours { ", at the octave you chose" } else { "" };
    if semitones == 0 {
        return format!("Fits {song} as it is{chosen}: nothing is moved.");
    }
    format!("Moved {} to fit {song}{chosen}: it plays, drags and goes to your DAW that way.", move_words(semitones))
}

/// The octaves you've chosen for sets of notes since the catalogue was read, by the notes' fingerprint: they go over
/// what the catalogue was read with (`StoredPhrase::your_octave`, the index's `your_octaves`), so a choice shows at
/// once and holds however long the index takes to write it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct YourOctaves(HashMap<String, Option<i32>>);

impl YourOctaves {
    /// You chose `octaves` for the notes with this fingerprint (from how they're saved); none: the app picks again.
    pub fn set(&mut self, hash: &str, octaves: Option<i32>) {
        self.0.insert(hash.to_owned(), octaves);
    }

    /// The octaves you chose for a part's notes: here, else as the catalogue has them; none when you haven't.
    pub fn of(&self, phrase: &StoredPhrase) -> Option<i32> {
        self.0.get(&phrase.hash).copied().unwrap_or(phrase.your_octave)
    }
}

/// Gives each line the octave you chose for its notes ([`Fit::yours`]), and takes it from a line whose notes are back
/// at the app's pick: after a search, and after each change of octave. Never a drum part's: its notes pick pads, so
/// an octave kept for the same notes (chosen while they counted as another kind, or in the old app) doesn't move it.
pub fn with_yours(fits: &mut [Fit], cat: &Catalogue, yours: &YourOctaves) {
    for fit in fits {
        let pitched = cat.entry(fit.entry).filter(|e| e.role() != Role::Drums);
        fit.yours = pitched.and_then(|e| yours.of(&e.phrase));
    }
}

/// What a part is called where it goes moved by `semitones` in all (the file dragged out, the clip in the DAW): "Arp
/// clip (3 semitones up)", so it isn't taken for the part as it is in the library. Its own name when it isn't moved.
pub fn moved_name(name: &str, semitones: i32) -> String {
    if semitones == 0 { name.to_owned() } else { format!("{name} ({})", move_words(semitones)) }
}

/// A part's notes moved by `semitones` in all ([`Fit::moved`]), as its line in the list says it fits: every note by
/// exactly that (`NoteEvent::moved`), so what's heard is what the line and the name say. A note that would go past
/// the lowest or the highest key there is has nowhere to go and is left out. The octave is never one that takes the
/// part itself off the keyboard (`Registers::octaves_for`), so that's a stray far from the rest of it (a key switch),
/// or a note within 6 keys of the keyboard's ends moved for the key.
pub fn moved(notes: Vec<NoteEvent>, semitones: i32) -> Vec<NoteEvent> {
    if semitones == 0 {
        return notes;
    }
    notes.into_iter().filter_map(|n| n.moved(semitones)).collect()
}

/// A key moved with its part (`semitones`: all of the move; whole octaves leave a key as it is): the key a chart of
/// the moved notes is spelled in.
pub fn moved_key(key: mo_harmony::Key, semitones: i32) -> mo_harmony::Key {
    mo_harmony::Key { tonic: mo_harmony::PitchClass::new(i32::from(key.tonic.number()) + semitones), ..key }
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
    /// The list: what was looked for, what of the song it was fitted to ([`fitted_text`]), the parts found, and
    /// what a part found is heard with.
    Fits { wants: Wants, to: String, looked: Looked, with: Arc<SongBars> },
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
    Answer::Fits { wants: fitted.wants, to: fitted_text(&song.song, fitted), looked, with: Arc::clone(&fitted.with) }
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
    fn a_part_found_is_heard_with_the_parts_and_bars_its_list_is_fitted_to() {
        let starts = |notes: &[NoteEvent]| notes.iter().map(|n| (n.start, n.length, n.key)).collect::<Vec<_>>();
        // What the list is fitted to is what plays: the keys from bar 9, moved to start at tick 0.
        let song = song();
        let over = fitted(&song, None).unwrap();
        let with = &over.with;
        assert_eq!((with.voices.len(), with.voices[0].name.as_str()), (1, "Keys"));
        assert_eq!(starts(&with.voices[0].notes), starts(&chords()));
        assert_eq!((with.length, with.bar, with.tempo, with.bpm()), (4 * BAR, BAR, Some(120.0), 120.0));
        assert_eq!((with.wants, with.names.as_str(), with.bars.clone()), (Wants::Tune, "Keys", 8..12));
        // The other way: the lead, from bar 13.
        let under = fitted(&song, Some(Wants::Chords)).unwrap();
        assert_eq!((under.with.voices.len(), under.with.bars.clone(), under.with.wants), (1, 12..16, Wants::Chords));
        assert_eq!(starts(&under.with.voices[0].notes), starts(&tune(0)));

        // Each picked part once, in the order they're picked (as the page's line names them); never drums, nor a
        // part that rests through the bars. A bassline called "808" is still a bass, not the drum kit.
        let bass: Vec<NoteEvent> =
            [45u8, 38, 43, 36].iter().zip(0i64..).map(|(&k, bar)| note(bar * BAR, BAR, k)).collect();
        let kick: Vec<NoteEvent> = (0..16).map(|beat| note(beat * PPQ, PPQ / 2, 36)).collect();
        let mut song = song_of(vec![
            lane("Keys", 8, &chords()),
            lane("808 sub", 8, &bass),
            lane("Pad", 0, &chords()),
            lane("Kit", 8, &kick),
        ]);
        song.parts[3].role = Role::Drums;
        let pick = Pick { parts: vec![3, 1, 2, 0, 1], bars: 8..12 };
        let with = SongBars::of(&song, &pick, Wants::Tune);
        let names: Vec<&str> = with.voices.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["808 sub", "Keys"]);
        assert_eq!(starts(&with.voices[0].notes), starts(&bass));
        assert!(
            with.voices.iter().all(|v| !v.sound.drums),
            "{:?}",
            with.voices.iter().map(|v| &v.sound).collect::<Vec<_>>()
        );
        assert_eq!(with.voices[0].sound.name(), "Synth Bass 1");
        assert_eq!(with.names, "808 sub and Keys");
        // The page's line names the same parts in the same order, and the one that rests there too.
        assert_eq!(part_names(&song, &Pick { parts: vec![1, 2, 0], bars: 8..12 }), "808 sub, Pad and Keys");
        assert_eq!(
            SongBars::of(&song, &Pick { parts: vec![1, 2, 0], bars: 8..12 }, Wants::Tune).names,
            "808 sub and Keys"
        );
        // A song whose save gives no tempo a part can play at: the one a part with none plays at.
        song.tempo = 0.0;
        let slow = SongBars::of(&song, &pick, Wants::Tune);
        assert_eq!((slow.tempo, slow.bpm()), (None, page::PLAY_TEMPO));
        // No bars: nothing plays, and nothing has a length.
        let none = SongBars::of(&song, &Pick { parts: vec![0], bars: 8..8 }, Wants::Tune);
        assert_eq!((none.voices.len(), none.length), (0, 0));
    }

    #[test]
    fn a_part_and_the_songs_bars_play_until_they_meet_again() {
        let with = fitted(&song(), None).unwrap().with;
        // Two minutes at the song's 120 beats a minute: 60 bars, more than is ever asked for.
        let most = 60 * BAR;
        // Four bars of the song: a part as long plays once, a shorter one until both are back at their starts.
        assert_eq!(with.round(4 * BAR, most), 4 * BAR);
        assert_eq!(with.round(2 * BAR, most), 4 * BAR);
        assert_eq!(with.round(8 * BAR, most), 8 * BAR);
        assert_eq!(with.round(3 * BAR, most), 12 * BAR);
        assert_eq!(with.round(7 * BAR, most), 28 * BAR);
        // Past 32 bars they'd take too long to meet: once through the longer.
        assert_eq!(with.round(9 * BAR, most), 9 * BAR);
        // As the matcher hears them, whatever the numbers.
        assert_eq!(with.round(5 * BAR, most), mo_harmony::round_of(5 * BAR, 4 * BAR, BAR));
        assert_eq!(with.round(0, most), 4 * BAR);
        let empty = SongBars { length: 0, bar: 0, ..(*with).clone() };
        assert_eq!(empty.round(0, most), 1);
    }

    /// A slow song: two minutes of sound is the most there is, and what's played stops on a bar line before it.
    #[test]
    fn in_a_slow_song_a_part_and_the_songs_bars_stop_on_a_bar_line_within_two_minutes() {
        let with = fitted(&song(), None).unwrap().with;
        // At 56 beats a minute two minutes are 28 bars: a round of 28 still fits.
        assert_eq!(with.round(7 * BAR, 28 * BAR), 28 * BAR);
        // A bar less room (or a tick less): as many times through the longer of the two as fit. The seven-bar part
        // three times; a three-bar part's twelve bars don't fit in eleven, so the song's four twice.
        assert_eq!(with.round(7 * BAR, 28 * BAR - 1), 21 * BAR);
        assert_eq!(with.round(3 * BAR, 11 * BAR), 8 * BAR);
        // Not even once through the longer: whole bars, as many as fit, and one at the least.
        assert_eq!(with.round(16 * BAR, 10 * BAR + 5), 10 * BAR);
        assert_eq!(with.round(2 * BAR, 3 * BAR + BAR / 2), 3 * BAR);
        assert_eq!(with.round(2 * BAR, BAR / 2), BAR);
    }

    #[test]
    fn the_card_says_what_a_part_plays_with_and_at_whose_tempo() {
        let sound = mo_midi::gm::sound_for(Role::Melody, false, None, &[], None);
        let with = fitted(&song(), None).unwrap().with;
        assert_eq!(
            with_song_line(&sound, "Night drive", &with),
            "Saw Lead · for its kind of part · with Keys from Night drive (bars 9–12), at the song's 120 BPM"
        );
        // Parts with no names: what they are to the song. A tempo that isn't a whole number keeps a tenth.
        let unnamed = SongBars { names: String::new(), tempo: Some(127.96), ..(*with).clone() };
        assert_eq!(
            with_song_line(&sound, "Night drive", &unnamed),
            "Saw Lead · for its kind of part · with Night drive's chords (bars 9–12), at the song's 128 BPM"
        );
        let under = SongBars { wants: Wants::Chords, bars: 12..13, tempo: None, ..unnamed };
        assert_eq!(
            with_song_line(&sound, "Night drive", &under),
            "Saw Lead · for its kind of part · with Night drive's melody (bar 13), at 120 BPM, as the song \
             gives no tempo"
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
        // With octaves in it the move is still said in one go, one way.
        assert_eq!(
            [12, -24, 15, -13, -9, 11, 36].map(move_words),
            [
                "an octave up",
                "2 octaves down",
                "an octave and 3 semitones up",
                "an octave and 1 semitone down",
                "9 semitones down",
                "11 semitones up",
                "3 octaves up"
            ]
        );
        let fit = |semitones, sits| Fit { entry: 0, semitones, octaves: 0, yours: None, edges: (60, 72), sits };
        assert_eq!(fit_text(&fit(0, 0.917)), "Fits 92% as it is");
        assert_eq!(fit_text(&fit(-2, 0.88)), "Fits 88%, 2 semitones down");
        // The key's semitones and the octaves add up: 3 up and an octave down is 9 down.
        let low = Fit { octaves: -1, ..fit(3, 0.71) };
        assert_eq!((low.moved(), fit_text(&low)), (-9, "Fits 71%, 9 semitones down".to_owned()));
        let high = Fit { octaves: 1, ..fit(0, 0.6) };
        assert_eq!((high.moved(), fit_text(&high)), (12, "Fits 60%, an octave up".to_owned()));
        assert_eq!(Fit { octaves: 2, ..fit(-6, 0.6) }.moved(), 18);
        // The score isn't held to 0 to 1; the words are.
        assert_eq!((fit(0, 1.07).percent(), fit(0, -0.2).percent()), (100, 0));
        assert_eq!(
            (wants_words(Wants::Tune), wants_words(Wants::Chords)),
            ("To play over its chords", "Chords for its melody")
        );
    }

    #[test]
    fn a_part_is_moved_by_what_its_line_says() {
        let keys = |notes: &[NoteEvent]| notes.iter().map(|n| n.key).collect::<Vec<_>>();
        let part = vec![note(0, PPQ, 60), note(PPQ, PPQ, 64), note(2 * PPQ, 2 * PPQ, 67)];
        // As it is: the same notes.
        assert_eq!(moved(part.clone(), 0), part);
        // Up and down: every note by as much, and nothing else about it changed.
        let down = moved(part.clone(), -2);
        assert_eq!(keys(&down), [58, 62, 65]);
        assert_eq!(down[2], NoteEvent { key: 65, ..part[2] });
        assert_eq!(keys(&moved(part.clone(), 5)), [65, 69, 72]);
        // Muted notes and steps move like the rest (what plays is decided where it's played).
        let muted = NoteEvent { muted: true, step: true, ..part[0] };
        assert_eq!(moved(vec![muted], 1), [NoteEvent { key: 61, ..muted }]);
        // A note at the keyboard's end with nowhere to go is left out; the rest move by what the line says, never
        // by anything else.
        let ends = vec![note(0, PPQ, 1), note(PPQ, PPQ, 60), note(2 * PPQ, PPQ, 125)];
        assert_eq!(keys(&moved(ends.clone(), 3)), [4, 63]);
        assert_eq!(keys(&moved(ends.clone(), 2)), [3, 62, 127], "the top key itself is a key");
        assert_eq!(keys(&moved(ends, -2)), [58, 123]);

        // Its key moves with it, round the octave, major or minor as it was.
        use mo_harmony::{Key, PitchClass};
        assert_eq!(moved_key(Key::major(PitchClass::D), 3), Key::major(PitchClass::F));
        assert_eq!(moved_key(Key::minor(PitchClass::C), -2), Key::minor(PitchClass::new(10)));
        assert_eq!(moved_key(Key::minor(PitchClass::C), 0), Key::minor(PitchClass::C));

        // What the card and the DAW say about it.
        assert_eq!(
            card_text(3, false, "Night drive"),
            "Moved 3 semitones up to fit Night drive: it plays, drags and goes to your DAW that way."
        );
        assert_eq!(
            card_text(-1, false, "Glass"),
            "Moved 1 semitone down to fit Glass: it plays, drags and goes to your DAW that way."
        );
        assert_eq!(card_text(0, false, "Glass"), "Fits Glass as it is: nothing is moved.");
        // At an octave you chose, the card says whose it is.
        assert_eq!(
            card_text(-9, true, "Glass"),
            "Moved 9 semitones down to fit Glass, at the octave you chose: it plays, drags and goes to your DAW that way."
        );
        assert_eq!(card_text(0, true, "Glass"), "Fits Glass as it is, at the octave you chose: nothing is moved.");
        assert_eq!(moved_name("Arp clip", 3), "Arp clip (3 semitones up)");
        assert_eq!(moved_name("Arp clip", 0), "Arp clip");
        // Moved to another octave as well, it's all said in one.
        assert_eq!(
            card_text(-9, false, "Glass"),
            "Moved 9 semitones down to fit Glass: it plays, drags and goes to your DAW that way."
        );
        assert_eq!(moved_name("Sub line", -12), "Sub line (an octave down)");
        assert_eq!(keys(&moved(part, -21)), [39, 43, 46]);
    }

    #[test]
    fn a_part_found_goes_at_the_octave_its_kind_plays_at() {
        // A pack's bassline on the chords' roots, in a bass's usual place: A2, D2, G2, C3.
        let roots = |keys: [u8; 4]| -> Vec<NoteEvent> {
            keys.iter().zip(0i64..).map(|(&k, bar)| note(bar * BAR, BAR, k)).collect()
        };
        let mut index = library();
        index.add("mid bass", "D:/Packs/Bass/Bass 02.mid", Role::Bass, roots([45, 38, 43, 48]));
        // And a tune 3 semitones up from the key and an octave down: its middle is 66.
        let low: Vec<NoteEvent> = tune(3).into_iter().map(|n| NoteEvent { key: n.key - 12, ..n }).collect();
        index.add("low tune", "D:/Packs/Leads/Lead 03.mid", Role::Melody, low);
        let cat = Catalogue::new(index.phrases.clone());
        let place = |hash: &str| cat.find(hash).unwrap();
        let filters = Filters::default();
        let fits = |song: &Song| {
            let over = fitted(song, Some(Wants::Tune)).unwrap();
            let looked = index.look(&over, &tried(&cat, &choice().own(&cat), &filters));
            move |hash: &str| *looked.fits.iter().find(|f| f.entry == place(hash)).unwrap()
        };

        // A tune belongs by a tune's usual place (72), wherever the song's own lead is. The pack's tune is saved an
        // octave under that (its middle is 65, and 63 moved into the key): it goes up, 2 semitones down for the key
        // and an octave up.
        let song = song();
        let fit = fits(&song);
        let registers = *fitted(&song, None).unwrap().target.registers();
        assert_eq!((registers.of_kind(Role::Melody), registers.wanted(Role::Melody)), (Some(75.0), Some(72.0)));
        let tune = fit("fits a tone down");
        assert_eq!((tune.semitones, tune.octaves, tune.moved()), (-2, 1, 10));
        assert_eq!(fit_text(&tune), format!("Fits {}%, 10 semitones up", tune.percent()));
        // The song has no bass: the basslines are by a bass's usual place already (40), and stay.
        assert_eq!((fit("bassline").octaves, fit("mid bass").octaves), (0, 0));

        // The same chords over a sub bass an octave under that (its middle is 31): a bassline now belongs at 35.5.
        // The pack's low one is there (34.5); the other (43) comes down an octave, to play where the sub does.
        let with_sub = song_of(vec![lane("Keys", 8, &chords()), lane("Sub", 8, &roots([33, 26, 31, 36]))]);
        let over = fitted(&with_sub, Some(Wants::Tune)).unwrap();
        assert_eq!((over.pick.parts.clone(), over.target.registers().of_kind(Role::Bass)), (vec![0, 1], Some(31.0)));
        let fit = fits(&with_sub);
        assert_eq!(fit("bassline").octaves, 0);
        let mid = fit("mid bass");
        assert_eq!((mid.semitones, mid.octaves, mid.moved()), (0, -1, -12));
        // A line comes with its part's lowest and highest key, for the octave buttons: back up is what's remembered.
        assert_eq!(
            (mid.edges, mid.yours, mid.octave_step(1), mid.octave_step(-1)),
            ((38, 48), None, Some(0), Some(-2))
        );
        // A tune found goes up here too.
        assert_eq!(fit("fits a tone down").octaves, 1);
        // The octave goes by where the part is once it's in key: as saved, the low tune is just within half an
        // octave of 72 (66); 3 semitones down for the key it's at 63, and goes up.
        let low = fit("low tune");
        assert_eq!((low.semitones, low.octaves, low.moved()), (-3, 1, 9));
    }

    #[test]
    fn an_octave_you_chose_goes_in_place_of_the_apps() {
        // A bassline saved from C3 to C4 that the app brings down an octave, 3 semitones up for the key.
        let fit = Fit { entry: 0, semitones: 3, octaves: -1, yours: None, edges: (48, 60), sits: 0.8 };
        assert_eq!((fit.octaves_now(), fit.moved()), (-1, -9));
        // A step is from where it is now, whoever put it there, and is what's then remembered.
        assert_eq!((fit.octave_step(1), fit.octave_step(-1)), (Some(0), Some(-2)));
        assert_eq!((fit.octave_step(5), fit.octave_step(-5)), (Some(0), Some(-2)), "a step is one octave");
        let yours = Fit { yours: Some(0), ..fit };
        assert_eq!((yours.octaves_now(), yours.moved(), fit_text(&yours).as_str()), (0, 3, "Fits 80%, 3 semitones up"));
        assert_eq!((yours.octave_step(1), yours.octave_step(-1)), (Some(1), Some(-1)));
        // 3 octaves either way at the most: from there it only comes back.
        let top = Fit { yours: Some(3), ..fit };
        assert_eq!((top.octaves_now(), top.moved(), top.octave_step(1), top.octave_step(-1)), (3, 39, None, Some(2)));
        let bottom = Fit { yours: Some(-3), ..fit };
        assert_eq!((bottom.octaves_now(), bottom.octave_step(-1), bottom.octave_step(1)), (-3, None, Some(-2)));
        // One chosen in the old app beyond that is held to 3.
        assert_eq!(
            (Fit { yours: Some(7), ..fit }.octaves_now(), Fit { yours: Some(-7), ..fit }.octaves_now()),
            (3, -3)
        );

        // Never off the keyboard. A part whose lowest key is 10 is at 7 once it's 3 semitones down for the key: it
        // has no octave under it to go to, and can go up.
        let low = Fit { semitones: -3, octaves: 0, edges: (10, 40), ..fit };
        assert_eq!((low.octave_step(-1), low.octave_step(1)), (None, Some(1)));
        // The same at the top: its highest key, 2 up for the key, is 5 short of the last one.
        let high = Fit { semitones: 2, octaves: 0, edges: (90, 120), ..fit };
        assert_eq!((high.octave_step(1), high.octave_step(-1)), (None, Some(-1)));
        // An octave chosen beside a song in another key (2 down) that has no room beside this one goes as far as
        // there are keys: none here, one for a part that starts on key 14.
        let chosen = Fit { yours: Some(-2), ..low };
        assert_eq!((chosen.octaves_now(), chosen.moved()), (0, -3));
        let wide = Fit { semitones: 0, edges: (14, 100), yours: Some(-3), ..fit };
        assert_eq!((wide.octaves_now(), wide.octave_step(-1), wide.octave_step(1)), (-1, None, Some(0)));
    }

    #[test]
    fn the_octave_you_chose_is_the_newest_said_else_the_one_the_catalogue_was_read_with() {
        let mut index = library();
        // The index has an octave for one of the parts (chosen before, here or in the old app).
        let before = index.phrases.iter_mut().find(|p| p.hash == "bassline").unwrap();
        before.your_octave = Some(-1);
        let cat = Catalogue::new(index.phrases.clone());
        let place = |hash: &str| cat.find(hash).unwrap();
        let line =
            |hash: &str| Fit { entry: place(hash), semitones: 0, octaves: 1, yours: None, edges: (60, 72), sits: 1.0 };
        let mut fits = vec![line("bassline"), line("fits a tone down"), Fit { yours: Some(2), ..line("clashes") }];
        let octaves =
            |fits: &[Fit]| -> Vec<(Option<i32>, i32)> { fits.iter().map(|f| (f.yours, f.octaves_now())).collect() };

        // Nothing said since: the index's choice for the first, the app's pick for the others (a choice a line was
        // given before and that's no longer anyone's is taken from it).
        let mut said = YourOctaves::default();
        with_yours(&mut fits, &cat, &said);
        assert_eq!(octaves(&fits), [(Some(-1), -1), (None, 1), (None, 1)]);
        // What's said since goes over it: another octave for the first, one for the second.
        said.set("bassline", Some(2));
        said.set("fits a tone down", Some(0));
        with_yours(&mut fits, &cat, &said);
        assert_eq!(octaves(&fits), [(Some(2), 2), (Some(0), 0), (None, 1)]);
        // Back to the app's pick, which the index's older choice doesn't undo.
        said.set("bassline", None);
        with_yours(&mut fits, &cat, &said);
        assert_eq!(octaves(&fits), [(None, 1), (Some(0), 0), (None, 1)]);
        // A line whose part the catalogue no longer has is left with the app's.
        let mut gone = [Fit { entry: 9_999, yours: Some(1), ..line("clashes") }];
        with_yours(&mut gone, &cat, &said);
        assert_eq!(gone[0].yours, None);

        // Drums are never moved by octaves (their notes pick pads): an octave kept for the same notes, from when
        // they counted as another kind or from the old app, isn't theirs, whoever said it and when.
        let drums = index.phrases.iter_mut().find(|p| p.hash == "scrap").unwrap();
        (drums.your_role, drums.your_octave) = (Some(Role::Drums), Some(2));
        let cat = Catalogue::new(index.phrases.clone());
        let scrap = cat.find("scrap").unwrap();
        assert_eq!(cat.entry(scrap).map(|e| e.role()), Some(Role::Drums));
        let mut beat = [Fit { entry: scrap, semitones: 0, octaves: 0, yours: Some(1), edges: (36, 46), sits: 1.0 }];
        with_yours(&mut beat, &cat, &said);
        assert_eq!((beat[0].yours, beat[0].moved()), (None, 0));
        said.set("scrap", Some(-1));
        with_yours(&mut beat, &cat, &said);
        assert_eq!((beat[0].yours, beat[0].moved()), (None, 0));
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
        assert_eq!(best.moved(), best.semitones + 12 * best.octaves);
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
        let fit = Fit { entry: 0, semitones: 0, octaves: 0, yours: None, edges: (60, 72), sits: 1.0 };
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
        let Answer::Fits { wants, to, looked, with } = &first else { panic!("{first:?}") };
        assert_eq!((*wants, to.as_str()), (Wants::Tune, "To play over Keys, bars 9–12"));
        assert_eq!((with.names.as_str(), with.bars.clone(), with.voices.len()), ("Keys", 8..12, 1));
        assert_eq!((looked.tried, looked.fits[0].entry, looked.fits[0].semitones), (3, place("fits a tone down"), -2));
        assert_eq!(page_line(to, looked), "To play over Keys, bars 9–12 · 3 parts, best fit first");
        assert_eq!(empty_words(&first), None);
        assert_eq!((fake.songs_read, fake.slots_read), (1, 1));
        assert!(!kept.is_empty());

        // The same again, and the other way: nothing is read again.
        assert_eq!(entries(&search(&mut kept, &mut fake, &ask(None, 1), &pool, &go)), entries(&first));
        let under = search(&mut kept, &mut fake, &ask(Some(Wants::Chords), 1), &pool, &go);
        let Answer::Fits { wants, to, looked, with: with_tune } = &under else { panic!("{under:?}") };
        assert_eq!((*wants, to.as_str(), looked.fits.len()), (Wants::Chords, "Chords for Lead, bars 13–16", 0));
        assert_eq!(
            (with_tune.names.as_str(), with_tune.bars.clone(), with_tune.wants),
            ("Lead", 12..16, Wants::Chords)
        );
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
        let with = fitted(&super::tests::song(), None).unwrap().with;
        let fits = |looked| Answer::Fits { wants: Wants::Tune, to: String::new(), looked, with: Arc::clone(&with) };
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
