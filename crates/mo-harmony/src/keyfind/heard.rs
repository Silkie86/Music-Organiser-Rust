//! What the key finder hears ([`Heard`]): whole-number counts, gathered in one pass over the notes, beat by beat on the
//! grid.
//!
//! - How long each note sounds in the tune, and in the other parts.
//! - The bass of each beat (its "anchor"), weighted by where the beat falls in its bar, and the major and minor
//!   chords standing on it.
//! - The bass on the first bar line that has one, and on the last.
//! - Which bars differ, so a loop written out four times, or a chord held for four bars, counts as what it is.
//!
//! Every count is a whole number, and nothing reads how high a note is, only the gaps between notes. So moving the
//! notes up or down moves every count to its new note exactly; the order of the notes, splitting a part in two, or
//! adding drums changes nothing; and the counts of each bar added up ([`Heard::merge`]) are exactly the counts of the
//! whole ([`KeyReader::hear_bars`]).

use crate::key::Key;
use crate::notes::{Grid, Note, Part, Role};
use crate::pitch::{PcSet, PerPitch, PitchClass};

use super::{Config, MINOR, Span, Stream};

/// The most beats heard, from the bar line at or before the first note (about nine hours at 120 beats a minute). A
/// note past them can only come from a damaged file, and is left out.
pub const MOST_BEATS: usize = 1 << 16;

/// The longest bar heard, in beats. Music has bars of a few beats (12 in 12/8); a longer one comes from a damaged
/// file, and is heard as bars of this many beats.
const MOST_BAR_BEATS: i64 = 1024;

/// A held note counts for at most two bars from its start, so a pad held through a whole song can't outweigh what's
/// played over it...
const REACH_BARS: i128 = 2;
/// ...and never for more than this many beats.
const REACH_BEATS: i128 = 256;

/// The lowest note of the chords is their bass when the next note up is at least this many semitones above it (a
/// fourth): C under E and G is a bass, and so is C under G, but C under D or E (a cluster, or close voicing) isn't.
const ANCHOR_GAP: u16 = 5;

/// How many beats of a stream count for half of its weight: a stream heard for N beats counts N / (N + 4) of it, so
/// one bar's notes say less than a song's.
const HALF_SIZE: u64 = 4;

/// Which kinds of part were heard: a bit for each [`Role`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RoleSet(u8);

impl RoleSet {
    pub const EMPTY: RoleSet = RoleSet(0);

    const fn bit(role: Role) -> u8 {
        match role {
            Role::Melody => 1,
            Role::Harmony => 2,
            Role::Bass => 4,
            Role::Drums => 8,
            Role::Unknown => 16,
        }
    }

    pub const fn with(self, role: Role) -> RoleSet {
        RoleSet(self.0 | RoleSet::bit(role))
    }

    pub const fn has(self, role: Role) -> bool {
        self.0 & RoleSet::bit(role) != 0
    }

