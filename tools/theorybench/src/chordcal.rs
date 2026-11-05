//! `theorybench chordcal`: fits the chord reader's calibration (harmony/design/chord-reader-design.md, "Calibration,
//! the graded answer and alternatives for Chord check"): how sharp its chances are (σ, 8 numbers:
//! `chordread::Calibration`), then the thresholds its answers are graded by (`chordread::Thresholds`), so a chance
//! says how often the reader is right and each answer is wrong at most as often as its target.
//!
//! - **What it fits on.** The pieces whose licence lets fitted numbers ship ([`Item::may_learn`]) in the folds
//!   `--folds` names: fold 1 unless it says otherwise, the fold the weights built in were measured on and never learned
//!   from, so the chances are the reader's on music new to it (the design's cross-fitting, with the one fold the shipped
//!   weights left out). The folds are drawn as `chordfit` draws them, so give the same files. With `--measure` it fits
//!   nothing: it reads every piece given (any licence, any fold) with the calibration and thresholds built in (or a
//!   file `--out` wrote, `--calibration`) and says how true the chances are, as a check on music it never saw.
//! - **What is read.** Each piece every way `chordfit` hears it (POP909-CL's song four ways, voice.py's pieces as
//!   given), whole, and cut into stretches of 1, 2, 4, 8 and 16 bars along its bar lines as the MIDI page cuts a clip
//!   from a song (read as windows: [`Span::Window`]): `--picks` of each length from each piece (2), picked by a hash of
//!   the piece and the bar. The sharpness is a curve over how much music is heard, so the short stretches are what fit
//!   its first knots.
//! - **What is judged.** Each stretch the reader reads as a chord, against the chord labelled for most of it (no chord
//!   and labels the reader can't read are left out). The event is the chord's root and core (`chordfit`'s accept sets:
//!   on POP909-CL, whose labels are a template's triads, a major label also takes a maj7 or a 6, a minor one an m7 or
//!   an m6). Each stretch counts once.
//! - **The sharpness** is fitted by likelihood: the chance the readings give the labelled chord, on every stretch read
//!   (each piece read at a grid of sharpnesses, the 8 numbers then chosen by coordinate descent on that grid, a
//!   straight line between its points on a log scale). Then each piece is read once more with the numbers fitted.
//! - **The thresholds**, texture by texture (a full mix, an accompaniment, chords alone; one line alone is never one
//!   chord), are each the smallest on a 0.01 grid, from an even chance up, at which the answers of that kind are wrong
//!   no more often than their target ([`TARGET`]), judged by the upper end of the error's likely range when the songs
//!   are drawn again (95%, the songs' answers going wrong together counted): one chord first, wrong when its root and
//!   core aren't the label's; then the same notes on the stretches left, wrong when the label is outside the group.
//!   A texture with fewer than [`MIN_SONGS`] songs keeps the thresholds built in.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use mo_harmony::chordread::{
    CALIBRATION, CALIBRATION_TERMS, Calibration, ChordAnswer, ChordReading, Grading, Label, MOST_READINGS, Model,
    SHARPNESS_KNOTS, SHIPPED, SpanFlags, THRESHOLDS, Texture, Thresholds, Weights, grade,
};
use mo_harmony::{ChordContext, ChordReader, Grid, Role, Span};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::chordfit::{self, Input, Split};
use crate::item::{self, Item};
use crate::keyfit::{self, fnv1a};
use crate::keys;
use crate::metrics;

/// Most often an answer of each kind may be wrong: one chord, the label not its root and core; the same notes, the
/// label outside the group.
pub const TARGET: f64 = 0.10;
/// No answer is given below an even chance: what it names must be likelier than not.
pub const LEAST: f64 = 0.5;
/// The fewest songs a texture's thresholds may be judged on, and a texture's part of the sharpness fitted on.
pub const MIN_SONGS: usize = 30;
/// The lengths of the stretches cut from each piece, in bars: the sharpness's knots.
const LENGTHS: [i64; SHARPNESS_KNOTS] = [1, 2, 4, 8, 16];
/// Stretches of each length cut from each piece, unless `--picks` says otherwise.
const PICKS: usize = 2;
/// The sharpnesses each piece is read at to fit the curve: from 0.1 to about 4, each 6% above the one before.
const GRID_LOW: f64 = 0.1;
const GRID_STEP: f64 = 1.06;
const GRID_POINTS: usize = 64;
/// The least chance a labelled chord is given when it's none of the readings kept: what the readings leave, but never
/// less than this (a reading the reader can't name, or one under the last kept).
const FLOOR: f64 = 1e-4;
/// The bins the calibration error is worked out over (equal numbers of stretches in each).
const BINS: usize = 15;

/// The textures, in `Texture::ALL`'s order, as the report names them.
const TEXTURES: [&str; 4] = ["full mix", "accompaniment", "chords alone", "one line"];

/// One piece heard one way, whole or a stretch of it, ready to read.
struct Take {
    /// The song it's from (for the likely ranges): the ways a song is heard and its stretches share one.
    song: usize,
    source: String,
    input: Input,
    /// None for the whole piece, or the stretch's length in bars.
    cut: Option<i64>,
    parts: Vec<(Role, Vec<mo_harmony::Note>)>,
    grid: Grid,
    span: Span,
    /// The labelled chords, in the take's own ticks.
    labels: Arc<[(i64, i64, String)]>,
    lenient: bool,
}

