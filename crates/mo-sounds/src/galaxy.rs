//! The sound galaxy: every sound becomes a point on a flat map, and sounds that sound alike end up close together.
//! What it does follows the C# app's `GalaxyMap` (and the galaxy part of `SoundOrganiser`); how it does it is Rust's
//! own (see DIFFERENCES.md).
//!
//! It works like UMAP, cut down to what a sample library needs:
//! 1. Squash each sound's fingerprint to its 32 most telling directions (PCA, learned from a fair sample of at most
//!    20,000 sounds, which pins those directions down as well as the whole library would), so the search is fast.
//! 2. Find each sound's 15 nearest sound-alikes: exactly below 6,000 sounds, and above by "neighbours of
//!    neighbours" (NN-descent), which gets almost all of them right in a fraction of the time.
//! 3. Start from the PCA picture, then let close sound-alikes pull together and random pairs push apart for a few
//!    hundred rounds, until the clusters settle. Every round is spread over every core, and every star's moves come
//!    from a random sequence of its own, so the same library always gives the same map, however many cores.
//!
//! The map is worked out once and saved; new sounds are then placed beside their nearest sound-alikes
//! ([`place_new`]), so the stars you know don't move.
//!
//! Laying out a big library takes a while and uses every core: call it on the app's low-priority pool
//! (`mo_app::background::on_pool`), never the window's thread.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::StarPoint;
use crate::index::{SoundIndex, unit};

/// How many sound-alikes each sound is tied to.
pub const NEIGHBOURS: usize = 15;
/// Libraries smaller than this get exact neighbours; bigger ones, neighbours of neighbours.
pub const EXACT_BELOW: usize = 6000;
/// The seed the app lays out with.
pub const SEED: u64 = 42;
const DIMS: usize = 32;
const PCA_SAMPLE: usize = 20_000;
const NEGATIVE_SAMPLES: usize = 5;
/// Placing new sounds compares each with at least this many mapped sounds (all of them, in a smaller library)…
const PLACE_SAMPLE: usize = 10_000;
/// …and with more while the comparisons stay under this many…
const PLACE_BUDGET: usize = 500_000_000;
/// …but never more than this many (so a joined fingerprint per mapped sound never takes more than a few hundred MB).
const PLACE_MOST: usize = 50_000;
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

/// The fingerprints a galaxy is laid out from: every sound the features index has, in id order, and each one's
/// fingerprint. As C#'s `SoundOrganiser.GalaxyVectors`.
///
/// "clap+features" joins CLAP's "what it is" (80 %) to the measured "how it sounds" (60 %), so neighbours match in
/// kind and in character. A sound deep listening hasn't heard yet falls back to its measurements in every space.
/// The measurements alone are lent straight from the index; joined ones are made when asked for.
pub struct Fingerprints<'a> {
    pub ids: &'a [i64],
    index: &'a SoundIndex,
    /// Joined with CLAP (and how); `None` for the measurements alone.
    clap: Option<(usize, bool)>,
}

impl<'a> Fingerprints<'a> {
    /// One sound's fingerprint; empty for a sound the features index doesn't have.
    pub fn get(&self, id: i64) -> Cow<'a, [f32]> {
        let Some(f) = self.index.features.get(id) else { return Cow::Borrowed(&[]) };
        let Some((clap_dim, clap_only)) = self.clap else { return Cow::Borrowed(f) };
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
        Cow::Owned(v)
    }
}

/// The fingerprints for `space` (see [`space`]).
pub fn fingerprints<'a>(index: &'a SoundIndex, space: &str) -> Fingerprints<'a> {
    let clap = if space == "features" { None } else { index.clap.as_ref() };
    let clap = clap.map(|c| c.dim()).filter(|d| *d > 0);
    Fingerprints { ids: index.features.ids(), index, clap: clap.map(|d| (d, space == "clap")) }
}

/// True when the saved `map` lacks some sound the features index has. As C#'s `SoundOrganiser.GalaxyOutOfDate`.
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
        let map = layout(prints.ids, &vector, SEED, progress, cancel)?;
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

