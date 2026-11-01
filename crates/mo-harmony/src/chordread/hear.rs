//! Hearing the notes, the first of the chord reader's three steps. It needs no chord: it says where chords can change
//! and how strong each of those places is, which notes are the bass, the chords and the tune, and how long each note
//! sounds where, all as whole numbers of ticks.
//!
//! - **Units.** Chords change only where a unit starts: every half beat in simple metre (2/4, 3/4, 4/4, 5/4, 7/8...),
//!   every grid beat in compound metre (6, 9 or 12 beats a bar: 6/8, 9/8, 12/8, and 6/4 too). A felt beat is two units
//!   in simple metre and three in compound. Each unit's start has a [`Level`]: a bar's start, the middle of a bar, a
//!   felt beat, or between beats.
//! - **Snap.** A note struck just before a unit starts (at most a quarter of a unit before) is heard on it: that's a
//!   player's timing, not a new rhythm.
//! - **Streams.** Bass parts are the bass, melody parts the tune, and every other pitched part the chords
//!   ([`Stream`]). In a bar where no bass part plays, the lowest chord note of a unit is its bass when the next chord
//!   note up of another pitch class is at least a fifth above it ([`BASS_GAP`]); its octave doubles go with it.
//!   With only its own octaves above it, it stays the bass if it was the unit before's.
//! - **Broken chords.** In a felt beat where a chord part plays its notes one at a time (an arpeggio, an Alberti
//!   bass), its short notes are heard to the end of the felt beat, as a player's pedal holds them, and are never the
//!   bass.
//! - **Held over.** The end of a note that rings a little past a bar line or the middle of a bar, after sounding for a
//!   felt beat or more before it, is held over: legato and pedal blur, kept apart from what the next chord plays.
//! - **Masses.** For each unit and pitch class, the longest time any one note of it sounds in the unit, in each
//!   stream, added up from the first unit (prefix sums), so what sounds in any stretch is a subtraction ([`Mass`]).
//! - **Loops.** A loop is heard with its last bar copied in front, so its first chord has the one before it, as when
//!   it goes round; only the loop's own units are reported.
//! - **Lines and shapes.** The bass, the top line of each tune and of the chords, and each broken chord are lines;
//!   their notes that move like passing notes, neighbours, approaches, anticipations, suspensions or appoggiaturas
//!   are marked ([`Shaped`]), with the notes they hang on, for the reader to test against each chord.
//! - **Pushes.** A chord struck just before a felt beat and held over it, with the bass on the beat (or pushed too, or
//!   silent), may be counted from the beat ([`Push`]).
//!
//! Nothing reads how high a note is, only the gaps between notes, and every count is a whole number. So moving the
//! notes moves every answer to its new note exactly; the order of the notes, a drum part, or the notes doubled an
//! octave up change nothing; and the same music at another resolution (ticks a beat) is heard the same, its counts in
//! proportion.

use std::ops::Range;

use crate::keyfind::Span;
use crate::notes::{self, Grid, Part, Role};
use crate::pitch::{PcSet, PerPitch, PitchClass};

mod lines;
mod push;

pub use lines::{Line, Shape, Shaped};
pub use push::Push;

/// The most units heard: about an hour of 4/4 at 120 beats a minute. Only a damaged file has notes further apart: then
/// the stretch with the most notes is heard, by the key finder's rule, and [`Hearing::truncated`] says so.
pub const MOST_UNITS: usize = 1 << 14;

/// In a bar where no bass part plays, the lowest chord note sounding in a unit is its bass when the next note up of
/// another pitch class is at least this many semitones above it (a fifth). Only the gap counts, never how high the
/// notes are: it takes a left hand's root under its chord (D3 under C4 F4), a shell's root and the Kenny Barron
/// voicing's C3 under G3, and never the bottom of a rootless voicing (B2 under E3) or of a chord in fourths (D3 under
/// G3). A note's own octave double is the note doubled, not a chord above it: with only its octaves above it (or
/// nothing), a note is the bass only when it was the unit before's, as a held bass stays the bass while an arpeggio
/// passes its octave; C3 and C4 alone have no bass, as C3 alone has none.
pub const BASS_GAP: u8 = 7;

/// The longest bar heard, in beats, as in the key finder: a longer one comes from a damaged file, and is heard as bars
/// of this many beats.
const MOST_BAR_BEATS: i64 = 1024;

/// How strong the start of a unit is, by where it falls in its bar. Stronger compares greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    /// Between felt beats.
    Off,
    /// A felt beat.
    Beat,
    /// The middle felt beat of a bar with an even number of them: beat 3 of 4/4, the second dotted quarter of 6/8.
    HalfBar,
    /// The start of a bar.
    Bar,
}

impl Level {
    /// Its weight (the design's μ): 4 for a bar's start, 3 for the middle of a bar, 2 for another felt beat, 1 between
    /// beats.
    pub const fn weight(self) -> u64 {
        match self {
            Level::Off => 1,
            Level::Beat => 2,
            Level::HalfBar => 3,
            Level::Bar => 4,
        }
    }
}

/// How long a unit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Half a beat: simple metre. A swung or triplet off-beat lands in the second half.
    HalfBeat,
    /// A whole grid beat: compound metre (6, 9 or 12 beats a bar), or a grid of one tick a beat.
    Beat,
}

/// What a part's notes are to the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stream {
    /// The bass: bass parts; and in a bar where none plays, the lowest chord note of a unit standing apart from the
    /// rest ([`BASS_GAP`]). Its notes say where the chord stands and never name a tension.
    Bass,
    /// The tune: melody parts.
    Melody,
    /// The chords: every other pitched part (keys, pads, arpeggios, parts of unknown role).
    Harmony,
}

impl Stream {
    pub const ALL: [Stream; 3] = [Stream::Bass, Stream::Melody, Stream::Harmony];

    /// The stream a part's notes are in, by its role: none for drums.
    pub const fn of(role: Role) -> Option<Stream> {
        match role {
            Role::Bass => Some(Stream::Bass),
            Role::Melody => Some(Stream::Melody),
            Role::Harmony | Role::Unknown => Some(Stream::Harmony),
            Role::Drums => None,
        }
    }
}

/// What a mass counts ([`Hearing::mass`]): for each pitch class, the ticks its longest-sounding note sounds in each
/// unit, added up over the units of a stretch. A note doubled in another octave counts once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mass {
    /// The chords, without what's held over, and without each unit's gap bass and its octave doubles (they are the
    /// bass, doubled).
    Harmony,
    /// The bass: bass parts, and each unit's gap bass in the bars where none plays ([`Hearing::gap_bass`]); without
    /// what's held over.
    Bass,
    /// The tune, without what's held over.
    Melody,
    /// The tune's notes held at least a felt beat: the tune's long notes say more about the chord than its quick ones,
    /// which often pass by.
    HeldMelody,
    /// What the chords hold over a bar line or the middle of a bar ([`HeardNote::held_over`]): smear from the chord
    /// before, which costs a reading less than a wrong note struck.
    HeldOver,
    /// The bass, each unit's ticks times its level's weight ([`Level::weight`]): a bass note on the beat leans towards
    /// the root more than one between beats.
    WeightedBass,
}