/// What a take's stretch read as a chord says: how long it is, its readings' chances (with their labels and groups,
/// best first), and what's left; whether its first reading is rootless with no bass; and the labelled chord's accept
/// set.
#[derive(Clone, Debug)]
struct Seen {
    readings: Vec<(Label, f64, u8)>,
    other: f64,
    rootless_with_no_bass: bool,
    accepts: Vec<Label>,
}

impl Seen {
    /// The chance the readings give the labelled chord.
    fn truth(&self) -> f64 {
        let found: f64 = self.readings.iter().filter(|(l, _, _)| self.accepts.contains(l)).map(|(_, p, _)| p).sum();
        if found > 0.0 { found } else { self.other.max(FLOOR) }
    }

    fn first(&self) -> Option<(Label, f64, u8)> {
        self.readings.first().copied()
    }

    /// Whether the first reading is the labelled chord.
    fn right(&self) -> bool {
        self.first().is_some_and(|(l, _, _)| self.accepts.contains(&l))
    }

    /// The first reading's same-notes group: how many readings, their chances added up, and whether the labelled
    /// chord is among them.
    fn group(&self) -> (usize, f64, bool) {
        let Some((_, _, g)) = self.first() else { return (0, 0.0, false) };
        let members = || self.readings.iter().filter(move |r| r.2 == g);
        (members().count(), members().map(|r| r.1).sum(), members().any(|r| self.accepts.contains(&r.0)))
    }

    /// The chance of the first `k` readings, and whether the labelled chord is among them.
    fn top(&self, k: usize) -> (f64, bool) {
        let kept = self.readings.iter().take(k);
        (kept.clone().map(|r| r.1).sum(), kept.clone().any(|r| self.accepts.contains(&r.0)))
    }

    /// The answer its readings are graded to with these thresholds in this texture.
    fn answer(&self, texture: Texture, thresholds: &Thresholds) -> ChordAnswer {
        let (group, group_chance, _) = self.group();
        let g = Grading {
            rest: false,
            given: false,
            heard: 2,
            decoded: true,
            first: self.first().map_or(0.0, |r| r.1),
            rootless_with_no_bass: self.rootless_with_no_bass,
            group,
            group_chance,
            texture,
        };
        grade(g, thresholds)
    }
}

/// A take read: its texture, the bars heard, and each stretch judged.
struct Read {
    texture: Texture,
    bars: f64,
    seen: Vec<Seen>,
}

/// The stretches of a reading judged against the labels: each stretch read as a chord with readings, against the
/// label of most of its time, when the label is a chord the reader can read.
fn judge(reading: &ChordReading, labels: &[(i64, i64, String)], lenient: bool) -> Vec<Seen> {
    reading
        .spans
        .iter()
        .filter(|s| s.label.is_some() && !s.readings.is_empty())
        .filter_map(|s| {
            let overlap = |(a, b): (i64, i64)| (b.min(s.end) - a.max(s.start)).max(0);
            let most = labels.iter().filter(|l| overlap((l.0, l.1)) > 0).max_by_key(|l| overlap((l.0, l.1)))?;
            let accepts = chordfit::accepts(&most.2, lenient);
            (!accepts.is_empty()).then(|| Seen {
                readings: s.readings.iter().map(|r| (r.label, r.chance, r.group)).collect(),
                other: s.other,
                rootless_with_no_bass: s.readings[0].rootless && s.flags.contains(SpanFlags::NO_BASS),
                accepts,
            })
        })
        .collect()
}

/// The notes starting in a stretch, moved to start at 0 and cut off at its end ([`item::cut`]).
fn cut_notes(notes: &[mo_harmony::Note], from: i64, length: i64) -> Vec<mo_harmony::Note> {
    notes
        .iter()
        .filter(|n| n.start >= from && n.start < from + length)
        .map(|n| mo_harmony::Note { start: n.start - from, length: n.length.min(from + length - n.start).max(1), ..*n })
        .collect()
}

/// The labels over a stretch, moved to start at 0 and cut to it.
fn cut_labels(labels: &[(i64, i64, String)], from: i64, length: i64) -> Vec<(i64, i64, String)> {
    labels
        .iter()
        .filter(|(s, e, _)| *s < from + length && *e > from)
        .map(|(s, e, t)| ((*s).max(from) - from, (*e).min(from + length) - from, t.clone()))
        .collect()
}

