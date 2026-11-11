//! Fitting a part from the library to the song being worked on: a tune over the song's chords, or chords under the
//! song's tune. `mo-harmony` does the judging ([`mo_harmony::sit`]); this says what is fitted to what, and how the two
//! are lined up.
//!
//! What's fitted to is a [`Target`], made once: the picked parts of a song over some of its bars
//! ([`Target::of_song`], from [`crate::playing`]), or any chords or tune ([`Target::chords`], [`Target::tune`]: a part
//! of the library can be the target too). Each part tried against it is then quick ([`Target::fit`]). Measured in a
//! release build on the tests' 8-bar parts: 0.16 ms to make a target from a song's chords and bass, 7 microseconds
//! for a tune over chords, 0.23 ms for chords under a tune (their own chords are read first). The answer says how the
//! found part sits moved up by each of 0 to 11 semitones, so a part saved in another key comes with the move that
//! makes it fit.
//!
//! How the two are lined up: the bars picked from a song go round as picked, every one of them, a bar the picked
//! parts rest in too (the first or the last as well), so the part found is heard as it will sound dropped on the
//! first picked bar. The found part is a loop from the library, and how long that loop is isn't saved with it: it
//! goes round from the bar its first note is in to the bar its last note ends in, so a four-bar loop that rests in
//! its last bar is heard as three bars going round.
//!
//! A song that isn't in 4/4 is fitted to as it is, but the library's parts are all read as 4/4 (the MIDI page's
//! rule), so there the answer says less.
//!
//! Trying the whole library ([`Target::search`]) takes two looks. Every part of the right kind is first sorted by
//! its slots alone (how long it plays each pitch class in every two quarter notes, kept in the index and held in
//! memory: about a microsecond a part), and only the best share of them have their notes read and get the close
//! look: the best 2 in 100 when looking for tunes, 10 in 100 when looking for chords, which keeps 99 and 98 in 100 of
//! what the close look alone would put in its best ten (`docs/harmony/together-experiments.md`).
//!
//! Not here yet: sending the part found into the project.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_formats::NoteEvent;
use mo_harmony::{Backing, ChordContext, Grid, LineUp, Note, Outline, Part, Sits, Sitting, Slot, Span, sit};
use rayon::prelude::*;

use crate::PPQ;
use crate::chart;
use crate::index::StoredPhrase;
use crate::keys::{heard_as, heard_notes};
use crate::playing::Song;
use crate::roles::Role;
use crate::store::SlotBook;

/// How long a first look's slot is ([`mo_harmony::Slot`]), in the index's ticks: two quarter notes, half a bar of
/// 4/4, whatever the song's time signature.
pub const SLOT: i64 = 2 * PPQ;

/// How this crate asks for a part's slots ([`slots_of`]: which of its notes are heard, the slot's length, bars of
/// 4/4): bump it when that changes.
const SLOTS_ASKING: u32 = 1;

/// What made the slots the index keeps of each set of notes: how `mo_harmony` makes slots and how it was asked, like
/// [`crate::keys::VERSION`]. When the app's is another, the index makes them again from the notes it keeps (rule 7).
pub const SLOTS_VERSION: u32 = mo_harmony::SLOTS_VERSION * 100 + SLOTS_ASKING;

/// A library part's slots, for a search's first look: how long it plays each pitch class in every [`SLOT`], from the
/// bar its first note is in to the bar its last note ends in (bars of 4/4, as every part of the library is read), its
/// first 32 bars at the most. The same stretch the close look hears go round ([`Target::fit`]).
pub fn slots_of(notes: &[NoteEvent]) -> Vec<Slot> {
    mo_harmony::slots_of(&heard_notes(notes), Grid::four_four(PPQ as u32), SLOT)
}

/// What's looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wants {
    /// A tune (or a bassline, or an arp: anything played over chords) to go over the target's chords.
    Tune,
    /// Chords to go under the target's tune.
    Chords,
}

/// What a part from the library is fitted to.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    given: Given,
}

#[derive(Clone, Debug, PartialEq)]
enum Given {
    /// Chords, read once.
    Chords(Backing),
    /// A tune, and how it goes round.
    Tune { notes: Vec<Note>, line_up: LineUp },
}

