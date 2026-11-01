//! The chord scores mir_eval doesn't have, which the chord reader's design asks for (harmony/design/
//! chord-reader-design.md, "Bench changes"): the notes and the bass, the whole name, each extension, the root among
//! the same notes, the bass kind, the root where no part plays it, and how often the labelled chord is among an
//! engine's first readings.
//!
//! Every score is over the time the label names a chord: no chord and unknown labels are left to mir_eval's scores. A
//! label is read whole ([`harte::named`]), so its ninths, elevenths and thirteenths count, and every note of a chord,
//! label and reading alike, gets its degree from mo-harmony's [`chord::degree`], so "the 9 of C" means the same on
//! both sides.

use std::collections::HashMap;

use mo_harmony::chord;
use mo_harmony::{Chord, Degree, Family, PcSet};

use crate::harte::{self, Named};
use crate::metrics::{self, Share, pct};

/// The scores of the whole chord, in the order [`Tally::right`] keeps them.
/// - **notes+bass**: every note the label names and the bass, on any root (the design's "exact notes+bass").
/// - **name**: the same on the right root, with the bass of the same kind ([`KINDS`]): the whole chord as written, so
///   D7/C isn't D/C (the C only a bass) though they sound the same.
/// - **event**: the root, the core ([`core`]), the bass and its kind, tensions left out: what the reader's chances are
///   about.
/// - **lenient**: the event, where a plain major label also takes a maj7 or 6 reading, a minor one an m7 or m6, and a
///   sus4 a 7sus4 (POP909-CL labels by triad templates while the piano plays more).
pub const SCORES: [&str; 4] = ["notes+bass", "name", "event", "lenient"];

/// The extensions scored one at a time: the 6 of a chord with no seventh, and the tensions; and whether each is an
/// altered tone (b9, #9, #11, b13), which are also scored pooled.
pub const EXTENSIONS: [(&str, Degree, bool); 8] = [
    ("6", Degree::SIXTH, false),
    ("b9", Degree::FLAT_NINTH, true),
    ("9", Degree::NINTH, false),
    ("#9", Degree::SHARP_NINTH, true),
    ("11", Degree::ELEVENTH, false),
    ("#11", Degree::SHARP_ELEVENTH, true),
    ("b13", Degree::FLAT_THIRTEENTH, true),
    ("13", Degree::THIRTEENTH, false),
];

/// How the bass stands to a chord: its root, another of its notes (an inversion, C/E), or only a bass (a slash
/// chord, D/C).
pub const KINDS: [&str; 3] = ["root", "inversion", "slash"];

/// A chord read over a stretch of ticks, with the other readings the engine offers there, likeliest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub start: i64,
    pub end: i64,
    pub chord: Chord,
    /// The engine's own name for it, for `--dump`.
    pub name: String,
    /// The notes of `chord` its name stands for without saying they sound ([`may_leave_out`]), for a reading scored
    /// by the name it shows (the C# app's are): they count only where the label has them. Empty for a reading scored
    /// by its notes.
    pub optional: PcSet,
    pub others: Vec<Chord>,
}

/// A chord's extensions, as bits in the order of [`EXTENSIONS`].
pub fn extensions(c: Chord) -> u8 {
    let (intervals, family) = (c.intervals(), c.family());
    (1..12)
        .filter(|&i| intervals.has(i))
        .filter_map(|i| EXTENSIONS.iter().position(|(_, d, _)| *d == chord::degree(intervals, family, i)))
        .fold(0, |bits, k| bits | 1 << k)
}

/// Every note a chord's name says: its own notes, the bass and the root (a name says its root even when a rootless
/// voicing leaves it out).
pub fn named_notes(c: Chord) -> PcSet {
    c.sounding().with(c.root())
}

/// The notes a chord's name stands for without saying they sound, as charts write names and mo-harmony reads them
/// (its `every_note_is_in_the_name` test): the plain fifth of a chord with a third or a seventh, the 9 under an 11 or a
/// 13, and the 11 of a minor 13. So "Cm13" names a minor 13 with no 11 and "Cm11" a minor 7 with an 11 and no 9, as
/// players read them. A 7alt says it has alterations but not which, so they are no such notes: it is right as the
/// event, not as the name.
pub fn may_leave_out(c: Chord) -> PcSet {
    let degrees = c.degrees();
    let has = |d: Degree| degrees.contains(&d);
    let third = has(Degree::MAJOR_THIRD) || has(Degree::MINOR_THIRD);
    let seventh = has(Degree::MINOR_SEVENTH) || has(Degree::MAJOR_SEVENTH);
    let mut out = PcSet::EMPTY;
    if has(Degree::FIFTH) && (third || seventh) {
        out = out.union(PcSet::of(&[7]));
    }
    if has(Degree::NINTH) && (has(Degree::ELEVENTH) || has(Degree::THIRTEENTH)) {
        out = out.union(PcSet::of(&[2]));
    }
    if c.family() == Family::Minor && has(Degree::ELEVENTH) && has(Degree::THIRTEENTH) {
        out = out.union(PcSet::of(&[5]));
    }
    out.on(c.root())
}

