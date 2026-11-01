//! What the MIDI page keeps of a reading (harmony/design/chord-reader-design.md, "API and the compact fact for the MIDI
//! page"): the chords in a few bytes a stretch ([`ChordFact`]), so the page can show a part's chords again without
//! hearing its notes again, and eight bytes its search looks in ([`ChordSummary`]).
//!
//! The fact keeps notes, not names: each stretch's first reading as its root, the notes it names and its bass, so a
//! change to how chords are named needs nothing read again. Both are kept with the reader's version
//! ([`super::CHORD_READER_VERSION`]) and made again when it changes; the fact's bytes have a layout number of their own
//! too ([`FACT_LAYOUT`]).

use std::ops::Range;

use super::answer::{ChordAnswer, NoChord, Texture};
use super::candidates::Label;
use super::cores::{Core, CoreSet};
use super::hear::{MOST_UNITS, Unit, unit_offset};
use super::name::{BassKind, SpanFlags};
use super::reader::{ChordReading, spell_chart};
use crate::chord::{Chord, is_altered};
use crate::fnv::Fnv;
use crate::key::Key;
use crate::notes::Grid;
use crate::pitch::{PcSet, PitchClass};
use crate::spell::{Degree, Spelled};

/// The layout of a fact's bytes: its first byte. A fact of another layout isn't read ([`ChordFact::read`]).
pub const FACT_LAYOUT: u8 = 1;

/// The bytes before the stretches, and the bytes of each stretch.
const HEAD: usize = 14;
const STRETCH: usize = 11;

// Where a stretch starts, and how many units and stretches there are, each fit in two bytes.
const _: () = assert!(MOST_UNITS <= u16::MAX as usize);

/// How a bass stands, as a fact numbers it (in three bits).
const BASS_KINDS: [BassKind; 5] =
    [BassKind::Root, BassKind::Inversion, BassKind::Slash, BassKind::Moving, BassKind::None];

/// What a reading has, kept in a few bytes ([`ChordReading::fact`], read back by [`ChordFact::read`]). Numbers of two
/// bytes or more are big-endian. First, 14 bytes:
///
/// - 0: the layout ([`FACT_LAYOUT`]);
/// - 1: the unit chords change on (bit 0: 1 for a beat, 0 for half a beat), the texture (bits 1 and 2: its place in
///   [`Texture::ALL`]), whether it was read as a loop (bit 3), and whether notes were left out (bit 4);
/// - 2 and 3: how many units it covers;
/// - 4 and 5: how many stretches it has;
/// - 6 to 13: the tick it starts at, in the music's own ticks.
///
/// Then 11 bytes for each stretch, in order (a 16-bar part takes about 100 to 370 bytes in all), keeping its first
/// reading as it's written, its answer and its runner-up:
///
/// - 0 and 1: its first unit, counted from the reading's first (it ends where the next starts, the last where the
///   reading ends);
/// - 2: what the decode read for its first reading: its root (the high four bits, 0 to 11 up from C) and core (the
///   low four: its place in [`Core::ALL`]); or 0xFF when it has no reading (a rest has none), and then nothing of one
///   is kept (bytes 3 to 5, 8 and 10, bits 0 to 2 of 6 and bits 3 and 4 of 7 are 0, and 9 is 0xFF);
/// - 3: the root it's written on (the high four bits) and its bass, in semitones above that root (the low four): not
///   always the root the decode read (a diminished seventh read on E is written on C# before Dm, and F over G is
///   written G9sus4);
/// - 4 and 5: the notes it names, in semitones above the root it's written on (bits 0 to 11), whether it's rootless
///   (bit 12), whether some tensions are doubtful (13), and whether the same notes are written another way too (14:
///   a sus chord over its bass, also a chord over a bass that's only a bass, or a triad over its seventh);
/// - 6: how the bass stands under it (bits 0 to 2: root, inversion, slash, moving, none), and whether it's pushed (3),
///   over a pedal (4), over a walking bass (5), over no bass (6), or half of a chord cut in two (7);
/// - 7: its answer (bits 0 to 2: its place in [`ChordAnswer::ALL`]), whether the runner-up has the same notes (3),
///   whether the best way reads a chord there (4), and whether the chord was given (5);
/// - 8: its first reading's chance, out of 255;
/// - 9: the runner-up's root and core as the decode read them, as byte 2, or 0xFF when there's none (then 10 and bit
///   3 of 7 are 0);
/// - 10: the runner-up's chance, out of 255.
///
/// A fact whose bytes break any of this is damaged, and isn't read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChordFact(pub Vec<u8>);

