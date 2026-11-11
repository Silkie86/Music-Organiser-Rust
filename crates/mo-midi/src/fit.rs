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
//! rule): a part found goes round by its own bars of 4/4, as a clip dropped into the song keeps its own length, so
//! there the answer says less.
//!
//! Trying the whole library ([`Target::search`]) takes two looks. Every part of the right kind is first sorted by
//! its slots alone (how long it plays each pitch class in every two quarter notes, kept in the index and held in
//! memory: about a microsecond a part), and only the best share of them have their notes read and get the close
//! look: the best 2 in 100 when looking for tunes, 10 in 100 when looking for chords, which keeps 99 and 98 in 100 of
//! what the close look alone would put in its best ten (`docs/harmony/together-experiments.md`). A loop a pack has in
//! every key is one loop to both looks: one of its copies is read, and the copy given is the one that needs moving
//! least.
//!
//! What the MIDI page asks for (the song saved last, its own parts left out, the page's filters kept, the answer as
//! places in its catalogue) is in [`for_song`].
//!
//! The octave a part found goes at beside a song is [`octave`]'s.
//!
//! Not here yet: sending the part found into the project.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;

use mo_formats::NoteEvent;
use mo_harmony::{Backing, ChordContext, Grid, LineUp, Note, Outline, Part, Sits, Sitting, Slot, Span, sit};
use rayon::prelude::*;

use crate::chart;
use crate::index::StoredPhrase;
use crate::keys::{heard_as, heard_notes};
use crate::playing::Song;
use crate::roles::Role;
use crate::store::SlotBook;
use crate::{BAR, PPQ};

pub mod for_song;
pub mod octave;

use octave::Registers;

/// How long a first look's slot is ([`mo_harmony::Slot`]), in the index's ticks: two quarter notes, half a bar of
/// 4/4, whatever the song's time signature.
pub const SLOT: i64 = 2 * PPQ;

/// How this crate asks for a part's slots ([`slots_of`]: which of its notes are heard, the slot's length, bars of
/// 4/4, the stretch they're made over): bump it when that changes. 2: over the part's clip
/// ([`crate::analyser::clip_length`]), where 1 went from the bar of its first note to the bar its last note ended
/// in, a bar more for a last note a few ticks over a bar line.
const SLOTS_ASKING: u32 = 2;

/// What made the slots the index keeps of each set of notes: how `mo_harmony` makes slots and how it was asked, like
/// [`crate::keys::VERSION`]. When the app's is another, the index makes them again from the notes it keeps (rule 7).
pub const SLOTS_VERSION: u32 = mo_harmony::SLOTS_VERSION * 100 + SLOTS_ASKING;

