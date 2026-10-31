//! `theorybench keyfit`: fits mo-harmony's key finder on labelled music, measures it on songs held out of each fit,
//! and writes the weights into the crate.
//!
//! - **Folds.** Each song goes to one of five folds by a hash of its name (`pop909/001`), so POP909 and POP909-CL put
//!   a song in the same fold and every run splits the songs the same way; songs whose notes are the same, moved to
//!   another key, share a fold. Fold 0 is kept back. Every choice (evidence, penalties, calibration) is made on folds
//!   1 to 4, each scored by a finder fitted on the other three. `--final` scores fold 0 on its own, with what was
//!   chosen, and adds a line to `test-runs.md`, so how often fold 0 was looked at is on record. `--write`, the last
//!   step, scores every fold before it writes the weights, so it adds a line too.
//! - **Cases.** Whole songs (read as songs); stretches of 1 to 16 bars cut along the bar lines (read as windows, and
//!   scored against the key labelled at their middle, as the C# bench did); and each song's first bars from its first
//!   bar with notes (read as loops, standing in for a pack's loops). Each is also heard as four other kinds of input:
//!   the tune alone, the piano without its bass line, the bass line taken from the piano, and the piano.
//! - **Model.** Each stream's weights are a counted table (the average share of each note counted up from the labelled
//!   tonic, over the training songs) times one trust weight, plus a correction for each note, held near 0 by its own
//!   penalty (`--lambda-delta`; `inf` keeps the tables as they are). A song is fitted with tables counted without its
//!   own fifth of the training songs, so the tables can't flatter themselves.
//! - **Calibration.** How sure a reading may be is fitted on scores of songs held out of the fit: a curve over how many
//!   bars differ, offsets for the kind of input, a curve a loop adds and an offset for a song (`Calibration` in the
//!   crate).
//! - **Answers.** Each reading is also graded as the app shows it (one key, a key or its relative, a home note, no
//!   clear key: `keyfind::grade`), and the report says how often each kind is given and how often it's right. The
//!   chance each kind needs at each length, and the share of notes in one scale a key needs, are fitted on the
//!   chances of songs held out ([`answerfit`]); each fold is graded with those fitted on the other folds.
//! - **Hand cases.** Every model fitted is read on the design's hand cases (`keyfind::cases`), and the report says how
//!   many keep each gate and meet each target. `--write` won't write weights that break a gate.
//!
//! Only stretches inside one key region are fitted on (at most eight a song for each length); every stretch is
//! measured, so the numbers stay comparable with the C# app's on the same cases.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use mo_harmony::keyfind::cases::{self, Book, Verdict};
use mo_harmony::keyfind::{
    self, CALIBRATION_TERMS, Calibration, Config, FEATURES, KEY_FINDER_VERSION, KeyAnswer, KeyReader, MINOR, STREAMS,
    Span, Stream, Why,
};
use mo_harmony::{Grid, Key, PitchClass, Role};
use rayon::prelude::*;

use crate::answerfit::{self, Fitted};
use crate::baseline;
use crate::fit::{self, Choice, Scored};
use crate::item::{self, Item, Note};
use crate::keys::{self, Answer, Found, MIN_NOTES, Run};
use crate::metrics::{self, pct};

/// Clip lengths fitted and measured unless `--bars` says otherwise: from a one-bar loop to a 16-bar section.
const LENGTHS: [i64; 5] = [1, 2, 4, 8, 16];

/// Folds the songs are split into.
const FOLDS: u64 = 5;
/// The fold kept back for `--final`.
const FROZEN: usize = 0;
/// Inner folds for counting the tables a song is fitted with.
const INNER: u64 = 5;
/// Windows of each length fitted on from each song, unless `--picks` says otherwise.
const PICKS: usize = 8;
/// How much the whole songs (as a full mix) count in the fit, against 1 for every other set of cases.
const SONG_WEIGHT: f64 = 5.0;
/// The penalty on the trust weights and the minor weight.
const LAMBDA_W: f64 = 1e-4;
/// The penalty on the corrections unless `--lambda-delta` says otherwise.
const LAMBDA_DELTA: f64 = 1e-4;

/// FNV-1a, 64 bits: the fold hash, the same on every computer. The finder hashes bars with FNV-1a too, but this is
/// its own copy: the folds must never move, even if how the finder tells bars apart changes.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The kinds of input every case is heard as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Input {
    /// Every pitched part: the tune (and POP909's second tune) as melody, the piano as harmony.
    Full,
    /// The tunes alone.
    Tune,
    /// The piano without its bass line.
    Upper,
    /// The piano's bass line, as a bass part.
    Bass,
    /// The piano.
    Piano,
}

impl Input {
    const ALL: [Input; 5] = [Input::Full, Input::Tune, Input::Upper, Input::Bass, Input::Piano];

    fn name(self) -> &'static str {
        match self {
            Input::Full => "full mix",
            Input::Tune => "tune only",
            Input::Upper => "upper piano",
            Input::Bass => "bass from the piano",
            Input::Piano => "piano",
        }
    }

    fn parse(text: &str) -> Option<Input> {
        Input::ALL.into_iter().find(|i| i.name().split(' ').next() == Some(text))
    }
}

/// Where a case is cut from the song.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Cut {
    Song,
    /// A stretch of this many bars along the bar lines.
    Window(i64),
    /// The song's first bars, from its first bar with notes.
    Opening(i64),
}

impl Cut {
    fn span(self) -> Span {
        match self {
            Cut::Song => Span::Song,
            Cut::Window(_) => Span::Window,
            Cut::Opening(_) => Span::Loop,
        }
    }

    fn name(self) -> String {
        match self {
            Cut::Song => "Whole songs".to_owned(),
            Cut::Window(b) => format!("{b}-bar windows"),
            Cut::Opening(b) => format!("{b}-bar openings"),
        }
    }
}

/// What the finder heard in one case, as the fit reads it: each stream's features for the keys with tonic C (any
/// other key's are the same, counted from its own tonic), and what the calibration and the answer need.
#[derive(Clone, Debug)]
struct Evidence {
    x: [[f64; 12]; STREAMS],
    bars: u32,
    config: Config,
    diatonic_fit: f64,
    notes: u32,
}

impl Evidence {
    fn of(heard: &keyfind::Heard, span: Span) -> Evidence {
        let mut x = [[0.0; 12]; STREAMS];
        heard.features(Key::major(PitchClass::C), span, |i, v| {
            if let Some(cell) = x.get_mut(i / 24).and_then(|row| row.get_mut(i % 24)) {
                *cell = v;
            }
        });
        Evidence {
            x,
            bars: heard.bars_heard(),
            config: heard.config(),
            diatonic_fit: heard.diatonic_fit(),
            notes: heard.notes,
        }
    }

    /// A key's features, exactly as `Heard::features` gives them (the same values, in the same order).
    fn features(&self, key: Key, each: &mut dyn FnMut(usize, f64)) {
        let mode = if key.is_minor() { 12 } else { 0 };
        let tonic = key.tonic.index();
        for (s, row) in self.x.iter().enumerate() {
            for d in 0..12 {
                let v = row[(tonic + d) % 12];
                if v != 0.0 {
                    each(s * 24 + mode + d, v);
                }
            }
        }
        if key.is_minor() {
            each(MINOR, 1.0);
        }
    }
}

/// One case: a song, or a stretch of it, heard as one kind of input.
struct Case {
    song: usize,
    truth: Key,
    cut: Cut,
    input: Input,
    /// It lies wholly inside one key region: only these are fitted on.
    one_key: bool,
    /// Picked to be fitted on: every song and opening, and a few windows of each length from each song.
    pick: bool,
    ev: Evidence,
    /// The C# app's key and confidence on the same notes.
    cs: Option<(Key, f64)>,
}

/// A song's name and folds.
struct Song {
    name: String,
    fold: usize,
    inner: usize,
}

/// The name a song's fold is drawn from: POP909 and POP909-CL share their songs' names.
fn fold_name(item: &Item) -> String {
    let source = if item.source.starts_with("pop909") { "pop909" } else { item.source.as_str() };
    format!("{source}/{}", item.id)
}

/// A hash of a song's notes that doesn't change when they're all moved up or down: the same song in another key.
fn shape_of(item: &Item) -> u64 {
    let notes = item.notes_of(|r| r != "drums");
    let low = notes.iter().map(|n| n.key).min().unwrap_or(0);
    let first = notes.first().map_or(0, |n| n.start);
    let mut bytes = Vec::with_capacity(notes.len() * 17);
    for n in &notes {
        bytes.extend((n.start - first).to_le_bytes());
        bytes.extend(n.length.to_le_bytes());
        bytes.push(n.key - low);
    }
    fnv1a(&bytes)
}

/// Each song's folds, songs with the same notes in another key going where the first of them by name goes.
fn songs_of(items: &[Item]) -> Vec<Song> {
    let shapes: Vec<u64> = items.par_iter().map(shape_of).collect();
    let mut first_name: HashMap<u64, String> = HashMap::new();
    for (item, shape) in items.iter().zip(&shapes) {
        let name = fold_name(item);
        first_name.entry(*shape).and_modify(|n| *n = n.clone().min(name.clone())).or_insert(name);
    }
    items
        .iter()
        .zip(&shapes)
        .map(|(item, shape)| {
            let name = fold_name(item);
            let drawn = first_name.get(shape).unwrap_or(&name);
            let h = fnv1a(drawn.as_bytes());
            Song { name, fold: (h % FOLDS) as usize, inner: (h / FOLDS % INNER) as usize }
        })
        .collect()
}

