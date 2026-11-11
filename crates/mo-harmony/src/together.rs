//! How well a tune sits over chords, played together: the heart of "find chords for this melody" and "find a melody
//! for these chords".
//!
//! The C# app laid the two parts' notes over each other half a bar at a time and asked whether each note of one was
//! sounding in the other. This keeps that look (it's hard to beat, and it works where no chord can be named) and adds
//! what a musician asks: what is each note of the tune over the chord under it? The chords are read once by the chord
//! reader ([`Backing`]); each tune note is then the chord's root, third, fifth or seventh, a colour it names (the 9 of
//! Dm9) or could carry, or a note that doesn't belong (an F held over C major). Thirdly the tune as a whole is laid
//! beside the backing as a whole: a tune mostly on notes the backing never plays is in another key.
//!
//! Each of those gives a tune note (or the tune) some shares ([`Shares`]), and how the tune sits is the shares times
//! what each is worth ([`Sitting`]). The worths are fitted, not hand-set (`theorybench pairs --fit`): on songs with a
//! tune and an accompaniment written for each other, a stretch's own pair must sit better than its tune over another
//! song's accompaniment, another song's tune over its own, and its own pair moved to another key. Two things a
//! musician would expect didn't hold on the songs measured, so they're not here: a note counting more on a strong
//! beat (it measured worse), and a short note between beats being forgiven a clash (the fit marked it down like any
//! other note outside the chord). What was tried is in `docs/harmony/together-experiments.md`.
//!
//! The tune is tried moved up by each of 0 to 11 semitones ([`Sits::by_move`]), so a part saved in another key is
//! found, with the move that makes it fit.
//!
//! Which way round the search goes makes no difference: chords for a tune and a tune for chords both ask how the
//! tune's notes sit over the chords.

use crate::chord::{Chord, ToneRole};
use crate::chordread::{ChordContext, ChordReader, ChordReading};
use crate::keyfind::Span;
use crate::notes::{Grid, Note, Part};
use crate::pitch::{PerPitch, PitchClass};

mod first_look;
pub use first_look::{MOST_SLOTS, Outline, SLOT_FULL, SLOTS_VERSION, Slot, slots_of, slots_over};

/// Bumped when what [`sit`] finds changes (rule 7): a new number in [`Sitting::SHIPPED`], a new rule. The chord
/// reader's own version counts too: a [`Backing`] is its reading.
pub const TOGETHER_VERSION: u32 = 1;

/// The longest two loops are played round together before they meet again, in bars: past it, they play once through
/// the longer.
const MOST_BARS_ROUND: i64 = 32;

/// A note of the backing counts as played in a half bar when it sounds for at least this share of the time the half
/// bar's most-played note does (the C# app's share).
const PLAYED_SHARE: f64 = 0.25;

/// Below this share of the backing's most-played note, the backing hardly plays a note (the C# app's share).
const RARE: f64 = 0.1;

/// A tune counts as heard over the backing when at least this share of it (by weight) plays where the backing has
/// sound.
const LEAST_HEARD: f64 = 0.25;

/// How many things a tune note (or the tune) can be over the backing ([`Sitting::worths`]).
pub const WORTHS: usize = 20;

/// The first this many worths are a note's kinds: every heard note has just one, so their shares add up to 1.
pub const KINDS: usize = 7;

/// The worths' names, in [`Sitting::worths`]' order.
pub const WORTH_NAMES: [&str; WORTHS] = [
    "root",
    "third",
    "fifth",
    "colour",
    "beside",
    "outside",
    "no_chord",
    "near_played",
    "above1",
    "above2",
    "above3",
    "above4",
    "above5",
    "above6",
    "above7",
    "above8",
    "above9",
    "above10",
    "above11",
    "alike",
];

// Places in `Sitting::worths`.
const ROOT: usize = 0;
const THIRD: usize = 1;
const FIFTH: usize = 2;
const COLOUR: usize = 3;
const BESIDE: usize = 4;
const OUTSIDE: usize = 5;
const NO_CHORD: usize = 6;
const NEAR_PLAYED: usize = 7;
/// The first of the eleven: a half step above a played note.
const ABOVE: usize = 8;
const ALIKE: usize = 19;

/// What each thing a tune note can be over the backing is worth, and how much a note counts by its length: the
/// numbers the bench measures and fits (`theorybench pairs`). A note's worth is its kind's (the first seven) plus
/// what the backing's own notes say of it; the tune's as a whole (`alike`) is added once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sitting {
    /// A note of the chord as it's named: its root, its third (or a sus chord's 4th or 2nd), its fifth, and any other
    /// note it names: its seventh or sixth, its 9, 11 or 13 (the E of Dm9).
    pub root: f64,
    pub third: f64,
    pub fifth: f64,
    pub colour: f64,
    /// A note the chord doesn't name: one the backing plays somewhere (it's in the backing's scale), and one it
    /// hardly plays anywhere ([`RARE`]).
    pub beside: f64,
    pub outside: f64,
    /// A note where the backing sounds but no chord is read.
    pub no_chord: f64,
    /// By the half bar the note sounds in, whatever is struck when in it (an arpeggio's notes all count): added for a
    /// note the backing plays in that half bar ([`PLAYED_SHARE`]).
    pub near_played: f64,
    /// Added for a note the backing doesn't play in that half bar, by the notes it does: one for each number of
    /// semitones the note is above a played one (1 to 11), by how strongly that one is played. Only the rubs count
    /// (a half step, a whole step, a fourth, a tritone and the sevenths): a third, a fifth or a sixth above a played
    /// note is worth nothing, so a note that rubs one note (a B over C7's B flat) can't buy that back by sitting well
    /// over the others.
    pub above: [f64; 11],
    /// For the tune as a whole: how alike its notes are to the backing's, by how long each sounds (1 the same
    /// balance of notes).
    pub alike: f64,
    /// A note counts by how long it sounds, in beats: this many at least and at most.
    pub shortest: f64,
    pub longest: f64,
}

impl Sitting {
    /// The numbers the app uses, fitted on POP909-CL's folds 2 to 4 and held to the two rules [`Sitting::above`] and
    /// `colour` tell of: `theorybench pairs pop909cl.jsonl --folds 2,3,4 --fit 8000 --tie colour=fifth --without
    /// above3,above4,above7,above8,above9` prints them (tools/theorybench/README.md, "Tunes and chords together").
    pub const SHIPPED: Sitting = Sitting {
        root: 0.732,
        third: 0.960,
        fifth: 0.667,
        colour: 0.667,
        beside: 0.060,
        outside: -0.394,
        no_chord: 0.238,
        near_played: 0.068,
        above: [-0.592, -0.460, 0.0, 0.0, -0.393, -0.702, 0.0, 0.0, 0.0, -0.274, -0.258],
        alike: 0.335,
        shortest: 0.25,
        longest: 4.0,
    };