impl Mass {
    pub const ALL: [Mass; 6] =
        [Mass::Harmony, Mass::Bass, Mass::Melody, Mass::HeldMelody, Mass::HeldOver, Mass::WeightedBass];
}

/// How the bass behaves over a stretch ([`Hearing::bass`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BassTexture {
    /// One pitch class has at least three quarters of the bass's time (octave jumps and repeated notes count as it):
    /// it says where the chord stands.
    Held(PitchClass),
    /// The bass moves (a walking or busy bass): it leans towards the root on strong beats, and never makes a slash.
    Moving,
    /// No bass sounds.
    None,
}

impl BassTexture {
    /// How a bass behaves, from its ticks on each pitch class over some units ([`Mass::Bass`]).
    pub fn of(bass: &PerPitch<u64>) -> BassTexture {
        let total: u128 = bass.0.iter().map(|&t| u128::from(t)).sum();
        match bass.iter().max_by_key(|&(_, t)| t) {
            Some((pc, most)) if total > 0 && 4 * u128::from(most) >= 3 * total => BassTexture::Held(pc),
            _ if total > 0 => BassTexture::Moving,
            _ => BassTexture::None,
        }
    }
}

/// A note as the reader hears it. Its ticks count from the start of the first unit ([`Hearing::tick`] turns them
/// back into the music's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HeardNote {
    /// Where it came from: its part's place in the list of parts, and its place in the part's notes.
    pub part: usize,
    pub note: usize,
    pub key: u8,
    /// Its part's stream. A chord note that is a unit's bass ([`Hearing::gap_bass`]), or its octave double, is still
    /// `Harmony` here.
    pub stream: Stream,
    /// Where it's heard to start: on a unit's start when it was struck just before it.
    pub start: i64,
    /// Where it stops sounding: at least at the end of its felt beat when it's broken, and never past the end of what's
    /// heard.
    pub end: i64,
    /// How long it was played, from where it's heard to start: its own length, without a broken chord's pedal.
    pub length: i64,
    /// Where its held-over end starts, when it rings a little past a bar line or the middle of a bar.
    pub held_over: Option<i64>,
    /// Struck in a felt beat where its part plays a broken chord, and held by the pedal at least to the end of the felt
    /// beat. A note held through the whole felt beat, such as a bass under an arpeggio, isn't broken.
    pub broken: bool,
    /// A copy, in the bar copied in front of a loop.
    pub copy: bool,
    /// Whether it's struck where it starts: a copy whose start was before the loop's last bar isn't.
    pub struck: bool,
    /// Held to the end of a loop, so as the loop goes round it's held into its start. Only by how it was played: a
    /// broken chord's pedal doesn't make it wrap, as an arpeggio is never pushed.
    pub wraps: bool,
}

impl HeardNote {
    pub const fn pitch_class(self) -> PitchClass {
        PitchClass::of_key(self.key)
    }
}

/// What the reader heard in some notes: the units, and what sounds in each. Keep one and reuse it ([`Hearing::hear`]):
/// it keeps its working space between calls, so hearing many parts doesn't ask for memory each time.
#[derive(Debug, Default)]
pub struct Hearing {
    layout: Layout,
    /// Where what's heard ends, in ticks from the first unit's start: the last note's end, or the loop's end.
    end: i64,
    truncated: bool,
    has_bass_part: bool,
    units: Vec<UnitFacts>,
    /// The masses of the units before each unit, and of all of them last.
    sums: Vec<Sums>,
    /// The ticks of the units before each unit in which a chord or bass note sounds, and of all of them last.
    sound: Vec<u64>,
    notes: Vec<HeardNote>,
    shaped: Vec<Shaped>,
    pushes: Vec<Push>,
    /// The notes of the pushes, by their place in `notes`.
    pushed: Vec<usize>,
    // Working space, kept between calls.
    raw: Vec<Raw>,
    order: Vec<usize>,
    held: Vec<(i64, u8)>,
    pieces: Vec<Piece>,
    touches: Vec<Touch>,
    edges: Vec<Edge>,
    filled: Filled,
    velocities: Vec<u16>,
    starts: Vec<i64>,
    bass_bars: Vec<i64>,
    line_space: lines::LineSpace,
}

impl Hearing {
    pub fn new() -> Hearing {
        Hearing::default()
    }

    /// Hears the parts on the grid, as a loop, a song, or a stretch cut from one (`span`). False when no pitched note
    /// is heard (drums and ghost notes are left out); then there are no units.
    pub fn hear(&mut self, parts: &[Part], grid: Grid, span: Span) -> bool {
        self.clear();
        let mut layout = Layout::new(grid);

        // The notes heard: ghost notes left out, each start snapped.
        let usual = ByStream {
            bass: self.usual(parts, Stream::Bass),
            melody: self.usual(parts, Stream::Melody),
            harmony: self.usual(parts, Stream::Harmony),
        };
        for (p, part) in parts.iter().enumerate() {
            let Some(stream) = Stream::of(part.role) else { continue };
            for (n, note) in part.notes.iter().enumerate() {
                if notes::is_ghost(note, layout.beat, usual.get(stream)) {
                    continue;
                }
                let played = i128::from(note.start);
                let start = layout.snap(played);
                let end = (played + i128::from(note.duration())).max(start + 1);
                self.raw.push(Raw { part: p, note: n, key: note.key, stream, played: note.start, start, end });
            }
        }
        let first = self.raw.iter().map(|r| r.start).min();
        let last_played = self.raw.iter().map(|r| i128::from(r.played)).max();
        let last_end = self.raw.iter().map(|r| r.end).max();
        let (Some(first), Some(last_played), Some(last_end)) = (first, last_played, last_end) else { return false };

        // Where the units start and end. A loop runs from the bar line at or before its first note to the bar line
        // after where its last note is played (a note played just before that line is heard on the loop's start, as
        // the loop goes round); or, when a note is held on to a later bar line (ending within a felt beat of it), to
        // that line, as a chord held through a loop's last bars is. A note ringing on past the loop's end to nowhere
        // near a bar line (a release) is cut off there. A loop is heard with its last bar copied in front. Anything
        // else runs to its last note's end; when its notes lie further apart than the units can hold, the busiest
        // stretch is heard.
        let bar = i128::from(layout.bar);
        let most_bars = i128::from((count(MOST_UNITS) / layout.per_bar).max(1));
        let hearing = most_bars * bar;
        let line = |t: i128| layout.origin + (t - layout.origin).div_euclid(bar) * bar;
        let after_onsets = line(last_played.max(first)) + bar;
        let felt = i128::from(layout.felt_ticks());
        let held_to = |r: &Raw| {
            let nearest = line(r.end + bar / 2);
            (nearest > after_onsets && (r.end - nearest).abs() <= felt).then_some(nearest)
        };
        let loop_end = self.raw.iter().filter_map(held_to).max().unwrap_or(after_onsets);
        let looped = span == Span::Loop && (loop_end - line(first)) / bar < most_bars;
        let (origin, end) = if looped {
            (line(first) - bar, loop_end)
        } else {
            let mut from = first;
            if last_end - first > hearing - bar {
                self.starts.clear();
                self.starts.extend(self.raw.iter().map(|r| r.played));
                from = notes::busiest(&mut self.starts, hearing - bar).unwrap_or(first);
            }
            let origin = line(from);
            let limit = origin + hearing;
            let end = self.raw.iter().filter(|r| (origin..limit).contains(&r.start)).map(|r| r.end.min(limit)).max();
            (origin, end.unwrap_or(limit))
        };
        layout.origin = origin;
        layout.first = if looped { uncount(layout.per_bar) } else { 0 };
        self.layout = layout;
        self.end = ticks(end - origin);

        // A loop's length: the first bar heard is the copy of its last.
        let length = end - origin - bar;
        for r in &self.raw {
            // Only a note struck just before a loop's end is snapped onto it: it's heard on the loop's start.
            let back = if looped && r.start >= end { length } else { 0 };
            let (start, stop) = (r.start - back, r.end - back);
            if !(origin..end).contains(&start) {
                self.truncated = true;
                continue;
            }
            self.truncated |= stop > end && !looped;
            self.notes.push(HeardNote {
                part: r.part,
                note: r.note,
                key: r.key,
                stream: r.stream,
                start: ticks(start - origin),
                end: ticks(stop.min(end) - origin),
                length: ticks(stop - start),
                held_over: None,
                broken: false,
                copy: false,
                struck: true,
                wraps: looped && stop >= end,
            });
        }
        self.has_bass_part = self.notes.iter().any(|n| n.stream == Stream::Bass);
        self.find_broken();
        for n in &mut self.notes {
            n.held_over = layout.held_over(n.start, n.end);
        }
        if looped {
            self.copy_last_bar();
        }
        self.notes.sort_unstable_by_key(|n| (n.start, n.key, n.end, n.copy, n.part, n.note));
        self.gather();
        self.find_shapes();
        self.find_pushes();
        true
    }