/// A song's notes as each kind of input hears them, each list in order of start.
struct Voices {
    tune: Vec<Note>,
    /// The parts under the tune, by role: a dataset's bass part is heard as a bass part.
    others: Vec<(Role, Vec<Note>)>,
    /// The parts under the tune split into the bass line and the rest, as if they were one piano part.
    upper: Vec<Note>,
    bass: Vec<Note>,
    all: Vec<Note>,
}

impl Voices {
    fn of(item: &Item) -> Voices {
        let tune = item.notes_of(|r| r == "melody");
        let mut others: Vec<(Role, Vec<Note>)> = Vec::new();
        for part in item.parts.iter().filter(|p| p.role != "melody" && p.role != "drums") {
            let role = keys::role(&part.role);
            let notes = part.notes.iter().map(|&(start, length, key, velocity)| Note { start, length, key, velocity });
            match others.iter_mut().find(|(r, _)| *r == role) {
                Some((_, list)) => list.extend(notes),
                None => others.push((role, notes.collect())),
            }
        }
        for (_, list) in &mut others {
            list.sort_by_key(|n| (n.start, n.key));
        }
        let piano = item.notes_of(|r| r != "melody" && r != "drums");
        let (bass, upper) = split_bass(&piano);
        let all = item.notes_of(|r| r != "drums");
        Voices { tune, others, upper, bass, all }
    }

    /// The parts of one kind of input, cut to a stretch (moved to start at 0) when one is given.
    fn parts(&self, input: Input, cut: Option<(i64, i64)>) -> Vec<(Role, Vec<Note>)> {
        let others = self.others.iter().map(|(role, notes)| (*role, notes));
        let lists: Vec<(Role, &Vec<Note>)> = match input {
            Input::Full => std::iter::once((Role::Melody, &self.tune)).chain(others).collect(),
            Input::Tune => vec![(Role::Melody, &self.tune)],
            Input::Upper => vec![(Role::Harmony, &self.upper)],
            Input::Bass => vec![(Role::Bass, &self.bass)],
            Input::Piano => others.collect(),
        };
        lists
            .into_iter()
            .map(|(role, notes)| {
                (role, cut.map_or_else(|| notes.clone(), |(from, length)| item::cut(notes, from, length)))
            })
            .filter(|(_, notes)| !notes.is_empty())
            .collect()
    }

    /// How many notes start in a stretch, in every part.
    fn count(&self, from: i64, length: i64) -> usize {
        self.all.partition_point(|n| n.start < from + length) - self.all.partition_point(|n| n.start < from)
    }
}

/// The piano's bass line and the rest: a note is in the bass line when, as it starts, it's the lowest note the piano
/// sounds and nothing else sounds within a fourth above it (the key finder's own test for a bass). `notes` must be in
/// order of start.
fn split_bass(notes: &[Note]) -> (Vec<Note>, Vec<Note>) {
    let (mut bass, mut upper) = (Vec::new(), Vec::new());
    let mut sounding: Vec<Note> = Vec::new();
    let mut i = 0;
    while let Some(first) = notes.get(i) {
        let at = first.start;
        let together = notes.get(i..).map_or(0, |rest| rest.partition_point(|n| n.start == at)).max(1);
        let group = notes.get(i..i + together).unwrap_or_default();
        sounding.retain(|n| n.start + n.length.max(1) > at);
        for n in group {
            let low = u16::from(n.key);
            let alone = sounding.iter().chain(group).all(|m| {
                let k = u16::from(m.key);
                k == low || k >= low + 5
            });
            if alone {
                bass.push(*n);
            } else {
                upper.push(*n);
            }
        }
        sounding.extend_from_slice(group);
        i += together;
    }
    (bass, upper)
}

/// What one case gave: the finder's evidence, and the C# app's key and confidence.
type Heard = (Evidence, Option<(Key, f64)>);

/// Hears one case's parts and asks the C# app too.
fn hear(reader: &mut KeyReader, parts: &[(Role, Vec<Note>)], grid: Grid, ppq: u32, span: Span) -> Option<Heard> {
    let notes: Vec<Vec<mo_harmony::Note>> = parts
        .iter()
        .map(|(_, notes)| {
            notes
                .iter()
                .map(|n| mo_harmony::Note {
                    start: n.start,
                    length: n.length,
                    key: n.key,
                    velocity: keys::velocity16(n.velocity),
                })
                .collect()
        })
        .collect();
    let heard_parts: Vec<mo_harmony::Part> =
        parts.iter().zip(&notes).map(|((role, _), notes)| mo_harmony::Part { role: *role, notes }).collect();
    let heard = reader.hear(&heard_parts, grid)?;
    let mut all: Vec<Note> = parts.iter().flat_map(|(_, n)| n.iter().copied()).collect();
    all.sort_by_key(|n| (n.start, n.key));
    let cs = baseline::key(&all, 4 * i64::from(ppq));
    Some((Evidence::of(&heard, span), cs))
}

/// Whether every key region a stretch touches is labelled `truth`.
fn one_key_in(item: &Item, from: i64, to: i64, truth: Key) -> bool {
    item.keys.iter().filter(|(s, e, _)| *s < to && *e > from).all(|(_, _, k)| Key::parse(k) == Some(truth))
}

/// What to cut from each song, and how each cut is heard.
struct Cutting<'a> {
    lengths: &'a [i64],
    inputs: &'a [Input],
    picks: usize,
}

/// Every case from one song.
fn song_cases(item: &Item, song: usize, name: &str, cutting: &Cutting) -> Vec<Case> {
    let mut reader = KeyReader::new();
    let voices = Voices::of(item);
    let mut out = Vec::new();
    let mut add = |cut: Cut, stretch: Option<(i64, i64)>, truth: Key, one_key: bool, pick: bool| {
        for &input in cutting.inputs {
            let grid = keys::grid_of(item, stretch.is_some());
            if let Some((ev, cs)) = hear(&mut reader, &voices.parts(input, stretch), grid, item.ppq, cut.span()) {
                out.push(Case { song, truth, cut, input, one_key, pick, ev, cs });
            }
        }
    };
    if let Some(truth) = item.main_key().filter(|_| voices.all.len() >= MIN_NOTES) {
        add(Cut::Song, None, truth, item.one_key(), true);
    }
    let (bar, origin) = (item.bar(), item.grid_origin());
    // The first bar with notes in it, where the openings start.
    let opening = voices.all.iter().find(|n| n.start >= origin).map(|n| origin + (n.start - origin) / bar * bar);
    for &bars in cutting.lengths {
        let length = bars * bar;
        let mut windows = Vec::new();
        let mut at = origin;
        while at + length <= item.length {
            if let Some(truth) = item.key_at(at + length / 2).filter(|_| voices.count(at, length) >= MIN_NOTES) {
                windows.push((at, truth, one_key_in(item, at, at + length, truth)));
            }
            at += length;
        }
        // The windows fitted on: a few in one key, picked by a hash of the song and the bar.
        let mut ranked: Vec<(u64, i64)> = windows
            .iter()
            .filter(|w| w.2)
            .map(|w| (fnv1a(format!("{name}/{bars}/{}", (w.0 - origin) / bar).as_bytes()), w.0))
            .collect();
        ranked.sort_unstable();
        let picked: HashSet<i64> = ranked.iter().take(cutting.picks).map(|r| r.1).collect();
        for (at, truth, one_key) in windows {
            add(Cut::Window(bars), Some((at, length)), truth, one_key, picked.contains(&at));
        }
        if let Some(at) = opening.filter(|at| at + length <= item.length)
            && let Some(truth) = item.key_at(at + length / 2).filter(|_| voices.count(at, length) >= MIN_NOTES)
        {
            let one_key = one_key_in(item, at, at + length, truth);
            add(Cut::Opening(bars), Some((at, length)), truth, one_key, true);
        }
    }
    out
}

/// The counted tables: for each stream and mode, the log of the average share of each note counted up from the
/// labelled tonic.
struct Tables([[[f64; 12]; 2]; STREAMS]);

impl Tables {
    /// Counts the tables from the songs `keep` takes: the note and bass streams and a song's ends from whole songs in
    /// one key, a loop's start from the shortest openings in one key.
    fn count(cases: &[Case], loop_cut: Cut, keep: impl Fn(usize) -> bool) -> Tables {
        let mut sum = [[[0.0f64; 12]; 2]; STREAMS];
        let mut songs = [[0.0f64; 2]; STREAMS];
        for c in cases.iter().filter(|c| c.input == Input::Full && c.one_key && keep(c.song)) {
            let streams: &[Stream] = match c.cut {
                Cut::Song => &[
                    Stream::Tune,
                    Stream::Harmony,
                    Stream::Bass,
                    Stream::MajorChords,
                    Stream::MinorChords,
                    Stream::FirstBassSong,
                    Stream::LastBassSong,
                ],
                cut if cut == loop_cut => &[Stream::FirstBassLoop],
                _ => &[],
            };
            let m = usize::from(c.truth.is_minor());
            let tonic = c.truth.tonic.index();
            for &s in streams {
                let row = &c.ev.x[s as usize];
                let total: f64 = row.iter().sum();
                if total <= 0.0 {
                    continue;
                }
                for (d, cell) in sum[s as usize][m].iter_mut().enumerate() {
                    *cell += row[(tonic + d) % 12] / total;
                }
                songs[s as usize][m] += 1.0;
            }
        }
        Tables(std::array::from_fn(|s| {
            std::array::from_fn(|m| std::array::from_fn(|d| ((sum[s][m][d] + 2.0 / 12.0) / (songs[s][m] + 2.0)).ln()))
        }))
    }
}