/// A sound's fingerprint, lent or made when asked for. It must be safe to ask from several threads at once.
pub type Vector<'v> = dyn Fn(i64) -> Cow<'v, [f32]> + Sync + 'v;

/// Lays out the sounds `ids`, fetching each fingerprint from `vector` when it's needed. The same ids and seed always
/// give the same map. `None` when `cancel` was set. As C#'s `GalaxyMap.Layout`.
pub fn layout(
    ids: &[i64],
    vector: &Vector<'_>,
    seed: u64,
    progress: &(dyn Fn(Step) + Sync),
    cancel: &AtomicBool,
) -> Option<HashMap<i64, StarPoint>> {
    layout_with(ids, vector, seed, EXACT_BELOW, progress, cancel)
}

/// [`layout`], with exact neighbours only below `exact_below` sounds (so the tests can try neighbours of neighbours
/// on a small library).
fn layout_with(
    ids: &[i64],
    vector: &Vector<'_>,
    seed: u64,
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

    progress(Step { what: "Finding what each sound is like", done: 0 });
    let (reduced, mut pos) = pca(&order, vector, seed, &stop)?;

    progress(Step { what: "Finding sound-alikes", done: 1 });
    let k = NEIGHBOURS.min(n - 1);
    let (nbr, dist) = if n < exact_below {
        exact_neighbours(&reduced, n, k)
    } else {
        descent_neighbours(&reduced, n, k, seed, &stop)?
    };

    progress(Step { what: "Weighing sound-alikes", done: 2 });
    let ties = Ties::new(n, &edges(&nbr, &dist));
    drop((nbr, dist, reduced));

    progress(Step { what: "Letting the galaxy settle", done: 3 });
    let epochs = if n <= 10_000 {
        400
    } else if n <= 100_000 {
        200
    } else {
        120
    };
    settle(&mut pos, &ties, epochs, seed, &stop)?;

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
    for p in pos.as_chunks::<2>().0.iter() {
        mx += p[0];
        my += p[1];
    }
    mx /= n as f64;
    my /= n as f64;
    let dist: Vec<f64> =
        pos.as_chunks::<2>().0.iter().map(|p| ((p[0] - mx).powi(2) + (p[1] - my).powi(2)).sqrt()).collect();
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
    for p in pos.as_chunks::<2>().0.iter() {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
    }
    let span = (max_x - min_x).max(max_y - min_y).max(1e-9);
    let off_x = (1.0 - (max_x - min_x) / span) / 2.0;
    let off_y = (1.0 - (max_y - min_y) / span) / 2.0;
    for p in pos.as_chunks_mut::<2>().0.iter_mut() {
        p[0] = 0.07 + 0.86 * (off_x + (p[0] - min_x) / span);
        p[1] = 0.07 + 0.86 * (off_y + (p[1] - min_y) / span);
    }
}

/// Places sounds the map doesn't have yet at the middle of their 3 nearest mapped sound-alikes, nudged a little so
/// they don't sit exactly on top of one. Each is compared with every mapped sound when there are few new ones; with
/// many, with a fair sample (at least 10,000), which is plenty to find their neighbourhood. As C#'s
/// `GalaxyMap.PlaceNew`.
pub fn place_new(
    new_ids: &[i64],
    mapped_ids: &[i64],
    vector: &Vector<'_>,
    map: &HashMap<i64, StarPoint>,
) -> HashMap<i64, StarPoint> {
    if mapped_ids.is_empty() || new_ids.is_empty() {
        return HashMap::new();
    }
    let size = (PLACE_BUDGET / new_ids.len()).clamp(PLACE_SAMPLE, PLACE_MOST);
    let mut sample = mapped_ids.to_vec();
    if sample.len() > size {
        sample.sort_by_key(|id| hash(*id));
        sample.truncate(size);
    }
    let vectors: Vec<Cow<'_, [f32]>> = sample.par_iter().map(|id| vector(*id)).collect();
    new_ids
        .par_iter()
        .filter_map(|&id| {
            let v = vector(id);
            // The 3 most alike, most alike first; on a tie, the one met first.
            let mut best = [(f32::NEG_INFINITY, None::<i64>); 3];
            for (w, other) in vectors.iter().zip(&sample) {
                let s = dot(&v, w);
                if s > best[2].0 {
                    best[2] = (s, Some(*other));
                    best.sort_by(|p, q| q.0.total_cmp(&p.0));
                }
            }
            let near: Vec<&StarPoint> = best.iter().filter_map(|b| b.1.and_then(|id| map.get(&id))).collect();
            if near.is_empty() {
                return None;
            }
            let x = near.iter().map(|p| f64::from(p.x)).sum::<f64>() / near.len() as f64;
            let y = near.iter().map(|p| f64::from(p.y)).sum::<f64>() / near.len() as f64;
            let mut jitter = Rng::new(hash(id));
            let x = (x + (jitter.unit() - 0.5) * 0.004).clamp(0.0, 1.0);
            let y = (y + (jitter.unit() - 0.5) * 0.004).clamp(0.0, 1.0);
            Some((id, StarPoint { x: x as f32, y: y as f32 }))
        })
        .collect()
}