impl Target {
    /// Chords to find a tune for: `parts` are what plays together (the chords, the bass), each with its kind, on
    /// `grid`. `span`: the ticks they're a stretch of, from a bar line to a bar line, when they're cut from a longer
    /// piece (bars picked from a song): the first chord needn't come after the last, a bar the parts rest in is still
    /// one of the bars that go round, and what they play outside it is left out. Without one they're a whole loop,
    /// from the bar of its first note to the bar its last note ends in. None for drums alone, a span with no length,
    /// or when no chord can be read in them.
    pub fn chords(parts: &[(Role, &[NoteEvent])], grid: Grid, span: Option<Range<i64>>) -> Option<Target> {
        let heard: Vec<(Role, Vec<Note>)> = parts
            .iter()
            .filter(|(role, _)| *role != Role::Drums)
            .map(|(role, notes)| (*role, heard_notes(notes)))
            .collect();
        let parts: Vec<Part> = heard
            .iter()
            .filter(|(_, notes)| !notes.is_empty())
            .map(|(role, notes)| Part { role: heard_as(*role), notes })
            .collect();
        if parts.is_empty() {
            return None;
        }
        let backing = chart::with_reader(|reader| match span {
            Some(span) => {
                let length = span.end.checked_sub(span.start).filter(|l| *l > 0)?;
                let reading = reader.read(&parts, grid, &ChordContext::new(Span::Window))?;
                Some(Backing::over(&reading, &parts, span.start, length))
            }
            None => Backing::read(reader, &parts, grid, Span::Loop),
        })?;
        Some(Target { given: Given::Chords(backing) })
    }

    /// A tune to find chords for: its notes, with tick 0 on one of its bar lines. `span`: the ticks it goes round in,
    /// from a bar line to a bar line (bars picked from a song: a bar the tune rests in is still one of them; its
    /// notes lie within it). Without one it goes round from the bar of its first note to the bar its last note ends
    /// in. None without notes, or for a span with no length.
    pub fn tune(notes: &[NoteEvent], span: Option<Range<i64>>) -> Option<Target> {
        let line_up = match span {
            Some(span) => {
                let length = span.end.checked_sub(span.start).filter(|l| *l > 0)?;
                LineUp::LoopsFrom { start: span.start, length }
            }
            None => LineUp::Loops { bar_line: 0 },
        };
        let notes = heard_notes(notes);
        (!notes.is_empty()).then_some(Target { given: Given::Tune { notes, line_up } })
    }

    /// The picked parts of a song (places in [`Song::parts`]) over `bars` of it, counted from 0: a tune to find chords
    /// for when every one of them is a melody, else chords to find a tune for. Drums are left out; a sampler's notes
    /// ([`crate::playing::Playing::sample`]) are heard as written when it's picked. Every picked bar counts, one the
    /// parts rest in too. None when the picked parts play nothing pitched in those bars.
    pub fn of_song(song: &Song, parts: &[usize], bars: &Range<i64>) -> Option<Target> {
        // Each part once, in the song's order, however it was picked.
        let mut places = parts.to_vec();
        places.sort_unstable();
        places.dedup();
        let picked: Vec<(Role, Vec<NoteEvent>)> = places
            .into_iter()
            .filter_map(|at| {
                let part = song.parts.get(at).filter(|p| !p.drums && p.role != Role::Drums)?;
                let notes = song.stretch(at, bars);
                (!notes.is_empty()).then_some((part.role, notes))
            })
            .collect();
        if picked.is_empty() {
            return None;
        }
        // The picked bars in ticks, the first of them at 0 as `Song::stretch` gives their notes.
        let span = Some(0..bars.end.saturating_sub(bars.start).saturating_mul(song.bar()));
        if picked.iter().all(|(role, _)| *role == Role::Melody) {
            let mut tune: Vec<NoteEvent> = picked.into_iter().flat_map(|(_, notes)| notes).collect();
            tune.sort_by_key(|n| (n.start, n.key));
            return Target::tune(&tune, span);
        }
        let parts: Vec<(Role, &[NoteEvent])> = picked.iter().map(|(role, notes)| (*role, notes.as_slice())).collect();
        Target::chords(&parts, song.grid(), span)
    }

    /// What kind of part goes with it.
    pub fn wants(&self) -> Wants {
        match self.given {
            Given::Chords(_) => Wants::Tune,
            Given::Tune { .. } => Wants::Chords,
        }
    }

    /// Whether a part of this kind is worth trying against it: anything but drums over chords; under a tune, a part
    /// that plays chords.
    pub fn tries(&self, role: Role) -> bool {
        match self.given {
            Given::Chords(_) => role != Role::Drums,
            Given::Tune { .. } => matches!(role, Role::Chords | Role::Arp | Role::Other),
        }
    }

