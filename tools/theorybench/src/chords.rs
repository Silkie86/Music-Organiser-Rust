//! `theorybench chords`: how often each chord reader names the labelled chord, scored as mir_eval scores chord
//! recognition (each stretch of time weighted by its length, pooled over all the songs), with mir_eval's
//! segmentation score for where the chords change; then the scores the chord reader's design adds ([`naming`]):
//! the whole chord with its extensions, the root among the same notes, the bass kind and the readings listed.
//!
//! The C# app's chords come from files its helper (`cs/`) writes; mo-harmony's chord reader runs here
//! (`--engine rust`), on the same notes.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mo_harmony::chordread::{Model, SHIPPED};
use mo_harmony::{Chord, ChordContext, ChordReader, PcSet, Pin, PitchClass, Role};
use rayon::prelude::*;
use serde::Deserialize;

use crate::harte::{self, Label, Measure, Named};
use crate::item::{self, Item};
use crate::keys::{self, Parts, SongNotes};
use crate::metrics::{self, pct};
use crate::naming::{self, Read, Tally};
use crate::{chordfit, keyfit};

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

/// A chord as the C# app shows it: the notes its name says, on the root and over the bass it gives, and the ones its
/// name stands for without saying they sound ([`naming::may_leave_out`]: the fifth, the 9 under an 11 or 13, the 11 of
/// a minor 13), which count only where the label has them. So "C7" names the G it may not have, "Gsus4" heard with an
/// E leaves the E out, as the page shows them, and "Cm13" is right for a minor 13 with no 11. A name that doesn't
/// read back on that root and bass (none in the bench's sets) falls back to every pitch class heard with the root (the
/// bass among them, as the C# app's names have it: D7/C, never D/C), and nothing left out.
fn cs_chord(name: &str, root: u8, bass: u8, pcs: u16) -> (Chord, PcSet) {
    let root = PitchClass::new(i32::from(root));
    let bass = PitchClass::new(i32::from(bass));
    by_name(name, root, bass).unwrap_or((Chord::new(root, PcSet::from_bits(pcs).with(root), bass), PcSet::EMPTY))
}

/// A chord as mo-harmony's chart shows it, scored as the C# app's are ([`cs_chord`]): by its name, as mo-harmony's
/// names stand for notes too (its `every_note_is_in_the_name` test). So a shell voiced C E Bb, shown as "C7", is right
/// for a C7 label. A name that doesn't read back (none should) falls back to the notes read, nothing left out.
fn shown_by_name(name: &str, read: Chord) -> (Chord, PcSet) {
    by_name(name, read.root(), read.bass()).unwrap_or((read, PcSet::EMPTY))
}

/// How a chord shown by its name is scored, when the name reads back on the root and bass given: the notes it says,
/// and the ones it stands for without saying they sound, which count only where the label has them.
fn by_name(name: &str, root: PitchClass, bass: PitchClass) -> Option<(Chord, PcSet)> {
    read_back(name, root, bass).map(|c| (c, naming::may_leave_out(c)))
}

/// An engine's name read with mo-harmony, when it reads back on the root and bass the engine gives.
fn read_back(name: &str, root: PitchClass, bass: PitchClass) -> Option<Chord> {
    Chord::parse(name).filter(|c| c.root() == root && c.bass() == bass)
}

/// Warns of an engine's chords whose name doesn't read back on their root and bass: they're scored by the notes heard.
fn warn_unread(engine: &Engine) {
    let all = || engine.read.values().flatten();
    let unread: Vec<&str> = all()
        .filter(|r| read_back(&r.name, r.chord.root(), r.chord.bass()).is_none())
        .map(|r| r.name.as_str())
        .collect();
    if let Some(first) = unread.iter().min() {
        eprintln!(
            "{}: {} of {} chords have a name that doesn't read back on their root and bass (such as {first}); they \
             are scored by the notes heard",
            engine.name,
            unread.len(),
            all().count()
        );
    }
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
                        let (chord, optional) = cs_chord(&name, root, bass, pcs);
                        Read { start, end, chord, name, optional, others: Vec::new() }
                    }
                    Row::Listed(start, end, name, root, bass, pcs, others) => {
                        lists = true;
                        let others = others.into_iter().map(|(n, r, b, p)| cs_chord(&n, r, b, p).0).collect();
                        let (chord, optional) = cs_chord(&name, root, bass, pcs);
                        Read { start, end, chord, name, optional, others }
                    }
                })
                .collect();
            ((a.source, a.id), chords)
        })
        .collect();
    let engine = Engine { name: name.to_owned(), read, lists };
    warn_unread(&engine);
    Ok(engine)
}