    /// The worths in order ([`WORTH_NAMES`]), as [`Shares`] counts them. The bench fits the numbers through this.
    pub const fn worths(&self) -> [f64; WORTHS] {
        let [a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11] = self.above;
        [
            self.root,
            self.third,
            self.fifth,
            self.colour,
            self.beside,
            self.outside,
            self.no_chord,
            self.near_played,
            a1,
            a2,
            a3,
            a4,
            a5,
            a6,
            a7,
            a8,
            a9,
            a10,
            a11,
            self.alike,
        ]
    }

    /// These numbers with other worths ([`Sitting::worths`]).
    pub const fn with_worths(self, w: [f64; WORTHS]) -> Sitting {
        let [
            root,
            third,
            fifth,
            colour,
            beside,
            outside,
            no_chord,
            near_played,
            a1,
            a2,
            a3,
            a4,
            a5,
            a6,
            a7,
            a8,
            a9,
            a10,
            a11,
            alike,
        ] = w;
        Sitting {
            root,
            third,
            fifth,
            colour,
            beside,
            outside,
            no_chord,
            near_played,
            above: [a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11],
            alike,
            ..self
        }
    }

    /// Sets one number by its name (a worth's, or `shortest` or `longest`); false when there's none of that name.
    pub fn set(&mut self, name: &str, value: f64) -> bool {
        if let Some(k) = WORTH_NAMES.iter().position(|n| *n == name) {
            let mut worths = self.worths();
            if let Some(w) = worths.get_mut(k) {
                *w = value;
            }
            *self = self.with_worths(worths);
            return true;
        }
        match name {
            "shortest" => self.shortest = value,
            "longest" => self.longest = value,
            _ => return false,
        }
        true
    }
}

impl Default for Sitting {
    fn default() -> Sitting {
        Sitting::SHIPPED
    }
}

/// One stretch of the backing with a chord the chart shows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stretch {
    /// From the backing's start, in ticks.
    start: i64,
    end: i64,
    /// The place in [`Sitting::worths`] of each pitch class's kind over the chord.
    kinds: PerPitch<usize>,
}

/// The kind of each pitch class over a chord, in a backing that plays each pitch class this much (`scale`): a place
/// in [`Sitting::worths`].
fn kinds_over(chord: Chord, scale: &PerPitch<f64>) -> PerPitch<usize> {
    // Whether the chord names a fifth of any kind (a flat or sharp one too).
    let names_a_fifth = [6, 7, 8].iter().any(|&i| chord.has(i) && chord.role(i) == ToneRole::Fifth);
    let mut out = PerPitch([OUTSIDE; 12]);
    for pc in PitchClass::ALL {
        let interval = i32::from(pc.above(chord.root()));
        out[pc] = match chord.role(interval) {
            // The root is the chord's whether or not the voicing plays it.
            ToneRole::Root => ROOT,
            // Only the third and fifth the chord names: an F over Csus2 is no third, an A flat over C7 no fifth.
            ToneRole::Third if chord.has(interval) => THIRD,
            ToneRole::Fifth if chord.has(interval) => FIFTH,
            // A shell voicing leaves the fifth out: the plain fifth is still the chord's, when it names no other.
            ToneRole::Fifth if interval == 7 && !names_a_fifth => FIFTH,
            _ if chord.has(interval) => COLOUR,
            _ if scale[pc] >= RARE => BESIDE,
            _ => OUTSIDE,
        };
    }
    out
}

/// What a pitch class is in one half bar of the backing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Near {
    played: bool,
    /// For a note not played there: how strongly the note it's 1 to 11 semitones above is played, if it is.
    above: [f64; 11],
}

/// The chords something is fitted to, read once: every tune tried against them is then quick.
#[derive(Clone, Debug, PartialEq)]
pub struct Backing {
    grid: Grid,
    /// The tick, in the notes' own time, its first bar starts at.
    origin: i64,
    /// How long it plays before it goes round again, in ticks: whole bars.
    length: i64,
    /// The chords, in order, none overlapping; between them there's no chord to name.
    stretches: Vec<Stretch>,
    /// How much of the backing's sounding time each pitch class takes, the most-played 1.
    scale: PerPitch<f64>,
    /// Half a bar at a time ([`half_bar`]): what each pitch class is there; none where nothing sounds.
    halves: Vec<Option<PerPitch<Near>>>,
}

impl Backing {
    /// Reads the chords of `parts` (the chords, the bass; a tune among them is heard as a tune) and keeps them to fit
    /// tunes to. None when the chord reader hears nothing in them.
    pub fn read(reader: &mut ChordReader, parts: &[Part], grid: Grid, span: Span) -> Option<Backing> {
        let reading = reader.read(parts, grid, &ChordContext::new(span))?;
        Some(Backing::of(&reading, parts))
    }

    /// The backing of a reading already made of `parts`: from the bar its first note is in to where its last ends.
    pub fn of(reading: &ChordReading, parts: &[Part]) -> Backing {
        Backing::over(reading, parts, reading.start, reading.end.saturating_sub(reading.start))
    }

    /// The backing of a reading already made of `parts`, over a stretch of time the caller gives: `length` ticks from
    /// `from`, a bar line. For bars picked from a song, where a bar the parts rest in, first or last, is still a bar
    /// of what goes round ([`Backing::of`] would start at the first note's bar and stop with the last note). What
    /// the parts play outside the stretch is left out.
    pub fn over(reading: &ChordReading, parts: &[Part], from: i64, length: i64) -> Backing {
        let origin = from;
        let grid = Grid { bar_line: origin, ..reading.grid };
        let bar = grid.bar();
        let span = length.max(1);
        // Whole bars: a stretch that ends mid-bar goes round at the next bar line.
        let length = span.saturating_add(bar - 1).div_euclid(bar).max(1).saturating_mul(bar);
        // How long each pitch class sounds in each half bar, and in all of it.
        let half = half_bar(grid);
        let count = usize::try_from(length.saturating_add(half - 1) / half).unwrap_or(0);
        let pitched = parts.iter().filter(|p| p.role.pitched()).flat_map(|p| p.notes);
        let times = times_in(pitched, origin, length, half, count);
        let mut whole = PerPitch([0i64; 12]);
        for time in &times {
            for pc in PitchClass::ALL {
                whole[pc] = whole[pc].saturating_add(time[pc]);
            }
        }
        let halves = times.iter().map(|time| Some(near_of(&strongest(time)?))).collect();
        let scale = strongest(&whole).unwrap_or_default();
        let stretches = reading
            .spans
            .iter()
            .filter(|s| s.end > s.start)
            .filter_map(|s| {
                let chord = s.shown()?.written;
                // Only what lies in the stretch.
                let (start, end) = (s.start.saturating_sub(origin).max(0), s.end.saturating_sub(origin).min(length));
                (end > start).then(|| Stretch { start, end, kinds: kinds_over(chord, &scale) })
            })
            .collect();
        Backing { grid, origin, length, stretches, scale, halves }
    }

