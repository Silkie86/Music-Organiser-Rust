//! Chord labels in Harte's syntax ("C#:min7/b3", "A:(1,b3,5)", "N"), read and compared exactly as `mir_eval.chord`
//! does, so the bench's chord numbers mean what published chord-recognition numbers mean.
//!
//! mir_eval turns a label into a root, twelve flags for the intervals above it (the bass added) and the bass. A
//! quality's ninths, elevenths and thirteenths aren't among the flags ("C:9" has the flags of "C:7"), nor is any
//! interval written in brackets an octave or more up ("C:maj(9)" is "C:maj"): the scores are about the chord's core.

use mo_harmony::chord::{Family, ToneRole, role};
use mo_harmony::{Chord, PcSet, PitchClass};

/// A label as mir_eval encodes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Label {
    /// 0 for C up to 11; -1 for no chord or an unknown one.
    pub root: i8,
    /// For each interval above the root (0 the root itself), 1 when it's in the chord; all -1 for an unknown chord
    /// ("X"), all 0 for no chord ("N").
    pub semitones: [i8; 12],
    /// The bass, in semitones above the root; -1 for no chord or an unknown one.
    pub bass: i8,
}

impl Label {
    pub const NO_CHORD: Label = Label { root: -1, semitones: [0; 12], bass: -1 };
    pub const UNKNOWN: Label = Label { root: -1, semitones: [-1; 12], bass: -1 };

    /// Whether it's "X", which every score leaves out.
    pub fn is_unknown(&self) -> bool {
        self.semitones.iter().any(|&s| s < 0)
    }

    /// The label a chord-chart writer would give a chord the engine read, as mir_eval would encode it: its root,
    /// third, fifth and seventh (or sixth), with tensions left out as a labelled "C:9" or "C:7(b9)" leaves them out,
    /// and the bass added.
    pub fn of_chord(chord: Chord) -> Label {
        let intervals = chord.intervals();
        let family = chord.family();
        // mir_eval always counts the root, even when the chord leaves it out (a rootless voicing).
        let mut semitones = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0i8];
        for (i, s) in semitones.iter_mut().enumerate().skip(1) {
            let i = i as i32;
            if intervals.has(i) && is_core(role(intervals, family, i), family, i) {
                *s = 1;
            }
        }
        let bass = chord.bass().above(chord.root());
        if let Some(s) = semitones.get_mut(usize::from(bass)) {
            *s = 1;
        }
        Label { root: chord.root().number() as i8, semitones, bass: bass as i8 }
    }

    /// A chord read, written in Harte's syntax so that mir_eval reads it as [`Label::of_chord`] does: its core notes
    /// as degrees within the octave, its tensions as ninths, elevenths and thirteenths, which mir_eval scores
    /// without but keeps when it joins chords in a row ("C:(1,3,5,b7,9,13)/3").
    pub fn write(chord: Chord) -> String {
        const SIMPLE: [&str; 12] = ["1", "b2", "2", "b3", "3", "4", "b5", "5", "#5", "6", "b7", "7"];
        // The third, the fifth and the sevenths are always core (`is_core`), so they're never looked up here.
        const COMPOUND: [&str; 12] = ["1", "b9", "9", "#9", "3", "11", "#11", "5", "b13", "13", "b7", "7"];
        let intervals = chord.intervals();
        let family = chord.family();
        let degrees: Vec<&str> = std::iter::once("1")
            .chain((1..12).filter(|&i| intervals.has(i)).filter_map(|i| {
                let table = if is_core(role(intervals, family, i), family, i) { &SIMPLE } else { &COMPOUND };
                table.get(i as usize).copied()
            }))
            .collect();
        let root = mo_harmony::names::name(&mo_harmony::names::SHARPS, chord.root());
        let mut text = format!("{root}:({})", degrees.join(","));
        let bass = chord.bass().above(chord.root());
        if let Some(b) = SIMPLE.get(usize::from(bass)).filter(|_| bass != 0) {
            text.push('/');
            text.push_str(b);
        }
        text
    }

    /// The pitch classes it holds, bit 0 for C (the bass among them); empty for no chord or an unknown one.
    pub fn pitch_classes(&self) -> PcSet {
        if self.root < 0 {
            return PcSet::EMPTY;
        }
        let intervals: PcSet = (0..12)
            .filter(|&i| self.semitones.get(i).is_some_and(|&s| s > 0))
            .map(|i| PitchClass::new(i as i32))
            .collect();
        intervals.on(PitchClass::new(i32::from(self.root)))
    }
}