/// A library part's slots, for a search's first look: how long it plays each pitch class in every [`SLOT`], over its
/// clip as the app plays, drags and sends it ([`crate::analyser::clip_length`]: its bars of 4/4 from its first bar
/// line), its first 32 bars at the most. The same stretch the close look hears go round ([`Target::fit`]). None
/// when nothing in it is heard.
pub fn slots_of(notes: &[NoteEvent]) -> Vec<Slot> {
    let heard = heard_notes(notes);
    if heard.is_empty() {
        return Vec::new();
    }
    mo_harmony::slots_over(&heard, SLOT, 0, crate::analyser::clip_length(notes))
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
    /// Where each kind of part plays beside it: what says the octave a part found goes at. A song's
    /// ([`Target::of_song`]); for chords or a tune alone, nothing, so each kind goes by its usual place.
    registers: Registers,
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
        Some(Target { given: Given::Chords(backing), registers: Registers::default() })
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
        (!notes.is_empty()).then_some(Target { given: Given::Tune { notes, line_up }, registers: Registers::default() })
    }

    /// The picked parts of a song (places in [`Song::parts`]) over `bars` of it, counted from 0: a tune to find chords
    /// for when every one of them is a melody, else chords to find a tune for. Drums are left out; a sampler's notes
    /// ([`crate::playing::Playing::sample`]) are heard as written when it's picked. Every picked bar counts, one the
    /// parts rest in too. None when the picked parts play nothing pitched in those bars. A part found goes at the
    /// octave its kind plays at beside those bars, whichever of the song's parts are picked
    /// ([`Registers::by_bars`]).
    pub fn of_song(song: &Song, parts: &[usize], bars: &Range<i64>) -> Option<Target> {
        let target = Target::of_picked(song, parts, bars)?;
        Some(Target { registers: Registers::by_bars(song, bars), ..target })
    }

    fn of_picked(song: &Song, parts: &[usize], bars: &Range<i64>) -> Option<Target> {
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

    /// Where each kind of part plays beside it: a song's bars' ([`Target::of_song`]), or nothing known.
    pub fn registers(&self) -> &Registers {
        &self.registers
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
            Given::Chords(backing) => {
                let heard = heard_notes(notes);
                sit(&heard, backing, own_round(notes)?, &Sitting::SHIPPED)
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

/// How a part from the library goes round over a song's chords: as its clip does dropped into the song
/// ([`crate::analyser::clip_length`]: from its first bar line, for its bars as the index counts them, bars of 4/4
/// whatever the song's time signature), so what's judged is what's then played, dragged and sent. An empty bar at
/// its start is a bar of the clip, and a last note that hangs a few ticks over a bar line doesn't make one. The
/// same stretch its slots are made over ([`slots_of`]), so the first look and the close look hear it go round
/// alike. None for no notes.
fn own_round(notes: &[NoteEvent]) -> Option<LineUp> {
    let length = crate::analyser::clip_length(notes);
    (length > 0).then_some(LineUp::LoopsFrom { start: 0, length })
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

/// Batches of notes read and waiting to be judged, at the most: past that the next batch is judged before another
/// is read, so notes read never pile up in memory.
const WAITING_MOST: usize = 2;

/// The most slots of a chord part the first look goes by: the bars the close look reads a long part's chords in
/// ([`chart::MOST_BARS`]; it takes the notes that start in them, so a last chord held past them is a little more to
/// the close look than to the first).
const CHORD_SLOTS: usize = chart::MOST_BARS * (BAR / SLOT) as usize;

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
    /// Its lowest note: with the shape, how many semitones a copy in another key is from this one.
    pub lowest: u8,
}

/// What makes two candidates copies of one loop to a search: the same shape, read the same way (their kind, and
/// whether they're a piece cut from a longer stretch, which says how a chord part's chords are read).
type Loop = (i64, Role, bool);

impl<'a> Candidate<'a> {
    /// The loop it's a copy of; none for a part with no shape.
    fn of_loop(&self) -> Option<Loop> {
        Some((self.shape?, self.role, self.cut))
    }

    pub fn of(part: &'a StoredPhrase) -> Candidate<'a> {
        Candidate {
            id: part.id,
            hash: &part.hash,
            role: if part.drums { Role::Drums } else { part.role() },
            cut: part.is_cut(),
            shape: part.shape,
            lowest: part.lowest,
        }
    }
}

/// A part a search found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    /// Its place among the candidates given.
    pub at: usize,
    /// How it sits with the target, the part found moved up by each of 0 to 11 semitones (the song stays as it is):
    /// [`Sits::best`] gives the move of the part found that fits best, and how it sits there.
    pub sits: Sits,
    /// How many octaves to move it as well, once it's moved into key, so it plays where its kind of part plays
    /// beside the target ([`Registers::octaves_for`]): up for more than 0. Octaves make no difference to how it sits.
    pub octaves: i32,
    /// The lowest and the highest key it plays as it is, strays aside ([`octave::edges`]): what an octave chosen by
    /// hand mustn't take off the keyboard either ([`octave::on_keyboard`]).
    pub edges: (u8, u8),
}

impl Found {
    /// How far it's moved in all, in semitones, up for more than 0: into the key ([`Sits::best`]) and to the octave.
    pub fn moved(&self) -> i32 {
        self.sits.best().0 + 12 * self.octaves
    }

    /// Whether this copy of a loop is the one to give in place of `other`, a copy of the same loop: it fits as
    /// well and needs moving less. A semitone less shows in [`Sits::judged`]; copies octaves apart sit alike, and of
    /// those it's the one that goes fewer octaves.
    fn handier_than(&self, other: &Found) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self.sits.judged().total_cmp(&other.sits.judged()) {
            Greater => true,
            Equal => self.octaves.abs() < other.octaves.abs(),
            Less => false,
        }
    }
}

/// What a search came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Search {
    /// The parts that fit best, the best first.
    pub found: Vec<Found>,
    /// How many parts the first look sorted, and how many had their notes read for the close look.
    pub looked: usize,
    pub close: usize,
    /// Parts of the right kind left out because the book has no slots for them: the index makes them after its next
    /// read (or, where a part's slots are damaged, when its file is read again).
    pub not_ready: usize,
    /// Parts picked for the close look whose notes couldn't be read (the caller's `notes` gave none): left out.
    pub unread: usize,
    /// Stopped before the end, because a newer search was wanted: `found` isn't the whole answer.
    pub stopped: bool,
}

/// What a close look at some parts came to.
#[derive(Default)]
struct Closely {
    /// What fits, in no order.
    found: Vec<Found>,
    /// The places (among the candidates) of parts whose notes couldn't be read.
    unread: Vec<usize>,
    /// Stopped before every part's notes were read.
    stopped: bool,
}

/// What a first look tries each part's slots against.
enum First {
    /// The target's chords, for tunes.
    Chords(Outline),
    /// The target's tune, for chord parts.
    Tune(Vec<Slot>),
}

impl First {
    /// How a part with these slots is likely to sit with the target at its best move, a part that needs moving
    /// less a hair ahead ([`Sits::judged`]).
    fn look(&self, slots: &[Slot]) -> Option<f64> {
        let sits = match self {
            First::Chords(outline) => outline.sits(slots),
            First::Tune(tune) => {
                Outline::guessed(slots.get(..CHORD_SLOTS).unwrap_or(slots), &Sitting::SHIPPED).sits(tune)
            }
        };
        sits.map(|sits| sits.judged())
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
    /// `parts` are the library's parts to try, in the order the page lists them (the same notes are tried once, as
    /// the first part given that has them). What shouldn't come up is left out of them by the caller, before the
    /// search: the parts of the song the target was made from are in the library too, and would be found first.
    /// `book` has their slots ([`crate::store::MidiStore::slots`]), and `notes` reads one part's notes from the
    /// index, by its row and note hash ([`crate::store::MidiStore::notes`]; none when they can't be read): called on
    /// this thread, and only for the share of parts that gets the close look. It's the caller's to keep notes it has
    /// read for the next search. The first look and the close look run on `pool`. `stop` set (a newer search is
    /// wanted) ends it early.
    ///
    /// Call it off the window's thread.
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
        let looked: Vec<(f64, usize)> = pool.install(|| {
            tried
                .par_iter()
                .filter_map(|&(at, slots)| if stopped() { None } else { Some((first.look(slots)?, at)) })
                .collect()
        });
        if stopped() {
            search.stopped = true;
            return search;
        }
        // A loop that's there in several keys is one loop: the copy the first look puts first (the one that needs
        // moving least, as they sit alike) stands for all of them, so a loop takes one place in the share kept and
        // one read, however many keys a pack has it in.
        let mut standing: HashMap<Loop, usize> = HashMap::new();
        let mut loops: Vec<(f64, usize)> = Vec::with_capacity(looked.len());
        for &(judged, at) in &looked {
            let place = match parts.get(at).and_then(Candidate::of_loop) {
                Some(of_loop) => match standing.entry(of_loop) {
                    Entry::Occupied(place) => Some(*place.get()),
                    Entry::Vacant(place) => {
                        place.insert(loops.len());
                        None
                    }
                },
                None => None,
            };
            match place.and_then(|place| loops.get_mut(place)) {
                Some(stands) if judged > stands.0 => *stands = (judged, at),
                Some(_) => {}
                None => loops.push((judged, at)),
            }
        }
        drop(standing);
        // The best share of them, the best first, so a search stopped early has looked closely at the likeliest.
        let share = match self.wants() {
            Wants::Tune => TUNES_KEPT,
            Wants::Chords => CHORDS_KEPT,
        };
        let keep = ((loops.len() as f64 * share).ceil() as usize).max(LEAST_KEPT).min(loops.len());
        let best_first = |a: &(f64, usize), b: &(f64, usize)| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1));
        if keep < loops.len() {
            loops.select_nth_unstable_by(keep, best_first);
            loops.truncate(keep);
        }
        loops.sort_unstable_by(best_first);
        let kept: Vec<usize> = loops.into_iter().map(|(_, at)| at).collect();

        // The close look. The best of what it finds first, a shape once (the same loop read two ways, as a pack's
        // file and as a piece cut from a song, is still one loop to list).
        let best_first = |found: &mut Vec<Found>| {
            found.sort_by(|a, b| b.sits.judged().total_cmp(&a.sits.judged()).then(a.at.cmp(&b.at)));
            let mut shapes = HashSet::new();
            found.retain(|f| parts.get(f.at).and_then(|p| p.shape).is_none_or(|shape| shapes.insert(shape)));
        };
        let closely = self.close_look(parts, &kept, notes, pool, stop, &mut search);
        search.stopped = closely.stopped;
        let mut found = closely.found;
        best_first(&mut found);

        // A second, short close look, at other copies of some loops. Stopped part of the way, the list is still
        // whole: only some copies in it aren't the handiest.
        if !search.stopped {
            // Of a loop's copies, the one to give is the one that needs moving least: where the copy read fits best
            // moved (into the key, or to another octave), the copy that many semitones from it (if the library has
            // it) is read too, and given in its place when it does fit as well with less moving. For the parts
            // that will be listed, and any below them that such a copy could lift into the list.
            let last = most.checked_sub(1).and_then(|last| found.get(last)).map(|f| f.sits.judged());
            let in_reach = |place: usize, f: &Found| place < most || last.is_some_and(|last| f.sits.best().1 >= last);
            let nearer = nearer_copies(parts, &looked, &found, &in_reach);
            // And where the copy picked couldn't be read, the next best copy of that loop.
            let spare = spare_copies(parts, &looked, &closely.unread);
            let places: Vec<usize> = nearer.iter().map(|&(at, _)| at).chain(spare).collect();
            let given: HashMap<usize, usize> = nearer.into_iter().collect();
            for copy in self.close_look(parts, &places, notes, pool, stop, &mut search).found {
                match given.get(&copy.at).map(|place| found.get_mut(*place)) {
                    Some(Some(first)) if copy.handier_than(first) => *first = copy,
                    Some(_) => {}
                    None => found.push(copy),
                }
            }
            best_first(&mut found);
        }
        found.truncate(most);
        search.found = found;
        search
    }

    /// The close look at the parts at `places` among `parts`, in that order: their notes are read here, a batch at
    /// a time (the index's connection is this thread's), and judged on `pool` while the next batch is read. `stop`
    /// ends it after the batch being read. Counts the parts read and the ones that couldn't be in `search`.
    fn close_look<'p>(
        &self,
        parts: &[Candidate<'p>],
        places: &[usize],
        notes: &mut dyn FnMut(i64, &str) -> Option<Vec<NoteEvent>>,
        pool: &rayon::ThreadPool,
        stop: &AtomicBool,
        search: &mut Search,
    ) -> Closely {
        let judge = |read: &[Read<'p>]| -> Vec<Found> {
            read.par_iter()
                .filter_map(|(at, part, notes)| {
                    let sits = self.fit(notes, part.role, part.cut)?;
                    let edges = octave::edges(notes)?;
                    let octaves = self.registers.octaves_for(part.role, edges, sits.best().0);
                    Some(Found { at: *at, sits, octaves, edges })
                })
                .collect()
        };
        let mut closely = Closely::default();
        let (judged, fits) = channel::<Vec<Found>>();
        // Batches read and not yet judged.
        let waiting = AtomicUsize::new(0);
        // The reading stays on this thread; each batch read is handed to the pool.
        pool.in_place_scope(|scope| {
            let (judge, waiting) = (&judge, &waiting);
            for batch in places.chunks(CLOSE_BATCH) {
                if stop.load(Ordering::Relaxed) {
                    closely.stopped = true;
                    break;
                }
                let mut read: Vec<Read<'p>> = Vec::with_capacity(batch.len());
                for &at in batch {
                    let Some(part) = parts.get(at) else { continue };
                    match notes(part.id, part.hash).filter(|notes| !notes.is_empty()) {
                        Some(notes) => read.push((at, *part, notes)),
                        None => closely.unread.push(at),
                    }
                }
                search.close += read.len();
                let judged = judged.clone();
                if waiting.load(Ordering::Relaxed) >= WAITING_MOST {
                    // The pool is behind: this batch is judged before another is read.
                    let _ = judged.send(pool.install(|| judge(&read)));
                } else {
                    waiting.fetch_add(1, Ordering::Relaxed);
                    scope.spawn(move |_| {
                        let _ = judged.send(judge(&read));
                        waiting.fetch_sub(1, Ordering::Relaxed);
                    });
                }
            }
        });
        drop(judged);
        search.unread += closely.unread.len();
        closely.found = fits.into_iter().flatten().collect();
        closely
    }
}

