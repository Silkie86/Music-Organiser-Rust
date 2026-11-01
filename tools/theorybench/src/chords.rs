//! `theorybench chords`: how often each chord reader names the labelled chord, scored as mir_eval scores chord
//! recognition (each stretch of time weighted by its length, pooled over all the songs), with mir_eval's
//! segmentation score for where the chords change; then the scores the chord reader's design adds ([`naming`]):
//! the whole chord with its extensions, the root among the same notes, the bass kind and the readings listed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mo_harmony::{Chord, PcSet, PitchClass};
use rayon::prelude::*;
use serde::Deserialize;

use crate::harte::{self, Label, Measure, Named};
use crate::item::{self, Item};
use crate::metrics::{self, pct};
use crate::naming::{self, Read, Tally};

/// The chords one engine read in one piece, as the C# helper (`cs/`) writes them: start, end, name, root, bass and
/// the pitch classes (twelve bits, bit 0 for C), then, when the helper lists them, the other readings of the same
/// notes (name, root, bass, pitch classes), likeliest first.
#[derive(Deserialize)]
struct Answers {
    source: String,
    id: String,
    chords: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Row {
    Listed(i64, i64, String, u8, u8, u16, Vec<(serde::de::IgnoredAny, u8, u8, u16)>),
    Plain(i64, i64, String, u8, u8, u16),
}

/// One engine's chords for every piece, by source and id.
struct Engine {
    name: String,
    read: HashMap<(String, String), Vec<Read>>,
    /// Whether it lists other readings anywhere.
    lists: bool,
}

/// A chord as the C# app writes one: its notes are every pitch class it heard (the bass among them, so a bass is
/// always one of its notes, as its name has it: D7/C, never D/C) with the root added, as its name says the root.
fn cs_chord(root: u8, bass: u8, pcs: u16) -> Chord {
    let root = PitchClass::new(i32::from(root));
    Chord::new(root, PcSet::from_bits(pcs).with(root), PitchClass::new(i32::from(bass)))
}

fn load(name: &str, path: &Path) -> Result<Engine, String> {
    let mut lists = false;
    let read = item::read_lines::<Answers>(path)?
        .into_iter()
        .map(|a| {
            let chords = a
                .chords
                .into_iter()
                .map(|row| match row {
                    Row::Plain(start, end, name, root, bass, pcs) => {
                        Read { start, end, chord: cs_chord(root, bass, pcs), name, others: Vec::new() }
                    }
                    Row::Listed(start, end, name, root, bass, pcs, others) => {
                        lists = true;
                        let others = others.into_iter().map(|(_, r, b, p)| cs_chord(r, b, p)).collect();
                        Read { start, end, chord: cs_chord(root, bass, pcs), name, others }
                    }
                })
                .collect();
            ((a.source, a.id), chords)
        })
        .collect();
    Ok(Engine { name: name.to_owned(), read, lists })
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut engines = Vec::new();
    let mut dump = None;
    let mut part = None;
    let mut per_chord = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump" => dump = Some(PathBuf::from(args.next().ok_or("--dump needs a file")?)),
            "--part" => part = Some(args.next().ok_or("--part needs a role")?.clone()),
            "--per-segment" => per_chord = true,
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

    let heard = |role: &str| role != "drums" && part.as_deref().is_none_or(|p| p == role);
    let scores: Vec<Vec<Score>> = engines
        .iter()
        .map(|e| {
            items.par_iter().map(|i| score(i, e.read.get(&key(i)).map_or(&[][..], Vec::as_slice), &heard)).collect()
        })
        .collect();
    let song: Vec<usize> = (0..items.len()).collect();
    if let Some(path) = dump {
        write_dump(&path, &items, &engines, &scores)?;
    }

    println!();
    println!("Share of the time right, each way mir_eval scores it (likely range when the songs are drawn again):");
    print_measures(&engines, &scores, &song, |s, i| s.measures.get(i).copied().unwrap_or_default());
    if per_chord {
        println!();
        println!(
            "The same per labelled chord: each counts once, right when it's right for more than half its scored time:"
        );
        print_measures(&engines, &scores, &song, |s, i| s.per_chord.get(i).copied().unwrap_or_default());
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

    let scored: Vec<naming::Scored> = engines
        .iter()
        .zip(scores)
        .map(|(e, s)| naming::Scored {
            name: &e.name,
            tallies: s.into_iter().map(|x| x.named).collect(),
            lists: e.lists,
        })
        .collect();
    naming::report(&scored, &song);
    Ok(())
}

/// The table of mir_eval's measures, each engine's share right and its range, then each engine against the first.
/// `cell` gives a piece's right and scored amounts for a measure: time, or labelled chords.
fn print_measures(
    engines: &[Engine],
    scores: &[Vec<Score>],
    song: &[usize],
    cell: impl Fn(&Score, usize) -> (f64, f64),
) {
    let columns = |s: &[Score], i: usize| -> (Vec<f64>, Vec<f64>) { s.iter().map(|x| cell(x, i)).unzip() };
    print!("{:<28}", "");
    for m in Measure::ALL {
        print!(" {:>11}", m.name());
    }
    println!();
    for (e, s) in engines.iter().zip(scores) {
        let shares: Vec<_> = (0..Measure::ALL.len())
            .map(|i| {
                let (right, total) = columns(s, i);
                metrics::ratio(&right, &total, song)
            })
            .collect();
        print!("{:<28}", e.name);
        for r in &shares {
            print!(" {:>11}", pct(r.value));
        }
        println!();
        print!("{:<28}", "");
        for r in &shares {
            print!(" {:>11}", format!("{:.1}-{:.1}", 100.0 * r.low, 100.0 * r.high));
        }
        println!();
    }
    if let Some(((first, base), rest)) = engines.iter().zip(scores).collect::<Vec<_>>().split_first()
        && !rest.is_empty()
    {
        println!("Against the {} (points; a range that doesn't cross 0 is a real difference):", first.name);
        for (e, s) in rest {
            let diffs: Vec<(f64, f64, f64)> = (0..Measure::ALL.len())
                .map(|i| {
                    let (a, total) = columns(base, i);
                    let (b, _) = columns(s, i);
                    metrics::paired_ratio(&a, &b, &total, song)
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
            let measures: Vec<(i64, i64)> = score.measures.iter().map(|&(r, t)| (r as i64, t as i64)).collect();
            let line = serde_json::json!({
                "engine": e.name,
                "source": item.source,
                "id": item.id,
                "reference": item.chords,
                "estimate": read.iter().map(|r| (r.start, r.end, Label::write(r.chord))).collect::<Vec<_>>(),
                "names": read.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
                "measures": Measure::ALL.iter().map(|m| m.name()).zip(&measures).collect::<Vec<_>>(),
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
    measures: Vec<(f64, f64)>,
    /// For each of [`Measure::ALL`]: the labelled chords it got right for more than half their scored time, and the
    /// labelled chords scored.
    per_chord: Vec<(f64, f64)>,
    over_lost: i64,
    over_total: i64,
    under_lost: i64,
    under_total: i64,
    /// How many chords it read (chords in a row that are the same counted once).
    changes: usize,
    /// The scores beyond mir_eval's.
    named: Tally,
}

/// `heard` says which parts' notes the engine heard (by role), for the score of roots no part plays.
fn score(item: &Item, read: &[Read], heard: &impl Fn(&str) -> bool) -> Score {
    let reference = reference(item);
    let indexed: Vec<(i64, i64, Option<usize>)> =
        read.iter().enumerate().map(|(i, r)| (r.start, r.end, Some(i))).collect();
    let labels: Vec<Label> = read.iter().map(|r| Label::of_chord(r.chord)).collect();
    let lined = harte::line_up_by(&reference, &indexed, None);
    let mut measures = vec![(0.0, 0.0); Measure::ALL.len()];
    // For each labelled chord and measure, the time right and the time scored.
    let mut by_label = vec![vec![(0i64, 0i64); Measure::ALL.len()]; reference.len()];
    for &(length, r, e) in &lined {
        let (Some(label), Some(cells)) = (reference.get(r), by_label.get_mut(r)) else {
            continue;
        };
        let estimate = e.and_then(|e| labels.get(e)).copied().unwrap_or(Label::NO_CHORD);
        for ((m, total), cell) in Measure::ALL.iter().zip(measures.iter_mut()).zip(cells.iter_mut()) {
            if let Some(right) = m.compare(&label.2, &estimate) {
                total.1 += length as f64;
                cell.1 += length;
                if right {
                    total.0 += length as f64;
                    cell.0 += length;
                }
            }
        }
    }
    let per_chord = (0..Measure::ALL.len())
        .map(|m| {
            by_label
                .iter()
                .filter_map(|cells| cells.get(m))
                .filter(|c| c.1 > 0)
                .fold((0.0, 0.0), |(r, t), c| (r + f64::from(u8::from(2 * c.0 > c.1)), t + 1.0))
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
    let (from, to) = harte::stretch(&reference).unwrap_or_default();
    let whole: Vec<(i64, i64, Option<Chord>)> = read.iter().map(|r| (r.start, r.end, Some(r.chord))).collect();
    let joined_estimate = harte::join(&harte::adjust(from, to, &whole, None));
    let (over_lost, over_total) = harte::hamming_parts(&joined_reference, &joined_estimate);
    let (under_lost, under_total) = harte::hamming_parts(&joined_estimate, &joined_reference);
    let changes = harte::join(&whole).len();

    let named: Vec<(i64, i64, Named)> = item.chords.iter().map(|(s, e, l)| (*s, *e, harte::named(l))).collect();
    let notes = item.notes_of(heard);
    let played = played(&named, &notes);
    let bass_part = item.parts.iter().any(|p| p.role == "bass" && heard(&p.role));
    let named = naming::tally(&named, read, &played, bass_part);
    Score { measures, per_chord, over_lost, over_total, under_lost, under_total, changes, named }
}

/// For each labelled span, the pitch classes of the notes sounding in it. `notes` are in order of start.
fn played<K>(spans: &[(i64, i64, K)], notes: &[item::Note]) -> Vec<PcSet> {
    let mut by_start: Vec<usize> = (0..spans.len()).collect();
    by_start.sort_by_key(|&i| spans.get(i).map(|s| s.0));
    let starts: Vec<i64> = by_start.iter().filter_map(|&i| spans.get(i).map(|s| s.0)).collect();
    let mut out = vec![PcSet::EMPTY; spans.len()];
    // The longest span, so a note's search can start that far back.
    let longest = spans.iter().map(|s| s.1 - s.0).max().unwrap_or(0);
    for n in notes {
        let end = n.start.saturating_add(n.length.max(1));
        let first = starts.partition_point(|&s| s < n.start.saturating_sub(longest));
        for &i in by_start.iter().skip(first) {
            let Some(&(s, e, _)) = spans.get(i) else { continue };
            if s >= end {
                break;
            }
            if e > n.start
                && let Some(set) = out.get_mut(i)
            {
                *set = set.with(PitchClass::of_key(n.key));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notes_played_in_each_labelled_span_are_found() {
        let note = |start, length, key| item::Note { start, length, key, velocity: 90 };
        // C held over both spans, E only in the first, G struck as the second starts, B after both.
        let notes = [note(0, 20, 60), note(2, 3, 64), note(10, 5, 67), note(30, 5, 71)];
        let spans = [(10, 20, ()), (0, 10, ())];
        assert_eq!(played(&spans, &notes), vec![PcSet::of(&[0, 7]), PcSet::of(&[0, 4])]);
    }
}
