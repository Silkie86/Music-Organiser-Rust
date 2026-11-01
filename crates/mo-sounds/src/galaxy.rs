//! The sound galaxy: every sound becomes a point on a flat map, and sounds that sound alike end up close together. A
//! port of the C# app's `GalaxyMap` and the galaxy part of `SoundOrganiser`, checked by the galaxy twin test.
//!
//! It works like UMAP, cut down to what a sample library needs:
//! 1. Squash each sound's fingerprint to its 32 most telling directions (PCA, learned from at most 20,000 sounds).
//! 2. Find each sound's 15 nearest sound-alikes: exactly below 6,000 sounds, and above by "neighbours of
//!    neighbours" (NN-descent), which gets almost all of them right in a fraction of the time.
//! 3. Start from the PCA picture, then let close sound-alikes pull together and random pairs push apart for a few
//!    hundred rounds, until the clusters settle.
//!
//! A small library always gives the same map (a fixed seed, the same numbers as the C# app gives). A big one settles
//! on every core at once, so it can differ slightly between layouts. Either way it's worked out once and saved; new
//! sounds are then placed beside their nearest sound-alikes ([`place_new`]), so the stars you know don't move.
//!
//! Laying out a big library takes a while and uses every core: call it on the app's low-priority pool
//! (`mo_app::background::on_pool`), never the window's thread.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::StarPoint;
use crate::dotnet_random::DotnetRandom;
use crate::index::{SoundIndex, dot, unit};

/// How many sound-alikes each sound is tied to.
pub const NEIGHBOURS: usize = 15;
/// Libraries smaller than this get exact neighbours and a single-threaded (repeatable) layout.
pub const EXACT_BELOW: usize = 6000;
/// The seed the app lays out with.
pub const SEED: i32 = 42;
const DIMS: usize = 32;
const PCA_SAMPLE: usize = 20_000;
const NEGATIVE_SAMPLES: usize = 5;
/// How many mapped sounds a new sound is compared with.
const PLACE_SAMPLE: usize = 10_000;
// UMAP's curve for min_dist 0.5: close neighbours keep a little room, so a family of sounds reads as a cloud of
// separate stars rather than one blob.
const A: f64 = 0.583;
const B: f64 = 1.334;
/// How much the final picture is evened out (see `finish`), 0 to 1.
const EVEN: f64 = 0.35;

/// What the stars can be placed by: the choice on the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placing {
    /// Deep listening and the measurements together.
    #[default]
    Both,
    /// Deep listening alone.
    Clap,
    /// The analyser's own measurements alone.
    Own,
}

/// What the stars are placed by this time, as saved maps are named ("features", "clap" or "clap+features"): the wish,
/// unless deep listening hasn't heard enough of the library yet, when it's the measurements. As C#'s
/// `SoundOrganiser.GalaxySpace`.
pub fn space(wanted: Placing, index: &SoundIndex) -> &'static str {
    if !index.deep || wanted == Placing::Own {
        "features"
    } else if wanted == Placing::Clap {
        "clap"
    } else {
        "clap+features"
    }
}

/// The fingerprints a galaxy is laid out from: every sound the features index has, in id order, and how to make each
/// one's fingerprint. As C#'s `SoundOrganiser.GalaxyVectors`.
///
/// "clap+features" joins CLAP's "what it is" (80 %) to the measured "how it sounds" (60 %), so neighbours match in
/// kind and in character. A sound deep listening hasn't heard yet falls back to its measurements in every space.
/// The joined fingerprints are made when asked for, never all held at once.
pub struct Fingerprints<'a> {
    pub ids: Vec<i64>,
    index: &'a SoundIndex,
    /// Joined with CLAP (and how); `None` for the measurements alone.
    clap: Option<(usize, bool)>,
}

impl Fingerprints<'_> {
    /// One sound's fingerprint; empty for a sound the features index doesn't have.
    pub fn get(&self, id: i64) -> Vec<f32> {
        let Some(f) = self.index.features.get(id) else { return Vec::new() };
        let Some((clap_dim, clap_only)) = self.clap else { return f.to_vec() };
        let mut v = vec![0f32; clap_dim + f.len()];
        let c = self.index.clap.as_ref().and_then(|c| c.get(id)).filter(|c| c.len() == clap_dim);
        let (wc, wf) = match c {
            None => (0f32, 1f32),
            Some(_) if clap_only => (1.0, 0.0),
            Some(_) => (0.8, 0.6),
        };
        if let Some(c) = c {
            for (out, x) in v.iter_mut().zip(c) {
                *out = x * wc;
            }
        }
        for (out, x) in v[clap_dim..].iter_mut().zip(f) {
            *out = x * wf;
        }
        unit(&mut v);
        v
    }
}

/// The fingerprints for `space` (see [`space`]).
pub fn fingerprints<'a>(index: &'a SoundIndex, space: &str) -> Fingerprints<'a> {
    let ids = index.features.ids().to_vec();
    let clap = if space == "features" { None } else { index.clap.as_ref() };
    let clap = clap.map(|c| c.dim()).filter(|d| *d > 0);
    Fingerprints { ids, index, clap: clap.map(|d| (d, space == "clap")) }
}