    /// How many units were heard (a loop's copied bar included).
    pub fn units(&self) -> usize {
        self.units.len()
    }

    /// The units a reading reports: all of them, or a loop's own after its copied bar.
    pub fn reported(&self) -> Range<usize> {
        self.layout.first.min(self.units.len())..self.units.len()
    }

    /// How long a unit is: half a beat, or in compound metre a whole one.
    pub fn unit(&self) -> Unit {
        if self.layout.per_beat == 2 { Unit::HalfBeat } else { Unit::Beat }
    }

    /// Units in a felt beat: 2 in simple metre, 3 in compound (1 on a grid of one tick a beat).
    pub fn felt_units(&self) -> usize {
        uncount(self.layout.per_felt)
    }

    /// Units in a bar.
    pub fn bar_units(&self) -> usize {
        uncount(self.layout.per_bar)
    }

    /// Ticks in a felt beat.
    pub fn felt_ticks(&self) -> i64 {
        self.layout.felt_ticks()
    }

    /// The tick a unit starts at, in the music's own ticks; `start(units())` is where the last one ends.
    pub fn start(&self, unit: usize) -> i64 {
        self.tick(self.layout.start(unit.min(self.units.len())))
    }

    /// A time given in ticks from the first unit's start (a [`HeardNote`]'s), in the music's own ticks.
    pub fn tick(&self, from_first: i64) -> i64 {
        ticks(self.layout.origin + i128::from(from_first))
    }

    /// How strong a unit's start is.
    pub fn level(&self, unit: usize) -> Level {
        self.layout.level(unit)
    }

    /// Whether some notes were left out because they lay further apart than [`MOST_UNITS`] units (a damaged file).
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Whether a bass part was heard anywhere. In the bars where none plays, the bass is found in the chords by the
    /// gaps ([`BASS_GAP`]).
    pub fn has_bass_part(&self) -> bool {
        self.has_bass_part
    }

    /// The notes heard, in order of where they start (a loop's copies included).
    pub fn notes(&self) -> &[HeardNote] {
        &self.notes
    }

    /// A mass over some units: for each pitch class, its ticks in the stream.
    pub fn mass(&self, units: Range<usize>, mass: Mass) -> PerPitch<u64> {
        let units = self.clamp(units);
        let (Some(from), Some(to)) = (self.sums.get(units.start), self.sums.get(units.end)) else {
            return PerPitch::default();
        };
        let (from, to) = (from.get(mass), to.get(mass));
        let mut out = PerPitch::default();
        for pc in PitchClass::ALL {
            out[pc] = to[pc].saturating_sub(from[pc]);
        }
        out
    }

    /// The ticks of the units in which a chord or bass note sounds (the design's S): silence inside a stretch is free.
    pub fn sounding_ticks(&self, units: Range<usize>) -> u64 {
        let units = self.clamp(units);
        match (self.sound.get(units.start), self.sound.get(units.end)) {
            (Some(from), Some(to)) => to.saturating_sub(*from),
            _ => 0,
        }
    }

    /// What presence counts of each pitch class over some units, in ticks: twice its ticks in the chords and the bass,
    /// and its ticks in the tune's held notes (which count half as much).
    pub fn presence_ticks(&self, units: Range<usize>) -> PerPitch<u64> {
        presence_of(
            &self.mass(units.clone(), Mass::Harmony),
            &self.mass(units.clone(), Mass::Bass),
            &self.mass(units, Mass::HeldMelody),
        )
    }

    /// How present each pitch class is over some units, 0 to 1: four times its ticks in the chords and the bass, and
    /// half its ticks in the tune's held notes, over the ticks that sound ([`Hearing::sounding_ticks`]). A note
    /// sounding a quarter of the time is fully present. The same at any resolution, since both sides are ticks.
    pub fn presence(&self, units: Range<usize>) -> PerPitch<f64> {
        let ticks = self.presence_ticks(units.clone());
        let s = self.sounding_ticks(units).max(1) as f64;
        let mut out = PerPitch([0.0; 12]);
        for pc in PitchClass::ALL {
            out[pc] = (2.0 * ticks[pc] as f64 / s).min(1.0);
        }
        out
    }

    /// The notes heard over some units (the design's P): those at least half present, so sounding an eighth of the
    /// time ([`heard_in`]).
    pub fn heard(&self, units: Range<usize>) -> PcSet {
        heard_in(&self.presence_ticks(units.clone()), self.sounding_ticks(units))
    }

