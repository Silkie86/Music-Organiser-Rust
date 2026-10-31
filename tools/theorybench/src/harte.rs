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
    let mut degrees: Vec<String> = degrees.into_iter().filter(|d| !d.is_empty()).map(str::to_owned).collect();
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

/// Reads a label as mir_eval scores it, or `None` when mir_eval would call it invalid.
pub fn parse(text: &str) -> Option<Label> {
    encode(text, false)
}

/// Reads a label as mir_eval does when it joins chords in a row that are the same (ninths and so on kept).
pub fn parse_for_joining(text: &str) -> Option<Label> {
    encode(text, true)
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

/// The chords read laid against the labelled ones over the labelled stretch, as mir_eval's `evaluate` does: the
/// chords read are cut to the stretch and padded with no chord ([`adjust`]); then the stretch is cut wherever either
/// changes, and in a gap between two chords read the one before goes on. Each piece: its length, the label, the
/// chord read.
pub fn line_up(reference: &[Span], estimate: &[Span]) -> Vec<(i64, Label, Label)> {
    let (Some(first), Some(last)) = (reference.first(), reference.last()) else {
        return Vec::new();
    };
    let est = adjust(first.0, last.1, estimate, Label::NO_CHORD);
    let mut cuts: Vec<i64> = reference.iter().chain(&est).flat_map(|s| [s.0, s.1]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    // The span starting last at or before a time (mir_eval's searchsorted on the starts).
    let at = |spans: &[Span], t: i64| spans.iter().rev().find(|s| s.0 <= t).map(|s| s.2);
    cuts.windows(2)
        .filter_map(|w| {
            let (&a, &b) = (w.first()?, w.get(1)?);
            Some((b - a, at(reference, a)?, at(&est, a)?))
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
/// estimate's boundaries cut it into, summed, over the total length. 0 when every reference span is whole.
pub fn directional_hamming(reference: &[(i64, i64)], estimate: &[(i64, i64)]) -> f64 {
    let (lost, total) = hamming_parts(reference, estimate);
    if total <= 0 { 0.0 } else { lost as f64 / total as f64 }
}

/// The two numbers [`directional_hamming`] divides: the length lost, and the reference's whole length.
pub fn hamming_parts(reference: &[(i64, i64)], estimate: &[(i64, i64)]) -> (i64, i64) {
    let mut cuts: Vec<i64> = estimate.iter().flat_map(|&(s, e)| [s, e]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let total: i64 = reference.iter().map(|(s, e)| e - s).sum();
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
    }
}
