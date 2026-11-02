//! `theorybench chordfit`: fits the chord reader's 39 numbers (the weight of each piece of evidence, the trusts in
//! the two template priors and the pair term, and w_N) on labelled music, keeping every rule the weights keep
//! (`chordread::RULES`), and measures the fit on pieces it never learned from (harmony/design/chord-reader-design.md,
//! "Model, training and data").
//!
//! - **What it learns from.** The pieces whose licence lets fitted numbers ship ([`Item::may_learn`]), in the folds
//!   `--train` names (2, 3 and 4 unless it says otherwise). The folds are drawn from each piece's name, so a chart's
//!   renders share its fold whichever set they're in ([`keyfit::drawn_folds`]); fold 0 is kept back for the final
//!   look, and the folds `--test` names (1) are measured, the pieces only measured among them.
//! - **How each is heard.** A piece `convert/voice.py` played as given, each part with its role. A dataset's song
//!   (POP909-CL) four ways (`--inputs`), all from its own piano's notes: the piano with its role; with the tune; with
//!   the piano's bass line (each lowest note with no other within a fifth above it, the reader's own test) as a bass
//!   part; and the rest of the piano alone, with no bass, where the roots often aren't played.
//! - **What a label accepts** ([`accepts`]): its root and core. POP909-CL labels a template's triads while its piano
//!   often plays more, so there a major label also accepts a maj7 or 6, a minor one an m7 or m6, and a sus4 a 7sus4.
//!   A dominant 11 accepts the 9sus4 it's voiced as too, and a diminished seventh its four roots (an augmented triad
//!   its three). No chord, a label that can't be read, or a chord the reader can't read (a root alone) accepts any
//!   reading.
//! - **The aim.** The chance the reader gives the readings the labels allow, over every way it can read each piece:
//!   ln Z over the allowed ways less ln Z over every way (`chordread::ChordFitter`), summed over the pieces each
//!   weighed by `--weigh` and taken per bar, less a pull towards the numbers it starts from (`--lambda`). A way is
//!   allowed when every reading it takes is one the labels accept and it changes chord within a unit of where they do.
//! - **The method.** Projected quasi-Newton ([`crate::ruled`]): every point it tries keeps the rules. Each piece's sums
//!   are added in the pieces' order, so a fit gives the same bits however many threads read them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use mo_harmony::chordread::{
    BASS_GAP, Bound, COUNTED, ChordFitter, Core, FitLabel, FitRead, HAND, Label, NUMBERS, RULES, SHARPNESS, Weights,
};
use mo_harmony::{Grid, Role, Span};
use rayon::prelude::*;

use crate::harte::{self, Named};
use crate::item::{self, Item};
use crate::keyfit;
use crate::keys::{self, Parts, SongNotes};
use crate::ruled::{self, Plan, Reading};

/// How a piece is heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Input {
    /// The parts under the tune (POP909-CL's piano), each with its role.
    Piano,
    /// Every part, the tune with the rest.
    Full,
    /// The piano with its bass line as a bass part.
    Split,
    /// The piano with its bass line taken out, and no bass part.
    Upper,
    /// A piece `convert/voice.py` played, as it was played: every part with its role.
    Given,
}

impl Input {
    /// The ways a dataset's song can be heard (`--inputs`); a piece voice.py played is heard as given.
    const SONG: [Input; 4] = [Input::Piano, Input::Full, Input::Split, Input::Upper];

    fn short(self) -> &'static str {
        match self {
            Input::Piano => "piano",
            Input::Full => "full",
            Input::Split => "split",
            Input::Upper => "upper",
            Input::Given => "given",
        }
    }

    fn parse(text: &str) -> Option<Input> {
        Input::SONG.into_iter().find(|i| i.short() == text)
    }

    /// The ways a piece is heard: a piece voice.py played as given, a dataset's song each way asked for.
    fn of(item: &Item, asked: &[Input]) -> Vec<Input> {
        if item.render.is_empty() { asked.to_vec() } else { vec![Input::Given] }
    }

    /// The piece's parts as this way hears them, each in order of start; none left empty.
    fn parts(self, item: &Item) -> Vec<(Role, Vec<mo_harmony::Note>)> {
        let parts = match self {
            Input::Given | Input::Full => SongNotes::of(item, &Parts::Pitched).by_part(),
            Input::Piano => SongNotes::of(item, &Parts::Accompaniment).by_part(),
            Input::Split | Input::Upper => {
                let piano = item.notes_of(|role| role != "melody" && role != "drums");
                let (bass, upper) = keyfit::split_bass(&piano, BASS_GAP);
                let mut parts = vec![(Role::Harmony, keys::harmony_notes(&upper))];
                if self == Input::Split {
                    parts.push((Role::Bass, keys::harmony_notes(&bass)));
                }
                parts
            }
        };
        parts.into_iter().filter(|(_, notes)| !notes.is_empty()).collect()
    }
}

