//! The sound type classifier: learns from your own library what a kick, a pad or a vocal sounds like, so a sound
//! whose name says nothing still gets a type. A port of the C# app's `SoundClassifier`, step for step, so it reads the
//! model the C# app saved in `sounds.db` and trains the same model from the same examples (checked by the sounds twin
//! test, `tools/twin/src/classify.rs`).
//!
//! How it works, in plain words: every sound is a list of numbers (the analyser's 73 features, or a CLAP embedding
//! once deep listening is installed). The classifier gives each type a weight per number; a sound's score for a type
//! is the weighted sum, and the scores are turned into chances that add up to 1 (a "softmax"). Training nudges the
//! weights, a few examples at a time, until the types of the examples come out on top (with "Adam", a common way of
//! choosing how big each nudge is). Labels come from folder and file names and from your own corrections, which count
//! three times as much.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analyse::round_to;
use crate::dotnet_random::DotnetRandom;

/// Where the C# app keeps the type model in `sounds.db`'s settings.
pub const TYPE_MODEL_KEY: &str = "type_model";

/// One labelled example: a sound's vector, its label, and how much to trust the label (your own labels are 3, names 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    pub x: Vec<f32>,
    pub label: String,
    pub weight: f32,
}

/// How training goes. The defaults are the C# app's; learned tags use 60 epochs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// How many times training goes through every example.
    pub epochs: usize,
    pub learning_rate: f64,
    /// How strongly big weights are pulled back towards 0, so no one number decides everything.
    pub l2: f64,
    /// At most this many examples of each type, so a huge folder of hats doesn't drown out everything else.
    pub max_per_class: usize,
    /// The seed for the shuffles, so training the same examples twice gives the same model.
    pub seed: i32,
}

impl Default for Options {
    fn default() -> Options {
        Options { epochs: 40, learning_rate: 0.01, l2: 1e-4, max_per_class: 800, seed: 7 }
    }
}

/// Why a model couldn't be trained. The messages are the C# app's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrainError {
    Nothing,
    OneKind,
    /// Some vectors are longer or shorter than others (the C# app would stop with an error, or use part of them).
    MixedLengths,
}

impl std::fmt::Display for TrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TrainError::Nothing => "Nothing to train on yet",
            TrainError::OneKind => "Need at least two kinds of sound to learn the difference",
            TrainError::MixedLengths => "The sounds' vectors aren't all the same length",
        })
    }
}

impl std::error::Error for TrainError {}

/// A trained classifier, in the shape the C# app saves it (JSON with these names in `sounds.db`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Classifier {
    pub classes: Vec<String>,
    /// Each number's average over the examples, taken off before the weights are used.
    pub mean: Vec<f32>,
    /// One over each number's spread (0 for a number that never changed), so every number counts on the same scale.
    pub scale: Vec<f32>,
    /// Weights, one row of `mean.len()` numbers per class.
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    /// Which vectors it was trained on: "clap" or "features".
    pub space: String,
    /// How many examples it was given (before each type was capped).
    pub examples: i32,
    /// How often it was right on a fifth of the examples kept back from training (0 to 1); `None` with fewer than 40.
    pub held_out_accuracy: Option<f64>,
    /// When it was trained, as .NET writes a local time (kept as text: only shown).
    pub trained: String,
}

impl Classifier {
    /// Reads a saved model; `None` when there's none or it can't be read. A model whose sizes don't fit together is
    /// treated as unreadable (the C# app would have stopped with an error when using it).
    pub fn from_json(json: Option<&str>) -> Option<Classifier> {
        let json = json.filter(|j| !j.is_empty())?;
        let c: Classifier = serde_json::from_str(json).ok()?;
        let fits = c.scale.len() == c.mean.len()
            && c.b.len() == c.classes.len()
            && c.w.len() == c.classes.len() * c.mean.len();
        fits.then_some(c)
    }

    pub fn to_json(&self) -> String {
        // Every field is a plain list, number or text, so this can't fail.
        serde_json::to_string(self).unwrap_or_default()
    }

    fn dim(&self) -> usize {
        self.mean.len()
    }

