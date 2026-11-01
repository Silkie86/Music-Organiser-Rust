//! `theorybench tables`: the counted tables the chord reader's priors read (harmony/design/chord-reader-design.md,
//! "Model, training and data"): how often each core is the labelled chord (f_core), how often a chord of each core
//! names each tension it can carry (f_ten), and how the labelled chords move from one to the next (f_motion).
//!
//! They are counted from the labels alone, never from what a reader hears, on folds 1 to 4 of the pieces whose licence
//! lets the numbers ship: fold 0 stays kept back, drawn as keyfit draws it. f_core and f_ten are counted in beats,
//! f_motion in changes of chord. Each source counts as much as any other, and half a beat (or half a change) is added
//! to each cell, so nothing is ever impossible. A source only counts for what its labels can say: each core is counted
//! from the sources whose labels name it (POP909-CL has no 6, m6, 7sus4 or power chord), and f_ten from those whose
//! labels name 9ths as a matter of course. POP909-CL names its chords by triad and seventh templates, so its labels
//! never say a 9 is played, even where the piano plays one; counting them would say tensions are rarer than they are.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use mo_harmony::PcSet;
use mo_harmony::chordread::{Core, EVEN, Group, Label, Tables};

use crate::harte::{self, Named};
use crate::item::{self, Item};
use crate::keyfit;

/// What's added to each cell, in beats.
const PRIOR: f64 = 0.5;

/// How many cores there are, and how many groups the pair term tells apart.
const CORES: usize = Core::ALL.len();
const GROUPS: usize = Group::ALL.len();

/// The least share of its labelled time on which a source's labels name a 9 for it to count towards f_ten: a source
/// that names tensions does so on a few percent of its time (Billboard 5.2%), and one stray label in a source that
/// never names them stays far below.
const NAMES_TENSIONS: f64 = 0.01;

/// The beats of labelled chords in one source, and how they move from one to the next.
#[derive(Clone, Debug, Default, PartialEq)]
struct Counts {
    /// For each core (in [`Core::ALL`]'s order), the beats labelled with it.
    core: [f64; CORES],
    /// For each core, and each interval above its root, the beats labelled with it whose label names that tension.
    tension: [[f64; 12]; CORES],
    /// The beats labelled with no chord, a chord no core names (a root alone), or a label that can't be read.
    other: f64,
    /// For each group of a chord ([`Group::ALL`]'s order), each interval the root moves up to the next chord's, and
    /// the next chord's group: the changes of chord labelled.
    motion: [[[f64; GROUPS]; 12]; GROUPS],
    /// The pieces counted.
    pieces: usize,
}

impl Counts {
    /// The beats labelled with a core.
    fn total(&self) -> f64 {
        self.core.iter().sum()
    }

    /// The changes of chord labelled.
    fn changes(&self) -> f64 {
        self.motion.iter().flatten().flatten().sum()
    }

    /// The share of its labelled time whose label names a 9, the commonest tension, which any chart that names
    /// tensions names.
    fn nines(&self) -> f64 {
        let named: f64 = self.tension.iter().filter_map(|row| row.get(2)).sum();
        if self.total() > 0.0 { named / self.total() } else { 0.0 }
    }

    /// Whether its labels name tensions as a matter of course (a 9 on at least [`NAMES_TENSIONS`] of its time). Only
    /// those sources count towards f_ten: POP909-CL names a few altered fifths (C7♯5's ♭13), but never a 9.
    fn names_tensions(&self) -> bool {
        self.nines() >= NAMES_TENSIONS
    }

    /// Whether its labels name a core at all: a source whose labels can't name a core says nothing about how common
    /// it is.
    fn names(&self, core: usize) -> bool {
        self.core.get(core).is_some_and(|&beats| beats > 0.0)
    }