/// The readings a label accepts (the design's accept sets): its own root and core. On a dataset whose labels are a
/// template's triads while its piano often plays more (`lenient`: POP909-CL), a major label also accepts a maj7 or 6, a
/// minor one an m7 or m6 and a sus4 a 7sus4. A dominant 11 (a major third and an 11 with a ♭7, and no ♯9) also accepts
/// the 9sus4 players voice it as, as `convert/voice.py` plays it. A diminished seventh accepts its four roots and an
/// augmented triad its three: the same notes on each, which the reader names after its decode. None for no chord, a
/// label that can't be read, or a chord with no third, 4th, 2nd or fifth (a root alone).
pub(crate) fn accepts(text: &str, lenient: bool) -> Vec<Label> {
    let Named::Chord(chord) = harte::named(text) else { return Vec::new() };
    let Some(core) = Core::of_chord(chord) else { return Vec::new() };
    let mut cores = match core {
        Core::Major if lenient => vec![Core::Major, Core::Major7, Core::Major6],
        Core::Minor if lenient => vec![Core::Minor, Core::Minor7, Core::Minor6],
        Core::Sus4 if lenient => vec![Core::Sus4, Core::Dominant7Sus4],
        core => vec![core],
    };
    let intervals = chord.intervals();
    if core == Core::Dominant7 && intervals.has(5) && !intervals.has(3) {
        cores.push(Core::Dominant7Sus4);
    }
    let mut out = Vec::new();
    for core in cores {
        let apart = match core {
            Core::Diminished7 => 3,
            Core::Augmented => 4,
            _ => 12,
        };
        out.extend((0..12).step_by(apart).map(|up| Label::new(chord.root().up(up), core)));
    }
    out
}

/// Whether a source's labels are a template's triads while its piano plays more (POP909 and POP909-CL).
fn lenient(item: &Item) -> bool {
    item.source.starts_with("pop909")
}

/// A piece's labels as the fit is told them: each labelled stretch in ticks, with the readings it accepts.
fn labels_of(item: &Item) -> Vec<FitLabel> {
    let lenient = lenient(item);
    item.chords
        .iter()
        .map(|(start, end, text)| FitLabel { start: *start, end: *end, accepts: accepts(text, lenient) })
        .collect()
}

/// One piece heard one way, ready to read again and again.
struct Piece {
    source: String,
    input: Input,
    parts: Vec<(Role, Vec<mo_harmony::Note>)>,
    grid: Grid,
    span: Span,
    labels: Vec<FitLabel>,
    /// How much it counts (`--weigh`), and how long it is in bars.
    weight: f64,
    bars: f64,
}

/// The pieces `keep` keeps (given each one and its fold), each heard every way asked for that counts for something.
fn pieces_of(items: &[Item], folds: &[usize], keep: impl Fn(&Item, usize) -> bool + Sync, o: &Options) -> Vec<Piece> {
    let per_item: Vec<Vec<Piece>> = items
        .par_iter()
        .zip(folds)
        .map(|(item, &fold)| {
            if !keep(item, fold) {
                return Vec::new();
            }
            Input::of(item, &o.inputs)
                .into_iter()
                .map(|input| (input, o.weight(&item.source, input)))
                .filter(|&(_, weight)| weight > 0.0)
                .map(|(input, weight)| Piece {
                    source: item.source.clone(),
                    input,
                    parts: input.parts(item),
                    grid: keys::grid_of(item, false),
                    span: item.whole.span(),
                    labels: labels_of(item),
                    weight,
                    bars: item.length as f64 / item.bar() as f64,
                })
                .collect()
        })
        .collect();
    per_item.into_iter().flatten().collect()
}

/// Every piece read with some numbers, in the pieces' order: what each gives the fit, none where nothing was heard.
fn read_all(pieces: &[Piece], numbers: &[f64; NUMBERS]) -> Vec<Option<FitRead>> {
    let weights = Weights::from_numbers(numbers);
    pieces
        .par_iter()
        .map_init(ChordFitter::new, |fitter, piece| {
            let parts: Vec<mo_harmony::Part> =
                piece.parts.iter().map(|(role, notes)| mo_harmony::Part { role: *role, notes }).collect();
            fitter.read(&parts, piece.grid, piece.span, &piece.labels, &weights, &COUNTED)
        })
        .collect()
}

/// A piece's part in the aim: ln Z over the allowed ways less ln Z (how likely the decode finds the labelled
/// readings, 0 at best), and its slope in each number (the sharpness times the evidence the allowed ways hold on
/// average less what every way holds). None when nothing was heard, or no way is allowed.
fn piece_aim(read: Option<&FitRead>) -> Option<(f64, [f64; NUMBERS])> {
    let sums = read?.sums;
    sums.ln_z_labelled.is_finite().then(|| {
        let slope = std::array::from_fn(|k| SHARPNESS * (sums.expected_labelled[k] - sums.expected[k]));
        (sums.ln_z_labelled - sums.ln_z, slope)
    })
}