/// The reading as it stands against a label: the notes its name stands for without saying they sound count only
/// where the label has them (or where one is the bass).
fn against(r: &Read, label: Chord) -> Chord {
    let drop = r.optional.difference(named_notes(label)).without(r.chord.bass());
    if drop.is_empty() { r.chord } else { Chord::new(r.chord.root(), r.chord.notes().difference(drop), r.chord.bass()) }
}

/// [`KINDS`]: which one a chord's bass is.
pub fn kind(c: Chord) -> usize {
    if c.bass() == c.root() {
        0
    } else if c.notes().contains(c.bass()) {
        1
    } else {
        2
    }
}

/// What kind of chord it is with its tensions left out: its third (or the 4th or 2nd standing in for it), its
/// seventh (or the 6 of a chord with no seventh) and an altered fifth. A missing fifth doesn't change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Core {
    pub third: Option<Degree>,
    pub seventh: Option<Degree>,
    pub fifth: Option<Degree>,
}

pub fn core(c: Chord) -> Core {
    let (intervals, family) = (c.intervals(), c.family());
    let mut core = Core { third: None, seventh: None, fifth: None };
    for i in (1..12).filter(|&i| intervals.has(i)) {
        let d = chord::degree(intervals, family, i);
        let slot = match d {
            Degree::MINOR_THIRD | Degree::MAJOR_THIRD | Degree::FOURTH | Degree::SECOND => &mut core.third,
            Degree::SIXTH | Degree::DIMINISHED_SEVENTH | Degree::MINOR_SEVENTH | Degree::MAJOR_SEVENTH => {
                &mut core.seventh
            }
            Degree::FLAT_FIFTH | Degree::SHARP_FIFTH => &mut core.fifth,
            _ => continue,
        };
        // The lowest, should a chord have two (a b7 beside a maj7).
        slot.get_or_insert(d);
    }
    core
}

/// Whether `read` is the labelled chord as an event: the same root, bass and bass kind, and the same core, or with
/// `lenient` the core a plain triad's label takes ([`SCORES`]).
pub fn is_event(label: Chord, read: Chord, lenient: bool) -> bool {
    if read.root() != label.root() || read.bass() != label.bass() || kind(read) != kind(label) {
        return false;
    }
    let (l, r) = (core(label), core(read));
    l == r
        || lenient
            && l.seventh.is_none()
            && l.fifth.is_none()
            && r.fifth.is_none()
            && l.third == r.third
            && match l.third {
                Some(Degree::MAJOR_THIRD) => matches!(r.seventh, Some(Degree::MAJOR_SEVENTH | Degree::SIXTH)),
                Some(Degree::MINOR_THIRD) => matches!(r.seventh, Some(Degree::MINOR_SEVENTH | Degree::SIXTH)),
                Some(Degree::FOURTH) => r.seventh == Some(Degree::MINOR_SEVENTH),
                _ => false,
            }
}

/// What one engine got right on one piece: time in beats, over the time the labels name a chord.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    /// The time the labels name a chord.
    pub time: f64,
    /// Of it, the time each of [`SCORES`] was right.
    pub right: [f64; 4],
    /// The labelled chords (each counted once) and, for each of [`SCORES`], how many were right for more than half
    /// their time.
    pub chords: f64,
    pub chords_right: [f64; 4],
    /// For each of [`EXTENSIONS`]: the time both have it on the same root, the time only the reading has it there,
    /// and the time only the label has it there.
    pub extensions: [[f64; 3]; 8],
    /// The time the notes and the bass were right, and of it the time the root was right too.
    pub same_notes: [f64; 2],
    /// Where the notes and bass were right and the root wasn't: the label's quality, the reading's, and how many
    /// semitones the reading's root is above the label's, with the time.
    pub other_roots: HashMap<(String, String, u8), f64>,
    /// The time by the label's bass kind ([`KINDS`]) and the reading's (or no chord, last).
    pub kinds: [[f64; 4]; 3],
    /// The time the label's root sounds in none of the parts heard, with no bass part and with one; and of each, the
    /// time the root was read right.
    pub rootless: [[f64; 2]; 2],
    /// By the event, strict and lenient: the time the labelled chord is the engine's first reading, among its first
    /// two, among its first three, and the time weighted by one over its place (for the mean reciprocal rank).
    pub listed: [[f64; 4]; 2],
}