/// Every take from the pieces `keep` keeps: each heard every way `chordfit` hears it, whole and cut.
fn takes_of(items: &[Item], keep: impl Fn(usize) -> bool + Sync, picks: usize) -> Vec<Take> {
    let per_item: Vec<Vec<Take>> = items
        .par_iter()
        .enumerate()
        .map(|(song, item)| {
            if !keep(song) {
                return Vec::new();
            }
            let inputs = Input::of(item, &Input::SONG);
            let split = inputs.iter().any(|i| i.splits()).then(|| Split::of(item));
            let labels: Arc<[(i64, i64, String)]> = item.chords.clone().into();
            let lenient = chordfit::lenient(item);
            let (bar, origin) = (item.bar(), item.grid_origin());
            let mut out = Vec::new();
            for input in inputs {
                let parts = input.parts(item, split.as_ref());
                if parts.is_empty() {
                    continue;
                }
                for &bars in &LENGTHS {
                    let length = bars * bar;
                    let mut starts = Vec::new();
                    let mut at = origin;
                    while at + length <= item.length {
                        starts.push(at);
                        at += bar;
                    }
                    let mut ranked: Vec<(u64, i64)> = starts
                        .iter()
                        .map(|&at| {
                            let name =
                                format!("{}/{}/{}/{bars}/{}", item.source, item.id, input.short(), (at - origin) / bar);
                            (fnv1a(name.as_bytes()), at)
                        })
                        .collect();
                    ranked.sort_unstable();
                    for &(_, at) in ranked.iter().take(picks) {
                        let parts: Vec<(Role, Vec<mo_harmony::Note>)> = parts
                            .iter()
                            .map(|(role, notes)| (*role, cut_notes(notes, at, length)))
                            .filter(|(_, notes)| !notes.is_empty())
                            .collect();
                        if parts.is_empty() {
                            continue;
                        }
                        out.push(Take {
                            song,
                            source: item.source.clone(),
                            input,
                            cut: Some(bars),
                            parts,
                            grid: keys::grid_of(item, true),
                            span: Span::Window,
                            labels: cut_labels(&labels, at, length).into(),
                            lenient,
                        });
                    }
                }
                out.push(Take {
                    song,
                    source: item.source.clone(),
                    input,
                    cut: None,
                    parts,
                    grid: keys::grid_of(item, false),
                    span: item.whole.span(),
                    labels: Arc::clone(&labels),
                    lenient,
                });
            }
            out
        })
        .collect();
    per_item.into_iter().flatten().collect()
}

/// Reads a take with a model; none when nothing is heard.
fn read_take(reader: &mut ChordReader, take: &Take, model: &Model) -> Option<Read> {
    let parts: Vec<mo_harmony::Part> =
        take.parts.iter().map(|(role, notes)| mo_harmony::Part { role: *role, notes }).collect();
    let cx = ChordContext { span: take.span, pins: &[], readings: MOST_READINGS };
    let reading = reader.read_with(&parts, take.grid, &cx, model)?;
    let bar = i64::from(take.grid.beats_per_bar.max(1)) * i64::from(take.grid.ticks_per_beat.max(1));
    let bars = (reading.end - reading.start) as f64 / bar as f64;
    Some(Read { texture: reading.texture, bars, seen: judge(&reading, &take.labels, take.lenient) })
}

/// Every take read with a model, in the takes' order.
fn read_all(takes: &[Take], model: &Model) -> Vec<Option<Read>> {
    takes.par_iter().map_init(ChordReader::new, |reader, take| read_take(reader, take, model)).collect()
}

/// A calibration whose sharpness is `s` whatever is heard.
fn flat(s: f64) -> Calibration {
    let mut c = [0.0; CALIBRATION_TERMS];
    for x in c.iter_mut().take(SHARPNESS_KNOTS) {
        *x = s;
    }
    Calibration(c)
}

/// The sharpnesses each take is read at.
fn grid() -> Vec<f64> {
    (0..GRID_POINTS).map(|k| GRID_LOW * GRID_STEP.powi(k as i32)).collect()
}

/// What each take loses at each sharpness of the grid: minus the log of the chance its stretches give their labelled
/// chords, added up, with how many stretches; and its terms (what each calibration number is multiplied by).
struct Curve {
    song: usize,
    texture: Texture,
    terms: [f64; CALIBRATION_TERMS],
    stretches: usize,
    loss: Vec<f64>,
}

impl Curve {
    /// The loss at a sharpness: a straight line between the grid's points on a log scale (the ends carried on).
    fn at(&self, s: f64) -> f64 {
        let x = (s.max(1e-9) / GRID_LOW).ln() / GRID_STEP.ln();
        let last = self.loss.len().saturating_sub(1);
        let x = x.clamp(0.0, last as f64);
        let k = (x.floor() as usize).min(last.saturating_sub(1));
        let (a, b) = (self.loss.get(k).copied().unwrap_or(0.0), self.loss.get(k + 1).copied().unwrap_or(0.0));
        let t = x - k as f64;
        a + (b - a) * t
    }
}

/// The loss over every curve with a calibration.
fn total(curves: &[Curve], c: &[f64; CALIBRATION_TERMS]) -> f64 {
    curves.iter().map(|k| k.at(Calibration(*c).of_terms(&k.terms))).sum()
}

/// The number at `k` that makes `f` least, between `low` and `high` (golden section, to a ten-thousandth).
fn least_on(mut low: f64, mut high: f64, f: impl Fn(f64) -> f64) -> f64 {
    let r = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (high - r * (high - low), low + r * (high - low));
    let (mut fa, mut fb) = (f(a), f(b));
    while high - low > 1e-4 {
        if fa <= fb {
            high = b;
            (b, fb) = (a, fa);
            a = high - r * (high - low);
            fa = f(a);
        } else {
            low = a;
            (a, fa) = (b, fb);
            b = low + r * (high - low);
            fb = f(b);
        }
    }
    (low + high) / 2.0
}