    /// Each type with its chance, most likely first (types with the same chance stay in the model's order). Empty when
    /// `x` isn't the length the model was trained on.
    pub fn predict(&self, x: &[f32]) -> Vec<(&str, f64)> {
        let dim = self.dim();
        let k = self.classes.len();
        // A model made by hand whose sizes don't fit together judges nothing (a saved one is checked when read).
        if x.len() != dim || self.scale.len() != dim || self.b.len() != k || self.w.len() != k * dim {
            return Vec::new();
        }
        let z: Vec<f32> = x.iter().zip(&self.mean).zip(&self.scale).map(|((x, m), s)| (x - m) * s).collect();
        let mut p: Vec<f64> = (0..k)
            .map(|c| {
                // As C#: each product in single precision, added up in double.
                let mut s = f64::from(self.b[c]);
                for (w, z) in self.w[c * dim..(c + 1) * dim].iter().zip(&z) {
                    s += f64::from(w * z);
                }
                s
            })
            .collect();
        let max = p.iter().fold(f64::MIN, |m, &s| dotnet_max(m, s));
        let mut sum = 0.0;
        for v in &mut p {
            *v = (*v - max).exp();
            sum += *v;
        }
        let mut out: Vec<(&str, f64)> =
            self.classes.iter().map(String::as_str).zip(p.into_iter().map(|v| v / sum)).collect();
        out.sort_by(|a, b| dotnet_compare(b.1, a.1));
        out
    }

    /// The most likely type and its chance, if the model can judge `x` at all.
    pub fn top(&self, x: &[f32]) -> Option<(&str, f64)> {
        self.predict(x).into_iter().next()
    }
}

/// `Math.Max` for doubles: NaN if either is NaN.
fn dotnet_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// How .NET orders numbers when sorting (`CompareTo`): NaN before everything, and -0 the same as 0.
fn dotnet_compare<T: PartialOrd + Copy>(a: T, b: T) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    #[allow(clippy::eq_op)] // a value not equal to itself is NaN
    let nan = |v: T| v != v;
    match a.partial_cmp(&b) {
        Some(o) => o,
        None => match (nan(a), nan(b)) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            _ => Ordering::Greater,
        },
    }
}

/// Trains a classifier on `all`, as the C# app's `SoundClassifier.Train`: each type is capped at
/// [`Options::max_per_class`] (your own labels first, then picked at random), and when there are at least 40
/// examples a second model is trained on four fifths of them and tested on the rest, for
/// [`Classifier::held_out_accuracy`]. The two models train at the same time (on rayon's threads, so call it from a
/// low-priority pool, never the window's thread).
pub fn train(all: &[Example], space: &str, o: &Options) -> Result<Classifier, TrainError> {
    let mut rng = DotnetRandom::new(o.seed);
    // Balance, as C#'s `GroupBy(label).SelectMany(g => g.OrderByDescending(weight).ThenBy(random).Take(cap))
    // .OrderBy(random)`: types in the order they first appear; within each, every example draws its number in turn
    // (all of them, even past the cap, as .NET does), and ties go to the earlier example.
    let mut groups: Vec<(&str, Vec<&Example>)> = Vec::new();
    let mut group_of: HashMap<&str, usize> = HashMap::new();
    for e in all {
        let g = *group_of.entry(&e.label).or_insert_with(|| {
            groups.push((&e.label, Vec::new()));
            groups.len() - 1
        });
        groups[g].1.push(e);
    }
    let mut kept: Vec<&Example> = Vec::new();
    for (_, members) in &groups {
        let draws: Vec<i32> = members.iter().map(|_| rng.next()).collect();
        let mut order: Vec<usize> = (0..members.len()).collect();
        order.sort_by(|&a, &b| {
            dotnet_compare(members[b].weight, members[a].weight).then(draws[a].cmp(&draws[b])).then(a.cmp(&b))
        });
        kept.extend(order.into_iter().take(o.max_per_class).map(|i| members[i]));
    }
    let draws: Vec<i32> = kept.iter().map(|_| rng.next()).collect();
    let mut order: Vec<usize> = (0..kept.len()).collect();
    order.sort_by(|&a, &b| draws[a].cmp(&draws[b]).then(a.cmp(&b)));
    let data: Vec<&Example> = order.into_iter().map(|i| kept[i]).collect();

    let Some(first) = data.first() else { return Err(TrainError::Nothing) };
    if data.iter().any(|e| e.x.len() != first.x.len()) {
        return Err(TrainError::MixedLengths);
    }
    let mut classes: Vec<String> = data.iter().map(|e| e.label.clone()).collect();
    // C#'s `Order()` on text: by the current culture's rules. Labels are type names (plain letters), where that is
    // ordinal order once case is set aside; ties in case go lower-case first, as .NET's invariant culture does.
    classes.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a)));
    classes.dedup();
    if classes.len() < 2 {
        return Err(TrainError::OneKind);
    }

    let (model, accuracy) = rayon::join(
        || fit(&data, &classes, space, o),
        || {
            if data.len() < 40 {
                return None;
            }
            let test: Vec<&Example> = data.iter().step_by(5).copied().collect();
            let rest: Vec<&Example> = data.iter().enumerate().filter(|(i, _)| i % 5 != 0).map(|(_, e)| *e).collect();
            let mut kinds: Vec<&str> = rest.iter().map(|e| e.label.as_str()).collect();
            kinds.sort_unstable();
            kinds.dedup();
            if kinds.len() < 2 {
                return None;
            }
            let probe = fit(&rest, &classes, space, o);
            let right = test.iter().filter(|e| probe.top(&e.x).is_some_and(|(l, _)| l == e.label)).count();
            Some(round_to(right as f64 / test.len() as f64, 3))
        },
    );
    Ok(Classifier {
        examples: i32::try_from(all.len()).unwrap_or(i32::MAX),
        held_out_accuracy: accuracy,
        trained: dotnet_now(),
        ..model
    })
}

