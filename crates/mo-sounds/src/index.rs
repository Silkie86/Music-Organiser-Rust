//! Sounds that sound alike: every sound's vector made unit length, so likeness is one dot product (cosine
//! similarity). A port of the C# app's `VectorIndex` and `SoundOrganiser.Similar`, checked by the sounds twin test.
//!
//! Two indexes: the analyser's features (standardised, so every number counts on the same scale, with the 12 chroma
//! numbers counting 0.35 as much, since which notes are present matters less than timbre for "sounds like"), and CLAP
//! embeddings once deep listening has heard at least 9 in 10 analysed sounds.
//!
//! Building reads every vector once; build it off the window's thread and keep it until the library changes.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use rayon::prelude::*;

use crate::Sound;
use crate::learn::Vectors;

/// Unit-length vectors held in memory for fast likeness search: all in one block, a row per sound, in id order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VectorIndex {
    /// How many numbers each vector has.
    dim: usize,
    /// The ids, in order; row `i` of `rows` is `ids[i]`'s vector.
    ids: Vec<i64>,
    rows: Rows,
}

/// An index's numbers: full precision, or half (16 bits, about 3 significant digits). Held in parts of [`CHUNK`] rows
/// (the last may be shorter), the blocks a search gives each worker: so no part is ever grown or copied whole, and
/// the index never needs one big piece of memory.
#[derive(Clone, Debug, PartialEq)]
enum Rows {
    Full(Vec<Box<[f32]>>),
    /// Each number's half-precision bits. CLAP's 512 numbers a sound were 0.93 GB for half a million sounds at full
    /// precision; at half they're 0.46 GB, and a likeness (a sum of 512 products, rounded to 3 places) moves by 0.001
    /// at most now and then (Solomon chose this, 2026-11-03).
    Half(Vec<Box<[u16]>>),
}

impl Default for Rows {
    fn default() -> Rows {
        Rows::Full(Vec::new())
    }
}

/// Every half-precision number as a float, looked up rather than worked out: 256 KB, made once.
static HALVES: LazyLock<Box<[f32; 1 << 16]>> = LazyLock::new(|| {
    let mut all = Box::new([0f32; 1 << 16]);
    for (b, x) in (0..=u16::MAX).zip(all.iter_mut()) {
        *x = half::f16::from_bits(b).to_f32();
    }
    all
});

/// A half-precision number's bits as a float, from [`HALVES`] (a table exactly as long as there are bit patterns, so
/// there's nothing to check).
fn from_half(halves: &[f32; 1 << 16], bits: u16) -> f32 {
    halves[usize::from(bits)]
}

/// `x` at half precision (rounded to the nearest), as its bits.
fn to_half(x: f32) -> u16 {
    half::f16::from_f32(x).to_bits()
}

/// How many sounds each worker scores before keeping its best: as the C# app's chunks.
const CHUNK: usize = 4096;

/// How many numbers each block of [`Gathered`] holds: 4 MB. Small enough that the spare room in the last one doesn't
/// count, big enough that there are only a few hundred.
#[cfg(not(test))]
const BLOCK: usize = 1 << 20;
/// Small in tests, so even a small test library is spread over many blocks.
#[cfg(test)]
const BLOCK: usize = 100;

/// Vectors gathered one after another into blocks of numbers as the library is read: the whole library's fingerprints
/// held once, with no list of its own for each sound. With half a million sounds, a list each (as [`Vectors`] keeps
/// them) cost about a gigabyte more, and building the index from them copied them all again. The blocks are of a fixed
/// size, so none is ever grown (which would copy it, holding two copies meanwhile, and leave up to half of it spare),
/// and building an index lets go of each one as soon as its numbers have been taken.
///
/// CLAP's fingerprints may come already as CLAP's index holds them ([`held`]: unit length, at half precision), as the
/// app's own sound library keeps them: those go into the index as they are, with no numbers of full precision held on
/// the way (0.93 GB for half a million sounds).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gathered {
    ids: Vec<i64>,
    /// Where each vector is: its block, where it starts in it and how long it is.
    places: Vec<(usize, usize, usize)>,
    blocks: Blocks,
}

/// A [`Gathered`]'s numbers: as they were stored, or as CLAP's index holds them. One kind or the other, never both.
#[derive(Clone, Debug, PartialEq)]
enum Blocks {
    Full(Vec<Vec<f32>>),
    /// Each vector [`held`]: made unit length, each number's half-precision bits.
    Held(Vec<Vec<u16>>),
}

impl Default for Blocks {
    fn default() -> Blocks {
        Blocks::Full(Vec::new())
    }
}

/// A CLAP fingerprint as CLAP's index holds it: made unit length, then each number at half precision. The very numbers
/// [`VectorIndex::from_gathered_halved`] keeps for it (CLAP's index is neither standardised nor weighted), so one kept
/// this way loses nothing the app uses.
pub fn held(v: &[f32]) -> Vec<u16> {
    let mut row = v.to_vec();
    unit(&mut row);
    row.iter().map(|x| to_half(*x)).collect()
}

/// Numbers as [`held`] gives them, as little-endian bytes, 2 each: how the app's own sound library stores them.
pub fn held_bytes(held: &[u16]) -> Vec<u8> {
    held.iter().flat_map(|b| b.to_le_bytes()).collect()
}