/// A fact read back ([`ChordFact::read`]): a reading as the MIDI page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FactReading {
    /// The unit chords change on.
    pub unit: Unit,
    pub texture: Texture,
    /// Whether it was read as a loop, and whether some notes were left out (a damaged file).
    pub looped: bool,
    pub truncated: bool,
    /// The tick it starts at, in the music's own ticks, and how many units it covers.
    pub start: i64,
    pub units: usize,
    /// Its stretches, in order: the next starts where one ends.
    pub spans: Vec<FactSpan>,
}

/// A stretch of a fact ([`ChordSpan`](super::ChordSpan) kept: its first reading, its answer and its runner-up).
#[derive(Clone, Debug, PartialEq)]
pub struct FactSpan {
    /// Its units, counted from the reading's first.
    pub units: Range<usize>,
    /// Its first reading: what the decode read (its root and core) and how it's written (its root, the notes named and
    /// the bass). None for a rest.
    pub label: Option<Label>,
    pub chord: Option<Chord>,
    /// How the bass stands under it ([`BassKind::Root`] when it has no reading).
    pub bass: BassKind,
    /// Whether its root isn't heard, whether some of its tensions are doubtful (left out of its name), and whether the
    /// same notes are written another way too (a sus chord over its bass is also a chord over a bass that's only a
    /// bass: G9sus4, also F/G; and a triad over its seventh, also the seventh chord: D/C, also D7/C).
    pub rootless: bool,
    pub doubtful: bool,
    pub also: bool,
    /// Its first reading's chance, to the nearest 255th.
    pub chance: f64,
    /// Whether the best way reads a chord there: then the first reading is the one it reads.
    pub decoded: bool,
    pub answer: ChordAnswer,
    pub flags: SpanFlags,
    pub runner_up: Option<RunnerUp>,
}

/// A stretch's second reading, as a fact keeps it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunnerUp {
    pub label: Label,
    /// Its chance, to the nearest 255th.
    pub chance: f64,
    /// Whether it's the same notes as the first, written another way (C6 and Am7/C).
    pub same_notes: bool,
}

impl ChordReading {
    /// The reading kept in a few bytes, for the MIDI page's index ([`ChordFact`]). It keeps each stretch's first two
    /// readings, so it's made from a reading that keeps two or more.
    pub fn fact(&self) -> ChordFact {
        let mut out = Vec::with_capacity(HEAD + STRETCH * self.spans.len());
        out.push(FACT_LAYOUT);
        let texture = u8::try_from(self.texture.index()).unwrap_or(0);
        let looped = u8::from(self.looped) << 3 | u8::from(self.truncated) << 4;
        out.push(u8::from(self.unit == Unit::Beat) | texture << 1 | looped);
        out.extend(two(self.spans.last().map_or(0, |s| s.units.end)));
        out.extend(two(self.spans.len()));
        out.extend(self.start.to_be_bytes());
        for s in &self.spans {
            let flag = |f: SpanFlags| u8::from(s.flags.contains(f));
            let (first, runner_up) = (s.readings.first(), s.readings.get(1));
            out.extend(two(s.units.start));
            out.push(first.map_or(0xFF, |r| label_byte(r.label)));
            let chord = first.map(|r| r.chord);
            out.push(chord.map_or(0, |c| c.root().number() << 4 | c.bass().above(c.root())));
            let notes = chord.map_or(0, |c| c.intervals().bits() & 0xFFF);
            let rootless = first.is_some_and(|r| r.rootless);
            let doubtful = first.is_some_and(|r| !r.doubtful.is_empty());
            let also = first.is_some_and(|r| r.also.is_some());
            out.extend(
                (notes | u16::from(rootless) << 12 | u16::from(doubtful) << 13 | u16::from(also) << 14).to_be_bytes(),
            );
            out.push(
                first.map_or(0, |r| bass_code(r.bass))
                    | flag(SpanFlags::PUSHED) << 3
                    | flag(SpanFlags::PEDAL) << 4
                    | flag(SpanFlags::WALKING) << 5
                    | flag(SpanFlags::NO_BASS) << 6
                    | flag(SpanFlags::SPLIT) << 7,
            );
            let same = first.zip(runner_up).is_some_and(|(a, b)| a.group == b.group);
            out.push(
                answer_code(s.answer)
                    | u8::from(same) << 3
                    | u8::from(s.label.is_some()) << 4
                    | flag(SpanFlags::GIVEN) << 5,
            );
            out.push(first.map_or(0, |r| out_of_255(r.chance)));
            out.push(runner_up.map_or(0xFF, |r| label_byte(r.label)));
            out.push(runner_up.map_or(0, |r| out_of_255(r.chance)));
        }
        ChordFact(out)
    }
}

