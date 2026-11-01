//! `theorybench tables`: the counted tables the chord reader's template priors read (harmony/design/
//! chord-reader-design.md, "Model, training and data"): how often each core is the labelled chord (f_core), and how
//! often a chord of each core names each tension it can carry (f_ten).
//!
//! They are counted from the labels alone, never from what a reader hears, in beats, on folds 1 to 4 of the pieces
//! whose licence lets the numbers ship: fold 0 stays kept back, as keyfit keeps it. Each source counts as much as any
//! other, and half a beat is added to each cell, so nothing is ever impossible. Only the sources whose labels name
//! 9ths count towards f_ten: POP909-CL names its chords by triad and seventh templates, so its labels never say a 9 is
//! played, even where the piano plays one, and counting them would say tensions are rarer than they are.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use mo_harmony::PcSet;
use mo_harmony::chordread::{Core, EVEN, Tables};

use crate::harte::{self, Named};
use crate::item::{self, Item};
use crate::keyfit;

/// What's added to each cell, in beats.
const PRIOR: f64 = 0.5;

/// How many cores there are.
const CORES: usize = Core::ALL.len();

/// The beats of labelled chords in one source.
#[derive(Clone, Debug, Default, PartialEq)]
struct Counts {
    /// For each core (in [`Core::ALL`]'s order), the beats labelled with it.
    core: [f64; CORES],
    /// For each core, and each interval above its root, the beats labelled with it whose label names that tension.
    tension: [[f64; 12]; CORES],
    /// The beats labelled with no chord, a chord no core names (a root alone), or a label that can't be read.
    other: f64,
    /// The pieces counted.
    pieces: usize,
}

impl Counts {
    /// The beats labelled with a core.
    fn total(&self) -> f64 {
        self.core.iter().sum()
    }

    /// Whether its labels name tensions: whether any names a 9, the commonest, which any chart that names tensions
    /// names. Only those sources count towards f_ten (POP909-CL names a few altered fifths, C7♯5's ♭13, but never a
    /// 9).
    fn names_tensions(&self) -> bool {
        self.tension.iter().any(|row| row.get(2).is_some_and(|&beats| beats > 0.0))
    }

    /// Counts a piece's labels.
    fn add(&mut self, item: &Item) {
        self.pieces += 1;
        let ppq = f64::from(item.ppq.max(1));
        for (start, end, label) in &item.chords {
            let beats = (end - start).max(0) as f64 / ppq;
            let Some((core, tensions)) = reading(label) else {
                self.other += beats;
                continue;
            };
            if let Some(cell) = self.core.get_mut(core.index()) {
                *cell += beats;
            }
            for t in tensions.iter() {
                let cell = self.tension.get_mut(core.index()).and_then(|row| row.get_mut(usize::from(t.number())));
                if let Some(cell) = cell {
                    *cell += beats;
                }
            }
        }
    }
}

/// The core a label names, and the tensions it names (as intervals above its root); `None` for no chord, a chord no
/// core names (a root alone) or a label that can't be read. A dominant eleventh (its third and its 11 with a ♭7) is
/// read as the 9sus4 players voice it, as `convert/voice.py` plays it.
fn reading(label: &str) -> Option<(Core, PcSet)> {
    let Named::Chord(chord) = harte::named(label) else { return None };
    let mut intervals = chord.intervals();
    if intervals.has(4) && intervals.has(5) && intervals.has(10) {
        intervals = intervals.difference(PcSet::of(&[4]));
    }
    let core = Core::of(intervals)?;
    Some((core, intervals.difference(core.intervals()).intersection(core.tensions())))
}

/// The tables from each source's counts: each source weighed so that it counts as much as any other (its beats
/// scaled to the sources' mean), and half a beat added to each cell. f_ten comes from the sources whose labels name
/// tensions; its cells for notes a core doesn't carry stay at a half (the priors never read them).
fn tables(sources: &BTreeMap<String, Counts>) -> Tables {
    let mut out = EVEN;
    let weighed = |counts: &[&Counts]| -> Vec<(f64, Counts)> {
        let mean = counts.iter().map(|c| c.total()).sum::<f64>() / counts.len().max(1) as f64;
        counts.iter().map(|&c| (if c.total() > 0.0 { mean / c.total() } else { 0.0 }, c.clone())).collect()
    };
    let all: Vec<&Counts> = sources.values().collect();
    let all = weighed(&all);
    let total: f64 = all.iter().map(|(w, c)| w * c.total()).sum();
    for (i, cell) in out.core.iter_mut().enumerate() {
        let beats: f64 = all.iter().map(|(w, c)| w * c.core.get(i).copied().unwrap_or(0.0)).sum();
        *cell = (beats + PRIOR) / (total + PRIOR * CORES as f64);
    }
    let naming: Vec<&Counts> = sources.values().filter(|c| c.names_tensions()).collect();
    let naming = weighed(&naming);
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
    ["1", "b9", "9", "#9", "3", "11", "#11", "5", "b13", "13", "b7", "7"]
        .get(usize::from(interval))
        .copied()
        .unwrap_or("?")
}