/// The numbers [`held_bytes`] stored. `None` when the bytes aren't whole numbers of 2 bytes, or there are none.
pub fn held_from_bytes(bytes: &[u8]) -> Option<Vec<u16>> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return None;
    }
    Some(bytes.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect())
}

/// Held numbers ([`held`]) as floats: what CLAP's index gives for them ([`VectorIndex::get`]).
pub fn unheld(held: &[u16]) -> Vec<f32> {
    let halves = &*HALVES;
    held.iter().map(|b| from_half(halves, *b)).collect()
}

/// The block `len` more numbers go in (a new one when the last is too full), with their place noted in `places`. The
/// id is the caller's to add.
fn room<'a, T>(blocks: &'a mut Vec<Vec<T>>, places: &mut Vec<(usize, usize, usize)>, len: usize) -> &'a mut Vec<T> {
    let full = blocks.last().is_none_or(|b| b.capacity() - b.len() < len);
    if full {
        blocks.push(Vec::with_capacity(BLOCK.max(len)));
    }
    let at = blocks.len() - 1;
    let block = &mut blocks[at];
    places.push((at, block.len(), len));
    block
}

impl Gathered {
    /// Adds `id`'s vector (held as the rest are held: see [`Gathered::push_held`]).
    pub fn push(&mut self, id: i64, v: &[f32]) {
        match &mut self.blocks {
            Blocks::Full(blocks) => room(blocks, &mut self.places, v.len()).extend_from_slice(v),
            Blocks::Held(blocks) => room(blocks, &mut self.places, v.len()).extend(held(v)),
        }
        self.ids.push(id);
    }

    /// Adds `id`'s vector from a stored blob of little-endian floats. Anything but whole floats is left out (as the
    /// C# app leaves it out), and then this says false.
    pub fn push_blob(&mut self, id: i64, blob: &[u8]) -> bool {
        if blob.is_empty() || !blob.len().is_multiple_of(4) {
            return false;
        }
        let floats = blob.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b));
        match &mut self.blocks {
            Blocks::Full(blocks) => room(blocks, &mut self.places, blob.len() / 4).extend(floats),
            Blocks::Held(blocks) => {
                room(blocks, &mut self.places, blob.len() / 4).extend(held(&floats.collect::<Vec<f32>>()));
            }
        }
        self.ids.push(id);
        true
    }

    /// Adds `id`'s vector as CLAP's index holds it ([`held`]), from the bytes [`held_bytes`] stored. A gathering that
    /// starts with these holds every vector so; one that has others already takes these as floats ([`unheld`]).
    /// Anything but whole numbers of 2 bytes is left out, and then this says false.
    pub fn push_held(&mut self, id: i64, bytes: &[u8]) -> bool {
        let Some(numbers) = held_from_bytes(bytes) else { return false };
        if self.ids.is_empty() {
            self.blocks = Blocks::Held(Vec::new());
        }
        match &mut self.blocks {
            Blocks::Held(blocks) => room(blocks, &mut self.places, numbers.len()).extend_from_slice(&numbers),
            Blocks::Full(blocks) => room(blocks, &mut self.places, numbers.len()).extend(unheld(&numbers)),
        }
        self.ids.push(id);
        true
    }

    /// Whether it holds its vectors as CLAP's index holds them ([`held`]).
    pub fn is_held(&self) -> bool {
        matches!(self.blocks, Blocks::Held(_)) && !self.ids.is_empty()
    }

    /// The same vectors, each as CLAP's index holds it ([`held`]): what the app's own sound library keeps of CLAP's.
    pub fn into_held(self) -> Gathered {
        let Blocks::Full(_) = self.blocks else { return self };
        let mut out = Gathered::default();
        for (id, v) in self.iter() {
            if out.ids.is_empty() {
                out.blocks = Blocks::Held(Vec::new());
            }
            out.push(id, &v);
        }
        out
    }

    /// The same vectors at full precision: held ones as floats ([`unheld`]).
    fn into_full(self) -> Gathered {
        let Blocks::Held(_) = self.blocks else { return self };
        let mut out = Gathered::default();
        for (id, v) in self.iter() {
            out.push(id, &v);
        }
        out
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The most common length, and how many vectors have it; on a tie, the longer (newer analysers give more
    /// numbers). (0, 0) with none.
    pub(crate) fn common_length(&self) -> (usize, usize) {
        let mut lengths: HashMap<usize, usize> = HashMap::new();
        for &(_, _, len) in &self.places {
            *lengths.entry(len).or_default() += 1;
        }
        lengths.into_iter().max_by_key(|(len, n)| (*n, *len)).unwrap_or((0, 0))
    }

    /// The vector at `i` (in the order they were added), as floats.
    fn at(&self, i: usize) -> Cow<'_, [f32]> {
        let (b, start, len) = self.places[i];
        match &self.blocks {
            Blocks::Full(blocks) => Cow::Borrowed(&blocks[b][start..start + len]),
            Blocks::Held(blocks) => Cow::Owned(unheld(&blocks[b][start..start + len])),
        }
    }

    /// Each id with its vector, in the order they were added. Held ones come as floats ([`unheld`]).
    pub fn iter(&self) -> impl Iterator<Item = (i64, Cow<'_, [f32]>)> {
        self.ids.iter().enumerate().map(|(i, id)| (*id, self.at(i)))
    }

    /// The same vectors, a list each, by id (a later one for the same id wins).
    pub fn to_vectors(&self) -> Vectors {
        self.iter().map(|(id, v)| (id, v.into_owned())).collect()
    }

    /// `vectors` gathered, in id order.
    pub fn from_vectors(vectors: &Vectors) -> Gathered {
        let mut list: Vec<(i64, &Vec<f32>)> = vectors.iter().map(|(id, v)| (*id, v)).collect();
        list.sort_unstable_by_key(|(id, _)| *id);
        let mut g = Gathered::default();
        for (id, v) in list {
            g.push(id, v);
        }
        g
    }

    /// Lets go of the blocks before `to`, from `from` on: every vector in them has been taken.
    fn let_go(&mut self, from: usize, to: usize) {
        match &mut self.blocks {
            Blocks::Full(blocks) => blocks[from..to].iter_mut().for_each(|b| *b = Vec::new()),
            Blocks::Held(blocks) => blocks[from..to].iter_mut().for_each(|b| *b = Vec::new()),
        }
    }
}