/// Fits the calibration's numbers on the curves: coordinate descent from every score as it is, each number in turn
/// set where the loss is least (the knots between the grid's ends, the textures' parts within ±3), until no round
/// moves the loss by a millionth of itself. A texture heard in fewer than [`MIN_SONGS`] songs keeps 0, as does one
/// line alone's (its answers are never one chord).
fn fit_calibration(curves: &[Curve]) -> ([f64; CALIBRATION_TERMS], Vec<usize>) {
    let mut c = Calibration::NONE.0;
    let songs_of = |t: Texture| curves.iter().filter(|k| k.texture == t).map(|k| k.song).collect::<HashSet<_>>().len();
    let mut free: Vec<usize> = (0..SHARPNESS_KNOTS).collect();
    for t in [Texture::Accompaniment, Texture::HarmonyOnly] {
        if songs_of(t) >= MIN_SONGS {
            free.push(SHARPNESS_KNOTS + t.index() - 1);
        }
    }
    let high = GRID_LOW * GRID_STEP.powi(GRID_POINTS as i32 - 1);
    let mut loss = total(curves, &c);
    for _ in 0..100 {
        for &k in &free {
            let (low, top) = if k < SHARPNESS_KNOTS { (GRID_LOW, high) } else { (-3.0, 3.0) };
            let at = |x: f64| {
                let mut d = c;
                d[k] = x;
                total(curves, &d)
            };
            c[k] = least_on(low, top, at);
        }
        let now = total(curves, &c);
        let fell = loss - now;
        loss = now;
        if fell.abs() <= 1e-6 * now.abs() {
            break;
        }
    }
    (c, free)
}

/// How true some chances are, over the stretches judged: how many, the mean minus log chance of the labelled chord,
/// the first reading's share right, its mean chance, the calibration error (equal-mass bins), the Brier score, and
/// for the first 1, 2 and 3 readings their mean chance and how often the labelled chord is among them.
struct Truth {
    stretches: usize,
    nll: f64,
    right: f64,
    sure: f64,
    ece: f64,
    brier: f64,
    top: [(f64, f64); 3],
}

fn truth_of<'a>(seen: impl Iterator<Item = &'a Seen>) -> Truth {
    let seen: Vec<&Seen> = seen.collect();
    let n = seen.len().max(1) as f64;
    let answers: Vec<(Option<f64>, bool)> = seen.iter().map(|s| (s.first().map(|f| f.1), s.right())).collect();
    let mut top = [(0.0, 0.0); 3];
    for (k, slot) in top.iter_mut().enumerate() {
        let sums =
            seen.iter().map(|s| s.top(k + 1)).fold((0.0, 0.0), |(p, r), (q, y)| (p + q, r + f64::from(u8::from(y))));
        *slot = (sums.0 / n, sums.1 / n);
    }
    Truth {
        stretches: seen.len(),
        nll: seen.iter().map(|s| -s.truth().ln()).sum::<f64>() / n,
        right: seen.iter().filter(|s| s.right()).count() as f64 / n,
        sure: seen.iter().filter_map(|s| s.first()).map(|f| f.1).sum::<f64>() / n,
        ece: metrics::calibration_error_equal_mass(&answers, BINS).unwrap_or(f64::NAN),
        brier: seen
            .iter()
            .map(|s| {
                let p = s.first().map_or(0.0, |f| f.1);
                (p - f64::from(u8::from(s.right()))).powi(2)
            })
            .sum::<f64>()
            / n,
        top,
    }
}

fn print_truth_header() {
    println!(
        "{:<44} {:>9} {:>7} {:>7} {:>7} {:>6} {:>6} {:>13} {:>13} {:>13}",
        "", "stretches", "-ln p", "right", "sure", "ECE", "Brier", "top 1 p/right", "top 2", "top 3"
    );
}

fn print_truth(name: &str, t: &Truth) {
    let pr = |(p, r): (f64, f64)| format!("{:.3}/{:.3}", p, r);
    println!(
        "{:<44} {:>9} {:>7.4} {:>7.3} {:>7.3} {:>6.3} {:>6.4} {:>13} {:>13} {:>13}",
        name,
        t.stretches,
        t.nll,
        t.right,
        t.sure,
        t.ece,
        t.brier,
        pr(t.top[0]),
        pr(t.top[1]),
        pr(t.top[2])
    );
}

/// The group a take is reported in: its source and way of hearing, and whole or the length cut.
fn group_of(take: &Take) -> String {
    let cut = take.cut.map_or("whole".to_owned(), |b| format!("{b} bars"));
    format!("{} {} {cut}", take.source, take.input.short())
}

/// Prints how true the chances are, by group, by texture and in all.
fn report(takes: &[Take], reads: &[Option<Read>]) {
    print_truth_header();
    let mut groups: BTreeMap<String, Vec<&Seen>> = BTreeMap::new();
    let mut textures: BTreeMap<usize, Vec<&Seen>> = BTreeMap::new();
    let mut knots: BTreeMap<i64, Vec<&Seen>> = BTreeMap::new();
    for (take, read) in takes.iter().zip(reads) {
        let Some(read) = read else { continue };
        groups.entry(group_of(take)).or_default().extend(&read.seen);
        textures.entry(read.texture.index()).or_default().extend(&read.seen);
        knots.entry(take.cut.unwrap_or(0)).or_default().extend(&read.seen);
    }
    for (name, seen) in &groups {
        print_truth(name, &truth_of(seen.iter().copied()));
    }
    for (t, seen) in &textures {
        print_truth(&format!("texture: {}", TEXTURES.get(*t).unwrap_or(&"?")), &truth_of(seen.iter().copied()));
    }
    for (k, seen) in &knots {
        let name = if *k == 0 { "whole pieces".to_owned() } else { format!("{k}-bar stretches") };
        print_truth(&name, &truth_of(seen.iter().copied()));
    }
    let all: Vec<&Seen> = reads.iter().flatten().flat_map(|r| &r.seen).collect();
    print_truth("all", &truth_of(all.into_iter()));
}