/// How the bench runs mo-harmony's chord reader (`--engine rust`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RustRun {
    /// Every part heard as one part with no role (`--roles off`), as the C# helper hears them; else each part with the
    /// role its dataset gives.
    roles_off: bool,
    /// With the labelled changes given (`--oracle-cuts`): a pin at each labelled chord's ends, so each is read as one
    /// chord, whichever reads best (the C# helper's `--oracle-cuts` does the same for its answers).
    oracle: bool,
}

impl RustRun {
    /// The engine's name in the tables, which keep it short; [`RustRun::how`] says how it ran.
    const NAME: &'static str = "mo-harmony";

    /// How it was run, for a line above the tables.
    fn how(self) -> String {
        format!(
            "{} hears {}{}.",
            RustRun::NAME,
            if self.roles_off {
                "every part as one, with no role, as the C# app does"
            } else {
                "each part with its role"
            },
            if self.oracle { ", given the labelled changes" } else { "" }
        )
    }
}

/// The readings mo-harmony keeps for each stretch: the first, and two more for the lists' top 3.
const READINGS: u8 = 3;

/// An engine asked for: an engine's answers read from a file (the C# app's, from its helper), or mo-harmony, run here.
#[derive(Debug, PartialEq)]
enum EngineArg {
    Answers { name: String, path: PathBuf },
    Rust,
}

/// What `chords` is asked to do.
#[derive(Debug, Default, PartialEq)]
struct Options {
    files: Vec<PathBuf>,
    /// The engines in the order given: the first is the baseline.
    engines: Vec<EngineArg>,
    dump: Option<PathBuf>,
    part: Option<String>,
    per_chord: bool,
    by: Vec<String>,
    rust: RustRun,
    timing: bool,
    /// The weights a fit wrote, for mo-harmony to read with (`--weights`); the hand-set ones when none is given.
    weights: Option<PathBuf>,
    /// Only the pieces in these folds, as the chord fit draws them (`--folds`).
    folds: Option<Vec<usize>>,
    /// Whether an option only mo-harmony takes was given (`--roles`, `--oracle-cuts`, `--timing`, `--weights`).
    rust_only: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Options, String> {
        let mut o = Options::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dump" => o.dump = Some(PathBuf::from(args.next().ok_or("--dump needs a file")?)),
                "--part" => o.part = Some(args.next().ok_or("--part needs a role")?.clone()),
                "--per-segment" => o.per_chord = true,
                "--by" => o.by.push(args.next().ok_or("--by needs a tag, such as recipe or texture")?.clone()),
                "--answers" => {
                    let spec = args.next().ok_or("--answers needs name=file")?;
                    let (name, path) = spec.split_once('=').ok_or("--answers needs name=file")?;
                    o.engines.push(EngineArg::Answers { name: name.to_owned(), path: PathBuf::from(path) });
                }
                "--engine" => match args.next().map(String::as_str) {
                    Some("rust") if !o.engines.contains(&EngineArg::Rust) => o.engines.push(EngineArg::Rust),
                    Some("rust") => return Err("--engine rust is given twice".to_owned()),
                    _ => return Err("--engine takes rust (mo-harmony's chord reader, run here)".to_owned()),
                },
                "--roles" => {
                    o.rust_only = true;
                    match args.next().map(String::as_str) {
                        Some("off") => o.rust.roles_off = true,
                        Some("on") => o.rust.roles_off = false,
                        _ => return Err("--roles takes on or off".to_owned()),
                    }
                }
                "--oracle-cuts" => (o.rust_only, o.rust.oracle) = (true, true),
                "--timing" => (o.rust_only, o.timing) = (true, true),
                "--weights" => {
                    o.rust_only = true;
                    o.weights = Some(PathBuf::from(args.next().ok_or("--weights needs a file chordfit wrote")?));
                }
                "--folds" => {
                    let list = args.next().ok_or("--folds needs folds, such as 1")?;
                    o.folds = Some(chordfit::folds_in(list).map_err(|e| format!("--folds: {e}"))?);
                }
                a if a.starts_with("--") => return Err(format!("Unknown option {a}.\n\n{}", crate::USAGE)),
                a => o.files.push(PathBuf::from(a)),
            }
        }
        if o.files.is_empty() || o.engines.is_empty() {
            return Err(crate::USAGE.to_owned());
        }
        if o.rust_only && !o.engines.contains(&EngineArg::Rust) {
            return Err("--roles, --oracle-cuts, --timing and --weights are for --engine rust".to_owned());
        }
        Ok(o)
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let Options { files, engines, dump, part, per_chord, by, rust, timing, weights: weights_path, folds, .. } =
        Options::parse(args)?;
    let weights = weights_path.as_deref().map(chordfit::load_weights).transpose()?;
    // The answers read from files, read before the music so a wrong file stops the bench at once; mo-harmony (None
    // here) reads once the music is.
    let engines: Vec<Option<Engine>> = engines
        .iter()
        .map(|e| match e {
            EngineArg::Answers { name, path } => load(name, path).map(Some),
            EngineArg::Rust => Ok(None),
        })
        .collect::<Result<_, _>>()?;
    let mut items = Vec::new();
    for f in &files {
        items.extend(item::read(f)?);
    }
    let read = items.len();
    items.retain(|i| !i.chords.is_empty());
    if let Some(folds) = &folds {
        // Drawn over every piece read, so a chart's renders go where the chart goes.
        let drawn = keyfit::drawn_folds(&items);
        let mut drawn = drawn.into_iter();
        items.retain(|_| drawn.next().is_some_and(|f| folds.contains(&f)));
        println!("Only the {} pieces in folds {folds:?}, as the chord fit draws them.", items.len());
    }
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
    if engines.iter().any(Option::is_none) {
        println!("{}", rust.how());
        if let Some(path) = &weights_path {
            println!("It reads with the weights in {}.", path.display());
        }
    }

    // The parts the engines heard, as the C# helper picks them: those with the role given, or all but drums.
    let parts = part.map_or(Parts::Pitched, Parts::Role);
    let heard = |role: &str| parts.keeps(role);
    let prepared: Vec<Prepared> = items.par_iter().map(|i| prepare(i, &heard)).collect();
    let engines: Vec<Engine> = engines
        .into_iter()
        .map(|e| match e {
            Some(answers) => Ok(answers),
            None => {
                let model = Model { weights: weights.as_ref().unwrap_or(SHIPPED.weights), ..SHIPPED };
                let (engine, times) = read_rust(&items, &parts, rust, &model, timing)?;
                if timing {
                    print_timing(&items, &times);
                }
                Ok(engine)
            }
        })
        .collect::<Result<_, String>>()?;
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

