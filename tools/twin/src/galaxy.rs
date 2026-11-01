//! The galaxy twin: both apps lay a galaxy out from the same made-up fingerprints (families of sound-alikes, sounds
//! that are exact copies, a library of just four), then place the sounds marked new on it. Every star must sit in the
//! same place. Only small libraries are compared: a big one settles on every core at once, so even the C# app gives a
//! slightly different map each time. Run as part of `twin sounds`, in its own folder.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use mo_sounds::StarPoint;
use mo_sounds::galaxy::{EXACT_BELOW, SEED, layout, place_new};
use serde_json::{Value, json};

struct Case {
    name: &'static str,
    vectors: Vec<(i64, Vec<f32>)>,
    new: Vec<i64>,
}

/// Numbers from a fixed seed: -0.5 up to 0.5.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    }
}

/// `families` groups of `per` sounds, each group leaning one way, every number on a scale of its own.
fn families(families: usize, per: usize, dim: usize, seed: u64) -> Vec<(i64, Vec<f32>)> {
    let mut n = Numbers(seed | 1);
    let mut out = Vec::new();
    for f in 0..families {
        for s in 0..per {
            let mut v: Vec<f32> = (0..dim).map(|d| n.next() * (1.0 + (d % 3) as f32)).collect();
            v[(f * 7) % dim] += 4.0;
            v[(f * 3 + 1) % dim] -= 2.0;
            out.push(((f * per + s + 1) as i64, v));
        }
    }
    out
}

fn cases() -> Vec<Case> {
    let mut copies = families(3, 12, 9, 33);
    // Exact copies: no distance between them, so which comes first among equals matters.
    for id in [2, 5, 14] {
        let v = copies[0].1.clone();
        copies[id as usize - 1].1 = v;
    }
    let tiny = vec![
        (10, vec![1.0, 0.0, 0.5]),
        (20, vec![1.0, 0.0, 0.5]),
        (30, vec![0.0, 1.0, 0.2]),
        (40, vec![0.2, 0.9, 0.1]),
    ];
    vec![
        Case { name: "families", vectors: families(4, 30, 40, 7), new: vec![3, 31, 64, 100, 119, 120] },
        Case { name: "copies", vectors: copies, new: vec![5, 30] },
        Case { name: "four sounds", vectors: tiny, new: vec![] },
        Case { name: "two sounds", vectors: families(1, 3, 5, 9), new: vec![3] },
    ]
}

/// The map and the placed sounds, as the C# helper gives them: `[id, x, y]` in id order.
fn rust_side(c: &Case) -> (Value, Value) {
    let all: HashMap<i64, Vec<f32>> = c.vectors.iter().cloned().collect();
    let get = |id: i64| all[&id].clone();
    let old: Vec<i64> = c.vectors.iter().map(|(id, _)| *id).filter(|id| !c.new.contains(id)).collect();
    let map = layout(&old, &get, SEED, EXACT_BELOW, &|_| {}, &AtomicBool::new(false)).unwrap_or_default();
    // As C#'s PlaceNew(vectors, map): every sound the map lacks, compared with every one it has.
    let new: Vec<i64> = c.vectors.iter().map(|(id, _)| *id).filter(|id| !map.contains_key(id)).collect();
    let mapped: Vec<i64> = c.vectors.iter().map(|(id, _)| *id).filter(|id| map.contains_key(id)).collect();
    let placed = place_new(&new, &mapped, &get, &map);
    (points(&map), points(&placed))
}

fn points(map: &HashMap<i64, StarPoint>) -> Value {
    let mut ids: Vec<&i64> = map.keys().collect();
    ids.sort();
    ids.into_iter().map(|id| json!([id, map[id].x, map[id].y])).collect()
}

/// Every star where the other app put it. Single-precision places to within a millionth: the same numbers, worked
/// out in the same order, should land on the same float, so anything more is a real difference.
fn differences(what: &str, cs: &Value, rust: &Value, out: &mut Vec<String>) {
    let empty = Vec::new();
    let (a, b) = (cs.as_array().unwrap_or(&empty), rust.as_array().unwrap_or(&empty));
    if a.len() != b.len() {
        out.push(format!("{what}: C# has {} stars, Rust {}", a.len(), b.len()));
        return;
    }
    let mut worst = 0f64;
    let mut moved = 0;
    for (x, y) in a.iter().zip(b) {
        if x[0] != y[0] {
            out.push(format!("{what}: C# has sound {}, Rust {}", x[0], y[0]));
            return;
        }
        let gap = (1..3)
            .map(|t| (x[t].as_f64().unwrap_or(f64::NAN) - y[t].as_f64().unwrap_or(0.0)).abs())
            .fold(0.0, f64::max);
        if gap.is_nan() || gap > 1e-6 {
            moved += 1;
        }
        worst = if gap.is_nan() { f64::NAN } else { worst.max(gap) };
    }
    if moved > 0 {
        out.push(format!("{what}: {moved} of {} stars sit elsewhere (by up to {worst:.2e})", a.len()));
    }
}

/// Writes the cases into `dir` (the twin's own new folder), has both apps lay them out, and gives every difference.
pub fn compare(helper: &Path, dir: &Path) -> Result<Vec<String>, String> {
    let cases = cases();
    let mut files = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let path = dir.join(format!("galaxy-{i}.json"));
        let body = json!({
            "Vectors": c.vectors.iter().map(|(id, v)| json!([id, v])).collect::<Vec<_>>(),
            "New": c.new,
        });
        std::fs::write(&path, body.to_string()).map_err(|e| format!("Couldn't write a galaxy case: {e}"))?;
        files.push(path);
    }
    let cs = crate::run_helper_with(helper, &["--galaxy"], &files)?;
    let mut out = Vec::new();
    for (c, file) in cases.iter().zip(&files) {
        let Some(cs) = cs.get(file.to_string_lossy().as_ref()) else {
            out.push(format!("galaxy, {}: no C# answer", c.name));
            continue;
        };
        if let Some(e) = cs.get("error").or_else(|| cs.get("error_more")) {
            out.push(format!("galaxy, {}: the C# helper failed: {e}", c.name));
            continue;
        }
        let (map, placed) = rust_side(c);
        differences(&format!("galaxy, {}, laid out", c.name), &cs["map"], &map, &mut out);
        differences(&format!("galaxy, {}, new sounds placed", c.name), &cs["placed"], &placed, &mut out);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_lays_out_and_places_its_new_sounds() {
        for c in cases() {
            let (map, placed) = rust_side(&c);
            let count = |v: &Value| v.as_array().map_or(0, Vec::len);
            assert_eq!(count(&map) + count(&placed), c.vectors.len(), "{}", c.name);
            let mut none = Vec::new();
            differences(c.name, &map, &map, &mut none);
            assert!(none.is_empty());
        }
    }
}