/// True when the saved `map` lacks some sound the features index has (or there's no map yet, in a library with any
/// analysed sound). As C#'s `SoundOrganiser.GalaxyOutOfDate`.
pub fn out_of_date(index: &SoundIndex, map: &HashMap<i64, StarPoint>) -> bool {
    index.features.ids().iter().any(|id| !map.contains_key(id))
}

/// A galaxy brought up to date by [`update`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Updated {
    /// Every star.
    pub map: HashMap<i64, StarPoint>,
    /// What to save under the space's name: the whole map when `replace` (it was laid out afresh), else only the
    /// stars just placed.
    pub save: HashMap<i64, StarPoint>,
    pub replace: bool,
}

/// Brings the galaxy for `space` up to date from its `saved` map. It's laid out afresh when there's no map yet,
/// `fresh` asks for it, or more than a fifth of the sounds are new; otherwise the new sounds are placed beside their
/// sound-alikes, so the stars you know stay where they were. `None` when `cancel` was set. As C#'s
/// `SoundOrganiser.UpdateGalaxy`, less the saving: until sounds.db is trusted, the app keeps [`Updated::save`] to
/// itself.
pub fn update(
    index: &SoundIndex,
    space: &str,
    saved: HashMap<i64, StarPoint>,
    fresh: bool,
    progress: &(dyn Fn(Step) + Sync),
    cancel: &AtomicBool,
) -> Option<Updated> {
    let prints = fingerprints(index, space);
    let vector = |id: i64| prints.get(id);
    let new_ids: Vec<i64> = prints.ids.iter().copied().filter(|id| !saved.contains_key(id)).collect();
    if fresh || saved.is_empty() || new_ids.len() > prints.ids.len() / 5 {
        let map = layout(&prints.ids, &vector, SEED, EXACT_BELOW, progress, cancel)?;
        return Some(Updated { save: map.clone(), map, replace: true });
    }
    let mut map = saved;
    let mut save = HashMap::new();
    if !new_ids.is_empty() {
        let mapped: Vec<i64> = prints.ids.iter().copied().filter(|id| map.contains_key(id)).collect();
        save = place_new(&new_ids, &mapped, &vector, &map);
        map.extend(save.iter().map(|(id, p)| (*id, *p)));
    }
    Some(Updated { map, save, replace: false })
}

/// How far a layout has got: `done` of 4 steps, and what it's doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub what: &'static str,
    pub done: u32,
}

/// Lays out the sounds `ids`, fetching each fingerprint from `vector` when it's needed (it must be safe to call from
/// several threads). `seed` 42 and `exact_below` [`EXACT_BELOW`] are the app's. `None` when `cancel` was set. As
/// C#'s `GalaxyMap.Layout`.
pub fn layout(
    ids: &[i64],
    vector: &(dyn Fn(i64) -> Vec<f32> + Sync),
    seed: i32,
    exact_below: usize,
    progress: &(dyn Fn(Step) + Sync),
    cancel: &AtomicBool,
) -> Option<HashMap<i64, StarPoint>> {
    let mut order = ids.to_vec();
    order.sort_unstable();
    order.dedup();
    let n = order.len();
    if n < 3 {
        return Some(
            order.iter().enumerate().map(|(i, id)| (*id, StarPoint { x: 0.3 + 0.4 * i as f32, y: 0.5 })).collect(),
        );
    }
    let stop = || cancel.load(Ordering::Relaxed);
    let mut rng = DotnetRandom::new(seed);
    let exact = n < exact_below;

    progress(Step { what: "Finding what each sound is like", done: 0 });
    let (reduced, mut pos) = pca(&order, vector, &mut rng, &stop)?;

    progress(Step { what: "Finding sound-alikes", done: 1 });
    let k = NEIGHBOURS.min(n - 1);
    let (nbr, dist) =
        if exact { exact_neighbours(&reduced, n, k) } else { descent_neighbours(&reduced, n, k, seed, &stop)? };

    progress(Step { what: "Weighing sound-alikes", done: 2 });
    let edges = edges(&nbr, &dist);

    progress(Step { what: "Letting the galaxy settle", done: 3 });
    let epochs = if n <= 10_000 {
        400
    } else if n <= 100_000 {
        200
    } else {
        120
    };
    if exact {
        optimise(&mut pos, &edges, n, epochs, &mut rng, &stop)?;
    } else {
        pos = optimise_parallel(pos, &edges, n, epochs, seed, &stop)?;
    }

    finish(&mut pos, n);
    progress(Step { what: "Galaxy mapped", done: 4 });
    Some(
        order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, StarPoint { x: pos[2 * i] as f32, y: pos[2 * i + 1] as f32 }))
            .collect(),
    )
}

