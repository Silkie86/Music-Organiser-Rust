//! Naming the chords, the last of the reader's three steps (harmony/design/chord-reader-design.md, "Extensions, roots,
//! inversions and slash chords" and "Naming policy"): what a player writes over each chord the decode read, from the
//! notes really played.
//!
//! A decoded reading is a root and a core ([`Label`]): Dm7, G7, Cmaj7. What's written ([`Chord`]) holds the core's
//! notes that are heard, the tensions the music really voices, and the bass under it.
//!
//! - **Extensions** (the gate). Each note that isn't one of the reading's own is weighed on four signals. None of them
//!   counts a note that only passes by under the reading ([`Shaped::passes_under`]), the bass's notes, or what the
//!   chords hold over a bar line:
//!   - *voiced*: struck in the chords in a unit where at least two of the reading's own notes sound;
//!   - *held*: sounding in the chords for at least half the chord's time;
//!   - *arpeggiated*: struck in a broken chord in at least two felt beats;
//!   - *in the tune*: a tune note struck on a felt beat or stronger and played for a felt beat or more, that the chord
//!     can carry (not an avoid note).
//!
//!   A tension that's held, arpeggiated, voiced and sounding a quarter of the time, or in the tune for half of it is
//!   named. One that's only voiced, or in the tune for less, is doubtful: listed ([`Reading::doubtful`]) and left out
//!   of the name. Any other is left out, as passing colour. A note that doesn't belong to the reading enters the name
//!   when the chords play it as a named tension is played, so the name shows what the voicing plays; from the tune,
//!   never: a held F over Cmaj7 in the tune is a clash, not an 11.
//! - **The root and the bass** ([`BassKind`]). The name never claims a note that isn't heard: the root of a rootless
//!   voicing is left out of its notes, and the name still shows it ([`Reading::rootless`]). A held bass is the root,
//!   another of the chord's notes (an inversion: C/E, G7/F, Cmaj9/E) or only a bass (a slash chord: D/C). Over a
//!   moving bass no slash is written.
//! - **A seventh heard only in the bass.** When the held bass is the reading's seventh and the chords and the tune
//!   don't play it, while they play the root, the third and the fifth, the name is the triad over its seventh, and the
//!   seventh chord is the other way to write it ([`Reading::also`]): D/C, also D7/C. A triad over its own seventh is
//!   written the same way: C/B, also Cmaj7/B.
//! - **Sus chords over a bass.** When the bass is only a bass and the notes over it hold its 4th and its ♭7 but no
//!   third, the name is the sus chord on the bass, and the slash chord is the other way to write it: F/G is G9sus4,
//!   Fm9/B♭ is B♭13sus4.
//! - **Pedals.** A bass held under two chords or more, one of which it isn't a note of, is a pedal: the chords over
//!   it that aren't rooted on it are flagged ([`SpanFlags::PEDAL`]) and keep their slash names (C | F/C | G/C | C).
//! - **Diminished sevenths and augmented triads** have the same notes on four and three roots, so their readings on
//!   those roots are one reading ([`Reading::roots`] lists them). It's written on the held bass when that's one of its
//!   notes; else, for a diminished seventh, on the note a semitone under the next chord's root (C♯dim7 before Dm), or
//!   on the root of the chord on both sides of it (C | Cdim7 | C); for an augmented triad, on the note a fifth above
//!   the next chord's root (G+ before C); else on its lowest note.
//! - **A bass that moves to the chord's third or seventh** halfway through it, on a bar line or the middle of a bar,
//!   each note held two felt beats or more, and steps from there into the next chord's bass, cuts the chord in two: C
//!   C/E | F, C C/B | Am ([`SpanFlags::SPLIT`]). A bass moving to the fifth (a two-feel) never does.
//! - **Pins** (a Chord check answer or the user's fix: [`super::PinnedSpan`]). A pinned stretch is never cut in two,
//!   and a chord given for it is written as given, whatever is heard ([`SpanFlags::GIVEN`]): D♭7 pinned over C E G is
//!   D♭7, rootless. Given in root position, it stands over the bass held under it, as any reading does (Am7 given over
//!   a held C is Am7/C, and that D♭7 over a C bass D♭7/C); given with a bass of its own, it keeps it. The decode says
//!   which spans are pinned and the chord given for each ([`DecodedSpan::given`]).
//!
//! Over silence (a chord held on over it, or pinned there), a reading is written in full, as the decode read it. Every
//! reading the decode gave a chance on a span is named the same way, and readings written the same are one reading,
//! their chances added. The reading the best way reads comes first, then the others by their chances. Readings
//! that hold the same notes over the same bass are one same-notes group ([`Reading::group`]): C6 and Am7/C, a
//! diminished seventh and the rootless 7(♭9) it stands for.

use std::ops::Range;

use super::candidates::Label;
use super::cores::Core;
use super::decode::{Chance, DecodedSpan, Decoder, Decoding};
use super::hear::{BassTexture, Hearing, Level, Line, Mass, Push, Shaped, Stream, played_in};
use crate::chord::{Chord, Family, ToneRole, role};
use crate::pitch::{PcSet, PerPitch, PitchClass};

/// How the bass stands under a written chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BassKind {
    /// The held bass is the root: C, Dm9.
    Root,
    /// The held bass is another of the chord's notes: C/E, C/G, G7/F, Cmaj9/E.
    Inversion,
    /// The held bass is only a bass, not one of the chord's notes: D/C.
    Slash,
    /// The bass moves under the chord (a walking or a busy bass): it leans towards the root, and no slash is written.
    Moving,
    /// No bass sounds.
    None,
}