/// A part read for the close look: its place among the candidates, the candidate, and its notes.
type Read<'a> = (usize, Candidate<'a>, Vec<NoteEvent>);

/// For each part found that `in_reach` takes (by its place in `found`), fits best moved ([`Found::moved`]: into the
/// key, to another octave, or both), and is a loop the library has other copies of ([`Loop`]): the copy that many
/// semitones from it, which would fit as it is. That copy's place among `parts` with the found part's place in
/// `found`, in the order found. Of several copies in that key (octaves apart), the nearest in pitch to where the
/// found part would be moved; of copies as near, the first listed. `looked` are the parts the first look sorted, in
/// the order listed.
fn nearer_copies(
    parts: &[Candidate],
    looked: &[(f64, usize)],
    found: &[Found],
    in_reach: &dyn Fn(usize, &Found) -> bool,
) -> Vec<(usize, usize)> {
    // By loop: the found part's place in `found`, and the lowest note a copy that fits unmoved would have.
    let mut wanted: HashMap<Loop, (usize, i32)> = HashMap::new();
    for (place, given) in found.iter().enumerate().filter(|(place, given)| in_reach(*place, given)) {
        let moved = given.moved();
        if moved != 0
            && let Some(part) = parts.get(given.at)
            && let Some(of_loop) = part.of_loop()
        {
            wanted.insert(of_loop, (place, i32::from(part.lowest) + moved));
        }
    }
    if wanted.is_empty() {
        return Vec::new();
    }
    // By the found part's place: the best copy so far, and how far its lowest note is from the one wanted.
    let mut nearest: HashMap<usize, (usize, i32)> = HashMap::new();
    for &(_, at) in looked {
        let Some(part) = parts.get(at) else { continue };
        let Some(&(place, lowest)) = part.of_loop().and_then(|of_loop| wanted.get(&of_loop)) else { continue };
        let away = i32::from(part.lowest) - lowest;
        if away.rem_euclid(12) != 0 || found.get(place).is_some_and(|given| given.at == at) {
            continue;
        }
        match nearest.entry(place) {
            Entry::Occupied(mut best) if away.abs() < best.get().1 => {
                best.insert((at, away.abs()));
            }
            Entry::Occupied(_) => {}
            Entry::Vacant(best) => {
                best.insert((at, away.abs()));
            }
        }
    }
    let mut copies: Vec<(usize, usize)> = nearest.into_iter().map(|(place, (at, _))| (at, place)).collect();
    copies.sort_unstable_by_key(|&(_, place)| place);
    copies
}