/// One case as the fitter sees it: a trust weight for each stream (its table times its features), the minor weight,
/// then a correction for each feature when they're fitted.
struct Fitting<'a> {
    case: &'a Case,
    tables: &'a Tables,
    weight: f64,
    corrections: bool,
}

impl Choice for Fitting<'_> {
    fn candidates(&self) -> usize {
        24
    }

    fn truth(&self) -> usize {
        self.case.truth.index()
    }

    fn weight(&self) -> f64 {
        self.weight
    }

    fn features(&self, k: usize, each: &mut dyn FnMut(usize, f64)) {
        let Some(key) = Key::ALL.get(k) else { return };
        let mut trust = [0.0; STREAMS];
        self.case.ev.features(*key, &mut |i, v| {
            if i >= MINOR {
                each(STREAMS, v);
                return;
            }
            let (s, m, d) = (i / 24, i % 24 / 12, i % 12);
            trust[s] += self.tables.0[s][m][d] * v;
            if self.corrections {
                each(STREAMS + 1 + i, v);
            }
        });
        for (s, t) in trust.iter().enumerate() {
            if *t != 0.0 {
                each(s, *t);
            }
        }
    }
}

/// A fitted finder: one weight per feature, as `weights.rs` keeps them.
struct Model {
    weights: [f64; FEATURES],
}

impl Model {
    /// The weights from the fitted numbers: each stream's trust times its table, plus the corrections.
    fn of(fitted: &[f64], tables: &Tables, corrections: bool) -> Model {
        let at = |i: usize| fitted.get(i).copied().unwrap_or(0.0);
        let weights = std::array::from_fn(|i| {
            if i >= MINOR {
                return at(STREAMS);
            }
            let (s, m, d) = (i / 24, i % 24 / 12, i % 12);
            at(s) * tables.0[s][m][d] + if corrections { at(STREAMS + 1 + i) } else { 0.0 }
        });
        Model { weights }
    }

    /// Every key's score, as `keyfind::score` works it out.
    fn scores(&self, ev: &Evidence) -> [f64; 24] {
        Key::ALL.map(|k| {
            let mut s = 0.0;
            ev.features(k, &mut |i, v| s += v * self.weights.get(i).copied().unwrap_or(0.0));
            s
        })
    }
}

/// What a run is told on the command line.
struct Options {
    /// The penalty on the corrections; `None` fits the tables' trust weights alone.
    lambda_delta: Option<f64>,
    /// The shortest opening, whose first bass the loop table is counted from.
    loop_cut: Cut,
}

/// How much each set of cases counts in a fit or a calibration, in total.
fn set_weight(cut: Cut, input: Input) -> f64 {
    if cut == Cut::Song && input == Input::Full { SONG_WEIGHT } else { 1.0 }
}

/// Each case's share of its set's weight, over the cases `keep` takes (0 for the rest).
fn case_weights(cases: &[Case], keep: impl Fn(&Case) -> bool) -> Vec<f64> {
    let mut sizes: HashMap<(Cut, Input), usize> = HashMap::new();
    for c in cases.iter().filter(|c| keep(c)) {
        *sizes.entry((c.cut, c.input)).or_default() += 1;
    }
    cases
        .iter()
        .map(|c| {
            if keep(c) {
                set_weight(c.cut, c.input) / sizes.get(&(c.cut, c.input)).copied().unwrap_or(1).max(1) as f64
            } else {
                0.0
            }
        })
        .collect()
}

/// Which songs a fit learns from.
type Training<'a> = &'a (dyn Fn(usize) -> bool + Sync);

/// Fits a finder on the songs `train` takes: the tables from all of them, and the weights with each song's features
/// made from tables counted without its inner fold. Also gives the fit and how many cases it was fitted on.
fn fit_model(
    cases: &[Case],
    songs: &[Song],
    train: Training,
    opts: &Options,
) -> Result<(Model, fit::Fitted, usize), String> {
    let corrections = opts.lambda_delta.is_some();
    let all = Tables::count(cases, opts.loop_cut, train);
    let inner: Vec<Tables> = (0..INNER as usize)
        .map(|i| Tables::count(cases, opts.loop_cut, |s| train(s) && songs.get(s).is_some_and(|g| g.inner != i)))
        .collect();
    let fitted_on = |c: &Case| train(c.song) && c.one_key && c.pick;
    let weights = case_weights(cases, fitted_on);
    let choices: Vec<Fitting> = cases
        .iter()
        .zip(&weights)
        .filter(|(c, _)| fitted_on(c))
        .filter_map(|(case, &weight)| {
            let tables = inner.get(songs.get(case.song)?.inner)?;
            Some(Fitting { case, tables, weight, corrections })
        })
        .collect();
    let mut l2 = vec![LAMBDA_W; STREAMS + 1];
    if let Some(ld) = opts.lambda_delta {
        l2.extend(std::iter::repeat_n(ld, MINOR));
    }
    let fitted = fit::fit(&choices, l2.len(), &l2)?;
    Ok((Model::of(&fitted.weights, &all, corrections), fitted, choices.len()))
}

/// The average surprise (negative log-likelihood) of the labelled keys under scores, weighted.
fn weighted_surprise(cases: &[Case], scores: &[Option<[f64; 24]>], weights: &[f64]) -> f64 {
    let (mut total, mut loss) = (0.0, 0.0);
    for ((c, s), w) in cases.iter().zip(scores).zip(weights) {
        if let Some(s) = s
            && *w > 0.0
        {
            total += w;
            loss += w * fit::surprise(s, c.truth.index());
        }
    }
    if total > 0.0 { loss / total } else { 0.0 }
}

/// Scores for the cases of some folds, each from a finder fitted on those folds without its own (`None` for the rest).
type Held = Vec<Option<[f64; 24]>>;

/// Each case's scores from a finder fitted without its fold, for the cases in `folds` (fitting on those folds only).
fn cross_validate(
    cases: &[Case],
    songs: &[Song],
    folds: &[usize],
    opts: &Options,
) -> Result<(Held, Vec<(usize, Model)>), String> {
    let fold = |s: usize| songs.get(s).map_or(usize::MAX, |g| g.fold);
    let mut held: Held = vec![None; cases.len()];
    let mut models = Vec::with_capacity(folds.len());
    for &f in folds {
        let train = |s: usize| fold(s) != f && folds.contains(&fold(s));
        let (model, fitted, n) = fit_model(cases, songs, &train, opts)?;
        held.par_iter_mut()
            .zip(cases)
            .filter(|(_, c)| fold(c.song) == f)
            .for_each(|(h, c)| *h = Some(model.scores(&c.ev)));
        let weights = case_weights(cases, |c| fold(c.song) == f && c.one_key && c.pick);
        println!(
            "  fold {f}: fitted on {n} cases, {} Newton steps; loss {:.4} fitting, {:.4} on the fold held out",
            fitted.steps,
            fitted.loss,
            weighted_surprise(cases, &held, &weights)
        );
        models.push((f, model));
    }
    Ok((held, models))
}

/// The calibration fitted on held-out scores of the one-key cases `keep` takes, each set counting as in the fit.
fn calibrate(cases: &[Case], held: &[Option<[f64; 24]>], keep: impl Fn(&Case) -> bool) -> Result<Calibration, String> {
    let weights = case_weights(cases, |c| keep(c) && c.one_key);
    let scored: Vec<Scored> = cases
        .iter()
        .zip(held)
        .zip(&weights)
        .filter(|(_, w)| **w > 0.0)
        .filter_map(|((c, s), w)| {
            Some(Scored {
                scores: s.as_ref()?.to_vec(),
                truth: c.truth.index(),
                weight: *w,
                terms: Calibration::terms(c.ev.bars, c.ev.config, c.cut.span()).to_vec(),
            })
        })
        .collect();
    let numbers = fit::calibration(&scored, CALIBRATION_TERMS)?;
    let mut c = [0.0; CALIBRATION_TERMS];
    for (slot, v) in c.iter_mut().zip(numbers) {
        *slot = v;
    }
    Ok(Calibration(c))
}

/// The chances a calibration gives a case's scores.
fn chances_of(case: &Case, scores: &[f64; 24], calibration: &Calibration) -> [f64; 24] {
    let sharp = calibration.sharpness(case.ev.bars, case.ev.config, case.cut.span());
    keyfind::chances(scores.map(|s| s * sharp))
}

fn print_calibration(title: &str, c: &Calibration) {
    let n = |i: usize| c.0.get(i).copied().unwrap_or(0.0);
    println!(
        "{title}: at 1, 2, 4, 8, 16 bars {}; melody only {:+.3}, bass only {:+.3}, accompaniment {:+.3}; a loop adds \
         {}; a song adds {:+.3}.",
        (0..keyfind::KNOTS).map(|i| format!("{:.3}", n(i))).collect::<Vec<_>>().join(", "),
        n(keyfind::KNOTS),
        n(keyfind::KNOTS + 1),
        n(keyfind::KNOTS + 2),
        (0..keyfind::KNOTS).map(|i| format!("{:+.3}", n(keyfind::KNOTS + 3 + i))).collect::<Vec<_>>().join(", "),
        n(2 * keyfind::KNOTS + 3),
    );
}