impl BassKind {
    /// How a bass that's held, moving or not there stands under a chord.
    pub fn of(chord: Chord, bass: BassTexture) -> BassKind {
        match bass {
            BassTexture::Held(pc) if pc == chord.root() => BassKind::Root,
            BassTexture::Held(pc) if chord.notes().contains(pc) => BassKind::Inversion,
            BassTexture::Held(_) => BassKind::Slash,
            BassTexture::Moving => BassKind::Moving,
            BassTexture::None => BassKind::None,
        }
    }
}

/// What else is true of a named span, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SpanFlags(u8);

impl SpanFlags {
    pub const NONE: SpanFlags = SpanFlags(0);
    /// Its chord is pushed: struck in the unit before it starts and held over it.
    pub const PUSHED: SpanFlags = SpanFlags(1);
    /// Its chord stands over a pedal it isn't rooted on.
    pub const PEDAL: SpanFlags = SpanFlags(1 << 1);
    /// Its bass moves.
    pub const WALKING: SpanFlags = SpanFlags(1 << 2);
    /// No bass sounds under it.
    pub const NO_BASS: SpanFlags = SpanFlags(1 << 3);
    /// It's half of a chord cut in two where its bass moves to its third or seventh.
    pub const SPLIT: SpanFlags = SpanFlags(1 << 4);
    /// Its chord was given (a pin with its chord), and is written as given.
    pub const GIVEN: SpanFlags = SpanFlags(1 << 5);

    /// Whether all of some flags are set.
    pub const fn contains(self, flags: SpanFlags) -> bool {
        self.0 & flags.0 == flags.0
    }

    pub const fn with(self, flags: SpanFlags) -> SpanFlags {
        SpanFlags(self.0 | flags.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// A reading of a span, as it's written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// What the decode read and gave the chance to: for readings written the same, the one the best way reads, or else
    /// the likeliest.
    pub label: Label,
    /// What's written: its root, the notes named (the core's notes heard, the tensions named, and the notes that don't
    /// belong but are voiced as a named tension is), and the bass.
    pub chord: Chord,
    /// The same notes written the other way: F/G beside G9sus4, D7/C beside D/C.
    pub also: Option<Chord>,
    /// The roots it may be written on: a diminished seventh's four, an augmented triad's three, otherwise its root.
    pub roots: PcSet,
    pub bass: BassKind,
    /// Its root isn't heard in any part, while other notes are: a rootless voicing. The root is left out of the chord's
    /// notes, and the name still shows it.
    pub rootless: bool,
    /// Tensions heard too little to name: left out of the chord.
    pub doubtful: PcSet,
    /// Its chance on the span, with the chances of the readings written the same added.
    pub chance: f64,
    /// Its same-notes group within the span: the readings with the same notes over the same bass share one, numbered
    /// from 0 in the order they come.
    pub group: u8,
}

/// A stretch read as one chord, or as none, named.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedSpan {
    /// Its units, from the first unit heard, and the units whose sound is its own (as a [`DecodedSpan`]'s, but a chord
    /// pushed to the first unit reported is heard from where it's struck, in a loop's copied bar).
    pub units: Range<usize>,
    pub sound: Range<usize>,
    /// Where it starts and ends, in the music's own ticks, and where its chord is first struck: before `start` when
    /// it's pushed.
    pub start: i64,
    pub end: i64,
    pub heard_start: i64,
    /// The chance a chord starts where it starts.
    pub change: f64,
    /// What the best way reads there: a label, the first reading's, or none for no chord.
    pub label: Option<Label>,
    /// Whether it's a rest: then it has no readings.
    pub rest: bool,
    /// The chance it's no chord.
    pub none: f64,
    pub flags: SpanFlags,
    /// Its readings' places in [`Naming::readings`].
    readings: Range<usize>,
}

/// What the namer named ([`Namer::name`]): each span, and its readings. Keep one and reuse it.
#[derive(Clone, Debug, Default)]
pub struct Naming {
    spans: Vec<NamedSpan>,
    readings: Vec<Reading>,
}

impl Naming {
    pub fn new() -> Naming {
        Naming::default()
    }

    /// The spans, in order: the next starts where one ends.
    pub fn spans(&self) -> &[NamedSpan] {
        &self.spans
    }

    /// A span's readings: the one the best way reads first, then the others by their chances (equal chances in the
    /// decode's order, from the lowest note up).
    pub fn readings(&self, span: &NamedSpan) -> &[Reading] {
        self.readings.get(span.readings.clone()).unwrap_or_default()
    }

    fn clear(&mut self) {
        self.spans.clear();
        self.readings.clear();
    }
}

/// What the gate makes of a note that isn't one of a reading's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Band {
    /// Passing colour, a clash, or not played: left out.
    Out,
    /// Heard, but too little to name: listed beside the name.
    Doubtful,
    /// In the name.
    Named,
}

