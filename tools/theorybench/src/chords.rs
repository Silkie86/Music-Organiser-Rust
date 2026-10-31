//! `theorybench chords`: how often each chord reader names the labelled chord, scored as mir_eval scores chord
//! recognition (each stretch of time weighted by its length, pooled over all the songs), with mir_eval's
//! segmentation score for where the chords change.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use mo_harmony::{Chord, PcSet, PitchClass};
use rayon::prelude::*;
use serde::Deserialize;

use crate::harte::{self, Label, Measure};
use crate::item::{self, Item};
use crate::metrics::{self, pct};

/// The chords one engine read in one piece, as the C# helper (`cs/`) writes them: start, end, name, root, bass and
/// the pitch classes (twelve bits, bit 0 for C).
#[derive(Deserialize)]
struct Answers {
    source: String,
    id: String,
    chords: Vec<(i64, i64, String, u8, u8, u16)>,
}

/// A chord read over a stretch of ticks.
type Read = (i64, i64, Chord);

/// One engine's chords for every piece, by source and id.
struct Engine {
    name: String,
    read: HashMap<(String, String), Vec<Read>>,
}

fn load(name: &str, path: &Path) -> Result<Engine, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut read = HashMap::new();
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let a: Answers = serde_json::from_str(&line).map_err(|e| format!("{} line {}: {e}", path.display(), n + 1))?;
        let chords = a
            .chords
            .iter()
            .map(|&(start, end, _, root, bass, pcs)| {
                let root = PitchClass::new(i32::from(root));
                (start, end, Chord::new(root, PcSet::from_bits(pcs).with(root), PitchClass::new(i32::from(bass))))
            })
            .collect();
        read.insert((a.source, a.id), chords);
    }
    Ok(Engine { name: name.to_owned(), read })
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut engines = Vec::new();
    let mut dump = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump" => dump = Some(PathBuf::from(args.next().ok_or("--dump needs a file")?)),
            "--answers" => {
                let spec = args.next().ok_or("--answers needs name=file")?;
                let (name, path) = spec.split_once('=').ok_or("--answers needs name=file")?;
                engines.push(load(name, Path::new(path))?);
            }
            a if a.starts_with("--") => return Err(format!("Unknown option {a}.\n\n{}", crate::USAGE)),
            a => files.push(PathBuf::from(a)),
        }
    }
    if files.is_empty() || engines.is_empty() {
        return Err(crate::USAGE.to_owned());
    }
    let mut items = Vec::new();
    for f in &files {
        items.extend(item::read(f)?);
    }
    let read = items.len();
    items.retain(|i| !i.chords.is_empty());
    let unreadable = items.iter().flat_map(|i| &i.chords).filter(|c| harte::parse(&c.2).is_none()).count();
    let mut labels: Vec<&str> = items.iter().map(|i| i.chord_labels.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    println!(
        "{} pieces read, {} with chord labels (labelled by {}); {} labels mir_eval can't read, left out.",
        read,
        items.len(),
        if labels.is_empty() { "nobody".to_owned() } else { labels.join(", ") },
        unreadable
    );
    let bars: f64 = items.iter().map(|i| i.length as f64 / i.bar() as f64).sum();
    let label_changes: usize = items.iter().map(|i| harte::join(&reference(i)).len()).sum();
    println!("The labels change chord {:.2} times a bar.", label_changes as f64 / bars);

    let scores: Vec<Vec<Score>> =
        engines.iter().map(|e| items.par_iter().map(|i| score(i, e.read.get(&key(i)))).collect()).collect();
    let song: Vec<usize> = (0..items.len()).collect();
    if let Some(path) = dump {
        write_dump(&path, &items, &engines, &scores)?;
    }

    println!();
    println!("Share of the time right, each way mir_eval scores it (likely range when the songs are drawn again):");
    print!("{:<28}", "");
    for m in Measure::ALL {
        print!(" {:>11}", m.name());
    }
    println!();
    for (e, s) in engines.iter().zip(&scores) {
        print!("{:<28}", e.name);
        for (i, _) in Measure::ALL.iter().enumerate() {
            let (right, total) = columns(s, i);
            print!(" {:>11}", pct(metrics::ratio(&right, &total, &song).value));
        }
        println!();
        print!("{:<28}", "");
        for (i, _) in Measure::ALL.iter().enumerate() {
            let (right, total) = columns(s, i);
            let r = metrics::ratio(&right, &total, &song);
            print!(" {:>11}", format!("{:.1}-{:.1}", 100.0 * r.low, 100.0 * r.high));
        }
        println!();
    }

    println!();
    println!(
        "Where the chords change (mir_eval's segmentation: 100 is every labelled chord whole and none run together):"
    );
    println!("{:<28} {:>11} {:>11} {:>11} {:>14}", "", "over-cut", "under-cut", "score", "changes a bar");
    for (e, s) in engines.iter().zip(&scores) {
        let sum = |f: fn(&Score) -> i64| s.iter().map(f).sum::<i64>() as f64;
        let over = sum(|x| x.over_lost) / sum(|x| x.over_total).max(1.0);
        let under = sum(|x| x.under_lost) / sum(|x| x.under_total).max(1.0);
        let changes: usize = s.iter().map(|x| x.changes).sum();
        println!(
            "{:<28} {:>11.1} {:>11.1} {:>11.1} {:>14.2}",
            e.name,
            100.0 * over,
            100.0 * under,
            100.0 * (1.0 - over.max(under)),
            changes as f64 / bars
        );
    }

    if let Some(((first, base), rest)) = engines.iter().zip(&scores).collect::<Vec<_>>().split_first()
        && !rest.is_empty()
    {
        println!();
        println!("Against the {} (points; a range that doesn't cross 0 is a real difference):", first.name);
        for (e, s) in rest {
            let diffs: Vec<(f64, f64, f64)> = (0..Measure::ALL.len())
                .map(|i| {
                    let (a, total) = columns(base, i);
                    let (b, _) = columns(s, i);
                    metrics::paired_ratio(&a, &b, &total, &song)
                })
                .collect();
            print!("{:<28}", e.name);
            for (d, _, _) in &diffs {
                print!(" {:>+11.1}", 100.0 * d);
            }
            println!();
            print!("{:<28}", "");
            for (_, low, high) in &diffs {
                print!(" {:>11}", format!("{:+.1}/{:+.1}", 100.0 * low, 100.0 * high));
            }
            println!();
        }
    }
    Ok(())
}