/// Scores one engine's chords on one piece. `labels` are the labelled chords, read whole; `played`, for each label,
/// the pitch classes sounding in its time in the parts the engine heard; `bass_part`, whether those parts have one.
/// Times are counted in beats (ticks over `ppq`), so pieces with other ticks a beat count by their length.
pub fn tally(labels: &[(i64, i64, Named)], read: &[Read], played: &[PcSet], bass_part: bool, ppq: u32) -> Tally {
    let mut t = Tally::default();
    let reads: Vec<(i64, i64, Option<usize>)> =
        read.iter().enumerate().map(|(i, r)| (r.start, r.end, Some(i))).collect();
    // For each label: its time, and the time each of SCORES was right in it.
    let mut per_label = vec![[0.0f64; 5]; labels.len()];
    for (length, l, r) in harte::line_up_by(labels, &reads, None) {
        let Some(&(_, _, Named::Chord(label))) = labels.get(l) else {
            continue;
        };
        let length = length as f64 / f64::from(ppq.max(1));
        let reading = r.and_then(|i| read.get(i));
        let chord = reading.map(|r| against(r, label));
        t.time += length;
        let same_root = chord.is_some_and(|c| c.root() == label.root());
        let same_notes = chord.is_some_and(|c| named_notes(c) == named_notes(label) && c.bass() == label.bass());
        let rights = [
            same_notes,
            same_notes && same_root && chord.is_some_and(|c| kind(c) == kind(label)),
            chord.is_some_and(|c| is_event(label, c, false)),
            chord.is_some_and(|c| is_event(label, c, true)),
        ];
        if let Some(p) = per_label.get_mut(l) {
            p[0] += length;
            for (k, _) in rights.iter().enumerate().filter(|(_, r)| **r) {
                t.right[k] += length;
                p[k + 1] += length;
            }
        }

        let (has, read_has) = (extensions(label), chord.map_or(0, extensions));
        for (k, cell) in t.extensions.iter_mut().enumerate() {
            let (a, b) = (has >> k & 1 == 1, read_has >> k & 1 == 1);
            if a && b && same_root {
                cell[0] += length;
            } else {
                if b {
                    cell[1] += length;
                }
                if a {
                    cell[2] += length;
                }
            }
        }

        if same_notes {
            t.same_notes[0] += length;
            if same_root {
                t.same_notes[1] += length;
            } else if let Some(c) = chord {
                *t.other_roots.entry((label.quality(), c.quality(), c.root().above(label.root()))).or_default() +=
                    length;
            }
        }
        if let Some(row) = t.kinds.get_mut(kind(label)) {
            row[chord.map_or(3, kind)] += length;
        }
        if played.get(l).is_some_and(|p| !p.contains(label.root())) {
            let cell = &mut t.rootless[usize::from(bass_part)];
            cell[0] += length;
            if same_root {
                cell[1] += length;
            }
        }
        if let Some(r) = reading {
            for (lenient, row) in t.listed.iter_mut().enumerate() {
                let place = std::iter::once(&r.chord).chain(&r.others).position(|&c| is_event(label, c, lenient == 1));
                if let Some(place) = place {
                    for cell in row.iter_mut().take(3).skip(place) {
                        *cell += length;
                    }
                    row[3] += length / (place + 1) as f64;
                }
            }
        }
    }
    for p in per_label.iter().filter(|p| p[0] > 0.0) {
        t.chords += 1.0;
        for (k, right) in t.chords_right.iter_mut().enumerate() {
            if 2.0 * p[k + 1] > p[0] {
                *right += 1.0;
            }
        }
    }
    t
}

/// One engine's tallies for every piece, and whether it offers other readings at all (else its lists are its one
/// reading).
pub struct Scored<'a> {
    pub name: &'a str,
    pub tallies: Vec<Tally>,
    pub lists: bool,
}