/// The thresholds and the share of notes in one scale fitted on the chances of the one-key cases `keep` takes.
fn fit_answers(cases: &[Case], chances: &[Option<[f64; 24]>], keep: impl Fn(&Case) -> bool) -> Fitted {
    let graded: Vec<answerfit::Graded> = cases
        .iter()
        .zip(chances)
        .filter(|(c, _)| keep(c) && c.one_key)
        .filter_map(|(c, chances)| {
            Some(answerfit::Graded {
                song: c.song,
                truth: c.truth,
                knot: answerfit::knot_of(c.ev.bars, c.cut == Cut::Song),
                notes: c.ev.notes,
                diatonic_fit: c.ev.diatonic_fit,
                chances: (*chances)?,
            })
        })
        .collect();
    answerfit::fit(&graded, keyfind::SHIPPED.not_tonal_below)
}

fn print_answers(title: &str, fitted: &Fitted) {
    println!("{title}:");
    println!("  {}", fitted.scale_line);
    for line in &fitted.knot_lines {
        println!("    {line}");
    }
}

/// A finder with these weights, this calibration and the answers fitted.
fn finder<'a>(weights: &'a [f64; FEATURES], calibration: &'a Calibration, answers: &'a Fitted) -> keyfind::Model<'a> {
    keyfind::Model { weights, calibration, thresholds: &answers.thresholds, not_tonal_below: answers.not_tonal_below }
}

/// Each case's answer as the app would show it, with the thresholds `answers_of` gives its case.
fn answers_of<'a>(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    answers_of: impl Fn(&Case) -> Option<&'a Fitted> + Sync,
) -> Vec<Option<(KeyAnswer, f64)>> {
    cases
        .par_iter()
        .zip(chances)
        .map(|(c, chances)| Some(graded_answer(c, chances.as_ref()?, answers_of(c)?)))
        .collect()
}

/// For each fold, the thresholds fitted on the other folds' chances (of those `folds` lists), to grade it with.
fn fitted_per_fold(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    folds: &[usize],
    fold: &(dyn Fn(&Case) -> usize + Sync),
) -> HashMap<usize, Fitted> {
    folds.iter().map(|&f| (f, fit_answers(cases, chances, |c| fold(c) != f && folds.contains(&fold(c))))).collect()
}

pub fn run(args: &[String]) -> Result<(), String> {
    let setup = keys::setup(
        args,
        &["--lambda-delta", "--write", "--picks", "--inputs", "--dump", "--against", "--hand"],
        &["--final"],
        &LENGTHS,
    )?;
    if !matches!(setup.parts, keys::Parts::Pitched) {
        return Err("keyfit hears every kind of input itself (--inputs); --part is for `keys`".into());
    }
    let mut lambda_delta = Some(LAMBDA_DELTA);
    let mut write: Option<PathBuf> = None;
    let mut picks = PICKS;
    let mut inputs = Input::ALL.to_vec();
    let mut dump: Option<PathBuf> = None;
    let mut before: Option<PathBuf> = None;
    let mut show: Vec<String> = Vec::new();
    for pair in setup.rest.chunks(2) {
        match pair {
            [o, v] if o == "--lambda-delta" => {
                lambda_delta = if v == "inf" {
                    None
                } else {
                    let ld = v.parse().ok().filter(|&x: &f64| x > 0.0);
                    Some(ld.ok_or("--lambda-delta needs a number above 0, or inf")?)
                };
            }
            [o, v] if o == "--write" => write = Some(PathBuf::from(v)),
            [o, v] if o == "--picks" => picks = v.parse().map_err(|_| "--picks needs a whole number")?,
            [o, v] if o == "--dump" => dump = Some(PathBuf::from(v)),
            [o, v] if o == "--against" => before = Some(PathBuf::from(v)),
            [o, v] if o == "--hand" => show = v.split(',').map(|n| n.trim().to_owned()).collect(),
            [o, v] if o == "--inputs" => {
                inputs = v
                    .split(',')
                    .map(|i| {
                        Input::parse(i.trim()).ok_or_else(|| format!("--inputs: {i}? (full, tune, upper, bass, piano)"))
                    })
                    .collect::<Result<_, _>>()?;
                if !inputs.contains(&Input::Full) {
                    return Err("--inputs needs full: the tables are counted from the full mix".into());
                }
            }
            _ => return Err(crate::USAGE.to_owned()),
        }
    }
    let final_run = setup.flags.iter().any(|f| f == "--final");
    if final_run && write.is_some() {
        return Err("--final scores the kept-back fold; --write fits on every song. One at a time.".into());
    }
    if (final_run || write.is_some()) && (dump.is_some() || before.is_some()) {
        return Err(
            "--dump and --against compare runs on folds 1 to 4, for choosing; not with --final or --write".into()
        );
    }
    // Read before this run writes its own dump, which may go to the same file.
    let before = before
        .map(|path| {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok::<_, String>((path.display().to_string(), text))
        })
        .transpose()?;
    let mut lengths = setup.lengths.clone();
    lengths.sort_unstable();
    lengths.dedup();
    let loop_cut = Cut::Opening(lengths.first().copied().unwrap_or(1));
    let opts = Options { lambda_delta, loop_cut };

    let songs = songs_of(&setup.items);
    let cutting = Cutting { lengths: &lengths, inputs: &inputs, picks };
    let cases: Vec<Case> = setup
        .items
        .par_iter()
        .zip(&songs)
        .enumerate()
        .flat_map_iter(|(i, (item, song))| song_cases(item, i, &song.name, &cutting))
        .collect();
    let per_fold: Vec<usize> = (0..FOLDS as usize).map(|f| songs.iter().filter(|s| s.fold == f).count()).collect();
    println!(
        "{} cases ({} kinds of input); songs in folds 0 to 4: {}. Fold {FROZEN} is kept back for --final.",
        cases.len(),
        inputs.len(),
        per_fold.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
    );
    println!();
    match opts.lambda_delta {
        Some(ld) => println!(
            "Fitting {STREAMS} trust weights and the minor weight (penalty {LAMBDA_W}) and {MINOR} corrections \
             (penalty {ld})."
        ),
        None => println!("Fitting {STREAMS} trust weights and the minor weight (penalty {LAMBDA_W}); no corrections."),
    }

    let fold = |c: &Case| songs.get(c.song).map_or(usize::MAX, |s| s.fold);
    let design: Vec<usize> = (0..FOLDS as usize).filter(|&f| f != FROZEN).collect();
    let every: Vec<usize> = (0..FOLDS as usize).collect();

    if final_run {
        println!("Choosing the calibration and the answers' thresholds on folds 1 to 4:");
        let (held, _) = cross_validate(&cases, &songs, &design, &opts)?;
        let calibration = calibrate(&cases, &held, |c| design.contains(&fold(c)))?;
        print_calibration("Calibration", &calibration);
        let design_chances: Held =
            cases.par_iter().zip(&held).map(|(c, h)| h.as_ref().map(|s| chances_of(c, s, &calibration))).collect();
        let fitted_answers = fit_answers(&cases, &design_chances, |c| design.contains(&fold(c)));
        print_answers("Answers", &fitted_answers);
        let train = |s: usize| songs.get(s).is_some_and(|g| g.fold != FROZEN);
        let (model, fitted, n) = fit_model(&cases, &songs, &train, &opts)?;
        println!(
            "Fitted on folds 1 to 4: {n} cases, {} Newton steps. Scoring fold {FROZEN}, kept back until now.",
            fitted.steps
        );
        let chances: Held = cases
            .par_iter()
            .map(|c| (fold(c) == FROZEN).then(|| chances_of(c, &model.scores(&c.ev), &calibration)))
            .collect();
        let answers = answers_of(&cases, &chances, |_| Some(&fitted_answers));
        let summary = report(&cases, &chances, &answers, &songs, |c| fold(c) == FROZEN, &inputs);
        log_run("--final", &summary, &opts)?;
        let finder = finder(&model.weights, &calibration, &fitted_answers);
        hand_cases(&[("fitted on folds 1 to 4".to_owned(), finder)], &show)?;
    } else if let Some(path) = write {
        println!("Every fold held out in turn (fold {FROZEN} included: these weights are the ones that ship):");
        let (held, _) = cross_validate(&cases, &songs, &every, &opts)?;
        let calibration = calibrate(&cases, &held, |_| true)?;
        print_calibration("Calibration", &calibration);
        let chances: Held =
            cases.par_iter().zip(&held).map(|(c, h)| h.as_ref().map(|s| chances_of(c, s, &calibration))).collect();
        let fitted_answers = fit_answers(&cases, &chances, |_| true);
        print_answers(
            "Answers (the ones written; each fold below is graded with those fitted on the others)",
            &fitted_answers,
        );
        let per_fold = fitted_per_fold(&cases, &chances, &every, &fold);
        let answers = answers_of(&cases, &chances, |c| per_fold.get(&fold(c)));
        let summary = report(&cases, &chances, &answers, &songs, |_| true, &inputs);
        log_run("--write", &summary, &opts)?;
        let (model, fitted, n) = fit_model(&cases, &songs, &|_| true, &opts)?;
        println!();
        println!("Fitted on every song: {n} cases, {} Newton steps.", fitted.steps);
        let shipped = finder(&model.weights, &calibration, &fitted_answers);
        if !hand_cases(&[("fitted on every song".to_owned(), shipped)], &show)? {
            return Err("these weights break a hand case's gate (above), so they aren't written".into());
        }
        let sources: Vec<&str> = setup.items.iter().map(|i| i.source.as_str()).collect();
        std::fs::write(&path, weights_file(&model, &calibration, &fitted_answers, &opts, &sources, &summary))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        println!("Written to {}.", path.display());
    } else {
        println!("Folds 1 to 4, each held out in turn (fold {FROZEN} isn't touched):");
        let (held, models) = cross_validate(&cases, &songs, &design, &opts)?;
        // Each fold's chances come from a calibration fitted on the other folds' held-out scores.
        let mut calibrations = HashMap::new();
        for &f in &design {
            let c = calibrate(&cases, &held, |c| fold(c) != f && design.contains(&fold(c)))?;
            calibrations.insert(f, c);
        }
        let all = calibrate(&cases, &held, |c| design.contains(&fold(c)))?;
        print_calibration(
            "Calibration (on all four folds; each fold is scored with one fitted on the other three)",
            &all,
        );
        let chances: Held = cases
            .par_iter()
            .zip(&held)
            .map(|(c, h)| Some(chances_of(c, h.as_ref()?, calibrations.get(&fold(c))?)))
            .collect();
        // Each fold is graded with the thresholds fitted on the other folds' chances. Those printed are fitted on
        // all four folds' chances from the calibration printed above, as `--write` fits them.
        let fitted_answers = fitted_per_fold(&cases, &chances, &design, &fold);
        let chances_all: Held = cases
            .par_iter()
            .zip(&held)
            .map(|(c, h)| design.contains(&fold(c)).then(|| Some(chances_of(c, h.as_ref()?, &all))).flatten())
            .collect();
        print_answers(
            "Answers (fitted on all four folds; each fold is graded with those fitted on the other three)",
            &fit_answers(&cases, &chances_all, |c| design.contains(&fold(c))),
        );
        let answers = answers_of(&cases, &chances, |c| fitted_answers.get(&fold(c)));
        report(&cases, &chances, &answers, &songs, |c| design.contains(&fold(c)), &inputs);
        if let Some((name, text)) = &before {
            let was = read_dump(name, text, &cases, &songs)?;
            let worse = against(&cases, &chances, |c| design.contains(&fold(c)), &inputs, &was);
            if worse.is_empty() {
                println!("No set got worse beyond its range.");
            } else {
                println!("Worse beyond its range, or not comparable:");
                for line in worse {
                    println!("  {line}");
                }
            }
        }
        if let Some(path) = &dump {
            std::fs::write(path, dump_text(&cases, &chances, &songs, |c| design.contains(&fold(c))))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            println!();
            println!("Each case's outcome written to {}.", path.display());
        }
        let finders: Vec<(String, keyfind::Model)> = models
            .iter()
            .filter_map(|(f, m)| {
                Some((format!("fold {f} held out"), finder(&m.weights, calibrations.get(f)?, fitted_answers.get(f)?)))
            })
            .collect();
        hand_cases(&finders, &show)?;
    }
    Ok(())
}

