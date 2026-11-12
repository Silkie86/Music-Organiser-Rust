//! "Sounds like my favourites": how close each sound is to the favourites of its own type, so the Favourites filter can
//! show the sounds most like them too, a few or many (Solomon, 12 Nov 2026: "show similar to favourites so its not just
//! limited to favourites and make that something you can adjust how adventurous").
//!
//! Each sound is compared with the favourites of its type (a kick with your favourite kicks), in the same fingerprints
//! "Sounds like this" uses ([`SoundIndex::active`]). The sounds of a type are then put in order, the closest first,
//! and each is given its reach ([`reach`]): how many sounds like each favourite must be let through for it to show. With
//! 10 favourite kicks, the 10 kicks closest to them have reach 0, the next 10 reach 1, and so on, so "3 like each"
//! shows the 30 kicks most like them.
//!
//! A type with many favourites is compared with at most [`POINTS`] of them, picked to be as unlike each other as they
//! can be (each one is the favourite least like those picked so far), so a sound close to any favourite is still
//! found close, and a big library is gone through in a moment. Every sound is scored on rayon's threads: call it on the
//! app's low-priority pool, never the window's thread.

use rayon::prelude::*;

use crate::index::{SoundIndex, dot};

/// The most favourites of one type each sound is compared with.
pub const POINTS: usize = 24;

/// A sound's reach when no favourite of its type is known (or it has no fingerprint): it never shows as one like them.
pub const NEVER: u32 = u32::MAX;

/// For each of `sounds` (its id, its type as shown, and whether it's a favourite), in the same order: how many sounds
/// like each favourite of its type must be let through for it to show ([`NEVER`] when it can't be). A favourite's is 0.
/// Types are matched in any case ("kick" is Kick), as the Type filter matches them. A sound the fingerprints don't
/// have, and every sound of a type with no favourite that has one, is [`NEVER`].
pub fn reach(index: &SoundIndex, sounds: &[(i64, &str, bool)]) -> Vec<u32> {
    let index = index.active();
    let mut out = vec![NEVER; sounds.len()];
    // Each type's places. A library has a few dozen types at most, so a list is quicker than lowering each name.
    let mut by_type: Vec<(&str, Vec<usize>)> = Vec::new();
    for (i, (_, t, _)) in sounds.iter().enumerate() {
        match by_type.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(t)) {
            Some((_, places)) => places.push(i),
            None => by_type.push((t, vec![i])),
        }
    }
    for (_, places) in by_type {
        let favourites: Vec<Vec<f32>> = places
            .iter()
            .filter(|&&i| sounds[i].2)
            .filter_map(|&i| index.get(sounds[i].0).map(|v| v.into_owned()))
            .collect();
        if favourites.is_empty() {
            continue;
        }
        let points = spread(&favourites, POINTS);
        let mut scored: Vec<(usize, f64)> = places
            .par_iter()
            .filter(|&&i| !sounds[i].2)
            .filter_map(|&i| {
                let best =
                    points.iter().filter_map(|p| index.dot_with(sounds[i].0, p)).fold(f64::NEG_INFINITY, f64::max);
                best.is_finite().then_some((i, best))
            })
            .collect();
        // The closest first; equal ones by id, so the same sounds always come out the same.
        scored.par_sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(sounds[a.0].0.cmp(&sounds[b.0].0)));
        let each = favourites.len();
        for (rank, (i, _)) in scored.into_iter().enumerate() {
            out[i] = u32::try_from(rank / each).unwrap_or(NEVER - 1);
        }
        for &i in places.iter().filter(|&&i| sounds[i].2) {
            out[i] = 0;
        }
    }
    out
}

/// At most `max` of `vectors` (unit length), as unlike each other as they can be: the first, then each time the one
/// least like those picked so far. All of them when there are no more than `max`.
fn spread(vectors: &[Vec<f32>], max: usize) -> Vec<&[f32]> {
    if vectors.len() <= max {
        return vectors.iter().map(Vec::as_slice).collect();
    }
    let mut picked = vec![0usize];
    // How like each is to the closest one picked.
    let mut like: Vec<f64> = vectors.iter().map(|v| dot(v, &vectors[0])).collect();
    while picked.len() < max {
        let next = (0..vectors.len())
            .filter(|i| !picked.contains(i))
            .min_by(|&a, &b| like[a].total_cmp(&like[b]).then(a.cmp(&b)));
        let Some(next) = next else { break };
        picked.push(next);
        for (l, v) in like.iter_mut().zip(vectors) {
            *l = l.max(dot(v, &vectors[next]));
        }
    }
    picked.into_iter().map(|i| vectors[i].as_slice()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::VectorIndex;

    fn index(list: &[(i64, [f32; 2])]) -> SoundIndex {
        let v = list.iter().map(|(id, v)| (*id, v.to_vec())).collect();
        SoundIndex { features: VectorIndex::clap(&v), clap: None, deep: false }
    }

    #[test]
    fn the_sounds_closest_to_the_favourites_of_their_type_reach_least() {
        // 1 and 5 are favourite kicks, pointing two ways; 2, 3 and 4 kicks, 6 and 7 snares (no favourite snare), 8 a
        // kick with no fingerprint.
        let index = index(&[
            (1, [1.0, 0.0]),
            (2, [0.9, 0.1]),
            (3, [0.5, 0.5]),
            (4, [0.1, 0.9]),
            (5, [0.0, 1.0]),
            (6, [1.0, 0.0]),
            (7, [0.0, 1.0]),
        ]);
        let sounds = [
            (1, "Kick", true),
            (2, "Kick", false),
            (3, "kick", false),
            (4, "KICK", false),
            (5, "Kick", true),
            (6, "Snare", false),
            (7, "Snare", false),
            (8, "Kick", false),
        ];
        let r = reach(&index, &sounds);
        // 2 and 4 are as close to a favourite each (each is the first two), 3 is the furthest: reach 1 with two
        // favourites. Favourites 0; snares and the kick without a fingerprint never.
        assert_eq!(r, [0, 0, 1, 0, 0, NEVER, NEVER, NEVER]);
    }

    #[test]
    fn many_favourites_are_compared_through_the_most_unlike() {
        let v: Vec<Vec<f32>> =
            [[1.0, 0.0], [0.99, 0.141], [0.0, 1.0], [0.141, 0.99], [0.707, 0.707]].iter().map(|v| v.to_vec()).collect();
        let picked = spread(&v, 2);
        assert_eq!(picked, [&v[0][..], &v[2][..]]);
        assert_eq!(spread(&v, 3)[2], &v[4][..]);
        assert_eq!(spread(&v, 9).len(), 5);
    }

    #[test]
    fn nothing_to_compare_with_reaches_nothing() {
        let index = index(&[(1, [1.0, 0.0])]);
        assert_eq!(reach(&index, &[(1, "Kick", false)]), [NEVER]);
        assert!(reach(&index, &[]).is_empty());
    }
}