/// Prints the scores of every engine, each with its likely range when the songs are drawn again, and each engine's
/// difference from the first.
pub fn report(engines: &[Scored], song: &[usize]) {
    let Some(first) = engines.first() else {
        return;
    };
    let column = |s: &Scored, f: &dyn Fn(&Tally) -> f64| s.tallies.iter().map(f).collect::<Vec<f64>>();
    let time = column(first, &|t| t.time);
    let chords = column(first, &|t| t.chords);
    let chord_time = time.iter().sum::<f64>();
    if chord_time <= 0.0 {
        println!();
        println!("No labelled chords to score the chords named whole on.");
        return;
    }

    println!();
    println!(
        "The chords named whole, over the time the labels name a chord (likely ranges when the songs are drawn again; \
         per chord: each labelled chord counts once, right when right for more than half its time):"
    );
    print!("{:<28}", "");
    for s in SCORES {
        print!(" {:>12}", s);
    }
    for s in SCORES {
        print!(" {:>12}", format!("{s}/chord"));
    }
    println!();
    for e in engines {
        let shares: Vec<Share> = (0..4)
            .map(|k| metrics::ratio(&column(e, &|t| t.right[k]), &time, song))
            .chain((0..4).map(|k| metrics::ratio(&column(e, &|t| t.chords_right[k]), &chords, song)))
            .collect();
        print_shares(e.name, &shares);
    }
    for e in engines.iter().skip(1) {
        let diffs: Vec<(f64, f64, f64)> = (0..4)
            .map(|k| metrics::paired_ratio(&column(first, &|t| t.right[k]), &column(e, &|t| t.right[k]), &time, song))
            .chain((0..4).map(|k| {
                metrics::paired_ratio(
                    &column(first, &|t| t.chords_right[k]),
                    &column(e, &|t| t.chords_right[k]),
                    &chords,
                    song,
                )
            }))
            .collect();
        print_diffs(&format!("{} - {}", e.name, first.name), &diffs);
    }

    println!();
    let support: Vec<f64> = (0..8)
        .map(|k| first.tallies.iter().map(|t| t.extensions[k][0] + t.extensions[k][2]).sum::<f64>() / chord_time)
        .collect();
    if support.iter().all(|&s| s <= 0.0) {
        println!("Each extension: the labels here name none, so there's none to score.");
    } else {
        print_extensions(engines, song, &support);
    }

    println!();
    println!("The root, where the notes and the bass were read right (and the most common other roots then):");
    for e in engines {
        let notes = column(e, &|t| t.same_notes[0]);
        let s = metrics::ratio(&column(e, &|t| t.same_notes[1]), &notes, song);
        println!(
            "{:<28} {} ({:.1}-{:.1}), over {} of the chord time",
            e.name,
            pct(s.value),
            100.0 * s.low,
            100.0 * s.high,
            pct(notes.iter().sum::<f64>() / chord_time)
        );
        let mut others: HashMap<&(String, String, u8), f64> = HashMap::new();
        for t in &e.tallies {
            for (k, v) in &t.other_roots {
                *others.entry(k).or_default() += v;
            }
        }
        let mut others: Vec<_> = others.into_iter().collect();
        others.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        for ((label, read, up), time) in others.into_iter().take(8) {
            println!(
                "{:<28}   label {:<16} read as {:<16} on a root {:>2} semitones up: {} of the chord time",
                "",
                if label.is_empty() { "major" } else { label.as_str() },
                if read.is_empty() { "major" } else { read.as_str() },
                up,
                pct(time / chord_time)
            );
        }
    }

    println!();
    println!("The bass kind: each row is the time with that kind of label, split by what the engine read:");
    println!("{:<28} {:>11} {:>11} {:>11} {:>11} {:>11}", "", "share", "root", "inversion", "slash", "no chord");
    for e in engines {
        for (row, kind) in KINDS.iter().enumerate() {
            let cells: Vec<f64> = (0..4).map(|c| e.tallies.iter().map(|t| t.kinds[row][c]).sum()).collect();
            let total: f64 = cells.iter().sum();
            print!(
                "{:<28} {:>11}",
                if row == 0 { format!("{} {kind}", e.name) } else { format!("  {kind}") },
                pct(total / chord_time)
            );
            for c in &cells {
                print!(" {:>11}", if total > 0.0 { pct(c / total) } else { "-".to_owned() });
            }
            println!();
        }
        let kinds_right = column(e, &|t| t.kinds.iter().enumerate().map(|(k, row)| row[k]).sum());
        let s = metrics::ratio(&kinds_right, &time, song);
        let missed = metrics::ratio(&column(e, &|t| t.kinds[1][0]), &column(e, &|t| t.kinds[1].iter().sum()), song);
        let false_kind = metrics::ratio(
            &column(e, &|t| t.kinds[0][1] + t.kinds[0][2]),
            &column(e, &|t| t.kinds[0].iter().sum()),
            song,
        );
        println!(
            "{:<28}   kind right {} ({:.1}-{:.1}); inversions read in root position {}; root position read as an \
             inversion or slash {}",
            "",
            pct(s.value),
            100.0 * s.low,
            100.0 * s.high,
            pct(missed.value),
            pct(false_kind.value)
        );
    }

    println!();
    println!("The root, where the label's root sounds in none of the parts heard:");
    for e in engines {
        let part = |b: usize| {
            let time = column(e, &|t| t.rootless[b][0]);
            let s = metrics::ratio(&column(e, &|t| t.rootless[b][1]), &time, song);
            let share = time.iter().sum::<f64>() / chord_time;
            if share > 0.0 {
                format!("{} of the time, root right {}", pct(share), pct(s.value))
            } else {
                "never".to_owned()
            }
        };
        println!("{:<28} no bass part: {}; with a bass part: {}", e.name, part(0), part(1));
    }

    println!();
    println!("The labelled chord (as an event) among each engine's readings, and the mean reciprocal rank:");
    println!("{:<28} {:>11} {:>11} {:>11} {:>11}", "", "first", "first two", "first three", "MRR");
    for e in engines {
        for (lenient, name) in [(0, e.name.to_owned()), (1, "  lenient".to_owned())] {
            print!("{name:<28}");
            for k in 0..4 {
                let s = metrics::ratio(&column(e, &|t| t.listed[lenient][k]), &time, song);
                if k == 3 {
                    print!(" {:>11.3}", s.value);
                } else if k > 0 && !e.lists {
                    print!(" {:>11}", "(no list)");
                } else {
                    print!(" {:>11}", pct(s.value));
                }
            }
            println!();
        }
    }
}