    /// How long it plays before it goes round again, in ticks.
    pub const fn length(&self) -> i64 {
        self.length
    }

    /// The grid it was read on, its bar line where the backing starts.
    pub const fn grid(&self) -> Grid {
        self.grid
    }

    const fn half(&self) -> i64 {
        half_bar(self.grid)
    }
}

/// The stretch of time the backing's notes are taken together in: half a bar where a bar's beats halve (two beats in
/// 4/4, three eighths in 6/8), the whole bar where they don't (3/4), so it never runs over a bar line. The numbers
/// were fitted in 4/4 alone.
const fn half_bar(grid: Grid) -> i64 {
    let bar = grid.bar();
    if grid.bar_beats() % 2 == 0 && bar >= 2 { bar / 2 } else { bar }
}

/// How long each pitch class sounds in each `piece` ticks of the `length` ticks from `origin`, for `count` pieces.
/// What's played outside the stretch is left out. Where the pieces run on past the stretch (one that doesn't end on
/// a piece's line), it goes round again under them.
fn times_in<'a>(
    notes: impl Iterator<Item = &'a Note>,
    origin: i64,
    length: i64,
    piece: i64,
    count: usize,
) -> Vec<PerPitch<i64>> {
    let (length, piece) = (length.max(1), piece.max(1));
    let mut times = vec![PerPitch([0i64; 12]); count];
    let end = i64::try_from(count).unwrap_or(i64::MAX).saturating_mul(piece);
    for note in notes {
        let (from, to) = (note.start.saturating_sub(origin).max(0), note.end().saturating_sub(origin).min(length));
        let pc = note.pitch_class();
        // Once for each time the stretch goes round under the pieces.
        let mut lap = 0i64;
        while from < to && lap < end {
            let (mut at, to) = (from.saturating_add(lap), to.saturating_add(lap).min(end));
            while at < to {
                let k = at / piece;
                let until = (k + 1).saturating_mul(piece).min(to);
                if let Some(time) = usize::try_from(k).ok().and_then(|k| times.get_mut(k)) {
                    time[pc] = time[pc].saturating_add(until - at);
                }
                at = until;
            }
            lap = lap.saturating_add(length);
        }
    }
    times
}

/// How strongly each pitch class is played, the most-played 1; none where nothing sounds.
fn strongest(time: &PerPitch<i64>) -> Option<PerPitch<f64>> {
    let most = time.0.iter().copied().max().filter(|m| *m > 0)?;
    Some(PerPitch(time.0.map(|t| t as f64 / most as f64)))
}

/// What each pitch class is where the twelve are played this strongly: played itself ([`PLAYED_SHARE`] of the
/// strongest or more), or how strongly the notes 1 to 11 semitones under it are.
fn near_of(strength: &PerPitch<f64>) -> PerPitch<Near> {
    let mut out = PerPitch([Near::default(); 12]);
    for pc in PitchClass::ALL {
        let played = strength[pc] >= PLAYED_SHARE;
        let mut above = [0.0; 11];
        if !played {
            for (d, cell) in (1i32..).zip(above.iter_mut()) {
                let below = strength[pc.up(-d)];
                if below >= PLAYED_SHARE {
                    *cell = below;
                }
            }
        }
        out[pc] = Near { played, above };
    }
    out
}

/// How alike a tune's notes and a backing's are as wholes, by how long each pitch class sounds, with the tune moved
/// up by each of 0 to 11 semitones: 1 for the same notes in the same shares, 0 for none in common.
fn alike_by_move(tune_time: &PerPitch<f64>, scale: &PerPitch<f64>) -> [f64; 12] {
    let (mine, theirs): (f64, f64) = (tune_time.0.iter().map(|t| t * t).sum(), scale.0.iter().map(|t| t * t).sum());
    let mut out = [0.0; 12];
    for (s, alike) in (0i32..).zip(out.iter_mut()) {
        let on: f64 = tune_time.iter().map(|(pc, time)| time * scale[pc.up(s)]).sum();
        *alike = if mine > 0.0 && theirs > 0.0 { on / (mine * theirs).sqrt() } else { 0.0 };
    }
    out
}

/// How a tune and a backing are lined up to be played together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineUp {
    /// Two loops: both go round from their own first bars (the bar each one's first note is in) until they meet again,
    /// as a part from the library is tried against another.
    Loops {
        /// A tick where one of the tune's own bars starts (its grid's `bar_line`; 0 for a clip that starts on a bar).
        bar_line: i64,
    },
    /// Two loops, the tune's own round given: it goes round every `length` ticks from `start`, a bar line of its own,
    /// against the backing going round from its first bar. For bars picked from a song, where a bar the tune rests
    /// in, first or last, is still a bar of what goes round ([`LineUp::Loops`] would start at the first note's bar and
    /// go round after the last note's). The tune's notes lie within it.
    LoopsFrom { start: i64, length: i64 },
    /// Both as written, in the same time (a tick of the tune sounds with the same tick of the backing), played once:
    /// two parts of one song, or a tune already laid over the backing.
    AsWritten,
}

/// How much of a tune (by weight) is each thing a note can be over a backing ([`Sitting::worths`]), for each way of
/// moving it: how it sits is these times the worths, added up. The bench fits the worths through this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shares {
    /// With the tune moved up by 0 to 11 semitones.
    pub by_move: [[f64; WORTHS]; 12],
    /// The share of the tune (by weight) that plays where the backing has sound.
    pub heard: f64,
}

impl Shares {
    /// How the tune sits, with these numbers.
    pub fn sits(&self, how: &Sitting) -> Sits {
        let worths = how.worths();
        let by_move = self.by_move.map(|shares| shares.iter().zip(&worths).map(|(share, worth)| share * worth).sum());
        Sits { by_move, heard: self.heard }
    }
}