/// A stretch as an answer sees it: its song, texture and readings.
struct Answer<'a> {
    song: usize,
    source: &'a str,
    texture: Texture,
    seen: &'a Seen,
}

/// Each song's answers and errors among those `given` takes, for the likely range.
fn errors(answers: &[&Answer], songs: usize, wrong: impl Fn(&Answer) -> bool) -> (f64, f64, usize) {
    let mut per_song = vec![(0.0, 0.0); songs];
    for a in answers {
        if let Some((n, e)) = per_song.get_mut(a.song) {
            *n += 1.0;
            *e += f64::from(u8::from(wrong(a)));
        }
    }
    let (n, e) = per_song.iter().fold((0.0, 0.0), |(a, b), (n, e)| (a + n, b + e));
    let share = if n > 0.0 { e / n } else { 0.0 };
    (share, metrics::error_upper(&per_song), per_song.iter().filter(|(n, _)| *n > 0.0).count())
}

/// The smallest threshold on the 0.01 grid, [`LEAST`] or more, at which the answers `chance` gives (those with a
/// chance, at or above it) are wrong no more often than [`TARGET`] by the upper end of their likely range, from at
/// least [`MIN_SONGS`] songs; 1 (never given) when none is. Each step's error and upper end are printed.
fn smallest(
    name: &str,
    answers: &[&Answer],
    songs: usize,
    chance: impl Fn(&Answer) -> Option<f64>,
    wrong: impl Fn(&Answer) -> bool + Copy,
) -> (f64, Vec<String>) {
    let mut best = 1.0;
    let mut lines = Vec::new();
    for step in (50..100).rev() {
        let t = f64::from(step) / 100.0;
        if t < LEAST {
            break;
        }
        let given: Vec<&Answer> = answers.iter().copied().filter(|a| chance(a).is_some_and(|p| p >= t)).collect();
        let (share, upper, from) = errors(&given, songs, wrong);
        if from >= MIN_SONGS && share <= TARGET {
            best = t;
        }
        if step % 5 == 0 {
            lines.push(format!(
                "  {name} at {t:.2}: {} stretches from {from} songs ({}), wrong {} (upper {})",
                given.len(),
                metrics::pct(given.len() as f64 / answers.len().max(1) as f64),
                metrics::pct(share),
                metrics::pct(upper)
            ));
        }
    }
    (best, lines)
}

/// Fits the thresholds texture by texture on the stretches judged (one line alone keeps 1: never one chord).
fn fit_thresholds(answers: &[Answer], songs: usize) -> (Thresholds, Vec<String>) {
    let mut t = THRESHOLDS;
    let mut lines = Vec::new();
    for texture in [Texture::Full, Texture::Accompaniment, Texture::HarmonyOnly] {
        let i = texture.index();
        let here: Vec<&Answer> = answers.iter().filter(|a| a.texture == texture).collect();
        let from = here.iter().map(|a| a.song).collect::<HashSet<_>>().len();
        let name = TEXTURES.get(i).copied().unwrap_or("?");
        if from < MIN_SONGS {
            lines.push(format!("{name}: {} stretches from {from} songs, too few: kept as built in", here.len()));
            continue;
        }
        let one_chance = |a: &Answer| (!a.seen.rootless_with_no_bass).then(|| a.seen.first().map_or(0.0, |f| f.1));
        let (one, one_lines) = smallest("one chord", &here, songs, one_chance, |a| !a.seen.right());
        // The same notes, on the stretches not given one chord.
        let left: Vec<&Answer> =
            here.iter().copied().filter(|a| one_chance(a).is_none_or(|p| one >= 1.0 || p < one)).collect();
        let same_chance = |a: &Answer| {
            let (n, p, _) = a.seen.group();
            (n >= 2).then_some(p)
        };
        let (same, same_lines) = smallest("the same notes", &left, songs, same_chance, |a| !a.seen.group().2);
        if let (Some(o), Some(s)) = (t.one.get_mut(i), t.same.get_mut(i)) {
            (*o, *s) = (one, same);
        }
        lines.push(format!(
            "{name}: {} stretches from {from} songs: one chord at {one:.2}, the same notes at {same:.2}",
            here.len()
        ));
        lines.extend(one_lines);
        lines.extend(same_lines);
    }
    (t, lines)
}

