//! `theorybench chords`: how often each chord reader names the labelled chord, scored as mir_eval scores chord
//! recognition (each stretch of time weighted by its length, pooled over all the songs), with mir_eval's
//! segmentation score for where the chords change; then the scores the chord reader's design adds ([`naming`]):
//! the whole chord with its extensions, the root among the same notes, the bass kind and the readings listed.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use mo_harmony::{Chord, PcSet, PitchClass};
use rayon::prelude::*;
use serde::Deserialize;

use crate::harte::{self, Label, Measure, Named};
use crate::item::{self, Item};
use crate::metrics::{self, pct};
use crate::naming::{self, Read, Tally};

/// The chords one engine read in one piece, as the C# helper (`cs/`) writes them: start, end, name, root, bass and
/// the pitch classes heard (twelve bits, bit 0 for C), then, when the helper lists them, the other readings of the
/// same notes (name, root, bass, pitch classes), likeliest first.
#[derive(Deserialize)]
struct Answers {
    source: String,
    id: String,
    chords: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Row {
    Listed(i64, i64, String, u8, u8, u16, Vec<(String, u8, u8, u16)>),
    Plain(i64, i64, String, u8, u8, u16),
}

/// One engine's chords for every piece, by source and id.
struct Engine {
    name: String,
    read: HashMap<(String, String), Vec<Read>>,
    /// Whether it lists other readings anywhere.
    lists: bool,
}

/// How many of an engine's chords there are, and of them how many have a name that doesn't read back on the root and
/// bass it gives (so they're scored by the notes heard), for a warning.
#[derive(Default)]
struct Unread {
    chords: usize,
    unread: usize,
    first: Option<String>,
}

/// A chord as the C# app shows it: the notes its name says, on the root and over the bass it gives, and the ones its
/// name stands for without saying they sound ([`naming::may_leave_out`]: the fifth, the 9 under an 11 or 13, the 11 of
/// a minor 13), which count only where the label has them. So "C7" names the G it may not have, "Gsus4" heard with an
/// E leaves the E out, as the page shows them, and "Cm13" is right for a minor 13 with no 11. A name that doesn't
/// read back on that root and bass (none in the bench's sets) falls back to every pitch class heard with the root (the
/// bass among them, as the C# app's names have it: D7/C, never D/C), and nothing left out.
fn cs_chord(name: &str, root: u8, bass: u8, pcs: u16) -> (Chord, PcSet) {
    let root = PitchClass::new(i32::from(root));
    let bass = PitchClass::new(i32::from(bass));
    match read_back(name, root, bass) {
        Some(c) => (c, naming::may_leave_out(c)),
        None => (Chord::new(root, PcSet::from_bits(pcs).with(root), bass), PcSet::EMPTY),
    }
}

/// The C# app's name read with mo-harmony, when it reads back on the root and bass the app gives.
fn read_back(name: &str, root: PitchClass, bass: PitchClass) -> Option<Chord> {
    Chord::parse(name).filter(|c| c.root() == root && c.bass() == bass)
}

fn load(name: &str, path: &Path) -> Result<Engine, String> {
    let mut lists = false;
    let mut unread = Unread::default();
    // The chord an engine shows, counting the names that don't read back.
    let mut shown = |name: &str, root: u8, bass: u8, pcs: u16| {
        let (chord, optional) = cs_chord(name, root, bass, pcs);
        unread.chords += 1;
        if read_back(name, chord.root(), chord.bass()).is_none() {
            unread.unread += 1;
            unread.first.get_or_insert_with(|| name.to_owned());
        }
        (chord, optional)
    };
    let read = item::read_lines::<Answers>(path)?
        .into_iter()
        .map(|a| {
            let chords = a
                .chords
                .into_iter()
                .map(|row| match row {
                    Row::Plain(start, end, name, root, bass, pcs) => {
                        let (chord, optional) = shown(&name, root, bass, pcs);
                        Read { start, end, chord, name, optional, others: Vec::new() }
                    }
                    Row::Listed(start, end, name, root, bass, pcs, others) => {
                        lists = true;
                        let others = others.into_iter().map(|(n, r, b, p)| cs_chord(&n, r, b, p).0).collect();
                        let (chord, optional) = shown(&name, root, bass, pcs);
                        Read { start, end, chord, name, optional, others }
                    }
                })
                .collect();
            ((a.source, a.id), chords)
        })
        .collect();
    if unread.unread > 0 {
        eprintln!(
            "{name}: {} of {} chords have a name that doesn't read back on their root and bass (the first: {}); they are \
             scored by the notes heard",
            unread.unread,
            unread.chords,
            unread.first.unwrap_or_default()
        );
    }
    Ok(Engine { name: name.to_owned(), read, lists })
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut engines = Vec::new();
    let mut dump = None;
    let mut part = None;
    let mut per_chord = false;
    let mut by = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump" => dump = Some(PathBuf::from(args.next().ok_or("--dump needs a file")?)),
            "--part" => part = Some(args.next().ok_or("--part needs a role")?.clone()),
            "--per-segment" => per_chord = true,
            "--by" => by.push(args.next().ok_or("--by needs a tag, such as recipe or texture")?.clone()),
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

    // The parts the engines heard, as the C# helper picks them: those with the role given, or all but drums.
    let heard = |role: &str| match part.as_deref() {
        Some(p) => role == p,
        None => role != "drums",
    };
    let prepared: Vec<Prepared> = items.par_iter().map(|i| prepare(i, &heard)).collect();
    let scores: Vec<Vec<Score>> = engines
        .iter()
        .map(|e| {
            items
                .par_iter()
                .zip(&prepared)
                .map(|(i, p)| score(i, p, e.read.get(&key(i)).map_or(&[][..], Vec::as_slice)))
                .collect()
        })
        .collect();
    let song: Vec<usize> = (0..items.len()).collect();
    if let Some(path) = dump {
        write_dump(&path, &items, &engines, &scores)?;
    }

    println!();
    println!("Share of the time right, each way mir_eval scores it (likely range when the songs are drawn again):");
    print_measures(&engines, &scores, &song, |s, i| {
        s.measures.get(i).map_or((0.0, 0.0), |&(r, t)| (r as f64 / s.ppq, t as f64 / s.ppq))
    });
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
        // In beats, so pieces with other ticks a beat count by their length.
        let sum = |f: fn(&Score) -> i64| s.iter().map(|x| f(x) as f64 / x.ppq).sum::<f64>();
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
    if !by.is_empty() {
        print_by(&by, &items, &prepared, &engines);
    }
    Ok(())
}

/// The chords named whole, split by the tags `convert/voice.py` gives each labelled chord it plays (`--by`): each
/// group is scored on its own labelled chords alone, the rest of each piece left out.
fn print_by(by: &[String], items: &[Item], prepared: &[Prepared], engines: &[Engine]) {
    let groups: Vec<Vec<String>> = items.iter().map(|i| groups_of(i, by)).collect();
    // Each group and the pieces that have it, so a group is tallied over its own pieces only.
    let mut members: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (k, g) in groups.iter().enumerate() {
        for name in g {
            let pieces = members.entry(name.as_str()).or_default();
            if pieces.last() != Some(&k) {
                pieces.push(k);
            }
        }
    }
    let names: Vec<&str> = members.keys().copied().collect();
    let pooled: Vec<Vec<naming::Pooled>> = members
        .par_iter()
        .map(|(&name, pieces)| {
            engines
                .iter()
                .map(|e| {
                    let tallies: Vec<Tally> = pieces
                        .iter()
                        .filter_map(|&k| Some((items.get(k)?, prepared.get(k)?, groups.get(k)?)))
                        .map(|(item, p, g)| {
                            let labels: Vec<(i64, i64, Named)> = p
                                .named
                                .iter()
                                .zip(g)
                                .map(|(&(s, e, n), x)| (s, e, if x == name { n } else { Named::Unknown }))
                                .collect();
                            let read = e.read.get(&key(item)).map_or(&[][..], Vec::as_slice);
                            naming::tally(&labels, read, &p.played, p.bass_part, item.ppq)
                        })
                        .collect();
                    naming::pool(&tallies)
                })
                .collect()
        })
        .collect();
    let total: Vec<f64> =
        (0..engines.len()).map(|k| pooled.iter().filter_map(|row| row.get(k)).map(|p| p.time).sum()).collect();
    let cell = |x: Option<f64>| x.map_or("-".to_owned(), pct);
    println!();
    println!(
        "The chords named whole, by {} (each group's share of the chord time; kind: the bass kind right; ext F1: the \
         macro F1 of the extensions its labels have):",
        by.join(" and ")
    );
    print!("{:<40}", "");
    for h in ["time", "notes+bass", "name", "event", "lenient", "kind", "ext F1"] {
        print!(" {h:>10}");
    }
    println!();
    for (name, row) in names.iter().zip(&pooled) {
        for (k, (e, p)) in engines.iter().zip(row).enumerate() {
            let label = match (engines.len(), k) {
                (1, _) => (*name).to_owned(),
                (_, 0) => format!("{name}: {}", e.name),
                _ => format!("  {}", e.name),
            };
            print!(
                "{label:<40} {:>10}",
                total.get(k).filter(|&&t| t > 0.0).map_or("-".to_owned(), |t| pct(p.time / t))
            );
            for s in p.scores {
                print!(" {:>10}", cell(s));
            }
            println!(" {:>10} {:>10}", cell(p.kind_right), cell(p.macro_f1));
        }
    }
}

/// Each labelled chord's group for `--by`: the values of the tags asked for, joined by " / " ("-" for a tag it
/// hasn't, as in a piece voice.py didn't play).
fn groups_of(item: &Item, by: &[String]) -> Vec<String> {
    item.chords
        .iter()
        .enumerate()
        .map(|(i, &(s, e, _))| {
            let tags = item
                .render
                .get(i)
                .filter(|r| r.0 == s && r.1 == e)
                .or_else(|| item.render.iter().find(|r| r.0 == s && r.1 == e))
                .map(|r| &r.2);
            by.iter().map(|b| tags.and_then(|t| t.get(b)).map_or("-", String::as_str)).collect::<Vec<_>>().join(" / ")
        })
        .collect()
}

/// The table of mir_eval's measures, each engine's share right and its range, then each engine against the first.
/// `cell` gives a piece's right and scored amounts for a measure: time in beats, or labelled chords.
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
        println!();
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
            let line = serde_json::json!({
                "engine": e.name,
                "source": item.source,
                "id": item.id,
                "ppq": item.ppq,
                "reference": item.chords,
                "estimate": read.iter().map(|r| (r.start, r.end, Label::write(r.chord))).collect::<Vec<_>>(),
                "names": read.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
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

/// What every engine's score on a piece needs, read once: its labels read whole, the pitch classes sounding in each
/// in the parts heard, and whether those parts have a bass part that plays.
struct Prepared {
    named: Vec<(i64, i64, Named)>,
    played: Vec<PcSet>,
    bass_part: bool,
}

fn prepare(item: &Item, heard: &impl Fn(&str) -> bool) -> Prepared {
    let named: Vec<(i64, i64, Named)> = item.chords.iter().map(|(s, e, l)| (*s, *e, harte::named(l))).collect();
    let played = played(&named, &item.notes_of(heard));
    let bass_part = item.parts.iter().any(|p| p.role == "bass" && heard(&p.role) && !p.notes.is_empty());
    Prepared { named, played, bass_part }
}

/// How one engine did on one piece.
struct Score {
    /// Ticks a beat: the times below are in ticks (as `--dump` writes them), and pooled in beats.
    ppq: f64,
    /// For each of [`Measure::ALL`]: the time it got right, and the time scored.
    measures: Vec<(i64, i64)>,
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

fn score(item: &Item, prepared: &Prepared, read: &[Read]) -> Score {
    let reference = reference(item);
    let indexed: Vec<(i64, i64, Option<usize>)> =
        read.iter().enumerate().map(|(i, r)| (r.start, r.end, Some(i))).collect();
    let labels: Vec<Label> = read.iter().map(|r| Label::of_chord(r.chord)).collect();
    let lined = harte::line_up_by(&reference, &indexed, None);
    let mut measures = vec![(0, 0); Measure::ALL.len()];
    // For each labelled chord and measure, the time right and the time scored.
    let mut by_label = vec![vec![(0i64, 0i64); Measure::ALL.len()]; reference.len()];
    for &(length, r, e) in &lined {
        let (Some(label), Some(cells)) = (reference.get(r), by_label.get_mut(r)) else {
            continue;
        };
        let estimate = e.and_then(|e| labels.get(e)).copied().unwrap_or(Label::NO_CHORD);
        for ((m, total), cell) in Measure::ALL.iter().zip(measures.iter_mut()).zip(cells.iter_mut()) {
            if let Some(right) = m.compare(&label.2, &estimate) {
                total.1 += length;
                cell.1 += length;
                if right {
                    total.0 += length;
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

    let named = naming::tally(&prepared.named, read, &prepared.played, prepared.bass_part, item.ppq);
    let ppq = f64::from(item.ppq.max(1));
    Score { ppq, measures, per_chord, over_lost, over_total, under_lost, under_total, changes, named }
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
    fn the_c_sharp_apps_chords_are_the_notes_its_names_say() {
        let notes = |c: Chord| c.notes().iter().map(|p| i32::from(p.number())).collect::<Vec<_>>();
        let left_out =
            |(_, optional): (Chord, PcSet)| optional.iter().map(|p| i32::from(p.number())).collect::<Vec<_>>();
        // C7 heard as C E Bb names the G (which it may leave out); Gsus4 heard as G C E (C/G) names the D (its fifth,
        // which it may leave out) and leaves out the E.
        assert_eq!(notes(cs_chord("C7", 0, 0, 0b0100_0001_0001).0), vec![0, 4, 7, 10]);
        assert_eq!(left_out(cs_chord("C7", 0, 0, 0b0100_0001_0001)), vec![7]);
        assert_eq!(notes(cs_chord("Gsus4", 7, 7, 0b0000_1001_0001).0), vec![0, 2, 7]);
        assert_eq!(left_out(cs_chord("Gsus4", 7, 7, 0b0000_1001_0001)), vec![2]);
        // The bass is one of its notes, as its names have it.
        let (d7, _) = cs_chord("D7/C", 2, 0, 0b0010_0100_0101);
        assert_eq!((notes(d7), d7.bass().number()), (vec![0, 2, 6, 9], 0));
        // A minor 13 stands for its 9 and 11 and its fifth.
        assert_eq!(left_out(cs_chord("Cm13", 0, 0, 0b0110_1000_1001)), vec![2, 5, 7]);
        // A name that doesn't read back on its root keeps what was heard, with nothing left out.
        assert_eq!(notes(cs_chord("Q7", 0, 0, 0b0100_0001_0001).0), vec![0, 4, 10]);
        assert_eq!(
            cs_chord("D7", 0, 0, 0b0100_0001_0001),
            (Chord::new(PitchClass::C, PcSet::of(&[0, 4, 10]), PitchClass::C), PcSet::EMPTY)
        );
    }

    #[test]
    fn by_groups_each_labelled_chord_by_the_tags_voice_py_gave_it() {
        let item: Item = serde_json::from_str(
            r#"{"source":"x","id":"1","ppq":4,"length":24,"parts":[],
                "chords":[[0,8,"C:maj"],[8,16,"G:7"],[16,24,"C:maj"]],
                "render":[[0,8,{"texture":"held","recipe":"close"}],[16,24,{"texture":"walking"}],
                          [8,16,{"texture":"pedal","recipe":"shell"}]]}"#,
        )
        .unwrap();
        // The tags are found by the chord's span when they aren't at its place; a tag a chord hasn't is "-".
        let by = |tags: &[&str]| groups_of(&item, &tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>());
        assert_eq!(by(&["texture"]), ["held", "pedal", "walking"]);
        assert_eq!(by(&["texture", "recipe"]), ["held / close", "pedal / shell", "walking / -"]);
        // A piece voice.py didn't play has no tags at all.
        let plain = Item { render: Vec::new(), ..item.clone() };
        assert_eq!(groups_of(&plain, &["texture".to_owned()]), ["-", "-", "-"]);
    }

    #[test]
    fn only_a_bass_part_that_plays_counts_as_one() {
        let item = |bass: &str| -> Item {
            serde_json::from_str(&format!(
                r#"{{"source":"x","id":"1","ppq":4,"length":8,"chords":[[0,8,"C:maj"]],
                    "parts":[{{"role":"chords","notes":[[0,8,64,90]]}},{{"role":"bass","notes":[{bass}]}}]}}"#
            ))
            .unwrap()
        };
        let all = |_: &str| true;
        assert!(prepare(&item("[0,8,36,90]"), &all).bass_part);
        assert!(!prepare(&item(""), &all).bass_part);
        assert!(!prepare(&item("[0,8,36,90]"), &|role: &str| role != "bass").bass_part);
    }

    #[test]
    fn the_notes_played_in_each_labelled_span_are_found() {
        let note = |start, length, key| item::Note { start, length, key, velocity: 90 };
        // C held over both spans, E only in the first, G struck as the second starts, B after both.
        let notes = [note(0, 20, 60), note(2, 3, 64), note(10, 5, 67), note(30, 5, 71)];
        let spans = [(10, 20, ()), (0, 10, ())];
        assert_eq!(played(&spans, &notes), vec![PcSet::of(&[0, 7]), PcSet::of(&[0, 4])]);
    }
}
