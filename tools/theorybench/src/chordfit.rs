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
//!   the piano's bass line as a bass part (the notes the reader would hear as the bass, [`bass_line`]); and the rest
//!   of the piano alone, with no bass, where the roots often aren't played.
//! - **What a label accepts** ([`accepts`]): its root and core. POP909-CL labels a template's triads while its piano
//!   often plays more, so there a major label also accepts a maj7 or 6, a minor one an m7 or m6, and a sus4 a 7sus4.
//!   A dominant 11 accepts the 9sus4 it's voiced as too, and a diminished seventh its four roots (an augmented triad
//!   its three). No chord, a label that can't be read, or a chord the reader can't read (a root alone) accepts any
//!   reading.
//! - **The aim.** The chance the reader gives the readings the labels allow, over every way it can read each piece:
//!   ln Z over the allowed ways less ln Z over every way (`chordread::ChordFitter`), summed over the pieces each
//!   weighed by `--weigh` and taken per bar, less a pull towards the numbers it starts from (`--lambda`). A way is
//!   allowed when every reading it takes is one the labels accept and it changes chord within a unit of where they do.
//! - **The hand cases.** mo-harmony's hand cases (`chordread::cases`) can be learned from too (`--hand μ`): each music
//!   the cases read whole is a piece, labelled where its gates say what it must read ([`cases::fit_cases`]), and the
//!   aim adds μ times their mean −ln P(labels) to the data's loss per bar. `--hold n` keeps them as straight-line rules
//!   instead (or too): each case's labels keep a chance of at least 0.5, or what they have at the start if less, as
//!   the slope at the start sees it ([`hold_rules`]); then, for at most n more rounds, every rule is drawn again where
//!   the fit got to, and the cases whose gates still break must get likelier (a chance of 0.75, then 0.875, but never
//!   more than at the start). A gate that only asks for readings to be kept, or one on music the fit doesn't learn
//!   from, is checked but never held. After a fit, every gate is read with the fitted numbers, and the report (and the
//!   `--out` file) says which break: weights that break a gate don't ship. The cases the design holds out (G\*) are
//!   neither learned from nor looked at here; the tests check them.
//! - **The method.** Projected quasi-Newton ([`crate::ruled`]): every point it tries keeps the rules. Each piece's sums
//!   are added in the pieces' order, so a fit gives the same bits however many threads read them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use mo_harmony::chordread::cases::{self, Book, FitCase, Step, Verdict};
use mo_harmony::chordread::{
    BASS_GAP, Bound, ChordFitter, Core, Feature, FitLabel, FitRead, HAND, Label, Model, NUMBERS, RULES, SHARPNESS,
    SHIPPED, Weights,
};
use mo_harmony::{Grid, Role, Span};
use rayon::prelude::*;

use crate::harte::{self, Named};
use crate::item::{self, Item};
use crate::keyfit;
use crate::keys::{self, Parts, SongNotes};
use crate::ruled::{self, Plan, Plane, Reading};

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

    /// The piece's parts as this way hears them, each in order of start; none left empty. `split` is the piano's bass
    /// line and the rest ([`bass_line`]), for the ways that hear them apart.
    fn parts(self, item: &Item, split: Option<&Split>) -> Vec<(Role, Vec<mo_harmony::Note>)> {
        let parts = match (self, split) {
            (Input::Given | Input::Full, _) => SongNotes::of(item, &Parts::Pitched).by_part(),
            (Input::Piano, _) => SongNotes::of(item, &Parts::Accompaniment).by_part(),
            (Input::Split, Some(split)) => {
                vec![(Role::Harmony, split.upper.clone()), (Role::Bass, split.bass.clone())]
            }
            (Input::Upper, Some(split)) => vec![(Role::Harmony, split.upper.clone())],
            (Input::Split | Input::Upper, None) => Vec::new(),
        };
        parts.into_iter().filter(|(_, notes)| !notes.is_empty()).collect()
    }

    /// Whether this way hears the piano's bass line apart from the rest.
    fn splits(self) -> bool {
        matches!(self, Input::Split | Input::Upper)
    }
}

/// The piano's bass line and the rest of its notes, as mo-harmony takes them.
struct Split {
    bass: Vec<mo_harmony::Note>,
    upper: Vec<mo_harmony::Note>,
}

impl Split {
    fn of(item: &Item) -> Split {
        let piano = item.notes_of(|role| role != "melody" && role != "drums");
        let (bass, upper) = bass_line(&piano);
        Split { bass: keys::harmony_notes(&bass), upper: keys::harmony_notes(&upper) }
    }
}