/// What each kind of answer the thresholds give, by texture: how often given, and how often wrong.
fn print_answers(answers: &[Answer], songs: usize, thresholds: &Thresholds) {
    let mut sources: Vec<&str> = answers.iter().map(|a| a.source).collect();
    sources.sort_unstable();
    sources.dedup();
    if sources.len() > 1 {
        for source in sources {
            println!("{source}:");
            let here: Vec<Answer> = answers
                .iter()
                .filter(|a| a.source == source)
                .map(|a| Answer { song: a.song, source: a.source, texture: a.texture, seen: a.seen })
                .collect();
            print_answers(&here, songs, thresholds);
        }
        println!("all:");
    }
    println!(
        "{:<16} {:>9} {:>18} {:>18} {:>18} {:>9}",
        "", "stretches", "one chord (wrong)", "same notes (wrong)", "unclear (right)", "implied"
    );
    for texture in Texture::ALL {
        let here: Vec<&Answer> = answers.iter().filter(|a| a.texture == texture).collect();
        if here.is_empty() {
            continue;
        }
        let of = |kind: ChordAnswer| -> Vec<&Answer> {
            here.iter().copied().filter(|a| a.seen.answer(texture, thresholds) == kind).collect()
        };
        let (one, same, unclear, implied) =
            (of(ChordAnswer::Chord), of(ChordAnswer::SameNotes), of(ChordAnswer::Unclear), of(ChordAnswer::Implied));
        let n = here.len() as f64;
        let cell = |given: &[&Answer], wrong: &dyn Fn(&Answer) -> bool| {
            let (share, upper, _) = errors(given, songs, wrong);
            format!("{} ({} <{})", metrics::pct(given.len() as f64 / n), metrics::pct(share), metrics::pct(upper))
        };
        println!(
            "{:<16} {:>9} {:>18} {:>18} {:>18} {:>9}",
            TEXTURES.get(texture.index()).unwrap_or(&"?"),
            here.len(),
            cell(&one, &|a| !a.seen.right()),
            cell(&same, &|a| !a.seen.group().2),
            cell(&unclear, &|a| a.seen.right()),
            metrics::pct(implied.len() as f64 / n)
        );
    }
}

/// What `--out` writes and `--calibration` reads.
#[derive(Serialize, Deserialize)]
struct Fitted {
    calibration: [f64; CALIBRATION_TERMS],
    one: [f64; 4],
    same: [f64; 4],
}

struct Options {
    files: Vec<PathBuf>,
    weights: Option<PathBuf>,
    folds: Vec<usize>,
    picks: usize,
    measure: bool,
    keep_sharpness: bool,
    calibration: Option<PathBuf>,
    out: Option<PathBuf>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Options, String> {
        let mut o = Options {
            files: Vec::new(),
            weights: None,
            folds: vec![1],
            picks: PICKS,
            measure: false,
            keep_sharpness: false,
            calibration: None,
            out: None,
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |what: &str| args.next().ok_or_else(|| format!("{arg} needs {what}"));
            match arg.as_str() {
                "--weights" => o.weights = Some(PathBuf::from(value("a file chordfit wrote")?)),
                "--folds" => o.folds = chordfit::folds_in(value("folds, such as 1")?)?,
                "--picks" => {
                    let text = value("a number")?;
                    o.picks = text.parse().map_err(|_| format!("--picks {text}: a number of stretches"))?;
                }
                "--measure" => o.measure = true,
                "--keep-sharpness" => o.keep_sharpness = true,
                "--calibration" => o.calibration = Some(PathBuf::from(value("a file chordcal wrote")?)),
                "--out" => o.out = Some(PathBuf::from(value("a file to write the calibration to")?)),
                a if a.starts_with("--") => return Err(format!("Unknown option {a}.\n\n{}", crate::USAGE)),
                a => o.files.push(PathBuf::from(a)),
            }
        }
        if o.files.is_empty() {
            return Err(crate::USAGE.to_owned());
        }
        if o.folds.is_empty() {
            return Err("--folds needs at least one fold".to_owned());
        }
        Ok(o)
    }
}

/// The stretches judged in some reads, as answers.
fn answers_of<'a>(takes: &'a [Take], reads: &'a [Option<Read>]) -> Vec<Answer<'a>> {
    takes
        .iter()
        .zip(reads)
        .filter_map(|(take, read)| read.as_ref().map(|r| (take.song, take.source.as_str(), r)))
        .flat_map(|(song, source, r)| r.seen.iter().map(move |seen| Answer { song, source, texture: r.texture, seen }))
        .collect()
}