/// How long each piece's read took, none where nothing was heard.
type Times = Vec<Option<Duration>>;

/// mo-harmony's chord reader run on every piece: the parts heard (`parts`, as the C# helper heard them), read as the
/// piece is heard whole (a song, a loop or a window), each stretch's chord as the chart shows it and its other readings
/// as the lists; and how long each read took. With `one_core`, on one core (at low priority, as every thread the bench
/// runs: rule 21), one piece after another, so the times are a read's own.
fn read_rust(
    items: &[Item],
    parts: &Parts,
    run: RustRun,
    model: &Model,
    one_core: bool,
) -> Result<(Engine, Times), String> {
    let read_all = || -> Vec<(Vec<Read>, Option<Duration>)> {
        items
            .par_iter()
            .map_init(ChordReader::new, |reader, item| read_piece(reader, item, parts, run, model))
            .collect()
    };
    let read = if one_core {
        let pool = crate::at_low_priority(rayon::ThreadPoolBuilder::new().num_threads(1)).build();
        pool.map_err(|e| format!("--timing needs a thread of its own: {e}"))?.install(read_all)
    } else {
        read_all()
    };
    let times = read.iter().map(|(_, took)| *took).collect();
    let read = items.iter().zip(read).map(|(item, (chords, _))| (key(item), chords)).collect();
    let engine = Engine { name: RustRun::NAME.to_owned(), read, lists: true };
    warn_unread(&engine);
    Ok((engine, times))
}