/// One model from `data` (already balanced and shuffled), as C#'s `SoundClassifier.Fit`. The arithmetic follows C#'s
/// exactly, single precision where it used `float`, so the weights come out the same.
fn fit(data: &[&Example], classes: &[String], space: &str, o: &Options) -> Classifier {
    let dim = data[0].x.len();
    let n = data.len();
    let k = classes.len();
    let index: HashMap<&str, usize> = classes.iter().enumerate().map(|(i, c)| (c.as_str(), i)).collect();

    // Standardise each number: take off its average, divide by its spread.
    let mut mean = vec![0f32; dim];
    let mut scale = vec![0f32; dim];
    for e in data {
        for (m, x) in mean.iter_mut().zip(&e.x) {
            *m += x;
        }
    }
    for m in &mut mean {
        *m /= n as f32;
    }
    for e in data {
        for ((s, x), m) in scale.iter_mut().zip(&e.x).zip(&mean) {
            *s += (x - m) * (x - m);
        }
    }
    for s in &mut scale {
        let sd = (*s / n as f32).sqrt();
        *s = if sd > 1e-6 { 1.0 / sd } else { 0.0 };
    }
    let xs: Vec<Vec<f32>> =
        data.iter().map(|e| e.x.iter().zip(&mean).zip(&scale).map(|((x, m), s)| (x - m) * s).collect()).collect();
    let ys: Vec<usize> = data.iter().map(|e| index[e.label.as_str()]).collect();

    // Rare types count for more, so they still matter.
    let mut counts = vec![0f64; k];
    for &y in &ys {
        counts[y] += 1.0;
    }
    let class_w: Vec<f64> = counts.iter().map(|&c| if c > 0.0 { n as f64 / (k as f64 * c) } else { 0.0 }).collect();

    let mut w = vec![0f64; k * dim];
    let mut b = vec![0f64; k];
    // Adam's running averages of each weight's nudges and their squares.
    let (mut mw, mut vw) = (vec![0f64; w.len()], vec![0f64; w.len()]);
    let (mut mb, mut vb) = (vec![0f64; k], vec![0f64; k]);
    const BETA1: f64 = 0.9;
    const BETA2: f64 = 0.999;
    const EPS: f64 = 1e-8;
    const BATCH: usize = 64;
    let mut step = 0i32;
    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = DotnetRandom::new(o.seed);
    let mut gw = vec![0f64; w.len()];
    let mut gb = vec![0f64; k];
    let mut logits = vec![0f64; k];

    for _ in 0..o.epochs {
        rng.shuffle(&mut order);
        for batch in order.chunks(BATCH) {
            gw.fill(0.0);
            gb.fill(0.0);
            let mut total_weight = 0.0;
            for &i in batch {
                let x = &xs[i];
                softmax(&w, &b, x, dim, &mut logits);
                let sw = f64::from(data[i].weight) * class_w[ys[i]];
                total_weight += sw;
                for (c, &l) in logits.iter().enumerate() {
                    let g = (l - if c == ys[i] { 1.0 } else { 0.0 }) * sw;
                    if g == 0.0 {
                        continue;
                    }
                    gb[c] += g;
                    for (gw, x) in gw[c * dim..(c + 1) * dim].iter_mut().zip(x) {
                        *gw += g * f64::from(*x);
                    }
                }
            }
            step += 1;
            let lr = o.learning_rate * (1.0 - BETA2.powf(f64::from(step))).sqrt() / (1.0 - BETA1.powf(f64::from(step)));
            let inv = 1.0 / total_weight.max(1e-9);
            for p in 0..w.len() {
                let g = gw[p] * inv + o.l2 * w[p];
                mw[p] = BETA1 * mw[p] + (1.0 - BETA1) * g;
                vw[p] = BETA2 * vw[p] + (1.0 - BETA2) * g * g;
                w[p] -= lr * mw[p] / (vw[p].sqrt() + EPS);
            }
            for c in 0..k {
                let g = gb[c] * inv;
                mb[c] = BETA1 * mb[c] + (1.0 - BETA1) * g;
                vb[c] = BETA2 * vb[c] + (1.0 - BETA2) * g * g;
                b[c] -= lr * mb[c] / (vb[c].sqrt() + EPS);
            }
        }
    }
    Classifier {
        classes: classes.to_vec(),
        mean,
        scale,
        space: space.to_owned(),
        w: w.iter().map(|&v| v as f32).collect(),
        b: b.iter().map(|&v| v as f32).collect(),
        ..Classifier::default()
    }
}