/// The cases of one set heard as one kind of input, among those `keep` takes, in the run's order.
fn members(cases: &[Case], keep: &dyn Fn(&Case) -> bool, cut: Cut, one_key: Option<bool>, input: Input) -> Vec<usize> {
    cases
        .iter()
        .enumerate()
        .filter(|(_, c)| keep(c) && c.cut == cut && c.input == input && one_key.is_none_or(|o| c.one_key == o))
        .map(|(i, _)| i)
        .collect()
}

/// How a case came out: whether the likeliest key is the one labelled, and the log-loss (the chance given to the key
/// labelled, as -ln).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Outcome {
    right: bool,
    nll: f64,
}

impl Outcome {
    fn of(case: &Case, chances: &[f64; 24]) -> Outcome {
        let p = chances.get(case.truth.index()).copied().unwrap_or(0.0);
        Outcome { right: keyfind::best(chances).0 == case.truth, nll: -p.max(1e-12).ln() }
    }
}

/// What a case is, as a dump says it: its song, cut and kind of input (with its place in the run, enough to tell a
/// dump made with other cases).
fn identity(case: &Case, songs: &[Song]) -> String {
    let song = songs.get(case.song).map_or("?", |s| s.name.as_str());
    format!("{song}\t{}\t{}", case.cut.name(), case.input.name())
}

/// How each case `keep` takes came out, as `--dump` writes it: a line each, with its place in the run and what it is.
fn dump_text(cases: &[Case], chances: &[Option<[f64; 24]>], songs: &[Song], keep: impl Fn(&Case) -> bool) -> String {
    let mut out = String::from("# theorybench keyfit --dump: case, song, cut, input, right (1) or not (0), log-loss\n");
    for (i, (case, chances)) in cases.iter().zip(chances).enumerate() {
        if let Some(chances) = chances.as_ref().filter(|_| keep(case)) {
            let o = Outcome::of(case, chances);
            let _ = writeln!(out, "{i}\t{}\t{}\t{:?}", identity(case, songs), u8::from(o.right), o.nll);
        }
    }
    out
}

/// A run's outcomes read back from its dump (`--against`), by place in this run; refused if the dump's cases aren't
/// this run's (other --bars or --inputs, or other music). Cases missing from it are named by the comparison.
struct Dump {
    name: String,
    outcomes: Vec<Option<Outcome>>,
}

fn read_dump(name: &str, text: &str, cases: &[Case], songs: &[Song]) -> Result<Dump, String> {
    let mut outcomes = vec![None; cases.len()];
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#')) {
        let bad = || format!("{name} line {}: not a line --dump writes", n + 1);
        let fields: Vec<&str> = line.split('\t').collect();
        let [i, song, cut, input, right, nll] = fields.as_slice() else { return Err(bad()) };
        let i: usize = i.parse().map_err(|_| bad())?;
        let case = cases.get(i).ok_or_else(|| format!("{name}: case {i} isn't in this run"))?;
        if identity(case, songs) != format!("{song}\t{cut}\t{input}") {
            return Err(format!(
                "{name}: case {i} is {song}, {cut}, {input} there but {} here; was it made with other --bars or --inputs?",
                identity(case, songs).replace('\t', ", ")
            ));
        }
        let right = match *right {
            "1" => true,
            "0" => false,
            _ => return Err(bad()),
        };
        let nll: f64 = nll.parse().map_err(|_| bad())?;
        match outcomes.get_mut(i) {
            Some(slot @ None) => *slot = Some(Outcome { right, nll }),
            _ => return Err(format!("{name} line {}: case {i} is there twice", n + 1)),
        }
    }
    Ok(Dump { name: name.to_owned(), outcomes })
}

/// Prints how this run compares with a run dumped before (`--against`), set by set and input by input: the share
/// right and the log-loss before and now, and each difference with its likely range (each case paired with itself,
/// songs drawn again). Gives every set that got worse beyond its range, which a change being chosen mustn't do.
fn against(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    keep: impl Fn(&Case) -> bool,
    inputs: &[Input],
    was: &Dump,
) -> Vec<String> {
    println!();
    println!(
        "=== Against {} (each case in both runs paired with itself; ranges with the songs drawn again) ===",
        was.name
    );
    println!(
        "{:<30} {:<20} {:>7} {:>7} {:>7}   {:<22} {:>7} {:>7}   difference (range)",
        "", "", "cases", "before", "now", "difference (range)", "NLL was", "now"
    );
    let mut worse = Vec::new();
    for (cut, one_key) in sets(cases, &keep) {
        for &input in inputs {
            let members = members(cases, &keep, cut, one_key, input);
            let pairs: Vec<(usize, Outcome, Outcome)> = members
                .iter()
                .filter_map(|&i| {
                    let case = cases.get(i)?;
                    let now = Outcome::of(case, chances.get(i)?.as_ref()?);
                    Some((case.song, was.outcomes.get(i).copied().flatten()?, now))
                })
                .collect();
            let title = set_title(cut, one_key);
            // A case with an outcome in only one run (a dump cut short, or a finder that now has no chances there)
            // can't be paired: say so, rather than compare the rest as if they were all.
            if pairs.len() < members.len() {
                worse.push(format!(
                    "{title}, {}: {} of {} cases not in both runs, left out",
                    input.name(),
                    members.len() - pairs.len(),
                    members.len()
                ));
            }
            if pairs.is_empty() {
                continue;
            }
            let song: Vec<usize> = pairs.iter().map(|p| p.0).collect();
            let right = |f: fn(&(usize, Outcome, Outcome)) -> bool| pairs.iter().map(f).collect::<Vec<bool>>();
            let (before, now) = (right(|p| p.1.right), right(|p| p.2.right));
            let share = |r: &[bool]| r.iter().filter(|r| **r).count() as f64 / r.len() as f64;
            let p = metrics::paired(&before, &now, &song);
            let nll = |f: fn(&(usize, Outcome, Outcome)) -> f64| pairs.iter().map(f).sum::<f64>() / pairs.len() as f64;
            let diff: Vec<f64> = pairs.iter().map(|(_, b, n)| n.nll - b.nll).collect();
            let d = metrics::mean(&diff, &song);
            println!(
                "{:<30} {:<20} {:>7} {:>6.1}% {:>6.1}%   {:<22} {:>7.3} {:>7.3}   {:+.3} ({:+.3} to {:+.3})",
                title,
                input.name(),
                pairs.len(),
                100.0 * share(&before),
                100.0 * share(&now),
                format!("{:+.1} ({:+.1} to {:+.1})", 100.0 * p.diff, 100.0 * p.low, 100.0 * p.high),
                nll(|p| p.1.nll),
                nll(|p| p.2.nll),
                d.value,
                d.low,
                d.high,
            );
            if p.high < 0.0 {
                worse.push(format!(
                    "{title}, {}: {:+.1} points right ({:+.1} to {:+.1})",
                    input.name(),
                    100.0 * p.diff,
                    100.0 * p.low,
                    100.0 * p.high
                ));
            }
            if d.low > 0.0 {
                worse.push(format!(
                    "{title}, {}: log-loss {:+.3} ({:+.3} to {:+.3})",
                    input.name(),
                    d.value,
                    d.low,
                    d.high
                ));
            }
        }
    }
    worse
}