/// Evens the picture out a little and fits it into 0..1, with one scale both ways and a margin for the labels.
///
/// Evening out is the same in every direction: each star keeps its direction from the middle, but its distance moves
/// part of the way to where it would be if the stars were spread evenly over a disc.
fn finish(pos: &mut [f64], n: usize) {
    let (mut mx, mut my) = (0.0, 0.0);
    for i in 0..n {
        mx += pos[2 * i];
        my += pos[2 * i + 1];
    }
    mx /= n as f64;
    my /= n as f64;
    let dist: Vec<f64> = (0..n)
        .map(|i| ((pos[2 * i] - mx) * (pos[2 * i] - mx) + (pos[2 * i + 1] - my) * (pos[2 * i + 1] - my)).sqrt())
        .collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| dist[*a].total_cmp(&dist[*b]));
    let far = dist[order[n - 1]].max(1e-9);
    for (r, &i) in order.iter().enumerate() {
        if dist[i] < 1e-12 {
            continue;
        }
        let even = ((r as f64 + 0.5) / n as f64).sqrt() * far;
        let k = ((1.0 - EVEN) * dist[i] + EVEN * even) / dist[i];
        pos[2 * i] = mx + (pos[2 * i] - mx) * k;
        pos[2 * i + 1] = my + (pos[2 * i + 1] - my) * k;
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in 0..n {
        min_x = min_x.min(pos[2 * i]);
        max_x = max_x.max(pos[2 * i]);
        min_y = min_y.min(pos[2 * i + 1]);
        max_y = max_y.max(pos[2 * i + 1]);
    }
    let span = (max_x - min_x).max(max_y - min_y).max(1e-9);
    let off_x = (1.0 - (max_x - min_x) / span) / 2.0;
    let off_y = (1.0 - (max_y - min_y) / span) / 2.0;
    for i in 0..n {
        pos[2 * i] = 0.07 + 0.86 * (off_x + (pos[2 * i] - min_x) / span);
        pos[2 * i + 1] = 0.07 + 0.86 * (off_y + (pos[2 * i + 1] - min_y) / span);
    }
}

/// Places sounds the map doesn't have yet at the middle of their 3 nearest mapped sound-alikes, nudged a little so
/// they don't sit exactly on top of one. In a big library they're compared with a sample of 10,000 mapped sounds,
/// which is plenty to find their neighbourhood. As C#'s `GalaxyMap.PlaceNew`.
pub fn place_new(
    new_ids: &[i64],
    mapped_ids: &[i64],
    vector: &(dyn Fn(i64) -> Vec<f32> + Sync),
    map: &HashMap<i64, StarPoint>,
) -> HashMap<i64, StarPoint> {
    if mapped_ids.is_empty() || new_ids.is_empty() {
        return HashMap::new();
    }
    let mut sample = mapped_ids.to_vec();
    if sample.len() > PLACE_SAMPLE {
        sample.sort_by_key(|id| hash(*id));
        sample.truncate(PLACE_SAMPLE);
    }
    let vectors: Vec<Vec<f32>> = sample.par_iter().map(|id| vector(*id)).collect();
    new_ids
        .par_iter()
        .filter_map(|&id| {
            let v = vector(id);
            // The 3 best so far, best first; on equal likeness the one met first stays ahead.
            let mut best = [(f64::MIN, -1i64); 3];
            for (m, s) in vectors.iter().map(|w| dot(&v, w)).enumerate() {
                // As C#: anything not better than the third is passed over (a likeness that isn't a number isn't).
                if s <= best[2].0 {
                    continue;
                }
                best[2] = (s, sample[m]);
                best.sort_by(|p, q| dotnet_cmp(q.0, p.0));
            }
            let near: Vec<&StarPoint> = best.iter().filter(|b| b.1 >= 0).filter_map(|b| map.get(&b.1)).collect();
            if near.is_empty() {
                return None;
            }
            // As C#'s Average of floats: added up in double, the answer back in single precision.
            let average = |f: fn(&StarPoint) -> f32| {
                (near.iter().map(|p| f64::from(f(p))).sum::<f64>() / near.len() as f64) as f32
            };
            let (x, y) = (average(|p| p.x), average(|p| p.y));
            let mut jitter = DotnetRandom::new((id % i64::from(i32::MAX)) as i32);
            let x = (f64::from(x) + (jitter.next_double() - 0.5) * 0.004).clamp(0.0, 1.0);
            let y = (f64::from(y) + (jitter.next_double() - 0.5) * 0.004).clamp(0.0, 1.0);
            Some((id, StarPoint { x: x as f32, y: y as f32 }))
        })
        .collect()
}

/// .NET's `double.CompareTo`: a value that isn't a number is the smallest.
fn dotnet_cmp(a: f64, b: f64) -> std::cmp::Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal),
    }
}