/// Spreads ids evenly, so taking the first few of a big library is a fair sample.
fn hash(id: i64) -> u64 {
    mix(id as u64)
}

/// SplitMix64's mixer: every bit of `x` stirs every bit of the answer.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A small, fast random sequence (SplitMix64). Each star gets its own for each round, from the seed, the round and
/// the star, so what happens to it doesn't depend on which core did it or in what order.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn of(seed: u64, a: u64, b: u64) -> Rng {
        Rng(mix(mix(seed ^ mix(a)) ^ b))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.0)
    }

    /// 0 up to (not including) 1.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// 0 up to (not including) `n`.
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next()) * n as u128) >> 64) as usize
    }
}

/// The dot product, eight at a time (so it runs on the processor's wide registers), then the eight added up.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<8>(), b.as_chunks::<8>());
    for (x, y) in ca.iter().zip(cb) {
        for l in 0..8 {
            acc[l] += x[l] * y[l];
        }
    }
    let mut s = acc.iter().sum::<f32>();
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

/// Each sound's fingerprint squashed to the top directions, one row each.
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
fn pca(ids: &[i64], vector: &Vector<'_>, seed: u64, stop: &dyn Fn() -> bool) -> Option<(Reduced, Vec<f64>)> {
    let n = ids.len();
    let mut rng = Rng::of(seed, 1, 0);
    let mut sample_ids = ids.to_vec();
    if n > PCA_SAMPLE {
        sample_ids.sort_by_key(|id| hash(*id));
        sample_ids.truncate(PCA_SAMPLE);
    }
    let x: Vec<Cow<'_, [f32]>> = sample_ids.par_iter().map(|id| vector(*id)).collect();
    let count = x.len() as f64;
    let d = x.iter().map(|v| v.len()).max().unwrap_or(0);
    let mut mean = vec![0f64; d];
    for v in &x {
        for (m, a) in mean.iter_mut().zip(v.iter()) {
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
    drop(x);
    let upper: Vec<Vec<f64>> =
        (0..d).into_par_iter().map(|a| (a..d).map(|b| f64::from(dot(&cols[a], &cols[b])) / count).collect()).collect();
    drop(cols);
    let mut cov = vec![0f64; d * d];
    for (a, row) in upper.iter().enumerate() {
        for (t, c) in row.iter().enumerate() {
            cov[a * d + a + t] = *c;
            cov[(a + t) * d + a] = *c;
        }
    }
    if stop() {
        return None;
    }

    // The directions one at a time, by power iteration, each kept at right angles to those already found.
    let mut comps: Vec<Vec<f64>> = Vec::new();
    for _ in 0..DIMS.min(d) {
        if stop() {
            return None;
        }
        let mut w: Vec<f64> = (0..d).map(|_| rng.unit() - 0.5).collect();
        let mut found = false;
        for _ in 0..60 {
            let mut next: Vec<f64> =
                cov.par_chunks(d).map(|row| row.iter().zip(&w).map(|(c, x)| c * x).sum()).collect();
            for p in &comps {
                let along: f64 = next.iter().zip(p).map(|(a, b)| a * b).sum();
                for (a, b) in next.iter_mut().zip(p) {
                    *a -= along * b;
                }
            }
            let norm = next.iter().map(|t| t * t).sum::<f64>().sqrt();
            if norm < 1e-12 {
                break;
            }
            for a in &mut next {
                *a /= norm;
            }
            w = next;
            found = true;
        }
        // Nothing left to explain: the fingerprints vary in fewer directions than asked for.
        if !found {
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
                *out = v.iter().zip(&mean).zip(comp).map(|((x, m), c)| (f64::from(*x) - m) * c).sum::<f64>() as f32;
            }
        });
    }
    let reduced = Reduced { dims, rows };
    let mut init = vec![0f64; 2 * n];
    for (i, p) in init.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let r = reduced.row(i);
        p[0] = r.first().map_or(0.0, |x| f64::from(*x));
        p[1] = r.get(1).map_or(0.0, |x| f64::from(*x));
    }
    // Scaled to about ±10 (UMAP's usual range), with a whisker of noise so identical sounds don't stack. A big library
    // starts spread wider: every star wants about the same room.
    let max = init.iter().fold(1e-9f64, |m, x| m.max(x.abs()));
    let room = 10.0 * (n as f64 / 10_000.0).sqrt().max(1.0);
    for x in &mut init {
        *x = *x / max * room + (rng.unit() - 0.5) * 0.01;
    }
    Some((reduced, init))
}