    pub const fn union(self, other: RoleSet) -> RoleSet {
        RoleSet(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// One bar that differs from the others: its fingerprint, and for how many of its beats the tune and the other parts
/// sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BarPrint {
    print: u64,
    tune_beats: u32,
    accompaniment_beats: u32,
}

/// What the finder heard in some notes: the counts every key is scored by. Bars can be heard one by one and added up
/// ([`Heard::merge`]), giving exactly what hearing them together gives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// How long each note sounds in the tune (the melody parts), in ticks.
    pub tune: PerPitch<u64>,
    /// How long each note sounds in every other part (chords, bass, parts of unknown role), in ticks.
    pub harmony: PerPitch<u64>,
    /// The bass of each beat, weighted by where the beat falls in its bar: 4 for a bar's first beat, 2 for its
    /// middle (the third beat of 4/4, every third beat of 6/8, 9/8 and 12/8), 1 for the rest.
    pub bass: PerPitch<u64>,
    /// The same weights, for the beats where a major chord stands on the bass (its third and fifth sound over it), by
    /// the bass note. A minor third sounding too is heard as a sharp ninth, so C7♯9 still counts as C major.
    pub major_on_bass: PerPitch<u64>,
    /// The beats where a minor chord stands on the bass (minor third and fifth, no major third).
    pub minor_on_bass: PerPitch<u64>,
    /// The beats where the bass's minor seventh sounds over it (for the colour of a key, not the key).
    pub flat7_on_bass: PerPitch<u64>,
    /// The bass on the first bar line that has one, and on the last.
    pub first_bass: Option<PitchClass>,
    pub last_bass: Option<PitchClass>,
    /// Notes heard (ghost notes and drums left out).
    pub notes: u32,
    /// The kinds of part those notes came from.
    pub roles: RoleSet,
    /// The bars that differ, by fingerprint, each as first heard.
    bars: Vec<BarPrint>,
}

/// The share of a stream's weight its evidence earns: N / (N + 4) for N beats heard.
fn size(beats: u64) -> f64 {
    if beats == 0 { 0.0 } else { beats as f64 / beats.saturating_add(HALF_SIZE) as f64 }
}

/// All twelve counts added up.
fn total(counts: &PerPitch<u64>) -> u64 {
    counts.0.iter().fold(0u64, |t, c| t.saturating_add(*c))
}

impl Heard {
    /// How many bars differ: a bar is told from another by the notes starting in it (where in the bar, which key, how
    /// long), so a loop written out four times counts its bars once, and a chord held for four bars counts as one bar.
    pub fn bars_heard(&self) -> u32 {
        u32::try_from(self.bars.len()).unwrap_or(u32::MAX)
    }

    /// Beats the tune sounds in, over the bars that differ (each counted once).
    pub fn tune_beats(&self) -> u64 {
        self.bars.iter().map(|b| u64::from(b.tune_beats)).sum()
    }

    /// Beats the other parts sound in, over the bars that differ.
    pub fn accompaniment_beats(&self) -> u64 {
        self.bars.iter().map(|b| u64::from(b.accompaniment_beats)).sum()
    }

    /// Which streams have sound: tune and accompaniment, only one of them, or only a bass part.
    pub fn config(&self) -> Config {
        let tune = total(&self.tune) > 0;
        let others = total(&self.harmony) > 0;
        if tune && others {
            Config::Full
        } else if tune {
            Config::MelodyOnly
        } else if self.roles.has(Role::Bass) && !self.roles.has(Role::Harmony) && !self.roles.has(Role::Unknown) {
            Config::BassOnly
        } else {
            Config::Accompaniment
        }
    }

    /// The largest share of the notes' time (tune and other parts together) inside one major scale's seven notes:
    /// above 0.9 for music in a key, about 0.83 for a blues scale, 0.67 for a whole-tone scale, 0.63 for an
    /// octatonic one and 0.58 for all twelve notes evenly. 0 when nothing was heard.
    pub fn diatonic_fit(&self) -> f64 {
        let mut notes = self.tune;
        for (pc, c) in self.harmony.iter() {
            notes[pc] = notes[pc].saturating_add(c);
        }
        let all = total(&notes);
        if all == 0 {
            return 0.0;
        }
        let major = PcSet::of(&[0, 2, 4, 5, 7, 9, 11]);
        let best = PitchClass::ALL
            .iter()
            .map(|t| major.on(*t).iter().fold(0u64, |s, pc| s.saturating_add(notes[pc])))
            .max()
            .unwrap_or(0);
        best as f64 / all as f64
    }