/// Fits the sharpness: each take read at every point of the grid, then the calibration's numbers ([`fit_calibration`]).
fn fit_sharpness(takes: &[Take], before: &[Option<Read>], model: &Model) -> [f64; CALIBRATION_TERMS] {
    let grid = grid();
    let started = std::time::Instant::now();
    let mut losses: Vec<Vec<f64>> = vec![Vec::with_capacity(grid.len()); takes.len()];
    for &s in &grid {
        let c = flat(s);
        let at = Model { calibration: &c, ..*model };
        for (loss, read) in losses.iter_mut().zip(read_all(takes, &at)) {
            loss.push(read.map_or(0.0, |r| r.seen.iter().map(|x| -x.truth().ln()).sum()));
        }
    }
    println!();
    println!("Read every take at {} sharpnesses in {:.0} s.", grid.len(), started.elapsed().as_secs_f64());
    let curves: Vec<Curve> = takes
        .iter()
        .zip(before)
        .zip(losses)
        .filter_map(|((take, read), loss)| {
            let r = read.as_ref()?;
            (!r.seen.is_empty()).then(|| Curve {
                song: take.song,
                texture: r.texture,
                terms: Calibration::terms(r.bars, r.texture),
                stretches: r.seen.len(),
                loss,
            })
        })
        .collect();
    let stretches: usize = curves.iter().map(|c| c.stretches).sum();
    let at_one = total(&curves, &Calibration::NONE.0) / stretches.max(1) as f64;
    let (fitted, free) = fit_calibration(&curves);
    let after = total(&curves, &fitted) / stretches.max(1) as f64;
    println!("The sharpness fitted: -ln p of the labelled chord {at_one:.4} a stretch at 1, {after:.4} fitted.");
    let names = ["1 bar", "2 bars", "4 bars", "8 bars", "16 bars", "accompaniment", "chords alone", "one line"];
    for (k, (name, x)) in names.iter().zip(&fitted).enumerate() {
        let note = if free.contains(&k) { "" } else { " (not fitted: too few songs, or one line)" };
        println!("  {name:<14} {x:+.4}{note}");
    }
    for t in Texture::ALL {
        let line: Vec<String> =
            [1.0, 2.0, 4.0, 8.0, 16.0].iter().map(|&b| format!("{:.3}", Calibration(fitted).sharpness(b, t))).collect();
        println!("  sharpness, {:<14} at 1 to 16 bars: {}", TEXTURES.get(t.index()).unwrap_or(&"?"), line.join(" "));
    }

    fitted
}