/// Whether a label writes a note of a chord within the octave (scored by mir_eval) rather than as a ninth, eleventh
/// or thirteenth (which mir_eval scores without).
fn is_core(r: ToneRole, family: Family, interval: i32) -> bool {
    match r {
        ToneRole::Root | ToneRole::Third | ToneRole::Fifth | ToneRole::Seventh => true,
        // A chord with no third has nothing to stack tensions on: every note is written as it is ("C:(1,b7)").
        _ if matches!(family, Family::Power) => true,
        // "C:sus4(2)" and "C:sus2(4)" keep both, as both are under an octave.
        _ if matches!(family, Family::Sus) && matches!(interval, 2 | 5) => true,
        // The third, the fifth and the sevenths are always written within the octave, even when they're odd ones out
        // (a major 7th beside a flat 7th); the other notes are the ninths, elevenths and thirteenths.
        _ => matches!(interval, 4 | 7 | 10 | 11),
    }
}

/// mir_eval's QUALITIES: each shorthand's intervals above the root.
const QUALITIES: [(&str, [i8; 12]); 29] = [
    ("maj", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0]),
    ("min", [1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0]),
    ("aug", [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]),
    ("dim", [1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0]),
    ("sus4", [1, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0]),
    ("sus2", [1, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0]),
    ("7", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("maj7", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1]),
    ("min7", [1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0]),
    ("minmaj7", [1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1]),
    ("maj6", [1, 0, 0, 0, 1, 0, 0, 1, 0, 1, 0, 0]),
    ("min6", [1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0]),
    ("dim7", [1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0]),
    ("hdim7", [1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0]),
    ("maj9", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1]),
    ("min9", [1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0]),
    ("9", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("b9", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("#9", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("min11", [1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0]),
    ("11", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("#11", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("maj13", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1]),
    ("min13", [1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0]),
    ("13", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("b13", [1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0]),
    ("1", [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    ("5", [1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]),
    ("", [0; 12]),
];

/// mir_eval's EXTENDED_QUALITY_REDUX, used only to tell when two labels in a row are the same chord.
const REDUX: [(&str, &str, &[&str]); 13] = [
    ("minmaj7", "min", &["7"]),
    ("maj9", "maj7", &["9"]),
    ("min9", "min7", &["9"]),
    ("9", "7", &["9"]),
    ("b9", "7", &["b9"]),
    ("#9", "7", &["#9"]),
    ("11", "7", &["9", "11"]),
    ("#11", "7", &["9", "#11"]),
    ("13", "7", &["9", "11", "13"]),
    ("b13", "7", &["9", "11", "b13"]),
    ("min11", "min7", &["9", "11"]),
    ("maj13", "maj7", &["9", "11", "13"]),
    ("min13", "min7", &["9", "11", "13"]),
];

fn quality(name: &str) -> Option<[i8; 12]> {
    QUALITIES.iter().find(|(q, _)| *q == name).map(|(_, s)| *s)
}

/// A scale degree ("b3", "#11", "7") in semitones above the root, as mir_eval's `scale_degree_to_semitone`.
fn degree(text: &str) -> Option<i32> {
    let (offset, number) = if let Some(rest) = text.strip_prefix('#') {
        let n = rest.trim_start_matches('#');
        (1 + (rest.len() - n.len()) as i32, n)
    } else if let Some(rest) = text.strip_prefix('b') {
        let n = rest.trim_start_matches('b');
        (-1 - (rest.len() - n.len()) as i32, n)
    } else {
        (0, text)
    };
    let semitone = match number {
        "1" => 0,
        "2" => 2,
        "3" => 4,
        "4" => 5,
        "5" => 7,
        "6" => 9,
        "7" => 11,
        "8" => 12,
        "9" => 14,
        "10" => 16,
        "11" => 17,
        "12" => 19,
        "13" => 21,
        _ => return None,
    };
    Some(semitone + offset)
}

/// A root ("C#", "Bb") as a pitch class, as mir_eval's `pitch_class_to_semitone`.
fn root(text: &str) -> Option<i32> {
    let mut chars = text.chars();
    let mut semitone: i32 = match chars.next()? {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    for c in chars {
        semitone += match c {
            '#' => 1,
            'b' => -1,
            _ => return None,
        };
    }
    Some(semitone.rem_euclid(12))
}

/// Reads a label as mir_eval's `encode` does (`reduce` as its `reduce_extended_chords`), or `None` when mir_eval
/// would call it invalid.
fn encode(text: &str, reduce: bool) -> Option<Label> {
    let text = text.trim();
    match text {
        "N" => return Some(Label::NO_CHORD),
        "X" => return Some(Label::UNKNOWN),
        _ => {}
    }
    let (root, mut name, degrees, bass) = split(text)?;
    let mut degrees: Vec<String> = degrees.into_iter().map(str::to_owned).collect();
    if reduce && let Some((_, base, extra)) = REDUX.iter().find(|(q, _, _)| *q == name) {
        name = (*base).to_owned();
        degrees.extend(extra.iter().map(|d| (*d).to_owned()));
    }
    let mut sum = quality(&name)?.map(i32::from);
    sum[0] = 1;
    for d in &degrees {
        let (sign, d) = match d.strip_prefix('*') {
            Some(rest) => (-1, rest.trim_start_matches('*')),
            None => (1, d.as_str()),
        };
        let semitone = degree(d)?;
        // Without `reduce`, a degree an octave or more up is dropped (mir_eval's `modulo=False`).
        if ((0..12).contains(&semitone) || reduce)
            && let Some(s) = sum.get_mut(semitone.rem_euclid(12) as usize)
        {
            *s += sign;
        }
    }
    let mut semitones = sum.map(|s| i8::from(s > 0));
    let bass = degree(bass)?.rem_euclid(12);
    if let Some(s) = semitones.get_mut(bass as usize) {
        *s = 1;
    }
    Some(Label { root: root as i8, semitones, bass: bass as i8 })
}

/// A label (not "N" or "X") split as mir_eval's `split` and `encode` split it: the root, the quality's name (lower
/// case; "maj" when the label has neither a quality nor degrees, "" when it has only degrees), the degrees in
/// brackets (empty ones left out) and the bass degree ("1" when there's no slash). `None` when mir_eval would call it
/// invalid. [`encode`] and [`whole`] both read labels through it, so they agree on which labels read.
fn split(text: &str) -> Option<(i32, String, Vec<&str>, &str)> {
    let (chord, bass) = match text.split_once('/') {
        Some((c, b)) => (c, b),
        None => (text, "1"),
    };
    let (chord, degrees) = match chord.split_once('(') {
        Some((c, d)) => (c, Some(d.strip_suffix(')')?)),
        None => (chord, None),
    };
    let degrees: Vec<&str> = degrees.map(|d| d.split(',').map(str::trim).collect()).unwrap_or_default();
    let omission = degrees.iter().any(|d| d.contains('*'));
    if omission && !chord.contains(':') {
        return None;
    }
    let mut name = if degrees.is_empty() { "maj".to_owned() } else { String::new() };
    let root_text = match chord.split_once(':') {
        Some((r, q)) => {
            if !q.is_empty() {
                name = q.to_lowercase();
            }
            r
        }
        None => chord,
    };
    let root = root(root_text)?;
    Some((root, name, degrees.into_iter().filter(|d| !d.is_empty()).collect(), bass))
}

/// Reads a label as mir_eval scores it, or `None` when mir_eval would call it invalid.
pub fn parse(text: &str) -> Option<Label> {
    encode(text, false)
}

/// Reads a label as mir_eval does when it joins chords in a row that are the same (ninths and so on kept).
pub fn parse_for_joining(text: &str) -> Option<Label> {
    encode(text, true)
}

/// A label read whole ([`named`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    /// "N": no chord.
    NoChord,
    /// "X", or a label mir_eval can't read: left out of every score, as mir_eval leaves it out.
    Unknown,
    /// The chord the label names: its root, every note its name says, and the bass. The bass is one of the chord's own
    /// notes when the label's notes have it ("C:maj7/7" is Cmaj7/B) and only a bass when they don't ("C:maj/7" is C/B).
    Chord(Chord),
}

/// Each shorthand's notes in semitones above the root, its ninths, elevenths and thirteenths too (mir_eval scores
/// only the notes within an octave): Harte's, as `convert/charts.py` plays them, so a 13 has no 11 (players leave the
/// 11 out of a chord with a 13 in it), and mir_eval's extra "b9", "#9", "#11" and "b13" spelled out the same way.
const WHOLE: [(&str, &[i32]); 29] = [
    ("maj", &[0, 4, 7]),
    ("min", &[0, 3, 7]),
    ("aug", &[0, 4, 8]),
    ("dim", &[0, 3, 6]),
    ("sus4", &[0, 5, 7]),
    ("sus2", &[0, 2, 7]),
    ("7", &[0, 4, 7, 10]),
    ("maj7", &[0, 4, 7, 11]),
    ("min7", &[0, 3, 7, 10]),
    ("minmaj7", &[0, 3, 7, 11]),
    ("maj6", &[0, 4, 7, 9]),
    ("min6", &[0, 3, 7, 9]),
    ("dim7", &[0, 3, 6, 9]),
    ("hdim7", &[0, 3, 6, 10]),
    ("maj9", &[0, 4, 7, 11, 14]),
    ("min9", &[0, 3, 7, 10, 14]),
    ("9", &[0, 4, 7, 10, 14]),
    ("b9", &[0, 4, 7, 10, 13]),
    ("#9", &[0, 4, 7, 10, 15]),
    ("min11", &[0, 3, 7, 10, 14, 17]),
    ("11", &[0, 4, 7, 10, 14, 17]),
    ("#11", &[0, 4, 7, 10, 14, 18]),
    ("maj13", &[0, 4, 7, 11, 14, 21]),
    ("min13", &[0, 3, 7, 10, 14, 21]),
    ("13", &[0, 4, 7, 10, 14, 21]),
    ("b13", &[0, 4, 7, 10, 14, 20]),
    ("1", &[0]),
    ("5", &[0, 7]),
    ("", &[]),
];

/// Reads a label as the chord a chart names, every note of it. Unlike [`parse`], the ninths, elevenths and
/// thirteenths a label says, in its shorthand or in brackets, are kept, so extensions can be scored. Otherwise it reads
/// as mir_eval reads: the root is one of the notes unless "*1" leaves it out, and what mir_eval can't read is
/// [`Named::Unknown`].
pub fn named(text: &str) -> Named {
    match text.trim() {
        "N" => Named::NoChord,
        "X" => Named::Unknown,
        text => whole(text).map_or(Named::Unknown, Named::Chord),
    }
}

fn whole(text: &str) -> Option<Chord> {
    let (root, name, degrees, bass) = split(text)?;
    let root = PitchClass::new(root);
    let (_, shorthand) = WHOLE.iter().find(|(q, _)| *q == name)?;
    // How many times each note above the root is said, as mir_eval counts them: the root always, then the
    // shorthand's notes, and each degree in brackets adds one ("*" takes one away).
    let mut said = [0i32; 12];
    for &s in std::iter::once(&0).chain(shorthand.iter()) {
        if let Some(n) = said.get_mut(s.rem_euclid(12) as usize) {
            *n = 1;
        }
    }
    for d in degrees {
        let (sign, d) = match d.strip_prefix('*') {
            Some(rest) => (-1, rest.trim_start_matches('*')),
            None => (1, d),
        };
        if let Some(n) = said.get_mut(degree(d)?.rem_euclid(12) as usize) {
            *n += sign;
        }
    }
    let intervals = (0u16..12).filter(|&i| said.get(usize::from(i)).is_some_and(|&n| n > 0)).fold(0, |b, i| b | 1 << i);
    let bass = root.up(degree(bass)?);
    Some(Chord::new(root, PcSet::from_bits(intervals).on(root), bass))
}

/// The ways mir_eval compares a chord read with the labelled one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    /// The same root.
    Root,
    /// Root and third.
    Thirds,
    /// Root and the notes up to the fifth (a triad).
    Triads,
    /// Root and every interval (a four-note chord, tensions left out).
    Tetrads,
    /// Root and major or minor triad, scored only where the label is major, minor or no chord.
    MajMin,
    /// As `MajMin`, with the bass too.
    MajMinInv,
    /// Root and major, minor, major 7, minor 7 or dominant 7, scored only where the label is one of those or N.
    Sevenths,
    /// As `Sevenths`, with the bass too.
    SeventhsInv,
    /// At least three pitch classes in common (the MIREX rule).
    Mirex,
    /// The same bass note (the C# bench's "bass"), scored where the label has a chord.
    Bass,
}

impl Measure {
    pub const ALL: [Measure; 10] = [
        Measure::Root,
        Measure::MajMin,
        Measure::MajMinInv,
        Measure::Thirds,
        Measure::Triads,
        Measure::Sevenths,
        Measure::SeventhsInv,
        Measure::Tetrads,
        Measure::Mirex,
        Measure::Bass,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Measure::Root => "root",
            Measure::Thirds => "thirds",
            Measure::Triads => "triads",
            Measure::Tetrads => "tetrads",
            Measure::MajMin => "majmin",
            Measure::MajMinInv => "majmin+bass",
            Measure::Sevenths => "sevenths",
            Measure::SeventhsInv => "7ths+bass",
            Measure::Mirex => "mirex",
            Measure::Bass => "bass",
        }
    }

    /// Whether the chord read matches the labelled one by this measure; `None` where the measure leaves the label out.
    pub fn compare(self, reference: &Label, estimate: &Label) -> Option<bool> {
        if reference.is_unknown() {
            return None;
        }
        let r = reference;
        let e = estimate;
        let same_root = r.root == e.root;
        let same_bass = r.bass == e.bass;
        let first8 = r.semitones.get(..8) == e.semitones.get(..8);
        let all = r.semitones == e.semitones;
        let is = |q: &str| quality(q).is_some_and(|s| s == r.semitones);
        let is8 = |q: &str| quality(q).is_some_and(|s| s.get(..8) == r.semitones.get(..8));
        // The labelled bass is one of the label's own notes (a bass outside the quality doesn't count as an inversion).
        let bass_in_quality = r.bass < 0 || r.semitones.get(r.bass as usize).is_some_and(|&s| s > 0);
        let none = r.semitones.iter().all(|&s| s == 0);
        Some(match self {
            Measure::Root => same_root,
            Measure::Thirds => same_root && r.semitones.get(3) == e.semitones.get(3),
            Measure::Triads => same_root && first8,
            Measure::Tetrads => same_root && all,
            Measure::MajMin | Measure::MajMinInv => {
                if !(is8("maj") || is8("min") || none) {
                    return None;
                }
                if self == Measure::MajMinInv && !bass_in_quality {
                    return None;
                }
                same_root && first8 && (self == Measure::MajMin || same_bass)
            }
            Measure::Sevenths | Measure::SeventhsInv => {
                if !["maj", "min", "maj7", "7", "min7", ""].iter().any(|q| is(q)) {
                    return None;
                }
                if self == Measure::SeventhsInv && !bass_in_quality {
                    return None;
                }
                same_root && all && (self == Measure::Sevenths || same_bass)
            }
            Measure::Mirex => {
                let count = r.semitones.iter().filter(|&&s| s > 0).count();
                if count > 0 && count < 3 {
                    return None;
                }
                if r.root < 0 && e.root < 0 {
                    true
                } else {
                    let common = r.pitch_classes().intersection(e.pitch_classes()).len();
                    // An unknown chord read ("X") shares nothing.
                    !e.is_unknown() && common >= 3
                }
            }
            Measure::Bass => {
                if r.root < 0 {
                    return None;
                }
                e.root >= 0 && (r.root + r.bass) % 12 == (e.root + e.bass) % 12
            }
        })
    }
}

/// A run of time with one chord: start, end (any unit) and the chord.
pub type Span = (i64, i64, Label);

/// The chords read, cut to the labelled stretch `from`..`to` and padded with `none` where they don't reach it, as
/// mir_eval's `adjust_intervals` does.
pub fn adjust<K: Copy>(from: i64, to: i64, estimate: &[(i64, i64, K)], none: K) -> Vec<(i64, i64, K)> {
    let mut est: Vec<(i64, i64, K)> = estimate
        .iter()
        .filter(|s| s.1 > from && s.0 < to)
        .map(|&(s, e, l)| (s.max(from), e.min(to), l))
        .filter(|s| s.1 > s.0)
        .collect();
    match est.first() {
        Some(f) if f.0 > from => est.insert(0, (from, f.0, none)),
        None if to > from => est.push((from, to, none)),
        _ => {}
    }
    if let Some(l) = est.last().copied()
        && l.1 < to
    {
        est.push((l.1, to, none));
    }
    est
}

/// The labelled stretch: from the earliest start to the latest end, as mir_eval's `evaluate` takes it
/// (`ref_intervals.min()` and `.max()`), so labels out of order or overlapping give the same stretch. `None` when
/// nothing is labelled.
pub fn stretch<K>(spans: &[(i64, i64, K)]) -> Option<(i64, i64)> {
    let from = spans.iter().map(|s| s.0.min(s.1)).min()?;
    let to = spans.iter().map(|s| s.0.max(s.1)).max()?;
    Some((from, to))
}

/// [`line_up_by`] for labels and chords read as mir_eval encodes them: each piece's length, label and chord read.
#[cfg(test)]
pub fn line_up(reference: &[Span], estimate: &[Span]) -> Vec<(i64, Label, Label)> {
    line_up_by(reference, estimate, Label::NO_CHORD)
        .into_iter()
        .filter_map(|(length, r, e)| Some((length, reference.get(r)?.2, e)))
        .collect()
}

/// The chords read laid against the labelled ones over the labelled stretch, as mir_eval's `evaluate` does: the
/// chords read are cut to the stretch and padded with `none` ([`adjust`]); then the stretch is cut wherever either
/// changes, and in a gap between two chords read the one before goes on. Each piece: its length, which label it lies
/// in (its place in `reference`), and the chord read there.
pub fn line_up_by<R, E: Copy>(
    reference: &[(i64, i64, R)],
    estimate: &[(i64, i64, E)],
    none: E,
) -> Vec<(i64, usize, E)> {
    let Some((from, to)) = stretch(reference) else {
        return Vec::new();
    };
    let est = adjust(from, to, estimate, none);
    let mut cuts: Vec<i64> =
        reference.iter().map(|s| (s.0, s.1)).chain(est.iter().map(|s| (s.0, s.1))).flat_map(|(a, b)| [a, b]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    // The span starting last at or before a time (mir_eval's searchsorted on the starts).
    cuts.windows(2)
        .filter_map(|w| {
            let (&a, &b) = (w.first()?, w.get(1)?);
            let label = reference.iter().rposition(|s| s.0 <= a)?;
            let read = est.iter().rev().find(|s| s.0 <= a)?.2;
            Some((b - a, label, read))
        })
        .collect()
}

/// mir_eval's `merge_chord_intervals`: chords in a row that read the same are joined into one span. `K` says what
/// "the same" is: a label read with [`parse_for_joining`], or a chord read with all its notes.
pub fn join<K: PartialEq + Copy>(spans: &[(i64, i64, K)]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64, K)> = Vec::new();
    for &(s, e, l) in spans {
        match out.last_mut() {
            Some(last) if last.2 == l => last.1 = e,
            _ => out.push((s, e, l)),
        }
    }
    out.into_iter().map(|(s, e, _)| (s, e)).collect()
}

#[cfg(test)]
/// mir_eval's `directional_hamming_distance`: for each reference span, the part not covered by the largest piece the
/// estimate's boundaries cut it into, summed, over the reference's stretch. 0 when every reference span is whole.
pub fn directional_hamming(reference: &[(i64, i64)], estimate: &[(i64, i64)]) -> f64 {
    let (lost, total) = hamming_parts(reference, estimate);
    if total <= 0 { 0.0 } else { lost as f64 / total as f64 }
}

/// The two numbers [`directional_hamming`] divides: the length lost, and the reference's stretch, from its first
/// start to its last end, gaps and all (mir_eval's `reference_intervals[-1, 1] - reference_intervals[0, 0]`).
pub fn hamming_parts(reference: &[(i64, i64)], estimate: &[(i64, i64)]) -> (i64, i64) {
    let mut cuts: Vec<i64> = estimate.iter().flat_map(|&(s, e)| [s, e]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let total = match (reference.first(), reference.last()) {
        (Some(first), Some(last)) => last.1 - first.0,
        _ => 0,
    };
    let mut lost = 0;
    for &(start, end) in reference {
        let mut points = vec![start];
        points.extend(cuts.iter().copied().filter(|&t| t >= start && t < end));
        points.push(end);
        let longest = points.windows(2).filter_map(|w| Some(w.get(1)? - w.first()?)).max().unwrap_or(0);
        lost += end - start - longest;
    }
    (lost, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(text: &str) -> Label {
        parse(text).unwrap()
    }

    fn bits(l: &Label) -> String {
        l.semitones.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn labels_encode_as_mir_eval_encodes_them() {
        // The examples from mir_eval's documentation and tests.
        let c = label("C");
        assert_eq!((c.root, bits(&c), c.bass), (0, "100010010000".into(), 0));
        assert_eq!(label("C:maj"), c);
        let a = label("A:min7/b3");
        assert_eq!((a.root, bits(&a), a.bass), (9, "100100010010".into(), 3));
        // Ninths and the like aren't among the flags.
        assert_eq!(bits(&label("G:9")), bits(&label("G:7")));
        assert_eq!(bits(&label("C:maj(9)")), bits(&label("C:maj")));
        assert_eq!(bits(&label("C:(1,b3,5)")), bits(&label("C:min")));
        assert_eq!(bits(&label("C:maj(*5)")), "100010000000");
        assert_eq!(bits(&label("C:sus4(b7)")), "100001010010");
        // The bass joins the flags.
        assert_eq!(bits(&label("C:maj/b7")), "100010010010");
        assert_eq!(label("Db:maj").root, 1);
        assert_eq!(label("Cb:maj").root, 11);
        assert_eq!(label("N"), Label::NO_CHORD);
        assert!(label("X").is_unknown());
        for bad in ["H:maj", "C:foo", "C:maj(15)", "C(*5)", ""] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        // Joining keeps the ninth: C:9 then C:7 are two chords.
        assert_ne!(parse_for_joining("C:9"), parse_for_joining("C:7"));
    }

    #[test]
    fn chords_read_are_labelled_as_charts_write_them() {
        let chord = |t: &str| Label::of_chord(Chord::parse(t).unwrap());
        assert_eq!(chord("Cmaj9"), label("C:maj7"));
        assert_eq!(chord("G13"), label("G:7"));
        assert_eq!(chord("Dm7/C"), label("D:min7/b7"));
        assert_eq!(chord("C6"), label("C:maj6"));
        assert_eq!(chord("Bm7b5"), label("B:hdim7"));
        assert_eq!(chord("Csus4"), label("C:sus4"));
        assert_eq!(chord("C7sus4"), label("C:sus4(b7)"));
        assert_eq!(chord("Cadd9"), label("C:maj"));
        assert_eq!(chord("C#dim7"), label("C#:dim7"));
        assert_eq!(chord("D/F#"), label("D:maj/3"));
    }

    #[test]
    fn comparisons_follow_mir_eval() {
        let cmp = |m: Measure, r: &str, e: &str| m.compare(&label(r), &label(e));
        assert_eq!(cmp(Measure::Root, "C:maj", "C:min7"), Some(true));
        assert_eq!(cmp(Measure::Root, "N", "N"), Some(true));
        assert_eq!(cmp(Measure::Root, "X", "C"), None);
        assert_eq!(cmp(Measure::MajMin, "C:maj7", "C:maj"), Some(true));
        assert_eq!(cmp(Measure::MajMin, "C:dim", "C:dim"), None);
        assert_eq!(cmp(Measure::MajMin, "N", "N"), Some(true));
        assert_eq!(cmp(Measure::MajMin, "N", "C"), Some(false));
        assert_eq!(cmp(Measure::MajMinInv, "C:maj/3", "C:maj/3"), Some(true));
        assert_eq!(cmp(Measure::MajMinInv, "C:maj/3", "C:maj"), Some(false));
        assert_eq!(cmp(Measure::MajMinInv, "C:maj/2", "C:maj/2"), None);
        assert_eq!(cmp(Measure::Sevenths, "C:7", "C:7"), Some(true));
        assert_eq!(cmp(Measure::Sevenths, "C:7", "C:maj"), Some(false));
        assert_eq!(cmp(Measure::Sevenths, "C:sus4", "C:sus4"), None);
        assert_eq!(cmp(Measure::Thirds, "C:maj7", "C:7"), Some(true));
        assert_eq!(cmp(Measure::Triads, "C:maj7", "C:aug"), Some(false));
        assert_eq!(cmp(Measure::Tetrads, "C:9", "C:7"), Some(true));
        assert_eq!(cmp(Measure::Mirex, "C:maj7", "E:min"), Some(true));
        assert_eq!(cmp(Measure::Mirex, "C:maj", "A:min"), Some(false));
        assert_eq!(cmp(Measure::Mirex, "C:5", "C:5"), None);
        assert_eq!(cmp(Measure::Mirex, "N", "N"), Some(true));
        assert_eq!(cmp(Measure::Bass, "C:maj/5", "G:maj"), Some(true));
        assert_eq!(cmp(Measure::Bass, "N", "G:maj"), None);
    }

    #[test]
    fn chords_read_are_laid_against_the_labels_as_mir_eval_lays_them() {
        let c = label("C");
        let g = label("G");
        let n = Label::NO_CHORD;
        // Labelled C from 0 to 10, G from 10 to 20; read G from 5 to 8, nothing till 12, C from 12 to 25.
        let lined = line_up(&[(0, 10, c), (10, 20, g)], &[(5, 8, g), (12, 25, c)]);
        // Padded with N to 5; in the gap from 8 to 12 the G read before goes on.
        assert_eq!(lined, vec![(5, c, n), (3, c, g), (2, c, g), (2, g, g), (8, g, c)]);
        // Over-cutting: the read boundary at 5 cuts the first label in two (5 lost of 20).
        assert_eq!(directional_hamming(&[(0, 10), (10, 20)], &[(0, 5), (5, 20)]), 0.25);
        assert_eq!(directional_hamming(&[(0, 20)], &[(0, 20)]), 0.0);
        assert_eq!(join(&[(0, 4, c), (4, 8, c), (8, 9, g)]), vec![(0, 8), (8, 9)]);
        // A gap counts in the length divided by, as mir_eval counts it: 5 lost of the 40 from 0 to 40, not of 30.
        assert_eq!(hamming_parts(&[(0, 10), (10, 20), (30, 40)], &[(0, 5), (5, 40)]), (5, 40));
        // Labels out of order give the same stretch as in order.
        assert_eq!(stretch(&[(10, 20, c), (0, 10, g)]), Some((0, 20)));
        assert_eq!(
            line_up(&[(10, 20, g), (0, 10, c)], &[(0, 20, c)]).iter().map(|l| l.0).sum::<i64>(),
            line_up(&[(0, 10, c), (10, 20, g)], &[(0, 20, c)]).iter().map(|l| l.0).sum::<i64>()
        );
        assert_eq!(stretch::<Label>(&[]), None);
        // The same, with each piece's label by its place.
        assert_eq!(
            line_up_by(&[(0, 10, ()), (10, 20, ())], &[(5, 8, 'g'), (12, 25, 'c')], '-'),
            vec![(5, 0, '-'), (3, 0, 'g'), (2, 0, 'g'), (2, 1, 'g'), (8, 1, 'c')]
        );
    }

    #[test]
    fn labels_read_whole_keep_their_extensions() {
        let chord = |text: &str| match named(text) {
            Named::Chord(c) => c,
            other => panic!("{text}: {other:?}"),
        };
        let notes = |text: &str| {
            let c = chord(text);
            (c.root().number(), c.notes().above(c.root()).bits(), c.bass().above(c.root()))
        };
        let bits = |intervals: &[i32]| PcSet::of(intervals).bits();
        assert_eq!(notes("G:13"), (7, bits(&[0, 2, 4, 7, 9, 10]), 0));
        assert_eq!(notes("C:11"), (0, bits(&[0, 2, 4, 5, 7, 10]), 0));
        assert_eq!(notes("C:min7(11)"), (0, bits(&[0, 3, 5, 7, 10]), 0));
        assert_eq!(notes("A:sus4(b7,9)"), (9, bits(&[0, 2, 5, 7, 10]), 0));
        assert_eq!(notes("Eb:maj6(9)"), (3, bits(&[0, 2, 4, 7, 9]), 0));
        assert_eq!(notes("C:7(*5,#9)"), (0, bits(&[0, 3, 4, 10]), 0));
        assert_eq!(notes("C:(1,3,#5,b7)"), (0, bits(&[0, 4, 8, 10]), 0));
        assert_eq!(notes("C:(3,5)"), (0, bits(&[0, 4, 7]), 0));
        assert_eq!(notes("C:maj(*1)"), (0, bits(&[4, 7]), 0));
        assert_eq!(notes("C"), (0, bits(&[0, 4, 7]), 0));
        // The bass is one of the chord's notes when the label's notes have it, else only a bass.
        assert_eq!(notes("C:maj7/7"), (0, bits(&[0, 4, 7, 11]), 11));
        assert_eq!(notes("C:maj/7"), (0, bits(&[0, 4, 7]), 11));
        assert!(chord("C:maj7/7").notes().contains(chord("C:maj7/7").bass()));
        assert!(!chord("C:maj/7").notes().contains(chord("C:maj/7").bass()));
        assert_eq!(notes("C:maj/9"), (0, bits(&[0, 4, 7]), 2));
        assert_eq!(chord("G:13").name(), "G13");
        assert_eq!(chord("C:maj7/7").name(), "Cmaj7/B");
        assert_eq!(chord("C:maj/7").name(), "C/B");
        assert_eq!(chord("A:min7/b3").name(), "Am7/C");
        assert_eq!(named("N"), Named::NoChord);
        for bad in ["X", "H:maj", "C:foo", "C:maj(15)", "C(*5)", ""] {
            assert_eq!(named(bad), Named::Unknown, "{bad}");
        }
        // The whole shorthands are mir_eval's within the octave, and its own extensions above it (a 13 without its 11).
        for (q, notes) in WHOLE {
            let within: Vec<i32> = notes.iter().copied().filter(|&n| n < 12).collect();
            let flags = quality(q).unwrap().map(i32::from);
            let expected: Vec<i32> = (0..12).filter(|&i| flags[i as usize] > 0).collect();
            assert_eq!(within, expected, "{q}");
            let above: Vec<i32> = notes.iter().copied().filter(|&n| n >= 12).collect();
            let mut redux: Vec<i32> = REDUX
                .iter()
                .find(|(r, _, _)| r == &q)
                .map(|(_, _, extra)| extra.iter().map(|d| degree(d).unwrap()).filter(|&n| n >= 12).collect())
                .unwrap_or_default();
            if q.contains("13") {
                redux.retain(|&n| n != 17);
            }
            assert_eq!(above, redux, "{q}");
        }
        // Every label mir_eval reads, this reads too, with the same root and the same notes within the octave.
        for text in
            ["C:maj9", "D:min9/b7", "F#:hdim7", "Bb:7(b9,#11)", "E:maj13", "A:sus2", "G:5", "C:1(11)", "Db:aug(b7)"]
        {
            let (l, c) = (label(text), chord(text));
            assert_eq!(i32::from(l.root), i32::from(c.root().number()), "{text}");
            assert_eq!(l.pitch_classes().difference(c.sounding()), PcSet::EMPTY, "{text}");
        }
    }
}