/// The weight of each feature for "sounds like": the last 12 (chroma) count 0.35, the rest 1.
pub fn chroma_downweight(dim: usize) -> Vec<f32> {
    let mut w = vec![1f32; dim];
    for x in &mut w[dim.saturating_sub(12)..] {
        *x = 0.35;
    }
    w
}

/// Makes `v` unit length (left as it is when it's all zeros), as C#'s `VectorIndex.Unit`.
pub fn unit(v: &mut [f32]) {
    let n = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
    if n > 0.0 {
        for x in v {
            *x = (f64::from(*x) / n) as f32;
        }
    }
}

/// The dot product, as C#'s `VectorIndex.Dot`: each product in single precision, added up in double; over the
/// shorter of the two.
pub fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| f64::from(x * y)).sum()
}

impl VectorIndex {
    /// Builds an index of `vectors`. Only vectors of the most common length are kept (a few left from an older
    /// analyser can't be compared). With `standardise`, each number has its average taken off and is divided by its
    /// spread first; `weights` then scales each number.
    pub fn build(vectors: &Vectors, standardise: bool, weights: Option<fn(usize) -> Vec<f32>>) -> VectorIndex {
        VectorIndex::from_gathered(Gathered::from_vectors(vectors), standardise, weights)
    }

    /// As [`VectorIndex::build`], from vectors gathered as the library was read. Their blocks are let go of as the
    /// index takes their numbers, so the library's fingerprints are held about once all the way through.
    pub fn from_gathered(gathered: Gathered, standardise: bool, weights: Option<fn(usize) -> Vec<f32>>) -> VectorIndex {
        VectorIndex::make(gathered, standardise, weights, false)
    }

    /// As [`VectorIndex::from_gathered`], with the numbers held at half precision (see [`VectorIndex::halved`]),
    /// made straight from the gathered ones: the full-precision index is never held whole.
    pub fn from_gathered_halved(
        gathered: Gathered,
        standardise: bool,
        weights: Option<fn(usize) -> Vec<f32>>,
    ) -> VectorIndex {
        VectorIndex::make(gathered, standardise, weights, true)
    }

    fn make(mut g: Gathered, standardise: bool, weights: Option<fn(usize) -> Vec<f32>>, half: bool) -> VectorIndex {
        if g.is_empty() {
            return VectorIndex::default();
        }
        // Vectors held as CLAP's index holds them go in as they are, when that's the index made: else as floats.
        let as_held = !standardise && weights.is_none() && half;
        if !as_held {
            g = g.into_full();
        }
        let (dim, _) = g.common_length();
        if dim == 0 {
            return VectorIndex::default();
        }
        // The vectors of that length, in id order. Read in stored order they already are (the ids are the table's row
        // ids); otherwise sorted, a later one for the same id winning, as a map of them keeps it.
        let mut order: Vec<usize> = (0..g.len()).filter(|&i| g.places[i].2 == dim).collect();
        if !order.is_sorted_by(|&a, &b| g.ids[a] < g.ids[b]) {
            order.sort_by_key(|&i| (g.ids[i], std::cmp::Reverse(i)));
            order.dedup_by_key(|i| g.ids[*i]);
        }
        let in_place_order = order.is_sorted();
        let mut mean = vec![0f64; dim];
        let mut sd = vec![0f64; dim];
        if standardise {
            // As C#: divided by every vector's count, odd lengths included; added up in id order, so the sums come out
            // the same every time.
            let count = g.len() as f64;
            for &i in &order {
                for (m, x) in mean.iter_mut().zip(g.at(i).iter()) {
                    *m += f64::from(*x);
                }
            }
            for m in &mut mean {
                *m /= count;
            }
            for &i in &order {
                for ((s, x), m) in sd.iter_mut().zip(g.at(i).iter()).zip(&mean) {
                    *s += (f64::from(*x) - m) * (f64::from(*x) - m);
                }
            }
            for s in &mut sd {
                *s = (*s / count).sqrt();
            }
        }
        let w = weights.map(|f| f(dim));
        let finish = |row: &mut [f32]| {
            // Neither standardised nor weighted, each number stays as it is: only made unit length, as [`held`] does.
            if standardise || w.is_some() {
                for (d, out) in row.iter_mut().enumerate() {
                    let x = f64::from(*out);
                    let x = if !standardise {
                        x
                    } else if sd[d] > 1e-9 {
                        (x - mean[d]) / sd[d]
                    } else {
                        0.0
                    };
                    *out = (x * w.as_ref().map_or(1.0, |w| f64::from(w[d]))) as f32;
                }
            }
            self::unit(row);
        };
        let ids: Vec<i64> = order.iter().map(|&i| g.ids[i]).collect();
        let parts = order.len().div_ceil(CHUNK);
        let mut full: Vec<Box<[f32]>> = Vec::with_capacity(if half { 0 } else { parts });
        let mut halves: Vec<Box<[u16]>> = Vec::with_capacity(if half { parts } else { 0 });
        // A part at a time: worked out in parallel, kept as a part of the index, and (in stored order) each block let
        // go of once every row in it has been taken.
        let mut taken_to = 0;
        for part in order.chunks(CHUNK) {
            match &g.blocks {
                // As the index holds them already.
                Blocks::Held(blocks) => {
                    let mut rows: Vec<u16> = Vec::with_capacity(part.len() * dim);
                    for &i in part {
                        let (b, start, len) = g.places[i];
                        rows.extend_from_slice(&blocks[b][start..start + len]);
                    }
                    halves.push(rows.into_boxed_slice());
                }
                Blocks::Full(blocks) => {
                    let mut rows: Vec<f32> = Vec::with_capacity(part.len() * dim);
                    for &i in part {
                        let (b, start, len) = g.places[i];
                        rows.extend_from_slice(&blocks[b][start..start + len]);
                    }
                    rows.par_chunks_mut(dim).for_each(finish);
                    if half {
                        halves.push(rows.iter().map(|x| to_half(*x)).collect());
                    } else {
                        full.push(rows.into_boxed_slice());
                    }
                }
            }
            if in_place_order && let Some(&last) = part.last() {
                let block = g.places[last].0;
                g.let_go(taken_to, block);
                taken_to = taken_to.max(block);
            }
        }
        drop(g);
        let rows = if half { Rows::Half(halves) } else { Rows::Full(full) };
        VectorIndex { dim, ids, rows }
    }