    /// Adds the counts of the music heard right after this, as if both had been heard together: the counts add up,
    /// the first bass stays this one's (unless it has none), the last becomes the later one's (unless it has none),
    /// and a bar in both is counted once, as this one heard it.
    pub fn merge(&mut self, later: &Heard) {
        let pairs = [
            (&mut self.tune, &later.tune),
            (&mut self.harmony, &later.harmony),
            (&mut self.bass, &later.bass),
            (&mut self.major_on_bass, &later.major_on_bass),
            (&mut self.minor_on_bass, &later.minor_on_bass),
            (&mut self.flat7_on_bass, &later.flat7_on_bass),
        ];
        for (mine, theirs) in pairs {
            for (pc, c) in theirs.iter() {
                mine[pc] = mine[pc].saturating_add(c);
            }
        }
        self.first_bass = self.first_bass.or(later.first_bass);
        self.last_bass = later.last_bass.or(self.last_bass);
        self.notes = self.notes.saturating_add(later.notes);
        self.roles = self.roles.union(later.roles);
        // Both lists are in order of fingerprint, so they're merged in one walk.
        let mut bars = Vec::with_capacity(self.bars.len() + later.bars.len());
        let (mut mine, mut theirs) = (self.bars.iter().peekable(), later.bars.iter().peekable());
        loop {
            match (mine.peek(), theirs.peek()) {
                (Some(a), Some(b)) if b.print < a.print => {
                    bars.push(**b);
                    theirs.next();
                }
                (Some(a), b) => {
                    if b.is_some_and(|b| b.print == a.print) {
                        theirs.next();
                    }
                    bars.push(**a);
                    mine.next();
                }
                (None, Some(b)) => {
                    bars.push(**b);
                    theirs.next();
                }
                (None, None) => break,
            }
        }
        self.bars = bars;
    }

    /// The counts of one of the first five streams (those counted for every note); `None` for the others.
    pub fn counts(&self, stream: Stream) -> Option<&PerPitch<u64>> {
        match stream {
            Stream::Tune => Some(&self.tune),
            Stream::Harmony => Some(&self.harmony),
            Stream::Bass => Some(&self.bass),
            Stream::MajorOnBass => Some(&self.major_on_bass),
            Stream::MinorOnBass => Some(&self.minor_on_bass),
            Stream::FirstBassLoop | Stream::FirstBassSong | Stream::LastBassSong => None,
        }
    }

    /// Calls `each(feature, value)` for every feature `key` is scored by that isn't 0, always in the same order.
    ///
    /// - For each of the first five streams: the share of its count on each note, counted up from the key's tonic,
    ///   times how much evidence the stream has (N / (N + 4) for N beats); feature `24·stream + 12·minor + degree`.
    /// - For a loop, its first bass; for a song, its first and last bass: 1 on the note's degree. A stretch cut from
    ///   longer music starts and ends anywhere, so it has neither.
    /// - 1 for the minor keys' own weight ([`MINOR`]).
    ///
    /// The bench fits the weights through this, and the finder scores with it, so the two can't differ.
    pub fn features(&self, key: Key, span: Span, mut each: impl FnMut(usize, f64)) {
        let mode = if key.is_minor() { 12 } else { 0 };
        let tune = size(self.tune_beats());
        let others = size(self.accompaniment_beats());
        let counted = [
            (Stream::Tune, &self.tune, tune),
            (Stream::Harmony, &self.harmony, others),
            (Stream::Bass, &self.bass, others),
            (Stream::MajorOnBass, &self.major_on_bass, others),
            (Stream::MinorOnBass, &self.minor_on_bass, others),
        ];
        for (stream, counts, weight) in counted {
            let all = total(counts);
            if all == 0 || weight == 0.0 {
                continue;
            }
            let base = stream as usize * 24 + mode;
            for (degree, c) in counts.rotated(key.tonic).0.iter().enumerate() {
                if *c != 0 {
                    each(base + degree, weight * (*c as f64 / all as f64));
                }
            }
        }
        let ends = match span {
            Span::Loop => [(Stream::FirstBassLoop, self.first_bass), (Stream::LastBassSong, None)],
            Span::Song => [(Stream::FirstBassSong, self.first_bass), (Stream::LastBassSong, self.last_bass)],
            Span::Window => [(Stream::FirstBassLoop, None), (Stream::LastBassSong, None)],
        };
        for (stream, bass) in ends {
            if let Some(pc) = bass {
                each(stream as usize * 24 + mode + usize::from(pc.above(key.tonic)), 1.0);
            }
        }
        if key.is_minor() {
            each(MINOR, 1.0);
        }
    }
}

/// No key, in [`BeatFacts`].
const NO_KEY: u16 = u16::MAX;

/// What sounds in one beat, as far as the bass and the chords go.
#[derive(Clone, Copy, Debug)]
struct BeatFacts {
    /// The notes under the tune sounding for at least a quarter of the beat.
    chord: PcSet,
    /// The lowest key of a bass part sounding in the beat's first half.
    bass: u16,
    /// The lowest two different keys of the other parts under the tune sounding in the beat's first half.
    low: u16,
    next: u16,
    /// Bit 0: the tune sounds in the beat; bit 1: another part does.
    sounding: u8,
}

impl BeatFacts {
    const SILENT: BeatFacts = BeatFacts { chord: PcSet::EMPTY, bass: NO_KEY, low: NO_KEY, next: NO_KEY, sounding: 0 };