/// A stretch named as one chord: a decoded span, or half of one cut in two.
#[derive(Clone, Debug)]
struct Part {
    /// The decoded span it's from.
    span: usize,
    units: Range<usize>,
    sound: Range<usize>,
    label: Option<Label>,
    pushed: bool,
    /// Where its chord is struck when it's pushed, in ticks from the first unit's start.
    struck_at: Option<i64>,
    rest: bool,
    change: f64,
    split: bool,
    /// Whether it's pinned (never cut in two), and its chord as given, written as given.
    pinned: bool,
    given: Option<Chord>,
    /// How the bass behaves over its sound.
    bass: BassTexture,
    pedal: bool,
    /// The roots of the chords read before it and after it (round a loop), when they're chords. The halves of a span
    /// cut in two keep the span's: the chord after the first half is the same chord, not where it goes.
    before: Option<PitchClass>,
    after: Option<PitchClass>,
}

/// What a part sounds, for naming each reading on it.
#[derive(Clone, Debug)]
struct Sound {
    /// Its sound: its units, and where they start and end in ticks from the first unit's start.
    units: Range<usize>,
    from: i64,
    to: i64,
    /// S, the ticks of its sound in which a chord or bass note sounds.
    sounding: u64,
    /// The notes heard (P), and those heard counting only the chords and the tune's held notes.
    heard: PcSet,
    played: PcSet,
    bass: BassTexture,
    lowest: Option<PitchClass>,
}

/// A note that sounds in the part being named, with what it may count for in the gate whatever the reading.
#[derive(Clone, Copy, Debug)]
struct Near {
    pc: PitchClass,
    /// Its place in the notes.
    note: usize,
    /// Its shaped notes, as places in `Namer::near_shaped`.
    shaped: (usize, usize),
    counts: Counts,
}

/// What a note may count for in the gate, unless it only passes by under the reading.
#[derive(Clone, Copy, Debug)]
enum Counts {
    /// Nothing: it isn't struck in the part, it's the bass, or it's a tune note too short or struck too weakly.
    Nothing,
    /// Struck in the chords in the part: the pitch classes of the chords and the bass sounding in its unit, and its felt
    /// beat when it's in a broken chord.
    Chords { with: PcSet, beat: Option<usize> },
    /// A tune note struck on a felt beat or stronger and held a felt beat or more: where its own sound (not what it
    /// holds over a bar line) starts and ends in the part.
    Tune { start: i64, end: i64 },
}

/// A stretch in which a note sounds in the chords on the part being named: its own sound (not what it holds over a bar
/// line), less the units in which it's heard as the bass.
#[derive(Clone, Copy, Debug)]
struct Piece {
    pc: PitchClass,
    start: i64,
    end: i64,
    /// Its note's shaped notes, as places in `Namer::near_shaped`.
    shaped: (usize, usize),
}

/// A reading being named, before the readings written the same are put together.
#[derive(Clone, Copy, Debug)]
struct Draft {
    label: Label,
    chord: Chord,
    also: Option<Chord>,
    doubtful: PcSet,
    chance: f64,
    /// Whether it's (or holds) the reading the best way reads.
    decoded: bool,
}

/// Names the chords the decode read ([`Namer::name`]). Keep one and reuse it: it keeps its working space between
/// reads.
#[derive(Debug, Default)]
pub struct Namer {
    parts: Vec<Part>,
    /// The shaped notes in order of where their shaped part starts, and the longest shaped part and note.
    shaped: Vec<Shaped>,
    longest_shaped: i64,
    longest_note: i64,
    /// On the part being named: the notes that sound there in order of their pitch class, then of where they start; the
    /// shaped notes there in order of their note, then of where their shaped part starts; the pieces of the chords
    /// there in order of their pitch class; each pitch class's ticks in the chords, and those with a shaped note among
    /// them (which may pass by under a reading); and the chords' pitch classes from the lowest note up.
    near: Vec<Near>,
    in_order: Vec<Near>,
    near_shaped: Vec<Shaped>,
    pieces: Vec<Piece>,
    in_chords: PerPitch<u64>,
    shaped_in_chords: PcSet,
    upward: Vec<PitchClass>,
    keys: Vec<u8>,
    /// Working space: stretches of a pitch class's time.
    stretches: Vec<(i64, i64)>,
    chances: Vec<Chance>,
    drafts: Vec<Draft>,
    groups: Vec<PcSet>,
}

impl Namer {
    pub fn new() -> Namer {
        Namer::default()
    }

    /// Names what the last decode of `hearing` read: `decoder` is what decoded it, and `decoding` what it read. A
    /// pinned stretch is never cut in two, and a chord given for it is written as given. Each span gets at most `most`
    /// readings (at least one).
    pub fn name(&mut self, hearing: &Hearing, decoder: &Decoder, decoding: &Decoding, most: usize, out: &mut Naming) {
        out.clear();
        self.shaped.clear();
        self.shaped.extend_from_slice(hearing.shaped());
        self.shaped.sort_unstable_by_key(|s| (s.start, s.end, s.note));
        self.longest_shaped = self.shaped.iter().map(|s| s.end.saturating_sub(s.start)).max().unwrap_or(0).max(0);
        self.longest_note = hearing.notes().iter().map(|n| n.end.saturating_sub(n.start)).max().unwrap_or(0).max(0);
        self.find_parts(hearing, decoder, decoding);
        self.find_pedals();
        for p in 0..self.parts.len() {
            self.name_part(hearing, decoder, decoding, p, most.max(1), out);
        }
    }