/// Spreads ids evenly, so taking the first few of a big library is a fair sample.
fn hash(id: i64) -> u64 {
    let x = (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ (x >> 31)
}

/// As C#'s `GalaxyMap.Dot`, which adds the products eight at a time (one per lane of .NET's `Vector<float>` on a
/// computer with AVX2) and then the lanes together, pairwise as .NET does.
fn lane_dot(a: &[f32], b: &[f32]) -> f64 {
    const W: usize = 8;
    let mut acc = [0f32; W];
    let mut i = 0;
    while i + W <= a.len() {
        for l in 0..W {
            acc[l] += a[i + l] * b[i + l];
        }
        i += W;
    }
    let v: [f32; 4] = std::array::from_fn(|l| acc[l] + acc[l + 4]);
    let mut s = f64::from((v[0] + v[1]) + (v[2] + v[3]));
    while i < a.len() {
        s += f64::from(a[i] * b[i]);
        i += 1;
    }
    s
}

/// A sound's fingerprint, its numbers squashed to the top directions, row by row.
struct Reduced {
    dims: usize,
    rows: Vec<f32>,
}

impl Reduced {
    fn row(&self, i: usize) -> &[f32] {
        &self.rows[i * self.dims..(i + 1) * self.dims]
    }
}

/// The top 32 principal directions (learned from up to 20,000 sounds), every sound projected onto them, and a 2D
/// starting layout from the first two, as x,y pairs.
fn pca(
    ids: &[i64],
    vector: &(dyn Fn(i64) -> Vec<f32> + Sync),
    rng: &mut DotnetRandom,
    stop: &dyn Fn() -> bool,
) -> Option<(Reduced, Vec<f64>)> {
    let n = ids.len();
    let sample_ids: Vec<i64> = if n <= PCA_SAMPLE {
        ids.to_vec()
    } else {
        let mut s = ids.to_vec();
        s.sort_by_key(|id| hash(*id));
        s.truncate(PCA_SAMPLE);
        s
    };
    let x: Vec<Vec<f32>> = sample_ids.par_iter().map(|id| vector(*id)).collect();
    let count = x.len() as f64;
    let d = x.iter().map(Vec::len).max().unwrap_or(0);
    let dims = DIMS.min(d);
    let mut mean = vec![0f64; d];
    for v in &x {
        for (m, a) in mean.iter_mut().zip(v) {
            *m += f64::from(*a);
        }
    }
    for m in &mut mean {
        *m /= count;
    }

    // The covariance matrix. Each measurement's centred values sit in one row, so a pair is a straight dot product.
    let cols: Vec<Vec<f32>> = (0..d)
        .into_par_iter()
        .map(|a| x.iter().map(|v| (f64::from(v.get(a).copied().unwrap_or(0.0)) - mean[a]) as f32).collect())
        .collect();
    let upper: Vec<Vec<f64>> =
        (0..d).into_par_iter().map(|a| (a..d).map(|b| lane_dot(&cols[a], &cols[b]) / count).collect()).collect();
    let mut cov = vec![0f64; d * d];
    for (a, row) in upper.iter().enumerate() {
        for (t, c) in row.iter().enumerate() {
            cov[a * d + a + t] = *c;
            cov[(a + t) * d + a] = *c;
        }
    }
    drop(cols);
    if stop() {
        return None;
    }

    let mut comps: Vec<Vec<f64>> = Vec::new();
    for _ in 0..dims {
        if stop() {
            return None;
        }
        let mut w: Vec<f64> = (0..d).map(|_| rng.next_double() - 0.5).collect();
        for _ in 0..60 {
            let mut next: Vec<f64> =
                cov.par_chunks(d).map(|row| row.iter().zip(&w).fold(0.0, |s, (c, x)| s + c * x)).collect();
            // Kept at right angles to the directions already found.
            for p in &comps {
                let dot = next.iter().zip(p).fold(0.0, |s, (a, b)| s + a * b);
                for (a, b) in next.iter_mut().zip(p) {
                    *a -= dot * b;
                }
            }
            let norm = next.iter().fold(0.0, |s, t| s + t * t).sqrt();
            if norm < 1e-12 {
                break;
            }
            for a in &mut next {
                *a /= norm;
            }
            w = next;
        }
        if w.iter().all(|t| *t == 0.0) {
            break;
        }
        comps.push(w);
    }

    let dims = comps.len();
    let mut rows = vec![0f32; n * dims];
    if dims > 0 {
        rows.par_chunks_mut(dims).zip(ids.par_iter()).for_each(|(r, id)| {
            let v = vector(*id);
            for (out, comp) in r.iter_mut().zip(&comps) {
                let mut s = 0.0;
                for j in 0..v.len().min(d) {
                    s += (f64::from(v[j]) - mean[j]) * comp[j];
                }
                *out = s as f32;
            }
        });
    }
    let reduced = Reduced { dims, rows };
    let mut init = vec![0f64; 2 * n];
    for i in 0..n {
        let r = reduced.row(i);
        init[2 * i] = r.first().map_or(0.0, |x| f64::from(*x));
        init[2 * i + 1] = r.get(1).map_or(0.0, |x| f64::from(*x));
    }
    // Scaled to about ±10 (UMAP's usual range), with a whisker of noise so identical sounds don't stack. A big library
    // starts spread wider: every star wants about the same room.
    let max = init.iter().fold(1e-9f64, |m, x| m.max(x.abs()));
    let room = 10.0 * (n as f64 / 10_000.0).sqrt().max(1.0);
    for x in &mut init {
        *x = *x / max * room + (rng.next_double() - 0.5) * 0.01;
    }
    Some((reduced, init))
}

fn distance2(a: &[f32], b: &[f32]) -> f32 {
    let mut s = 0f32;
    for (x, y) in a.iter().zip(b) {
        let t = x - y;
        s += t * t;
    }
    s
}

type Neighbours = (Vec<Vec<usize>>, Vec<Vec<f32>>);

fn exact_neighbours(x: &Reduced, n: usize, k: usize) -> Neighbours {
    (0..n)
        .into_par_iter()
        .map(|i| {
            let mut best = KBest::new(k);
            for j in 0..n {
                if j != i {
                    best.offer(j, distance2(x.row(i), x.row(j)));
                }
            }
            best.sorted()
        })
        .unzip()
}

/// NN-descent: start each sound with likely neighbours (its neighbours along a few random directions), then
/// repeatedly check its neighbours' neighbours, keeping the closest. Each round only a sound's own list changes, so
/// every core can work at once. Stops when a round barely improves anything.
fn descent_neighbours(x: &Reduced, n: usize, k: usize, seed: i32, stop: &dyn Fn() -> bool) -> Option<Neighbours> {
    let mut best: Vec<KBest> = (0..n).map(|_| KBest::new(k)).collect();

    // Start: neighbours in sorted order along 8 random directions.
    let mut rng = DotnetRandom::new(seed);
    for _ in 0..8 {
        if stop() {
            return None;
        }
        let dir: Vec<f32> = (0..x.dims).map(|_| (rng.next_double() - 0.5) as f32).collect();
        let keys: Vec<f32> =
            (0..n).into_par_iter().map(|i| x.row(i).iter().zip(&dir).fold(0f32, |s, (a, b)| s + a * b)).collect();
        let mut idx: Vec<usize> = (0..n).collect();
        idx.par_sort_by(|a, b| keys[*a].total_cmp(&keys[*b]));
        let window = (k / 2).max(2);
        // Each sound's place in that order, so every sound can gather its own window at once.
        let mut rank = vec![0usize; n];
        for (r, i) in idx.iter().enumerate() {
            rank[*i] = r;
        }
        best.par_iter_mut().enumerate().for_each(|(i, b)| {
            let r = rank[i];
            let from = r.saturating_sub(window);
            for (o, &j) in idx[from..(r + window + 1).min(n)].iter().enumerate() {
                if from + o != r {
                    b.offer(j, distance2(x.row(i), x.row(j)));
                }
            }
        });
    }

    for _ in 0..10 {
        if stop() {
            return None;
        }
        // Each sound's candidates: its neighbours, plus up to 10 sounds that list it as theirs.
        let lists: Vec<Vec<usize>> = best.iter().map(|b| b.items().to_vec()).collect();
        let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, list) in lists.iter().enumerate() {
            for &j in list {
                if reverse[j].len() < 10 {
                    reverse[j].push(i);
                }
            }
        }
        let cands: Vec<Vec<usize>> = lists
            .iter()
            .zip(&reverse)
            .map(|(a, b)| {
                let mut seen = HashSet::new();
                a.iter().chain(b).copied().filter(|c| seen.insert(*c)).collect()
            })
            .collect();
        let changed = AtomicUsize::new(0);
        best.par_iter_mut().enumerate().for_each(|(i, b)| {
            let mut local = 0;
            for &u in &cands[i] {
                for &v in &cands[u] {
                    if v != i && b.offer(v, distance2(x.row(i), x.row(v))) {
                        local += 1;
                    }
                }
            }
            changed.fetch_add(local, Ordering::Relaxed);
        });
        if changed.load(Ordering::Relaxed) < n * k / 500 {
            break;
        }
    }
    Some(best.into_par_iter().map(|b| b.sorted()).unzip())
}