    /// Counts a piece's labels. A change of chord is a label read as another root or core than the one before it
    /// (C to C/E or G7 to G9 is no change); no chord, or a label that can't be read, comes between two chords that
    /// don't count as one moving to the other.
    fn add(&mut self, item: &Item) {
        self.pieces += 1;
        let ppq = f64::from(item.ppq.max(1));
        let mut before: Option<Label> = None;
        for (start, end, label) in &item.chords {
            let beats = (end - start).max(0) as f64 / ppq;
            let Some((chord, tensions)) = reading(label) else {
                self.other += beats;
                before = None;
                continue;
            };
            let core = chord.core;
            if let Some(cell) = self.core.get_mut(core.index()) {
                *cell += beats;
            }
            for t in tensions.iter() {
                let cell = self.tension.get_mut(core.index()).and_then(|row| row.get_mut(usize::from(t.number())));
                if let Some(cell) = cell {
                    *cell += beats;
                }
            }
            if let Some(from) = before.filter(|&from| from != chord) {
                let cell = self.motion.get_mut(from.core.group().index());
                let cell = cell.and_then(|row| row.get_mut(usize::from(chord.root.above(from.root))));
                if let Some(cell) = cell.and_then(|cell| cell.get_mut(core.group().index())) {
                    *cell += 1.0;
                }
            }
            before = Some(chord);
        }
    }
}

/// The chord a label names, as a root and a core, and the tensions it names (as intervals above its root); `None` for
/// no chord, a chord no core names (a root alone) or a label that can't be read. A dominant eleventh (its major third,
/// no ♯9, and its 11 with a ♭7) is read as the 9sus4 players voice it, as `convert/voice.py` plays it.
fn reading(label: &str) -> Option<(Label, PcSet)> {
    let Named::Chord(chord) = harte::named(label) else { return None };
    let mut intervals = chord.intervals();
    if intervals.has(4) && !intervals.has(3) && intervals.has(5) && intervals.has(10) {
        intervals = intervals.difference(PcSet::of(&[4]));
    }
    let core = Core::of(intervals)?;
    Some((Label::new(chord.root(), core), intervals.difference(core.intervals()).intersection(core.tensions())))
}

/// Each source's weight, so that it counts as much as any other: what it counts (its beats, or its changes of chord)
/// scaled to the sources' mean.
fn weighed<'a>(sources: impl Iterator<Item = &'a Counts>, counted: impl Fn(&Counts) -> f64) -> Vec<(f64, &'a Counts)> {
    let sources: Vec<&Counts> = sources.collect();
    let mean = sources.iter().map(|&c| counted(c)).sum::<f64>() / sources.len().max(1) as f64;
    sources.into_iter().map(|c| (if counted(c) > 0.0 { mean / counted(c) } else { 0.0 }, c)).collect()
}

/// The tables from each source's counts, each source weighed to count as much as any other, with half a beat (or
/// half a change) added to each cell.
///
/// Each core is counted from the sources whose labels name it (from all of them when none does), and the cores'
/// shares are then scaled to add up to 1: where every source names every core, that changes nothing. f_ten comes from
/// the sources whose labels name tensions; its cells for notes a core doesn't carry stay at a half (the priors never
/// read them). f_motion comes from every source, each group of the chord before on its own: its 36 shares add up to 1.
fn tables(sources: &BTreeMap<String, Counts>) -> Tables {
    let mut out = EVEN;
    let all = weighed(sources.values(), Counts::total);
    for (i, cell) in out.core.iter_mut().enumerate() {
        let naming = all.iter().filter(|(_, c)| c.names(i));
        let naming: Vec<&(f64, &Counts)> =
            if all.iter().any(|(_, c)| c.names(i)) { naming.collect() } else { all.iter().collect() };
        let beats: f64 = naming.iter().map(|(w, c)| w * c.core.get(i).copied().unwrap_or(0.0)).sum();
        let total: f64 = naming.iter().map(|(w, c)| w * c.total()).sum();
        *cell = (beats + PRIOR) / (total + PRIOR * CORES as f64);
    }
    let sum: f64 = out.core.iter().sum();
    out.core.iter_mut().for_each(|cell| *cell /= sum);
    let naming = weighed(sources.values().filter(|c| c.names_tensions()), Counts::total);
    for core in Core::ALL {
        let i = core.index();
        let of_core: f64 = naming.iter().map(|(w, c)| w * c.core.get(i).copied().unwrap_or(0.0)).sum();
        for t in core.tensions().iter() {
            let k = usize::from(t.number());
            let at = |c: &Counts| c.tension.get(i).and_then(|row| row.get(k)).copied().unwrap_or(0.0);
            let named: f64 = naming.iter().map(|(w, c)| w * at(c)).sum();
            if let Some(cell) = out.tension.get_mut(i).and_then(|row| row.get_mut(k)) {
                *cell = (named + PRIOR) / (of_core + 2.0 * PRIOR);
            }
        }
    }
    let moving = weighed(sources.values(), Counts::changes);
    for (g, row) in out.motion.iter_mut().enumerate() {
        let at = |c: &Counts, up: usize, to: usize| {
            c.motion.get(g).and_then(|row| row.get(up)).and_then(|cell| cell.get(to)).copied().unwrap_or(0.0)
        };
        let from: f64 = moving.iter().map(|(w, c)| w * c.motion.get(g).map_or(0.0, |r| r.iter().flatten().sum())).sum();
        for (up, cell) in row.iter_mut().enumerate() {
            for (to, share) in cell.iter_mut().enumerate() {
                let moved: f64 = moving.iter().map(|(w, c)| w * at(c, up, to)).sum();
                *share = (moved + PRIOR) / (from + PRIOR * (12 * GROUPS) as f64);
            }
        }
    }
    out
}