    /// The parts: each decoded span, or its two halves where its bass moves to its third or seventh ([`split_at`]),
    /// unless it's pinned.
    fn find_parts(&mut self, hearing: &Hearing, decoder: &Decoder, decoding: &Decoding) {
        self.parts.clear();
        let spans = decoding.spans();
        // A loop goes round: its first chord comes after its last.
        let looped = hearing.reported().start > 0;
        let around = |i: Option<usize>| match i {
            Some(i) if i < spans.len() => spans.get(i),
            Some(_) if looped => spans.first(),
            None if looped => spans.last(),
            _ => None,
        };
        let root = |span: Option<&DecodedSpan>| span.filter(|s| !s.rest).and_then(|s| s.label).map(|l| l.root);
        for (i, span) in spans.iter().enumerate() {
            let (before, after) = (around(i.checked_sub(1)), around(Some(i + 1)));
            // A pushed chord is named from where it's struck. At the first unit reported that's in a loop's copied bar,
            // before the units the decode reports its sound from: it scored the span with it there all the same
            // ([`super::Evidence::facts`]), and the span's chances are its own units'.
            let struck_at = span.pushed.then(|| push_to(hearing, span.units.start)).flatten().map(|push| push.at);
            let struck = struck_at.map_or(span.sound.start, |at| hearing.unit_of(at).min(span.sound.start));
            let sound = struck..span.sound.end;
            let part = Part {
                span: i,
                units: span.units.clone(),
                sound: sound.clone(),
                label: span.label,
                pushed: span.pushed,
                struck_at,
                rest: span.rest,
                change: span.change,
                split: false,
                pinned: span.pinned,
                given: span.given,
                bass: hearing.bass(sound),
                pedal: false,
                before: root(before),
                after: root(after),
            };
            let next_bass = after.filter(|s| !s.rest).and_then(|s| first_bass(hearing, s.sound.clone()));
            match split_at(hearing, &part, next_bass).filter(|_| !part.pinned) {
                Some(k) => {
                    let first = Part {
                        units: part.units.start..k,
                        sound: part.sound.start..k,
                        split: true,
                        bass: hearing.bass(part.sound.start..k),
                        ..part.clone()
                    };
                    let second = Part {
                        units: k..part.units.end,
                        sound: k..part.sound.end,
                        pushed: false,
                        struck_at: None,
                        change: decoder.change(k),
                        split: true,
                        bass: hearing.bass(k..part.sound.end),
                        ..part
                    };
                    self.parts.push(first);
                    self.parts.push(second);
                }
                None => self.parts.push(part),
            }
        }
    }

    /// Flags the chords over a pedal: in a run of two chords or more over one held bass, with the bass not one of the
    /// notes of one of them, each chord not rooted on it.
    fn find_pedals(&mut self) {
        let mut i = 0;
        while let Some(first) = self.parts.get(i) {
            let (Some(bass), Some(label), false) = (first.bass.held(), first.label, first.rest) else {
                i += 1;
                continue;
            };
            let over = |p: &Part| p.bass == BassTexture::Held(bass) && p.label.is_some() && !p.rest;
            let end = i + self.parts.get(i..).unwrap_or_default().iter().take_while(|p| over(p)).count();
            let run = self.parts.get_mut(i..end).unwrap_or_default();
            let changes = run.iter().any(|p| p.label != Some(label));
            let off = run.iter().any(|p| p.label.is_some_and(|l| !l.tones().contains(bass)));
            if changes && off {
                for p in run.iter_mut() {
                    p.pedal = p.label.is_some_and(|l| !rooted_on(l, bass));
                }
            }
            i = end.max(i + 1);
        }
    }

    /// Names a part, into `out`.
    fn name_part(
        &mut self,
        hearing: &Hearing,
        decoder: &Decoder,
        decoding: &Decoding,
        p: usize,
        most: usize,
        out: &mut Naming,
    ) {
        let Some(part) = self.parts.get(p).cloned() else { return };
        // The readings' chances: the decoded span's, or each half's own.
        self.chances.clear();
        if part.split {
            decoder.chances(hearing, part.sound.clone(), &mut self.chances);
        } else if let Some(span) = decoding.spans().get(part.span) {
            self.chances.extend_from_slice(decoding.chances(span));
        }
        let none = self.chances.iter().filter(|c| c.label.is_none()).map(|c| c.chance).sum::<f64>().min(1.0);
        let first = out.readings.len();
        if !part.rest {
            let sound = sound(hearing, &part);
            self.gather(hearing, &sound);
            self.drafts.clear();
            for k in 0..self.chances.len() {
                let Some(Chance { label: Some(label), chance }) = self.chances.get(k).copied() else { continue };
                let draft = self.draft(&sound, &part, label, chance);
                self.add(draft);
            }
            // The reading the best way reads is always named, even with no chance left on the part.
            if let Some(label) = part.label
                && !self.drafts.iter().any(|d| d.decoded)
            {
                let draft = self.draft(&sound, &part, label, 0.0);
                self.add(draft);
            }
            self.drafts.sort_by(|a, b| b.decoded.cmp(&a.decoded).then(b.chance.total_cmp(&a.chance)));
            self.drafts.truncate(most);
            let held = sound.bass.held().map_or(PcSet::EMPTY, |pc| PcSet::EMPTY.with(pc));
            self.groups.clear();
            for d in &self.drafts {
                let notes = d.chord.notes().union(held);
                let group = match self.groups.iter().position(|&g| g == notes) {
                    Some(group) => group,
                    None => {
                        self.groups.push(notes);
                        self.groups.len() - 1
                    }
                };
                out.readings.push(Reading {
                    label: d.label,
                    chord: d.chord,
                    also: d.also,
                    roots: roots_of(d.chord),
                    // A bass written under the chord is the bass it stands on (a chord given with its own bass too).
                    bass: if d.chord.bass() == d.chord.root() {
                        BassKind::of(d.chord, sound.bass)
                    } else {
                        BassKind::of(d.chord, BassTexture::Held(d.chord.bass()))
                    },
                    rootless: !sound.heard.is_empty() && !sound.heard.contains(d.chord.root()),
                    doubtful: d.doubtful.difference(d.chord.notes()),
                    chance: d.chance.min(1.0),
                    group: u8::try_from(group).unwrap_or(u8::MAX),
                });
            }
        }
        let mut flags = SpanFlags::NONE;
        for (yes, flag) in [
            (part.pushed, SpanFlags::PUSHED),
            (part.pedal, SpanFlags::PEDAL),
            (part.bass == BassTexture::Moving, SpanFlags::WALKING),
            (part.bass == BassTexture::None, SpanFlags::NO_BASS),
            (part.split, SpanFlags::SPLIT),
            (part.given.is_some(), SpanFlags::GIVEN),
        ] {
            if yes {
                flags = flags.with(flag);
            }
        }
        let start = hearing.start(part.units.start);
        out.spans.push(NamedSpan {
            units: part.units.clone(),
            sound: part.sound.clone(),
            start,
            end: hearing.start(part.units.end),
            heard_start: part.struck_at.map_or(start, |at| hearing.tick(at)),
            change: part.change,
            label: part.label.filter(|_| !part.rest),
            rest: part.rest,
            none,
            flags,
            readings: first..out.readings.len(),
        });
    }