/// How a tune sits over a backing, for each way of moving it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sits {
    /// About 0 (it clashes) to about 1 (it sits as a tune written for the backing does), with the tune moved up by 0
    /// to 11 semitones. It isn't held to that range, so that the best stay in order: a tune all on thirds passes 1
    /// and one that clashes throughout goes under 0. Hold it to 0 to 1 only to draw it.
    pub by_move: [f64; 12],
    /// The share of the tune (by weight) that plays where the backing has sound.
    pub heard: f64,
}

impl Sits {
    /// The best move, as semitones from -6 to +5 (down a fourth is nearer than up a fifth), and how it sits there. On
    /// a near tie the smaller move wins.
    pub fn best(&self) -> (i32, f64) {
        let mut best = (0, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (s, &score) in (0i32..).zip(&self.by_move) {
            let semitones = if s <= 5 { s } else { s - 12 };
            let judged = score - 0.0005 * f64::from(semitones.abs());
            if judged > best.2 {
                best = (semitones, score, judged);
            }
        }
        (best.0, best.1)
    }

    /// How it sits moved by `semitones` (any whole number: octaves make no difference).
    pub fn moved(&self, semitones: i32) -> f64 {
        self.by_move.get(semitones.rem_euclid(12) as usize).copied().unwrap_or(0.0)
    }
}

/// How well `tune` sits over `backing`, the two lined up as `line_up` says. The tune's notes are in the backing's ticks
/// (its beats and bars are the backing's). None for no notes, or when the tune hardly plays where the backing has
/// sound.
pub fn sit(tune: &[Note], backing: &Backing, line_up: LineUp, how: &Sitting) -> Option<Sits> {
    shares(tune, backing, line_up, how).map(|shares| shares.sits(how))
}

/// What the tune's notes are over the backing ([`Shares`]); of `how`, only how much a note counts by its length
/// matters here. See [`sit`].
pub fn shares(tune: &[Note], backing: &Backing, line_up: LineUp, how: &Sitting) -> Option<Shares> {
    let (beat, bar, half) = (backing.grid.beat(), backing.grid.bar(), backing.half());
    let first = tune.iter().map(|n| n.start).min()?;
    let last = tune.iter().map(|n| n.end()).max()?;
    // Where the tune's time starts, how long it is before it goes round, and how long the two play together.
    let (tune_start, tune_length, round) = match line_up {
        LineUp::Loops { bar_line } => {
            // The bar the tune's first note is in, on the tune's own bar lines (a bar is as long as the backing's).
            let start = first.saturating_sub(bar_line).div_euclid(bar).saturating_mul(bar).saturating_add(bar_line);
            let length = last.saturating_sub(start).saturating_add(bar - 1).div_euclid(bar).max(1).saturating_mul(bar);
            (start, length, round_of(length, backing.length, bar))
        }
        LineUp::LoopsFrom { start, length } => {
            let length = length.max(1);
            (start, length, round_of(length, backing.length, bar))
        }
        // Once through: a note before the backing or after it plays over silence.
        LineUp::AsWritten => (backing.origin, i64::MAX, backing.length),
    };

    let loops = !matches!(line_up, LineUp::AsWritten);
    // A note is followed over the backing no further than this: past it (a drone, or a damaged file's note that never
    // ends) it would only hear the same again.
    let furthest = MOST_BARS_ROUND.saturating_mul(bar);

    let mut sums = [[0.0f64; WORTHS]; 12];
    // How much of the tune (by weight, where it's heard) is on each pitch class, unmoved.
    let mut tune_time = PerPitch([0.0f64; 12]);
    let (mut heard, mut all) = (0.0f64, 0.0f64);
    let mut turn = 0i64;
    while turn.saturating_mul(tune_length) < round {
        let offset = turn.saturating_mul(tune_length);
        turn += 1;
        for note in tune {
            let struck = note.start.saturating_sub(tune_start).saturating_add(offset);
            let sounds = note.duration().min(furthest);
            let end = struck.saturating_add(sounds).min(round);
            // As two loops, the tune stops where the round ends; as written, a note outside the backing is still
            // part of the tune, only not heard over it.
            let start = struck.max(0);
            if loops && start >= round {
                continue;
            }
            let weight = (note.duration() as f64 / beat as f64).clamp(how.shortest, how.longest.max(how.shortest));
            all += weight;
            if end <= start {
                continue;
            }
            let length = if loops { (end - start) as f64 } else { sounds as f64 };
            // Each chord and each half bar of the backing the note sounds over, as the backing goes round under it.
            let mut at = start;
            while at < end {
                let within = at.rem_euclid(backing.length);
                let lap = at - within;
                let k = backing.stretches.partition_point(|s| s.end <= within);
                let stretch = backing.stretches.get(k).filter(|s| s.start <= within);
                // To the end of this chord, or over no chord to the next one, and no further than this half bar.
                let until = match stretch {
                    Some(s) => s.end,
                    None => backing.stretches.get(k).map_or(backing.length, |s| s.start),
                }
                .min(backing.length)
                .min((within / half + 1).saturating_mul(half))
                .saturating_add(lap)
                .min(end)
                .max(at + 1);
                let near = usize::try_from(within / half).ok().and_then(|h| backing.halves.get(h).copied().flatten());
                let share = weight * (until - at) as f64 / length;
                at = until;
                // Heard where there's a chord or sound.
                if stretch.is_none() && near.is_none() {
                    continue;
                }
                heard += share;
                let own = note.pitch_class();
                tune_time[own] += share;
                for (s, sum) in (0i32..).zip(sums.iter_mut()) {
                    let pc = own.up(s);
                    let mut add = |place: usize, much: f64| {
                        if let Some(cell) = sum.get_mut(place) {
                            *cell += much;
                        }
                    };
                    add(stretch.map_or(NO_CHORD, |s| s.kinds[pc]), share);
                    if let Some(near) = near {
                        let near = near[pc];
                        if near.played {
                            add(NEAR_PLAYED, share);
                        }
                        for (d, strength) in near.above.iter().enumerate().filter(|(_, s)| **s > 0.0) {
                            add(ABOVE + d, share * strength);
                        }
                    }
                }
            }
        }
    }
    if all <= 0.0 || heard < LEAST_HEARD * all {
        return None;
    }
    let mut by_move = sums.map(|sum| sum.map(|share| share / heard));
    // The tune as a whole beside the backing as a whole.
    for (shares, alike) in by_move.iter_mut().zip(alike_by_move(&tune_time, &backing.scale)) {
        if let Some(cell) = shares.get_mut(ALIKE) {
            *cell = alike;
        }
    }
    Some(Shares { by_move, heard: (heard / all).min(1.0) })
}

/// How long two loops play together: until they meet again, or once through the longer when that's past
/// [`MOST_BARS_ROUND`].
fn round_of(a: i64, b: i64, bar: i64) -> i64 {
    // Too long to count (a damaged file's stray note far away) is past it too.
    match (a / gcd(a, b)).checked_mul(b) {
        Some(both) if both <= MOST_BARS_ROUND.saturating_mul(bar) => both,
        _ => a.max(b),
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::notes::Role;
    use crate::pitch::PcSet;

    const BEAT: i64 = 96;
    const BAR: i64 = 4 * BEAT;

    /// Notes struck together at a beat (counted from 0) and held for `beats`.
    fn chord(beat: i64, beats: i64, keys: &[u8]) -> Vec<Note> {
        keys.iter().map(|&key| Note { start: beat * BEAT, length: beats * BEAT, key, velocity: 50_000 }).collect()
    }

    /// A tune: each note's start and length in half beats, and its key.
    fn tune(notes: &[(i64, i64, u8)]) -> Vec<Note> {
        notes
            .iter()
            .map(|&(start, length, key)| Note {
                start: start * BEAT / 2,
                length: length * BEAT / 2,
                key,
                velocity: 50_000,
            })
            .collect()
    }

    fn backing(chords: &[Vec<Note>]) -> Backing {
        let notes = chords.concat();
        let parts = [Part { role: Role::Harmony, notes: &notes }];
        Backing::read(&mut ChordReader::new(), &parts, Grid::four_four(BEAT as u32), Span::Loop).unwrap()
    }

    /// Am7, Dm7, G7, Cmaj7, a bar each, root in the bass.
    fn turnaround() -> Backing {
        backing(&[
            chord(0, 4, &[45, 60, 64, 67]),
            chord(4, 4, &[50, 60, 65, 69]),
            chord(8, 4, &[43, 59, 62, 65]),
            chord(12, 4, &[48, 59, 64, 67]),
        ])
    }

    /// Dm9, G13, Cmaj9, Cmaj9 as a pianist's left hand plays them over a bass: no root in the hand.
    fn rootless() -> Backing {
        backing(&[
            chord(0, 4, &[38, 53, 57, 60, 64]),
            chord(4, 4, &[43, 53, 57, 59, 64]),
            chord(8, 8, &[36, 52, 55, 59, 62]),
        ])
    }

    /// A 12-bar blues in C cut to four bars: C7, F7, C7, G7.
    fn blues() -> Backing {
        backing(&[
            chord(0, 4, &[36, 52, 55, 58]),
            chord(4, 4, &[41, 51, 57, 60]),
            chord(8, 4, &[36, 52, 55, 58]),
            chord(12, 4, &[43, 53, 59, 62]),
        ])
    }

    fn sits(line: &[Note], backing: &Backing) -> Sits {
        sit(line, backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap()
    }

    /// The numbers named in SET ("outside=-0.4,longest=2"), over the shipped ones: for trying a fit on the hand cases.
    fn from_env() -> Sitting {
        let mut how = Sitting::SHIPPED;
        if let Ok(set) = std::env::var("SET") {
            for pair in set.split(',') {
                let (name, value) = pair.split_once('=').unwrap();
                assert!(how.set(name.trim(), value.trim().parse().unwrap()), "{name} is none of the numbers");
            }
        }
        how
    }

    /// Prints how the hand cases sit, with the shipped numbers or SET's (the line `theorybench pairs --fit` prints):
    /// `SET="..." cargo test -p mo-harmony together::tests::show -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the hand cases, for trying a fit"]
    fn show_the_hand_cases() {
        let how = from_env();
        for (name, line, backing) in cases() {
            let sits = sit(&line, &backing, LineUp::Loops { bar_line: 0 }, &how).unwrap();
            let row: Vec<String> = sits.by_move.iter().map(|v| format!("{v:.2}")).collect();
            println!("{name:32} best {:+} | {}", sits.best().0, row.join(" "));
        }
    }

    /// The hand cases: a name, a tune and what it's played over.
    fn cases() -> Vec<(&'static str, Vec<Note>, Backing)> {
        vec![
            ("guide tones, turnaround", guide_tones(), turnaround()),
            ("scale tune, turnaround", scale_tune(), turnaround()),
            ("ninths and thirteenths, rootless", colours(), rootless()),
            ("blue third, blues", blue(63), blues()),
            ("major seventh on C7, blues", blue(71), blues()),
            ("held E on Cmaj7 (third)", held(76), turnaround()),
            ("held D on Cmaj7 (ninth)", held(74), turnaround()),
            ("held F on Cmaj7 (avoid)", held(77), turnaround()),
            ("passing F on Cmaj7", passing(), turnaround()),
        ]
    }

    /// Thirds and sevenths of Am7, Dm7, G7, Cmaj7: C G, F C, B F, E B.
    fn guide_tones() -> Vec<Note> {
        tune(&[(0, 4, 72), (4, 4, 79), (8, 4, 77), (12, 4, 72), (16, 4, 71), (20, 4, 77), (24, 4, 76), (28, 4, 71)])
    }

    /// A tune in C in quarters and eighths, on chord notes with steps between them.
    fn scale_tune() -> Vec<Note> {
        tune(&[
            (0, 2, 76),
            (2, 1, 74),
            (3, 1, 72),
            (4, 4, 69),
            (8, 2, 77),
            (10, 1, 76),
            (11, 1, 74),
            (12, 4, 72),
            (16, 2, 74),
            (18, 1, 72),
            (19, 1, 71),
            (20, 4, 67),
            (24, 2, 76),
            (26, 1, 74),
            (27, 1, 72),
            (28, 4, 67),
        ])
    }

    /// E over Dm9 (its 9), E over G13 (its 13), D then B over Cmaj9 (its 9 and 7).
    fn colours() -> Vec<Note> {
        tune(&[(0, 8, 76), (8, 8, 76), (16, 8, 74), (24, 8, 71)])
    }

    /// Over the blues: C, then `key` held half a bar over each C7, G between.
    fn blue(key: u8) -> Vec<Note> {
        tune(&[(0, 4, 72), (4, 4, key), (8, 8, 72), (16, 4, 67), (20, 4, key), (24, 8, 67)])
    }

    /// C over Am7, then `key` held for all of the Cmaj7 bar.
    fn held(key: u8) -> Vec<Note> {
        tune(&[(0, 8, 72), (24, 8, key)])
    }

    /// C over Am7; over Cmaj7, E, an F for an eighth between beats, then G.
    fn passing() -> Vec<Note> {
        tune(&[(0, 8, 72), (24, 3, 76), (27, 1, 77), (28, 4, 79)])
    }

    // The hand cases the fitted numbers are held to. The songs the numbers are fitted on are pop, where a tune seldom
    // sits on a chord's 9 or 13 and never on a blue note, so what jazz, gospel and blues need is checked here.

    #[test]
    fn a_tune_written_for_the_chords_sits_best_where_it_is() {
        for (line, backing) in [(guide_tones(), turnaround()), (scale_tune(), turnaround())] {
            let sits = sits(&line, &backing);
            let (semitones, score) = sits.best();
            assert_eq!(semitones, 0, "{:?}", sits.by_move);
            assert!(score > 0.9, "{score}");
            assert!(sits.heard > 0.99);
            // No other key comes near.
            assert!((1..12).all(|s| sits.moved(s) < 0.5), "{:?}", sits.by_move);
        }
    }

    #[test]
    fn a_tune_in_another_key_is_found_with_its_move() {
        let backing = turnaround();
        // The guide tones written a major third up: they fit moved down four.
        let up: Vec<Note> = guide_tones().iter().map(|n| Note { key: n.key + 4, ..*n }).collect();
        let moved = sits(&up, &backing);
        assert_eq!(moved.best().0, -4);
        assert_eq!(moved.moved(-4), moved.moved(8));
        assert_eq!(moved.moved(-4), sits(&guide_tones(), &backing).moved(0));
        // An octave makes no difference.
        let high: Vec<Note> = guide_tones().iter().map(|n| Note { key: n.key + 12, ..*n }).collect();
        assert_eq!(sits(&high, &backing), sits(&guide_tones(), &backing));
    }

    #[test]
    fn a_tune_on_ninths_and_thirteenths_sits_over_rootless_chords() {
        // What the C# app got wrong most (Solomon, 2026-11-01): the colours of jazz and gospel chords.
        let sits = sits(&colours(), &rootless());
        assert!(sits.moved(0) > 0.9, "{:?}", sits.by_move);
        // Up a fourth the same tune is the chords' fifths and thirds, which sits as well; nothing else does.
        assert!((1..12).filter(|s| *s != 5).all(|s| sits.moved(s) < 0.5), "{:?}", sits.by_move);
    }

    #[test]
    fn a_blue_third_sits_better_than_a_major_seventh_over_a_dominant_chord() {
        let backing = blues();
        let (blue_third, major_seventh) = (sits(&blue(63), &backing), sits(&blue(71), &backing));
        assert_eq!(blue_third.best().0, 0, "{:?}", blue_third.by_move);
        assert!(blue_third.moved(0) > major_seventh.moved(0), "{} {}", blue_third.moved(0), major_seventh.moved(0));
    }

    #[test]
    fn a_held_note_counts_by_what_it_is_over_the_chord() {
        let backing = turnaround();
        let (third, ninth, eleventh) = (
            sits(&held(76), &backing).moved(0),
            sits(&held(74), &backing).moved(0),
            sits(&held(77), &backing).moved(0),
        );
        // E over Cmaj7 is its third; D is beside it (a 9 the chord doesn't name); F rubs against the E.
        assert!(third > 0.9, "{third}");
        assert!(third > ninth + 0.3 && ninth > eleventh + 0.2, "{third} {ninth} {eleventh}");
        // The same F for an eighth, between an E and a G, hardly counts: a note counts by how long it sounds.
        assert!(sits(&passing(), &backing).moved(0) > 0.9);
    }

    #[test]
    fn two_loops_go_round_from_their_first_bars() {
        let backing = turnaround();
        assert_eq!(backing.length(), 4 * BAR);
        assert_eq!(backing.grid(), Grid::four_four(BEAT as u32));
        // A one-bar tune on E plays over all four chords, a bar each: it's a quarter each of Am7's fifth, a note
        // beside Dm7 (the 9 it doesn't name) a whole step above the D played, a note beside G7 a whole step above
        // its D, and Cmaj7's third.
        let one_bar = tune(&[(0, 8, 76)]);
        let shares = shares(&one_bar, &backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap();
        let unmoved = shares.by_move[0];
        assert_eq!((unmoved[FIFTH], unmoved[BESIDE], unmoved[THIRD]), (0.25, 0.5, 0.25));
        assert_eq!(unmoved[NEAR_PLAYED], 0.5, "the backing plays an E under two of the four");
        assert_eq!(unmoved[ABOVE + 1], 0.5, "a whole step above a played D, twice");
        assert_eq!(shares.heard, 1.0);
        // A tune that starts in a later bar still starts with the backing's first bar.
        let later = tune(&[(64, 8, 76)]);
        assert_eq!(
            super::shares(&later, &backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap().by_move,
            shares.by_move
        );
        // A three-bar tune and the four-bar backing meet again after twelve bars: each bar of one over each of the other.
        assert_eq!(round_of(3 * BAR, 4 * BAR, BAR), 12 * BAR);
        // Past 32 bars round, they play once through the longer.
        assert_eq!(round_of(7 * BAR, 16 * BAR, BAR), 16 * BAR);
        assert_eq!(round_of(BAR, BAR, BAR), BAR);
    }

    #[test]
    fn as_written_the_two_play_in_the_same_time_once() {
        // The backing's first bar is empty: its chords start in bar 2 (Dm7, G7, Cmaj7).
        let backing =
            backing(&[chord(4, 4, &[50, 60, 65, 69]), chord(8, 4, &[43, 59, 62, 65]), chord(12, 4, &[48, 59, 64, 67])]);
        assert_eq!(backing.length(), 3 * BAR);
        // F over Dm7, B over G7, E over Cmaj7, where they're written.
        let line = tune(&[(8, 8, 77), (16, 8, 71), (24, 8, 76)]);
        let written = sit(&line, &backing, LineUp::AsWritten, &Sitting::SHIPPED).unwrap();
        assert_eq!(written.best().0, 0);
        assert!(written.moved(0) > 0.9, "{:?}", written.by_move);
        assert_eq!(written.heard, 1.0);
        // A note written before the backing starts is part of the tune, but isn't heard over it.
        let early = tune(&[(0, 8, 73), (8, 8, 77), (16, 8, 71), (24, 8, 76)]);
        let with_early = sit(&early, &backing, LineUp::AsWritten, &Sitting::SHIPPED).unwrap();
        assert_eq!(with_early.heard, 0.75);
        assert!((with_early.moved(0) - written.moved(0)).abs() < 0.05, "only the tune's balance of notes moves");
        // A tune written wholly outside the backing isn't heard at all.
        assert!(sit(&tune(&[(0, 8, 72)]), &backing, LineUp::AsWritten, &Sitting::SHIPPED).is_none());
        assert!(sit(&tune(&[(40, 8, 72)]), &backing, LineUp::AsWritten, &Sitting::SHIPPED).is_none());
    }

    #[test]
    fn where_no_chord_is_read_the_notes_sounding_stand_in() {
        // Two bare notes a fifth apart: a backing whatever the reader makes of them.
        let backing = backing(&[chord(0, 8, &[48, 55])]);
        let how = Sitting::SHIPPED;
        // On a note that's played beats a half step above one.
        let on = sit(&tune(&[(0, 16, 72)]), &backing, LineUp::Loops { bar_line: 0 }, &how).unwrap();
        assert!(on.moved(0) > on.moved(1) + 0.3, "{:?}", on.by_move);
        assert!(sit(&[], &backing, LineUp::Loops { bar_line: 0 }, &how).is_none());
    }

    #[test]
    fn a_note_is_a_kind_over_a_chord() {
        // Dm9 in a backing that plays the notes of C major: D E F G A B C, and no others.
        let mut scale = PerPitch([0.0; 12]);
        for pc in [0, 2, 4, 5, 7, 9, 11] {
            scale[PitchClass::new(pc)] = 1.0;
        }
        let kinds = kinds_over(Chord::parse("Dm9").unwrap(), &scale);
        let kind = |pc: i32| kinds[PitchClass::new(pc)];
        assert_eq!((kind(2), kind(5), kind(9)), (ROOT, THIRD, FIFTH));
        assert_eq!((kind(0), kind(4)), (COLOUR, COLOUR), "its seventh and its ninth");
        assert_eq!((kind(7), kind(11)), (BESIDE, BESIDE), "G and B: not named, but in the backing's scale");
        assert_eq!((kind(1), kind(6), kind(10)), (OUTSIDE, OUTSIDE, OUTSIDE));
    }

    #[test]
    fn only_the_third_and_fifth_a_chord_names_are_its_own() {
        // Every note in the backing's scale, so a note the chord doesn't name is beside it.
        let all = PerPitch([1.0; 12]);
        let over =
            |notes: &[i32]| kinds_over(Chord::new(PitchClass::new(0), PcSet::of(notes), PitchClass::new(0)), &all);
        let kind = |kinds: &PerPitch<usize>, pc: i32| kinds[PitchClass::new(pc)];
        // A Cmaj7 shell, C E B: the G it leaves out is still its fifth; an A flat or a G flat isn't.
        let shell = over(&[0, 4, 11]);
        assert_eq!((kind(&shell, 7), kind(&shell, 8), kind(&shell, 6)), (FIFTH, BESIDE, BESIDE));
        // C7 with a flat fifth: G flat is its fifth, and G is none of its notes.
        let flat = over(&[0, 4, 6, 10]);
        assert_eq!((kind(&flat, 6), kind(&flat, 7)), (FIFTH, BESIDE));
        // C7 with a sharp fifth the same.
        let sharp = over(&[0, 4, 8, 10]);
        assert_eq!((kind(&sharp, 8), kind(&sharp, 7)), (FIFTH, BESIDE));
        // Csus2, C D G: the D stands where its third would; an F, an E and an E flat are no third of it.
        let sus = over(&[0, 2, 7]);
        assert_eq!((kind(&sus, 2), kind(&sus, 5), kind(&sus, 4), kind(&sus, 3)), (THIRD, BESIDE, BESIDE, BESIDE));
        // The root is the chord's even from a rootless voicing's notes.
        assert_eq!(kind(&shell, 0), ROOT);
    }

    #[test]
    fn two_loops_start_from_the_tunes_own_bar_lines() {
        // The same music twice: once with bars at 0, once with everything 100 ticks later and bars there.
        let chords = [chord(0, 4, &[45, 60, 64, 67]), chord(4, 4, &[50, 60, 65, 69])];
        let line = tune(&[(0, 8, 72), (8, 8, 77)]);
        let plain = sit(&line, &backing(&chords), LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap();
        assert_eq!(plain.best().0, 0);
        let later =
            |notes: &[Note]| -> Vec<Note> { notes.iter().map(|n| Note { start: n.start + 100, ..*n }).collect() };
        let notes = later(&chords.concat());
        let parts = [Part { role: Role::Harmony, notes: &notes }];
        let grid = Grid { bar_line: 100, ..Grid::four_four(BEAT as u32) };
        let moved = Backing::read(&mut ChordReader::new(), &parts, grid, Span::Loop).unwrap();
        // A tune whose bars are 100 ticks in too, and one from a clip whose bars start at 0: both sit as before.
        for (tune, bar_line) in [(later(&line), 100), (later(&line), 100 - 3 * BAR), (line.clone(), 0)] {
            let sits = sit(&tune, &moved, LineUp::Loops { bar_line }, &Sitting::SHIPPED).unwrap();
            assert_eq!(sits.by_move, plain.by_move, "bar line {bar_line}");
        }
    }

    #[test]
    fn bars_a_part_rests_in_are_still_bars_of_what_goes_round() {
        // A tune that plays over Am7, Dm7 and G7 and rests in the fourth bar, over the four-bar turnaround.
        let backing = turnaround();
        let line = tune(&[(0, 8, 72), (8, 8, 77), (16, 8, 71)]);
        // Taken from its notes it's three bars long: its second time round starts over Cmaj7, a bar early.
        let from_notes = sit(&line, &backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap();
        // Told it's four bars long it meets the same chords every time round, as it does written out once.
        let given = sit(&line, &backing, LineUp::LoopsFrom { start: 0, length: 4 * BAR }, &Sitting::SHIPPED).unwrap();
        let written = sit(&line, &backing, LineUp::AsWritten, &Sitting::SHIPPED).unwrap();
        for (given, written) in given.by_move.iter().zip(&written.by_move) {
            assert!((given - written).abs() < 1e-9, "{given} {written}");
        }
        assert_eq!(given.best().0, 0);
        assert!(given.best().1 > from_notes.best().1 + 0.2, "{given:?} {from_notes:?}");
        // The same for a tune that rests in its first bar: it starts where it's told, not at its first note's bar.
        let late = tune(&[(8, 8, 77), (16, 8, 71), (24, 8, 76)]);
        let given = sit(&late, &backing, LineUp::LoopsFrom { start: 0, length: 4 * BAR }, &Sitting::SHIPPED).unwrap();
        let from_notes = sit(&late, &backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap();
        assert_eq!(given.best().0, 0);
        assert!(given.best().1 > from_notes.best().1 + 0.2, "{given:?} {from_notes:?}");

        // A backing that rests in its first bar: Dm7, G7, Cmaj7 from bar 2 of four picked bars.
        let chords = [chord(4, 4, &[50, 60, 65, 69]), chord(8, 4, &[43, 59, 62, 65]), chord(12, 4, &[48, 59, 64, 67])];
        let notes = chords.concat();
        let parts = [Part { role: Role::Harmony, notes: &notes }];
        let reading =
            ChordReader::new().read(&parts, Grid::four_four(BEAT as u32), &ChordContext::new(Span::Window)).unwrap();
        let from_notes = Backing::of(&reading, &parts);
        let picked = Backing::over(&reading, &parts, 0, 4 * BAR);
        assert_eq!((from_notes.length(), picked.length()), (3 * BAR, 4 * BAR));
        assert_eq!(picked.grid().bar_line, 0);
        // A four-bar tune written for the picked bars (anything over the rest, then the chords' thirds).
        let line = tune(&[(0, 8, 72), (8, 8, 77), (16, 8, 71), (24, 8, 76)]);
        let over_picked = sits(&line, &picked);
        assert_eq!(over_picked.best().0, 0);
        assert!(over_picked.best().1 > sits(&line, &from_notes).best().1 + 0.2);
        // What the parts play outside the stretch is left out: the last two bars alone hold G7 and Cmaj7.
        let last_two = Backing::over(&reading, &parts, 2 * BAR, 2 * BAR);
        assert_eq!(last_two.length(), 2 * BAR);
        assert_eq!(sits(&tune(&[(0, 8, 71), (8, 8, 76)]), &last_two).best().0, 0);
    }

    #[test]
    fn a_damaged_tune_neither_hangs_nor_overflows() {
        // A note that never ends and one struck far away, as a damaged file gives them.
        let backing = turnaround();
        let mut line = guide_tones();
        line.push(Note { start: 0, length: i64::MAX, key: 64, velocity: 50_000 });
        line.push(Note { start: 1 << 62, length: 1 << 40, key: 60, velocity: 50_000 });
        line.push(Note { start: i64::MIN, length: 96, key: 60, velocity: 50_000 });
        let lengths =
            [LineUp::LoopsFrom { start: i64::MIN, length: i64::MAX }, LineUp::LoopsFrom { start: 0, length: 0 }];
        let line_ups = [LineUp::Loops { bar_line: 0 }, LineUp::Loops { bar_line: i64::MAX }, LineUp::AsWritten];
        for line_up in line_ups.into_iter().chain(lengths) {
            let sits = sit(&line, &backing, line_up, &Sitting::SHIPPED);
            assert!(sits.is_none_or(|s| s.by_move.iter().all(|v| v.is_finite())), "{line_up:?}");
        }
        // Two lengths too long to play round until they meet play once through the longer.
        assert_eq!(round_of(i64::MAX - 1, 4 * BAR, BAR), i64::MAX - 1);
    }

    #[test]
    fn the_backings_notes_are_taken_together_within_a_bar() {
        let grid = |ticks_per_beat, beats_per_bar| Grid { ticks_per_beat, beats_per_bar, bar_line: 0 };
        assert_eq!(half_bar(grid(96, 4)), 192, "4/4: two beats");
        assert_eq!(half_bar(grid(48, 6)), 144, "6/8: three eighths");
        assert_eq!(half_bar(grid(96, 3)), 288, "3/4: the bar, so it never runs over a bar line");
        assert_eq!(half_bar(grid(96, 2)), 96, "2/4: a beat");
        assert_eq!(half_bar(grid(1, 1)), 1);
    }

    #[test]
    fn the_best_tunes_stay_in_order_past_one() {
        // All on thirds and all on roots both sit as a tune written for the chords does: the thirds still come first.
        let backing = turnaround();
        let thirds = tune(&[(0, 8, 72), (8, 8, 77), (16, 8, 71), (24, 8, 76)]);
        let roots = tune(&[(0, 8, 69), (8, 8, 74), (16, 8, 67), (24, 8, 72)]);
        let sits =
            |line: &[Note]| sit(line, &backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap().moved(0);
        assert!(sits(&roots) > 0.95, "{}", sits(&roots));
        assert!(sits(&thirds) > sits(&roots) + 0.05, "{} {}", sits(&thirds), sits(&roots));
    }

    #[test]
    fn the_numbers_go_by_name_and_in_order() {
        let mut how = Sitting::SHIPPED;
        assert_eq!(how.with_worths(how.worths()), how);
        let names: Vec<f64> = WORTH_NAMES.iter().enumerate().map(|(k, _)| k as f64).collect();
        let numbered = how.with_worths(names.clone().try_into().unwrap());
        assert_eq!(numbered.worths().to_vec(), names);
        assert_eq!((numbered.root, numbered.no_chord, numbered.near_played, numbered.alike), (0.0, 6.0, 7.0, 19.0));
        assert_eq!((numbered.above[0], numbered.above[10]), (8.0, 18.0));
        for (k, name) in WORTH_NAMES.iter().enumerate() {
            assert!(how.set(name, k as f64 + 100.0));
            assert_eq!(how.worths()[k], k as f64 + 100.0, "{name}");
        }
        assert!(how.set("longest", 2.0) && how.set("shortest", 0.5));
        assert_eq!((how.shortest, how.longest), (0.5, 2.0));
        assert!(!how.set("loud", 1.0));
        // What the fit holds the shipped numbers to: a colour the chord names counts as its fifth does, and only
        // the rubs count against a note the backing doesn't play.
        let shipped = Sitting::SHIPPED;
        assert_eq!(shipped.colour, shipped.fifth);
        assert!(shipped.above.iter().all(|a| *a <= 0.0));
        assert_eq!([2, 3, 6, 7, 8].map(|d| shipped.above[d]), [0.0; 5], "thirds, the fifth and sixths");
    }

    #[test]
    fn the_best_move_is_the_nearest_on_a_tie() {
        let mut by_move = [0.2; 12];
        by_move[7] = 0.9;
        by_move[5] = 0.9;
        // Up a fourth and down a fourth (up a fifth) sit the same: -5 and +5 are as near, and the first found stays.
        assert_eq!(Sits { by_move, heard: 1.0 }.best(), (5, 0.9));
        by_move[11] = 0.9;
        assert_eq!(Sits { by_move, heard: 1.0 }.best(), (-1, 0.9));
        assert_eq!(Sits { by_move: [0.0; 12], heard: 1.0 }.best(), (0, 0.0));
    }
}
