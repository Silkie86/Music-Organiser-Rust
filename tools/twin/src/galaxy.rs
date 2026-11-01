//! The galaxy twin: both apps lay a galaxy out from the same made-up fingerprints (families of sound-alikes, sounds
//! that are exact copies, a library of just four), then place the sounds marked new on it.
//!
//! The smallest libraries must come out star for star the same. For the bigger ones, each step is compared on its
//! own: the C# app squashes fingerprints with sums it adds eight at a time, in an order .NET picks for the processor,
//! so its numbers can differ from Rust's in the last float digit, and 400 rounds of settling grow that into a
//! different (equally good) picture. So the steps must agree to within that rounding, with the same sound-alikes,
//! and Rust settling from the C# app's own start, and placing new sounds on its map, must land every star where C#
//! does. Libraries of 6,000 sounds or more settle on every core at once, which no two runs repeat, so they aren't
//! compared. Run as part of `twin sounds`, in its own folder.

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
    /// Compare each step of the layout too, to show where a difference starts.
    steps: bool,
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
        Case { name: "families", vectors: families(4, 30, 40, 7), new: vec![3, 31, 64, 100, 119, 120], steps: true },
        Case { name: "copies", vectors: copies, new: vec![5, 30], steps: true },
        Case { name: "four sounds", vectors: tiny, new: vec![], steps: false },
        Case { name: "two sounds", vectors: families(1, 3, 5, 9), new: vec![3], steps: false },
    ]
}

/// The map and the placed sounds, as the C# helper gives them: `[id, x, y]` in id order.
fn rust_side(c: &Case) -> (Value, Value) {
    let all: HashMap<i64, Vec<f32>> = c.vectors.iter().cloned().collect();
    let get = |id: i64| all[&id].clone();
    let map = layout(&old_ids(c), &get, SEED, EXACT_BELOW, &|_| {}, &AtomicBool::new(false)).unwrap_or_default();
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

/// Each step of a layout beside the C# app's. The sound-alikes found and the random numbers used must be the same;
/// the numbers worked out may differ in their last float digit (the C# app adds eight products at a time, in an order
/// .NET picks for the processor), so they must agree to within a hundred-thousandth of their size.
fn steps(c: &Case, cs: &Value, out: &mut Vec<String>) {
    let all: HashMap<i64, Vec<f32>> = c.vectors.iter().cloned().collect();
    let get = |id: i64| all[&id].clone();
    let (reduced, init, nbr, dist, edges, next) = mo_sounds::galaxy::twin_steps(&old_ids(c), &get, SEED);
    let edges: Vec<Value> = edges.iter().map(|(i, j, w)| json!([i, j, w])).collect();
    let rust = [json!(reduced), json!(init), json!(nbr), json!(dist), json!(edges), json!(next)];
    for (name, r) in ["Reduced", "Init", "Nbr", "Dist", "Edges", "Next"].iter().zip(rust) {
        let (a, b) = (flat(&cs[*name]), flat(&r));
        let exact = matches!(*name, "Nbr" | "Next");
        let off = if a.len() == b.len() {
            a.iter()
                .zip(&b)
                .filter(|(x, y)| {
                    let gap = (*x - *y).abs();
                    gap.is_nan() || if exact { gap > 0.0 } else { gap > 1e-5 * x.abs().max(1.0) }
                })
                .count()
        } else {
            a.len().max(b.len())
        };
        if off > 0 {
            out.push(format!(
                "galaxy, {}, step {name}: {off} of {} numbers differ (Rust has {})",
                c.name,
                a.len(),
                b.len()
            ));
        }
    }
}

/// Every number in a JSON value, in order.
fn flat(v: &Value) -> Vec<f64> {
    let mut o = Vec::new();
    let mut walk = vec![v];
    while let Some(x) = walk.pop() {
        match x {
            Value::Array(a) => walk.extend(a.iter().rev()),
            _ => o.push(x.as_f64().unwrap_or(f64::NAN)),
        }
    }
    o
}

fn old_ids(c: &Case) -> Vec<i64> {
    c.vectors.iter().map(|(id, _)| *id).filter(|id| !c.new.contains(id)).collect()
}

/// Rust settling from the C# app's starting picture and weighed pairs, and placing the new sounds on the C# map.
fn from_cs(c: &Case, cs: &Value) -> (Value, Value) {
    let all: HashMap<i64, Vec<f32>> = c.vectors.iter().cloned().collect();
    let get = |id: i64| all[&id].clone();
    let steps = &cs["steps"];
    let init = flat(&steps["Init"]);
    let edges: Vec<(usize, usize, f32)> = steps["Edges"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    (
                        e[0].as_u64().unwrap_or(0) as usize,
                        e[1].as_u64().unwrap_or(0) as usize,
                        e[2].as_f64().unwrap_or(0.0) as f32,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let settled = mo_sounds::galaxy::twin_settle(&old_ids(c), &get, SEED, init, &edges);
    let cs_map: HashMap<i64, StarPoint> = cs["map"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    let f = |t: usize| p[t].as_f64().unwrap_or(f64::NAN) as f32;
                    (p[0].as_i64().unwrap_or(0), StarPoint { x: f(1), y: f(2) })
                })
                .collect()
        })
        .unwrap_or_default();
    let new: Vec<i64> = c.vectors.iter().map(|(id, _)| *id).filter(|id| !cs_map.contains_key(id)).collect();
    let mapped: Vec<i64> = c.vectors.iter().map(|(id, _)| *id).filter(|id| cs_map.contains_key(id)).collect();
    (points(&settled), points(&place_new(&new, &mapped, &get, &cs_map)))
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
            "Steps": c.steps,
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
        if c.steps {
            // A bigger library: each step in turn, then the settling and placing from the C# app's own inputs.
            steps(c, &cs["steps"], &mut out);
            let (map, placed) = from_cs(c, cs);
            differences(&format!("galaxy, {}, settled from C#'s start", c.name), &cs["map"], &map, &mut out);
            differences(
                &format!("galaxy, {}, new sounds placed on C#'s map", c.name),
                &cs["placed"],
                &placed,
                &mut out,
            );
        } else {
            let (map, placed) = rust_side(c);
            differences(&format!("galaxy, {}, laid out", c.name), &cs["map"], &map, &mut out);
            differences(&format!("galaxy, {}, new sounds placed", c.name), &cs["placed"], &placed, &mut out);
        }
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