/// The `k` closest seen so far (squared distances), no repeats. As C#'s `KBest`, step for step: which one it drops
/// on a tie decides which sound-alikes a sound gets.
struct KBest {
    k: usize,
    ids: Vec<usize>,
    d: Vec<f32>,
    /// Where the farthest kept one is.
    worst: usize,
}

impl KBest {
    fn new(k: usize) -> KBest {
        KBest { k, ids: Vec::with_capacity(k), d: Vec::with_capacity(k), worst: 0 }
    }

    fn offer(&mut self, id: usize, d2: f32) -> bool {
        if self.k == 0 || (self.ids.len() == self.k && d2 >= self.d[self.worst]) || self.ids.contains(&id) {
            return false;
        }
        if self.ids.len() < self.k {
            self.ids.push(id);
            self.d.push(d2);
        } else {
            self.ids[self.worst] = id;
            self.d[self.worst] = d2;
        }
        self.worst = 0;
        for t in 1..self.d.len() {
            if self.d[t] > self.d[self.worst] {
                self.worst = t;
            }
        }
        true
    }

    fn items(&self) -> &[usize] {
        &self.ids
    }

    /// Closest first, as distances (no longer squared).
    fn sorted(self) -> (Vec<usize>, Vec<f32>) {
        let (mut d, mut ids) = (self.d, self.ids);
        dotnet_sort_pairs(&mut d, &mut ids);
        for x in &mut d {
            *x = x.sqrt();
        }
        (ids, d)
    }
}