fn distance2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
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
fn descent_neighbours(x: &Reduced, n: usize, k: usize, seed: u64, stop: &dyn Fn() -> bool) -> Option<Neighbours> {
    let mut best: Vec<KBest> = (0..n).map(|_| KBest::new(k)).collect();

    // Start: neighbours in sorted order along 8 random directions.
    let mut rng = Rng::of(seed, 2, 0);
    let window = (k / 2).max(2);
    for _ in 0..8 {
        if stop() {
            return None;
        }
        let dir: Vec<f32> = (0..x.dims).map(|_| (rng.unit() - 0.5) as f32).collect();
        let keys: Vec<f32> = (0..n).into_par_iter().map(|i| dot(x.row(i), &dir)).collect();
        let mut idx: Vec<usize> = (0..n).collect();
        idx.par_sort_by(|a, b| keys[*a].total_cmp(&keys[*b]).then(a.cmp(b)));
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
        let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, b) in best.iter().enumerate() {
            for &j in b.items() {
                if reverse[j].len() < 10 {
                    reverse[j].push(i);
                }
            }
        }
        let cands: Vec<Vec<usize>> = best
            .par_iter()
            .zip(&reverse)
            .map(|(b, r)| {
                let mut seen = HashSet::new();
                b.items().iter().chain(r).copied().filter(|c| seen.insert(*c)).collect()
            })
            .collect();
        drop(reverse);
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
    Some(best.into_par_iter().map(KBest::sorted).unzip())
}

/// The `k` closest seen so far (squared distances), no repeats.
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

    /// Closest first (on a tie, the lower number), as distances (no longer squared).
    fn sorted(self) -> (Vec<usize>, Vec<f32>) {
        let mut pairs: Vec<(f32, usize)> = self.d.into_iter().zip(self.ids).collect();
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        pairs.into_iter().map(|(d, id)| (id, d.sqrt())).unzip()
    }
}