    /// The lowest chord or bass note sounding in any of some units (for at least a quarter of a unit, held-over ends
    /// left out); where none does, the lowest note of any stream sounding there at all. It's the note ties between
    /// readings are broken from, so that they move with the music, and there is one wherever a note is heard.
    pub fn lowest(&self, units: Range<usize>) -> Option<u8> {
        let units = self.units.get(self.clamp(units)).unwrap_or_default();
        units.iter().filter_map(|u| u.lowest).min().or_else(|| units.iter().filter_map(|u| u.lowest_any).min())
    }

    /// The unit start nearest a tick (the music's own ticks), from 0 to [`Hearing::units`] (where the last unit
    /// ends); halfway between two, the earlier. It lines chords given in ticks up with the units.
    pub fn nearest_unit(&self, tick: i64) -> usize {
        let units = self.units.len();
        let last = self.layout.start(units);
        let t = ticks((i128::from(tick) - self.layout.origin).clamp(0, i128::from(last)));
        let unit = self.layout.unit_at(t).min(units);
        let (from, to) = (self.layout.start(unit), self.layout.start(unit + 1));
        if unit < units && to - t < t - from { unit + 1 } else { unit }
    }

    /// How the bass behaves over some units: held on one pitch class (three quarters of its ticks or more), moving,
    /// or not there.
    pub fn bass(&self, units: Range<usize>) -> BassTexture {
        BassTexture::of(&self.mass(units, Mass::Bass))
    }

    /// The notes of a stream struck in a unit (a held-over end is never struck).
    pub fn struck(&self, unit: usize, stream: Stream) -> PcSet {
        self.units.get(unit).map_or(PcSet::EMPTY, |u| u.struck.get(stream))
    }

    /// The notes of a stream struck in any of some units.
    pub fn struck_in(&self, units: Range<usize>, stream: Stream) -> PcSet {
        let units = self.clamp(units);
        self.units.get(units).unwrap_or_default().iter().fold(PcSet::EMPTY, |set, u| set.union(u.struck.get(stream)))
    }

    /// The notes of a stream carried into a unit: struck before it and still sounding as it starts, held-over ends
    /// included.
    pub fn carried(&self, unit: usize, stream: Stream) -> PcSet {
        self.units.get(unit).map_or(PcSet::EMPTY, |u| u.carried.get(stream))
    }

    /// The notes of a stream sounding in a unit, for at least a quarter of it (so the last moment of a note played
    /// legato doesn't count), held-over ends left out.
    pub fn sounding(&self, unit: usize, stream: Stream) -> PcSet {
        self.units.get(unit).map_or(PcSet::EMPTY, |u| u.sounding.get(stream))
    }

    /// The chord notes struck in a unit as part of a broken chord.
    pub fn broken_struck(&self, unit: usize) -> PcSet {
        self.units.get(unit).map_or(PcSet::EMPTY, |u| u.broken)
    }

    /// The first bass note struck in some units: the lowest of those struck first.
    pub fn bass_onset(&self, units: Range<usize>) -> Option<PitchClass> {
        let units = self.clamp(units);
        self.units.get(units).unwrap_or_default().iter().find_map(|u| u.bass_onset)
    }

    /// The key of the chord note that is a unit's bass when no bass part plays in its bar ([`BASS_GAP`]). Its octave
    /// doubles in the chords (not broken) are heard as the bass too.
    pub fn gap_bass(&self, unit: usize) -> Option<u8> {
        self.units.get(unit).and_then(|u| u.gap_bass)
    }

    /// Whether a note is heard as the bass in a unit, as the masses count it ([`Mass::Bass`]): a bass part's note, or
    /// a chord note (not broken) on the pitch class of the unit's gap bass. A held-over end is never the bass.
    pub fn is_bass_in(&self, unit: usize, n: &HeardNote) -> bool {
        match n.stream {
            Stream::Bass => true,
            Stream::Harmony => !n.broken && self.gap_bass(unit).is_some_and(|k| k % 12 == n.key % 12),
            Stream::Melody => false,
        }
    }

    /// The lowest chord or bass note sounding in a unit, for at least a quarter of it (held-over ends left out): unlike
    /// [`Hearing::lowest`], nothing when none does.
    pub fn lowest_sounding(&self, unit: usize) -> Option<u8> {
        self.units.get(unit).and_then(|u| u.lowest)
    }

    /// Where a unit starts, in ticks from the first unit's start (as [`HeardNote`]s count), from 0 to
    /// [`Hearing::units`].
    pub fn offset(&self, unit: usize) -> i64 {
        self.layout.start(unit.min(self.units.len()))
    }

    /// The unit a time is in, given in ticks from the first unit's start: 0 before the first, and
    /// [`Hearing::units`] at or after where the last one ends.
    pub fn unit_of(&self, from_first: i64) -> usize {
        let units = self.units.len();
        if from_first >= self.layout.start(units) {
            return units;
        }
        self.layout.unit_at(from_first.max(0)).min(units)
    }

    fn clamp(&self, units: Range<usize>) -> Range<usize> {
        let end = units.end.min(self.units.len());
        units.start.min(end)..end
    }

    fn clear(&mut self) {
        self.layout = Layout::default();
        self.end = 0;
        self.truncated = false;
        self.has_bass_part = false;
        self.units.clear();
        self.sums.clear();
        self.sound.clear();
        self.notes.clear();
        self.shaped.clear();
        self.pushes.clear();
        self.pushed.clear();
        self.raw.clear();
        self.order.clear();
        self.held.clear();
        self.pieces.clear();
        self.touches.clear();
        self.edges.clear();
        self.bass_bars.clear();
    }

    /// The usual velocity of one stream's notes, for the ghost-note rule.
    fn usual(&mut self, parts: &[Part], stream: Stream) -> u32 {
        let of_stream = parts.iter().filter(|p| Stream::of(p.role) == Some(stream));
        notes::usual_velocity(of_stream.flat_map(|p| p.notes.iter()), &mut self.velocities)
    }

