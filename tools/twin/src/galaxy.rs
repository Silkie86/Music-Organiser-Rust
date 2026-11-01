//! The galaxy twin: both apps lay a galaxy out from the same made-up fingerprints (families of sound-alikes, sounds
//! that are exact copies, a library of just four), then place the sounds marked new on it.
//!
//! The Rust galaxy does what the C# one does but its own way (its own random numbers, every core at once), so the
//! maps aren't star for star the same. What's compared is how good each map is: how many of each star's nearest
//! sound-alikes are also its nearest stars, and whether each new sound lands beside its sound-alikes. Rust must do
//! at least as well (to within a twentieth). A library of one or two sounds, laid out by a fixed rule, must match.
//! Run as part of `twin sounds`, in its own folder.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use mo_sounds::StarPoint;
use mo_sounds::galaxy::{SEED, layout, place_new};
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
    // Exact copies: no distance between them.
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
    let mut cases = vec![
        Case { name: "families", vectors: families(4, 30, 40, 7), new: vec![3, 31, 64, 100, 119, 120] },
        Case { name: "copies", vectors: copies, new: vec![5, 30] },
        Case { name: "four sounds", vectors: tiny, new: vec![] },
        Case { name: "two sounds", vectors: families(1, 3, 5, 9), new: vec![3] },
    ];
    // Unit length, as the app's fingerprints are, so "most alike" (a dot product) and "nearest" agree.
    for c in &mut cases {
        for (_, v) in &mut c.vectors {
            mo_sounds::index::unit(v);
        }
    }
    cases
}

/// The map and the placed sounds.
fn rust_side(c: &Case) -> (HashMap<i64, StarPoint>, HashMap<i64, StarPoint>) {
    let all: HashMap<i64, Vec<f32>> = c.vectors.iter().cloned().collect();
    let get = |id: i64| Cow::Borrowed(all[&id].as_slice());
    let old: Vec<i64> = c.vectors.iter().map(|(id, _)| *id).filter(|id| !c.new.contains(id)).collect();
    let map = layout(&old, &get, SEED, &|_| {}, &AtomicBool::new(false)).unwrap_or_default();
    let new: Vec<i64> = c.vectors.iter().map(|(id, _)| *id).filter(|id| !map.contains_key(id)).collect();
    let placed = place_new(&new, &old, &get, &map);
    (map, placed)
}

/// The C# helper's `[id, x, y]` list.
fn read_points(v: &Value) -> HashMap<i64, StarPoint> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    let f = |t: usize| p[t].as_f64().unwrap_or(f64::NAN) as f32;
                    (p[0].as_i64().unwrap_or(0), StarPoint { x: f(1), y: f(2) })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `k` ids nearest `of` among `ids` by `far` (ties by id).
fn nearest(of: i64, ids: &[i64], k: usize, far: impl Fn(i64, i64) -> f64) -> Vec<i64> {
    let mut others: Vec<(f64, i64)> = ids.iter().filter(|id| **id != of).map(|id| (far(of, *id), *id)).collect();
    others.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    others.into_iter().take(k).map(|(_, id)| id).collect()
}

fn apart(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| f64::from(x - y).powi(2)).sum()
}

fn on_map(map: &HashMap<i64, StarPoint>, a: i64, b: i64) -> f64 {
    let (p, q) = (map[&a], map[&b]);
    f64::from(p.x - q.x).powi(2) + f64::from(p.y - q.y).powi(2)
}

/// How good a map is: the share of each star's 10 nearest sound-alikes that are among its 10 nearest stars, and how
/// many new sounds landed with one of their nearest sound-alikes (the closest quarter of the map) as their nearest
/// star.
fn quality(c: &Case, map: &HashMap<i64, StarPoint>, placed: &HashMap<i64, StarPoint>) -> (f64, usize) {
    let all: HashMap<i64, &Vec<f32>> = c.vectors.iter().map(|(id, v)| (*id, v)).collect();
    let mut ids: Vec<i64> = map.keys().copied().collect();
    ids.sort_unstable();
    let k = 10.min(ids.len().saturating_sub(1));
    if k == 0 {
        return (1.0, placed.len());
    }
    let kept: usize = ids
        .iter()
        .map(|&id| {
            let alike = nearest(id, &ids, k, |a, b| apart(all[&a], all[&b]));
            let close = nearest(id, &ids, k, |a, b| on_map(map, a, b));
            alike.iter().filter(|x| close.contains(x)).count()
        })
        .sum();
    let mut with_new = map.clone();
    with_new.extend(placed);
    let landed = placed
        .keys()
        .filter(|&&id| {
            let alike = nearest(id, &ids, (ids.len() / 4).max(k), |a, b| apart(all[&a], all[&b]));
            let mut both = ids.clone();
            both.push(id);
            nearest(id, &both, 1, |a, b| on_map(&with_new, a, b)).first().is_some_and(|n| alike.contains(n))
        })
        .count();
    (kept as f64 / (ids.len() * k) as f64, landed)
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
        let (cs_map, cs_placed) = (read_points(&cs["map"]), read_points(&cs["placed"]));
        let (map, placed) = rust_side(c);
        if map.len() != cs_map.len() || placed.len() != cs_placed.len() {
            out.push(format!(
                "galaxy, {}: C# has {} stars and {} placed, Rust {} and {}",
                c.name,
                cs_map.len(),
                cs_placed.len(),
                map.len(),
                placed.len()
            ));
            continue;
        }
        if map.len() < 3 {
            // Laid out by a fixed rule; a placed sound differs only by its little nudge (under 0.002 each way).
            let off = |a: &HashMap<i64, StarPoint>, b: &HashMap<i64, StarPoint>, most: f32| {
                a.iter().any(|(id, p)| b.get(id).is_none_or(|q| (p.x - q.x).abs() > most || (p.y - q.y).abs() > most))
            };
            if off(&cs_map, &map, 0.0) || off(&cs_placed, &placed, 0.0041) {
                out.push(format!("galaxy, {}: C# {cs_map:?} {cs_placed:?}, Rust {map:?} {placed:?}", c.name));
            }
            continue;
        }
        let (cs_kept, cs_landed) = quality(c, &cs_map, &cs_placed);
        let (kept, landed) = quality(c, &map, &placed);
        println!(
            "  galaxy, {}: sound-alikes kept as neighbours C# {cs_kept:.2}, Rust {kept:.2}; new sounds beside their \
             sound-alikes C# {cs_landed}, Rust {landed} of {}",
            c.name,
            placed.len()
        );
        if kept < cs_kept - 0.05 {
            out.push(format!(
                "galaxy, {}: Rust keeps fewer sound-alikes together ({kept:.2} to C#'s {cs_kept:.2})",
                c.name
            ));
        }
        if landed < cs_landed {
            out.push(format!(
                "galaxy, {}: fewer new sounds land beside their sound-alikes (Rust {landed}, C# {cs_landed})",
                c.name
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_lays_out_and_places_its_new_sounds_well() {
        for c in cases() {
            let (map, placed) = rust_side(&c);
            assert_eq!(map.len() + placed.len(), c.vectors.len(), "{}", c.name);
            if map.len() >= 3 {
                let (kept, landed) = quality(&c, &map, &placed);
                assert!(kept > 0.5, "{}: {kept}", c.name);
                assert_eq!(landed, placed.len(), "{}", c.name);
            }
        }
    }
}