/// A core's name in the report and the file: "maj" for a major triad.
fn name(core: Core) -> &'static str {
    match core.name() {
        "" => "maj",
        name => name,
    }
}

/// A tension's name, by its interval above the root.
fn tension_name(interval: u8) -> &'static str {
    harte::COMPOUND.get(usize::from(interval)).copied().unwrap_or("?")
}

/// How a root moves to the next chord's, by the semitones it moves up: "up 4th" for V to I, "up m2" for C♯dim7 to Dm,
/// "up M7" for a tritone sub falling a semitone (D♭7 to C).
const UP: [&str; 12] = [
    "same root",
    "up m2",
    "up M2",
    "up m3",
    "up M3",
    "up 4th",
    "up tritone",
    "up 5th",
    "up m6",
    "up M6",
    "up m7",
    "up M7",
];

/// What was counted: each source's beats by core, and the tables.
fn report(sources: &BTreeMap<String, Counts>, out: &Tables) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "Counted on folds 1 to 4, in beats; each core from the sources that name it, each source weighed to count as\n\
         much as any other:\n"
    );
    let _ = write!(text, "{:<8}", "core");
    for (source, counts) in sources {
        let _ = write!(text, "{:>17}", format!("{source} ({})", counts.pieces));
    }
    let _ = writeln!(text, "{:>10}", "f_core");
    for core in Core::ALL {
        let _ = write!(text, "{:<8}", name(core));
        for counts in sources.values() {
            let _ = write!(text, "{:>17.1}", counts.core.get(core.index()).copied().unwrap_or(0.0));
        }
        let _ = writeln!(text, "{:>10.5}", out.core.get(core.index()).copied().unwrap_or(0.0));
    }
    let _ = write!(text, "{:<8}", "none");
    for counts in sources.values() {
        let _ = write!(text, "{:>17.1}", counts.other);
    }
    let _ = write!(text, "\n{:<8}", "a 9");
    for counts in sources.values() {
        let _ = write!(text, "{:>16.2}%", 100.0 * counts.nines());
    }
    let naming: Vec<&str> = sources.iter().filter(|(_, c)| c.names_tensions()).map(|(s, _)| s.as_str()).collect();
    let _ = writeln!(
        text,
        "\n\nf_ten, from the labels of {} (a 9 named on at least {}% of the time):\n",
        naming.join(", "),
        100.0 * NAMES_TENSIONS
    );
    for core in Core::ALL {
        let row: Vec<String> = core
            .tensions()
            .iter()
            .map(|t| {
                let f = out.tension.get(core.index()).and_then(|r| r.get(usize::from(t.number()))).copied();
                format!("{} {:.4}", tension_name(t.number()), f.unwrap_or(0.5))
            })
            .collect();
        let _ = writeln!(text, "{:<8}{}", name(core), row.join("  "));
    }
    let changes: Vec<String> = sources.iter().map(|(s, c)| format!("{s} {:.0}", c.changes())).collect();
    let _ = writeln!(
        text,
        "\nf_motion, from the changes of chord labelled ({}), each source counting as much as any other: for each\n\
         group of the chord before, the share of its changes (%) to a root each interval up, by the next chord's group:\n",
        changes.join(", ")
    );
    let _ = write!(text, "{:<11}", "root");
    for from in Group::ALL {
        let _ = write!(text, "{:>30}", format!("from {}", from.name()));
    }
    let _ = write!(text, "\n{:<11}", "");
    for _ in Group::ALL {
        for to in Group::ALL {
            let _ = write!(text, "{:>10}", to.name());
        }
    }
    let _ = writeln!(text);
    for (up, interval) in UP.iter().enumerate() {
        let _ = write!(text, "{interval:<11}");
        for from in Group::ALL {
            for to in Group::ALL {
                let f = out.motion.get(from.index()).and_then(|r| r.get(up)).and_then(|c| c.get(to.index()));
                let _ = write!(text, "{:>10.2}", 100.0 * f.copied().unwrap_or(0.0));
            }
        }
        let _ = writeln!(text);
    }
    text
}