    fn add_low(&mut self, key: u16) {
        if key < self.low {
            self.next = self.low;
            self.low = key;
        } else if key > self.low && key < self.next {
            self.next = key;
        }
    }

    /// The beat's bass: the lowest note of a bass part; or else the lowest note of the chords, if nothing sounds
    /// between it and the fourth above (so it stands apart as a bass does); or else none. The tune is never the bass,
    /// and how high the notes are doesn't matter, only the gap.
    fn anchor(self) -> Option<PitchClass> {
        if self.bass != NO_KEY {
            Some(PitchClass::new(i32::from(self.bass)))
        } else if self.low != NO_KEY && (self.next == NO_KEY || self.next >= self.low.saturating_add(ANCHOR_GAP)) {
            Some(PitchClass::new(i32::from(self.low)))
        } else {
            None
        }
    }
}

/// How much a beat counts by where it falls in its bar: 4 for the first beat; 2 for the middle beat of a bar of 4 or
/// more beats, or every third beat of a bar of 6, 9 or 12 (6/8, 9/8, 12/8); 1 for the rest. It never depends on where
/// the music starts, only on the bar.
fn metrical_weight(place: usize, bar_beats: usize) -> u64 {
    if place == 0 {
        4
    } else if matches!(bar_beats, 6 | 9 | 12) {
        if place.is_multiple_of(3) { 2 } else { 1 }
    } else if bar_beats >= 4 && bar_beats.is_multiple_of(2) && place == bar_beats / 2 {
        2
    } else {
        1
    }
}

/// One bar while the notes are being heard: the sum of the fingerprints of the notes starting in it, how many there
/// are and which kinds of part they came from.
#[derive(Clone, Copy, Debug, Default)]
struct BarScratch {
    print: u64,
    onsets: u32,
    roles: RoleSet,
}

/// A note's fingerprint, from the stream it's in, where in its bar it starts, its key and how long it's heard: FNV-1a
/// over those bytes. A bar's fingerprint is the sum of its notes', which doesn't depend on their order.
fn note_print(stream: u8, offset: i128, key: u8, length: i128) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&[stream]);
    eat(&offset.to_le_bytes());
    eat(&[key]);
    eat(&length.to_le_bytes());
    h
}

/// How long a note sounds, in ticks: at least 1.
fn length(note: &Note) -> i128 {
    i128::from(note.length).max(1)
}

/// Where the beats lie, from one pass over the notes.
#[derive(Clone, Copy, Debug)]
struct Layout {
    /// The bar line at or before the first note, in ticks.
    origin: i128,
    /// Beats heard, 1 to [`MOST_BEATS`].
    beats: usize,
    /// Beats in a bar, 1 to [`MOST_BAR_BEATS`].
    bar_beats: usize,
}

impl Layout {
    fn bars(self) -> usize {
        self.beats.div_ceil(self.bar_beats)
    }
}

/// Hears notes ([`KeyReader::hear`]) and reads their key ([`KeyReader::read`]). It keeps its working space between
/// calls, so reading many parts (the MIDI page's index) doesn't ask for memory each time: keep one per thread.
#[derive(Debug, Default)]
pub struct KeyReader {
    beats: Vec<BeatFacts>,
    bars: Vec<BarScratch>,
    /// Ticks of each note in the tune and in the other parts: one pair for the whole, or one for each bar.
    ticks: Vec<[PerPitch<u64>; 2]>,
    velocities: Vec<u16>,
}

impl KeyReader {
    pub fn new() -> KeyReader {
        KeyReader::default()
    }

    /// Hears the parts on the grid. `None` when nothing pitched is heard.
    pub fn hear(&mut self, parts: &[Part], grid: Grid) -> Option<Heard> {
        let layout = self.listen(parts, grid, false)?;
        let heard = self.gather(layout, 0..layout.bars(), 0);
        (heard.notes > 0).then_some(heard)
    }