/// The sets a report is made of, in the order it prints them: whole songs in one key, those that change key, then
/// each cut by length.
fn sets(cases: &[Case], keep: &dyn Fn(&Case) -> bool) -> Vec<(Cut, Option<bool>)> {
    let mut cuts: Vec<Cut> =
        cases.iter().filter(|c| keep(c)).map(|c| c.cut).collect::<HashSet<_>>().into_iter().collect();
    cuts.sort();
    let mut out = Vec::new();
    for cut in cuts {
        if cut == Cut::Song {
            out.push((cut, Some(true)));
            out.push((cut, Some(false)));
        } else {
            out.push((cut, None));
        }
    }
    out
}

fn set_title(cut: Cut, one_key: Option<bool>) -> String {
    match one_key {
        Some(true) => "Whole songs with one key".to_owned(),
        Some(false) => "Whole songs that change key".to_owned(),
        None => cut.name(),
    }
}

/// What the report sums up, for the weights file and the log of final runs.
struct Summary {
    lines: Vec<String>,
    /// Each set's title, share right, and the C# app's share right, for the full mix.
    full: Vec<(String, f64, f64)>,
}

/// Prints the report for the cases `keep` takes, with the chances given.
fn report(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    answers: &[Option<(KeyAnswer, f64)>],
    songs: &[Song],
    keep: impl Fn(&Case) -> bool,
    inputs: &[Input],
) -> Summary {
    let mut summary = Summary { lines: Vec::new(), full: Vec::new() };
    let mut compact = Vec::new();
    let right = |answers: &[Answer]| answers.iter().filter(|a| a.right()).count() as f64 / answers.len().max(1) as f64;
    for (cut, one_key) in sets(cases, &keep) {
        for &input in inputs {
            let members = members(cases, &keep, cut, one_key, input);
            if members.is_empty() {
                continue;
            }
            let song: Vec<usize> = members.iter().map(|&i| cases[i].song).collect();
            let mo: Vec<Answer> = members
                .iter()
                .map(|&i| Answer {
                    truth: cases[i].truth,
                    found: chances[i].map(|c| {
                        let (key, sure) = keyfind::best(&c);
                        let alone = Some(matches!(answers[i], Some((KeyAnswer::Key(_), _))));
                        Found { key, sure, chances: Some(c), alone }
                    }),
                })
                .collect();
            let cs: Vec<Answer> = members
                .iter()
                .map(|&i| Answer {
                    truth: cases[i].truth,
                    found: cases[i].cs.map(|(key, sure)| Found { key, sure, chances: None, alone: None }),
                })
                .collect();
            let title = set_title(cut, one_key);
            if input == Input::Full {
                let graded = graded(&members.iter().map(|&i| (&cases[i], answers[i])).collect::<Vec<_>>());
                let runs = [
                    Run {
                        name: "C# app (Songs page)".into(),
                        cut: Some(baseline::SONG_MIN_CONFIDENCE),
                        answers: cs.clone(),
                        micros: None,
                    },
                    Run { name: "mo-harmony (held out)".into(), cut: None, answers: mo.clone(), micros: None },
                ];
                keys::report(&title, "the key labelled (at the middle of a stretch)", &song, &runs);
                println!("{graded}");
                summary.lines.push(format!("{title}: {} right ({} cases)", pct(right(&mo)), mo.len()));
                summary.full.push((title.clone(), right(&mo), right(&cs)));
            }
            let cs_right: Vec<bool> = cs.iter().map(Answer::right).collect();
            let mo_right: Vec<bool> = mo.iter().map(Answer::right).collect();
            let p = metrics::paired(&cs_right, &mo_right, &song);
            let sure: Vec<(Option<f64>, bool)> = mo.iter().map(|a| (a.found.map(|f| f.sure), a.right())).collect();
            let nll =
                members.iter().filter_map(|&i| Some(Outcome::of(&cases[i], chances[i].as_ref()?).nll)).sum::<f64>()
                    / mo.len().max(1) as f64;
            let mean_sure = sure.iter().filter_map(|s| s.0).sum::<f64>() / mo.len().max(1) as f64;
            compact.push(format!(
                "{:<30} {:<20} {:>7} {:>7.1}% {:>7.1}% {:>+6.1} ({:+.1} to {:+.1}) {:>5.1}% {:>6.3} {:>6.3} {:>6.3}",
                title,
                input.name(),
                mo.len(),
                100.0 * right(&cs),
                100.0 * right(&mo),
                100.0 * p.diff,
                100.0 * p.low,
                100.0 * p.high,
                100.0 * mean_sure,
                nll,
                metrics::calibration_error_equal_mass(&sure, 15).unwrap_or(f64::NAN),
                metrics::risk_coverage_area(&sure).unwrap_or(f64::NAN),
            ));
        }
    }
    let used: HashSet<usize> = cases.iter().filter(|c| keep(c)).map(|c| c.song).collect();
    println!();
    println!("=== Every kind of input, from {} of the {} songs ===", used.len(), songs.len());
    // "sure" is the average chance mo-harmony gave its answers: above its share right, it was too sure.
    println!(
        "{:<30} {:<20} {:>7} {:>8} {:>8} {:>26} {:>6} {:>6} {:>6} {:>6}",
        "", "", "cases", "C# app", "mo-harm.", "difference (likely range)", "sure", "NLL", "ECE", "AURC"
    );
    for line in compact {
        println!("{line}");
    }
    summary
}

/// A case's answer as the app would show it, graded with these fitted thresholds, and the chance shown with it.
fn graded_answer(case: &Case, chances: &[f64; 24], answers: &Fitted) -> (KeyAnswer, f64) {
    let grading = keyfind::Grading {
        bars_heard: case.ev.bars,
        notes: case.ev.notes,
        diatonic_fit: case.ev.diatonic_fit,
        span: case.cut.span(),
    };
    // Only the thresholds and the share of notes in one scale grade; the weights are the ones built in.
    keyfind::grade(chances, grading, &finder(keyfind::SHIPPED.weights, keyfind::SHIPPED.calibration, answers))
}

/// How the answers would be shown (one key, a key or its relative, a home note, no clear key), how often each kind is
/// right (a pair when the labelled key is either, a home note when it's the tonic), and how sure the finder said it
/// was (the chance shown, on average): said above right, it was too sure.
fn graded(cases: &[(&Case, Option<(KeyAnswer, f64)>)]) -> String {
    // Given, right, chance shown; and for no clear key, why.
    let mut tally = [(0usize, 0usize, 0.0f64); 4];
    let mut why = [0usize; 3];
    for (case, answer) in cases {
        let Some((answer, shown)) = *answer else { continue };
        let kind = match answer {
            KeyAnswer::Key(_) => 0,
            KeyAnswer::OrRelative(_) => 1,
            KeyAnswer::HomeNote { .. } => 2,
            KeyAnswer::Unclear { why: w, .. } => {
                why[match w {
                    Why::Spread => 0,
                    Why::NotTonal => 1,
                    Why::TooFewNotes => 2,
                }] += 1;
                3
            }
        };
        let t = &mut tally[kind];
        t.0 += 1;
        t.1 += usize::from(answer.names(case.truth));
        t.2 += shown;
    }
    let total = tally.iter().map(|t| t.0).sum::<usize>().max(1) as f64;
    let share = |n: usize, of: f64| 100.0 * n as f64 / of.max(1.0);
    let names = ["one key", "key or its relative", "home note"];
    let mut parts: Vec<String> = names
        .iter()
        .zip(&tally)
        .map(|(name, &(n, right, said))| {
            if n == 0 {
                return format!("{name} 0%");
            }
            let of = n as f64;
            format!("{name} {:.1}% ({:.1}% right, said {:.1}%)", share(n, total), share(right, of), 100.0 * said / of)
        })
        .collect();
    let [spread, not_tonal, few] = why;
    parts.push(format!(
        "no clear key {:.1}% (spread {:.1}, not in one scale {:.1}, too few notes {:.1})",
        share(tally[3].0, total),
        share(spread, total),
        share(not_tonal, total),
        share(few, total)
    ));
    format!("  Shown as: {}.", parts.join(", "))
}

