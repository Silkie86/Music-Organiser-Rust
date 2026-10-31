//! The classifier twin: both apps train the sound type classifier on the same made-up examples (points in clusters,
//! one cluster per type, so nobody's sounds are in it) and the models must come out the same, number for number.
//! Then the Rust side reads the model the C# app trained, as it would from `sounds.db`, and both judge the same
//! probes. Run as part of `twin sounds`, in the same folder of its own.

use std::path::{Path, PathBuf};

use mo_sounds::classify::{Classifier, Example, Options, train};
use serde_json::{Value, json};

use crate::sounds::same;

struct Case {
    name: &'static str,
    space: &'static str,
    options: Options,
    examples: Vec<Example>,
    probes: Vec<Vec<f32>>,
}

/// Numbers from a fixed seed, so every run makes the same cases: 0 up to 1.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// `sizes` examples of each type, around a point of its own (`spread` sets how far they stray, so types can overlap),
/// handed out a type at a time in turn so the types first appear out of alphabetical order. One number never changes.
/// Every `heavy`th example of each type counts three times (your own label).
fn clusters(kinds: &[(&str, usize)], dim: usize, spread: f32, heavy: usize, seed: u64) -> Vec<Example> {
    let mut n = Numbers(seed | 1);
    let centres: Vec<Vec<f32>> = kinds.iter().map(|_| (0..dim).map(|_| n.next() * 4.0).collect()).collect();
    let mut left: Vec<usize> = kinds.iter().map(|k| k.1).collect();
    let mut out = Vec::new();
    while left.iter().any(|&l| l > 0) {
        for (k, (label, size)) in kinds.iter().enumerate() {
            if left[k] == 0 {
                continue;
            }
            left[k] -= 1;
            let i = size - left[k];
            let mut x: Vec<f32> = centres[k].iter().map(|c| c + (n.next() - 0.5) * spread).collect();
            x[dim - 1] = 0.5;
            let weight = if heavy > 0 && i.is_multiple_of(heavy) { 3.0 } else { 1.0 };
            out.push(Example { x, label: (*label).to_owned(), weight });
        }
    }
    out
}

fn probes(dim: usize, seed: u64) -> Vec<Vec<f32>> {
    let mut n = Numbers(seed | 1);
    let mut p: Vec<Vec<f32>> = (0..8).map(|_| (0..dim).map(|_| n.next() * 4.0).collect()).collect();
    // One the wrong length: both judge nothing.
    p.push(vec![1.0; dim + 1]);
    p
}

fn cases() -> Vec<Case> {
    let o = Options::default();
    vec![
        Case {
            name: "four types, overlapping",
            space: "features",
            options: o,
            examples: clusters(&[("Pad", 60), ("Kick", 120), ("Vocal", 30), ("Hat", 90)], 12, 6.0, 5, 21),
            probes: probes(12, 22),
        },
        Case {
            name: "capped, your labels first",
            space: "features",
            options: Options { epochs: 15, max_per_class: 20, seed: 3, ..o },
            examples: clusters(&[("Snare", 70), ("808", 25), ("Closed hat", 10)], 8, 3.0, 4, 31),
            probes: probes(8, 32),
        },
        Case {
            name: "deep listening vectors",
            space: "clap",
            options: o,
            examples: clusters(&[("Lead", 50), ("Bass", 50), ("FX", 50)], 64, 5.0, 0, 41),
            probes: probes(64, 42),
        },
        Case {
            name: "a learned tag",
            space: "features",
            options: Options { epochs: 60, ..o },
            examples: clusters(&[("yes", 15), ("no", 60)], 10, 8.0, 0, 51),
            probes: probes(10, 52),
        },
        Case {
            name: "few examples",
            space: "features",
            options: Options { epochs: 60, ..o },
            examples: clusters(&[("Kick", 6), ("Clap", 6)], 5, 1.0, 3, 61),
            probes: probes(5, 62),
        },
        Case {
            name: "one type",
            space: "features",
            options: o,
            examples: clusters(&[("Kick", 30)], 4, 1.0, 0, 71),
            probes: Vec::new(),
        },
        Case { name: "nothing", space: "features", options: o, examples: Vec::new(), probes: Vec::new() },
    ]
}

/// A case for the C# helper; `rust_model` is the model Rust trained, saved as it would go in `sounds.db`, for C# to
/// read back and judge the probes with.
fn case_json(c: &Case, rust_model: Option<&str>) -> Value {
    json!({
        "RustModel": rust_model,
        "Space": c.space,
        "Options": {
            "Epochs": c.options.epochs,
            "LearningRate": c.options.learning_rate,
            "L2": c.options.l2,
            "MaxPerClass": c.options.max_per_class,
            "Seed": c.options.seed,
        },
        "Examples": c.examples.iter().map(|e| json!({ "X": e.x, "Label": e.label, "Weight": e.weight })).collect::<Vec<_>>(),
        "Probes": c.probes,
    })
}

/// What's compared of a trained model: everything but when it was trained.
const MODEL_FIELDS: [&str; 8] = ["Classes", "Mean", "Scale", "W", "B", "Space", "Examples", "HeldOutAccuracy"];