    /// The same index with its numbers at half precision (the app makes it with [`VectorIndex::from_gathered_halved`]
    /// instead, never holding the full one whole): half the room, each number to about 3 significant digits.
    #[cfg(test)]
    pub fn halved(self) -> VectorIndex {
        let rows = match self.rows {
            Rows::Full(full) => Rows::Half(full.par_iter().map(|p| p.iter().map(|x| to_half(*x)).collect()).collect()),
            half => half,
        };
        VectorIndex { rows, ..self }
    }

    /// Whether its numbers are held at half precision.
    pub fn is_halved(&self) -> bool {
        matches!(self.rows, Rows::Half(_))
    }

    /// The features index: standardised, chroma counting less.
    pub fn features(vectors: &Vectors) -> VectorIndex {
        VectorIndex::build(vectors, true, Some(chroma_downweight))
    }

    /// The CLAP index: CLAP's vectors as they are, made unit length.
    pub fn clap(vectors: &Vectors) -> VectorIndex {
        VectorIndex::build(vectors, false, None)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// How many numbers each vector has (0 for an empty index).
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The ids it holds, in order.
    pub fn ids(&self) -> &[i64] {
        &self.ids
    }

    pub fn contains(&self, id: i64) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    /// A sound's unit vector, if the index has it: borrowed at full precision, made from the half-precision numbers
    /// otherwise.
    pub fn get(&self, id: i64) -> Option<Cow<'_, [f32]>> {
        let i = self.ids.binary_search(&id).ok()?;
        let (part, at) = (i / CHUNK, (i % CHUNK) * self.dim);
        let at = at..at + self.dim;
        Some(match &self.rows {
            Rows::Full(parts) => Cow::Borrowed(&parts[part][at]),
            Rows::Half(parts) => Cow::Owned(parts[part][at].iter().map(|b| from_half(&HALVES, *b)).collect()),
        })
    }

    /// [`dot`] of a sound's unit vector with `other`, if the index has it: as `dot(&get(id)?, other)`, worked out
    /// where the numbers lie, so a loop over many sounds makes no copy of each at half precision.
    pub fn dot_with(&self, id: i64, other: &[f32]) -> Option<f64> {
        let i = self.ids.binary_search(&id).ok()?;
        let (part, at) = (i / CHUNK, (i % CHUNK) * self.dim);
        let at = at..at + self.dim;
        Some(match &self.rows {
            Rows::Full(parts) => dot(&parts[part][at], other),
            Rows::Half(parts) => {
                let halves = &*HALVES;
                parts[part][at].iter().zip(other).map(|(x, y)| f64::from(from_half(halves, *x) * y)).sum()
            }
        })
    }

    /// The `max` closest sounds to `query`, closest first (ties by id), with their likeness (-1 to 1). Every vector is
    /// compared, in parallel; the query is made unit length first.
    pub fn nearest(&self, query: &[f32], max: usize) -> Vec<(i64, f64)> {
        if max == 0 || self.ids.is_empty() || self.dim == 0 {
            return Vec::new();
        }
        let mut q = query.to_vec();
        unit(&mut q);
        // Each worker scores a block of sounds and keeps only its best `max`, so a search over a big library never
        // holds a score for every sound at once.
        let mut best: Vec<(i64, f32)> = match &self.rows {
            Rows::Full(rows) => self.best_of(rows, max, |v| v.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>()),
            Rows::Half(rows) => {
                let halves = &*HALVES;
                self.best_of(rows, max, |v| v.iter().zip(&q).map(|(a, b)| from_half(halves, *a) * b).sum::<f32>())
            }
        };
        keep_best(&mut best, max);
        best.sort_unstable_by(closest_first);
        best.into_iter().map(|(id, s)| (id, f64::from(s))).collect()
    }