/// `Array.Sort(keys, items)` as .NET does it, for the short lists `KBest` keeps (at most 16, sorted by insertion,
/// and 2 or 3 by fixed swaps), so equal distances come out in the same order. Longer lists fall back to a stable sort.
fn dotnet_sort_pairs(keys: &mut [f32], items: &mut [usize]) {
    let swap_if_greater = |keys: &mut [f32], items: &mut [usize], i: usize, j: usize| {
        if keys[i] > keys[j] {
            keys.swap(i, j);
            items.swap(i, j);
        }
    };
    match keys.len() {
        0 | 1 => {}
        2 => swap_if_greater(keys, items, 0, 1),
        3 => {
            swap_if_greater(keys, items, 0, 1);
            swap_if_greater(keys, items, 0, 2);
            swap_if_greater(keys, items, 1, 2);
        }
        n if n <= 16 => {
            for i in 0..n - 1 {
                let (t, ti) = (keys[i + 1], items[i + 1]);
                let mut j = i as isize;
                while j >= 0 && t < keys[j as usize] {
                    keys[j as usize + 1] = keys[j as usize];
                    items[j as usize + 1] = items[j as usize];
                    j -= 1;
                }
                keys[(j + 1) as usize] = t;
                items[(j + 1) as usize] = ti;
            }
        }
        _ => {
            let mut pairs: Vec<(f32, usize)> = keys.iter().copied().zip(items.iter().copied()).collect();
            pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (t, (key, item)) in pairs.into_iter().enumerate() {
                keys[t] = key;
                items[t] = item;
            }
        }
    }
}

/// A pair of neighbours and how strongly they belong together.
#[derive(Clone, Copy, Debug)]
struct Edge {
    i: usize,
    j: usize,
    w: f32,
}

/// How strongly each pair of neighbours belongs together (UMAP's fuzzy weights, made symmetric).
fn edges(nbr: &[Vec<usize>], dist: &[Vec<f32>]) -> Vec<Edge> {
    let n = nbr.len();
    let target = (nbr.iter().map(Vec::len).max().unwrap_or(0).max(2) as f64).log2();
    let mut directed: Vec<(u64, f32)> = nbr
        .par_iter()
        .zip(dist)
        .enumerate()
        .flat_map_iter(|(i, (nbr, dist))| {
            let rho = dist.first().copied().unwrap_or(0.0);
            let weight = move |dd: f32, sigma: f64| (-f64::from((dd - rho).max(0.0)) / sigma).exp();
            // The width that makes the weights add up to log2(k), found by halving.
            let (mut lo, mut hi, mut sigma) = (1e-6, 1e3, 1.0);
            for _ in 0..48 {
                sigma = (lo + hi) / 2.0;
                let sum = dist.iter().fold(0.0, |s, dd| s + weight(*dd, sigma));
                if sum > target {
                    hi = sigma;
                } else {
                    lo = sigma;
                }
            }
            nbr.iter().zip(dist).map(move |(&j, &dd)| {
                let (a, b) = if i < j { (i, j) } else { (j, i) };
                ((a * n + b) as u64, weight(dd, sigma) as f32)
            })
        })
        .collect();
    // The same pair from both ends joins as a + b - ab.
    directed.par_sort_by_key(|(key, _)| *key);
    let mut edges = Vec::with_capacity(directed.len());
    let mut t = 0;
    while t < directed.len() {
        let (key, mut w) = directed[t];
        if t + 1 < directed.len() && directed[t + 1].0 == key {
            t += 1;
            let v = directed[t].1;
            w = w + v - w * v;
        }
        edges.push(Edge { i: (key / n as u64) as usize, j: (key % n as u64) as usize, w });
        t += 1;
    }
    edges
}

/// Star positions, x,y pairs, that an epoch can nudge: plain cells on one thread, or shared by every core.
trait Positions {
    fn get(&self, i: usize) -> f64;
    fn add(&self, i: usize, by: f64);
}

/// One thread's positions.
impl Positions for [Cell<f64>] {
    fn get(&self, i: usize) -> f64 {
        self[i].get()
    }
    fn add(&self, i: usize, by: f64) {
        self[i].set(self[i].get() + by);
    }
}

/// Positions every core nudges at once. Like UMAP, a nudge now and then lost to another core's is allowed: each one
/// is tiny.
impl Positions for [AtomicU64] {
    fn get(&self, i: usize) -> f64 {
        f64::from_bits(self[i].load(Ordering::Relaxed))
    }
    fn add(&self, i: usize, by: f64) {
        let v = Positions::get(self, i) + by;
        self[i].store(v.to_bits(), Ordering::Relaxed);
    }
}

fn max_weight(edges: &[Edge]) -> f32 {
    edges.iter().fold(1e-6f32, |m, e| m.max(e.w))
}

fn optimise(
    pos: &mut [f64],
    edges: &[Edge],
    n: usize,
    epochs: usize,
    rng: &mut DotnetRandom,
    stop: &dyn Fn() -> bool,
) -> Option<()> {
    let max_w = max_weight(edges);
    let cells = Cell::from_mut(pos).as_slice_of_cells();
    for epoch in 0..epochs {
        if stop() {
            return None;
        }
        run_epoch(cells, edges, n, 1.0 - epoch as f64 / epochs as f64, max_w, rng);
    }
    Some(())
}

/// The same rounds on every core: each takes a slice of the pairs, with a random sequence of its own.
fn optimise_parallel(
    pos: Vec<f64>,
    edges: &[Edge],
    n: usize,
    epochs: usize,
    seed: i32,
    stop: &dyn Fn() -> bool,
) -> Option<Vec<f64>> {
    let max_w = max_weight(edges);
    let shared: Vec<AtomicU64> = pos.into_iter().map(|x| AtomicU64::new(x.to_bits())).collect();
    let slices = (rayon::current_num_threads() * 4).max(1);
    let size = edges.len().div_ceil(slices).max(1);
    for epoch in 0..epochs {
        if stop() {
            return None;
        }
        let alpha = 1.0 - epoch as f64 / epochs as f64;
        edges.par_chunks(size).enumerate().for_each(|(s, slice)| {
            let mut rng =
                DotnetRandom::new(seed.wrapping_add((epoch as i32).wrapping_mul(7919)).wrapping_add(s as i32));
            run_epoch(shared.as_slice(), slice, n, alpha, max_w, &mut rng);
        });
    }
    Some(shared.into_iter().map(|x| f64::from_bits(x.into_inner())).collect())
}