impl ChordFact {
    /// The reading it keeps, or none when its bytes aren't a fact this layout reads: another layout, or damaged.
    pub fn read(&self) -> Option<FactReading> {
        let (head, stretches) = self.0.split_first_chunk::<HEAD>()?;
        let [layout, kind, u0, u1, c0, c1, t0, t1, t2, t3, t4, t5, t6, t7] = *head;
        if layout != FACT_LAYOUT || kind >> 5 != 0 {
            return None;
        }
        let units = usize::from(u16::from_be_bytes([u0, u1]));
        let count = usize::from(u16::from_be_bytes([c0, c1]));
        let (chunks, left) = stretches.as_chunks::<STRETCH>();
        // A reading has a stretch at least, and the fact as many as it says.
        if count == 0 || chunks.len() != count || !left.is_empty() {
            return None;
        }
        let mut spans: Vec<FactSpan> = Vec::with_capacity(count);
        for (i, chunk) in chunks.iter().enumerate() {
            let from = first_unit(chunk);
            let to = chunks.get(i + 1).map_or(units, first_unit);
            // The first starts at the reading's start, and each ends after it starts.
            if (i == 0 && from != 0) || to <= from {
                return None;
            }
            spans.push(read_stretch(from..to, chunk)?);
        }
        Some(FactReading {
            unit: if kind & 1 == 1 { Unit::Beat } else { Unit::HalfBeat },
            texture: *Texture::ALL.get(usize::from(kind >> 1 & 3))?,
            looped: kind >> 3 & 1 == 1,
            truncated: kind >> 4 & 1 == 1,
            start: i64::from_be_bytes([t0, t1, t2, t3, t4, t5, t6, t7]),
            units,
            spans,
        })
    }
}

/// Where a stretch starts: its first unit, counted from the reading's first.
fn first_unit(chunk: &[u8; STRETCH]) -> usize {
    let [high, low, ..] = *chunk;
    usize::from(u16::from_be_bytes([high, low]))
}

/// A stretch's bytes read back, or none when they're damaged.
fn read_stretch(units: Range<usize>, chunk: &[u8; STRETCH]) -> Option<FactSpan> {
    let [_, _, label, written, high, low, kind, answer, chance, runner_up, second_chance] = *chunk;
    let notes = u16::from_be_bytes([high, low]);
    let bit = |byte: u8, n: u8| byte >> n & 1 == 1;
    // Bits no layout-1 fact sets: a fact with any of them set is damaged.
    if notes >> 15 != 0 || answer >> 6 != 0 {
        return None;
    }
    let said = *ChordAnswer::ALL.get(usize::from(answer & 7))?;
    let (reading, second) = (label != 0xFF, runner_up != 0xFF);
    // A stretch with no reading has nothing of one (no root, notes, bass, chance or runner-up, and the best way reads
    // no chord there), a stretch with no runner-up has nothing of one either, and a rest has no reading.
    let no_first = written == 0 && notes == 0 && kind & 7 == 0 && chance == 0 && !second && !bit(answer, 4);
    let no_second = second_chance == 0 && !bit(answer, 3);
    let rest = said == ChordAnswer::NoChord(NoChord::Rest);
    if (!reading && !no_first) || (!second && !no_second) || (reading && rest) {
        return None;
    }
    let label = if reading { Some(label_of(label)?) } else { None };
    let (root, bass) = (written >> 4, written & 0xF);
    if root >= 12 || bass >= 12 {
        return None;
    }
    let root = PitchClass::new(i32::from(root));
    let chord = label.map(|_| Chord::new(root, PcSet::from_bits(notes & 0xFFF).on(root), root.up(i32::from(bass))));
    let mut flags = SpanFlags::NONE;
    for (yes, flag) in [
        (bit(kind, 3), SpanFlags::PUSHED),
        (bit(kind, 4), SpanFlags::PEDAL),
        (bit(kind, 5), SpanFlags::WALKING),
        (bit(kind, 6), SpanFlags::NO_BASS),
        (bit(kind, 7), SpanFlags::SPLIT),
        (bit(answer, 5), SpanFlags::GIVEN),
    ] {
        if yes {
            flags = flags.with(flag);
        }
    }
    let runner_up = if second {
        Some(RunnerUp { label: label_of(runner_up)?, chance: chance_of(second_chance), same_notes: bit(answer, 3) })
    } else {
        None
    };
    let notes_bit = |n: u32| notes >> n & 1 == 1;
    Some(FactSpan {
        units,
        label,
        chord,
        bass: *BASS_KINDS.get(usize::from(kind & 7))?,
        rootless: notes_bit(12),
        doubtful: notes_bit(13),
        also: notes_bit(14),
        chance: chance_of(chance),
        decoded: bit(answer, 4),
        answer: said,
        flags,
        runner_up,
    })
}