    /// Hears the parts bar by bar: the tick where the first bar starts (the bar line at or before the first note),
    /// and what was heard in each bar from there. Added up in order with [`Heard::merge`], they give exactly what
    /// [`KeyReader::hear`] gives. A note held over a bar line counts in each bar it sounds in, in the bar it starts in
    /// as a note. `None` when nothing pitched is heard.
    pub fn hear_bars(&mut self, parts: &[Part], grid: Grid) -> Option<(i64, Vec<Heard>)> {
        let layout = self.listen(parts, grid, true)?;
        let bars = (0..layout.bars()).map(|b| self.gather(layout, b..b + 1, b)).collect::<Vec<_>>();
        if bars.iter().all(|b| b.notes == 0) {
            return None;
        }
        let origin = i64::try_from(layout.origin).unwrap_or(if layout.origin < 0 { i64::MIN } else { i64::MAX });
        Some((origin, bars))
    }

    /// The velocity nine in ten of the notes of one stream (the tune, or the other parts) are played at or below.
    fn loudness(&mut self, parts: &[Part], tune: bool) -> u32 {
        self.velocities.clear();
        for part in parts.iter().filter(|p| p.role.pitched() && (p.role == Role::Melody) == tune) {
            self.velocities.extend(part.notes.iter().map(|n| n.velocity));
        }
        let n = self.velocities.len();
        if n == 0 {
            return 0;
        }
        // The smallest velocity with at least 90% of the notes at or below it: ceil(0.9 n) - 1 from the bottom. It
        // stays put when every note is doubled (an octave up), which a rank like 0.9 (n - 1) doesn't.
        let rank = (n.saturating_mul(9).saturating_add(9) / 10).saturating_sub(1).min(n - 1);
        let (_, v, _) = self.velocities.select_nth_unstable(rank);
        u32::from(*v)
    }

    /// One pass over the notes: fills the beats, the bars and the ticks.
    fn listen(&mut self, parts: &[Part], grid: Grid, by_bar: bool) -> Option<Layout> {
        let beat = i128::from(grid.beat());
        let bar_beats = grid.bar_beats().min(MOST_BAR_BEATS);
        let bar = beat * i128::from(bar_beats);
        let reach = (REACH_BARS * bar).min(REACH_BEATS * beat);
        let half = (beat + 1) / 2;

        // Ghost notes (shorter than an eighth of a beat, and played at under an eighth of their stream's usual
        // velocity) are left out: a stray touch of a key isn't part of the music.
        let loud = [self.loudness(parts, true), self.loudness(parts, false)];
        let ghost = |note: &Note, tune: bool| {
            length(note) * 8 < beat && u32::from(note.velocity) * 8 < if tune { loud[0] } else { loud[1] }
        };

        let mut first: Option<i128> = None;
        let mut last = i128::MIN;
        for part in parts.iter().filter(|p| p.role.pitched()) {
            let tune = part.role == Role::Melody;
            for note in part.notes.iter().filter(|n| !ghost(n, tune)) {
                let start = i128::from(note.start);
                first = Some(first.map_or(start, |f| f.min(start)));
                last = last.max(start + length(note).min(reach));
            }
        }
        let first = first?;
        let bar_line = i128::from(grid.bar_line);
        let origin = bar_line + (first - bar_line).div_euclid(bar) * bar;
        let beats = usize::try_from((last - origin + beat - 1) / beat).unwrap_or(MOST_BEATS).clamp(1, MOST_BEATS);
        let layout = Layout { origin, beats, bar_beats: usize::try_from(bar_beats).unwrap_or(1).max(1) };

        self.beats.clear();
        self.beats.resize(beats, BeatFacts::SILENT);
        self.bars.clear();
        self.bars.resize(layout.bars(), BarScratch::default());
        self.ticks.clear();
        self.ticks.resize(if by_bar { layout.bars() } else { 1 }, [PerPitch([0; 12]); 2]);

        for part in parts.iter().filter(|p| p.role.pitched()) {
            let tune = part.role == Role::Melody;
            let stream = usize::from(!tune);
            for note in part.notes.iter().filter(|n| !ghost(n, tune)) {
                let start = i128::from(note.start) - origin;
                let heard = length(note).min(reach);
                let end = start + heard;
                let Ok(first_beat) = usize::try_from(start / beat) else { continue };
                if first_beat >= beats {
                    continue;
                }
                let last_beat = usize::try_from((end - 1) / beat).unwrap_or(usize::MAX).min(beats - 1);
                let bar_index = first_beat / layout.bar_beats;
                if let Some(b) = self.bars.get_mut(bar_index) {
                    let offset = start - bar_index as i128 * bar;
                    b.print = b.print.wrapping_add(note_print(stream as u8, offset, note.key, heard));
                    b.onsets = b.onsets.saturating_add(1);
                    b.roles = b.roles.with(part.role);
                }
                let pc = note.pitch_class();
                for j in first_beat..=last_beat {
                    let at = j as i128 * beat;
                    let overlap = end.min(at + beat) - start.max(at);
                    if overlap <= 0 {
                        continue;
                    }
                    let slot = if by_bar { j / layout.bar_beats } else { 0 };
                    if let Some(counts) = self.ticks.get_mut(slot).and_then(|t| t.get_mut(stream)) {
                        counts[pc] = counts[pc].saturating_add(u64::try_from(overlap).unwrap_or(u64::MAX));
                    }
                    let Some(facts) = self.beats.get_mut(j) else { continue };
                    facts.sounding |= 1 << stream;
                    if tune {
                        continue;
                    }
                    if overlap * 4 >= beat {
                        facts.chord = facts.chord.with(pc);
                    }
                    // In the bass of this beat if it sounds in the beat's first half: a note pushed ahead of the
                    // beat and held over it counts on the beat, as it's heard.
                    if start < at + half {
                        let key = u16::from(note.key);
                        if part.role == Role::Bass {
                            facts.bass = facts.bass.min(key);
                        } else {
                            facts.add_low(key);
                        }
                    }
                }
            }
        }
        Some(layout)
    }