    /// Finds the broken chords. A felt beat of a chord part is broken when, of the notes the part strikes there:
    ///
    /// - they come in two or more strikes (notes struck within a 32nd of a beat of each other are one strike: a chord
    ///   played a little unevenly);
    /// - at least half of the strikes play one pitch class (an octave double counts once);
    /// - none overlaps a note of a later strike by more than a unit (an arpeggio written with the pedal in its notes
    ///   needs no pedal);
    /// - and the part holds nothing through the whole felt beat but the pitch class of its lowest note (a held bass
    ///   under an arpeggio is fine, but a held chord over a moving left hand isn't broken).
    ///
    /// Each note struck there is then broken, and held at least to the felt beat's end, but a note held through the
    /// whole felt beat (a bass under the arpeggio). A note is held through a felt beat when it's struck at its start
    /// and sounds to its end, each to within a quarter of a unit.
    ///
    /// The design said this of whole parts. It's worked out felt beat by felt beat instead, so a song's keys part can
    /// play arpeggios in one passage and held chords in another; and the one-note-a-strike test keeps a part repeating
    /// its chords in eighths (whose notes never overlap) from counting as broken and losing its bass.
    fn find_broken(&mut self) {
        let layout = self.layout;
        let unit = layout.unit_ticks();
        let together = (unit / 4).max(1);
        let Hearing { notes, order, end, held, .. } = self;
        order.clear();
        order.extend(notes.iter().enumerate().filter(|(_, n)| n.stream == Stream::Harmony).map(|(i, _)| i));
        order.sort_unstable_by_key(|&i| notes.get(i).map(|n| (n.part, n.start, n.key)));
        // The part's notes that may still sound through a later felt beat: where each ends, and its key.
        held.clear();
        let mut part = None;
        let mut rest = order.as_slice();
        while let Some(&head) = rest.first() {
            let Some(&first) = notes.get(head) else { break };
            if part != Some(first.part) {
                held.clear();
                part = Some(first.part);
            }
            let felt = layout.felt(first.start);
            let (from, to) = (layout.felt_start(felt), layout.felt_end(felt));
            let same =
                |i: &&usize| notes.get(**i).is_some_and(|n| n.part == first.part && layout.felt(n.start) == felt);
            let size = rest.iter().take_while(same).count().max(1);
            let Some((group, later)) = rest.split_at_checked(size) else { break };
            rest = later;
            // Held through the felt beat, or through as much of it as is heard (the music's last one may end first):
            // struck at its start and sounding to its end, each to within a quarter of a unit (a player's timing).
            let through_to = to.min(*end);
            let held_through = |n: &HeardNote| n.start - from < together && through_to - n.end < together;
            held.retain(|&(until, _)| through_to - until < together);
            let struck = group.iter().filter_map(|&i| notes.get(i));
            let through = struck.clone().filter(|n| held_through(n)).map(|n| n.key);
            let held_pcs: PcSet = held.iter().map(|&(_, key)| key).chain(through).map(PitchClass::of_key).collect();
            let lowest = struck.clone().map(|n| n.key).chain(held.iter().map(|&(_, key)| key)).min();
            let free = lowest.is_none_or(|low| held_pcs.without(PitchClass::of_key(low)).is_empty());
            let broken = free && plays_broken(struck.clone(), unit, together);
            held.extend(struck.filter(|n| n.end > to).map(|n| (n.end, n.key)));
            if broken {
                for &i in group {
                    if let Some(n) = notes.get_mut(i)
                        && !held_through(n)
                    {
                        n.end = n.end.max(through_to);
                        n.broken = true;
                    }
                }
            }
        }
    }

    /// Copies a loop's last bar in front of it, into the first bar heard: what sounds there, cut at the bar's start.
    fn copy_last_bar(&mut self) {
        // The loop runs from the end of the first bar heard to the end, so its length is also where its last bar
        // starts; each copy lands a loop's length earlier.
        let length = self.end - self.layout.bar;
        // The notes there are, as the copies are pushed after them.
        for i in 0..self.notes.len() {
            let Some(n) = self.notes.get(i).copied() else { continue };
            if n.end <= length {
                continue;
            }
            let start = n.start.max(length);
            self.notes.push(HeardNote {
                start: start - length,
                end: n.end - length,
                held_over: n.held_over.map(|h| h.max(start) - length),
                copy: true,
                struck: n.start >= length,
                ..n
            });
        }
    }

    /// Works out what sounds in each unit, from the notes: each note's main part and held-over end, the part of it in
    /// each unit it only partly fills (its first and last) and the units it fills, so a note held for a hundred bars
    /// costs no more than a short one.
    fn gather(&mut self) {
        let layout = self.layout;
        let felt = layout.felt_ticks();
        for n in &self.notes {
            let (main, over) = match n.stream {
                Stream::Harmony => (if n.broken { Kind::BrokenHarmony } else { Kind::Harmony }, Kind::HarmonyOver),
                Stream::Bass => (Kind::Bass, Kind::BassOver),
                Stream::Melody => (if n.length >= felt { Kind::HeldMelody } else { Kind::Melody }, Kind::MelodyOver),
            };
            let split = n.held_over.unwrap_or(n.end).clamp(n.start, n.end);
            if split > n.start {
                self.pieces.push(Piece { kind: main, key: n.key, start: n.start, end: split, onset: n.struck });
            }
            if split < n.end {
                self.pieces.push(Piece { kind: over, key: n.key, start: split, end: n.end, onset: false });
            }
        }
        for p in &self.pieces {
            let (first, last) = (layout.unit_at(p.start), layout.unit_at(p.end - 1));
            let struck = p.onset.then_some(p.start);
            let carried = !p.onset && p.start == layout.start(first);
            let (kind, key) = (p.kind, p.key);
            if first == last {
                self.touches.push(Touch { unit: first, kind, key, ticks: p.end - p.start, struck, carried });
                continue;
            }
            let ticks = layout.start(first + 1) - p.start;
            self.touches.push(Touch { unit: first, kind, key, ticks, struck, carried });
            let ticks = p.end - layout.start(last);
            self.touches.push(Touch { unit: last, kind, key, ticks, struck: None, carried: true });
            if last > first + 1 {
                self.edges.push(Edge { unit: first + 1, kind, key, enter: true });
                self.edges.push(Edge { unit: last, kind, key, enter: false });
            }
        }
        self.touches.sort_unstable_by_key(|t| t.unit);
        self.edges.sort_unstable_by_key(|e| e.unit);

        let units = layout.unit_at(self.end - 1) + 1;
        // The bars a bass part plays in (by how many of its notes do, each added where its bars start and taken off
        // after they end). Only in the other bars is a chord note taken as the bass.
        let per_bar = uncount(layout.per_bar).max(1);
        self.bass_bars.clear();
        self.bass_bars.resize(units / per_bar + 2, 0);
        for p in self.pieces.iter().filter(|p| p.kind == Kind::Bass) {
            if let Some((from, to)) = layout.played_units(p.start, p.end, p.onset) {
                for (bar, step) in [(from / per_bar, 1), (to / per_bar + 1, -1)] {
                    if let Some(b) = self.bass_bars.get_mut(bar) {
                        *b += step;
                    }
                }
            }
        }
        let mut bass_playing = 0i64;
        let mut last_gap = None;
        self.filled.reset();
        let (mut t, mut e) = (0, 0);
        let mut total = Sums::default();
        let mut sounded = 0u64;
        self.sums.push(total);
        self.sound.push(sounded);
        for u in 0..units {
            if u % per_bar == 0 {
                bass_playing += self.bass_bars.get(u / per_bar).copied().unwrap_or(0);
            }
            while let Some(edge) = self.edges.get(e).filter(|x| x.unit == u) {
                self.filled.change(edge.kind, edge.key, edge.enter);
                e += 1;
            }
            let from = t;
            while self.touches.get(t).is_some_and(|x| x.unit == u) {
                t += 1;
            }
            let here = self.touches.get(from..t).unwrap_or_default();
            let len = layout.start(u + 1) - layout.start(u);
            let gap = if bass_playing > 0 { None } else { gap_bass(&self.filled, here, len, last_gap) };
            last_gap = gap;
            let mut facts = UnitFacts { gap_bass: gap, ..UnitFacts::default() };
            let mut masses = Sums::default();
            let mut onset: Option<(i64, u8)> = None;
            let mut take = |c: &Touch| {
                // The gap bass takes its pitch class with it: its octave double in the chords is the bass doubled.
                let to_bass = c.kind == Kind::Harmony && gap.is_some_and(|g| g % 12 == c.key % 12);
                let stream = if to_bass { Stream::Bass } else { c.kind.stream() };
                let pc = PitchClass::of_key(c.key);
                let ticks = u64::try_from(c.ticks).unwrap_or(0);
                match c.kind {
                    Kind::Harmony => masses.raise(if to_bass { Mass::Bass } else { Mass::Harmony }, pc, ticks),
                    Kind::BrokenHarmony => masses.raise(Mass::Harmony, pc, ticks),
                    Kind::HarmonyOver => masses.raise(Mass::HeldOver, pc, ticks),
                    Kind::Bass => masses.raise(Mass::Bass, pc, ticks),
                    Kind::Melody => masses.raise(Mass::Melody, pc, ticks),
                    Kind::HeldMelody => {
                        masses.raise(Mass::Melody, pc, ticks);
                        masses.raise(Mass::HeldMelody, pc, ticks);
                    }
                    Kind::BassOver | Kind::MelodyOver => {}
                }
                if !c.kind.held_over() && c.ticks > 0 {
                    facts.lowest_any = Some(facts.lowest_any.map_or(c.key, |low| low.min(c.key)));
                }
                if !c.kind.held_over() && c.ticks.saturating_mul(4) >= len {
                    facts.sounding.add(stream, pc);
                    if stream != Stream::Melody {
                        facts.lowest = Some(facts.lowest.map_or(c.key, |low| low.min(c.key)));
                    }
                }
                if let Some(at) = c.struck {
                    facts.struck.add(stream, pc);
                    if c.kind == Kind::BrokenHarmony {
                        facts.broken = facts.broken.with(pc);
                    }
                    if stream == Stream::Bass && onset.is_none_or(|first| (at, c.key) < first) {
                        onset = Some((at, c.key));
                    }
                }
                if c.carried {
                    facts.carried.add(stream, pc);
                }
            };
            for kind in Kind::ALL {
                for key in self.filled.keys(kind).iter() {
                    take(&Touch { unit: u, kind, key, ticks: len, struck: None, carried: true });
                }
            }
            for c in here {
                take(c);
            }
            facts.bass_onset = onset.map(|(_, key)| PitchClass::of_key(key));
            let weight = layout.level(u).weight();
            for pc in PitchClass::ALL {
                masses.weighted_bass[pc] = masses.bass[pc].saturating_mul(weight);
            }
            total = total.added(&masses);
            if !facts.sounding.harmony.union(facts.sounding.bass).is_empty() {
                sounded = sounded.saturating_add(u64::try_from(len).unwrap_or(0));
            }
            self.units.push(facts);
            self.sums.push(total);
            self.sound.push(sounded);
        }
    }
}