/// The Rust file `--write` writes: the tables as `chordread::COUNTED`, and the sources they were counted from.
fn rust_file(sources: &BTreeMap<String, Counts>, out: &Tables) -> String {
    let mut text = String::new();
    text.push_str(
        "//! The counted tables the chord reader's priors read: how often each core is the labelled chord, how often a chord\n\
         //! of each core names each tension it can carry, and how the labelled chords move from one to the next. Written by\n\
         //! `theorybench tables --write` from the labels alone (folds 1 to 4; in beats, each core from the sources whose\n\
         //! labels name it, and in changes of chord; each source counting as much as any other, half a beat or half a\n\
         //! change added to each cell); don't edit it by hand.\n\nuse super::weights::Tables;\n\n",
    );
    let names: Vec<String> = sources.keys().map(|s| format!("{s:?}")).collect();
    let naming: Vec<String> =
        sources.iter().filter(|(_, c)| c.names_tensions()).map(|(s, _)| format!("{s:?}")).collect();
    let _ = writeln!(
        text,
        "/// The sources counted, by the names `theorybench` gives them; f_ten is counted from those whose labels name\n\
         /// tensions ({}).\npub const COUNTED_FROM: &[&str] = &[{}];\n",
        naming.join(", "),
        names.join(", ")
    );
    text.push_str("/// The counted tables: f_core for each core; f_ten for each core and interval above its root (a half where the\n");
    text.push_str("/// core doesn't carry that note: never read); and f_motion for each group of the chord before, each interval its\n");
    text.push_str("/// root moves up and each group of the next chord.\n#[rustfmt::skip]\npub const COUNTED: Tables = Tables {\n    core: [\n");
    for core in Core::ALL {
        let f = out.core.get(core.index()).copied().unwrap_or(0.0);
        let _ = writeln!(text, "        {f:?}, // {}", name(core));
    }
    text.push_str("    ],\n    tension: [\n");
    for core in Core::ALL {
        let row = out.tension.get(core.index()).copied().unwrap_or([0.5; 12]);
        let cells: Vec<String> = row.iter().map(|f| format!("{f:?}")).collect();
        let _ = writeln!(text, "        [{}], // {}", cells.join(", "), name(core));
    }
    text.push_str("    ],\n    motion: [\n");
    for from in Group::ALL {
        let _ = writeln!(text, "        [ // from {}", from.name());
        for (up, interval) in UP.iter().enumerate() {
            let cell = out.motion.get(from.index()).and_then(|r| r.get(up)).copied().unwrap_or([0.0; GROUPS]);
            let cells: Vec<String> = cell.iter().map(|f| format!("{f:?}")).collect();
            let _ = writeln!(text, "            [{}], // {interval}", cells.join(", "));
        }
        text.push_str("        ],\n");
    }
    text.push_str("    ],\n};\n");
    text
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut paths = Vec::new();
    let mut write: Option<PathBuf> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--write" => write = Some(rest.next().ok_or("--write needs a file")?.into()),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    if paths.is_empty() {
        return Err("tables needs at least one labelled .jsonl".to_owned());
    }
    let mut items: Vec<Item> = Vec::new();
    for path in &paths {
        items.extend(item::read(path)?);
    }
    // The folds are drawn as keyfit draws them on the same files (which may leave out a piece only measured).
    let folds = keyfit::folds(&mut items);
    let mut sources: BTreeMap<String, Counts> = BTreeMap::new();
    for (item, fold) in items.iter().zip(folds) {
        if item.may_learn() && fold != keyfit::FROZEN {
            sources.entry(item.source.clone()).or_default().add(item);
        }
    }
    if sources.is_empty() {
        return Err("no piece whose licence lets the numbers ship, outside the kept-back fold".to_owned());
    }
    let out = tables(&sources);
    print!("{}", report(&sources, &out));
    if let Some(path) = write {
        std::fs::write(&path, rust_file(&sources, &out)).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("\nwrote {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_harmony::PitchClass;

    fn item(source: &str, chords: &[(i64, i64, &str)]) -> Item {
        let mut item: Item =
            serde_json::from_str(r#"{"source":"x","id":"1","ppq":480,"length":3840,"parts":[]}"#).unwrap();
        item.source = source.to_owned();
        item.chords = chords.iter().map(|&(s, e, l)| (s, e, l.to_owned())).collect();
        item
    }

    fn counts(source: &str, chords: &[(i64, i64, &str)]) -> Counts {
        let mut c = Counts::default();
        c.add(&item(source, chords));
        c
    }

    /// The core a label is read as, and the tensions it names.
    fn core_of(label: &str) -> Option<(Core, PcSet)> {
        reading(label).map(|(chord, tensions)| (chord.core, tensions))
    }

    #[test]
    fn a_label_is_counted_by_its_core_and_the_tensions_it_names() {
        assert_eq!(reading("C:maj"), Some((Label::new(PitchClass::C, Core::Major), PcSet::EMPTY)));
        assert_eq!(reading("D:min9"), Some((Label::new(PitchClass::D, Core::Minor7), PcSet::of(&[2]))));
        // A 13 names its 9 and 13 (its 11 is no note a dominant carries); a maj13 its 9 and 13 (the 11 doesn't belong).
        assert_eq!(core_of("G:13"), Some((Core::Dominant7, PcSet::of(&[2, 9]))));
        assert_eq!(core_of("F:maj13"), Some((Core::Major7, PcSet::of(&[2, 9]))));
        // A dominant eleventh is the 9sus4 players voice; with a ♯9 it stays a dominant (its 11 a note it can't carry).
        assert_eq!(core_of("G:11"), Some((Core::Dominant7Sus4, PcSet::of(&[2]))));
        assert_eq!(core_of("C:7(#9,11)"), Some((Core::Dominant7, PcSet::of(&[3]))));
        assert_eq!(core_of("A:min11"), Some((Core::Minor7, PcSet::of(&[2, 5]))));
        // Inversions don't matter; no chord, a root alone and nonsense name no core.
        assert_eq!(core_of("C:maj/3"), Some((Core::Major, PcSet::EMPTY)));
        assert_eq!((reading("N"), reading("C:(1)"), reading("H:maj")), (None, None, None));
        assert_eq!(reading("Bb:7/b7").map(|(chord, _)| chord), Some(Label::new(PitchClass::new(10), Core::Dominant7)));
        // Beats: two beats of G9, one of C, half a beat of no chord.
        let c = counts("billboard", &[(0, 960, "G:9"), (960, 1440, "C:maj"), (1440, 1680, "N")]);
        assert_eq!((c.core[Core::Dominant7.index()], c.core[Core::Major.index()], c.other), (2.0, 1.0, 0.5));
        assert_eq!((c.tension[Core::Dominant7.index()][2], c.total(), c.pieces), (2.0, 3.0, 1));
        assert!(c.names_tensions() && !counts("pop909cl", &[(0, 960, "G:7")]).names_tensions());
        assert!(c.names(Core::Major.index()) && !c.names(Core::Minor.index()));
        // A 7♯5 names its ♭13, but a source that never names a 9 doesn't name tensions.
        let sharp_five = counts("pop909cl", &[(0, 960, "C:(1,3,#5,b7)")]);
        assert_eq!(sharp_five.tension[Core::Dominant7.index()][8], 2.0);
        assert!(!sharp_five.names_tensions());
        // Nor does one with a stray 9 on under 1% of its time: a beat in 101 doesn't make it, a beat in 100 does.
        let stray = |beats: i64| counts("x", &[(0, beats * 480, "C:maj"), (beats * 480, (beats + 1) * 480, "C:maj9")]);
        assert!(!stray(100).names_tensions() && stray(99).names_tensions());
    }

    #[test]
    fn each_core_counts_from_the_sources_that_name_it_each_as_much_as_any_other() {
        // a: 30 beats of major and minor triads, from labels that never name a 6 or a minor seventh. b: 10 beats that
        // name them too, with a 9 on one of them.
        let beats =
            |chords: &[(i64, i64, &'static str)]| chords.iter().map(|&(s, e, l)| (s * 480, e * 480, l)).collect();
        let a: Vec<(i64, i64, &str)> = beats(&[(0, 15, "C:maj"), (15, 30, "A:min")]);
        let b: Vec<(i64, i64, &str)> =
            beats(&[(0, 3, "C:maj"), (3, 5, "A:min"), (5, 7, "C:maj6"), (7, 9, "D:min7"), (9, 10, "D:min9")]);
        let mut sources = BTreeMap::new();
        sources.insert("a".to_owned(), counts("a", &a));
        sources.insert("b".to_owned(), counts("b", &b));
        let t = tables(&sources);
        // Each source counts as the mean, 20 beats (a's beats at two thirds, b's twice). Major: 10 + 6 of 40. The 6
        // and the minor seventh only from b: 4 and 6 of 20. Half a beat to each cell, then the shares scaled to add up
        // to 1.
        let raw = |beats: f64, total: f64| (beats + 0.5) / (total + 8.0);
        let sum = raw(16.0, 40.0) + raw(14.0, 40.0) + raw(4.0, 20.0) + raw(6.0, 20.0) + 12.0 * raw(0.0, 40.0);
        let near = |x: f64, y: f64| (x - y).abs() < 1e-12;
        assert!(near(t.core[Core::Major.index()], raw(16.0, 40.0) / sum));
        assert!(near(t.core[Core::Major6.index()], raw(4.0, 20.0) / sum));
        assert!(near(t.core[Core::Augmented.index()], raw(0.0, 40.0) / sum));
        assert!(near(t.core.iter().sum::<f64>(), 1.0));
        // f_ten from b alone (a names no 9): the 9 on one of the minor seventh's three beats, its 11 never; nothing
        // named on the major triad's three. A note a core doesn't carry keeps the half it starts with.
        let m7 = Core::Minor7.index();
        assert_eq!((t.tension[m7][2], t.tension[m7][5]), (1.5 / 4.0, 0.5 / 4.0));
        assert_eq!(t.tension[Core::Major.index()][2], 0.5 / 4.0);
        assert_eq!(t.tension[m7][4], 0.5);
        // Where the sources name the same cores, they pool, and the shares already add up to 1: a, 30 beats of major
        // and minor triads; b, 10.
        let mut pooled = BTreeMap::new();
        pooled.insert("a".to_owned(), counts("a", &beats(&[(0, 20, "C:maj"), (20, 30, "A:min")])));
        pooled.insert("b".to_owned(), counts("b", &beats(&[(0, 5, "C:maj"), (5, 10, "A:min")])));
        let t = tables(&pooled);
        let f = |beats: f64| (beats + 0.5) / (40.0 + 8.0);
        assert!(near(t.core[Core::Major.index()], f(20.0 * 2.0 / 3.0 + 10.0)));
        assert!(near(t.core[Core::Minor.index()], f(10.0 * 2.0 / 3.0 + 10.0)));
        assert!(near(t.core[Core::Augmented.index()], f(0.0)));
    }

    #[test]
    fn the_file_holds_the_numbers_to_the_last_bit() {
        let mut sources = BTreeMap::new();
        sources
            .insert("billboard".to_owned(), counts("billboard", &[(0, 480 * 3, "G:9"), (480 * 3, 480 * 7, "C:maj")]));
        let t = tables(&sources);
        let file = rust_file(&sources, &t);
        assert!(file.contains("pub const COUNTED_FROM: &[&str] = &[\"billboard\"];"));
        assert!(file.contains(&format!("        {:?}, // 7\n", t.core[Core::Dominant7.index()])));
        assert!(file.contains(&format!("{:?}, ", t.tension[Core::Dominant7.index()][2])));
        // G9 to C: from a dominant, up a fourth, to another chord.
        let [dominant, other] = [Group::Dominant.index(), Group::Other.index()];
        let moved = t.motion[dominant][5];
        assert!(file.contains(&format!("            [{:?}, {:?}, {:?}], // up 4th\n", moved[0], moved[1], moved[2])));
        assert!(moved[other] > moved[dominant] && file.contains("        [ // from dim/aug\n"));
        assert_eq!(file.matches(" // ").count(), 2 * CORES + GROUPS + 12 * GROUPS);
    }

    #[test]
    fn a_change_of_chord_is_counted_by_how_its_root_moves_and_the_groups_of_the_two() {
        let beats =
            |chords: &[(i64, i64, &'static str)]| chords.iter().map(|&(s, e, l)| (s * 480, e * 480, l)).collect();
        let chords: Vec<(i64, i64, &str)> = beats(&[
            (0, 1, "G:7"),
            (1, 2, "C:maj"),
            (2, 3, "C:maj/3"),
            (3, 4, "C:maj7"),
            (4, 5, "N"),
            (5, 6, "D:min"),
            (6, 7, "C#:dim7"),
            (7, 8, "D:min7"),
            (8, 9, "X"),
            (9, 10, "Db:9"),
            (10, 11, "C:maj(9)"),
        ]);
        let c = counts("billboard", &chords);
        let [dominant, dim_aug, other] = Group::ALL.map(Group::index);
        let mut want = [[[0.0; GROUPS]; 12]; GROUPS];
        // G7 to C and D♭9 to C: a dominant up a fourth, and down a semitone (a tritone sub), to another chord. C to
        // C/E is no change, C to Cmaj7 another chord on the same root; Dm to C♯dim7 down a semitone to a diminished
        // seventh, which rises a semitone to Dm7. No chord, and a label that can't be read, break the line.
        want[dominant][5][other] = 1.0;
        want[dominant][11][other] = 1.0;
        want[other][0][other] = 1.0;
        want[other][11][dim_aug] = 1.0;
        want[dim_aug][1][other] = 1.0;
        assert_eq!(c.motion, want);
        assert_eq!(c.changes(), 5.0);
        // Each source counts as much as any other: a, with its one change from a dominant up a fourth, and b, with
        // three from other chords up a fifth, count one change each over the mean of two. Each group's 36 shares
        // then add up to 1, with half a change added to each.
        let mut sources = BTreeMap::new();
        sources.insert("a".to_owned(), counts("a", &beats(&[(0, 1, "G:7"), (1, 2, "C:maj")])));
        let fifths = [(0, 1, "C:maj"), (1, 2, "G:maj"), (2, 3, "D:maj"), (3, 4, "A:maj")];
        sources.insert("b".to_owned(), counts("b", &beats(&fifths)));
        let t = tables(&sources);
        let near = |x: f64, y: f64| (x - y).abs() < 1e-12;
        assert!(near(t.motion[dominant][5][other], (2.0 + 0.5) / (2.0 + 18.0)));
        assert!(near(t.motion[dominant][5][dominant], 0.5 / 20.0));
        assert!(near(t.motion[other][7][other], (2.0 + 0.5) / (2.0 + 18.0)));
        assert!(near(t.motion[dim_aug][1][other], 1.0 / 36.0));
        for group in t.motion {
            assert!(near(group.iter().flatten().sum::<f64>(), 1.0));
        }
    }
}