/// The piano's bass line and the rest, as the reader hears a bass where no bass part plays (`chordread::BASS_GAP`), note
/// by note. At each note's start, the lowest note sounding is the bass when the next one up of another pitch class is
/// a fifth or more above it, or, with nothing of another pitch class sounding, when it carries on the bass before it
/// (the same key); a note that's the bass at any start while it sounds is in the bass line, with the notes of its key
/// sounding with it. So a bass note struck before the chord over it joins the line when the chord comes, and a figure
/// played alone (an Alberti right hand, a broken chord with no bass under it) stays in the rest. Unlike the reader, it
/// doesn't leave a broken part's notes out of the bass, nor ask a note to sound a quarter of a unit. `notes` must be in
/// order of start.
pub(crate) fn bass_line(notes: &[item::Note]) -> (Vec<item::Note>, Vec<item::Note>) {
    let mut bass = vec![false; notes.len()];
    let mut sounding: Vec<usize> = Vec::new();
    // The bass at the last start, if there was one.
    let mut before: Option<u8> = None;
    let mut i = 0;
    while let Some(first) = notes.get(i) {
        let at = first.start;
        let together = notes.get(i..).map_or(0, |rest| rest.partition_point(|n| n.start == at)).max(1);
        sounding.retain(|&k| notes.get(k).is_some_and(|n| n.start + n.length.max(1) > at));
        sounding.extend(i..(i + together).min(notes.len()));
        let key_of = |k: usize| notes.get(k).map_or(u8::MAX, |n| n.key);
        let lowest = sounding.iter().map(|&k| key_of(k)).min();
        before = lowest.filter(|&low| {
            let next = sounding.iter().map(|&k| key_of(k)).filter(|&k| k > low && k % 12 != low % 12).min();
            next.map_or(before == Some(low), |next| next - low >= BASS_GAP)
        });
        if let Some(low) = before {
            for &k in &sounding {
                if key_of(k) == low
                    && let Some(b) = bass.get_mut(k)
                {
                    *b = true;
                }
            }
        }
        i += together;
    }
    let (mut line, mut rest) = (Vec::new(), Vec::new());
    for (n, is_bass) in notes.iter().zip(bass) {
        if is_bass { line.push(*n) } else { rest.push(*n) }
    }
    (line, rest)
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
    /// Shared by the ways the same song is heard.
    labels: Arc<[FitLabel]>,
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
            let ways: Vec<(Input, f64)> = Input::of(item, &o.inputs)
                .into_iter()
                .map(|input| (input, o.weight(&item.source, input)))
                .filter(|&(_, weight)| weight > 0.0)
                .collect();
            let split = ways.iter().any(|(input, _)| input.splits()).then(|| Split::of(item));
            let labels: Arc<[FitLabel]> = labels_of(item).into();
            let (grid, span, bars) =
                (keys::grid_of(item, false), item.whole.span(), item.length as f64 / item.bar() as f64);
            ways.into_iter()
                .map(|(input, weight)| Piece {
                    source: item.source.clone(),
                    input,
                    parts: input.parts(item, split.as_ref()),
                    grid,
                    span,
                    labels: Arc::clone(&labels),
                    weight,
                    bars,
                })
                .collect()
        })
        .collect();
    per_item.into_iter().flatten().collect()
}

/// The hand cases the fit learns from ([`cases::fit_cases`]), each a piece heard as given, counting 1 and one "bar"
/// long, so their part in the aim is their mean.
fn hand_pieces(cases: &[FitCase]) -> Vec<Piece> {
    let piece = |case: &FitCase| Piece {
        source: format!("hand {}", case.name),
        input: Input::Given,
        parts: case.music.parts.clone(),
        grid: case.music.grid,
        span: case.music.span,
        labels: case.labels.clone().into(),
        weight: 1.0,
        bars: 1.0,
    };
    cases.iter().map(piece).collect()
}