/// How strongly each pair of neighbours belongs together (UMAP's fuzzy weights, made symmetric): `(i, j, weight)`
/// with `i < j`, each pair once.
fn edges(nbr: &[Vec<usize>], dist: &[Vec<f32>]) -> Vec<(usize, usize, f32)> {
    let target = (nbr.iter().map(Vec::len).max().unwrap_or(0).max(2) as f64).log2();
    let mut directed: Vec<(usize, usize, f32)> = nbr
        .par_iter()
        .zip(dist)
        .enumerate()
        .flat_map_iter(|(i, (nbr, dist))| {
            let rho = f64::from(dist.first().copied().unwrap_or(0.0));
            let weight = move |dd: f32, sigma: f64| (-(f64::from(dd) - rho).max(0.0) / sigma).exp();
            // The width that makes the weights add up to log2(k), found by halving.
            let (mut lo, mut hi, mut sigma) = (1e-6, 1e3, 1.0);
            for _ in 0..48 {
                sigma = (lo + hi) / 2.0;
                let sum: f64 = dist.iter().map(|dd| weight(*dd, sigma)).sum();
                if sum > target {
                    hi = sigma;
                } else {
                    lo = sigma;
                }
            }
            nbr.iter().zip(dist).map(move |(&j, &dd)| (i.min(j), i.max(j), weight(dd, sigma) as f32))
        })
        .collect();
    // The same pair from both ends joins as a + b - ab.
    directed.par_sort_by_key(|(i, j, _)| (*i, *j));
    let mut edges: Vec<(usize, usize, f32)> = Vec::with_capacity(directed.len());
    for (i, j, w) in directed {
        match edges.last_mut() {
            Some(last) if (last.0, last.1) == (i, j) => last.2 = last.2 + w - last.2 * w,
            _ => edges.push((i, j, w)),
        }
    }
    edges
}

/// Every star's ties to its sound-alikes, listed from both ends: star `i`'s are `to[from[i]..from[i + 1]]`, how
/// strongly (as a share of the strongest) in `pull`.
struct Ties {
    from: Vec<usize>,
    to: Vec<usize>,
    pull: Vec<f32>,
}

impl Ties {
    fn new(n: usize, edges: &[(usize, usize, f32)]) -> Ties {
        let strongest = edges.iter().fold(1e-6f32, |m, e| m.max(e.2));
        let mut from = vec![0usize; n + 1];
        for &(i, j, _) in edges {
            from[i + 1] += 1;
            from[j + 1] += 1;
        }
        for i in 0..n {
            from[i + 1] += from[i];
        }
        let mut next = from.clone();
        let (mut to, mut pull) = (vec![0usize; from[n]], vec![0f32; from[n]]);
        for &(i, j, w) in edges {
            for (a, b) in [(i, j), (j, i)] {
                to[next[a]] = b;
                pull[next[a]] = w / strongest;
                next[a] += 1;
            }
        }
        Ties { from, to, pull }
    }
}