impl FactSpan {
    /// The chord the chart shows for it, as [`ChordSpan::shown`](super::ChordSpan::shown) gives it: the first
    /// reading's, when the answer gives a chord and the best way reads one there, or when one line alone implies it.
    pub fn shown(&self) -> Option<Chord> {
        let reads = self.decoded || self.answer == ChordAnswer::Implied;
        self.chord.filter(|_| reads && self.answer.shows_chord())
    }
}

impl FactReading {
    /// The chord chart spelled in `key`, as [`ChordReading::names`] spells it.
    pub fn names(&self, key: Option<Key>) -> Vec<Option<Spelled>> {
        spell_chart(self.spans.iter().map(FactSpan::shown), self.looped, key)
    }

    /// Where a unit starts (counted from the reading's first, up to [`FactReading::units`]), in the music's own ticks:
    /// `grid` is the grid the reading was read on.
    pub fn tick(&self, grid: Grid, unit: usize) -> i64 {
        self.start.saturating_add(unit_offset(grid, unit.min(self.units)))
    }
}

/// What a reading has, in eight bytes, for the MIDI page's search ([`ChordReading::summary`]), from the chords its
/// chart shows, as they're written:
///
/// - 0 to 3: its progression, as a number that's the same in any key ([`ChordSummary::progression`]);
/// - 4 and 5: the cores it has, a bit for each ([`ChordSummary::cores`]);
/// - 6: the tensions it names ([`Extensions`]);
/// - 7: what else it has ([`SummaryFlags`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChordSummary(pub [u8; 8]);

impl ChordSummary {
    /// Its progression: the chords shown in order, each as its root above the first one's and its core (a chord held
    /// over stretches side by side counted once, the stretches with no chord left out, and a loop from the chord it
    /// starts on), as a hash: the same progression in any key, with any tensions and over any basses, is the same
    /// number. 0 when no chord is shown, and never otherwise.
    pub fn progression(self) -> u32 {
        let [a, b, c, d, ..] = self.0;
        u32::from_be_bytes([a, b, c, d])
    }

    /// The cores of the chords it shows, as they're written (Cmaj9 as [`Core::Major7`]).
    pub fn cores(self) -> CoreSet {
        let [.., a, b, _, _] = self.0;
        Core::ALL.into_iter().filter(|c| u16::from_be_bytes([a, b]) >> c.index() & 1 == 1).collect()
    }

    pub fn extensions(self) -> Extensions {
        let [.., e, _] = self.0;
        Extensions(e)
    }

    pub fn flags(self) -> SummaryFlags {
        let [.., f] = self.0;
        SummaryFlags(f)
    }
}

/// The tensions the chords a reading shows name, as bits ([`ChordSummary::extensions`]): a 7alt chord's too (its
/// altered ninths, eleventh and thirteenth), with [`Extensions::ALTERED`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Extensions(u8);