/// One piece read by mo-harmony with a model (its weights, tables and thresholds), and how long the read took (the
/// notes made ready and the chart spelled left out); none when nothing was heard.
fn read_piece(
    reader: &mut ChordReader,
    item: &Item,
    parts: &Parts,
    run: RustRun,
    model: &Model,
) -> (Vec<Read>, Option<Duration>) {
    let heard = parts_heard(item, parts, run.roles_off);
    let heard: Vec<mo_harmony::Part> =
        heard.iter().map(|(role, notes)| mo_harmony::Part { role: *role, notes }).collect();
    let pins = pins_of(item, run.oracle);
    let cx = ChordContext { span: item.whole.span(), pins: &pins, readings: READINGS };
    let started = Instant::now();
    let reading = reader.read_with(&heard, keys::grid_of(item, false), &cx, model);
    let took = started.elapsed();
    let Some(reading) = reading else { return (Vec::new(), None) };
    let chart = reading.names(None);
    let chords = reading
        .spans
        .iter()
        .zip(&chart)
        .filter_map(|(span, name)| {
            let first = span.shown()?;
            let name = name.map_or_else(|| first.chord.name(), |n| n.name());
            let (chord, optional) = shown_by_name(&name, first.chord);
            let others = span.readings.iter().skip(1).map(|r| shown_by_name(&r.chord.name(), r.chord).0).collect();
            Some(Read { start: span.start, end: span.end, chord, name, optional, others })
        })
        .collect();
    (chords, Some(took))
}

/// The parts mo-harmony hears in a piece (those `parts` keeps), each with the role its dataset gives; or with
/// `roles_off`, all of them as one part with no role, as the C# app hears them.
fn parts_heard(item: &Item, parts: &Parts, roles_off: bool) -> Vec<(Role, Vec<mo_harmony::Note>)> {
    let song = SongNotes::of(item, parts);
    if roles_off { vec![(Role::Unknown, keys::harmony_notes(&song.merged))] } else { song.by_part() }
}

/// With the changes given (`oracle`), a pin at each labelled chord's ends (no chord labelled ones too: where they
/// start and end is given as well); else none.
fn pins_of(item: &Item, oracle: bool) -> Vec<Pin> {
    if !oracle {
        return Vec::new();
    }
    item.chords.iter().map(|&(start, end, _)| Pin { start, end, chord: None }).collect()
}

/// How long mo-harmony took to read each piece, on one core: in all, and for 16 bars of music (the design's budget:
/// 3 ms for a dense 16-bar part, 99 times in 100). A piece where nothing was heard is left out: there was nothing to
/// read.
fn print_timing(items: &[Item], times: &[Option<Duration>]) {
    let read: Vec<(&Item, f64)> =
        items.iter().zip(times).filter_map(|(item, d)| Some((item, d.as_ref()?.as_secs_f64() * 1e3))).collect();
    let mut whole: Vec<f64> = read.iter().map(|&(_, ms)| ms).collect();
    let mut per_16: Vec<f64> = read
        .iter()
        .map(|&(item, ms)| (item.length as f64 / item.bar() as f64, ms))
        .filter(|&(bars, _)| bars > 0.0)
        .map(|(bars, ms)| ms * 16.0 / bars)
        .collect();
    let total: f64 = whole.iter().sum();
    let silent = times.len() - read.len();
    println!();
    println!(
        "mo-harmony read {} pieces in {:.2} s on one core{}.",
        read.len(),
        total / 1e3,
        if silent > 0 { format!(" ({silent} with nothing heard left out)") } else { String::new() }
    );
    println!("{:<28} {:>9} {:>9} {:>9} {:>9}", "ms a read", "middle", "9 in 10", "99 in 100", "longest");
    for (what, values) in [("each piece", &mut whole), ("16 bars of it", &mut per_16)] {
        values.sort_by(f64::total_cmp);
        let at = |p: f64| quantile(values, p).map_or("-".to_owned(), |v| format!("{v:.2}"));
        println!("{what:<28} {:>9} {:>9} {:>9} {:>9}", at(0.5), at(0.9), at(0.99), at(1.0));
    }
}