    /// What was heard in some whole bars, with the ticks from one slot of [`KeyReader::ticks`].
    fn gather(&self, layout: Layout, bars: std::ops::Range<usize>, slot: usize) -> Heard {
        let mut heard = Heard::default();
        if let Some([tune, harmony]) = self.ticks.get(slot) {
            heard.tune = *tune;
            heard.harmony = *harmony;
        }
        for b in bars {
            let from = b.saturating_mul(layout.bar_beats);
            let to = from.saturating_add(layout.bar_beats).min(layout.beats);
            let (mut tune_beats, mut accompaniment_beats) = (0u32, 0u32);
            for (place, facts) in self.beats.get(from..to).unwrap_or_default().iter().enumerate() {
                tune_beats += u32::from(facts.sounding & 1);
                accompaniment_beats += u32::from(facts.sounding >> 1 & 1);
                let Some(root) = facts.anchor() else { continue };
                let weight = metrical_weight(place, layout.bar_beats);
                let has = |up: i32| facts.chord.contains(root.up(up));
                let add = |counts: &mut PerPitch<u64>| counts[root] = counts[root].saturating_add(weight);
                add(&mut heard.bass);
                if has(4) && has(7) {
                    add(&mut heard.major_on_bass);
                } else if has(3) && has(7) {
                    add(&mut heard.minor_on_bass);
                }
                if has(10) {
                    add(&mut heard.flat7_on_bass);
                }
                if place == 0 {
                    heard.first_bass = heard.first_bass.or(Some(root));
                    heard.last_bass = Some(root);
                }
            }
            if let Some(bar) = self.bars.get(b)
                && bar.onsets > 0
            {
                heard.notes = heard.notes.saturating_add(bar.onsets);
                heard.roles = heard.roles.union(bar.roles);
                heard.bars.push(BarPrint { print: bar.print, tune_beats, accompaniment_beats });
            }
        }
        // The bars that differ, each as first heard: the sort keeps bars with the same print in time order.
        heard.bars.sort_by_key(|b| b.print);
        heard.bars.dedup_by_key(|b| b.print);
        heard
    }
}