impl Extensions {
    pub const NONE: Extensions = Extensions(0);
    pub const FLAT_NINTH: Extensions = Extensions(1);
    pub const NINTH: Extensions = Extensions(1 << 1);
    pub const SHARP_NINTH: Extensions = Extensions(1 << 2);
    pub const ELEVENTH: Extensions = Extensions(1 << 3);
    pub const SHARP_ELEVENTH: Extensions = Extensions(1 << 4);
    pub const FLAT_THIRTEENTH: Extensions = Extensions(1 << 5);
    pub const THIRTEENTH: Extensions = Extensions(1 << 6);
    /// An altered dominant, written 7alt.
    pub const ALTERED: Extensions = Extensions(1 << 7);

    pub const fn contains(self, other: Extensions) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn with(self, other: Extensions) -> Extensions {
        Extensions(self.0 | other.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// What else the chords a reading shows have, as bits ([`ChordSummary::flags`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SummaryFlags(u8);

impl SummaryFlags {
    pub const NONE: SummaryFlags = SummaryFlags(0);
    /// A chord over a bass that's only a bass (D/C).
    pub const SLASH: SummaryFlags = SummaryFlags(1);
    /// A chord over another of its notes (C/E).
    pub const INVERSION: SummaryFlags = SummaryFlags(1 << 1);
    /// A chord whose root isn't heard.
    pub const ROOTLESS: SummaryFlags = SummaryFlags(1 << 2);
    /// A chord over a pedal it isn't rooted on.
    pub const PEDAL: SummaryFlags = SummaryFlags(1 << 3);
    /// A chord pushed: struck before the bar line or beat it belongs to.
    pub const PUSHED: SummaryFlags = SummaryFlags(1 << 4);
    /// A chord over a walking bass.
    pub const WALKING: SummaryFlags = SummaryFlags(1 << 5);
    /// A sus chord over its bass that's also a chord over a bass that's only a bass (G9sus4, also F/G).
    pub const HYBRID: SummaryFlags = SummaryFlags(1 << 6);
    /// Unclear for more than a quarter of the time the music plays (rests left out).
    pub const UNCLEAR: SummaryFlags = SummaryFlags(1 << 7);

    pub const fn contains(self, other: SummaryFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn with(self, other: SummaryFlags) -> SummaryFlags {
        SummaryFlags(self.0 | other.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// The tensions a summary counts, by how a chord's name writes them.
const TENSIONS: [(Degree, Extensions); 7] = [
    (Degree::FLAT_NINTH, Extensions::FLAT_NINTH),
    (Degree::NINTH, Extensions::NINTH),
    (Degree::SHARP_NINTH, Extensions::SHARP_NINTH),
    (Degree::ELEVENTH, Extensions::ELEVENTH),
    (Degree::SHARP_ELEVENTH, Extensions::SHARP_ELEVENTH),
    (Degree::FLAT_THIRTEENTH, Extensions::FLAT_THIRTEENTH),
    (Degree::THIRTEENTH, Extensions::THIRTEENTH),
];

impl ChordReading {
    /// What the reading has, in eight bytes, for the MIDI page's search ([`ChordSummary`]).
    pub fn summary(&self) -> ChordSummary {
        summarise(self.spans.iter().map(|s| Charted {
            length: s.units.len(),
            answer: s.answer,
            flags: s.flags,
            shown: s.shown().map(|r| Shown {
                chord: r.chord,
                core: r.label.core,
                bass: r.bass,
                rootless: r.rootless,
                also: r.also.is_some(),
            }),
        }))
    }
}

impl FactReading {
    /// What the reading kept has, in eight bytes, as [`ChordReading::summary`] gives it.
    pub fn summary(&self) -> ChordSummary {
        summarise(self.spans.iter().map(|s| Charted {
            length: s.units.len(),
            answer: s.answer,
            flags: s.flags,
            shown: s.shown().zip(s.label).map(|(chord, label)| Shown {
                chord,
                core: label.core,
                bass: s.bass,
                rootless: s.rootless,
                also: s.also,
            }),
        }))
    }
}

/// A stretch as the summary sees it: how long it is, its answer and flags, and the chord the chart shows there.
struct Charted {
    length: usize,
    answer: ChordAnswer,
    flags: SpanFlags,
    shown: Option<Shown>,
}

/// A chord the chart shows: as it's written, the core the decode read (for a chord written with no core of its own),
/// how its bass stands, whether it's rootless, and whether the same notes are written another way too.
struct Shown {
    chord: Chord,
    core: Core,
    bass: BassKind,
    rootless: bool,
    also: bool,
}

/// What a reading has, in eight bytes ([`ChordSummary`]).
fn summarise(spans: impl Iterator<Item = Charted>) -> ChordSummary {
    let (mut cores, mut extensions, mut flags) = (CoreSet::EMPTY, Extensions::NONE, SummaryFlags::NONE);
    let mut progression = Fnv::new();
    let (mut first_root, mut last): (Option<PitchClass>, Option<(PitchClass, Core)>) = (None, None);
    let (mut playing, mut unclear) = (0usize, 0usize);
    for s in spans {
        if s.answer != ChordAnswer::NoChord(NoChord::Rest) {
            playing = playing.saturating_add(s.length);
        }
        if s.answer == ChordAnswer::Unclear {
            unclear = unclear.saturating_add(s.length);
        }
        let Some(shown) = s.shown else { continue };
        let c = shown.chord;
        let core = Core::of_chord(c).unwrap_or(shown.core);
        cores = cores.with(core);
        if last != Some((c.root(), core)) {
            let from = *first_root.get_or_insert(c.root());
            progression.bytes(&[c.root().above(from), u8::try_from(core.index()).unwrap_or(u8::MAX)]);
            last = Some((c.root(), core));
        }
        for interval in (0..12).filter(|&i| c.has(i)) {
            let degree = c.degree(interval);
            for (d, e) in TENSIONS {
                if d == degree {
                    extensions = extensions.with(e);
                }
            }
        }
        if is_altered(c.intervals()) {
            extensions = extensions.with(Extensions::ALTERED);
        }
        for (yes, flag) in [
            (shown.bass == BassKind::Slash, SummaryFlags::SLASH),
            (shown.bass == BassKind::Inversion, SummaryFlags::INVERSION),
            (shown.rootless, SummaryFlags::ROOTLESS),
            (s.flags.contains(SpanFlags::PEDAL), SummaryFlags::PEDAL),
            (s.flags.contains(SpanFlags::PUSHED), SummaryFlags::PUSHED),
            (s.flags.contains(SpanFlags::WALKING), SummaryFlags::WALKING),
            (shown.also && shown.bass == BassKind::Root, SummaryFlags::HYBRID),
        ] {
            if yes {
                flags = flags.with(flag);
            }
        }
    }
    if unclear.saturating_mul(4) > playing {
        flags = flags.with(SummaryFlags::UNCLEAR);
    }
    // The hash's two halves folded into one, so it takes four bytes; 0 is kept for no chord.
    let hash = progression.finish();
    let progression = if first_root.is_some() { ((hash >> 32) as u32 ^ hash as u32).max(1) } else { 0 };
    let [a, b, c, d] = progression.to_be_bytes();
    let [e, f] = cores.bits().to_be_bytes();
    ChordSummary([a, b, c, d, e, f, extensions.bits(), flags.bits()])
}

/// A number in two bytes, big-endian (a reading's units always fit).
fn two(n: usize) -> [u8; 2] {
    u16::try_from(n).unwrap_or(u16::MAX).to_be_bytes()
}

/// A label in a byte: its root in the high four bits, its core in the low four.
fn label_byte(label: Label) -> u8 {
    label.root.number() << 4 | u8::try_from(label.core.index()).unwrap_or(0) & 0xF
}

/// A label from its byte; none when the root isn't one.
fn label_of(byte: u8) -> Option<Label> {
    let root = byte >> 4;
    let core = Core::from_index(usize::from(byte & 0xF))?;
    (root < 12).then(|| Label::new(PitchClass::new(i32::from(root)), core))
}

fn bass_code(kind: BassKind) -> u8 {
    BASS_KINDS.iter().position(|&k| k == kind).and_then(|i| u8::try_from(i).ok()).unwrap_or(0)
}

fn answer_code(answer: ChordAnswer) -> u8 {
    ChordAnswer::ALL.iter().position(|&a| a == answer).and_then(|i| u8::try_from(i).ok()).unwrap_or(0)
}

/// A chance out of 255, to the nearest.
fn out_of_255(chance: f64) -> u8 {
    (chance.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn chance_of(byte: u8) -> f64 {
    f64::from(byte) / 255.0
}