/// The pieces' parts in the aim added up, each weighed, in the pieces' order.
struct Totals {
    /// Σ ω (ln Z allowed − ln Z), its slope, and Σ ω g gᵀ (the slopes' outer products: a first picture of the
    /// aim's curvature, the empirical Fisher).
    likelihood: f64,
    slope: [f64; NUMBERS],
    fisher: Vec<f64>,
    /// Σ ω bars.
    bars: f64,
}

impl Totals {
    /// Over the pieces `counts` says count: the first reading's pieces with a way the labels allow. Where one has none
    /// any longer, the likelihood is minus infinity (the fit steps back).
    fn of(pieces: &[Piece], reads: &[Option<FitRead>], counts: &[bool]) -> Totals {
        let mut t = Totals { likelihood: 0.0, slope: [0.0; NUMBERS], fisher: vec![0.0; NUMBERS * NUMBERS], bars: 0.0 };
        for ((piece, read), _) in pieces.iter().zip(reads).zip(counts).filter(|(_, counts)| **counts) {
            t.bars += piece.weight * piece.bars;
            let Some((likelihood, slope)) = piece_aim(read.as_ref()) else {
                t.likelihood = f64::NEG_INFINITY;
                continue;
            };
            t.likelihood += piece.weight * likelihood;
            for (k, g) in slope.iter().enumerate() {
                t.slope[k] += piece.weight * g;
                for (j, h) in slope.iter().enumerate() {
                    t.fisher[k * NUMBERS + j] += piece.weight * g * h;
                }
            }
        }
        t
    }

    /// The aim at `x` as the method reads it: minus the likelihood per bar plus the pull towards `start`, its slope and
    /// a curvature to start from. The numbers `fixed` holds aren't pulled.
    fn reading(&self, x: &[f64], start: &[f64; NUMBERS], lambda: f64, fixed: &[bool; NUMBERS]) -> Reading {
        let bars = self.bars.max(f64::MIN_POSITIVE);
        let pulled = |k: usize| if fixed[k] { 0.0 } else { x.get(k).copied().unwrap_or(0.0) - start[k] };
        let pull: f64 = (0..NUMBERS).map(|k| pulled(k) * pulled(k)).sum();
        let mut curvature: Vec<f64> = self.fisher.iter().map(|f| f / bars).collect();
        for k in 0..NUMBERS {
            curvature[k * NUMBERS + k] += lambda;
        }
        Reading {
            loss: -self.likelihood / bars + lambda / 2.0 * pull,
            slope: (0..NUMBERS).map(|k| -self.slope[k] / bars + lambda * pulled(k)).collect(),
            curvature: Some(curvature),
        }
    }
}

/// What `chordfit` is asked to do.
#[derive(Debug, PartialEq)]
struct Options {
    files: Vec<PathBuf>,
    inputs: Vec<Input>,
    /// How much each source's pieces count, or one way of hearing them ("pop909cl/upper").
    weigh: HashMap<String, f64>,
    train: Vec<usize>,
    test: Vec<usize>,
    lambda: f64,
    steps: usize,
    from: Option<PathBuf>,
    fix: Vec<usize>,
    out: Option<PathBuf>,
}

/// The pull towards the numbers a fit starts from, unless `--lambda` says otherwise.
const LAMBDA: f64 = 1e-3;
/// The most steps a fit takes, unless `--steps` says otherwise.
const STEPS: usize = 40;