/// What was counted: each source's beats by core, and the tables.
fn report(sources: &BTreeMap<String, Counts>, out: &Tables) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "Counted on folds 1 to 4, in beats (each source weighed to count as much as any other):\n");
    let _ = write!(text, "{:<8}", "core");
    for (source, counts) in sources {
        let _ = write!(text, "{:>14}", format!("{source} ({})", counts.pieces));
    }
    let _ = writeln!(text, "{:>10}", "f_core");
    for core in Core::ALL {
        let _ = write!(text, "{:<8}", name(core));
        for counts in sources.values() {
            let _ = write!(text, "{:>14.1}", counts.core.get(core.index()).copied().unwrap_or(0.0));
        }
        let _ = writeln!(text, "{:>10.5}", out.core.get(core.index()).copied().unwrap_or(0.0));
    }
    let _ = write!(text, "{:<8}", "none");
    for counts in sources.values() {
        let _ = write!(text, "{:>14.1}", counts.other);
    }
    let naming: Vec<&str> = sources.iter().filter(|(_, c)| c.names_tensions()).map(|(s, _)| s.as_str()).collect();
    let _ = writeln!(text, "\n\nf_ten, from the labels of {}:\n", naming.join(", "));
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
    text
}

/// The Rust file `--write` writes: the tables as `chordread::COUNTED`, and the sources they were counted from.
fn rust_file(sources: &BTreeMap<String, Counts>, out: &Tables) -> String {
    let mut text = String::new();
    text.push_str(
        "//! The counted tables the chord reader's template priors read: how often each core is the labelled chord, and how\n\
         //! often a chord of each core names each tension it can carry. Written by `theorybench tables --write` from the\n\
         //! labels alone (folds 1 to 4, in beats, each source counting as much as any other, half a beat added to each\n\
         //! cell); don't edit it by hand.\n\nuse super::weights::Tables;\n\n",
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
    text.push_str("/// The counted tables: f_core for each core, then f_ten for each core and interval above its root (a half where\n");
    text.push_str("/// the core doesn't carry that note: never read).\n#[rustfmt::skip]\npub const COUNTED: Tables = Tables {\n    core: [\n");
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
    // The folds are drawn over every piece read, as keyfit draws them on the same files.
    let folds = keyfit::folds(&items);
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

    #[test]
    fn a_label_is_counted_by_its_core_and_the_tensions_it_names() {
        assert_eq!(reading("C:maj"), Some((Core::Major, PcSet::EMPTY)));
        assert_eq!(reading("D:min9"), Some((Core::Minor7, PcSet::of(&[2]))));
        // A 13 names its 9 and 13 (its 11 is no note a dominant carries); a maj13 its 9 and 13 (the 11 doesn't belong).
        assert_eq!(reading("G:13"), Some((Core::Dominant7, PcSet::of(&[2, 9]))));
        assert_eq!(reading("F:maj13"), Some((Core::Major7, PcSet::of(&[2, 9]))));
        // A dominant eleventh is the 9sus4 players voice.
        assert_eq!(reading("G:11"), Some((Core::Dominant7Sus4, PcSet::of(&[2]))));
        assert_eq!(reading("A:min11"), Some((Core::Minor7, PcSet::of(&[2, 5]))));
        // Inversions don't matter; no chord, a root alone and nonsense name no core.
        assert_eq!(reading("C:maj/3"), Some((Core::Major, PcSet::EMPTY)));
        assert_eq!((reading("N"), reading("C:(1)"), reading("H:maj")), (None, None, None));
        // Beats: two beats of G9, one of C, half a beat of no chord.
        let c = counts("billboard", &[(0, 960, "G:9"), (960, 1440, "C:maj"), (1440, 1680, "N")]);
        assert_eq!((c.core[Core::Dominant7.index()], c.core[Core::Major.index()], c.other), (2.0, 1.0, 0.5));
        assert_eq!((c.tension[Core::Dominant7.index()][2], c.total(), c.pieces), (2.0, 3.0, 1));
        assert!(c.names_tensions() && !counts("pop909cl", &[(0, 960, "G:7")]).names_tensions());
        // A 7♯5 names its ♭13, but a source that never names a 9 doesn't name tensions.
        let sharp_five = counts("pop909cl", &[(0, 960, "C:(1,3,#5,b7)")]);
        assert_eq!(sharp_five.tension[Core::Dominant7.index()][8], 2.0);
        assert!(!sharp_five.names_tensions());
    }

    #[test]
    fn each_source_counts_as_much_as_any_other_and_half_a_beat_is_added_to_each_cell() {
        // One source of 30 beats, all major; one of 10, all minor seventh, half of them with a 9.
        let mut sources = BTreeMap::new();
        sources.insert("a".to_owned(), counts("a", &[(0, 30 * 480, "C:maj")]));
        sources.insert("b".to_owned(), counts("b", &[(0, 5 * 480, "D:min7"), (5 * 480, 10 * 480, "D:min9")]));
        let t = tables(&sources);
        // Each source counts as the mean, 20 beats: major 20, minor seventh 20, of 40, with half a beat a cell.
        let f = |beats: f64| (beats + 0.5) / (40.0 + 8.0);
        assert_eq!((t.core[Core::Major.index()], t.core[Core::Minor7.index()]), (f(20.0), f(20.0)));
        assert_eq!(t.core[Core::Augmented.index()], f(0.0));
        assert!((t.core.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        // f_ten from b alone (a names none): its 9 on half the minor sevenths; its 11, never; on major, nothing seen.
        let m7 = Core::Minor7.index();
        assert_eq!((t.tension[m7][2], t.tension[m7][5]), (5.5 / 11.0, 0.5 / 11.0));
        assert_eq!(t.tension[Core::Major.index()][2], 0.5);
        // A note a core doesn't carry keeps the half it starts with.
        assert_eq!(t.tension[m7][4], 0.5);
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
        assert_eq!(file.matches(" // ").count(), 2 * CORES);
    }
}
