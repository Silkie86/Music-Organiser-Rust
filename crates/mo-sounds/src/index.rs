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

/// Unit-length vectors held in memory for fast likeness search.
#[derive(Clone, Debug, Default)]
pub struct VectorIndex {
    unit: HashMap<i64, Vec<f32>>,
    /// The ids in a fixed order, for searching in parallel.
    ids: Vec<i64>,
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
        if vectors.is_empty() {
            return VectorIndex::default();
        }
        let mut lengths: HashMap<usize, usize> = HashMap::new();
        for v in vectors.values() {
            *lengths.entry(v.len()).or_default() += 1;
        }
        // The most common length; on a tie, the longer (newer analysers give more numbers).
        let dim = lengths.into_iter().max_by_key(|(len, n)| (*n, *len)).map_or(0, |(len, _)| len);
        let same: Vec<(&i64, &Vec<f32>)> = vectors.iter().filter(|(_, v)| v.len() == dim).collect();
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
        let unit: HashMap<i64, Vec<f32>> = same
            .par_iter()
            .map(|(id, v)| {
                let mut u: Vec<f32> = (0..dim)
                    .map(|d| {
                        let x = f64::from(v[d]);
                        let x = if !standardise {
                            x
                        } else if sd[d] > 1e-9 {
                            (x - mean[d]) / sd[d]
                        } else {
                            0.0
                        };
                        (x * w.as_ref().map_or(1.0, |w| f64::from(w[d]))) as f32
                    })
                    .collect();
                self::unit(&mut u);
                (**id, u)
            })
            .collect();
        let mut ids: Vec<i64> = unit.keys().copied().collect();
        ids.sort_unstable();
        VectorIndex { unit, ids }
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
        self.unit.len()
    }

    pub fn is_empty(&self) -> bool {
        self.unit.is_empty()
    }

    pub fn contains(&self, id: i64) -> bool {
        self.unit.contains_key(&id)
    }

    /// A sound's unit vector, if the index has it.
    pub fn get(&self, id: i64) -> Option<&[f32]> {
        self.unit.get(&id).map(Vec::as_slice)
    }

    /// The `max` closest sounds to `query`, closest first (ties by id), with their likeness (-1 to 1). Every vector is
    /// compared, in parallel; the query is made unit length first.
    pub fn nearest(&self, query: &[f32], max: usize) -> Vec<(i64, f64)> {
        if max == 0 || self.unit.is_empty() {
            return Vec::new();
        }
        let mut q = query.to_vec();
        unit(&mut q);
        let mut scored: Vec<(i64, f32)> = self
            .ids
            .par_iter()
            .map(|id| {
                let v = &self.unit[id];
                (*id, v.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>())
            })
            .collect();
        let order = |a: &(i64, f32), b: &(i64, f32)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
        if scored.len() > max {
            scored.select_nth_unstable_by(max - 1, order);
            scored.truncate(max);
        }
        scored.sort_unstable_by(order);
        scored.into_iter().map(|(id, s)| (id, f64::from(s))).collect()
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
        let deep = clap.as_ref().is_some_and(|c| !c.is_empty() && c.len() as f64 >= analysed as f64 * 0.9);
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
        index
            .nearest(v, max * 3)
            .into_iter()
            .filter(|(id, _)| *id != of.id)
            .filter_map(|(id, score)| get(id).filter(|s| !s.hidden).map(|s| (s, score)))
            .filter(|(s, _)| !same_type || s.sound_type == of.sound_type)
            .map(|(s, score)| (s.id, crate::analyse::round_to(score, 3)))
            .take(max)
            .collect()
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
        assert!(same.iter().all(|x| x.0 != 3 && x.0 != 2 && x.0 != 1));
        // A sound the index doesn't have: nothing.
        assert!(index.similar(&sound(9, "9.wav"), get, 5, false).is_empty());
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