    /// Finds the notes, the shaped notes and the pieces of the chords that sound in a part, and what they may count for
    /// whatever the reading.
    fn gather(&mut self, hearing: &Hearing, sound: &Sound) {
        let notes = hearing.notes();
        let from = notes.partition_point(|n| n.start < sound.from.saturating_sub(self.longest_note));
        let from_shaped = self.shaped.partition_point(|s| s.start < sound.from.saturating_sub(self.longest_shaped));
        self.near_shaped.clear();
        let shaped = self.shaped.get(from_shaped..).unwrap_or_default().iter().take_while(|s| s.start < sound.to);
        self.near_shaped.extend(shaped.filter(|s| s.end > sound.from));
        self.near_shaped.sort_unstable_by_key(|s| (s.note, s.start, s.end));
        let (felt_units, felt) = (hearing.felt_units().max(1), hearing.felt_ticks());
        self.in_order.clear();
        for (i, n) in notes.iter().enumerate().skip(from).take_while(|(_, n)| n.start < sound.to) {
            if n.end <= sound.from {
                continue;
            }
            let first = self.near_shaped.partition_point(|s| s.note < i);
            let shaped =
                (first, first + self.near_shaped.get(first..).unwrap_or_default().partition_point(|s| s.note == i));
            let struck_here = n.struck && n.start >= sound.from && n.start < sound.to;
            let counts = match n.stream {
                Stream::Harmony if struck_here => {
                    let unit = hearing.unit_of(n.start);
                    if hearing.is_bass_in(unit, n) {
                        Counts::Nothing
                    } else {
                        let with = hearing.sounding(unit, Stream::Harmony).union(hearing.sounding(unit, Stream::Bass));
                        Counts::Chords { with, beat: n.broken.then_some(unit / felt_units) }
                    }
                }
                Stream::Melody
                    if n.struck
                        && n.length >= felt
                        && hearing.strength_at(n.start).is_some_and(|level| level >= Level::Beat) =>
                {
                    let end = n.held_over.unwrap_or(n.end).clamp(n.start, n.end).min(sound.to);
                    Counts::Tune { start: n.start.max(sound.from), end }
                }
                Stream::Harmony | Stream::Melody | Stream::Bass => Counts::Nothing,
            };
            self.in_order.push(Near { pc: n.pitch_class(), note: i, shaped, counts });
        }
        // In order of their pitch class, each kept in order of where they start.
        let mut first = PerPitch::<usize>::default();
        for near in &self.in_order {
            first[near.pc] += 1;
        }
        let mut total = 0;
        for pc in PitchClass::ALL {
            (first[pc], total) = (total, total + first[pc]);
        }
        self.near.clear();
        self.near.extend_from_slice(&self.in_order);
        for near in &self.in_order {
            if let Some(slot) = self.near.get_mut(first[near.pc]) {
                *slot = *near;
            }
            first[near.pc] += 1;
        }
        // The chords' notes in the part: their own sound, less the units where they're heard as the bass (only where
        // no bass part plays).
        let gap = sound.units.clone().any(|u| hearing.gap_bass(u).is_some());
        self.pieces.clear();
        for near in &self.near {
            let Some(n) = notes.get(near.note).filter(|n| n.stream == Stream::Harmony) else { continue };
            let (pc, shaped) = (near.pc, near.shaped);
            let end = n.held_over.unwrap_or(n.end).clamp(n.start, n.end).min(sound.to);
            let mut start = n.start.max(sound.from);
            if !gap {
                if start < end {
                    self.pieces.push(Piece { pc, start, end, shaped });
                }
                continue;
            }
            let mut joined = false;
            while start < end {
                let unit = hearing.unit_of(start);
                let next = hearing.offset(unit + 1).min(end);
                if next <= start {
                    break;
                }
                let bass = hearing.is_bass_in(unit, n);
                match self.pieces.last_mut() {
                    Some(p) if joined && !bass && p.end == start => p.end = next,
                    _ if !bass => self.pieces.push(Piece { pc, start, end: next, shaped }),
                    _ => {}
                }
                joined = !bass;
                start = next;
            }
        }
        // Each pitch class's ticks in the chords, and those that have a shaped note among them.
        self.in_chords = PerPitch::default();
        self.shaped_in_chords = PcSet::EMPTY;
        for pc in PitchClass::ALL {
            if self.pieces_of(pc).any(|p| self.near_shaped.get(p.shaped.0..p.shaped.1).is_some_and(|s| !s.is_empty())) {
                self.shaped_in_chords = self.shaped_in_chords.with(pc);
            }
            self.in_chords[pc] = self.held_by(pc, |_| false);
        }
        // The chords' notes (not the bass) from the lowest up, each pitch class once.
        self.keys.clear();
        for near in &self.near {
            let Some(n) = notes.get(near.note) else { continue };
            if n.stream == Stream::Harmony && !hearing.is_bass_in(hearing.unit_of(n.start.max(sound.from)), n) {
                self.keys.push(n.key);
            }
        }
        self.keys.sort_unstable();
        self.upward.clear();
        for pc in self.keys.iter().map(|&k| PitchClass::of_key(k)) {
            if !self.upward.contains(&pc) {
                self.upward.push(pc);
            }
        }
    }