/// The extensions table: the share of the chord time the labels have each, then each engine's precision, recall and
/// F1 for each, for the altered tones pooled, and its F1 averaged over the extensions the labels have (macro F1) with
/// its range, and each engine's macro F1 against the first's.
fn print_extensions(engines: &[Scored], song: &[usize], support: &[f64]) {
    let Some(first) = engines.first() else {
        return;
    };
    println!(
        "Each extension, wherever it is (the 6 only with no seventh; right means the same degree on the same root): \
         the share of the chord time the labels have it, then each engine's precision, recall and F1 (- where there's \
         nothing to divide by):"
    );
    print!("{:<28}", "");
    for (name, _, _) in EXTENSIONS {
        print!(" {:>9}", name);
    }
    println!(" {:>9} {:>9}", "altered", "macro F1");
    print!("{:<28}", "labels have it");
    for s in support {
        print!(" {:>9}", pct(*s));
    }
    let altered: f64 = support.iter().zip(EXTENSIONS).filter(|(_, (_, _, a))| *a).map(|(s, _)| s).sum();
    println!(" {:>9}", pct(altered));
    let counts = |t: &Tally| -> [f64; 24] {
        let mut a = [0.0; 24];
        for (k, cell) in t.extensions.iter().enumerate() {
            a[3 * k..3 * k + 3].copy_from_slice(cell);
        }
        a
    };
    let cell = |x: Option<f64>| x.map_or("-".to_owned(), pct);
    for e in engines {
        let values: Vec<[f64; 24]> = e.tallies.iter().map(counts).collect();
        let mut sums = [0.0; 24];
        for v in &values {
            for (s, x) in sums.iter_mut().zip(v) {
                *s += x;
            }
        }
        for (row, which) in [("precision", 0), ("recall", 1), ("F1", 2)] {
            print!("{:<28}", if which == 0 { format!("{} {row}", e.name) } else { format!("  {row}") });
            for c in sums.as_chunks::<3>().0 {
                print!(" {:>9}", cell(score_of(c[0], c[1], c[2], which)));
            }
            let pooled = |j: usize| {
                sums.as_chunks::<3>()
                    .0
                    .iter()
                    .zip(EXTENSIONS)
                    .filter(|(_, (_, _, a))| *a)
                    .map(|(c, _)| c[j])
                    .sum::<f64>()
            };
            print!(" {:>9}", cell(score_of(pooled(0), pooled(1), pooled(2), which)));
            if which == 2 {
                // The labels have at least one extension here (else this table isn't printed), so it's never None.
                let m = metrics::statistic(&values, song, |a| macro_f1(a).unwrap_or(0.0));
                print!(" {:>9}", format!("{} ({:.1}-{:.1})", pct(m.value), 100.0 * m.low, 100.0 * m.high));
            }
            println!();
        }
    }
    for e in engines.iter().skip(1) {
        let values: Vec<[f64; 48]> = first
            .tallies
            .iter()
            .zip(&e.tallies)
            .map(|(a, b)| {
                let mut v = [0.0; 48];
                v[..24].copy_from_slice(&counts(a));
                v[24..].copy_from_slice(&counts(b));
                v
            })
            .collect();
        let d = metrics::statistic(&values, song, |v| {
            let (a, b) = v.split_at(24);
            macro_f1(b).unwrap_or(0.0) - macro_f1(a).unwrap_or(0.0)
        });
        println!(
            "{} - {}: macro F1 {:+.1} points ({:+.1}/{:+.1})",
            e.name,
            first.name,
            100.0 * d.value,
            100.0 * d.low,
            100.0 * d.high
        );
    }
}