    /// How a part from the library (its notes at [`crate::PPQ`] from its own bar 1, its kind, and whether it was `cut`
    /// from a longer stretch) sits with the target, with the part found moved up by each of 0 to 11 semitones:
    /// [`Sits::best`] gives the move that fits best and how it sits there, [`Sits::moved`] how it sits as it is (0)
    /// or moved any other way. None for drums or no notes, when the part found hardly plays where the target has
    /// sound, or, under a tune, when no chords can be read in it.
    pub fn fit(&self, notes: &[NoteEvent], role: Role, cut: bool) -> Option<Sits> {
        if role == Role::Drums {
            return None;
        }
        match &self.given {
            // The part found goes round from its own first bar.
            Given::Chords(backing) => {
                sit(&heard_notes(notes), backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED)
            }
            Given::Tune { notes: tune, line_up } => {
                let backing = chart::backing(notes, role, cut)?;
                let tune_moved = sit(tune, &backing, *line_up, &Sitting::SHIPPED)?;
                // The chords moved up by a number of semitones under the tune sound as the tune moved down by as
                // many over the chords.
                let mut by_move = [0.0; 12];
                for (up, sits) in (0i32..).zip(&mut by_move) {
                    *sits = tune_moved.moved(-up);
                }
                Some(Sits { by_move, heard: tune_moved.heard })
            }
        }
    }
}

/// Of the parts a first look sorts, the share that gets the close look, looking for tunes and for chords. Measured
/// (`theorybench pairs --quick`): they keep 99.1% and 97.8% of the close look's best ten.
const TUNES_KEPT: f64 = 0.02;
const CHORDS_KEPT: f64 = 0.10;

/// However few that is, this many get the close look (all of them, in a small library).
const LEAST_KEPT: usize = 300;

/// Parts whose notes are read and looked at closely at once: a search stopped for a newer one ends after the batch
/// it's on.
const CLOSE_BATCH: usize = 256;

/// A part of the library for a search to try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    /// The part's row and its note hash, to read its notes by.
    pub id: i64,
    pub hash: &'a str,
    /// What kind of part it is: what you said, else the app's guess.
    pub role: Role,
    /// A piece cut from a longer stretch.
    pub cut: bool,
    /// Its notes' shape in any key ([`StoredPhrase::shape`]). A loop a pack has in every key is found once: the copy
    /// that fits best, and of copies that fit alike the one that needs moving least.
    pub shape: Option<i64>,
}

impl<'a> Candidate<'a> {
    pub fn of(part: &'a StoredPhrase) -> Candidate<'a> {
        Candidate {
            id: part.id,
            hash: &part.hash,
            role: if part.drums { Role::Drums } else { part.your_role.unwrap_or(part.role) },
            cut: part.cut.unwrap_or(false),
            shape: part.shape,
        }
    }
}

/// A part a search found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    /// Its place among the candidates given.
    pub at: usize,
    /// How it sits with the target moved up by each of 0 to 11 semitones: [`Sits::best`] gives the move that fits
    /// best and how it sits there.
    pub sits: Sits,
}

impl Found {
    /// What it's sorted by: how it sits at its best move, a part that needs moving less a hair ahead.
    fn judged(&self) -> f64 {
        let (semitones, sits) = self.sits.best();
        sits - 0.0005 * f64::from(semitones.abs())
    }
}

/// What a search came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Search {
    /// The parts that fit best, the best first.
    pub found: Vec<Found>,
    /// How many parts the first look sorted, and how many of them got the close look.
    pub looked: usize,
    pub close: usize,
    /// Parts of the right kind left out because the index has no slots for them yet: it makes them after its next
    /// read.
    pub not_ready: usize,
    /// Stopped before the end, because a newer search was wanted: `found` isn't the whole answer.
    pub stopped: bool,
}

/// What a first look tries each part's slots against.
enum First {
    /// The target's chords, for tunes.
    Chords(Outline),
    /// The target's tune, for chord parts.
    Tune(Vec<Slot>),
}

impl First {
    /// How a part with these slots is likely to sit with the target at its best move.
    fn look(&self, slots: &[Slot]) -> Option<f64> {
        let sits = match self {
            First::Chords(outline) => outline.sits(slots),
            First::Tune(tune) => Outline::guessed(slots, &Sitting::SHIPPED).sits(tune),
        };
        sits.map(|sits| sits.best().1)
    }
}