    /// A reading named on a part, before the readings written the same are put together.
    fn draft(&mut self, sound: &Sound, part: &Part, label: Label, chance: f64) -> Draft {
        let decoded = part.label == Some(label);
        // A chord given is written as given; one given in root position stands over the bass held under it, as any
        // reading does.
        if decoded && let Some(given) = part.given {
            let chord = match sound.bass.held() {
                Some(bass) if given.bass() == given.root() => Chord::new(given.root(), given.notes(), bass),
                _ => given,
            };
            return Draft { label, chord, also: None, doubtful: PcSet::EMPTY, chance, decoded };
        }
        // Over silence (a chord held on over it, or pinned there), a reading is written in full, as the decode read it.
        if sound.heard.is_empty() {
            let chord = written_root(Chord::of(label.root, label.core.intervals()), None, part, &[], None);
            return Draft { label, chord, also: None, doubtful: PcSet::EMPTY, chance, decoded };
        }
        let (named, doubtful) = self.gate(sound, label);
        let notes = label.tones().intersection(sound.heard).union(named);
        let held = sound.bass.held();
        let chord = Chord::new(label.root, notes, held.unwrap_or(label.root));
        let mut chord = written_root(chord, held, part, &self.upward, sound.lowest);
        let mut also = None;
        if let Some((triad, seventh)) = seventh_in_bass(label, notes, held, sound.played) {
            chord = triad;
            also = Some(seventh);
        } else if let (false, BassKind::Slash, Some(sus)) =
            (part.pedal, BassKind::of(chord, sound.bass), sus_over_bass(chord))
        {
            also = Some(chord);
            chord = sus;
        }
        Draft { label, chord, also, doubtful, chance, decoded }
    }

    /// Puts a draft with the drafts so far: with the one written the same, if there is one. The reading the best way
    /// reads, or else the first (the likeliest), says what was decoded and which tensions are doubtful (they depend on
    /// the reading); the chances add up.
    fn add(&mut self, draft: Draft) {
        match self.drafts.iter_mut().find(|d| d.chord == draft.chord) {
            Some(d) => {
                if draft.decoded && !d.decoded {
                    d.label = draft.label;
                    d.doubtful = draft.doubtful;
                }
                d.chance += draft.chance;
                d.decoded |= draft.decoded;
                d.also = d.also.or(draft.also);
            }
            None => self.drafts.push(draft),
        }
    }

    /// What the gate makes of the notes that aren't a reading's own on a part: the notes named (tensions, and others
    /// voiced as a named tension is), and the tensions doubtful.
    fn gate(&mut self, sound: &Sound, label: Label) -> (PcSet, PcSet) {
        let (tones, tensions) = (label.tones(), label.tensions());
        let intervals = label.core.intervals();
        let family = Family::of(intervals);
        let (mut named, mut doubtful) = (PcSet::EMPTY, PcSet::EMPTY);
        for pc in PcSet::ALL.difference(tones).iter() {
            // The tune counts only for a tension the chord can carry there: never for a note that doesn't belong.
            let interval = i32::from(pc.above(label.root));
            let tension = tensions.contains(pc);
            let tune = tension && role(intervals, family, interval) != ToneRole::Avoid;
            match self.band(sound, tones, pc, tune) {
                Band::Named => named = named.with(pc),
                Band::Doubtful if tension => doubtful = doubtful.with(pc),
                Band::Doubtful | Band::Out => {}
            }
        }
        (named, doubtful)
    }