/// Precision (0), recall (1) or F1 (2) from the time both have it, only the reading has it, only the label has it;
/// `None` when there's nothing to divide by.
fn score_of(both: f64, read_only: f64, label_only: f64, which: usize) -> Option<f64> {
    let (num, den) = match which {
        0 => (both, both + read_only),
        1 => (both, both + label_only),
        _ => (2.0 * both, 2.0 * both + read_only + label_only),
    };
    (den > 0.0).then(|| num / den)
}

/// The F1 averaged over the extensions the labels have, from each one's three times in turn; `None` when the labels
/// have none.
fn macro_f1(a: &[f64]) -> Option<f64> {
    let f1s: Vec<f64> = a
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|c| c[0] + c[2] > 0.0)
        .filter_map(|c| score_of(c[0], c[1], c[2], 2))
        .collect();
    (!f1s.is_empty()).then(|| f1s.iter().sum::<f64>() / f1s.len() as f64)
}

/// Tallies pooled for one row of a split table (`chords --by`): the chord time, each of [`SCORES`]' share of it right,
/// the bass kind's share right and the macro F1 of the extensions the labels have (`None` where there's nothing to
/// divide by).
pub struct Pooled {
    pub time: f64,
    pub scores: [Option<f64>; 4],
    pub kind_right: Option<f64>,
    pub macro_f1: Option<f64>,
}

pub fn pool(tallies: &[Tally]) -> Pooled {
    let time: f64 = tallies.iter().map(|t| t.time).sum();
    let share = |x: f64| (time > 0.0).then(|| x / time);
    let mut right = [0.0; 4];
    let mut extensions = [0.0; 24];
    let mut kinds = 0.0;
    for t in tallies {
        for (sum, x) in right.iter_mut().zip(t.right) {
            *sum += x;
        }
        for (sums, cell) in extensions.as_chunks_mut::<3>().0.iter_mut().zip(&t.extensions) {
            for (sum, x) in sums.iter_mut().zip(cell) {
                *sum += x;
            }
        }
        kinds += t.kinds.iter().enumerate().map(|(k, row)| row[k]).sum::<f64>();
    }
    Pooled { time, scores: right.map(share), kind_right: share(kinds), macro_f1: macro_f1(&extensions) }
}

fn print_shares(name: &str, shares: &[Share]) {
    print!("{name:<28}");
    for s in shares {
        print!(" {:>12}", pct(s.value));
    }
    println!();
    print!("{:<28}", "");
    for s in shares {
        print!(" {:>12}", format!("{:.1}-{:.1}", 100.0 * s.low, 100.0 * s.high));
    }
    println!();
}