impl Options {
    fn parse(args: &[String]) -> Result<Options, String> {
        let mut o = Options {
            files: Vec::new(),
            inputs: Input::SONG.to_vec(),
            weigh: HashMap::new(),
            train: vec![2, 3, 4],
            test: vec![1],
            lambda: LAMBDA,
            steps: STEPS,
            from: None,
            fix: Vec::new(),
            out: None,
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |what: &str| args.next().ok_or_else(|| format!("{arg} needs {what}"));
            match arg.as_str() {
                "--inputs" => {
                    let list = value("a list, such as piano,full,split,upper")?;
                    o.inputs = list
                        .split(',')
                        .map(|i| {
                            Input::parse(i.trim()).ok_or_else(|| format!("--inputs: {i}? (piano, full, split, upper)"))
                        })
                        .collect::<Result<_, _>>()?;
                }
                "--weigh" => {
                    let spec = value("<source>=<w> or <source>/<input>=<w>")?;
                    let (name, w) = spec.split_once('=').ok_or("--weigh needs <source>=<w>")?;
                    let w: f64 = w.parse().map_err(|_| format!("--weigh {spec}: {w} isn't a number"))?;
                    if !(w.is_finite() && w >= 0.0) {
                        return Err(format!("--weigh {spec}: a weight is 0 or more"));
                    }
                    o.weigh.insert(name.to_owned(), w);
                }
                "--train" => o.train = folds_in(value("folds, such as 2,3,4")?)?,
                "--test" => o.test = folds_in(value("folds, such as 1")?)?,
                "--lambda" => {
                    let text = value("a number")?;
                    o.lambda = text
                        .parse()
                        .ok()
                        .filter(|x: &f64| x.is_finite() && *x >= 0.0)
                        .ok_or_else(|| format!("--lambda {text}: the pull is a number, 0 or more"))?;
                }
                "--steps" => {
                    let text = value("a number")?;
                    o.steps = text.parse().map_err(|_| format!("--steps {text}: a number of steps"))?;
                }
                "--from" => o.from = Some(PathBuf::from(value("a weights file a fit wrote")?)),
                "--fix" => {
                    for name in value("numbers' names, such as P1,P2,pair")?.split(',') {
                        o.fix.push(place_of(name.trim()).ok_or_else(|| format!("--fix: {name} is no number's name"))?);
                    }
                }
                "--out" => o.out = Some(PathBuf::from(value("a file to write the weights to")?)),
                a if a.starts_with("--") => return Err(format!("Unknown option {a}.\n\n{}", crate::USAGE)),
                a => o.files.push(PathBuf::from(a)),
            }
        }
        if o.files.is_empty() {
            return Err(crate::USAGE.to_owned());
        }
        if o.train.is_empty() {
            return Err("--train needs at least one fold".to_owned());
        }
        if let Some(fold) = o.train.iter().find(|f| o.test.contains(f)) {
            return Err(format!("fold {fold} is both learned from and measured"));
        }
        Ok(o)
    }

    /// How much a source's pieces heard one way count: `--weigh source/input=w`, else `--weigh source=w`, else 1.
    fn weight(&self, source: &str, input: Input) -> f64 {
        let named = format!("{source}/{}", input.short());
        self.weigh.get(&named).or_else(|| self.weigh.get(source)).copied().unwrap_or(1.0)
    }
}

/// Folds named in a list ("2,3,4"): 1 to 4, fold 0 being kept back for the final look.
pub(crate) fn folds_in(list: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for fold in list.split(',').map(str::trim).filter(|f| !f.is_empty()) {
        match fold.parse::<usize>() {
            Ok(0) => return Err("fold 0 is kept back for the final look".to_owned()),
            Ok(f @ 1..=4) if !out.contains(&f) => out.push(f),
            Ok(1..=4) => {}
            _ => return Err(format!("{fold} is no fold (1 to 4)")),
        }
    }
    Ok(out)
}

/// A number's place by its name ("T1", "P2", "pair", "none").
fn place_of(name: &str) -> Option<usize> {
    (0..NUMBERS).find(|&k| Weights::number_name(k) == name)
}