fn run_epoch<P: Positions + ?Sized>(pos: &P, edges: &[Edge], n: usize, alpha: f64, max_w: f32, rng: &mut DotnetRandom) {
    let clip = |v: f64| v.clamp(-4.0, 4.0);
    for e in edges {
        let (i, j) = (e.i, e.j);
        // Stronger pairs get a pull more often, like UMAP's edge sampling.
        if rng.next_double() > f64::from(e.w / max_w) {
            continue;
        }
        let dx = pos.get(2 * i) - pos.get(2 * j);
        let dy = pos.get(2 * i + 1) - pos.get(2 * j + 1);
        let d2 = dx * dx + dy * dy;
        if d2 > 0.0 {
            let pb = d2.powf(B - 1.0);
            let g = -2.0 * A * B * pb / (1.0 + A * pb * d2);
            let gx = clip(g * dx) * alpha;
            let gy = clip(g * dy) * alpha;
            pos.add(2 * i, gx);
            pos.add(2 * i + 1, gy);
            pos.add(2 * j, -gx);
            pos.add(2 * j + 1, -gy);
        }
        for _ in 0..NEGATIVE_SAMPLES {
            let k = rng.next_in(0, n as i32) as usize;
            if k == i {
                continue;
            }
            let dx = pos.get(2 * i) - pos.get(2 * k);
            let dy = pos.get(2 * i + 1) - pos.get(2 * k + 1);
            let d2 = dx * dx + dy * dy;
            let g = 2.0 * B / ((0.001 + d2) * (1.0 + A * d2.powf(B)));
            pos.add(2 * i, clip(g * dx) * alpha);
            pos.add(2 * i + 1, clip(g * dy) * alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `per` sounds in each of `clusters` families: each family around a corner of its own, a little spread.
    fn families(clusters: usize, per: usize, dim: usize) -> HashMap<i64, Vec<f32>> {
        let mut state = 12345u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        };
        let mut out = HashMap::new();
        for c in 0..clusters {
            for s in 0..per {
                let mut v: Vec<f32> = (0..dim).map(|_| next() * 0.2).collect();
                v[c % dim] += 3.0;
                out.insert((c * per + s + 1) as i64, v);
            }
        }
        out
    }

    fn run(vectors: &HashMap<i64, Vec<f32>>, exact_below: usize) -> HashMap<i64, StarPoint> {
        let ids: Vec<i64> = vectors.keys().copied().collect();
        let get = |id: i64| vectors[&id].clone();
        layout(&ids, &get, SEED, exact_below, &|_| {}, &AtomicBool::new(false)).unwrap()
    }

    fn gap(a: StarPoint, b: StarPoint) -> f32 {
        ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
    }

    /// The average distance within families, and between them.
    fn spread(map: &HashMap<i64, StarPoint>, per: usize) -> (f32, f32) {
        let (mut within, mut across, mut nw, mut na) = (0f32, 0f32, 0, 0);
        for (a, pa) in map {
            for (b, pb) in map {
                if a >= b {
                    continue;
                }
                if (a - 1) / per as i64 == (b - 1) / per as i64 {
                    within += gap(*pa, *pb);
                    nw += 1;
                } else {
                    across += gap(*pa, *pb);
                    na += 1;
                }
            }
        }
        (within / nw as f32, across / na as f32)
    }

    #[test]
    fn one_or_two_sounds_sit_in_the_middle() {
        let v = families(1, 2, 4);
        let map = run(&v, EXACT_BELOW);
        assert_eq!(map[&1], StarPoint { x: 0.3, y: 0.5 });
        assert_eq!(map[&2], StarPoint { x: 0.3 + 0.4, y: 0.5 });
        assert!(run(&HashMap::new(), EXACT_BELOW).is_empty());
    }

    #[test]
    fn families_gather_and_a_small_library_always_gives_the_same_map() {
        let v = families(4, 40, 24);
        let map = run(&v, EXACT_BELOW);
        assert_eq!(map.len(), 160);
        assert!(map.values().all(|p| (0.07..=0.93).contains(&p.x) && (0.07..=0.93).contains(&p.y)));
        let (within, across) = spread(&map, 40);
        assert!(within * 3.0 < across, "{within} {across}");
        assert_eq!(map, run(&v, EXACT_BELOW));
    }

    #[test]
    fn a_big_library_finds_its_neighbours_by_descent() {
        // Laid out as a big library would be (neighbours of neighbours, every core at once).
        let v = families(5, 60, 16);
        let map = run(&v, 0);
        assert_eq!(map.len(), 300);
        let (within, across) = spread(&map, 60);
        assert!(within * 3.0 < across, "{within} {across}");
    }

    #[test]
    fn descent_finds_nearly_every_exact_neighbour() {
        let v = families(3, 100, 8);
        let mut ids: Vec<i64> = v.keys().copied().collect();
        ids.sort_unstable();
        let rows: Vec<f32> = ids.iter().flat_map(|id| v[id].iter().copied()).collect();
        let x = Reduced { dims: 8, rows };
        let (exact, _) = exact_neighbours(&x, 300, 15);
        let (found, _) = descent_neighbours(&x, 300, 15, SEED, &|| false).unwrap();
        let hits: usize = exact.iter().zip(&found).map(|(e, f)| e.iter().filter(|j| f.contains(j)).count()).sum();
        assert!(hits as f64 > 0.95 * (300 * 15) as f64, "{hits}");
    }

    #[test]
    fn a_new_sound_joins_its_family() {
        let v = families(3, 30, 12);
        let map = run(&v, EXACT_BELOW);
        let mut all = v.clone();
        let mut new = v[&5].clone();
        new[0] += 0.01;
        all.insert(1000, new);
        let get = |id: i64| all[&id].clone();
        let mut mapped: Vec<i64> = v.keys().copied().collect();
        mapped.sort_unstable();
        let placed = place_new(&[1000], &mapped, &get, &map);
        let p = placed[&1000];
        let family: Vec<StarPoint> = (1..=30).map(|id| map[&id]).collect();
        let others: Vec<StarPoint> = (31..=90).map(|id| map[&id]).collect();
        let near = |ps: &[StarPoint]| ps.iter().map(|q| gap(p, *q)).fold(f32::MAX, f32::min);
        assert!(near(&family) < near(&others));
        // The nudge is the same every time.
        assert_eq!(placed, place_new(&[1000], &mapped, &get, &map));
        assert!(place_new(&[1000], &[], &get, &map).is_empty());
    }

    #[test]
    fn cancelling_stops_the_layout() {
        let v = families(2, 20, 6);
        let ids: Vec<i64> = v.keys().copied().collect();
        let get = |id: i64| v[&id].clone();
        assert!(layout(&ids, &get, SEED, EXACT_BELOW, &|_| {}, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn short_lists_sort_as_dotnet_sorts_them() {
        // Three: .NET's fixed swaps put the two equal ones the other way round.
        let (mut k, mut v) = (vec![1.0, 1.0, 0.0], vec![0, 1, 2]);
        dotnet_sort_pairs(&mut k, &mut v);
        assert_eq!((k, v), (vec![0.0, 1.0, 1.0], vec![2, 1, 0]));
        // More: by insertion, so equal ones keep their order.
        let (mut k, mut v) = (vec![2.0, 1.0, 1.0, 0.5], vec![0, 1, 2, 3]);
        dotnet_sort_pairs(&mut k, &mut v);
        assert_eq!((k, v), (vec![0.5, 1.0, 1.0, 2.0], vec![3, 1, 2, 0]));
    }

    #[test]
    fn the_lane_dot_is_a_dot_product() {
        let a: Vec<f32> = (0..21).map(|i| i as f32 * 0.25).collect();
        let b: Vec<f32> = (0..21).map(|i| 1.0 - i as f32 * 0.1).collect();
        assert!((lane_dot(&a, &b) - dot(&a, &b)).abs() < 1e-4);
    }

    #[test]
    fn the_space_follows_deep_listening() {
        let features: HashMap<i64, Vec<f32>> = (1..=10).map(|i| (i, vec![i as f32, 1.0, 2.0])).collect();
        let clap: HashMap<i64, Vec<f32>> = (1..=10).map(|i| (i, vec![1.0, i as f32])).collect();
        let shallow = SoundIndex::build(&features, &HashMap::new(), 10);
        assert_eq!(space(Placing::Both, &shallow), "features");
        let deep = SoundIndex::build(&features, &clap, 10);
        assert_eq!(space(Placing::Both, &deep), "clap+features");
        assert_eq!(space(Placing::Clap, &deep), "clap");
        assert_eq!(space(Placing::Own, &deep), "features");
        // Joined: CLAP then the measurements, one unit-length fingerprint.
        let joined = fingerprints(&deep, "clap+features").get(3);
        assert_eq!(joined.len(), 5);
        assert!((joined.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(fingerprints(&deep, "features").get(3), deep.features.get(3).unwrap());
    }

    #[test]
    fn update_lays_out_afresh_or_places_the_new_sounds() {
        let v = families(3, 20, 6);
        let index = SoundIndex::build(&v, &HashMap::new(), v.len());
        let quiet = |_| {};
        let first = update(&index, "features", HashMap::new(), false, &quiet, &AtomicBool::new(false)).unwrap();
        assert!(first.replace);
        assert_eq!(first.map.len(), 60);
        // Two sounds missing from the saved map: only they are placed, the rest stay put.
        let mut saved = first.map.clone();
        saved.remove(&3);
        saved.remove(&40);
        let second = update(&index, "features", saved.clone(), false, &quiet, &AtomicBool::new(false)).unwrap();
        assert!(!second.replace);
        assert_eq!(second.save.keys().copied().collect::<HashSet<_>>(), HashSet::from([3, 40]));
        assert!(saved.iter().all(|(id, p)| second.map[id] == *p));
        assert!(!out_of_date(&index, &second.map));
        assert!(out_of_date(&index, &saved));
    }
}