/// Writes the cases into `dir` (the twin's own new folder), has both apps train on them, and gives every difference.
pub fn compare(helper: &Path, dir: &Path) -> Result<Vec<String>, String> {
    let cases = cases();
    let trained: Vec<_> = cases.iter().map(|c| train(&c.examples, c.space, &c.options)).collect();
    let mut files = Vec::new();
    for (i, (c, rust)) in cases.iter().zip(&trained).enumerate() {
        let path: PathBuf = dir.join(format!("classify-{i}.json"));
        let saved = rust.as_ref().ok().and_then(Classifier::to_json);
        std::fs::write(&path, case_json(c, saved.as_deref()).to_string())
            .map_err(|e| format!("Couldn't write a classifier case: {e}"))?;
        files.push(path);
    }
    let cs = crate::run_helper_with(helper, &["--classify"], &files)?;
    let mut differences = Vec::new();
    for ((c, file), rust) in cases.iter().zip(&files).zip(trained) {
        let name = c.name;
        let Some(cs) = cs.get(file.to_string_lossy().as_ref()) else {
            differences.push(format!("classifier, {name}: no C# answer"));
            continue;
        };
        if let Some(e) = cs.get("error").or_else(|| cs.get("error_more")) {
            differences.push(format!("classifier, {name}: the C# helper failed: {e}"));
            continue;
        }
        let (cs_model, rust_model) = match (cs.get("refused").and_then(Value::as_str), rust) {
            (Some(cs_why), Err(rust_why)) => {
                if cs_why != rust_why.to_string() {
                    differences
                        .push(format!("classifier, {name}: refused as \"{cs_why}\" in C#, \"{rust_why}\" in Rust"));
                }
                continue;
            }
            (Some(cs_why), Ok(_)) => {
                differences.push(format!("classifier, {name}: C# refused (\"{cs_why}\"), Rust trained"));
                continue;
            }
            (None, Err(rust_why)) => {
                differences.push(format!("classifier, {name}: C# trained, Rust refused (\"{rust_why}\")"));
                continue;
            }
            (None, Ok(m)) => (cs.get("model").and_then(Value::as_str).unwrap_or_default(), m),
        };
        let cs_value: Value = serde_json::from_str(cs_model).unwrap_or(Value::Null);
        let rust_value = serde_json::to_value(&rust_model).map_err(|e| e.to_string())?;
        for field in MODEL_FIELDS {
            let (a, b) = (cs_value.get(field).unwrap_or(&Value::Null), rust_value.get(field).unwrap_or(&Value::Null));
            if !same(a, b) {
                differences.push(format!("classifier, {name}: {field} differs: C# {}, Rust {}", short(a), short(b)));
            }
        }
        // The model Rust trained, saved and read back by the C# app as it would be from sounds.db, judging the probes.
        match cs.get("rust_model_predictions").and_then(Value::as_array) {
            None => differences.push(format!("classifier, {name}: C# couldn't read the model Rust saved")),
            Some(cs_read) => {
                for (i, probe) in c.probes.iter().enumerate() {
                    let rust_says = says(&rust_model, probe);
                    let cs_one = cs_read.get(i).unwrap_or(&Value::Null);
                    if !same(cs_one, &rust_says) {
                        differences.push(format!(
                            "classifier, {name}: probe {i} with the Rust model: C# {}, Rust {}",
                            short(cs_one),
                            short(&rust_says)
                        ));
                    }
                }
            }
        }
        // The model C# trained, read as the Rust app reads it from sounds.db, judging the same probes.
        let Some(read) = Classifier::from_json(Some(cs_model)) else {
            differences.push(format!("classifier, {name}: Rust couldn't read the C# model"));
            continue;
        };
        let cs_says = cs.get("predictions").and_then(Value::as_array).cloned().unwrap_or_default();
        for (i, probe) in c.probes.iter().enumerate() {
            let rust_says = says(&read, probe);
            let cs_one = cs_says.get(i).unwrap_or(&Value::Null);
            if !same(cs_one, &rust_says) {
                differences.push(format!(
                    "classifier, {name}: probe {i}: C# {}, Rust {}",
                    short(cs_one),
                    short(&rust_says)
                ));
            }
        }
    }
    Ok(differences)
}

/// What `model` says of `probe`, in the helper's shape.
fn says(model: &Classifier, probe: &[f32]) -> Value {
    model.predict(probe).into_iter().map(|(label, p)| json!({ "Label": label, "P": p })).collect()
}

/// A value for a message, cut short: weights run to hundreds of numbers.
fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 300 { format!("{}…", &s[..s.floor_char_boundary(300)]) } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cases_train_or_are_refused_as_meant() {
        for c in cases() {
            let r = train(&c.examples, c.space, &c.options);
            match c.name {
                "one type" | "nothing" => assert!(r.is_err(), "{}", c.name),
                _ => {
                    let m = r.unwrap();
                    // Overlapping types: right often, never always (so the check means something).
                    if c.name == "four types, overlapping" {
                        let a = m.held_out_accuracy.unwrap();
                        assert!(a > 0.5 && a < 1.0, "{a}");
                    }
                    assert_eq!(m.predict(c.probes.last().unwrap()), []);
                }
            }
        }
    }
}
