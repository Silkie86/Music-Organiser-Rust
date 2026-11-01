//! The "sounds like" twin: both apps build the likeness index from the same made-up vectors (numbers on very
//! different scales, one that never changes, one vector of an odd length) and find each probe's nearest sounds. The
//! ids must come out in the same order, with the same likeness. Run as part of `twin sounds`, in its own folder.

use std::path::Path;

use mo_sounds::index::VectorIndex;
use mo_sounds::learn::Vectors;
use serde_json::{Value, json};

struct Case {
    name: &'static str,
    /// Standardised with chroma counting less (the features index), or as they are (CLAP).
    features: bool,
    vectors: Vectors,
    queries: Vec<i64>,
    max: usize,
}

/// Numbers from a fixed seed: 0 up to 1.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn made(count: i64, dim: usize, seed: u64) -> Vectors {
    let mut n = Numbers(seed | 1);
    // Each number on its own scale, as the analyser's are (seconds, hertz, decibels…).
    let scales: Vec<f32> = (0..dim).map(|d| 10f32.powi((d % 5) as i32 - 1)).collect();
    let mut v: Vectors = (1..=count)
        .map(|id| {
            let mut x: Vec<f32> = scales.iter().map(|s| (n.next() - 0.3) * s).collect();
            x[0] = 2.5;
            (id, x)
        })
        .collect();
    v.insert(count + 1, vec![1.0; dim - 1]);
    v
}

fn cases() -> Vec<Case> {
    vec![
        Case { name: "features", features: true, vectors: made(300, 73, 81), queries: vec![1, 7, 150, 299], max: 12 },
        Case { name: "deep listening", features: false, vectors: made(200, 64, 91), queries: vec![2, 99, 200], max: 8 },
    ]
}

fn rust_side(c: &Case) -> Value {
    let index = if c.features { VectorIndex::features(&c.vectors) } else { VectorIndex::clap(&c.vectors) };
    c.queries
        .iter()
        .map(|q| {
            let found = index.get(*q).map(|v| index.nearest(v, c.max)).unwrap_or_default();
            found.into_iter().map(|(id, score)| json!({ "Id": id, "Score": score })).collect::<Value>()
        })
        .collect()
}

/// Likeness to within a hundred-thousandth: C# adds the products several at a time, Rust one by one.
fn same(cs: &Value, rust: &Value) -> bool {
    let (Some(a), Some(b)) = (cs.as_array(), rust.as_array()) else { return false };
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x["Id"] == y["Id"]
                && (x["Score"].as_f64().unwrap_or(f64::NAN) - y["Score"].as_f64().unwrap_or(0.0)).abs() < 1e-5
        })
}

/// Writes the cases into `dir` (the twin's own new folder), has both apps search, and gives every difference.
pub fn compare(helper: &Path, dir: &Path) -> Result<Vec<String>, String> {
    let cases = cases();
    let mut files = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let path = dir.join(format!("similar-{i}.json"));
        let mut vectors: Vec<(&i64, &Vec<f32>)> = c.vectors.iter().collect();
        vectors.sort_by_key(|(id, _)| **id);
        let body = json!({
            "Features": c.features,
            "Vectors": vectors.iter().map(|(id, v)| json!([id, v])).collect::<Vec<_>>(),
            "Queries": c.queries,
            "Max": c.max,
        });
        std::fs::write(&path, body.to_string()).map_err(|e| format!("Couldn't write a likeness case: {e}"))?;
        files.push(path);
    }
    let cs = crate::run_helper_with(helper, &["--similar"], &files)?;
    let mut differences = Vec::new();
    for (c, file) in cases.iter().zip(&files) {
        let Some(cs) = cs.get(file.to_string_lossy().as_ref()) else {
            differences.push(format!("likeness, {}: no C# answer", c.name));
            continue;
        };
        if let Some(e) = cs.get("error").or_else(|| cs.get("error_more")) {
            differences.push(format!("likeness, {}: the C# helper failed: {e}", c.name));
            continue;
        }
        let rust = rust_side(c);
        let empty = Vec::new();
        let cs_found = cs.get("nearest").and_then(Value::as_array).unwrap_or(&empty);
        for (i, q) in c.queries.iter().enumerate() {
            let (a, b) = (cs_found.get(i).unwrap_or(&Value::Null), &rust[i]);
            if !same(a, b) {
                differences.push(format!("likeness, {}: sound {q}: C# {a}, Rust {b}", c.name));
            }
        }
    }
    Ok(differences)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_finds_its_own_sound_first() {
        for c in cases() {
            let found = rust_side(&c);
            for (i, q) in c.queries.iter().enumerate() {
                assert_eq!(found[i][0]["Id"], json!(q), "{}", c.name);
                assert_eq!(found[i].as_array().unwrap().len(), c.max);
            }
        }
    }
}