/// Reads the design's hand cases with each fitted model and prints how many models keep each case's gate and meet its
/// target, with what broke a gate. Returns whether every model keeps every gate.
fn hand_cases(models: &[(String, keyfind::Model)], show: &[String]) -> Result<bool, String> {
    let books: Vec<(&str, Book)> =
        models.iter().map(|(name, model)| Ok((name.as_str(), Book::read(model)?))).collect::<Result<_, String>>()?;
    let judged: Vec<(&str, Vec<Verdict>)> = books.iter().map(|(name, book)| (*name, book.verdicts())).collect();
    let n = judged.len();
    println!();
    println!(
        "=== The hand cases: gates (must hold) and targets (to reach), for {n} fitted model{} ===",
        if n == 1 { "" } else { "s" }
    );
    let mut every = true;
    let mut broken = Vec::new();
    // Every case the hand cases judge (each model judges the same ones).
    let cases: Vec<u8> = judged.first().map(|(_, vs)| vs.iter().map(|v| v.case).collect()).unwrap_or_default();
    for case in cases {
        let verdicts: Vec<(&str, &Verdict)> =
            judged.iter().filter_map(|(name, vs)| vs.iter().find(|v| v.case == case).map(|v| (*name, v))).collect();
        let Some((_, first)) = verdicts.first() else { continue };
        let gates = verdicts.iter().filter(|(_, v)| v.passes_gate()).count();
        let targets: Vec<bool> = verdicts.iter().filter_map(|(_, v)| v.meets_target()).collect();
        let has_gate = verdicts.iter().any(|(_, v)| v.gate.is_some());
        let gate = if has_gate { format!("gate {gates}/{n}") } else { "no gate".to_owned() };
        let target = if targets.is_empty() {
            "no target".to_owned()
        } else {
            format!("target {}/{n}", targets.iter().filter(|t| **t).count())
        };
        println!("{case:>3}  {gate:<10} {target:<11} {}", first.about);
        if gates < n {
            every = false;
            if let Some((name, v)) = verdicts.iter().find(|(_, v)| !v.passes_gate()) {
                broken.extend(v.broken().iter().map(|g| format!("  {name}: {g}")));
            }
        }
    }
    if !broken.is_empty() {
        println!("Gates broken (the first model to break each case):");
        for line in &broken {
            println!("{line}");
        }
    }
    for name in show {
        println!("Case {name} (--hand):");
        for (model, book) in &books {
            let read = book.readings.iter().find(|(c, _)| c.name == name.as_str()).map(|(_, r)| r);
            match read {
                None => println!("  {model}: no case of that name"),
                Some(None) => println!("  {model}: nothing pitched heard"),
                Some(Some((_, r))) => {
                    let top: Vec<String> = r.ranked().iter().take(4).map(|(k, p)| format!("{k} {p:.3}")).collect();
                    println!("  {model}: {}; {}", cases::describe(r.answer), top.join(", "));
                }
            }
        }
    }
    Ok(every)
}

/// Today's date, from the clock (UTC).
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    // Howard Hinnant's days-to-civil.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Adds a line to `test-runs.md` for a run that scored the kept-back fold.
/// Adds a line to `test-runs.md` for a run that showed fold 0: `--final`, or `--write` (which scores every fold
/// before it writes the weights).
fn log_run(run: &str, summary: &Summary, opts: &Options) -> Result<(), String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-runs.md");
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or("unknown".to_owned(), |o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        "# Runs on the kept-back fold\n\nEvery `theorybench keyfit --final` or `--write` run adds a line here: \
         fold 0 is only looked at these ways (`--final` scores it alone; `--write` scores every fold, fold 0 \
         included, before it writes the weights that ship), so this is how often it was.\n\n| Date | Run | Finder \
         version | Commit | Corrections penalty | Full mix: mo-harmony (C# app) |\n|---|---|---|---|---|---|\n"
            .to_owned()
    });
    let results: Vec<String> =
        summary.full.iter().map(|(t, r, c)| format!("{t} {:.1}% ({:.1}%)", 100.0 * r, 100.0 * c)).collect();
    let penalty = opts.lambda_delta.map_or("inf".to_owned(), |l| format!("{l}"));
    let _ = writeln!(
        text,
        "| {} | {run} | {KEY_FINDER_VERSION} | {commit} | {penalty} | {} |",
        today(),
        results.join("; ")
    );
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    println!();
    println!("Logged in {}.", path.display());
    Ok(())
}