    /// What the gate makes of a pitch class `pc` on a part, under a reading whose own notes are `tones`; `tune` when the
    /// tune may count.
    fn band(&mut self, sound: &Sound, tones: PcSet, pc: PitchClass, tune: bool) -> Band {
        let s = sound.sounding;
        if s == 0 {
            return Band::Out;
        }
        // Its ticks in the chords, but for what only passes by under the reading.
        let held = if self.shaped_in_chords.contains(pc) {
            self.held_by(pc, |shaped| shaped.line != Line::Bass && shaped.passes_under(tones))
        } else {
            self.in_chords[pc]
        };
        let (mut voiced, mut arpeggiated) = (false, false);
        let mut beat: Option<usize> = None;
        // The tune's time on it from notes that count (they're in order of where they start, so their overlaps are
        // added up once), and where the last of them ends.
        let (mut in_tune, mut tune_to) = (0u64, sound.from);
        let first = self.near.partition_point(|near| near.pc < pc);
        for near in self.near.get(first..).unwrap_or_default().iter().take_while(|near| near.pc == pc) {
            if matches!(near.counts, Counts::Nothing) || self.passes(near.shaped, tones) {
                continue;
            }
            match near.counts {
                Counts::Chords { with, beat: here } => {
                    voiced |= with.intersection(tones).len() >= 2;
                    if let Some(here) = here {
                        arpeggiated |= beat.is_some_and(|b| b != here);
                        beat = beat.or(Some(here));
                    }
                }
                Counts::Tune { start, end } if tune => {
                    let start = start.max(tune_to);
                    in_tune = in_tune.saturating_add(u64::try_from(end.saturating_sub(start)).unwrap_or(0));
                    tune_to = tune_to.max(end);
                }
                Counts::Tune { .. } | Counts::Nothing => {}
            }
        }
        let half = |ticks: u64| ticks.saturating_mul(2) >= s;
        if half(held) || arpeggiated || (voiced && held.saturating_mul(4) >= s) || (in_tune > 0 && half(in_tune)) {
            Band::Named
        } else if voiced || in_tune > 0 {
            Band::Doubtful
        } else {
            Band::Out
        }
    }

    /// The pieces of the chords of a pitch class on the part being named.
    fn pieces_of(&self, pc: PitchClass) -> impl Iterator<Item = &Piece> {
        let first = self.pieces.partition_point(|p| p.pc < pc);
        self.pieces.get(first..).unwrap_or_default().iter().take_while(move |p| p.pc == pc)
    }

    /// The ticks in which a pitch class sounds in the chords on the part being named, less where a note sounding it
    /// only passes by (`passing` says which shaped notes do): each tick once, however many notes sound it then.
    fn held_by(&mut self, pc: PitchClass, passing: impl Fn(&Shaped) -> bool) -> u64 {
        let mut stretches = std::mem::take(&mut self.stretches);
        stretches.clear();
        for piece in self.pieces_of(pc) {
            let mut from = piece.start;
            for sh in self.near_shaped.get(piece.shaped.0..piece.shaped.1).unwrap_or_default().iter() {
                let (a, b) = (sh.start.max(piece.start), sh.end.min(piece.end));
                if a >= b || !passing(sh) {
                    continue;
                }
                if a > from {
                    stretches.push((from, a));
                }
                from = from.max(b);
            }
            if from < piece.end {
                stretches.push((from, piece.end));
            }
        }
        stretches.sort_unstable();
        let (mut ticks, mut to) = (0u64, i64::MIN);
        for &(a, b) in &stretches {
            let a = a.max(to);
            if b > a {
                ticks = ticks.saturating_add(u64::try_from(b - a).unwrap_or(0));
                to = b;
            }
        }
        self.stretches = stretches;
        ticks
    }

    /// Whether a note only passes by under a reading whose own notes are `tones`, from its shaped notes (as places in
    /// `near_shaped`).
    fn passes(&self, shaped: (usize, usize), tones: PcSet) -> bool {
        let shaped = self.near_shaped.get(shaped.0..shaped.1).unwrap_or_default();
        shaped.iter().any(|sh| sh.line != Line::Bass && sh.passes_under(tones))
    }
}

/// What a part sounds.
fn sound(hearing: &Hearing, part: &Part) -> Sound {
    let units = part.sound.clone();
    let s = hearing.sounding_ticks(units.clone());
    let (chords, held_tune) =
        (hearing.mass(units.clone(), Mass::Harmony), hearing.mass(units.clone(), Mass::HeldMelody));
    Sound {
        from: hearing.offset(units.start),
        to: hearing.offset(units.end),
        sounding: s,
        heard: hearing.heard(units.clone()),
        played: played_in(&chords, &held_tune, s),
        bass: part.bass,
        lowest: hearing.lowest(units.clone()).map(PitchClass::of_key),
        units,
    }
}

/// The push to a unit, if there is one (the pushes are in order of their units).
fn push_to(hearing: &Hearing, unit: usize) -> Option<&Push> {
    let pushes = hearing.pushes();
    pushes.get(pushes.partition_point(|p| p.unit < unit)).filter(|p| p.unit == unit)
}

/// The first bass note of some units: the lowest of those struck first; when none is struck there, the bass held.
fn first_bass(hearing: &Hearing, units: Range<usize>) -> Option<PitchClass> {
    hearing.bass_onset(units.clone()).or_else(|| hearing.bass(units).held())
}