fn print_diffs(name: &str, diffs: &[(f64, f64, f64)]) {
    print!("{name:<28}");
    for (d, _, _) in diffs {
        print!(" {:>+12.1}", 100.0 * d);
    }
    println!();
    print!("{:<28}", "");
    for (_, low, high) in diffs {
        print!(" {:>12}", format!("{:+.1}/{:+.1}", 100.0 * low, 100.0 * high));
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).unwrap()
    }

    fn label(text: &str) -> Chord {
        match harte::named(text) {
            Named::Chord(c) => c,
            other => panic!("{text}: {other:?}"),
        }
    }

    fn names(bits: u8) -> Vec<&'static str> {
        EXTENSIONS.iter().enumerate().filter(|(k, _)| bits >> k & 1 == 1).map(|(_, (n, _, _))| *n).collect()
    }

    #[test]
    fn extensions_are_named_by_degree() {
        assert_eq!(names(extensions(chord("G13"))), ["9", "13"]);
        assert_eq!(names(extensions(chord("C6"))), ["6"]);
        assert_eq!(names(extensions(chord("C6/9"))), ["6", "9"]);
        assert_eq!(names(extensions(chord("C7(#9)"))), ["#9"]);
        assert_eq!(names(extensions(chord("Cm7"))), Vec::<&str>::new());
        assert_eq!(names(extensions(chord("Cm11"))), ["9", "11"]);
        assert_eq!(names(extensions(chord("C7(b9,b13)"))), ["b9", "b13"]);
        assert_eq!(names(extensions(chord("Cmaj7(#11)"))), ["#11"]);
        assert_eq!(names(extensions(label("G:13"))), ["9", "13"]);
        assert_eq!(names(extensions(label("A:sus4(b7,9)"))), ["9"]);
        // The b5 of a m7b5 and the #5 of an augmented chord are no extensions.
        assert_eq!(extensions(chord("Cm7b5")), 0);
        assert_eq!(extensions(chord("C+")), 0);
    }

    #[test]
    fn events_compare_root_core_and_bass() {
        assert!(is_event(label("C:maj7"), chord("Cmaj9"), false));
        assert!(!is_event(label("C:maj7"), chord("C7"), false));
        assert!(!is_event(label("C:maj"), chord("Cmaj7"), false));
        assert!(is_event(label("C:maj"), chord("Cmaj7"), true));
        assert!(is_event(label("C:maj"), chord("C6"), true));
        assert!(!is_event(label("C:maj"), chord("C7"), true));
        assert!(is_event(label("A:min"), chord("Am7"), true));
        assert!(is_event(label("C:sus4"), chord("C7sus4"), true));
        assert!(!is_event(label("C:dim"), chord("Cm7b5"), true));
        assert!(!is_event(label("C:maj/3"), chord("C"), true));
        assert!(is_event(label("C:maj/3"), chord("C/E"), false));
        assert!(!is_event(label("C:7"), chord("C7(b5)"), false));
        assert!(is_event(label("C:7"), chord("C7(#11)"), false));
        // A bass that is only a bass isn't one of the chord's notes: D/C isn't D7/C, C/D isn't Cadd9/D.
        assert!(is_event(label("D:maj/b7"), chord("D/C"), false));
        assert!(!is_event(label("D:maj/b7"), chord("D7/C"), true));
        assert!(!is_event(label("C:maj/9"), chord("Cadd9/D"), true));
    }

    #[test]
    fn bass_kinds_tell_inversions_from_slashes() {
        assert_eq!(kind(label("C:maj")), 0);
        assert_eq!(kind(label("C:maj/3")), 1);
        assert_eq!(kind(label("C:maj7/7")), 1);
        assert_eq!(kind(label("C:maj/7")), 2);
        assert_eq!(kind(label("D:maj/b7")), 2);
        assert_eq!(kind(chord("D/C")), 2);
        assert_eq!(kind(chord("D7/C")), 1);
    }

    #[test]
    fn a_piece_is_tallied_over_the_time_the_labels_name_a_chord() {
        let labels = [(0, 10, harte::named("C:maj6")), (10, 20, harte::named("N")), (20, 40, harte::named("G:13"))];
        let read = |s, e, c: &str, others: &[&str]| Read {
            start: s,
            end: e,
            chord: chord(c),
            name: c.to_owned(),
            optional: PcSet::EMPTY,
            others: others.iter().map(|o| chord(o)).collect(),
        };
        // C6 read as Am7/C (the same notes and bass, another root), then G13 read right after a G7 for 5 ticks.
        let reads = [read(0, 10, "Am7/C", &["C6"]), read(10, 25, "G7", &[]), read(25, 40, "G13", &[])];
        let played = [PcSet::of(&[0, 4, 7, 9]), PcSet::EMPTY, PcSet::of(&[5, 9, 11, 4])];
        let t = tally(&labels, &reads, &played, false, 1);
        assert_eq!(t.time, 30.0);
        // notes+bass: the 10 of C6 and the 15 of G13; the name only G13's 15; the event the G7's 5 too (tensions
        // left out).
        assert_eq!(t.right, [25.0, 15.0, 20.0, 20.0]);
        assert_eq!((t.chords, t.chords_right), (2.0, [2.0, 1.0, 1.0, 1.0]));
        // The 6: on C6 the reading is on another root, so it's a miss, not a hit.
        assert_eq!(t.extensions[0], [0.0, 0.0, 10.0]);
        // The 9 and 13 of G13: right for 15, missed for 5 (G7). Am7/C has no 9 or 13 above A.
        assert_eq!(t.extensions[2], [15.0, 0.0, 5.0]);
        assert_eq!(t.extensions[7], [15.0, 0.0, 5.0]);
        assert_eq!(t.same_notes, [25.0, 15.0]);
        assert_eq!(t.other_roots.get(&("6".to_owned(), "m7".to_owned(), 9)), Some(&10.0));
        // The G13's root sounds in no part: its 20 counts, root right all along.
        assert_eq!(t.rootless, [[20.0, 20.0], [0.0, 0.0]]);
        assert_eq!(t.kinds[0], [20.0, 10.0, 0.0, 0.0]);
        // C6 is the second reading for its 10; G7 and G13 the first for the 20 after the rest.
        assert_eq!(t.listed[0], [20.0, 30.0, 30.0, 10.0 / 2.0 + 20.0]);
        // Counted in beats: the same at 2 ticks a beat is half.
        assert_eq!(tally(&labels, &reads, &played, false, 2).right, [12.5, 7.5, 10.0, 10.0]);
    }

    #[test]
    fn a_slash_chord_read_with_its_bass_as_a_chord_note_has_the_notes_but_not_the_name() {
        let labels = [(0, 8, harte::named("D:maj/b7"))];
        let read = |c: &str| Read {
            start: 0,
            end: 8,
            chord: chord(c),
            name: c.to_owned(),
            optional: PcSet::EMPTY,
            others: Vec::new(),
        };
        let played = [PcSet::of(&[0, 2, 6, 9])];
        // D7/C: the same four notes over the same bass, but the C written as the chord's seventh.
        let t = tally(&labels, &[read("D7/C")], &played, true, 1);
        assert_eq!(t.right, [8.0, 0.0, 0.0, 0.0]);
        assert_eq!(t.kinds[2], [0.0, 8.0, 0.0, 0.0]);
        let t = tally(&labels, &[read("D/C")], &played, true, 1);
        assert_eq!(t.right, [8.0, 8.0, 8.0, 8.0]);
        assert_eq!(t.kinds[2], [0.0, 0.0, 8.0, 0.0]);
    }

    #[test]
    fn a_name_stands_for_the_notes_a_chord_may_leave_out_only_where_the_label_has_them() {
        let left_out = |c: &str| may_leave_out(chord(c)).iter().map(|p| p.number()).collect::<Vec<_>>();
        assert_eq!(left_out("Cm13"), [2, 5, 7]);
        assert_eq!(left_out("C13"), [2, 7]);
        assert_eq!(left_out("Cm11"), [2, 7]);
        assert_eq!(left_out("Cmaj9"), [7]);
        assert_eq!(left_out("C7sus4"), [7]);
        assert_eq!(left_out("Csus4"), Vec::<u8>::new());
        assert_eq!(left_out("C5"), Vec::<u8>::new());
        assert_eq!(left_out("C7alt"), Vec::<u8>::new());
        assert_eq!(left_out("Cm7b5"), Vec::<u8>::new());
        // On another root: the 9 (F) and fifth (Bb) of Eb13.
        assert_eq!(left_out("Eb13"), [5, 10]);

        // Scored as the C# app's names are: right only where the name's own notes are the label's.
        let named = |name: &str| Read {
            start: 0,
            end: 4,
            chord: chord(name),
            name: name.to_owned(),
            optional: may_leave_out(chord(name)),
            others: Vec::new(),
        };
        let right =
            |l: &str, name: &str| tally(&[(0, 4, harte::named(l))], &[named(name)], &[PcSet::EMPTY], true, 1).right;
        for (l, name) in [("C:min13", "Cm13"), ("C:min7(11)", "Cm11"), ("C:7(13)", "C13"), ("C:7(*5)", "C7")] {
            assert_eq!(right(l, name), [4.0; 4], "{l} {name}");
        }
        // A name's own notes count: the 13 of Cm13 isn't in a min7, nor the 11 of Cm11 in a min9.
        assert_eq!(right("C:min7", "Cm13"), [0.0, 0.0, 4.0, 4.0]);
        assert_eq!(right("C:min9", "Cm11"), [0.0, 0.0, 4.0, 4.0]);
        assert_eq!(right("C:9", "C13"), [0.0, 0.0, 4.0, 4.0]);
        // A 7alt doesn't say which alterations: the event, not the name.
        assert_eq!(right("C:7(#9,b13)", "C7alt"), [0.0, 0.0, 4.0, 4.0]);
        // The extensions it is given are the label's: the 11 of Cm13 is no false 11 on a minor 13 without one.
        let t = tally(&[(0, 4, harte::named("C:min13"))], &[named("Cm13")], &[PcSet::EMPTY], true, 1);
        assert_eq!(names(extensions(label("C:min13"))), ["9", "13"]);
        assert_eq!(t.extensions[4], [0.0, 0.0, 0.0]);
        // An engine that gives its notes gets nothing left out.
        let exact = Read { optional: PcSet::EMPTY, ..named("Cm13") };
        let t = tally(&[(0, 4, harte::named("C:min13"))], &[exact], &[PcSet::EMPTY], true, 1);
        assert_eq!(t.right, [0.0, 0.0, 4.0, 4.0]);
        assert_eq!(t.extensions[4], [0.0, 4.0, 0.0]);
    }
}