/// What presence counts of each pitch class ([`Hearing::presence_ticks`]), from the masses of the chords, the bass and
/// the tune's held notes over the same units: twice the chords' and the bass's ticks, and the held tune's once.
pub fn presence_of(chords: &PerPitch<u64>, bass: &PerPitch<u64>, held_tune: &PerPitch<u64>) -> PerPitch<u64> {
    let mut out = PerPitch::default();
    for pc in PitchClass::ALL {
        out[pc] = chords[pc].saturating_add(bass[pc]).saturating_mul(2).saturating_add(held_tune[pc]);
    }
    out
}

/// The notes heard (the design's P), from what presence counts ([`Hearing::presence_ticks`]) and the ticks in which a
/// chord or bass note sounds ([`Hearing::sounding_ticks`]): those at least half present, so sounding an eighth of the
/// time. Worked out in whole numbers, so it never wobbles at the edge.
pub fn heard_in(ticks: &PerPitch<u64>, sounding: u64) -> PcSet {
    let s = u128::from(sounding.max(1));
    PitchClass::ALL.into_iter().filter(|&pc| 4 * u128::from(ticks[pc]) >= s).collect()
}

/// Whether a chord part's notes struck in one felt beat (in order of their start) play a broken chord there: see
/// [`Hearing::find_broken`].
fn plays_broken<'a>(notes: impl Iterator<Item = &'a HeardNote>, unit: i64, together: i64) -> bool {
    let (mut strikes, mut single) = (0u32, 0u32);
    let mut last: Option<i64> = None;
    let mut pcs = PcSet::EMPTY;
    // The latest end of the notes of the strikes before this one, and of this one so far.
    let (mut before, mut now): (Option<i64>, Option<i64>) = (None, None);
    for n in notes {
        if last.is_none_or(|l| n.start - l >= together) {
            if strikes > 0 {
                single += u32::from(pcs.len() == 1);
                before = before.max(now);
            }
            strikes += 1;
            pcs = PcSet::EMPTY;
            now = None;
        }
        if before.is_some_and(|b| b.min(n.end) - n.start > unit) {
            return false;
        }
        pcs = pcs.with(n.pitch_class());
        now = now.max(Some(n.end));
        last = Some(n.start);
    }
    single += u32::from(pcs.len() == 1);
    strikes >= 2 && single * 2 >= strikes
}

/// The key of the unit's bass when no bass part plays in its bar: the lowest chord note sounding there (for at least a
/// quarter of the unit), when it isn't broken and the next one up of another pitch class is [`BASS_GAP`] or more
/// semitones above it; or, when nothing of another pitch class sounds, when it was the unit before's bass (`before`).
fn gap_bass(filled: &Filled, here: &[Touch], len: i64, before: Option<u8>) -> Option<u8> {
    let mut all = filled.keys(Kind::Harmony).union(filled.keys(Kind::BrokenHarmony));
    let mut unbroken = filled.keys(Kind::Harmony);
    for c in here.iter().filter(|c| c.ticks.saturating_mul(4) >= len) {
        match c.kind {
            Kind::Harmony => {
                all = all.with(c.key);
                unbroken = unbroken.with(c.key);
            }
            Kind::BrokenHarmony => all = all.with(c.key),
            _ => {}
        }
    }
    let low = all.lowest().filter(|&low| unbroken.contains(low))?;
    // Its own octave double is the bass doubled, not a chord above it.
    match all.iter().find(|&k| k % 12 != low % 12) {
        Some(next) => (next - low >= BASS_GAP).then_some(low),
        None => (before == Some(low)).then_some(low),
    }
}

/// A count of units, as a number of ticks can be multiplied by.
fn count(units: usize) -> i64 {
    i64::try_from(units).unwrap_or(i64::MAX)
}