/// The value a share `p` of sorted values are at or under (the nearest rank), none for none.
fn quantile(sorted: &[f64], p: f64) -> Option<f64> {
    let n = sorted.len();
    let rank = ((p * n as f64).ceil() as usize).clamp(1, n.max(1));
    sorted.get(rank - 1).copied()
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

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn the_options_say_which_engines_run_and_how() {
        let parse = |a: &[&str]| Options::parse(&args(a));
        let all = parse(&[
            "a.jsonl",
            "--answers",
            "C# app=cs.jsonl",
            "--engine",
            "rust",
            "--roles",
            "off",
            "--oracle-cuts",
            "--timing",
        ])
        .unwrap();
        assert_eq!(all.files, [PathBuf::from("a.jsonl")]);
        assert_eq!(
            all.engines,
            [EngineArg::Answers { name: "C# app".to_owned(), path: PathBuf::from("cs.jsonl") }, EngineArg::Rust]
        );
        assert_eq!(all.rust, RustRun { roles_off: true, oracle: true });
        assert!(all.timing);
        // mo-harmony alone is an engine like any other (the baseline when it's given first), its roles on unless
        // they're turned off.
        let alone = parse(&["a.jsonl", "--engine", "rust", "--roles", "off", "--roles", "on"]).unwrap();
        assert_eq!((alone.engines, alone.rust), (vec![EngineArg::Rust], RustRun::default()));
        // What's wrong is said.
        let wrong = |a: &[&str]| parse(a).unwrap_err();
        assert_eq!(wrong(&["a.jsonl", "--engine", "rust", "--engine", "rust"]), "--engine rust is given twice");
        assert!(wrong(&["a.jsonl", "--engine", "python"]).starts_with("--engine takes rust"));
        assert!(wrong(&["a.jsonl", "--engine"]).starts_with("--engine takes rust"));
        assert_eq!(wrong(&["a.jsonl", "--engine", "rust", "--roles", "maybe"]), "--roles takes on or off");
        for rust_only in
            [&["--roles", "off"][..], &["--roles", "on"], &["--oracle-cuts"], &["--timing"], &["--weights", "w.json"]]
        {
            let a = [&["a.jsonl", "--answers", "C# app=cs.jsonl"][..], rust_only].concat();
            assert_eq!(wrong(&a), "--roles, --oracle-cuts, --timing and --weights are for --engine rust");
        }
        // Weights a fit wrote, and only some folds.
        let fitted = parse(&["a.jsonl", "--engine", "rust", "--weights", "w.json", "--folds", "1,2"]).unwrap();
        assert_eq!((fitted.weights, fitted.folds), (Some(PathBuf::from("w.json")), Some(vec![1, 2])));
        assert_eq!(
            wrong(&["a.jsonl", "--engine", "rust", "--folds", "0"]),
            "--folds: fold 0 is kept back for the final look"
        );
        assert_eq!(wrong(&["a.jsonl", "--engine", "rust", "--folds", "x"]), "--folds: x is no fold (1 to 4)");
        // No engine or no music: how to use it.
        assert_eq!(wrong(&["a.jsonl"]), crate::USAGE);
        assert_eq!(wrong(&["--engine", "rust"]), crate::USAGE);
    }

    #[test]
    fn how_mo_harmony_ran_is_said_once() {
        let how = |roles_off, oracle| RustRun { roles_off, oracle }.how();
        assert_eq!(how(false, false), "mo-harmony hears each part with its role.");
        assert_eq!(
            how(true, true),
            "mo-harmony hears every part as one, with no role, as the C# app does, given the labelled changes."
        );
        // Its name fits the tables' first column with " precision" or " - C# app" after it.
        assert!(RustRun::NAME.len() + " precision".len() <= 28);
    }

    /// Block chords at 96 ticks a beat, a bar each: (keys played, the bass, the label).
    fn block_chords(bars: &[(&[u8], u8, &str)]) -> Item {
        let bar = 384;
        let mut chords = Vec::new();
        let mut bass = Vec::new();
        let mut labels = Vec::new();
        for (k, (keys, low, label)) in (0_i64..).zip(bars) {
            chords.extend(keys.iter().map(|key| format!("[{},{bar},{key},90]", k * bar)));
            bass.push(format!("[{},{bar},{low},90]", k * bar));
            labels.push(format!("[{},{},\"{label}\"]", k * bar, (k + 1) * bar));
        }
        serde_json::from_str(&format!(
            r#"{{"source":"t","id":"1","ppq":96,"length":{},"chords":[{}],
                "parts":[{{"role":"chords","notes":[{}]}},{{"role":"bass","notes":[{}]}}]}}"#,
            bars.len() as i64 * bar,
            labels.join(","),
            chords.join(","),
            bass.join(",")
        ))
        .unwrap()
    }

    #[test]
    fn mo_harmony_reads_each_stretch_as_its_chart_shows_it() {
        let item = block_chords(&[
            (&[60, 64, 67], 48, "C:maj"),
            (&[59, 62, 65, 67], 43, "G:7"),
            (&[57, 60, 64, 67], 45, "A:min7"),
            // A shell: C, E and Bb, no fifth.
            (&[64, 70], 48, "C:7"),
            (&[60, 65, 69], 41, "F:maj"),
        ]);
        let (read, _) = read_piece(&mut ChordReader::new(), &item, &Parts::Pitched, RustRun::default(), &SHIPPED);
        let rows: Vec<(i64, i64, &str)> = read.iter().map(|r| (r.start, r.end, r.name.as_str())).collect();
        assert_eq!(rows, [(0, 384, "C"), (384, 768, "G7"), (768, 1152, "Am7"), (1152, 1536, "C7"), (1536, 1920, "F")]);
        for r in &read {
            // Each is scored by the name the chart shows, as the C# app's are: the notes it says, those it stands for
            // without saying they sound counting only where the label has them. The other readings are the lists'
            // next two, each another chord.
            assert_eq!(Chord::parse(&r.name), Some(r.chord), "{}", r.name);
            assert_eq!(r.optional, naming::may_leave_out(r.chord), "{}", r.name);
            assert!(r.others.len() < usize::from(READINGS) && !r.others.contains(&r.chord), "{}", r.name);
            // The other readings are scored by their names too.
            assert!(r.others.iter().all(|o| Chord::parse(&o.name()) == Some(*o)), "{:?}", r.others);
        }
        assert!(read.iter().any(|r| !r.others.is_empty()));
        // The shell's C7 stands for the G it hasn't.
        let shell = read.get(3).unwrap();
        assert_eq!((shell.chord.notes(), shell.optional), (PcSet::of(&[0, 4, 7, 10]), PcSet::of(&[7])));
        // A piece with no notes heard reads nothing.
        let (none, took) =
            read_piece(&mut ChordReader::new(), &item, &Parts::Role("tune".to_owned()), RustRun::default(), &SHIPPED);
        assert!(none.is_empty() && took.is_none());
    }

    #[test]
    fn the_changes_given_cut_a_held_chord_where_the_labels_change() {
        // C held for two bars, labelled as two chords: read whole, it's one C; given the changes, a chord starts
        // where the second label does.
        let item = block_chords(&[(&[60, 64, 67], 48, "C:maj"), (&[60, 64, 67], 48, "A:min")]);
        let ends = |oracle| {
            let run = RustRun { oracle, ..RustRun::default() };
            let (read, _) = read_piece(&mut ChordReader::new(), &item, &Parts::Pitched, run, &SHIPPED);
            read.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>()
        };
        assert_eq!(ends(false), [(0, 768)]);
        assert_eq!(ends(true), [(0, 384), (384, 768)]);
        assert_eq!(
            pins_of(&item, true),
            [Pin { start: 0, end: 384, chord: None }, Pin { start: 384, end: 768, chord: None }]
        );
        assert!(pins_of(&item, false).is_empty());
    }

    #[test]
    fn the_parts_heard_keep_their_roles_unless_roles_are_off() {
        let item: Item = serde_json::from_str(
            r#"{"source":"x","id":"1","ppq":4,"length":8,"chords":[[0,8,"C:maj"]],
                "parts":[{"role":"chords","notes":[[0,8,64,90],[0,8,60,90]]},{"role":"bass","notes":[[0,8,36,90]]},
                         {"role":"melody","notes":[[2,2,72,90]]},{"role":"drums","notes":[[0,1,36,90]]}]}"#,
        )
        .unwrap();
        let heard = |parts: &Parts, roles_off| -> Vec<(Role, Vec<u8>)> {
            parts_heard(&item, parts, roles_off)
                .into_iter()
                .map(|(role, notes)| (role, notes.iter().map(|n| n.key).collect()))
                .collect()
        };
        // Each part with its role (the drums never heard), its notes in order.
        assert_eq!(
            heard(&Parts::Pitched, false),
            [(Role::Harmony, vec![60, 64]), (Role::Bass, vec![36]), (Role::Melody, vec![72])]
        );
        // With the roles off, every note heard as one part, as the C# app hears them.
        assert_eq!(heard(&Parts::Pitched, true), [(Role::Unknown, vec![36, 60, 64, 72])]);
        // Only the parts asked for.
        assert_eq!(heard(&Parts::Role("bass".to_owned()), false), [(Role::Bass, vec![36])]);
    }

    #[test]
    fn a_share_of_the_times_is_the_nearest_rank() {
        let times: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(quantile(&times, 0.5), Some(5.0));
        assert_eq!(quantile(&times, 0.9), Some(9.0));
        assert_eq!(quantile(&times, 0.99), Some(10.0));
        assert_eq!(quantile(&times, 1.0), Some(10.0));
        assert_eq!(quantile(&times, 0.0), Some(1.0));
        assert_eq!(quantile(&[2.5], 0.99), Some(2.5));
        assert_eq!(quantile(&[], 0.5), None);
    }
}