/// The rounds of pulling together and pushing apart. Each round every star moves itself (on every core at once)
/// against where the others stood at the start of the round, with a random sequence of its own, so the map is the
/// same however many cores do it, and in whatever order.
fn settle(pos: &mut Vec<f64>, ties: &Ties, epochs: usize, seed: u64, stop: &dyn Fn() -> bool) -> Option<()> {
    let n = pos.len() / 2;
    let clip = |v: f64| v.clamp(-4.0, 4.0);
    let mut next = vec![0f64; pos.len()];
    for epoch in 0..epochs {
        if stop() {
            return None;
        }
        let alpha = 1.0 - epoch as f64 / epochs as f64;
        let was: &[f64] = pos;
        next.par_chunks_mut(2).enumerate().for_each(|(i, p)| {
            let mut rng = Rng::of(seed, 3 + epoch as u64, i as u64);
            let (mut x, mut y) = (was[2 * i], was[2 * i + 1]);
            for t in ties.from[i]..ties.from[i + 1] {
                // Stronger ties pull more often, like UMAP's edge sampling.
                if rng.unit() > f64::from(ties.pull[t]) {
                    continue;
                }
                let j = ties.to[t];
                let (dx, dy) = (x - was[2 * j], y - was[2 * j + 1]);
                let d2 = dx * dx + dy * dy;
                if d2 > 0.0 {
                    let pb = d2.powf(B - 1.0);
                    let g = -2.0 * A * B * pb / (1.0 + A * pb * d2);
                    x += clip(g * dx) * alpha;
                    y += clip(g * dy) * alpha;
                }
                for _ in 0..NEGATIVE_SAMPLES {
                    let k = rng.below(n);
                    if k == i {
                        continue;
                    }
                    let (dx, dy) = (x - was[2 * k], y - was[2 * k + 1]);
                    let d2 = dx * dx + dy * dy;
                    let g = 2.0 * B / ((0.001 + d2) * (1.0 + A * d2.powf(B)));
                    x += clip(g * dx) * alpha;
                    y += clip(g * dy) * alpha;
                }
            }
            p[0] = x;
            p[1] = y;
        });
        std::mem::swap(pos, &mut next);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `per` sounds in each of `clusters` families: each family around a corner of its own, a little spread.
    fn families(clusters: usize, per: usize, dim: usize) -> HashMap<i64, Vec<f32>> {
        let mut rng = Rng::new(12345);
        let mut out = HashMap::new();
        for c in 0..clusters {
            for s in 0..per {
                let mut v: Vec<f32> = (0..dim).map(|_| (rng.unit() as f32 - 0.5) * 0.2).collect();
                v[c % dim] += 3.0;
                out.insert((c * per + s + 1) as i64, v);
            }
        }
        out
    }

    fn run(vectors: &HashMap<i64, Vec<f32>>, exact_below: usize) -> HashMap<i64, StarPoint> {
        let ids: Vec<i64> = vectors.keys().copied().collect();
        let get = |id: i64| Cow::Borrowed(vectors[&id].as_slice());
        layout_with(&ids, &get, SEED, exact_below, &|_| {}, &AtomicBool::new(false)).unwrap()
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
    fn families_gather_and_the_same_library_always_gives_the_same_map() {
        let v = families(4, 40, 24);
        let map = run(&v, EXACT_BELOW);
        assert_eq!(map.len(), 160);
        assert!(map.values().all(|p| (0.07..=0.93).contains(&p.x) && (0.07..=0.93).contains(&p.y)));
        let (within, across) = spread(&map, 40);
        assert!(within * 3.0 < across, "{within} {across}");
        assert_eq!(map, run(&v, EXACT_BELOW));
    }

    #[test]
    fn the_map_is_the_same_however_many_cores_work_on_it() {
        let v = families(5, 60, 16);
        let on = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            pool.install(|| run(&v, 0))
        };
        let one = on(1);
        assert_eq!(one, on(4));
        // Laid out as a big library would be (neighbours of neighbours), the families still gather.
        let (within, across) = spread(&one, 60);
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
        let get = |id: i64| Cow::Borrowed(all[&id].as_slice());
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

    /// How long a big library takes (run by hand: `cargo test --release -p mo-sounds big_library -- --ignored`).
    #[test]
    #[ignore = "slow; run by hand to time a big library"]
    fn big_library_lays_out_in_reasonable_time() {
        let v = families(40, 1000, 64);
        let started = std::time::Instant::now();
        let map = run(&v, EXACT_BELOW);
        println!("40,000 sounds in {:?}", started.elapsed());
        let (within, across) = spread(&map, 1000);
        assert!(within * 2.0 < across, "{within} {across}");
    }

    #[test]
    fn cancelling_stops_the_layout() {
        let v = families(2, 20, 6);
        let ids: Vec<i64> = v.keys().copied().collect();
        let get = |id: i64| Cow::Borrowed(v[&id].as_slice());
        assert!(layout(&ids, &get, SEED, &|_| {}, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn the_wide_dot_is_a_dot_product() {
        let a: Vec<f32> = (0..21).map(|i| i as f32 * 0.25).collect();
        let b: Vec<f32> = (0..21).map(|i| 1.0 - i as f32 * 0.1).collect();
        let plain: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        assert!((dot(&a, &b) - plain).abs() < 1e-4);
    }

    #[test]
    fn random_numbers_stay_in_range() {
        let mut r = Rng::of(SEED, 1, 2);
        for _ in 0..10_000 {
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            assert!(r.below(7) < 7);
        }
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
        // The measurements alone are lent, not copied.
        assert!(matches!(fingerprints(&deep, "features").get(3), Cow::Borrowed(_)));
        assert_eq!(&*fingerprints(&deep, "features").get(3), deep.features.get(3).unwrap());
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