/// For each part at `unread` (places among `parts`) that's a loop the library has other copies of: the copy the
/// first look puts next (of copies it puts alike, the first listed), its place among `parts`. In the order listed.
fn spare_copies(parts: &[Candidate], looked: &[(f64, usize)], unread: &[usize]) -> Vec<usize> {
    let gone: HashSet<usize> = unread.iter().copied().collect();
    // By loop: the best other copy so far, and how the first look judged it.
    let mut spare: HashMap<Loop, Option<(f64, usize)>> =
        unread.iter().filter_map(|at| Some((parts.get(*at)?.of_loop()?, None))).collect();
    if spare.is_empty() {
        return Vec::new();
    }
    for &(judged, at) in looked.iter().filter(|(_, at)| !gone.contains(at)) {
        if let Some(best) = parts.get(at).and_then(Candidate::of_loop).and_then(|of_loop| spare.get_mut(&of_loop))
            && best.is_none_or(|(best, _)| judged > best)
        {
            *best = Some((judged, at));
        }
    }
    let mut copies: Vec<usize> = spare.into_values().flatten().map(|(_, at)| at).collect();
    copies.sort_unstable();
    copies
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::{BAR, PPQ, playing};
    use mo_formats::NoteAttribute;
    use mo_formats::arrangement::{Arrangement, LaneKind, PreviewClip, PreviewLane, PreviewNote};

    pub(super) fn note(start: i64, length: i64, key: u8) -> NoteEvent {
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
    pub(super) fn chords(up: u8) -> Vec<NoteEvent> {
        [[57, 60, 64, 67], [50, 60, 65, 69], [55, 59, 62, 65], [48, 59, 64, 67]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| keys.iter().map(move |&k| note(bar * BAR, BAR, k + up)))
            .collect()
    }

    /// Their roots, low, a bar each.
    pub(super) fn bass() -> Vec<NoteEvent> {
        [33, 38, 31, 36].iter().zip(0i64..).map(|(&k, bar)| note(bar * BAR, BAR, k)).collect()
    }

    /// A tune on the chords' thirds, sevenths and roots, two notes a beat apart then a held one each bar, moved up by
    /// `up` semitones.
    pub(super) fn tune(up: u8) -> Vec<NoteEvent> {
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

    pub(super) fn lane(name: &str, at_bar: i64, notes: &[NoteEvent]) -> PreviewLane {
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
    pub(super) fn song() -> playing::Song {
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
        /// Note hashes whose notes the index can't give.
        unreadable: Vec<&'static str>,
        /// A newer search is wanted (the stop is set) once this many parts' notes are read.
        stop_after: usize,
    }

    impl Library {
        fn of(parts: Vec<(String, Role, Vec<NoteEvent>)>) -> Library {
            let mut book = SlotBook::default();
            for (hash, _, notes) in &parts {
                book.add(hash, &slots_of(notes));
            }
            Library { parts, book, unreadable: Vec::new(), stop_after: usize::MAX }
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
                    lowest: notes.iter().map(|n| n.key).min().unwrap_or(0),
                })
                .collect();
            let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
            let stop = AtomicBool::new(stop);
            let mut read = 0;
            let mut notes = |id: i64, hash: &str| {
                read += 1;
                if read >= self.stop_after {
                    stop.store(true, Ordering::Relaxed);
                }
                let (own, _, notes) = self.parts.get(usize::try_from(id).ok()?)?;
                (own == hash && !self.unreadable.contains(&hash)).then(|| notes.clone())
            };
            let search = target.search(&candidates, &self.book, &mut notes, most, &pool, &stop);
            (search, read)
        }
    }

    /// Four bars of notes a beat long that go nowhere in particular, a different set for each `seed`.
    pub(super) fn wandering(seed: u32) -> Vec<NoteEvent> {
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
        assert!(search.found.windows(2).all(|pair| pair[0].sits.judged() >= pair[1].sits.judged()));
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

    /// Some notes moved up by a number of semitones.
    fn up(notes: &[NoteEvent], semitones: u8) -> Vec<NoteEvent> {
        notes.iter().map(|n| NoteEvent { key: n.key + semitones, ..*n }).collect()
    }

    #[test]
    fn a_loop_in_every_key_is_read_once_and_given_as_the_copy_that_fits_as_it_is() {
        // As a pack has them: 40 loops, each in all twelve keys, and the tune written for the chords the same way
        // (its own key listed last).
        let mut parts: Vec<(String, Role, Vec<NoteEvent>)> = Vec::new();
        for i in 0..40 {
            parts.extend((0..12).map(|key| (format!("loop {i} up {key}"), Role::Melody, up(&wandering(i), key))));
        }
        parts.extend((0..12).rev().map(|key| (format!("written up {key}"), Role::Melody, tune(key))));
        let library = Library::of(parts);
        let target =
            Target::chords(&[(Role::Chords, &chords(0)), (Role::Bass, &bass())], Grid::four_four(PPQ as u32), None)
                .unwrap();
        let (search, read) = library.search(&target, 20, false);
        // Every copy gets the first look; each loop's notes are read once, and once more at the most for the copy
        // that needs no moving.
        assert_eq!(search.looked, 41 * 12);
        assert!((41..=41 + 20).contains(&read), "{read}");
        assert_eq!((search.close, search.unread), (read, 0));
        // Twenty loops, not twenty copies of one or two; each as the copy that fits as it is.
        assert_eq!(search.found.len(), 20);
        assert_eq!(search.found[0].at, library.place("written up 0"));
        assert!(search.found.iter().all(|f| f.sits.best().0 == 0), "{:?}", search.found);
        assert!(search.found.windows(2).all(|pair| pair[0].sits.judged() >= pair[1].sits.judged()));
    }

    #[test]
    fn the_copy_given_is_the_one_the_close_look_finds_fits_as_it_is() {
        // The copy a fourth up is read first. The close look hears it fit best moved down a fourth, and the copy
        // that is there is read and given: of the two in that key, the one nearest where the copy read would be
        // moved to.
        let octave_down: Vec<NoteEvent> = tune(0).iter().map(|n| NoteEvent { key: n.key - 12, ..*n }).collect();
        let library = out_of_step(vec![("an octave down".into(), Role::Melody, octave_down)]);
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        let (search, read) = library.search(&target, 10, false);
        assert_eq!((search.found.len(), read, search.close), (1, 2, 2));
        assert_eq!(search.found[0].at, library.place("written"));
        assert_eq!(search.found[0].sits.best().0, 0);
    }

    #[test]
    fn a_part_whose_notes_cant_be_read_is_counted_and_left_out() {
        let mut library = Library::of(vec![
            ("written".into(), Role::Melody, tune(0)),
            ("wander".into(), Role::Melody, wandering(1)),
            ("no notes any more".into(), Role::Melody, wandering(2)),
        ]);
        library.unreadable.push("written");
        library.parts[2].2.clear();
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        let (search, read) = library.search(&target, 10, false);
        assert_eq!((search.looked, read, search.close, search.unread), (3, 3, 1, 2));
        assert_eq!(search.found.iter().map(|f| f.at).collect::<Vec<_>>(), [library.place("wander")]);
    }

    #[test]
    fn a_long_chord_part_gets_its_first_look_by_the_bars_its_chords_are_read_in() {
        let first = Target::tune(&tune(0), None).unwrap().first();
        let later = |notes: Vec<NoteEvent>, bars: i64| {
            notes.into_iter().map(move |n| NoteEvent { start: n.start + bars * BAR, ..n })
        };
        // The chords written for the tune four times round, 16 bars, then 8 bars of something else.
        let sixteen: Vec<NoteEvent> = (0..4).flat_map(|round| later(chords(0), round * 4)).collect();
        let mut long = sixteen.clone();
        long.extend(later(wandering_chords(7), 16).chain(later(wandering_chords(8), 20)));
        assert_eq!((slots_of(&sixteen).len(), slots_of(&long).len()), (32, 48));
        assert!(first.look(&slots_of(&sixteen)).is_some());
        assert_eq!(first.look(&slots_of(&long)), first.look(&slots_of(&sixteen)));
    }

    #[test]
    fn in_a_song_not_in_four_four_a_part_found_goes_round_by_its_own_bars() {
        // A waltz: the chords a bar of 3/4 each.
        let waltz: Vec<NoteEvent> = [[57, 60, 64], [50, 62, 65], [55, 59, 62], [48, 60, 64]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| keys.iter().map(move |&k| note(bar * 3 * PPQ, 3 * PPQ, k)))
            .collect();
        let three_four = Grid { ticks_per_beat: PPQ as u32, beats_per_bar: 3, bar_line: 0 };
        let target = Target::chords(&[(Role::Chords, &waltz)], three_four, None).unwrap();
        // A part of the library, four bars of 4/4 from its second bar, each note two quarter notes long. Dropped
        // into the song it keeps its own length, the empty bar at its start too: it comes round every 20 quarter
        // notes, the chords every 12.
        let found: Vec<NoteEvent> =
            [72, 76, 74, 77, 71, 74, 72, 76].iter().zip(0i64..).map(|(&k, i)| note(BAR + i * SLOT, SLOT, k)).collect();
        let close = target.fit(&found, Role::Melody, false).unwrap();
        // Its notes fill whole slots, so the first look hears just what the close look does.
        let first = target.first().look(&slots_of(&found)).unwrap();
        assert!((first - close.judged()).abs() < 1e-9, "{first} {}", close.judged());
    }

    /// What the search judges is the clip you then get: the part's bars as the index counts them, from its first bar
    /// line. A last note a few ticks over a bar line doesn't make the loop a bar longer, and an empty or muted bar
    /// at its start or end is a bar of it.
    #[test]
    fn a_part_found_is_judged_over_the_clip_you_get() {
        let four_bars = Some(LineUp::LoopsFrom { start: 0, length: 4 * BAR });
        // The tune written for the chords, its last note held a 200th of a bar past the fourth bar line: still four bars
        // (five, by the bar its last note ends in), so it comes round with the chords, not a bar later each time.
        let mut over = tune(0);
        over.last_mut().unwrap().length += BAR / 200;
        assert_eq!((crate::analyser::bars_of(&over), slots_of(&over).len()), (4, 8));
        assert_eq!((own_round(&over), own_round(&tune(0))), (four_bars, four_bars));
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        let (exact, hangs) =
            (target.fit(&tune(0), Role::Melody, false).unwrap(), target.fit(&over, Role::Melody, false));
        let hangs = hangs.unwrap();
        assert_eq!((exact.best().0, hangs.best().0), (0, 0));
        assert!((exact.judged() - hangs.judged()).abs() < 0.01, "{} {}", exact.judged(), hangs.judged());
        // Going round a bar late each time, as it was judged before, it sat worse: the search would have ranked it
        // under parts that fit less well.
        let Given::Chords(backing) = &target.given else { panic!("chords were given") };
        let late =
            sit(&heard_notes(&over), backing, LineUp::LoopsFrom { start: 0, length: 5 * BAR }, &Sitting::SHIPPED);
        assert!(late.unwrap().judged() < hangs.judged() - 0.05);
        // The first look hears the same four bars.
        let first = target.first();
        let (a, b) = (first.look(&slots_of(&tune(0))).unwrap(), first.look(&slots_of(&over)).unwrap());
        assert!((a - b).abs() < 0.01, "{a} {b}");

        // A muted bar before the tune: the clip is five bars with a bar of rest first, and so is what's judged.
        let mut led: Vec<NoteEvent> = vec![NoteEvent { muted: true, ..note(0, BAR, 60) }];
        led.extend(tune(0).into_iter().map(|n| NoteEvent { start: n.start + BAR, ..n }));
        assert_eq!(own_round(&led), Some(LineUp::LoopsFrom { start: 0, length: 5 * BAR }));
        let slots = slots_of(&led);
        assert_eq!(slots.len(), 10);
        assert!(slots[..2].iter().all(|slot| slot.iter().all(|t| *t == 0)) && slots[2].iter().any(|t| *t > 0));
        // Muted notes after it count the same way: they're in the clip.
        let mut trailed = tune(0);
        trailed.push(NoteEvent { muted: true, ..note(4 * BAR, BAR, 60) });
        assert_eq!(own_round(&trailed), Some(LineUp::LoopsFrom { start: 0, length: 5 * BAR }));
        // Nothing heard: no slots; no notes: nothing to go round.
        assert!(slots_of(&[NoteEvent { muted: true, ..note(0, BAR, 60) }]).is_empty());
        assert_eq!((own_round(&[]), slots_of(&[]).len()), (None, 0));

        // The other way, chords found for a tune: chords whose last one is held a 200th of a bar past the fourth line
        // go round every four bars under the tune, as the same chords cut on the line do.
        let mut held = chords(0);
        for n in held.iter_mut().filter(|n| n.start == 3 * BAR) {
            n.length += BAR / 200;
        }
        let under = Target::tune(&tune(0), Some(0..4 * BAR)).unwrap();
        let (exact, hangs) =
            (under.fit(&chords(0), Role::Chords, false).unwrap(), under.fit(&held, Role::Chords, false));
        let hangs = hangs.unwrap();
        assert_eq!((exact.best().0, hangs.best().0), (0, 0));
        assert!((exact.judged() - hangs.judged()).abs() < 0.01, "{} {}", exact.judged(), hangs.judged());
    }

    #[test]
    fn a_search_run_on_one_of_the_pools_own_threads_gives_the_same() {
        let parts: Vec<(String, Vec<NoteEvent>)> = (0..600).map(|i| (format!("wander {i}"), wandering(i))).collect();
        let mut book = SlotBook::default();
        for (hash, notes) in &parts {
            book.add(hash, &slots_of(notes));
        }
        let candidates: Vec<Candidate> = parts
            .iter()
            .zip(0i64..)
            .map(|((hash, _), id)| Candidate { id, hash, role: Role::Melody, cut: false, shape: None, lowest: 0 })
            .collect();
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        // One thread: a search that waited for the pool from the pool's own thread would never end.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let search = || {
            let mut notes = |id: i64, _: &str| parts.get(usize::try_from(id).ok()?).map(|(_, notes)| notes.clone());
            target.search(&candidates, &book, &mut notes, 50, &pool, &AtomicBool::new(false))
        };
        let outside = search();
        assert_eq!((outside.close, outside.found.len()), (300, 50));
        assert_eq!(pool.install(search), outside);
    }

    #[test]
    fn a_candidate_is_the_part_as_its_card_shows_it() {
        let source = crate::index::Source {
            path: r"D:\Packs\Long take.mid".into(),
            kind: crate::index::SourceKind::File,
            daw: "MIDI".into(),
            mtime: 1,
            size: 1,
            tempo: None,
        };
        let mut part = StoredPhrase {
            id: 7,
            source: std::sync::Arc::new(source),
            part: String::new(),
            clip: "Long take · bars 9–16".into(),
            role: Role::Melody,
            role_why: String::new(),
            your_role: None,
            bars: 8,
            notes: 16,
            lowest: 48,
            highest: 72,
            key: None,
            key_fact: None,
            chroma: [0; 12],
            chords: Vec::new(),
            features: Vec::new(),
            hash: "P".into(),
            members: String::new(),
            program: None,
            drums: false,
            track_name: String::new(),
            your_octave: None,
            shape: Some(5),
            cut: None,
        };
        // Before its key is read, a piece cut from a longer stretch is known by its name, as on its card.
        let candidate = Candidate::of(&part);
        assert_eq!((candidate.id, candidate.hash, candidate.shape, candidate.lowest), (7, "P", Some(5), 48));
        assert!(candidate.cut && candidate.role == Role::Melody);
        // What the key reading said, and what you said it is.
        part.cut = Some(false);
        part.your_role = Some(Role::Chords);
        let candidate = Candidate::of(&part);
        assert!(!candidate.cut && candidate.role == Role::Chords);
        part.drums = true;
        assert_eq!(Candidate::of(&part).role, Role::Drums);
    }

    #[test]
    fn a_loop_the_library_has_octaves_apart_is_given_as_the_copy_at_the_right_octave() {
        // The written tune (its middle is 75, where a tune plays) and the same an octave and two octaves down. The
        // lowest is listed first, so it stands for all three and is the one read.
        let down = |octaves: u8| -> Vec<NoteEvent> {
            tune(0).iter().map(|n| NoteEvent { key: n.key - 12 * octaves, ..*n }).collect()
        };
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        let alone = Library::of(vec![("two down".into(), Role::Melody, down(2))]);
        let (search, read) = alone.search(&target, 10, false);
        // With no other copy it's given with the octaves that bring it up to a tune's place.
        assert_eq!((search.found.len(), read), (1, 1));
        assert_eq!((search.found[0].sits.best().0, search.found[0].octaves, search.found[0].moved()), (0, 2, 24));
        // With the copies there, the one given is the one that needs no moving: the copy two octaves up from the
        // one read, not the nearer one that would still go up an octave.
        let library = Library::of(vec![
            ("two down".into(), Role::Melody, down(2)),
            ("one down".into(), Role::Melody, down(1)),
            ("written".into(), Role::Melody, tune(0)),
        ]);
        let (search, read) = library.search(&target, 10, false);
        assert_eq!((search.found.len(), read, search.close), (1, 2, 2));
        assert_eq!((search.found[0].at, search.found[0].moved()), (library.place("written"), 0));
        // Without the written one, the copy an octave down is the handier of the two left.
        let library =
            Library::of(vec![("two down".into(), Role::Melody, down(2)), ("one down".into(), Role::Melody, down(1))]);
        let (search, read) = library.search(&target, 10, false);
        assert_eq!((search.found.len(), read), (1, 2));
        assert_eq!((search.found[0].at, search.found[0].octaves), (library.place("one down"), 1));
    }

    /// A library whose book is out of step with its notes, so that the copy read first isn't the one that fits as
    /// it is: by its slots "a fourth up" looks like the copy that does. With them, whatever else is given.
    fn out_of_step(more: Vec<(String, Role, Vec<NoteEvent>)>) -> Library {
        let mut parts =
            vec![("a fourth up".to_string(), Role::Melody, tune(5)), ("written".to_string(), Role::Melody, tune(0))];
        parts.extend(more);
        let mut library = Library::of(parts);
        library.book = SlotBook::default();
        for (hash, _, notes) in &library.parts {
            let slots = match hash.as_str() {
                "a fourth up" => slots_of(&tune(0)),
                "written" => slots_of(&tune(5)),
                _ => slots_of(notes),
            };
            library.book.add(hash, &slots);
        }
        library
    }

    #[test]
    fn a_copy_that_fits_as_it_is_can_lift_its_loop_into_the_list() {
        // A tune that sits a hair under the written one (its first note a little short), as it is.
        let mut nearly = tune(0);
        nearly[0].length -= 8;
        let library = out_of_step(vec![("nearly".into(), Role::Melody, nearly.clone())]);
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        // The written tune's copy a fourth up is read first and sits better, but moved: by a hair it comes second.
        let judged = |notes: &[NoteEvent]| target.fit(notes, Role::Melody, false).unwrap().judged();
        assert!(judged(&tune(5)) < judged(&nearly) && judged(&nearly) < judged(&tune(0)));
        // With room for one in the list, it's the written tune as it is: its copy is looked for though the copy
        // read was below the list.
        let (search, read) = library.search(&target, 1, false);
        assert_eq!(read, 3);
        assert_eq!(search.found.iter().map(|f| f.at).collect::<Vec<_>>(), [library.place("written")]);
    }

    #[test]
    fn stopped_while_looking_for_handier_copies_the_list_is_still_whole() {
        let mut library = out_of_step(vec![("wander".into(), Role::Melody, wandering(1))]);
        // A newer search is wanted once the two loops are read.
        library.stop_after = 2;
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        let (search, read) = library.search(&target, 10, false);
        assert_eq!((read, search.close, search.stopped), (2, 2, false));
        // Both loops, the written tune as the copy that was read: moved, not the handiest.
        assert_eq!(search.found.len(), 2);
        assert_eq!(search.found[0].at, library.place("a fourth up"));
        assert_eq!(search.found[0].sits.best().0, -5);
    }

    #[test]
    fn a_loop_whose_picked_copy_cant_be_read_is_found_by_its_next_copy() {
        let mut library = Library::of(vec![
            ("written".into(), Role::Melody, tune(0)),
            ("a fourth up".into(), Role::Melody, tune(5)),
            ("a tone up".into(), Role::Melody, tune(2)),
            ("wander".into(), Role::Melody, wandering(1)),
        ]);
        library.unreadable.push("written");
        let target = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None).unwrap();
        let (search, read) = library.search(&target, 10, false);
        // The written tune's notes can't be read; the copy that needs moving least of the others is.
        assert_eq!((read, search.close, search.unread), (3, 2, 1));
        assert_eq!(search.found[0].at, library.place("a tone up"));
        assert_eq!(search.found[0].sits.best().0, -2);
        assert_eq!(search.found.len(), 2);
    }

    #[test]
    fn the_same_notes_read_two_ways_are_both_looked_at_and_listed_once() {
        // The same chords as a chord part and, a third up, as what the library calls an arp: how a part's chords
        // are read goes by its kind, so each gets the close look; they're one loop in the list.
        let library = Library::of(vec![
            ("as an arp, a third up".into(), Role::Arp, chords(3)),
            ("as chords".into(), Role::Chords, chords(0)),
            ("wander".into(), Role::Chords, wandering_chords(1)),
        ]);
        let target = Target::tune(&tune(0), None).unwrap();
        let (search, read) = library.search(&target, 10, false);
        assert_eq!((search.looked, read), (3, 3));
        assert_eq!(search.found.len(), 2);
        assert_eq!(search.found[0].at, library.place("as chords"));
        assert_eq!(search.found[0].sits.best().0, 0);
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
                .map(|((hash, _), id)| Candidate { id, hash, role, cut: false, shape: None, lowest: 0 })
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
