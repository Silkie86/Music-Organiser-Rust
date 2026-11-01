//! Sounds that sound alike: every sound's vector made unit length, so likeness is one dot product (cosine
//! similarity). A port of the C# app's `VectorIndex` and `SoundOrganiser.Similar`, checked by the sounds twin test.
//!
//! Two indexes: the analyser's features (standardised, so every number counts on the same scale, with the 12 chroma
//! numbers counting 0.35 as much, since which notes are present matters less than timbre for "sounds like"), and CLAP
//! embeddings once deep listening has heard at least 9 in 10 analysed sounds.
//!
//! Building reads every vector once; build it off the window's thread and keep it until the library changes.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::learn::Vectors;
use crate::{Sound, SoundsDb, SoundsError};

/// Unit-length vectors held in memory for fast likeness search: all in one block, a row per sound, in id order.
#[derive(Clone, Debug, Default)]
pub struct VectorIndex {
    /// How many numbers each vector has.
    dim: usize,
    /// The ids, in order; row `i` of `rows` is `ids[i]`'s vector.
    ids: Vec<i64>,
    rows: Vec<f32>,
}

/// How many sounds each worker scores before keeping its best: as the C# app's chunks.
const CHUNK: usize = 4096;

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
        if vectors.is_empty() {
            return VectorIndex::default();
        }
        let mut lengths: HashMap<usize, usize> = HashMap::new();
        for v in vectors.values() {
            *lengths.entry(v.len()).or_default() += 1;
        }
        // The most common length; on a tie, the longer (newer analysers give more numbers).
        let dim = lengths.into_iter().max_by_key(|(len, n)| (*n, *len)).map_or(0, |(len, _)| len);
        // In id order, so the sums below add up the same way every time.
        let mut same: Vec<(i64, &Vec<f32>)> =
            vectors.iter().filter(|(_, v)| v.len() == dim).map(|(id, v)| (*id, v)).collect();
        same.sort_unstable_by_key(|(id, _)| *id);
        let mut mean = vec![0f64; dim];
        let mut sd = vec![0f64; dim];
        if standardise {
            // As C#: divided by every vector's count, odd lengths included.
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
        if dim > 0 {
            rows.par_chunks_mut(dim).zip(&same).for_each(|(row, (_, v))| {
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
                self::unit(row);
            });
        }
        VectorIndex { dim, ids: same.iter().map(|(id, _)| *id).collect(), rows }
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

    pub fn contains(&self, id: i64) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    /// A sound's unit vector, if the index has it.
    pub fn get(&self, id: i64) -> Option<&[f32]> {
        let i = self.ids.binary_search(&id).ok()?;
        Some(&self.rows[i * self.dim..(i + 1) * self.dim])
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
        let mut best: Vec<(i64, f32)> = self
            .rows
            .par_chunks(CHUNK * self.dim)
            .zip(self.ids.par_chunks(CHUNK))
            .flat_map_iter(|(rows, ids)| {
                let mut scored: Vec<(i64, f32)> = rows
                    .chunks_exact(self.dim)
                    .zip(ids)
                    .map(|(v, id)| (*id, v.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>()))
                    .collect();
                keep_best(&mut scored, max);
                scored
            })
            .collect();
        keep_best(&mut best, max);
        best.sort_unstable_by(closest_first);
        best.into_iter().map(|(id, s)| (id, f64::from(s))).collect()
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
#[derive(Clone, Debug, Default)]
pub struct SoundIndex {
    pub features: VectorIndex,
    /// `None` until deep listening has heard any sound.
    pub clap: Option<VectorIndex>,
    /// Deep listening has heard at least 9 in 10 analysed sounds, so CLAP is the one to use.
    pub deep: bool,
}

impl SoundIndex {
    /// Builds both from the vectors, as the C# app does when it reads the library. `analysed` is how many sounds
    /// have been analysed.
    pub fn build(features: &Vectors, clap: &Vectors, analysed: usize) -> SoundIndex {
        let features = VectorIndex::features(features);
        let clap = (!clap.is_empty()).then(|| VectorIndex::clap(clap));
        // As C#: counts the CLAP vectors the index kept (the most common length).
        let deep = clap.as_ref().is_some_and(|c| crate::learn::heard_enough(c.len(), analysed));
        SoundIndex { features, clap, deep }
    }

    /// Reads the vectors from `db` and builds both. Reads every vector: off the window's thread.
    pub fn read(db: &SoundsDb, sounds: &[Sound]) -> Result<SoundIndex, SoundsError> {
        let analysed = sounds.iter().filter(|s| s.analysis.is_some()).count();
        Ok(SoundIndex::build(&db.vectors("features")?, &db.vectors("clap")?, analysed))
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
        let d1 = dot(a, i.get(1).unwrap());
        let d2 = dot(a, i.get(2).unwrap());
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

    #[test]
    fn clap_is_used_once_deep_listening_has_heard_most_sounds() {
        let f = vectors(&[(1, &[1.0, 0.0]), (2, &[0.0, 1.0])]);
        let c = vectors(&[(1, &[1.0, 0.0, 0.0])]);
        assert!(!SoundIndex::build(&f, &c, 2).deep);
        let i = SoundIndex::build(&f, &c, 1);
        assert!(i.deep);
        assert_eq!(i.active().len(), 1);
        assert!(!SoundIndex::build(&f, &Vectors::new(), 0).deep);
    }
}