    /// Each part of the rows scored by `score` on a worker of its own, keeping its best `max`, all joined.
    fn best_of<T: Sync>(&self, parts: &[Box<[T]>], max: usize, score: impl Fn(&[T]) -> f32 + Sync) -> Vec<(i64, f32)> {
        parts
            .par_iter()
            .zip(self.ids.par_chunks(CHUNK))
            .flat_map_iter(|(rows, ids)| {
                let mut scored: Vec<(i64, f32)> =
                    rows.chunks_exact(self.dim).zip(ids).map(|(v, id)| (*id, score(v))).collect();
                keep_best(&mut scored, max);
                scored
            })
            .collect()
    }
}

/// Closest first, then by id. A score that isn't a number counts as the least alike (as .NET sorts them), and -0
/// counts the same as 0, so equal scores always fall back to the id.
fn closest_first(a: &(i64, f32), b: &(i64, f32)) -> std::cmp::Ordering {
    let key = |s: f32| if s.is_nan() { f32::NEG_INFINITY } else { s + 0.0 };
    key(b.1).total_cmp(&key(a.1)).then(a.0.cmp(&b.0))
}

/// Cuts `scored` down to its `max` closest, in no particular order.
fn keep_best(scored: &mut Vec<(i64, f32)>, max: usize) {
    if scored.len() > max {
        scored.select_nth_unstable_by(max - 1, closest_first);
        scored.truncate(max);
    }
}

/// Both indexes, built once from the library: what "Sounds like this", Rediscover and kits search in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundIndex {
    pub features: VectorIndex,
    /// `None` until deep listening has heard enough of the sounds for it to be used (see `deep`).
    pub clap: Option<VectorIndex>,
    /// Deep listening has heard at least 9 in 10 analysed sounds, so CLAP is the one to use.
    pub deep: bool,
}

impl SoundIndex {
    /// Builds both from the vectors (as [`crate::SoundsDb::everything`] reads them), as the C# app does when it reads
    /// the library. `analysed` is how many sounds have been analysed, hidden copies too, as the C# app counts them.
    pub fn build(features: &Vectors, clap: &Vectors, analysed: usize) -> SoundIndex {
        SoundIndex::from_gathered(Gathered::from_vectors(features), Gathered::from_vectors(clap), analysed)
    }

    /// As [`SoundIndex::build`], from vectors gathered as the library was read; their blocks become the indexes.
    pub fn from_gathered(features: Gathered, clap: Gathered, analysed: usize) -> SoundIndex {
        let features = VectorIndex::from_gathered(features, true, Some(chroma_downweight));
        // As C#: counts the CLAP vectors the index keeps (those of the most common length).
        let deep = crate::learn::heard_enough(clap.common_length().1, analysed);
        // Until deep listening has heard enough, nothing searches CLAP's vectors (searches, kits and the galaxy all use
        // the features), so they aren't kept: with a big library half heard, that's hundreds of megabytes. Once they
        // are, they're held at half precision: CLAP's is the biggest block the app holds (Solomon, 2026-11-03).
        let clap = (deep && !clap.is_empty()).then(|| VectorIndex::from_gathered_halved(clap, false, None));
        SoundIndex { features, clap, deep }
    }

    /// The index searches use: CLAP when deep listening is on, else the features.
    pub fn active(&self) -> &VectorIndex {
        match &self.clap {
            Some(c) if self.deep => c,
            _ => &self.features,
        }
    }