pub fn run(args: &[String]) -> Result<(), String> {
    let o = Options::parse(args)?;
    let mut items = Vec::new();
    for f in &o.files {
        items.extend(item::read(f)?);
    }
    let read = items.len();
    items.retain(|i| !i.chords.is_empty());
    let weights: Weights = match &o.weights {
        Some(path) => chordfit::load_weights(path)?.0,
        None => *SHIPPED.weights,
    };
    let built_in = Fitted { calibration: CALIBRATION.0, one: THRESHOLDS.one, same: THRESHOLDS.same };
    let given = match &o.calibration {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => built_in,
    };
    let folds = keyfit::drawn_folds(&items);
    let keep = |song: usize| {
        o.measure
            || items.get(song).is_some_and(|i| i.may_learn()) && folds.get(song).is_some_and(|f| o.folds.contains(f))
    };
    let takes = takes_of(&items, keep, o.picks);
    let songs = items.len();
    let kept = (0..songs).filter(|&s| keep(s)).count();
    println!(
        "{read} pieces read, {} with chord labels; {} {kept} of them ({} takes: each way each is heard, whole and {} \
         stretches of each of 1, 2, 4, 8 and 16 bars).",
        items.len(),
        if o.measure { "measuring" } else { "fitting on" },
        takes.len(),
        o.picks
    );
    if !o.measure {
        println!("Fitting on the pieces whose licence lets fitted numbers ship, in folds {:?}.", o.folds);
    }
    let calibration = Calibration(given.calibration);
    let thresholds = Thresholds { one: given.one, same: given.same };
    let model = Model { weights: &weights, calibration: &calibration, thresholds: &thresholds, ..SHIPPED };

    let start = std::time::Instant::now();
    let before = read_all(&takes, &model);
    println!("Read every take in {:.1} s.", start.elapsed().as_secs_f64());
    println!();
    println!(
        "How true the chances are with the calibration {}:",
        if o.calibration.is_some() { "given" } else { "built in" }
    );
    report(&takes, &before);
    let answers = answers_of(&takes, &before);
    println!();
    println!("The answers with its thresholds (wrong: the share given that's wrong, < its likely range's upper end):");
    print_answers(&answers, songs, &thresholds);
    if o.measure {
        return Ok(());
    }

    let fitted = if o.keep_sharpness { given.calibration } else { fit_sharpness(&takes, &before, &model) };
    let calibration = Calibration(fitted);
    let model = Model { calibration: &calibration, ..model };
    let reads = read_all(&takes, &model);
    println!();
    println!("How true the chances are with the sharpness fitted:");
    report(&takes, &reads);
    let answers = answers_of(&takes, &reads);
    let (thresholds, lines) = fit_thresholds(&answers, songs);
    println!();
    println!(
        "The thresholds fitted (the least chance, on a 0.01 grid from {LEAST}, at which an answer's error is at most \
         {} by the upper end of its likely range):",
        metrics::pct(TARGET)
    );
    for line in &lines {
        println!("{line}");
    }
    println!();
    println!("The answers with the thresholds fitted:");
    print_answers(&answers, songs, &thresholds);
    println!();
    println!("calibration: {:?}", fitted);
    println!("thresholds: one {:?}, same {:?}", thresholds.one, thresholds.same);
    if let Some(path) = &o.out {
        let out = Fitted { calibration: fitted, one: thresholds.one, same: thresholds.same };
        let text = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
        std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
        println!("Wrote {}.", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, length: i64) -> mo_harmony::Note {
        mo_harmony::Note { start, length, key: 60, velocity: 32768 }
    }

    #[test]
    fn a_stretch_keeps_the_notes_struck_in_it_moved_to_its_start_and_cut_at_its_end() {
        let notes = [note(0, 500), note(480, 960), note(960, 100), note(1900, 500), note(1920, 10)];
        let cut = cut_notes(&notes, 480, 1440);
        assert_eq!(cut, vec![note(0, 960), note(480, 100), note(1420, 20)]);
        let labels = vec![(0, 960, "C:maj".to_owned()), (960, 1920, "G:7".to_owned()), (1920, 2880, "N".to_owned())];
        assert_eq!(cut_labels(&labels, 480, 1440), vec![(0, 480, "C:maj".to_owned()), (480, 1440, "G:7".to_owned())]);
    }

    fn label(text: &str) -> Label {
        chordfit::accepts(text, false)[0]
    }

    fn seen(readings: &[(&str, f64, u8)], truth: &str, lenient: bool) -> Seen {
        let readings: Vec<(Label, f64, u8)> = readings.iter().map(|&(t, p, g)| (label(t), p, g)).collect();
        let kept: f64 = readings.iter().map(|r| r.1).sum();
        Seen { readings, other: 1.0 - kept, rootless_with_no_bass: false, accepts: chordfit::accepts(truth, lenient) }
    }

    #[test]
    fn a_stretch_is_judged_by_its_readings_chances_of_the_labelled_chord() {
        // C or Cmaj7 for a C label: strict, only C counts; lenient (POP909-CL), both.
        let s = seen(&[("C:maj", 0.6, 0), ("C:maj7", 0.3, 1), ("A:min7", 0.05, 0)], "C:maj", false);
        assert!(s.right() && (s.truth() - 0.6).abs() < 1e-12);
        assert_eq!(s.group(), (2, 0.65, true));
        assert_eq!(s.top(2), (0.8999999999999999, true));
        let lenient = seen(&[("C:maj", 0.6, 0), ("C:maj7", 0.3, 1)], "C:maj", true);
        assert!((lenient.truth() - 0.9).abs() < 1e-12);
        // A label none of the readings is: what they leave, never less than the floor.
        let missed = seen(&[("D:min", 0.7, 0), ("F:maj", 0.3, 1)], "C:maj", false);
        assert!(!missed.right() && missed.truth() == FLOOR && !missed.group().2);
        let rest = seen(&[("D:min", 0.5, 0)], "C:maj", false);
        assert!((rest.truth() - 0.5).abs() < 1e-12);
        // Graded as the reader grades: one chord at its threshold, else unclear (the same notes never given).
        let t = Thresholds { one: [0.8, 0.8, 0.8, 1.0], same: [1.0; 4] };
        assert_eq!(seen(&[("C:maj", 0.85, 0)], "C:maj", false).answer(Texture::Full, &t), ChordAnswer::Chord);
        assert_eq!(s.answer(Texture::Full, &t), ChordAnswer::Unclear);
        assert_eq!(s.answer(Texture::SingleLine, &t), ChordAnswer::Implied);
    }

    #[test]
    fn a_curve_is_a_straight_line_between_its_points_on_a_log_scale() {
        let grid = grid();
        assert_eq!(grid.len(), GRID_POINTS);
        let c = Curve {
            song: 0,
            texture: Texture::Full,
            terms: Calibration::terms(1.0, Texture::Full),
            stretches: 1,
            loss: (0..GRID_POINTS).map(|k| (k as f64 - 30.0).powi(2)).collect(),
        };
        assert!((c.at(grid[30]) - 0.0).abs() < 1e-9 && (c.at(grid[31]) - 1.0).abs() < 1e-9);
        let between = (grid[30] * grid[31]).sqrt();
        assert!((c.at(between) - 0.5).abs() < 1e-9);
        // The ends carried on.
        assert!((c.at(0.001) - 900.0).abs() < 1e-9);
        // The least of the curve is found where it is.
        let x = least_on(GRID_LOW, grid[GRID_POINTS - 1], |s| c.at(s));
        assert!((x / grid[30] - 1.0).abs() < 1e-3, "{x}");
        let (fitted, free) = fit_calibration(std::slice::from_ref(&c));
        assert!((fitted[0] / grid[30] - 1.0).abs() < 1e-3 && free == [0, 1, 2, 3, 4], "{fitted:?} {free:?}");
    }

    #[test]
    fn a_threshold_is_the_least_whose_answers_are_wrong_no_more_than_the_target() {
        // 40 songs: chances 0.95 always right, 0.7 right 9 times in 10, 0.55 wrong half the time.
        let mut seens = Vec::new();
        for song in 0..40 {
            for (p, right) in [(0.95, true), (0.7, song % 10 != 0), (0.55, song % 2 == 0)] {
                let first = if right { "C:maj" } else { "D:min" };
                seens.push((song, seen(&[(first, p, 0)], "C:maj", false)));
            }
        }
        let answers: Vec<Answer> = seens
            .iter()
            .map(|(song, s)| Answer { song: *song, source: "x", texture: Texture::Full, seen: s })
            .collect();
        let (t, lines) = fit_thresholds(&answers, 40);
        // At 0.56 to 0.70, a twentieth wrong; at 0.55, a fifth.
        assert_eq!(t.one[0], 0.56, "{lines:?}");
        assert_eq!(t.same[0], 1.0);
        // Too few songs: kept as built in.
        assert_eq!((t.one[1], t.one[2]), (THRESHOLDS.one[1], THRESHOLDS.one[2]));
    }
}