/// A number of units worked out in ticks' terms (never negative).
fn uncount(units: i64) -> usize {
    usize::try_from(units).unwrap_or(0)
}

/// Ticks from the first unit's start: always within the units heard, so they fit.
fn ticks(t: i128) -> i64 {
    i64::try_from(t).unwrap_or(if t < 0 { i64::MIN } else { i64::MAX })
}

/// Where the units lie on the grid.
#[derive(Clone, Copy, Debug)]
struct Layout {
    /// Ticks in a grid beat, and in a bar (of at most [`MOST_BAR_BEATS`] beats).
    beat: i64,
    bar: i64,
    /// Units in a grid beat (1 or 2), a felt beat (1, 2 or 3) and a bar; felt beats in a bar.
    per_beat: i64,
    per_felt: i64,
    per_bar: i64,
    felts: i64,
    /// The tick where the first unit starts, on a bar line (before that's known, the grid's own bar line).
    origin: i128,
    /// The first unit reported: after a loop's copied bar.
    first: usize,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout::new(Grid::four_four(2))
    }
}

impl Layout {
    fn new(grid: Grid) -> Layout {
        let beat = grid.beat();
        let bar_beats = grid.bar_beats().min(MOST_BAR_BEATS);
        let compound = matches!(bar_beats, 6 | 9 | 12);
        let per_beat = if compound || beat < 2 { 1 } else { 2 };
        Layout {
            beat,
            bar: beat * bar_beats,
            per_beat,
            per_felt: if compound { 3 } else { per_beat },
            per_bar: per_beat * bar_beats,
            felts: if compound { bar_beats / 3 } else { bar_beats },
            origin: i128::from(grid.bar_line),
            first: 0,
        }
    }

    /// Ticks from a bar's start to the start of its unit `j` (0 to the units in a bar): unit j starts ⌊j·beat/2⌋ in.
    fn offset(&self, j: i64) -> i64 {
        j * self.beat / self.per_beat
    }

    /// The unit of its bar a tick `r` ticks into the bar is in.
    fn unit_in_bar(&self, r: i64) -> i64 {
        (self.per_beat * (r + 1) - 1) / self.beat
    }

    /// Where a unit starts, in ticks from the first unit's start.
    fn start(&self, unit: usize) -> i64 {
        let k = count(unit);
        k / self.per_bar * self.bar + self.offset(k % self.per_bar)
    }

    /// The unit a tick (from the first unit's start, not before it) is in.
    fn unit_at(&self, tick: i64) -> usize {
        uncount(tick / self.bar * self.per_bar + self.unit_in_bar(tick % self.bar))
    }

    /// The felt beat a tick is in, counted from the first unit.
    fn felt(&self, tick: i64) -> usize {
        self.unit_at(tick) / uncount(self.per_felt).max(1)
    }

    /// Where a felt beat starts, and where it ends.
    fn felt_start(&self, felt: usize) -> i64 {
        self.start(felt.saturating_mul(uncount(self.per_felt)))
    }

    fn felt_end(&self, felt: usize) -> i64 {
        self.start(felt.saturating_add(1).saturating_mul(uncount(self.per_felt)))
    }

    /// Ticks in a felt beat, and in a unit (the shorter, when a beat's ticks are odd).
    fn felt_ticks(&self) -> i64 {
        self.offset(self.per_felt)
    }

    fn unit_ticks(&self) -> i64 {
        (self.beat / self.per_beat).max(1)
    }

    /// The unit of a bar where its middle felt beat starts, when it has an even number of them.
    fn half_bar(&self) -> Option<i64> {
        (self.felts % 2 == 0).then_some(self.felts / 2 * self.per_felt)
    }

    fn level(&self, unit: usize) -> Level {
        let j = count(unit) % self.per_bar;
        if j == 0 {
            Level::Bar
        } else if j % self.per_felt != 0 {
            Level::Off
        } else if self.half_bar() == Some(j) {
            Level::HalfBar
        } else {
            Level::Beat
        }
    }

    /// The first and last units a stretch of a note (from the first unit's start) plays in: those it sounds in for at
    /// least a quarter of the unit, and the one where it's struck when `struck`. None when it only touches the end of
    /// one (the last moment of a note played legato).
    fn played_units(&self, start: i64, end: i64, struck: bool) -> Option<(usize, usize)> {
        let (first, last) = (self.unit_at(start), self.unit_at(end - 1));
        let sounds = |u: usize| {
            let (from, to) = (self.start(u), self.start(u + 1));
            (end.min(to) - start.max(from)).saturating_mul(4) >= to - from
        };
        let from = if struck || sounds(first) { first } else { first + 1 };
        let to = if sounds(last) || (struck && last == first) { last } else { last.checked_sub(1)? };
        (from <= to).then_some((from, to))
    }

    /// The last unit at or before this one that starts a bar or the middle of one.
    fn strong_at_or_before(&self, unit: usize) -> usize {
        let k = count(unit);
        let (bar, j) = (k - k % self.per_bar, k % self.per_bar);
        uncount(match self.half_bar() {
            Some(h) if j >= h => bar + h,
            _ => bar,
        })
    }

    /// Where a note's held-over end starts: the last bar line or middle of a bar it rings past, when it sounded at least
    /// a felt beat before it and ends within a felt beat after it.
    fn held_over(&self, start: i64, end: i64) -> Option<i64> {
        let felt = self.felt_ticks();
        let line = self.start(self.strong_at_or_before(self.unit_at(end - 1)));
        (line - start >= felt && end - line <= felt).then_some(line)
    }

    /// Where a note struck at `tick` (the music's own ticks) is heard to start: on the next unit's start when it's at
    /// most a quarter of a unit before it.
    fn snap(&self, tick: i128) -> i128 {
        let r = (tick - self.origin).rem_euclid(i128::from(self.bar));
        let into = i64::try_from(r).unwrap_or(0);
        let j = self.unit_in_bar(into);
        let (from, to) = (self.offset(j), self.offset(j + 1));
        if into != from && to - into <= (to - from) / 4 { tick - r + i128::from(to) } else { tick }
    }
}

/// A note before it's placed on the units: its ticks are still the music's own (snapped).
#[derive(Clone, Copy, Debug)]
struct Raw {
    part: usize,
    note: usize,
    key: u8,
    stream: Stream,
    /// Where it was played, before the snap.
    played: i64,
    start: i128,
    end: i128,
}

/// One of each stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ByStream<T> {
    bass: T,
    melody: T,
    harmony: T,
}

impl<T: Copy> ByStream<T> {
    fn get(&self, stream: Stream) -> T {
        match stream {
            Stream::Bass => self.bass,
            Stream::Melody => self.melody,
            Stream::Harmony => self.harmony,
        }
    }
}