    /// Sounds that sound like `of`, most alike first, with their likeness to 3 places: CLAP when deep listening is on
    /// and has heard it, else the features. Hidden copies and `of` itself are left out; with `same_type`, so is
    /// any sound of another type. `get` finds a sound by id (one it can't find is left out). As the C# app's
    /// `SoundOrganiser.Similar`.
    pub fn similar<'a>(
        &self,
        of: &Sound,
        get: impl Fn(i64) -> Option<&'a Sound>,
        max: usize,
        same_type: bool,
    ) -> Vec<(i64, f64)> {
        let index = Some(self.active()).filter(|i| i.contains(of.id)).unwrap_or(&self.features);
        let Some(v) = index.get(of.id) else { return Vec::new() };
        let v = &*v;
        // The C# app asks for 3 times as many and stops there; when hidden copies or other types use those up, this
        // asks for more, so the list is only short when the library has no more.
        let mut fetch = max.saturating_mul(3);
        loop {
            let found: Vec<(i64, f64)> = index
                .nearest(v, fetch)
                .into_iter()
                .filter(|(id, _)| *id != of.id)
                .filter_map(|(id, score)| get(id).filter(|s| !s.hidden).map(|s| (s, score)))
                .filter(|(s, _)| !same_type || s.sound_type == of.sound_type)
                .map(|(s, score)| (s.id, crate::analyse::round_to(score, 3)))
                .take(max)
                .collect();
            if found.len() >= max || fetch >= index.len() {
                return found;
            }
            fetch = fetch.saturating_mul(4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sound;

    fn vectors(list: &[(i64, &[f32])]) -> Vectors {
        list.iter().map(|(id, v)| (*id, v.to_vec())).collect()
    }

    #[test]
    fn unit_and_dot() {
        let mut v = [3.0, 4.0];
        unit(&mut v);
        assert_eq!(v, [0.6, 0.8]);
        let mut zero = [0.0, 0.0];
        unit(&mut zero);
        assert_eq!(zero, [0.0, 0.0]);
        assert!((dot(&[1.0, 2.0, 3.0], &[4.0, 5.0]) - 14.0).abs() < 1e-12);
        assert_eq!(chroma_downweight(14), [1.0, 1.0].into_iter().chain([0.35; 12]).collect::<Vec<f32>>());
        assert_eq!(chroma_downweight(5), [0.35; 5]);
    }

    #[test]
    fn nearest_finds_the_closest_first_and_skips_odd_lengths() {
        let v = vectors(&[
            (1, &[1.0, 0.0]),
            (2, &[0.9, 0.1]),
            (3, &[0.0, 1.0]),
            (4, &[-1.0, 0.0]),
            // Another length: left out.
            (5, &[1.0, 0.0, 0.0]),
        ]);
        let i = VectorIndex::clap(&v);
        assert_eq!(i.len(), 4);
        assert!(!i.contains(5));
        let n = i.nearest(&[2.0, 0.0], 3);
        assert_eq!(n.iter().map(|x| x.0).collect::<Vec<_>>(), [1, 2, 3]);
        assert!((n[0].1 - 1.0).abs() < 1e-6);
        assert_eq!(i.nearest(&[1.0, 0.0], 0), []);
        assert_eq!(i.nearest(&[1.0, 0.0], 10).len(), 4);
    }

    #[test]
    fn standardising_makes_every_number_count_alike() {
        // The first number spreads 0..1000, the second 0..1, the third never changes (counts 0).
        let v = vectors(&[(1, &[0.0, 0.0, 7.0]), (2, &[1000.0, 1.0, 7.0]), (3, &[0.0, 1.0, 7.0])]);
        let i = VectorIndex::build(&v, true, None);
        let a = i.get(3).unwrap();
        assert_eq!(a[2], 0.0);
        // Sound 3 is as far from 1 as from 2 once both numbers count alike.
        let d1 = dot(&a, &i.get(1).unwrap());
        let d2 = dot(&a, &i.get(2).unwrap());
        assert!((d1 - d2).abs() < 1e-6, "{d1} {d2}");
    }

    #[test]
    fn similar_leaves_out_itself_hidden_copies_and_other_types() {
        let v = vectors(&[(1, &[1.0, 0.0]), (2, &[0.95, 0.05]), (3, &[0.9, 0.1]), (4, &[0.8, 0.2]), (5, &[0.0, 1.0])]);
        let mut sounds: Vec<Sound> = (1..=5).map(|id| sound(id, &format!("{id}.wav"))).collect();
        sounds[1].hidden = true;
        sounds[2].sound_type = "Kick".into();
        let by_id: HashMap<i64, &Sound> = sounds.iter().map(|s| (s.id, s)).collect();
        let index = SoundIndex { features: VectorIndex::clap(&v), clap: None, deep: false };
        let get = |id| by_id.get(&id).copied();
        let s = index.similar(&sounds[0], get, 2, false);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), [3, 4]);
        let same = index.similar(&sounds[0], get, 5, true);
        assert_eq!(same.iter().map(|x| x.0).collect::<Vec<_>>(), [4, 5]);
        // A sound the index doesn't have: nothing.
        assert!(index.similar(&sound(9, "9.wav"), get, 5, false).is_empty());
    }

    #[test]
    fn similar_asks_for_more_when_hidden_copies_use_up_the_first_ones() {
        // Ten hidden copies right beside sound 1, then two visible sounds further off.
        let mut list: Vec<(i64, Vec<f32>)> = (2..12).map(|id| (id, vec![1.0, 0.001 * id as f32])).collect();
        list.extend([(1, vec![1.0, 0.0]), (20, vec![0.5, 0.5]), (21, vec![0.0, 1.0])]);
        let v: Vectors = list.into_iter().collect();
        let mut sounds: Vec<Sound> = v.keys().map(|id| sound(*id, &format!("{id}.wav"))).collect();
        for s in &mut sounds {
            s.hidden = (2..12).contains(&s.id);
        }
        let by_id: HashMap<i64, &Sound> = sounds.iter().map(|s| (s.id, s)).collect();
        let index = SoundIndex { features: VectorIndex::clap(&v), clap: None, deep: false };
        let one = &by_id[&1];
        let s = index.similar(one, |id| by_id.get(&id).copied(), 2, false);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), [20, 21]);
    }

    #[test]
    fn ties_go_by_id_and_odd_scores_go_last() {
        let mut v = [(3, 0.5), (1, f32::NAN), (2, 0.5), (5, -0.0), (4, 0.0), (6, 0.9)];
        v.sort_by(closest_first);
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), [6, 2, 3, 4, 5, 1]);
    }

    #[test]
    fn nearest_over_many_blocks_matches_scoring_every_sound() {
        // More sounds than one worker's block, so the best of each block are joined.
        let v: Vectors = (0..(CHUNK as i64 * 2 + 37))
            .map(|id| (id, vec![(id % 97) as f32 - 48.0, (id % 13) as f32 - 6.0, 1.0]))
            .collect();
        let i = VectorIndex::clap(&v);
        let q = [1.0, 0.5, 0.2];
        let found = i.nearest(&q, 25);
        let mut q1 = q.to_vec();
        unit(&mut q1);
        let mut all: Vec<(i64, f32)> = (0..i.len() as i64)
            .map(|id| (id, i.get(id).unwrap().iter().zip(&q1).map(|(a, b)| a * b).sum::<f32>()))
            .collect();
        all.sort_by(closest_first);
        assert_eq!(found.iter().map(|x| x.0).collect::<Vec<_>>(), all[..25].iter().map(|x| x.0).collect::<Vec<_>>());
    }

    /// The index as it was built before the vectors were gathered in one block: a list each, copied row by row.
    fn built_from_lists(vectors: &Vectors, standardise: bool, weights: Option<fn(usize) -> Vec<f32>>) -> VectorIndex {
        let mut lengths: HashMap<usize, usize> = HashMap::new();
        for v in vectors.values() {
            *lengths.entry(v.len()).or_default() += 1;
        }
        let dim = lengths.into_iter().max_by_key(|(len, n)| (*n, *len)).map_or(0, |(len, _)| len);
        let mut same: Vec<(i64, &Vec<f32>)> =
            vectors.iter().filter(|(_, v)| v.len() == dim).map(|(id, v)| (*id, v)).collect();
        same.sort_unstable_by_key(|(id, _)| *id);
        let (mut mean, mut sd) = (vec![0f64; dim], vec![0f64; dim]);
        if standardise {
            let count = vectors.len() as f64;
            for (_, v) in &same {
                for (m, x) in mean.iter_mut().zip(v.iter()) {
                    *m += f64::from(*x);
                }
            }
            for m in &mut mean {
                *m /= count;
            }
            for (_, v) in &same {
                for ((s, x), m) in sd.iter_mut().zip(v.iter()).zip(&mean) {
                    *s += (f64::from(*x) - m) * (f64::from(*x) - m);
                }
            }
            for s in &mut sd {
                *s = (*s / count).sqrt();
            }
        }
        let w = weights.map(|f| f(dim));
        let mut rows = vec![0f32; same.len() * dim];
        for (row, (_, v)) in rows.chunks_mut(dim.max(1)).zip(&same) {
            for (d, out) in row.iter_mut().enumerate() {
                let x = f64::from(v[d]);
                let x = if !standardise {
                    x
                } else if sd[d] > 1e-9 {
                    (x - mean[d]) / sd[d]
                } else {
                    0.0
                };
                *out = (x * w.as_ref().map_or(1.0, |w| f64::from(w[d]))) as f32;
            }
            unit(row);
        }
        let parts = rows.chunks(CHUNK * dim.max(1)).map(Box::from).collect();
        VectorIndex { dim, ids: same.iter().map(|(id, _)| *id).collect(), rows: Rows::Full(parts) }
    }

    /// A library's worth of made-up vectors: mostly 16 long, some of an older length, a few ids missing.
    fn made(count: i64) -> Vec<(i64, Vec<f32>)> {
        (1..=count)
            .filter(|id| id % 11 != 0)
            .map(|id| {
                let len = if id % 7 == 0 { 9 } else { 16 };
                (id, (0..len).map(|d| ((id * 31 + d * 17) % 101) as f32 / 7.0 - 5.0).collect())
            })
            .collect()
    }

    #[test]
    fn gathering_in_one_block_builds_the_same_index_to_the_last_bit() {
        let list = made(3000);
        let lists: Vectors = list.iter().cloned().collect();
        // In stored (id) order, as the library is read, and jumbled.
        let mut in_order = Gathered::default();
        for (id, v) in &list {
            in_order.push(*id, v);
        }
        let mut jumbled = Gathered::default();
        for (id, v) in list.iter().rev() {
            jumbled.push(*id, v);
        }
        for (standardise, weights) in [(true, Some(chroma_downweight as fn(usize) -> Vec<f32>)), (false, None)] {
            let want = built_from_lists(&lists, standardise, weights);
            for g in [&in_order, &jumbled] {
                let got = VectorIndex::from_gathered(g.clone(), standardise, weights);
                assert_eq!((got.dim, &got.ids), (want.dim, &want.ids));
                let bits = |r: &Rows| match r {
                    Rows::Full(r) => r.iter().flat_map(|p| p.iter().map(|x| x.to_bits())).collect::<Vec<_>>(),
                    Rows::Half(_) => unreachable!("built at full precision"),
                };
                assert_eq!(bits(&got.rows), bits(&want.rows));
            }
        }
        // Made at half precision straight away: the same numbers as halving the full index.
        let halves = |i: &VectorIndex| match &i.rows {
            Rows::Half(h) => h.clone(),
            Rows::Full(_) => unreachable!("halved"),
        };
        let want = VectorIndex::from_gathered(in_order.clone(), false, None).halved();
        for g in [&in_order, &jumbled] {
            let got = VectorIndex::from_gathered_halved(g.clone(), false, None);
            assert_eq!((got.dim, &got.ids, halves(&got)), (want.dim, &want.ids, halves(&want)));
        }
        assert!(matches!(&in_order.blocks, Blocks::Full(b) if b.len() > 100), "spread over many blocks");
        assert_eq!(in_order.to_vectors(), lists);
        assert_eq!(Gathered::from_vectors(&lists), in_order);
    }

    #[test]
    fn half_precision_finds_the_same_sounds_to_within_a_thousandth() {
        // CLAP-like: 512 numbers a sound, spread around nought.
        let v: Vectors = (1..=2000i64)
            .map(|id| {
                let mut z = id as u64;
                let v = (0..512)
                    .map(|_| {
                        // SplitMix64: a good spread of made-up numbers, the same every run.
                        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
                        let mut x = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                        ((x ^ (x >> 31)) >> 40) as f32 / (1u64 << 23) as f32 - 1.0
                    })
                    .collect();
                (id, v)
            })
            .collect();
        let full = VectorIndex::clap(&v);
        let half = full.clone().halved();
        assert!(half.is_halved() && !full.is_halved());
        assert_eq!(half.dot_with(5000, &[1.0]), None);
        assert_eq!((half.len(), half.dim()), (full.len(), full.dim()));
        for id in [1, 500, 1999] {
            let (a, b) = (full.get(id).unwrap(), half.get(id).unwrap());
            assert!(a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() <= x.abs() / 1024.0 + 1e-7));
            // Worked out where the numbers lie: the same as taking a copy first.
            let q = v[&999].as_slice();
            assert_eq!(full.dot_with(id, q), Some(dot(&a, q)));
            assert_eq!(half.dot_with(id, q), Some(dot(&b, q)));
            let (want, got) = (full.nearest(&a, 8), half.nearest(&b, 8));
            assert_eq!(want[0].0, id);
            assert_eq!(got[0].0, id);
            for ((_, w), (_, g)) in want.iter().zip(&got) {
                assert!((w - g).abs() < 1e-3, "{w} {g}");
            }
        }
    }

    #[test]
    fn a_blob_that_isnt_whole_floats_is_left_out() {
        let mut g = Gathered::default();
        assert!(g.push_blob(1, &[0, 0, 128, 63, 0, 0, 0, 64]));
        assert!(!g.push_blob(2, &[1, 2, 3]));
        assert!(!g.push_blob(3, &[]));
        assert_eq!(g.to_vectors(), Vectors::from([(1, vec![1.0f32, 2.0])]));
        // Held numbers likewise: whole numbers of 2 bytes.
        let mut h = Gathered::default();
        assert!(!h.push_held(1, &[1, 2, 3]) && !h.push_held(2, &[]));
        assert!(h.push_held(3, &held_bytes(&held(&[3.0, 4.0]))));
        assert!(h.is_held());
        // Made unit length, at half precision: 3, 4 is 0.6, 0.8 to about 3 significant digits.
        let back = &h.to_vectors()[&3];
        assert_eq!(back, &unheld(&held(&[3.0, 4.0])));
        assert!((back[0] - 0.6).abs() < 1e-3 && (back[1] - 0.8).abs() < 1e-3, "{back:?}");
    }

    /// CLAP's fingerprints kept as its index holds them ([`held`]) make the very same index as the fingerprints
    /// themselves, to the last bit: in stored order or not, with vectors of other lengths, ones of nothing but
    /// noughts, and numbers far out of the usual range.
    #[test]
    fn held_fingerprints_make_the_same_clap_index_to_the_last_bit() {
        let mut list = made(3000);
        list.push((4000, vec![0.0; 16]));
        list.push((4001, (0..16).map(|d| if d == 3 { -0.0 } else { 1e-30 }).collect()));
        list.push((4002, (0..16).map(|d| d as f32 * 3.0e4).collect()));
        let mut in_order = Gathered::default();
        for (id, v) in &list {
            in_order.push(*id, v);
        }
        let mut jumbled = Gathered::default();
        for (id, v) in list.iter().rev() {
            jumbled.push(*id, v);
        }
        for g in [in_order, jumbled] {
            let want = VectorIndex::from_gathered_halved(g.clone(), false, None);
            // Held as the sound library keeps them, then read back from its bytes.
            let mut stored = Gathered::default();
            for (id, v) in g.iter() {
                assert!(stored.push_held(id, &held_bytes(&held(&v))));
            }
            assert!(stored.is_held());
            assert_eq!(stored, g.clone().into_held());
            let got = VectorIndex::from_gathered_halved(stored.clone(), false, None);
            assert_eq!(got, want);
            assert!(got.is_halved());
            // Asked for an index of another kind, they're taken as the floats they stand for.
            assert_eq!(VectorIndex::from_gathered(stored.clone(), false, None).dim(), want.dim());
            assert_eq!(stored.common_length(), g.common_length());
        }
    }

    #[test]
    fn clap_is_used_once_deep_listening_has_heard_most_sounds() {
        let f = vectors(&[(1, &[1.0, 0.0]), (2, &[0.0, 1.0])]);
        let c = vectors(&[(1, &[1.0, 0.0, 0.0])]);
        let shallow = SoundIndex::build(&f, &c, 2);
        assert!(!shallow.deep);
        // Nothing searches CLAP's vectors until then, so they aren't held.
        assert!(shallow.clap.is_none());
        let i = SoundIndex::build(&f, &c, 1);
        assert!(i.deep);
        assert!(i.clap.as_ref().is_some_and(VectorIndex::is_halved));
        assert_eq!(i.active().len(), 1);
        assert!(!SoundIndex::build(&f, &Vectors::new(), 0).deep);
    }
}