impl Target {
    fn first(&self) -> First {
        match &self.given {
            Given::Chords(backing) => First::Chords(backing.outline(SLOT, &Sitting::SHIPPED)),
            Given::Tune { notes, line_up: LineUp::LoopsFrom { start, length } } => {
                First::Tune(mo_harmony::slots_over(notes, SLOT, *start, *length))
            }
            Given::Tune { notes, .. } => First::Tune(mo_harmony::slots_of(notes, Grid::four_four(PPQ as u32), SLOT)),
        }
    }

    /// The parts of the library that go best with the target, at most `most` of them.
    ///
    /// `parts` are the library's parts in the order the page lists them (the same notes are tried once, as the
    /// first part given that has them), `book` their slots ([`crate::store::MidiStore::slots`]), and `notes` reads
    /// one part's notes from the index, by its row and note hash ([`crate::store::MidiStore::notes`]): called on
    /// this thread, and only for the share of parts that gets the close look. The first look and the close look run
    /// on `pool`. `stop` set (a newer search is wanted) ends it early.
    ///
    /// Call it off the window's thread, and not from one of `pool`'s own.
    pub fn search(
        &self,
        parts: &[Candidate],
        book: &SlotBook,
        notes: &mut dyn FnMut(i64, &str) -> Option<Vec<NoteEvent>>,
        most: usize,
        pool: &rayon::ThreadPool,
        stop: &AtomicBool,
    ) -> Search {
        let mut search = Search::default();
        let stopped = || stop.load(Ordering::Relaxed);
        // Each set of notes once, of a kind worth trying, with its slots.
        let mut seen = HashSet::new();
        let mut tried: Vec<(usize, &[Slot])> = Vec::new();
        for (at, part) in parts.iter().enumerate() {
            if !self.tries(part.role) || !seen.insert(part.hash) {
                continue;
            }
            match book.of(part.hash) {
                Some(slots) => tried.push((at, slots)),
                None => search.not_ready += 1,
            }
        }
        search.looked = tried.len();

        // The first look: every part by its slots alone.
        let first = self.first();
        let mut looked: Vec<(f64, usize)> = pool.install(|| {
            tried
                .par_iter()
                .filter_map(|&(at, slots)| if stopped() { None } else { Some((first.look(slots)?, at)) })
                .collect()
        });
        if stopped() {
            search.stopped = true;
            return search;
        }
        // The best share of them, the best first, so a search stopped early has looked closely at the likeliest.
        let share = match self.wants() {
            Wants::Tune => TUNES_KEPT,
            Wants::Chords => CHORDS_KEPT,
        };
        let keep = ((looked.len() as f64 * share).ceil() as usize).max(LEAST_KEPT).min(looked.len());
        let best_first = |a: &(f64, usize), b: &(f64, usize)| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1));
        if keep < looked.len() {
            looked.select_nth_unstable_by(keep, best_first);
            looked.truncate(keep);
        }
        looked.sort_unstable_by(best_first);

        // The close look: their notes read here (the index's connection is this thread's), judged on the pool.
        let mut found: Vec<Found> = Vec::with_capacity(looked.len());
        for batch in looked.chunks(CLOSE_BATCH) {
            if stopped() {
                search.stopped = true;
                break;
            }
            let read: Vec<(usize, Candidate, Vec<NoteEvent>)> = batch
                .iter()
                .filter_map(|&(_, at)| {
                    let part = *parts.get(at)?;
                    Some((at, part, notes(part.id, part.hash)?))
                })
                .collect();
            search.close += read.len();
            let fits: Vec<Found> = pool.install(|| {
                read.par_iter()
                    .filter_map(|(at, part, notes)| {
                        Some(Found { at: *at, sits: self.fit(notes, part.role, part.cut)? })
                    })
                    .collect()
            });
            found.extend(fits);
        }
        // The best first; a loop that's there in several keys once, as its best copy.
        found.sort_by(|a, b| b.judged().total_cmp(&a.judged()).then(a.at.cmp(&b.at)));
        let mut shapes = HashSet::new();
        found.retain(|f| parts.get(f.at).and_then(|p| p.shape).is_none_or(|shape| shapes.insert(shape)));
        found.truncate(most);
        search.found = found;
        search
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::{BAR, PPQ, playing};
    use mo_formats::NoteAttribute;
    use mo_formats::arrangement::{Arrangement, LaneKind, PreviewClip, PreviewLane, PreviewNote};

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

    /// Am7, Dm7, G7, Cmaj7, a bar each, moved up by `up` semitones.
    fn chords(up: u8) -> Vec<NoteEvent> {
        [[57, 60, 64, 67], [50, 60, 65, 69], [55, 59, 62, 65], [48, 59, 64, 67]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| keys.iter().map(move |&k| note(bar * BAR, BAR, k + up)))
            .collect()
    }

    /// Their roots, low, a bar each.
    fn bass() -> Vec<NoteEvent> {
        [33, 38, 31, 36].iter().zip(0i64..).map(|(&k, bar)| note(bar * BAR, BAR, k)).collect()
    }

    /// A tune on the chords' thirds, sevenths and roots, two notes a beat apart then a held one each bar, moved up by
    /// `up` semitones.
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

    /// A song whose chords and bass come in at bar 9 and whose lead plays the tune over them from bar 13 (the chords
    /// go round twice).
    fn song() -> playing::Song {
        let twice: Vec<NoteEvent> = chords(0)
            .into_iter()
            .chain(chords(0).into_iter().map(|n| NoteEvent { start: n.start + 4 * BAR, ..n }))
            .collect();
        let mut keys = lane("Keys", 8, &twice);
        keys.clips[0].length = 32.0;
        playing::of(&Arrangement {
            view: "Test",
            tempo: 120.0,
            beats_per_bar: 4,
            lanes: vec![keys, lane("Sub", 8, &bass()), lane("Lead", 12, &tune(0)), lane("FPC", 8, &bass())],
            markers: Vec::new(),
            notes: Vec::new(),
        })
    }

    #[test]
    fn a_tune_in_another_key_is_found_with_the_move_that_fits_it() {
        let song = song();
        assert_eq!(
            song.parts.iter().map(|p| p.role).collect::<Vec<_>>(),
            [Role::Chords, Role::Bass, Role::Melody, Role::Drums]
        );
        assert_eq!((song.harmony_parts(), song.melody_parts()), (vec![0, 1], vec![2]));
        // The song's chords and bass over bars 9 to 12.
        let target = Target::of_song(&song, &song.harmony_parts(), &(8..12)).unwrap();
        assert_eq!(target.wants(), Wants::Tune);
        // The tune as written sits well as it is; saved a fourth up, it's found with the move back down.
        let written = target.fit(&tune(0), Role::Melody, false).unwrap();
        assert_eq!(written.best().0, 0);
        assert!(written.best().1 > 0.8, "{written:?}");
        let fourth_up = target.fit(&tune(5), Role::Melody, false).unwrap();
        assert_eq!(fourth_up.best().0, -5);
        assert!((fourth_up.best().1 - written.best().1).abs() < 1e-9);
        assert!(fourth_up.moved(0) < written.moved(0) - 0.3, "{fourth_up:?}");
        // A tune a half step off the chords sits far below it, whatever the move.
        let off = target.fit(&clashing(), Role::Melody, false).unwrap();
        assert!(off.best().1 < written.best().1 - 0.3, "{off:?}");
        // Drums are never fitted, and nor is nothing.
        assert!(target.fit(&tune(0), Role::Drums, false).is_none());
        assert!(target.fit(&[], Role::Melody, false).is_none());
        assert!(target.tries(Role::Bass) && target.tries(Role::Arp) && !target.tries(Role::Drums));
    }

    #[test]
    fn chords_in_another_key_are_found_for_the_songs_tune() {
        let song = song();
        // The lead alone, where it plays: a tune to find chords for.
        let bars = song.busiest_bars(&[2], 8).unwrap();
        assert_eq!(bars, 12..16);
        let target = Target::of_song(&song, &[2], &bars).unwrap();
        assert_eq!(target.wants(), Wants::Chords);
        let written = target.fit(&chords(0), Role::Chords, false).unwrap();
        assert_eq!(written.best().0, 0);
        assert!(written.best().1 > 0.8, "{written:?}");
        // The same chords saved a minor third up: found with the move back down, sitting the same there.
        let third_up = target.fit(&chords(3), Role::Chords, false).unwrap();
        assert_eq!(third_up.best().0, -3);
        assert!((third_up.best().1 - written.best().1).abs() < 1e-9);
        // Both ways round agree: the song's chords with this tune, or the song's tune with these chords.
        let other_way = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None)
            .unwrap()
            .fit(&tune(0), Role::Melody, false)
            .unwrap();
        assert!((other_way.moved(0) - written.moved(0)).abs() < 1e-9);
        // Under a tune only parts that play chords are tried.
        assert!(target.tries(Role::Chords) && target.tries(Role::Arp));
        assert!(!target.tries(Role::Melody) && !target.tries(Role::Bass) && !target.tries(Role::Drums));
        assert!(target.fit(&chords(0), Role::Drums, false).is_none());
    }

    #[test]
    fn the_bars_picked_are_what_is_fitted_to() {
        // Four bars of the chords, then the same four a tritone up: the tune fits the first four as it is, and the
        // second four moved a tritone.
        let both: Vec<NoteEvent> = chords(0)
            .into_iter()
            .chain(chords(6).into_iter().map(|n| NoteEvent { start: n.start + 4 * BAR, ..n }))
            .collect();
        let mut keys = lane("Keys", 0, &both);
        keys.clips[0].length = 32.0;
        let song = playing::of(&Arrangement {
            view: "Test",
            tempo: 120.0,
            beats_per_bar: 4,
            lanes: vec![keys],
            markers: Vec::new(),
            notes: Vec::new(),
        });
        let first = Target::of_song(&song, &[0], &(0..4)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        let second = Target::of_song(&song, &[0], &(4..8)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        assert_eq!(first.best().0, 0);
        assert_eq!(second.best().0, -6);
        assert!((first.best().1 - second.best().1).abs() < 1e-9);
    }

    #[test]
    fn a_bar_the_picked_parts_rest_in_is_still_one_of_the_bars() {
        let grid = Grid::four_four(PPQ as u32);
        let three_bars =
            |notes: Vec<NoteEvent>| -> Vec<NoteEvent> { notes.into_iter().filter(|n| n.start < 3 * BAR).collect() };
        let song_of = |lanes: Vec<PreviewLane>| {
            playing::of(&Arrangement {
                view: "Test",
                tempo: 120.0,
                beats_per_bar: 4,
                lanes,
                markers: Vec::new(),
                notes: Vec::new(),
            })
        };
        // A lead that rests in the last of the four bars picked: the four bars of chords written for it still line
        // up with it bar for bar.
        let lead = song_of(vec![lane("Lead", 12, &three_bars(tune(0)))]);
        let target = Target::of_song(&lead, &[0], &(12..16)).unwrap();
        assert_eq!(target.wants(), Wants::Chords);
        let rested = target.fit(&chords(0), Role::Chords, false).unwrap();
        assert_eq!(rested.best().0, 0);
        // Heard as three bars going round (no bars given), the tune slips against the chords and sits lower.
        let slips = Target::tune(&three_bars(tune(0)), None).unwrap().fit(&chords(0), Role::Chords, false).unwrap();
        assert!(rested.moved(0) > slips.moved(0) + 0.1, "{rested:?} {slips:?}");

        // Chords that rest in the last picked bar (Am7, Dm7, G7, a bar of nothing): the same for a four-bar tune,
        // whose last bar then plays over the rest.
        let keys = song_of(vec![lane("Keys", 8, &three_bars(chords(0)))]);
        let rested = Target::of_song(&keys, &[0], &(8..12)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        assert_eq!(rested.best().0, 0);
        assert!(rested.heard < 0.9, "{rested:?}");
        let slips = Target::chords(&[(Role::Chords, &three_bars(chords(0)))], grid, None)
            .unwrap()
            .fit(&tune(0), Role::Melody, false)
            .unwrap();
        assert!((slips.heard - 1.0).abs() < 1e-9);
        assert!(rested.moved(0) > slips.moved(0) + 0.1, "{rested:?} {slips:?}");

        // Picked from the bar before the chords come in, the tune found is heard as it will sound dropped on that
        // bar: a bar early for these chords.
        let song = song();
        let on_time = Target::of_song(&song, &[0, 1], &(8..12)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        let early = Target::of_song(&song, &[0, 1], &(7..11)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        assert!(early.moved(0) < on_time.moved(0) - 0.1, "{early:?} {on_time:?}");
    }

    #[test]
    fn nothing_to_fit_to_gives_no_target() {
        let song = song();
        // No part picked, a part that isn't there, drums alone, or bars where the picked parts are silent.
        assert!(Target::of_song(&song, &[], &(8..12)).is_none());
        assert!(Target::of_song(&song, &[9], &(8..12)).is_none());
        assert!(Target::of_song(&song, &[3], &(8..12)).is_none());
        assert!(Target::of_song(&song, &[0, 1], &(0..8)).is_none());
        // Drums picked beside the chords are left out, not heard as notes.
        let with_drums = Target::of_song(&song, &[0, 3], &(8..12)).unwrap();
        assert_eq!(with_drums, Target::of_song(&song, &[0], &(8..12)).unwrap());
        assert!(Target::tune(&[], None).is_none());
        assert!(Target::chords(&[], Grid::four_four(PPQ as u32), None).is_none());
        assert!(Target::chords(&[(Role::Drums, &bass())], Grid::four_four(PPQ as u32), None).is_none());
        // A span with no length, or the wrong way round, is nothing to fit to.
        #[allow(clippy::reversed_empty_ranges)]
        let backwards = Some(4 * BAR..0);
        assert!(Target::tune(&tune(0), Some(0..0)).is_none());
        assert!(Target::tune(&tune(0), backwards.clone()).is_none());
        assert!(Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), Some(0..0)).is_none());
        assert!(Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), backwards).is_none());
    }

    /// A made-up library: each part's note hash, kind and notes, and the book of their slots as the index keeps it.
    struct Library {
        parts: Vec<(String, Role, Vec<NoteEvent>)>,
        book: SlotBook,
    }

    impl Library {
        fn of(parts: Vec<(String, Role, Vec<NoteEvent>)>) -> Library {
            let mut book = SlotBook::default();
            for (hash, _, notes) in &parts {
                book.add(hash, &slots_of(notes));
            }
            Library { parts, book }
        }

        fn place(&self, hash: &str) -> usize {
            self.parts.iter().position(|(h, ..)| h == hash).unwrap()
        }

        /// A search of it, and how many parts' notes it read.
        fn search(&self, target: &Target, most: usize, stop: bool) -> (Search, usize) {
            let candidates: Vec<Candidate> = self
                .parts
                .iter()
                .zip(0i64..)
                .map(|((hash, role, notes), id)| Candidate {
                    id,
                    hash,
                    role: *role,
                    cut: false,
                    shape: crate::analyser::shape_of(notes),
                })
                .collect();
            let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
            let mut read = 0;
            let mut notes = |id: i64, hash: &str| {
                read += 1;
                let (own, _, notes) = self.parts.get(usize::try_from(id).ok()?)?;
                (own == hash).then(|| notes.clone())
            };
            let search = target.search(&candidates, &self.book, &mut notes, most, &pool, &AtomicBool::new(stop));
            (search, read)
        }
    }

    /// Four bars of notes a beat long that go nowhere in particular, a different set for each `seed`.
    fn wandering(seed: u32) -> Vec<NoteEvent> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12_345);
        (0..16i64)
            .map(|beat| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                note(beat * PPQ, PPQ, 60 + (state >> 24) as u8 % 24)
            })
            .collect()
    }

    /// The same, as three-note chords a bar long.
    fn wandering_chords(seed: u32) -> Vec<NoteEvent> {
        wandering(seed)
            .chunks(4)
            .zip(0i64..)
            .flat_map(|(beats, bar)| beats[..3].iter().map(move |n| note(bar * BAR, BAR, n.key - 12)))
            .collect()
    }

    #[test]
    fn a_search_finds_the_tune_written_for_the_chords_in_a_whole_library() {
        let mut parts: Vec<(String, Role, Vec<NoteEvent>)> =
            (0..500).map(|i| (format!("wander {i}"), Role::Melody, wandering(i))).collect();
        parts.push(("written".into(), Role::Melody, tune(0)));
        // The same tune as a pack has it in another key, and a copy of the very same notes from another file.
        parts.push(("a fourth up".into(), Role::Melody, tune(5)));
        parts.push(("written".into(), Role::Melody, tune(0)));
        parts.push(("clashing".into(), Role::Melody, clashing()));
        parts.push(("drums".into(), Role::Drums, tune(0)));
        let mut library = Library::of(parts);
        // One the index has no slots for yet.
        library.parts.push(("just read".into(), Role::Melody, tune(2)));

        let target =
            Target::chords(&[(Role::Chords, &chords(0)), (Role::Bass, &bass())], Grid::four_four(PPQ as u32), None)
                .unwrap();
        let (search, read) = library.search(&target, 20, false);
        // Every pitched set of notes with slots is sorted by the first look, each once; only the best share have
        // their notes read for the close look (300 at the least).
        assert_eq!((search.looked, search.not_ready, search.stopped), (503, 1, false));
        assert_eq!((search.close, read), (300, 300));
        assert_eq!(search.found.len(), 20);
        // The tune written for the chords comes first, as it is; its copy a fourth up isn't listed beside it.
        let best = search.found[0];
        assert_eq!(best.at, library.place("written"));
        assert_eq!(best.sits.best().0, 0);
        assert!(best.sits.best().1 > 0.8, "{best:?}");
        assert!(search.found.iter().all(|f| f.at != library.place("a fourth up")));
        // Best first, and never the drums.
        assert!(search.found.windows(2).all(|pair| pair[0].judged() >= pair[1].judged()));
        assert!(search.found.iter().all(|f| library.parts[f.at].1 != Role::Drums));

        // Only copies in other keys in the library (the one just read has its slots now): the tune is found as the
        // copy that needs moving least, with the move that makes it fit.
        library.parts.retain(|(hash, ..)| hash != "written");
        let library = Library::of(library.parts);
        let (search, _) = library.search(&target, 20, false);
        assert_eq!(search.not_ready, 0);
        assert_eq!(search.found[0].at, library.place("just read"));
        assert_eq!(search.found[0].sits.best().0, -2);
        assert!(search.found.iter().all(|f| f.at != library.place("a fourth up")));
    }

    #[test]
    fn a_search_finds_chords_for_a_tune() {
        let mut parts: Vec<(String, Role, Vec<NoteEvent>)> =
            (0..400).map(|i| (format!("wander {i}"), Role::Chords, wandering_chords(i))).collect();
        parts.push(("written".into(), Role::Chords, chords(0)));
        parts.push(("a third up".into(), Role::Chords, chords(3)));
        // Under a tune only parts that play chords are tried.
        parts.push(("a bassline".into(), Role::Bass, bass()));
        parts.push(("another tune".into(), Role::Melody, tune(0)));
        let library = Library::of(parts);
        let target = Target::tune(&tune(0), None).unwrap();
        let (search, read) = library.search(&target, 10, false);
        assert_eq!((search.looked, search.not_ready), (402, 0));
        // A tenth of them would be 41: 300 at the least.
        assert_eq!((search.close, read), (300, 300));
        assert_eq!(search.found[0].at, library.place("written"));
        assert_eq!(search.found[0].sits.best().0, 0);
        assert!(search.found.iter().all(|f| f.at != library.place("a third up")));
    }

    #[test]
    fn a_search_stopped_for_a_newer_one_reads_nothing() {
        let library = Library::of((0..50).map(|i| (format!("wander {i}"), Role::Melody, wandering(i))).collect());
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        let (search, read) = library.search(&target, 10, true);
        assert!(search.stopped && search.found.is_empty());
        assert_eq!(read, 0);
        // Nothing to search: nothing found, and nothing goes wrong.
        let (search, _) = Library::of(Vec::new()).search(&target, 10, false);
        assert_eq!(search, Search::default());
    }

    /// How long a search of a big library takes: `cargo test -p mo-midi --release --lib how_long -- --ignored
    /// --nocapture`. The notes are made up, so it says nothing of what's found.
    #[test]
    #[ignore = "a measure, not a test"]
    fn how_long_a_search_takes() {
        let pool = rayon::ThreadPoolBuilder::new().build().unwrap();
        for (count, role, wants) in [(100_000u32, Role::Melody, "tunes"), (100_000, Role::Chords, "chords")] {
            let made = std::time::Instant::now();
            let parts: Vec<(String, Vec<NoteEvent>)> = (0..count)
                .map(|i| (format!("{i:08X}"), if role == Role::Chords { wandering_chords(i) } else { wandering(i) }))
                .collect();
            let mut book = SlotBook::default();
            for (hash, notes) in &parts {
                book.add(hash, &slots_of(notes));
            }
            let candidates: Vec<Candidate> = parts
                .iter()
                .zip(0i64..)
                .map(|((hash, _), id)| Candidate { id, hash, role, cut: false, shape: None })
                .collect();
            let target = if role == Role::Chords {
                Target::tune(&tune(0), None).unwrap()
            } else {
                Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap()
            };
            let mut notes = |id: i64, _: &str| parts.get(usize::try_from(id).ok()?).map(|(_, notes)| notes.clone());
            let made = made.elapsed();
            let started = std::time::Instant::now();
            let search = target.search(&candidates, &book, &mut notes, 200, &pool, &AtomicBool::new(false));
            println!(
                "{wants}: {count} parts made in {made:.1?}; searched in {:.1?} on {} threads ({} looked at closely, \
                 {} found, the best at {:.2})",
                started.elapsed(),
                pool.current_num_threads(),
                search.close,
                search.found.len(),
                search.found.first().map_or(0.0, |f| f.sits.best().1)
            );
        }
    }
}