/// The chances of each class for `x` while training (weights in double precision), into `out`.
fn softmax(w: &[f64], b: &[f64], x: &[f32], dim: usize, out: &mut [f64]) {
    let mut max = f64::MIN;
    for (c, o) in out.iter_mut().enumerate() {
        let mut s = b[c];
        for (w, x) in w[c * dim..(c + 1) * dim].iter().zip(x) {
            s += w * f64::from(*x);
        }
        *o = s;
        if s > max {
            max = s;
        }
    }
    let mut sum = 0.0;
    for o in out.iter_mut() {
        *o = (*o - max).exp();
        sum += *o;
    }
    for o in out.iter_mut() {
        *o /= sum;
    }
}

/// The time now, as .NET writes a local `DateTime` in JSON ("2026-10-31T18:06:00.1234567+00:00").
fn dotnet_now() -> String {
    let now = chrono::Local::now();
    let ticks = now.timestamp_subsec_nanos() / 100;
    let fraction = format!("{ticks:07}");
    let fraction = fraction.trim_end_matches('0');
    let dot = if fraction.is_empty() { String::new() } else { format!(".{fraction}") };
    format!("{}{dot}{}", now.format("%Y-%m-%dT%H:%M:%S"), now.format("%:z"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Examples in clusters: each type sits around its own point.
    fn clusters(kinds: &[(&str, usize)], dim: usize) -> Vec<Example> {
        let mut rng = DotnetRandom::new(1);
        let mut out = Vec::new();
        for (k, (label, count)) in kinds.iter().enumerate() {
            for i in 0..*count {
                let x = (0..dim)
                    .map(|d| {
                        let centre = if d % kinds.len() == k { 3.0 } else { 0.0 };
                        centre + (rng.next() as f32 / i32::MAX as f32 - 0.5)
                    })
                    .collect();
                out.push(Example { x, label: (*label).into(), weight: if i % 7 == 0 { 3.0 } else { 1.0 } });
            }
        }
        out
    }

    #[test]
    fn it_learns_types_that_sit_apart() {
        let data = clusters(&[("Kick", 60), ("Hat", 40), ("Pad", 20)], 6);
        let m = train(&data, "features", &Options::default()).unwrap();
        assert_eq!(m.classes, ["Hat", "Kick", "Pad"]);
        assert_eq!(m.examples, 120);
        assert_eq!(m.held_out_accuracy, Some(1.0));
        for e in &data {
            let p = m.predict(&e.x);
            assert_eq!(p[0].0, e.label);
            assert!((p.iter().map(|t| t.1).sum::<f64>() - 1.0).abs() < 1e-9);
            assert!(p.windows(2).all(|w| w[0].1 >= w[1].1));
        }
        assert!(m.predict(&[1.0]).is_empty());
    }

    #[test]
    fn training_twice_gives_the_same_model() {
        let data = clusters(&[("Kick", 30), ("Snare", 30)], 4);
        let a = train(&data, "features", &Options::default()).unwrap();
        let b = train(&data, "features", &Options::default()).unwrap();
        assert_eq!(Classifier { trained: String::new(), ..a }, Classifier { trained: String::new(), ..b });
    }

    #[test]
    fn each_type_is_capped_and_few_examples_skip_the_check() {
        let data = clusters(&[("Kick", 50), ("Hat", 5)], 3);
        let o = Options { max_per_class: 6, ..Options::default() };
        let m = train(&data, "clap", &o).unwrap();
        // 11 kept: fewer than 40, so no held-out check, but it still counts every example it was given.
        assert_eq!(m.held_out_accuracy, None);
        assert_eq!(m.examples, 55);
        assert_eq!(m.space, "clap");
    }

    #[test]
    fn it_needs_two_kinds_and_equal_lengths() {
        assert_eq!(train(&[], "features", &Options::default()), Err(TrainError::Nothing));
        let one = clusters(&[("Kick", 10)], 3);
        assert_eq!(train(&one, "features", &Options::default()), Err(TrainError::OneKind));
        let mut mixed = clusters(&[("Kick", 10), ("Hat", 10)], 3);
        mixed[4].x.push(1.0);
        assert_eq!(train(&mixed, "features", &Options::default()), Err(TrainError::MixedLengths));
    }

    #[test]
    fn a_number_that_never_changes_is_left_out() {
        let mut data = clusters(&[("Kick", 20), ("Hat", 20)], 3);
        for e in &mut data {
            e.x[2] = 5.0;
        }
        let m = train(&data, "features", &Options::default()).unwrap();
        assert_eq!(m.scale[2], 0.0);
        assert_eq!(m.mean[2], 5.0);
    }

    #[test]
    fn it_reads_the_c_sharp_model_and_writes_it_back() {
        let json = r#"{"Classes":["Kick","Pad"],"Mean":[0.5,1],"Scale":[2,0],"W":[1,0,-1,0],"B":[0,0.25],
            "Space":"features","Examples":30,"HeldOutAccuracy":null,"Trained":"2026-10-31T18:06:00.1234567+00:00"}"#;
        let m = Classifier::from_json(Some(json)).unwrap();
        assert_eq!(m.classes, ["Kick", "Pad"]);
        assert_eq!(m.held_out_accuracy, None);
        // z = ((1 - 0.5) * 2, 0) = (1, 0): Kick scores 1, Pad 0.25 - 1.
        let p = m.predict(&[1.0, 9.0]);
        assert_eq!(p[0].0, "Kick");
        let expect = 1.0 / (1.0 + (-1.75f64).exp());
        assert!((p[0].1 - expect).abs() < 1e-12);
        assert_eq!(Classifier::from_json(Some(&m.to_json())), Some(m.clone()));
        assert!(m.to_json().contains("\"HeldOutAccuracy\":null"));
        assert!(Classifier::from_json(None).is_none());
        assert!(Classifier::from_json(Some("")).is_none());
        assert!(Classifier::from_json(Some("not json")).is_none());
        // Weights that don't fit the classes.
        assert!(Classifier::from_json(Some(&json.replace("[1,0,-1,0]", "[1,0,-1]"))).is_none());
    }

    #[test]
    fn equal_chances_keep_the_models_order() {
        let m = Classifier {
            classes: vec!["B".into(), "A".into()],
            mean: vec![0.0],
            scale: vec![1.0],
            w: vec![0.0, 0.0],
            b: vec![0.0, 0.0],
            ..Classifier::default()
        };
        assert_eq!(m.predict(&[1.0]), [("B", 0.5), ("A", 0.5)]);
    }

    #[test]
    fn the_time_is_written_as_dotnet_does() {
        let t = dotnet_now();
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], "T");
        assert!(t.ends_with(|c: char| c.is_ascii_digit()));
        assert_eq!(&t[t.len() - 3..t.len() - 2], ":");
    }
}