impl ByStream<PcSet> {
    fn add(&mut self, stream: Stream, pc: PitchClass) {
        let set = match stream {
            Stream::Bass => &mut self.bass,
            Stream::Melody => &mut self.melody,
            Stream::Harmony => &mut self.harmony,
        };
        *set = set.with(pc);
    }
}

/// What sounds in one unit, beside its masses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UnitFacts {
    struck: ByStream<PcSet>,
    carried: ByStream<PcSet>,
    sounding: ByStream<PcSet>,
    /// Chord notes struck as part of a broken chord.
    broken: PcSet,
    /// The first bass note struck (the lowest of those struck first).
    bass_onset: Option<PitchClass>,
    /// In a bar where no bass part plays, the chord note that is the unit's bass.
    gap_bass: Option<u8>,
    /// The lowest chord or bass note sounding (for at least a quarter of the unit), held-over ends left out.
    lowest: Option<u8>,
    /// The lowest note of any stream sounding at all, held-over ends left out.
    lowest_any: Option<u8>,
}

/// The masses of some units, one table of twelve for each [`Mass`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sums {
    harmony: PerPitch<u64>,
    bass: PerPitch<u64>,
    melody: PerPitch<u64>,
    held_melody: PerPitch<u64>,
    held_over: PerPitch<u64>,
    weighted_bass: PerPitch<u64>,
}

impl Sums {
    fn get(&self, mass: Mass) -> &PerPitch<u64> {
        match mass {
            Mass::Harmony => &self.harmony,
            Mass::Bass => &self.bass,
            Mass::Melody => &self.melody,
            Mass::HeldMelody => &self.held_melody,
            Mass::HeldOver => &self.held_over,
            Mass::WeightedBass => &self.weighted_bass,
        }
    }

    fn get_mut(&mut self, mass: Mass) -> &mut PerPitch<u64> {
        match mass {
            Mass::Harmony => &mut self.harmony,
            Mass::Bass => &mut self.bass,
            Mass::Melody => &mut self.melody,
            Mass::HeldMelody => &mut self.held_melody,
            Mass::HeldOver => &mut self.held_over,
            Mass::WeightedBass => &mut self.weighted_bass,
        }
    }

    /// Keeps the longest any one note of `pc` sounds: octave doubles count once.
    fn raise(&mut self, mass: Mass, pc: PitchClass, ticks: u64) {
        let m = &mut self.get_mut(mass)[pc];
        *m = (*m).max(ticks);
    }

    fn added(&self, unit: &Sums) -> Sums {
        let mut out = *self;
        for mass in Mass::ALL {
            let (to, add) = (out.get_mut(mass), unit.get(mass));
            for pc in PitchClass::ALL {
                to[pc] = to[pc].saturating_add(add[pc]);
            }
        }
        out
    }
}

/// How one stretch of a note sounds: what the masses and the masks make of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    Harmony,
    BrokenHarmony,
    HarmonyOver,
    Bass,
    BassOver,
    Melody,
    HeldMelody,
    MelodyOver,
}

const KINDS: usize = 8;

impl Kind {
    const ALL: [Kind; KINDS] = [
        Kind::Harmony,
        Kind::BrokenHarmony,
        Kind::HarmonyOver,
        Kind::Bass,
        Kind::BassOver,
        Kind::Melody,
        Kind::HeldMelody,
        Kind::MelodyOver,
    ];

    const fn stream(self) -> Stream {
        match self {
            Kind::Harmony | Kind::BrokenHarmony | Kind::HarmonyOver => Stream::Harmony,
            Kind::Bass | Kind::BassOver => Stream::Bass,
            Kind::Melody | Kind::HeldMelody | Kind::MelodyOver => Stream::Melody,
        }
    }

    const fn held_over(self) -> bool {
        matches!(self, Kind::HarmonyOver | Kind::BassOver | Kind::MelodyOver)
    }
}

/// A note's main part, or its held-over end, in ticks from the first unit's start.
#[derive(Clone, Copy, Debug)]
struct Piece {
    kind: Kind,
    key: u8,
    start: i64,
    end: i64,
    /// Whether it starts where the note is struck.
    onset: bool,
}

/// A piece in a unit it sounds in for less than all of it, or where it starts or ends.
#[derive(Clone, Copy, Debug)]
struct Touch {
    unit: usize,
    kind: Kind,
    key: u8,
    ticks: i64,
    /// Where it's struck, when it's struck in this unit.
    struck: Option<i64>,
    /// Whether it's sounding as the unit starts, struck before it.
    carried: bool,
}

/// Where a piece starts filling whole units (`enter`), or stops.
#[derive(Clone, Copy, Debug)]
struct Edge {
    unit: usize,
    kind: Kind,
    key: u8,
    enter: bool,
}

/// The keys of the pieces filling the unit being worked out, by kind.
#[derive(Debug, Default)]
struct Filled {
    counts: Vec<u32>,
    keys: [Keys; KINDS],
}

impl Filled {
    fn reset(&mut self) {
        self.counts.clear();
        self.counts.resize(KINDS * 256, 0);
        self.keys = [Keys::default(); KINDS];
    }

    fn change(&mut self, kind: Kind, key: u8, enter: bool) {
        let Some(count) = self.counts.get_mut(kind as usize * 256 + usize::from(key)) else { return };
        *count = if enter { count.saturating_add(1) } else { count.saturating_sub(1) };
        if let Some(keys) = self.keys.get_mut(kind as usize) {
            *keys = if *count > 0 { keys.with(key) } else { keys.without(key) };
        }
    }

    fn keys(&self, kind: Kind) -> Keys {
        self.keys.get(kind as usize).copied().unwrap_or_default()
    }
}

/// A set of keys, 0 to 255 (a damaged file's keys go past 127).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Keys {
    low: u128,
    high: u128,
}

impl Keys {
    fn with(self, key: u8) -> Keys {
        let bit = 1u128 << (key & 127);
        if key < 128 { Keys { low: self.low | bit, ..self } } else { Keys { high: self.high | bit, ..self } }
    }

    fn without(self, key: u8) -> Keys {
        let bit = 1u128 << (key & 127);
        if key < 128 { Keys { low: self.low & !bit, ..self } } else { Keys { high: self.high & !bit, ..self } }
    }

    fn contains(self, key: u8) -> bool {
        let bit = 1u128 << (key & 127);
        (if key < 128 { self.low } else { self.high }) & bit != 0
    }

    fn union(self, other: Keys) -> Keys {
        Keys { low: self.low | other.low, high: self.high | other.high }
    }

    fn lowest(self) -> Option<u8> {
        if self.low != 0 {
            u8::try_from(self.low.trailing_zeros()).ok()
        } else if self.high != 0 {
            u8::try_from(self.high.trailing_zeros() + 128).ok()
        } else {
            None
        }
    }

    fn iter(self) -> impl Iterator<Item = u8> {
        let mut rest = self;
        std::iter::from_fn(move || {
            let key = rest.lowest()?;
            rest = rest.without(key);
            Some(key)
        })
    }
}