/// The weights as Rust source for `crates/mo-harmony/src/keyfind/weights.rs`.
fn weights_file(
    model: &Model,
    calibration: &Calibration,
    answers: &Fitted,
    opts: &Options,
    sources: &[&str],
    summary: &Summary,
) -> String {
    let mut sources: Vec<&str> = sources.to_vec();
    sources.sort_unstable();
    sources.dedup();
    let penalty = opts.lambda_delta.map_or("none fitted".to_owned(), |l| format!("penalty {l}"));
    let mut out = String::new();
    out.push_str(
        "//! The key finder's weights. Made by `theorybench keyfit`; don't edit by hand, except the last two lines when\n\
         //! a test asks (another number the finder is built from changed).\n//!\n",
    );
    let _ = writeln!(
        out,
        "//! Fitted on: {} (trust weights penalty {LAMBDA_W}, corrections {penalty}).",
        sources.join(", ")
    );
    out.push_str(
        "//! POP909-CL: Yao, Chen, Dubnov and Berg-Kirkpatrick (2026), MIT licence; only these weights come from it.\n",
    );
    out.push_str("//!\n//! Right on songs held out of the fit (cross-validation by song, every fold):\n");
    for line in &summary.lines {
        let _ = writeln!(out, "//! - {line}");
    }
    out.push_str("\nuse super::{Calibration, FEATURES, Thresholds};\n\n");
    out.push_str("/// Whether these weights were fitted (`false` for the placeholder).\npub(super) const FITTED: bool = true;\n\n");
    out.push_str(
        "/// One weight per feature (see `Heard::features`): for each stream, twelve for the notes above a major\n",
    );
    out.push_str("/// key's tonic, then twelve for a minor key's; last, one for minor keys as a whole.\n");
    out.push_str("#[rustfmt::skip]\npub(super) const WEIGHTS: [f64; FEATURES] = [\n");
    for (s, stream) in Stream::ALL.iter().enumerate() {
        for (m, mode) in ["major", "minor"].iter().enumerate() {
            let _ = writeln!(out, "    // {stream:?}, {mode}: the tonic, then each semitone up.");
            let row: Vec<String> = (0..12)
                .map(|d| format!("{:?}", model.weights.get(s * 24 + m * 12 + d).copied().unwrap_or(0.0)))
                .collect();
            let _ = writeln!(out, "    {},", row.join(", "));
        }
    }
    let _ = writeln!(out, "    // Minor keys.\n    {:?},\n];", model.weights.get(MINOR).copied().unwrap_or(0.0));
    out.push_str(
        "/// How much the scores are sharpened (see `Calibration`): the knots at 1 to 16 bars; melody only, bass only,\n",
    );
    out.push_str("/// accompaniment; what a loop adds at 1 to 16 bars; what a song adds.\n");
    let numbers: Vec<String> = calibration.0.iter().map(|v| format!("{v:?}")).collect();
    let _ = writeln!(
        out,
        "#[rustfmt::skip]\npub(super) const CALIBRATION: Calibration = Calibration([{}]);",
        numbers.join(", ")
    );
    let knots = |k: &[f64]| k.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join(", ");
    out.push_str(
        "/// How likely each kind of answer must be (see `Thresholds`) at 1, 2, 4, 8, 16 and 32 bars, fitted on the chances\n",
    );
    out.push_str("/// of songs held out (`theorybench` `answerfit`):\n");
    for line in &answers.knot_lines {
        let _ = writeln!(out, "/// - {line}");
    }
    let t = &answers.thresholds;
    let _ = writeln!(
        out,
        "#[rustfmt::skip]\npub(super) const THRESHOLDS: Thresholds = Thresholds {{\n    one: [{}],\n    pair: [{}],\n    \
         home: [{}],\n}};",
        knots(&t.one),
        knots(&t.pair),
        knots(&t.home),
    );
    out.push_str(
        "/// Notes with less than this share of their time inside one scale have no key (see `Why::NotTonal`).\n",
    );
    let _ = writeln!(out, "/// {}", answers.scale_line);
    let _ = writeln!(out, "pub(super) const NOT_TONAL_BELOW: f64 = {:?};", answers.not_tonal_below);
    // The hash of every number, and the version they were first given with: the next one when any number changed.
    let hash = keyfind::numbers_hash(&finder(&model.weights, calibration, answers));
    let (shipped_hash, shipped_version) = keyfind::SHIPPED_HASH;
    let version = if hash == shipped_hash { shipped_version } else { KEY_FINDER_VERSION + 1 };
    out.push_str(
        "/// The hash of every number the finder is built from (see `numbers_hash`) when this file was written, and the version\n",
    );
    out.push_str(
        "/// of the finder these numbers were first given with: a test checks both, so changing any number fails until the\n",
    );
    out.push_str("/// version is bumped.\n");
    let _ = writeln!(out, "pub(super) const SHIPPED_HASH: u64 = {hash:#018x};");
    let _ = writeln!(out, "pub(super) const HASH_VERSION: u32 = {version};");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, length: i64, key: u8) -> Note {
        Note { start, length, key, velocity: 100 }
    }

    #[test]
    fn the_fit_reads_the_same_features_the_finder_scores_with() {
        let tune = [note(0, 96, 72), note(96, 192, 74), note(288, 96, 71)];
        let piano = [note(0, 384, 48), note(0, 384, 63), note(0, 384, 67), note(384, 384, 43), note(384, 384, 62)];
        let parts = vec![(Role::Melody, tune.to_vec()), (Role::Harmony, piano.to_vec())];
        let mut reader = KeyReader::new();
        let grid = Grid::four_four(96);
        for span in Span::ALL {
            let (ev, _) = hear(&mut reader, &parts, grid, 96, span).unwrap();
            let notes: Vec<Vec<mo_harmony::Note>> = parts
                .iter()
                .map(|(_, n)| {
                    n.iter()
                        .map(|n| mo_harmony::Note { start: n.start, length: n.length, key: n.key, velocity: 1 << 15 })
                        .collect()
                })
                .collect();
            let heard_parts: Vec<mo_harmony::Part> =
                parts.iter().zip(&notes).map(|((role, _), notes)| mo_harmony::Part { role: *role, notes }).collect();
            let heard = reader.hear(&heard_parts, grid).unwrap();
            for key in Key::ALL {
                let (mut ours, mut theirs) = (Vec::new(), Vec::new());
                ev.features(key, &mut |i, v| ours.push((i, v)));
                heard.features(key, span, |i, v| theirs.push((i, v)));
                assert_eq!(ours, theirs, "{key:?} {span:?}");
            }
            // And a fitted model scores as the crate does, to the last bit.
            let weights: [f64; FEATURES] = std::array::from_fn(|i| (i as f64 * 0.37).sin());
            let model = Model { weights };
            let scores = model.scores(&ev);
            for key in Key::ALL {
                assert_eq!(scores[key.index()].to_bits(), keyfind::score(&heard, key, span, &weights).to_bits());
            }
        }
    }

    #[test]
    fn the_piano_bass_is_the_lowest_note_standing_apart() {
        // C2 under a C chord, then a close E-G with nothing under it, then G1 under D2 (a fifth up).
        let notes = [
            note(0, 96, 36),
            note(0, 96, 60),
            note(0, 96, 64),
            note(96, 96, 64),
            note(96, 96, 67),
            note(192, 96, 31),
            note(192, 96, 38),
        ];
        let (bass, upper) = split_bass(&notes);
        assert_eq!(bass.iter().map(|n| n.key).collect::<Vec<_>>(), [36, 31]);
        assert_eq!(upper.len(), 5);
    }

    /// A whole song as a full mix, labelled C major, with no evidence (the comparison only reads chances).
    fn song_case(song: usize) -> Case {
        let ev = Evidence { x: [[0.0; 12]; STREAMS], bars: 8, config: Config::Full, diatonic_fit: 0.9, notes: 20 };
        let truth = Key::major(PitchClass::C);
        Case { song, truth, cut: Cut::Song, input: Input::Full, one_key: true, pick: true, ev, cs: None }
    }

    /// Chances giving C major `p` and A minor the rest.
    fn c_or_a(p: f64) -> Option<[f64; 24]> {
        let mut chances = [0.0; 24];
        chances[Key::major(PitchClass::C).index()] = p;
        chances[Key::minor(PitchClass::A).index()] = 1.0 - p;
        Some(chances)
    }

    #[test]
    fn a_dumped_run_is_read_back_case_by_case() {
        let songs: Vec<Song> = (0..3).map(|i| Song { name: format!("pop909/{i:03}"), fold: 1, inner: 0 }).collect();
        let cases: Vec<Case> = (0..3).map(song_case).collect();
        let chances = vec![c_or_a(0.9), c_or_a(0.2), None];
        let text = dump_text(&cases, &chances, &songs, |_| true);
        assert!(text.contains("0\tpop909/000\tWhole songs\tfull mix\t1\t"), "{text}");
        let was = read_dump("before.tsv", &text, &cases, &songs).unwrap();
        assert_eq!(was.outcomes[0], Some(Outcome { right: true, nll: -(0.9f64.ln()) }));
        assert_eq!(was.outcomes[1].map(|o| o.right), Some(false));
        assert_eq!(was.outcomes[2], None, "a case with no chances isn't written");
        // A dump made with other cases is refused, not compared.
        let other: Vec<Case> = (0..3).map(|i| Case { input: Input::Tune, ..song_case(i) }).collect();
        let refused = read_dump("before.tsv", &text, &other, &songs).err().unwrap();
        assert!(refused.contains("tune only here"), "{refused}");
        assert!(read_dump("before.tsv", "0\tpop909/000\tWhole songs\tfull mix\t2\t0.1\n", &cases, &songs).is_err());
        // Blank lines are skipped; a case written twice is refused.
        assert!(read_dump("before.tsv", &format!("\n{text}\n\n"), &cases, &songs).is_ok());
        let twice = read_dump("before.tsv", &format!("{text}{text}"), &cases, &songs).err().unwrap();
        assert!(twice.contains("case 0 is there twice"), "{twice}");
    }

    #[test]
    fn a_set_that_got_worse_beyond_its_range_is_named() {
        let songs: Vec<Song> = (0..60).map(|i| Song { name: format!("pop909/{i:03}"), fold: 1, inner: 0 }).collect();
        let cases: Vec<Case> = (0..60).map(song_case).collect();
        let sure: Vec<Option<[f64; 24]>> = (0..60).map(|_| c_or_a(0.9)).collect();
        let was = read_dump("before.tsv", &dump_text(&cases, &sure, &songs, |_| true), &cases, &songs).unwrap();
        // The same chances: nothing got worse.
        assert!(against(&cases, &sure, |_| true, &[Input::Full], &was).is_empty());
        // A third of the songs now wrong: the share right and the log-loss both got worse beyond their ranges.
        let worse: Vec<Option<[f64; 24]>> = (0..60).map(|i| c_or_a(if i % 3 == 0 { 0.3 } else { 0.9 })).collect();
        let named = against(&cases, &worse, |_| true, &[Input::Full], &was);
        assert_eq!(named.len(), 2, "{named:?}");
        assert!(named[0].starts_with("Whole songs with one key, full mix: -33.3 points right"), "{named:?}");
        assert!(named[1].contains("log-loss +"), "{named:?}");
        // A dump cut short: the cases it lacks are named, not quietly left out.
        let short: String =
            dump_text(&cases, &sure, &songs, |_| true).lines().take(41).map(|l| format!("{l}\n")).collect();
        let was = read_dump("before.tsv", &short, &cases, &songs).unwrap();
        let named = against(&cases, &sure, |_| true, &[Input::Full], &was);
        assert_eq!(named, ["Whole songs with one key, full mix: 20 of 60 cases not in both runs, left out"]);
    }

    #[test]
    fn folds_are_drawn_by_name() {
        // Fixed for good: changing the hash would move songs in and out of the kept-back fold.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn dates_are_written_year_first() {
        let today = today();
        assert!(today.len() == 10 && today.starts_with("20") && today.as_bytes()[4] == b'-');
    }

    #[test]
    fn the_weights_file_carries_the_version_its_numbers_belong_to() {
        let opts = Options { lambda_delta: Some(LAMBDA_DELTA), loop_cut: Cut::Opening(1) };
        let summary =
            Summary { lines: vec!["Whole songs with one key: 90.0% right (1 cases)".into()], full: Vec::new() };
        let calibration = *keyfind::SHIPPED.calibration;
        let answers = Fitted {
            thresholds: *keyfind::SHIPPED.thresholds,
            not_tonal_below: keyfind::SHIPPED.not_tonal_below,
            scale_line: "Notes in one scale a key needs: 0.750.".into(),
            knot_lines: vec!["1 bar (10 cases): one key from 0.85.".into()],
        };
        let file = |weights: [f64; FEATURES]| {
            weights_file(&Model { weights }, &calibration, &answers, &opts, &["x"], &summary)
        };
        // The numbers built in: the hash and version built in.
        let same = file(*keyfind::SHIPPED.weights);
        assert!(same.contains("/// - 1 bar (10 cases): one key from 0.85.\n"), "{same}");
        assert!(same.contains("/// Notes in one scale a key needs: 0.750.\npub(super) const NOT_TONAL_BELOW"));
        let (hash, version) = keyfind::SHIPPED_HASH;
        assert!(same.contains(&format!("SHIPPED_HASH: u64 = {hash:#018x};")), "{same}");
        assert!(same.contains(&format!("HASH_VERSION: u32 = {version};")));
        assert!(same.contains("pub(super) const THRESHOLDS: Thresholds = Thresholds {"));
        assert!(same.contains(&format!("NOT_TONAL_BELOW: f64 = {:?};", keyfind::SHIPPED.not_tonal_below)));
        // Any other number: the next version, so the tests fail until the crate's version is bumped too.
        let mut weights = *keyfind::SHIPPED.weights;
        weights[5] += 0.25;
        let changed = file(weights);
        assert!(changed.contains(&format!("HASH_VERSION: u32 = {};", KEY_FINDER_VERSION + 1)));
        assert!(!changed.contains(&format!("{hash:#018x}")));
    }
}