/// Reads weights a fit wrote (`--out`): each number by its name, any it leaves out as the hand-set weights have it.
/// Refuses a name that's no number's, and weights that break a rule.
pub(crate) fn load_weights(path: &Path) -> Result<Weights, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    weights_from(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn weights_from(text: &str) -> Result<Weights, String> {
    let file: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let given = file.get("numbers").and_then(serde_json::Value::as_object).ok_or("it has no \"numbers\"")?;
    let mut numbers = HAND.numbers();
    for (name, value) in given {
        let place = place_of(name).ok_or_else(|| format!("{name} is no number of the chord reader's"))?;
        numbers[place] = value.as_f64().ok_or_else(|| format!("{name} isn't a number"))?;
    }
    let weights = Weights::from_numbers(&numbers);
    let broken = weights.check();
    if broken.is_empty() { Ok(weights) } else { Err(format!("the weights break these rules: {}", broken.join("; "))) }
}

/// The weights file a fit writes: each number by its name, in the reader's order (to the last bit), then what it was
/// fitted on.
fn weights_file(numbers: &[f64; NUMBERS], about: &[(&str, String)]) -> String {
    let json = |value: serde_json::Value| value.to_string();
    let numbers: Vec<String> = (0..NUMBERS)
        .map(|k| format!("    {}: {}", json(Weights::number_name(k).into()), json(numbers[k].into())))
        .collect();
    let about: Vec<String> =
        about.iter().map(|(k, v)| format!("    {}: {}", json((*k).into()), json(v.as_str().into()))).collect();
    format!("{{\n  \"numbers\": {{\n{}\n  }},\n  \"fit\": {{\n{}\n  }}\n}}\n", numbers.join(",\n"), about.join(",\n"))
}

/// The numbers as `SET` takes them in mo-harmony's tests (`show_the_grid`, `show_the_cases_read_whole`), to read the
/// hand cases with them.
fn set_line(numbers: &[f64; NUMBERS]) -> String {
    let set: Vec<String> = (0..NUMBERS).map(|k| format!("{}={}", Weights::number_name(k), numbers[k])).collect();
    set.join(",")
}

/// Each source and way of hearing: its pieces' loss per bar (minus their likelihood per bar, unweighed) and how many
/// pieces count, at some reads.
fn by_group(pieces: &[Piece], reads: &[Option<FitRead>]) -> BTreeMap<(String, Input), (f64, f64, usize, usize)> {
    let mut out: BTreeMap<(String, Input), (f64, f64, usize, usize)> = BTreeMap::new();
    for (piece, read) in pieces.iter().zip(reads) {
        let group = out.entry((piece.source.clone(), piece.input)).or_default();
        match piece_aim(read.as_ref()) {
            Some((likelihood, _)) => {
                group.0 -= likelihood;
                group.1 += piece.bars;
                group.2 += 1;
            }
            None => group.3 += 1,
        }
    }
    out
}

/// The loss per bar of each source and way of hearing, at the start and at the end.
fn print_groups(title: &str, pieces: &[Piece], before: &[Option<FitRead>], after: &[Option<FitRead>]) {
    println!();
    println!("{title}: loss per bar (minus ln of the labelled readings' chance, a bar), from the start to the fit:");
    let (before, after) = (by_group(pieces, before), by_group(pieces, after));
    for ((source, input), (loss, bars, counted, left)) in &before {
        let end = after.get(&(source.clone(), *input)).copied().unwrap_or_default();
        let per = |loss: f64, bars: f64| if bars > 0.0 { loss / bars } else { f64::NAN };
        let left =
            if *left > 0 { format!(" ({left} with no way the labels allow, or nothing heard)") } else { String::new() };
        println!(
            "  {:<28} {:>9.4} {:>9.4}   {counted} pieces, {:.0} bars{left}",
            format!("{source}, {}", input.short()),
            per(*loss, *bars),
            per(end.0, end.1),
            bars
        );
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let o = Options::parse(args)?;
    let mut items = Vec::new();
    for f in &o.files {
        items.extend(item::read(f)?);
    }
    let read = items.len();
    items.retain(|i| !i.chords.is_empty());
    let folds = keyfit::drawn_folds(&items);
    let start = match &o.from {
        Some(path) => load_weights(path)?.numbers(),
        None => HAND.numbers(),
    };
    let mut fixed = [false; NUMBERS];
    for &k in &o.fix {
        fixed[k] = true;
    }
    let train = pieces_of(&items, &folds, |item, fold| item.may_learn() && o.train.contains(&fold), &o);
    let test = pieces_of(&items, &folds, |_, fold| o.test.contains(&fold), &o);
    let learned = items.iter().zip(&folds).filter(|(i, f)| i.may_learn() && o.train.contains(f)).count();
    println!(
        "{read} pieces read, {} with chord labels; learning from {learned} in folds {:?} ({} readings of them), \
         measuring {} readings in folds {:?}.",
        items.len(),
        o.train,
        train.len(),
        test.len(),
        o.test
    );
    if train.is_empty() {
        return Err(
            "Nothing to learn from: no piece whose licence lets fitted numbers ship is in those folds.".to_owned()
        );
    }
    let bounds: Vec<Bound> = RULES.iter().flat_map(|rule| rule.bounds.iter().copied()).collect();
    let plan = Plan { steps: o.steps, slope: 1e-6, fall: 1e-7 };
    println!(
        "Starting from {}; the pull towards it {}.",
        o.from.as_ref().map_or("the hand-set weights".to_owned(), |p| p.display().to_string()),
        o.lambda
    );

    let mut counts: Option<Vec<bool>> = None;
    let mut first: Option<Vec<Option<FitRead>>> = None;
    let mut seconds = 0.0f64;
    let fitted = ruled::minimise(
        &start,
        &bounds,
        &fixed,
        plan,
        |x| {
            let began = Instant::now();
            let numbers: [f64; NUMBERS] = std::array::from_fn(|k| x.get(k).copied().unwrap_or(0.0));
            let reads = read_all(&train, &numbers);
            let counts = counts.get_or_insert_with(|| reads.iter().map(|r| piece_aim(r.as_ref()).is_some()).collect());
            let reading = Totals::of(&train, &reads, counts).reading(x, &start, o.lambda, &fixed);
            seconds += began.elapsed().as_secs_f64();
            if first.is_none() {
                println!("  start: loss {:.6} ({:.1} s a reading)", reading.loss, seconds);
                first = Some(reads);
            }
            reading
        },
        |line| println!("  {line}"),
    );
    let numbers: [f64; NUMBERS] = std::array::from_fn(|k| fitted.x.get(k).copied().unwrap_or(0.0));
    println!(
        "Stopped after {} steps ({}): loss {:.6}; {} readings of every piece, {:.1} s each.",
        fitted.steps,
        fitted.why,
        fitted.loss,
        fitted.readings,
        seconds / fitted.readings.max(1) as f64
    );
    let counts = counts.unwrap_or_default();
    let left: usize = counts.iter().filter(|c| !**c).count();
    if left > 0 {
        println!("{left} readings were left out: no way the labels allow (their changes too close), or nothing heard.");
    }
    let start_reads = first.unwrap_or_default();
    let forced: usize = start_reads.iter().flatten().map(|r| r.forced).sum();
    let units: usize = start_reads.iter().flatten().map(|r| r.units).sum();
    let moved: usize = start_reads.iter().flatten().map(|r| r.moved).sum();
    let ends: usize = train.iter().map(|p| 2 * p.labels.iter().filter(|l| !l.accepts.is_empty()).count()).sum();
    println!(
        "At the start, over {units} units: {forced} readings kept only because the labels accept them; {moved} of the \
         labelled chords' {ends} ends moved to the nearest place a chord may change (none within a unit)."
    );

    println!();
    println!("{:<6} {:>9} {:>9} {:>9}", "number", "start", "fitted", "moved");
    for k in 0..NUMBERS {
        let held = if fixed[k] { "  (held)" } else { "" };
        let on_bound = bounds.iter().any(|b| on(b, &numbers, k));
        let on_bound = if on_bound && !fixed[k] { "  on a rule's bound" } else { "" };
        println!(
            "{:<6} {:>9.4} {:>9.4} {:>+9.4}{held}{on_bound}",
            Weights::number_name(k),
            start[k],
            numbers[k],
            numbers[k] - start[k]
        );
    }
    let fitted_weights = Weights::from_numbers(&numbers);
    let broken = fitted_weights.check();
    if !broken.is_empty() {
        return Err(format!("The fit broke these rules (a bug): {}", broken.join("; ")));
    }

    print_groups("Learned from", &train, &start_reads, &read_all(&train, &numbers));
    if !test.is_empty() {
        print_groups(
            &format!("Held out (folds {:?})", o.test),
            &test,
            &read_all(&test, &start),
            &read_all(&test, &numbers),
        );
    }
    println!();
    println!("The hand cases with these numbers (mo-harmony's tests):");
    println!(
        "  SET=\"{}\" cargo test -p mo-harmony --release show_the_cases_read_whole -- --ignored --nocapture",
        set_line(&numbers)
    );
    if let Some(path) = &o.out {
        let files: Vec<String> = o.files.iter().map(|f| f.display().to_string()).collect();
        let about = [
            ("files", files.join(" ")),
            ("train", format!("{:?}", o.train)),
            ("inputs", o.inputs.iter().map(|i| i.short()).collect::<Vec<_>>().join(",")),
            ("weigh", format!("{:?}", o.weigh.iter().collect::<BTreeMap<_, _>>())),
            ("lambda", o.lambda.to_string()),
            ("loss", fitted.loss.to_string()),
            ("steps", format!("{} ({})", fitted.steps, fitted.why)),
        ];
        std::fs::write(path, weights_file(&numbers, &about)).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("Written to {}.", path.display());
    }
    Ok(())
}

/// Whether a number sits on one of a bound's limits: at 0, or level with the number it's held under or over.
fn on(bound: &Bound, numbers: &[f64; NUMBERS], k: usize) -> bool {
    match *bound {
        Bound::AtLeast0(i) | Bound::AtMost0(i) => i == k && numbers[k] == 0.0,
        Bound::AtMost(i, j) => (i == k || j == k) && numbers[i] == numbers[j],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_harmony::PitchClass;

    #[test]
    fn a_label_accepts_its_own_reading_and_on_pop909_the_template_triads_more() {
        assert_eq!(accepts("C:maj", false), [Label::new(PitchClass::C, Core::Major)]);
        assert_eq!(
            accepts("C:maj", true),
            [
                Label::new(PitchClass::C, Core::Major),
                Label::new(PitchClass::C, Core::Major7),
                Label::new(PitchClass::C, Core::Major6),
            ]
        );
        assert_eq!(accepts("A:min/b3", true).len(), 3);
        assert_eq!(
            accepts("G:sus4", true),
            [Label::new(PitchClass::G, Core::Sus4), Label::new(PitchClass::G, Core::Dominant7Sus4)]
        );
        // Sevenths are as labelled, and so is every label of a set voice.py played.
        assert_eq!(accepts("G:7", true), [Label::new(PitchClass::G, Core::Dominant7)]);
        assert_eq!(accepts("D:min7", false), [Label::new(PitchClass::D, Core::Minor7)]);
        assert_eq!(accepts("F:maj9", false), [Label::new(PitchClass::F, Core::Major7)]);
        // A dominant 11 is voiced as a 9sus4; with a ♯9 it stays a dominant.
        assert_eq!(
            accepts("G:11", false),
            [Label::new(PitchClass::G, Core::Dominant7), Label::new(PitchClass::G, Core::Dominant7Sus4)]
        );
        assert_eq!(accepts("C:7(#9,11)", false), [Label::new(PitchClass::C, Core::Dominant7)]);
        // The same notes on other roots.
        let dim7: Vec<u8> = accepts("B:dim7", false).iter().map(|l| l.root.number()).collect();
        assert_eq!(dim7, [11, 2, 5, 8]);
        let aug: Vec<u8> = accepts("C:aug", false).iter().map(|l| l.root.number()).collect();
        assert_eq!(aug, [0, 4, 8]);
        // No chord, a label that can't be read, or a root alone: any reading.
        for free in ["N", "X", "C:(1)", "nonsense"] {
            assert!(accepts(free, false).is_empty(), "{free}");
        }
    }

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn the_options_say_what_to_learn_from_and_how() {
        let parse = |a: &[&str]| Options::parse(&args(a));
        let plain = parse(&["a.jsonl"]).unwrap();
        assert_eq!((plain.train, plain.test, plain.inputs.len()), (vec![2, 3, 4], vec![1], 4));
        let all = parse(&[
            "a.jsonl",
            "b.jsonl",
            "--inputs",
            "piano,split",
            "--weigh",
            "billboard=0.25",
            "--weigh",
            "pop909cl/upper=0",
            "--train",
            "1,2,3",
            "--test",
            "4",
            "--lambda",
            "0.01",
            "--steps",
            "5",
            "--fix",
            "P1,pair",
            "--out",
            "w.json",
        ])
        .unwrap();
        assert_eq!(all.inputs, [Input::Piano, Input::Split]);
        assert_eq!((all.weight("billboard", Input::Given), all.weight("pop909cl", Input::Upper)), (0.25, 0.0));
        assert_eq!((all.weight("pop909cl", Input::Piano), all.weight("rs200", Input::Given)), (1.0, 1.0));
        assert_eq!((all.train, all.test, all.lambda, all.steps), (vec![1, 2, 3], vec![4], 0.01, 5));
        assert_eq!(all.fix, [place_of("P1").unwrap(), Weights::PAIR]);
        assert_eq!(all.out, Some(PathBuf::from("w.json")));
        let wrong = |a: &[&str]| parse(a).unwrap_err();
        assert_eq!(wrong(&["a.jsonl", "--train", "0,1"]), "fold 0 is kept back for the final look");
        assert_eq!(wrong(&["a.jsonl", "--train", "1,2", "--test", "2"]), "fold 2 is both learned from and measured");
        assert_eq!(wrong(&["a.jsonl", "--test", "5"]), "5 is no fold (1 to 4)");
        assert_eq!(wrong(&["a.jsonl", "--fix", "T99"]), "--fix: T99 is no number's name");
        assert_eq!(wrong(&["a.jsonl", "--inputs", "tune"]), "--inputs: tune? (piano, full, split, upper)");
        assert_eq!(wrong(&["a.jsonl", "--weigh", "x=-1"]), "--weigh x=-1: a weight is 0 or more");
        assert_eq!(wrong(&["a.jsonl", "--lambda"]), "--lambda needs a number");
        assert_eq!(wrong(&[]), crate::USAGE);
    }

    #[test]
    fn a_weights_file_reads_back_to_the_last_bit_and_refuses_what_it_cant_be() {
        let mut numbers = HAND.numbers();
        numbers[0] = std::f64::consts::E;
        numbers[Weights::NONE] = -1.0 / 3.0;
        let text = weights_file(&numbers, &[("train", "[2, 3, 4]".to_owned())]);
        assert_eq!(weights_from(&text).unwrap().numbers(), numbers);
        // A number left out is the hand-set one.
        assert_eq!(weights_from(r#"{"numbers": {"T1": 4}}"#).unwrap().numbers()[1..], HAND.numbers()[1..]);
        assert_eq!(weights_from(r#"{"numbers": {"Z1": 4}}"#).unwrap_err(), "Z1 is no number of the chord reader's");
        assert_eq!(weights_from(r#"{"numbers": {"T1": "a"}}"#).unwrap_err(), "T1 isn't a number");
        assert_eq!(weights_from(r#"{"weights": {}}"#).unwrap_err(), "it has no \"numbers\"");
        assert!(weights_from(r#"{"numbers": {"T1": -1}}"#).unwrap_err().contains("T1 counts for the label"));
        // The SET line names every number.
        assert_eq!(set_line(&HAND.numbers()).split(',').count(), NUMBERS);
        assert!(set_line(&HAND.numbers()).starts_with("T1=3,T2=1,T3=-3,"));
    }

    fn song() -> Item {
        serde_json::from_str(
            r#"{"source":"pop909cl","id":"1","ppq":4,"length":32,"licence":"MIT",
                "chords":[[0,16,"C:maj"],[16,32,"N"]],
                "parts":[{"role":"accompaniment","notes":[[0,16,36,90],[0,16,60,90],[0,16,64,90],[16,16,67,90]]},
                         {"role":"melody","notes":[[0,4,72,90]]}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn a_song_is_heard_each_way_asked_for_and_a_render_as_given() {
        let item = song();
        let keys = |input: Input| -> Vec<(Role, Vec<u8>)> {
            input.parts(&item).into_iter().map(|(role, notes)| (role, notes.iter().map(|n| n.key).collect())).collect()
        };
        assert_eq!(keys(Input::Piano), [(Role::Harmony, vec![36, 60, 64, 67])]);
        assert_eq!(keys(Input::Full), [(Role::Harmony, vec![36, 60, 64, 67]), (Role::Melody, vec![72])]);
        // The bass line by the reader's own test: C2 has nothing within a fifth above it; G4 alone is the lowest
        // note sounding with nothing above it.
        assert_eq!(keys(Input::Split), [(Role::Harmony, vec![60, 64]), (Role::Bass, vec![36, 67])]);
        assert_eq!(keys(Input::Upper), [(Role::Harmony, vec![60, 64])]);
        assert_eq!(Input::of(&item, &Input::SONG), Input::SONG);
        let mut render = item.clone();
        render.render = vec![(0, 16, BTreeMap::new())];
        assert_eq!(Input::of(&render, &Input::SONG), [Input::Given]);
        // Its labels: C (lenient on POP909-CL), then no chord, which accepts anything.
        let labels = labels_of(&item);
        assert_eq!((labels[0].start, labels[0].end, labels[0].accepts.len()), (0, 16, 3));
        assert!(labels[1].accepts.is_empty());
    }

    #[test]
    fn the_fit_moves_the_numbers_towards_the_labels_and_keeps_the_rules() {
        // Four bars of C F G7 C, each as labelled: the start reads them, and the fit makes them likelier.
        let bars: [(&[u8], &str); 4] = [
            (&[48, 60, 64, 67], "C:maj"),
            (&[41, 60, 65, 69], "F:maj"),
            (&[43, 59, 62, 65], "G:7"),
            (&[48, 60, 64, 67], "C:maj"),
        ];
        let (mut notes, mut chords) = (Vec::new(), Vec::new());
        for (k, (keys, label)) in (0_i64..).zip(bars) {
            notes.extend(keys.iter().map(|key| format!("[{},16,{key},90]", k * 16)));
            chords.push(format!("[{},{},\"{label}\"]", k * 16, (k + 1) * 16));
        }
        let item: Item = serde_json::from_str(&format!(
            r#"{{"source":"t","id":"1","ppq":4,"length":64,"licence":"MIT","chords":[{}],
                "parts":[{{"role":"chords","notes":[{}]}}]}}"#,
            chords.join(","),
            notes.join(",")
        ))
        .unwrap();
        let o = Options::parse(&args(&["a.jsonl", "--inputs", "piano"])).unwrap();
        let pieces = pieces_of(std::slice::from_ref(&item), &[2], |_, _| true, &o);
        assert_eq!(pieces.len(), 1);
        let start = HAND.numbers();
        let counts = [true];
        let at = |x: &[f64]| {
            let numbers: [f64; NUMBERS] = std::array::from_fn(|k| x[k]);
            Totals::of(&pieces, &read_all(&pieces, &numbers), &counts).reading(x, &start, 0.0, &[false; NUMBERS])
        };
        let begin = at(&start);
        assert!(begin.loss.is_finite() && begin.loss > 0.0, "{}", begin.loss);
        // The slope is the change in the loss: a small step against it lowers the loss by about as much.
        let step: Vec<f64> = begin.slope.iter().map(|g| -1e-4 * g).collect();
        let moved: Vec<f64> = start.iter().zip(&step).map(|(x, s)| x + s).collect();
        let fall = begin.loss - at(&moved).loss;
        let said: f64 = begin.slope.iter().zip(&step).map(|(g, s)| -g * s).sum();
        assert!(fall > 0.0 && (fall - said).abs() < 0.05 * said, "fell {fall}, the slope said {said}");
        let bounds: Vec<Bound> = RULES.iter().flat_map(|rule| rule.bounds.iter().copied()).collect();
        let plan = Plan { steps: 5, ..Plan::default() };
        let fitted = ruled::minimise(&start, &bounds, &[false; NUMBERS], plan, at, |_| {});
        assert!(fitted.loss < begin.loss, "{} against {}", fitted.loss, begin.loss);
        let numbers: [f64; NUMBERS] = std::array::from_fn(|k| fitted.x[k]);
        assert!(Weights::from_numbers(&numbers).check().is_empty());
        // The same numbers read the same, to the bit, on one thread or many.
        let one =
            rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(|| read_all(&pieces, &numbers));
        assert_eq!(one, read_all(&pieces, &numbers));
    }
}