/// Where a part is cut in two, when its bass moves from its root to its third or its seventh (either seventh under a
/// major or minor triad) on a bar line or the middle of a bar, each held at least two felt beats, and the second steps
/// (by one or two semitones) into the next chord's first bass note (`next_bass`): the unit its second half starts at.
/// A diminished seventh or an augmented triad is never cut: any of its notes is its root.
fn split_at(hearing: &Hearing, part: &Part, next_bass: Option<PitchClass>) -> Option<usize> {
    let label = part.label.filter(|l| !part.rest && !matches!(l.core, Core::Diminished7 | Core::Augmented))?;
    let next_bass = next_bass?;
    let two_beats = hearing.felt_units().max(1).saturating_mul(2);
    let two_beats_ticks = u64::try_from(hearing.felt_ticks()).unwrap_or(0).saturating_mul(2);
    let sound = part.sound.clone();
    let cut = |k: usize| {
        if hearing.level(k) < Level::HalfBar {
            return false;
        }
        let (first, second) = (sound.start..k, k..sound.end);
        let (BassTexture::Held(root), BassTexture::Held(to)) =
            (hearing.bass(first.clone()), hearing.bass(second.clone()))
        else {
            return false;
        };
        let up = to.above(label.root);
        let third = matches!(up, 3 | 4) && label.core.third().has(i32::from(up));
        let triad = matches!(label.core, Core::Major | Core::Minor);
        let seventh = matches!(up, 10 | 11) && (triad || label.core.intervals().has(i32::from(up)));
        let step = to.above(next_bass).min(next_bass.above(to));
        root == label.root
            && (third || seventh)
            && matches!(step, 1 | 2)
            && hearing.mass(first, Mass::Bass)[root] >= two_beats_ticks
            && hearing.mass(second, Mass::Bass)[to] >= two_beats_ticks
    };
    (sound.start.saturating_add(two_beats)..=sound.end.saturating_sub(two_beats)).find(|&k| cut(k))
}

/// Whether a reading is rooted on a pitch class: its root, or one of the roots a diminished seventh or an augmented
/// triad may be written on.
fn rooted_on(label: Label, pc: PitchClass) -> bool {
    match label.core {
        Core::Diminished7 | Core::Augmented => label.tones().contains(pc),
        _ => label.root == pc,
    }
}

/// A chord as written on the root it's written on. A diminished seventh or an augmented triad (with any notes added
/// that don't change which it is) is written on the held bass when that's one of its notes; else, for a diminished
/// seventh, on the note a semitone under the next chord's root, or on the root of the chord on both sides of it; for an
/// augmented triad, on the note a fifth above the next chord's root; else on its lowest note in the chords (`upward`
/// holds their pitch classes from the lowest up); else on the first of its notes up from the lowest note heard. Any
/// other chord stays on its root.
fn written_root(
    chord: Chord,
    held: Option<PitchClass>,
    part: &Part,
    upward: &[PitchClass],
    lowest: Option<PitchClass>,
) -> Chord {
    let Some(core @ (Core::Diminished7 | Core::Augmented)) = Core::of(chord.intervals()) else {
        return chord;
    };
    let roots = core.intervals().on(chord.root());
    let leads = part.after.map(|next| if core == Core::Diminished7 { next.up(-1) } else { next.up(7) });
    let both_sides = part.before.filter(|_| core == Core::Diminished7 && part.before == part.after);
    let lowest_note = upward.iter().copied().find(|&pc| roots.contains(pc));
    let up_from_lowest = lowest.and_then(|low| (0..12).map(|k| low.up(k)).find(|&pc| roots.contains(pc)));
    let written = [held, leads, both_sides, lowest_note, up_from_lowest]
        .into_iter()
        .flatten()
        .filter(|&pc| roots.contains(pc))
        .map(|pc| Chord::new(pc, chord.notes(), held.unwrap_or(pc)))
        .find(|c| Core::of(c.intervals()) == Some(core));
    written.unwrap_or(chord)
}

/// The roots a chord as written may be written on: a diminished seventh's four and an augmented triad's three (with
/// any notes added that don't change which it is), any other chord's own.
fn roots_of(chord: Chord) -> PcSet {
    match Core::of(chord.intervals()) {
        Some(core @ (Core::Diminished7 | Core::Augmented)) => core.intervals().on(chord.root()),
        _ => PcSet::EMPTY.with(chord.root()),
    }
}

/// A triad over its own seventh: when the held bass is a seventh of the reading's triad (its own seventh, when it has
/// one), the chords and the tune don't play it, and they play the root, the third and the fifth. The triad as a slash
/// chord, and the seventh chord, to write the same notes the other way.
fn seventh_in_bass(label: Label, notes: PcSet, held: Option<PitchClass>, played: PcSet) -> Option<(Chord, Chord)> {
    let bass = held?;
    let third = match (label.core, bass.above(label.root)) {
        (Core::Major, 10 | 11) | (Core::Dominant7, 10) | (Core::Major7, 11) => 4,
        (Core::Minor, 10 | 11) | (Core::Minor7, 10) | (Core::MinorMajor7, 11) => 3,
        _ => return None,
    };
    let triad = PcSet::of(&[0, third, 7]).on(label.root);
    if played.contains(bass) || !played.contains_all(triad) {
        return None;
    }
    let over = notes.without(bass);
    Some((Chord::new(label.root, over, bass), Chord::new(label.root, over.with(bass), bass)))
}

/// The sus chord on a bass that's only a bass, when the notes over it hold its 4th and its ♭7 and no third: G9sus4
/// for F/G.
fn sus_over_bass(chord: Chord) -> Option<Chord> {
    let bass = chord.bass();
    let over = chord.notes().above(bass);
    let sus = over.has(5) && over.has(10) && !over.has(3) && !over.has(4);
    sus.then(|| Chord::new(bass, chord.notes().with(bass), bass))
}