/// Every piece's labels, chords read and scores, one JSON object a line for each engine and piece, so the scoring can
/// be checked against mir_eval itself (`convert/check_mir_eval.py`) and a piece looked at closely.
fn write_dump(path: &Path, items: &[Item], engines: &[Engine], scores: &[Vec<Score>]) -> Result<(), String> {
    use std::io::Write;
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = std::io::BufWriter::new(file);
    for (e, s) in engines.iter().zip(scores) {
        for (item, score) in items.iter().zip(s) {
            let read = e.read.get(&key(item)).map_or(&[][..], Vec::as_slice);
            let line = serde_json::json!({
                "engine": e.name,
                "source": item.source,
                "id": item.id,
                "reference": item.chords,
                "estimate": read.iter().map(|&(s, e, c)| (s, e, Label::write(c))).collect::<Vec<_>>(),
                "measures": Measure::ALL.iter().map(|m| m.name()).zip(&score.measures).collect::<Vec<_>>(),
                "segmentation": [score.over_lost, score.over_total, score.under_lost, score.under_total],
            });
            writeln!(out, "{line}").map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    out.flush().map_err(|e| format!("{}: {e}", path.display()))
}

fn key(item: &Item) -> (String, String) {
    (item.source.clone(), item.id.clone())
}

/// The labelled chords, as mir_eval reads them (a label it can't read counts as "X", left out of every score).
fn reference(item: &Item) -> Vec<harte::Span> {
    item.chords.iter().map(|(s, e, l)| (*s, *e, harte::parse(l).unwrap_or(Label::UNKNOWN))).collect()
}

/// How one engine did on one piece.
struct Score {
    /// For each of [`Measure::ALL`]: the time it got right, and the time scored.
    measures: Vec<(i64, i64)>,
    over_lost: i64,
    over_total: i64,
    under_lost: i64,
    under_total: i64,
    /// How many chords it read (chords in a row that are the same counted once).
    changes: usize,
}

fn score(item: &Item, read: Option<&Vec<Read>>) -> Score {
    let reference = reference(item);
    let read = read.map_or(&[][..], Vec::as_slice);
    let estimate: Vec<harte::Span> = read.iter().map(|&(s, e, c)| (s, e, Label::of_chord(c))).collect();
    let lined = harte::line_up(&reference, &estimate);
    let measures = Measure::ALL
        .iter()
        .map(|m| {
            lined.iter().fold((0, 0), |(right, total), (length, r, e)| match m.compare(r, e) {
                Some(true) => (right + length, total + length),
                Some(false) => (right, total + length),
                None => (right, total),
            })
        })
        .collect();
    // Segmentation, on chords in a row joined when they read the same: labels with their ninths and so on, chords
    // read with all their notes.
    let joined_reference = harte::join(
        &item
            .chords
            .iter()
            .map(|(s, e, l)| (*s, *e, harte::parse_for_joining(l).unwrap_or(Label::UNKNOWN)))
            .collect::<Vec<_>>(),
    );
    let (from, to) = (reference.first().map_or(0, |r| r.0), reference.last().map_or(0, |r| r.1));
    let whole: Vec<(i64, i64, Option<Chord>)> = read.iter().map(|&(s, e, c)| (s, e, Some(c))).collect();
    let joined_estimate = harte::join(&harte::adjust(from, to, &whole, None));
    let (over_lost, over_total) = harte::hamming_parts(&joined_reference, &joined_estimate);
    let (under_lost, under_total) = harte::hamming_parts(&joined_estimate, &joined_reference);
    let changes = harte::join(&whole).len();
    Score { measures, over_lost, over_total, under_lost, under_total, changes }
}

/// One measure's time right and time scored, song by song.
fn columns(scores: &[Score], measure: usize) -> (Vec<f64>, Vec<f64>) {
    scores.iter().map(|s| s.measures.get(measure).map_or((0.0, 0.0), |&(r, t)| (r as f64, t as f64))).unzip()
}