/// Every piece read with some numbers, in the pieces' order: what each gives the fit, none where nothing was heard.
fn read_all(pieces: &[Piece], numbers: &[f64; NUMBERS]) -> Vec<Option<FitRead>> {
    let weights = Weights::from_numbers(numbers);
    pieces
        .par_iter()
        .map_init(ChordFitter::new, |fitter, piece| {
            let parts: Vec<mo_harmony::Part> =
                piece.parts.iter().map(|(role, notes)| mo_harmony::Part { role: *role, notes }).collect();
            fitter.read(&parts, piece.grid, piece.span, &piece.labels, &weights, SHIPPED.tables)
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

/// How likely `--hold` keeps each hand case's labels at first, unless they're less likely at the start.
const HOLD: f64 = 0.5;

/// The chance `--hold` asks of a case's labels once its gates have broken `level` times: 0.5, then 0.75, 0.875...,
/// but never more than they had at the start (from the hand-set weights, every gate holds there).
fn held_at(level: u32, start: f64) -> f64 {
    (1.0 - (1.0 - HOLD) / 2f64.powi(i32::try_from(level).unwrap_or(i32::MAX))).min(start)
}

/// The straight-line rule that keeps a hand case's labels at least e^`least` likely as the numbers move from `x`, as
/// the slope there sees it: ln P(labels) + g · (y - x) ≥ least. None when nothing was heard, or no way is allowed.
fn hold(read: Option<&FitRead>, x: &[f64], least: f64) -> Option<Plane> {
    let (ln_p, slope) = piece_aim(read)?;
    let along: f64 = slope.iter().zip(x).map(|(g, x)| g * x).sum();
    Some(Plane { a: slope.to_vec(), b: least - ln_p + along })
}

/// Where `--hold` stands with one hand case: the chance its labels had at the start, and how many rounds its gates
/// have broken.
#[derive(Clone, Copy, Debug)]
struct Held {
    start: f64,
    level: u32,
}

/// The rules holding the hand cases, drawn where the numbers are (`x`, each case read there as `reads`): a case whose
/// gates break must reach the chance its level asks ([`held_at`]); every other keeps that chance or, if it has less
/// now, what it has. Gives each rule with the case it holds.
fn hold_rules(reads: &[Option<FitRead>], x: &[f64], held: &[Option<Held>], broken: &[usize]) -> Vec<(usize, Plane)> {
    let rule = |(k, (read, held)): (usize, (&Option<FitRead>, &Option<Held>))| {
        let held = (*held)?;
        let (ln_p, _) = piece_aim(read.as_ref())?;
        let least = held_at(held.level, held.start).ln();
        let least = if broken.contains(&k) { least } else { least.min(ln_p) };
        Some((k, hold(read.as_ref(), x, least)?))
    };
    reads.iter().zip(held).enumerate().filter_map(rule).collect()
}

/// The bounds the fit keeps: every rule's ([`RULES`]), and B1's floor. B1 is at least 0 only because B2, B3 and B4
/// are and none is more than B1; said as a bound of its own, the projection can always put it right (it only ever
/// lowers the larger number of an order), and rules that would need B1 below 0 are seen to leave no numbers.
fn fit_bounds() -> Vec<Bound> {
    let mut bounds: Vec<Bound> = RULES.iter().flat_map(|rule| rule.bounds.iter().copied()).collect();
    bounds.push(Bound::AtLeast0(Feature::BassRoot as usize));
    bounds
}

/// The hand cases the fit learns from whose music a gate that breaks reads, each once, in order.
fn broken_cases(broken: &[&Verdict], cases: &[FitCase]) -> Vec<usize> {
    let mut out: Vec<usize> =
        broken.iter().filter_map(|v| cases.iter().position(|case| case.music == v.music)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The data's reading with `mu` times the hand cases' added: the loss, its slope and its curvature.
fn with_hand(data: Reading, hand: Reading, mu: f64) -> Reading {
    let slope = data.slope.iter().zip(&hand.slope).map(|(a, b)| a + mu * b).collect();
    let curvature = match (data.curvature, hand.curvature) {
        (Some(a), Some(b)) => Some(a.iter().zip(&b).map(|(a, b)| a + mu * b).collect()),
        (a, _) => a,
    };
    Reading { loss: data.loss + mu * hand.loss, slope, curvature }
}

/// The model the reader ships with, on other weights.
fn model_with(weights: &Weights) -> Model<'_> {
    Model { weights, ..SHIPPED }
}

/// The gates the fit looks at (not those held out) that break, in a book.
fn broken_gates(book: &Book) -> Vec<&Verdict> {
    book.broken().into_iter().filter(|v| !v.held_out).collect()
}

/// The hand cases read with the numbers the fit started from and with the fitted ones: how many gates break at each
/// step, every gate the fitted numbers break (with what they read), and the targets met. The gates the design holds
/// out aren't looked at. Gives the gates the fitted numbers break.
fn print_gates(start: &Book, fitted: &Book) -> Vec<String> {
    println!();
    println!("The hand cases' gates (weights that break one aren't shipped), from the start to the fit:");
    let (before, after) = (broken_gates(start), broken_gates(fitted));
    for step in Step::ALL {
        let gates = fitted.verdicts.iter().filter(|v| v.step == step && v.gate && !v.held_out).count();
        let count = |broken: &[&Verdict]| broken.iter().filter(|v| v.step == step).count();
        println!(
            "  {:<22} {gates:>3} gates: {:>3} break at the start, {:>3} fitted",
            step.about(),
            count(&before),
            count(&after)
        );
    }
    let held_out = |gate: bool| fitted.verdicts.iter().filter(|v| v.gate == gate && v.held_out).count();
    println!(
        "  ({} gates and {} targets the design holds out aren't looked at here: the tests check them.)",
        held_out(true),
        held_out(false)
    );
    for v in &after {
        println!("  BREAKS {} ({}): {}", v.name, v.step.about(), v.wrong.join(", "));
        println!("         read {}", v.read.replace('\n', "\n         "));
    }
    let ((met_before, targets), (met_after, _)) = (start.targets(), fitted.targets());
    println!("  Targets met: {met_before} of {targets} at the start, {met_after} fitted.");
    after.iter().map(|v| format!("{} ({})", v.name, v.step.about())).collect()
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
    /// How much the hand cases count (μ): their mean −ln P(labels), against the data's loss per bar.
    hand: f64,
    /// The most rounds the hand cases' gates are held as straight-line rules (0: not held).
    hold: usize,
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
            hand: 0.0,
            hold: 0,
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
                "--hand" => {
                    let text = value("a number")?;
                    o.hand = text
                        .parse()
                        .ok()
                        .filter(|x: &f64| x.is_finite() && *x >= 0.0)
                        .ok_or_else(|| format!("--hand {text}: how much the hand cases count, 0 or more"))?;
                }
                "--hold" => {
                    let text = value("a number of rounds")?;
                    o.hold = text.parse().map_err(|_| format!("--hold {text}: a number of rounds"))?;
                }
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
    // A piece learned from that shares its notes with one named before it goes to that one's fold: a bench of its
    // renders read without it (`chords --folds`) draws them by their own names, so it can measure another fold there.
    let elsewhere = items.iter().zip(&folds).filter(|(i, f)| i.may_learn() && keyfit::own_fold(i) != **f).count();
    if elsewhere > 0 {
        println!(
            "{elsewhere} pieces learned from share their notes with one named before them and go to its fold: \
             `chords --folds` on their renders alone draws those by their own names."
        );
    }
    if train.is_empty() {
        return Err(
            "Nothing to learn from: no piece whose licence lets fitted numbers ship is in those folds.".to_owned()
        );
    }
    let fit_cases = if o.hand > 0.0 || o.hold > 0 { cases::fit_cases()? } else { Vec::new() };
    let hand = hand_pieces(&fit_cases);
    if o.hand > 0.0 {
        let labels: usize = hand.iter().map(|p| p.labels.len()).sum();
        println!(
            "Learning from {} hand cases too ({labels} stretches their gates label), counting {} against the data's \
             loss per bar.",
            hand.len(),
            o.hand
        );
    }
    let bounds = fit_bounds();
    let plan = Plan { steps: o.steps, slope: 1e-6, fall: 1e-7 };
    println!(
        "Starting from {}; the pull towards it {}.",
        o.from.as_ref().map_or("the hand-set weights".to_owned(), |p| p.display().to_string()),
        o.lambda
    );
    let start_weights = Weights::from_numbers(&start);
    let start_book = Book::read(&model_with(&start_weights))?;
    let broken_at_start = broken_gates(&start_book).len();
    if o.hold > 0 && broken_at_start > 0 {
        println!(
            "The numbers it starts from break {broken_at_start} gates already: --hold asks no case for more than it has \
             at the start, so those may stay broken."
        );
    }

    let mut counts: Option<Vec<bool>> = None;
    let mut hand_counts: Option<Vec<bool>> = None;
    let mut first: Option<Vec<Option<FitRead>>> = None;
    let mut hand_first: Option<Vec<Option<FitRead>>> = None;
    let mut seconds = 0.0f64;
    // With --hold, every hand case's labels keep a chance of at least 0.5 (or what they have, if less), as a
    // straight-line rule (a plane) drawn at the start; each round after, every rule is drawn again where the fit got
    // to, the cases whose gates still break asked for more.
    let mut held: Vec<Option<Held>> = Vec::new();
    let mut planes: Vec<Plane> = Vec::new();
    if o.hold > 0 {
        let reads = read_all(&hand, &start);
        held =
            reads.iter().map(|r| piece_aim(r.as_ref()).map(|(ln_p, _)| Held { start: ln_p.exp(), level: 0 })).collect();
        planes = hold_rules(&reads, &start, &held, &[]).into_iter().map(|(_, plane)| plane).collect();
        println!(
            "Holding the hand cases' gates: each case's labels keep a chance of at least {HOLD} (or what they have at \
             the start, if less), as {} straight-line rules on the numbers, for at most {} rounds of at most {} steps.",
            planes.len(),
            o.hold.saturating_add(1),
            o.steps
        );
    }
    let (mut x, mut steps, mut readings) = (start.to_vec(), 0, 0);
    let mut why = "no step taken";
    let mut loss = f64::NAN;
    let (mut rounds, mut ended) = (0, "the most rounds were fitted");
    for round in 0..=o.hold {
        let fitted = ruled::minimise(
            &x,
            &bounds,
            &planes,
            &fixed,
            plan,
            |x| {
                let began = Instant::now();
                let numbers: [f64; NUMBERS] = std::array::from_fn(|k| x.get(k).copied().unwrap_or(0.0));
                let reads = read_all(&train, &numbers);
                let counts =
                    counts.get_or_insert_with(|| reads.iter().map(|r| piece_aim(r.as_ref()).is_some()).collect());
                let mut reading = Totals::of(&train, &reads, counts).reading(x, &start, o.lambda, &fixed);
                if o.hand > 0.0 && !hand.is_empty() {
                    let hand_reads = read_all(&hand, &numbers);
                    let hand_counts = hand_counts
                        .get_or_insert_with(|| hand_reads.iter().map(|r| piece_aim(r.as_ref()).is_some()).collect());
                    let cases = Totals::of(&hand, &hand_reads, hand_counts).reading(x, &start, 0.0, &fixed);
                    if hand_first.is_none() {
                        println!("  start: the hand cases' mean -ln P {:.6}", cases.loss);
                        hand_first = Some(hand_reads);
                    }
                    reading = with_hand(reading, cases, o.hand);
                }
                seconds += began.elapsed().as_secs_f64();
                if first.is_none() {
                    println!("  start: loss {:.6} ({:.1} s a reading)", reading.loss, seconds);
                    first = Some(reads);
                }
                reading
            },
            |line| println!("  {line}"),
        );
        (x, loss, why) = (fitted.x, fitted.loss, fitted.why);
        steps += fitted.steps;
        readings += fitted.readings;
        rounds += 1;
        if round == o.hold {
            break;
        }
        // The cases whose gates the fit breaks are asked for more, and every rule is drawn again where it got to.
        let numbers: [f64; NUMBERS] = std::array::from_fn(|k| x.get(k).copied().unwrap_or(0.0));
        let weights = Weights::from_numbers(&numbers);
        let book = Book::read(&model_with(&weights))?;
        let gates = broken_gates(&book);
        if gates.is_empty() {
            println!("Round {}: every gate holds.", round + 1);
            ended = "every gate holds";
            break;
        }
        let names: Vec<&str> = gates.iter().map(|v| v.name.as_str()).collect();
        let broken = broken_cases(&gates, &fit_cases);
        let said = format!("Round {}: {} gates break ({})", round + 1, gates.len(), names.join("; "));
        if broken.is_empty() {
            println!("{said}, none of them on music the fit learns from: stopping.");
            ended = "no gate that breaks is on music the fit learns from";
            break;
        }
        for &k in &broken {
            if let Some(Some(h)) = held.get_mut(k) {
                h.level += 1;
            }
        }
        let reads = read_all(&hand, &numbers);
        let rules = hold_rules(&reads, &x, &held, &broken);
        // A rule that already holds asks for nothing new: the fit would end where it is.
        if !rules.iter().any(|(k, plane)| broken.contains(k) && plane.room(&x) < 0.0) {
            println!(
                "{said}; their cases' labels are already as likely as they're asked to be (never more than at the \
                 start), so holding them again changes nothing: stopping."
            );
            ended = "the cases whose gates break are as likely as they're asked to be";
            break;
        }
        let tried: Vec<Plane> = rules.into_iter().map(|(_, plane)| plane).collect();
        // Rules may leave no numbers that keep them all: then the fit would never find a point it may read.
        if !ruled::can_keep(&x, &bounds, &tried, &fixed) {
            println!("{said}; holding them again would leave no numbers that keep every rule: stopping.");
            ended = "holding them again would leave no numbers that keep every rule";
            break;
        }
        planes = tried;
        let asked: Vec<String> = broken
            .iter()
            .filter_map(|&k| {
                let h = held.get(k).copied().flatten()?;
                Some(format!("{} at {:.3}", hand.get(k)?.source.trim_start_matches("hand "), held_at(h.level, h.start)))
            })
            .collect();
        println!("{said}; asking more of {} and fitting on, every rule drawn again here.", asked.join(", "));
    }
    let fitted = ruled::Fitted { x, loss, steps, readings, why };
    let numbers: [f64; NUMBERS] = std::array::from_fn(|k| fitted.x.get(k).copied().unwrap_or(0.0));
    if !planes.is_empty() {
        let on = planes.iter().filter(|p| p.room(&fitted.x) < 1e-6).count();
        println!(
            "{} of the {} straight-line rules holding the hand cases are met with nothing to spare.",
            on,
            planes.len()
        );
    }
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
    if !hand.is_empty() {
        let left = hand_counts.as_ref().map_or(0, |c| c.iter().filter(|c| !**c).count());
        let mean = |reads: &[Option<FitRead>]| {
            let aims: Vec<f64> = reads.iter().filter_map(|r| piece_aim(r.as_ref()).map(|(l, _)| -l)).collect();
            aims.iter().sum::<f64>() / aims.len().max(1) as f64
        };
        let after = read_all(&hand, &numbers);
        println!(
            "  {:<28} {:>9.4} {:>9.4}   {} cases (the mean -ln P of each case's labels){}",
            "the hand cases",
            mean(&hand_first.unwrap_or_else(|| read_all(&hand, &start))),
            mean(&after),
            hand.len(),
            if left > 0 { format!(", {left} with no way the labels allow") } else { String::new() }
        );
    }
    if !test.is_empty() {
        print_groups(
            &format!("Held out (folds {:?})", o.test),
            &test,
            &read_all(&test, &start),
            &read_all(&test, &numbers),
        );
    }
    let broken = print_gates(&start_book, &Book::read(&model_with(&fitted_weights))?);
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
            ("hand", o.hand.to_string()),
            (
                "hold",
                if o.hold == 0 {
                    "not held".to_owned()
                } else {
                    format!(
                        "{rounds} of at most {} rounds ({ended}), {} straight-line rules at the end",
                        o.hold.saturating_add(1),
                        planes.len()
                    )
                },
            ),
            ("loss", fitted.loss.to_string()),
            ("steps", format!("{} ({})", fitted.steps, fitted.why)),
            (
                "gates",
                if broken.is_empty() {
                    "every gate holds".to_owned()
                } else {
                    format!("{} break, so these weights aren't shipped: {}", broken.len(), broken.join("; "))
                },
            ),
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

    fn note(start: i64, length: i64, key: u8) -> item::Note {
        item::Note { start, length, key, velocity: 100 }
    }

    /// The keys of some notes, in order.
    fn keys_of(notes: &[item::Note]) -> Vec<u8> {
        notes.iter().map(|n| n.key).collect()
    }

    #[test]
    fn the_bass_line_is_what_the_reader_would_hear_as_the_bass() {
        // A held C2 under C E G, then G1 struck alone a beat before the chord over it: both are the bass (the G1 once
        // the chord comes), and the chord isn't.
        let notes = [
            note(0, 384, 36),
            note(0, 384, 60),
            note(0, 384, 64),
            note(0, 384, 67),
            note(384, 384, 31),
            note(480, 288, 62),
            note(480, 288, 65),
            note(480, 288, 71),
        ];
        let (bass, rest) = bass_line(&notes);
        assert_eq!((keys_of(&bass), rest.len()), (vec![36, 31], 6));
        // An Alberti right hand alone, its notes one after another: never a bass, as the reader hears it.
        let alberti: Vec<item::Note> =
            [60, 67, 64, 67, 60, 67, 64, 67].iter().zip(0..).map(|(&k, i)| note(i * 48, 48, k)).collect();
        let (bass, rest) = bass_line(&alberti);
        assert!(bass.is_empty(), "{bass:?}");
        assert_eq!(rest.len(), 8);
        // A fourth up is too near (the reader's gap is a fifth): A1 under D2 isn't a bass; a fifth up, G1 under D2, is.
        assert!(bass_line(&[note(0, 96, 33), note(0, 96, 38)]).0.is_empty());
        assert_eq!(keys_of(&bass_line(&[note(0, 96, 31), note(0, 96, 38)]).0), [31]);
        // Its own octave above isn't a chord over it: C2 and C3 alone, struck after C2 was the bass, carry it on; a
        // new key struck alone doesn't (D2), nor what follows it alone (D2 again).
        let carried = [
            note(0, 96, 36),
            note(0, 96, 52),
            note(96, 96, 36),
            note(96, 96, 48),
            note(192, 96, 38),
            note(288, 96, 38),
        ];
        let (bass, rest) = bass_line(&carried);
        assert_eq!(keys_of(&bass), [36, 36]);
        assert_eq!(keys_of(&rest), [52, 48, 38, 38]);
    }

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
            "--hand",
            "0.5",
            "--hold",
            "2",
        ])
        .unwrap();
        assert_eq!(all.inputs, [Input::Piano, Input::Split]);
        assert_eq!((all.weight("billboard", Input::Given), all.weight("pop909cl", Input::Upper)), (0.25, 0.0));
        assert_eq!((all.weight("pop909cl", Input::Piano), all.weight("rs200", Input::Given)), (1.0, 1.0));
        assert_eq!((all.train, all.test, all.lambda, all.steps), (vec![1, 2, 3], vec![4], 0.01, 5));
        assert_eq!(all.fix, [place_of("P1").unwrap(), Weights::PAIR]);
        assert_eq!(all.out, Some(PathBuf::from("w.json")));
        assert_eq!((plain.hand, all.hand, plain.hold, all.hold), (0.0, 0.5, 0, 2));
        let wrong = |a: &[&str]| parse(a).unwrap_err();
        assert_eq!(wrong(&["a.jsonl", "--train", "0,1"]), "fold 0 is kept back for the final look");
        assert_eq!(wrong(&["a.jsonl", "--train", "1,2", "--test", "2"]), "fold 2 is both learned from and measured");
        assert_eq!(wrong(&["a.jsonl", "--test", "5"]), "5 is no fold (1 to 4)");
        assert_eq!(wrong(&["a.jsonl", "--fix", "T99"]), "--fix: T99 is no number's name");
        assert_eq!(wrong(&["a.jsonl", "--inputs", "tune"]), "--inputs: tune? (piano, full, split, upper)");
        assert_eq!(wrong(&["a.jsonl", "--weigh", "x=-1"]), "--weigh x=-1: a weight is 0 or more");
        assert_eq!(wrong(&["a.jsonl", "--lambda"]), "--lambda needs a number");
        assert_eq!(wrong(&["a.jsonl", "--hand", "-1"]), "--hand -1: how much the hand cases count, 0 or more");
        assert_eq!(wrong(&["a.jsonl", "--hold", "x"]), "--hold x: a number of rounds");
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
            let split = input.splits().then(|| Split::of(&item));
            let parts = input.parts(&item, split.as_ref());
            parts.into_iter().map(|(role, notes)| (role, notes.iter().map(|n| n.key).collect())).collect()
        };
        assert_eq!(keys(Input::Piano), [(Role::Harmony, vec![36, 60, 64, 67])]);
        assert_eq!(keys(Input::Full), [(Role::Harmony, vec![36, 60, 64, 67]), (Role::Melody, vec![72])]);
        // The bass line as the reader hears it: C2, with nothing of another pitch class within a fifth above it. G4,
        // struck alone after it, carries on no bass, so it stays with the rest.
        assert_eq!(keys(Input::Split), [(Role::Harmony, vec![60, 64, 67]), (Role::Bass, vec![36])]);
        assert_eq!(keys(Input::Upper), [(Role::Harmony, vec![60, 64, 67])]);
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
        let bounds = fit_bounds();
        let plan = Plan { steps: 5, ..Plan::default() };
        let fitted = ruled::minimise(&start, &bounds, &[], &[false; NUMBERS], plan, at, |_| {});
        assert!(fitted.loss < begin.loss, "{} against {}", fitted.loss, begin.loss);
        let numbers: [f64; NUMBERS] = std::array::from_fn(|k| fitted.x[k]);
        assert!(Weights::from_numbers(&numbers).check().is_empty());
        // The same numbers read the same, to the bit, on one thread or many.
        let one =
            rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(|| read_all(&pieces, &numbers));
        assert_eq!(one, read_all(&pieces, &numbers));
    }

    #[test]
    fn the_hand_cases_are_learned_from_as_pieces_each_counting_once() {
        let hand = hand_pieces(&cases::fit_cases().unwrap());
        assert!(hand.len() > 40, "{} cases", hand.len());
        assert!(hand.iter().all(|p| p.weight == 1.0 && p.bars == 1.0 && !p.labels.is_empty() && p.span == Span::Loop));
        // The built-in weights find every case's labelled ways, so the aim is finite, and each counts once: the hand
        // cases' part is their mean.
        let start = HAND.numbers();
        let reads = read_all(&hand, &start);
        let counts: Vec<bool> = reads.iter().map(|r| piece_aim(r.as_ref()).is_some()).collect();
        assert!(counts.iter().all(|c| *c));
        let totals = Totals::of(&hand, &reads, &counts);
        assert_eq!(totals.bars, hand.len() as f64);
        let reading = totals.reading(&start, &start, 0.0, &[false; NUMBERS]);
        assert!(reading.loss > 0.0 && reading.loss.is_finite(), "{}", reading.loss);
        assert!((reading.loss + totals.likelihood / hand.len() as f64).abs() < 1e-12);
    }

    #[test]
    fn the_hand_cases_add_to_the_aim_by_how_much_they_count() {
        let data = Reading { loss: 2.0, slope: vec![1.0, -1.0], curvature: Some(vec![1.0, 0.0, 0.0, 1.0]) };
        let hand = Reading { loss: 0.5, slope: vec![2.0, 4.0], curvature: Some(vec![4.0, 2.0, 2.0, 8.0]) };
        let both = with_hand(data, hand, 0.5);
        assert_eq!((both.loss, both.slope), (2.25, vec![2.0, 1.0]));
        assert_eq!(both.curvature, Some(vec![3.0, 1.0, 1.0, 5.0]));
    }

    #[test]
    fn the_gates_are_read_with_the_numbers_fitted() {
        // The hand-set numbers keep every gate the fit looks at; numbers turned round break some, each named.
        let book = |w: &Weights| Book::read(&model_with(w)).unwrap();
        let hand = book(&HAND);
        assert!(broken_gates(&hand).is_empty());
        let wrong = Weights::from_numbers(&HAND.numbers().map(|x| -x));
        let wrong = book(&wrong);
        assert!(!broken_gates(&wrong).is_empty());
        assert!(broken_gates(&wrong).iter().all(|v| !v.held_out));
        let said = print_gates(&hand, &wrong);
        assert_eq!(said.len(), broken_gates(&wrong).len());
        assert!(print_gates(&hand, &hand).is_empty());
    }

    #[test]
    fn a_case_held_again_is_asked_for_more_but_never_more_than_it_had_at_the_start() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
        assert!(close(held_at(0, 0.9), 0.5) && close(held_at(1, 0.9), 0.75) && close(held_at(2, 0.9), 0.875));
        assert!(close(held_at(0, 0.3), 0.3) && close(held_at(3, 0.3), 0.3));
    }

    #[test]
    fn the_hold_rules_ask_more_of_the_broken_cases_and_keep_the_rest_where_they_are() {
        let fit = cases::fit_cases().unwrap();
        let hand = hand_pieces(&fit);
        let start = HAND.numbers();
        let reads = read_all(&hand, &start);
        let held: Vec<Option<Held>> =
            reads.iter().map(|r| piece_aim(r.as_ref()).map(|(ln_p, _)| Held { start: ln_p.exp(), level: 0 })).collect();
        // At the start every rule holds: each case keeps half its chance, or what it has if less.
        let rules = hold_rules(&reads, &start, &held, &[]);
        assert_eq!(rules.len(), hand.len());
        assert!(rules.iter().all(|(_, plane)| plane.room(&start) >= -1e-12));
        let chance = |k: usize| piece_aim(reads[k].as_ref()).unwrap().0.exp();
        // A case likely at the start and now broken (h1, C6 held 8 bars): held again at 0.75, its rule binds where
        // its labels are no likelier than that; the others aren't asked for more than they have.
        let k = fit.iter().position(|c| c.name.starts_with("h1 ")).unwrap();
        assert!(chance(k) > 0.75, "{}", chance(k));
        let mut again = held.clone();
        if let Some(Some(h)) = again.get_mut(k) {
            h.level = 1;
        }
        let rules = hold_rules(&reads, &start, &again, &[k]);
        let room = |rules: &[(usize, Plane)], k: usize| rules.iter().find(|(j, _)| *j == k).unwrap().1.room(&start);
        assert!((room(&rules, k) - (chance(k).ln() - 0.75f64.ln())).abs() < 1e-9);
        assert!(rules.iter().filter(|(j, _)| *j != k).all(|(_, plane)| plane.room(&start) >= -1e-12));
        // A broken case less likely at the start than it's asked for is asked only for what it had then.
        let low = (0..hand.len()).find(|&j| chance(j) < 0.5).unwrap();
        let mut again = held.clone();
        if let Some(Some(h)) = again.get_mut(low) {
            h.level = 2;
        }
        assert!(room(&hold_rules(&reads, &start, &again, &[low]), low).abs() < 1e-9);
    }

    #[test]
    fn a_broken_gate_is_held_on_the_music_it_reads_and_nowhere_else() {
        let fit = cases::fit_cases().unwrap();
        let wrong = Weights::from_numbers(&HAND.numbers().map(|x| -x));
        let book = Book::read(&model_with(&wrong)).unwrap();
        let gates = broken_gates(&book);
        let held = broken_cases(&gates, &fit);
        assert!(!held.is_empty());
        // Each case held reads music a broken gate reads; a gate on music no case learned from holds nothing.
        for &k in &held {
            assert!(gates.iter().any(|v| v.music == fit[k].music), "{}", fit[k].name);
        }
        let mut sorted = held.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, held);
        let elsewhere: Vec<&Verdict> =
            gates.iter().copied().filter(|v| fit.iter().all(|c| c.music != v.music)).collect();
        assert!(!elsewhere.is_empty() && broken_cases(&elsewhere, &fit).is_empty());
        // A stretch case's gate, one stretch at a time, is held on the case that reads its music whole.
        let four = book.verdicts.iter().find(|v| v.step == Step::Stretch && v.name == "4 Am").unwrap();
        let at = broken_cases(&[four], &fit);
        assert_eq!(at.len(), 1);
        assert!(fit[at[0]].names.iter().any(|n| n == "w4 Am"));
    }

    #[test]
    fn rules_that_would_need_b1_below_0_are_seen_to_leave_no_numbers() {
        // B1 is at least 0 only through 0 ≤ B2, B3, B4 ≤ B1: the fit says it as a bound of its own.
        let bounds = fit_bounds();
        let b1 = place_of("B1").unwrap();
        assert!(bounds.contains(&Bound::AtLeast0(b1)));
        let start = HAND.numbers();
        let none = [false; NUMBERS];
        let b1_at_most = |most: f64| {
            let mut a = vec![0.0; NUMBERS];
            a[b1] = -1.0;
            Plane { a, b: -most }
        };
        // B1 below 0 leaves no numbers, with the rules' bounds alone too.
        let rules: Vec<Bound> = RULES.iter().flat_map(|rule| rule.bounds.iter().copied()).collect();
        assert!(!ruled::can_keep(&start, &bounds, &[b1_at_most(-0.1)], &none));
        assert!(!ruled::can_keep(&start, &rules, &[b1_at_most(-0.1)], &none));
        // B1 at 0 can be kept, by B2 to B4 going to 0 with it, and the nearest such numbers keep every bound.
        assert!(ruled::can_keep(&start, &bounds, &[b1_at_most(0.0)], &none));
        let near = ruled::project(&start, &bounds, &[b1_at_most(0.0)], &none);
        assert!(bounds.iter().all(|b| b.holds(&near)), "{near:?}");
        assert!(["B1", "B2", "B3", "B4"].iter().all(|code| near[place_of(code).unwrap()] == 0.0), "{near:?}");
    }
}
