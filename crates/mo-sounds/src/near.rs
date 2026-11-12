//! "Sounds like my favourites": how close each sound is to the favourites of its own type, so the Favourites filter can
//! show the sounds most like them too, a few or many (Solomon, 12 Nov 2026: "show similar to favourites so its not just
//! limited to favourites and make that something you can adjust how adventurous").
//!
//! Each sound is compared with the favourites of its type (a kick with your favourite kicks), in the same fingerprints
//! "Sounds like this" uses ([`SoundIndex::active`]), and goes with the favourite it's most like. Each favourite's sounds
//! are then put in order, the closest first, and each is given its reach ([`reach`]): how many sounds like each
//! favourite must be let through for it to show. So "3 like each" shows the 3 sounds most like every favourite, not
//! 30 piled round one of them in a crowded corner while the others get none.
//!
//! A type with many favourites is compared through at most [`POINTS`] of them, picked to be as unlike each other as
//! they can be (each one is the favourite least like those picked so far): the favourites close to a picked one share
//! its sounds, as many more as there are of them. So a big library is gone through in a moment. Every sound is scored
//! on rayon's threads: call it on the app's low-priority pool, never the window's thread.

use rayon::prelude::*;

use crate::index::{SoundIndex, dot};

/// The most favourites of one type each sound is compared with.
pub const POINTS: usize = 24;

/// A sound's reach when no favourite of its type is known (or it has no fingerprint): it never shows as one like them.
pub const NEVER: u32 = u32::MAX;

/// What a sound is to the favourites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Favourite,
    /// One that may be like them.
    Other,
    /// Never one like them: the very same audio as a favourite (a copy in another pack is no new sound to try).
    Out,
}

/// For each of `sounds` (its id, its type as shown, and what it is to the favourites), in the same order: how many
/// sounds like each favourite of its type must be let through for it to show ([`NEVER`] when it can't be). A
/// favourite's is 0. Types are matched in any case ("kick" is Kick), as the Type filter matches them. A sound the
/// fingerprints don't have, every sound of a type with no favourite that has one, and every [`Role::Out`], is
/// [`NEVER`].
pub fn reach(index: &SoundIndex, sounds: &[(i64, &str, Role)]) -> Vec<u32> {
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
            .filter(|&&i| sounds[i].2 == Role::Favourite)
            .filter_map(|&i| index.get(sounds[i].0).map(|v| v.into_owned()))
            .collect();
        if favourites.is_empty() {
            continue;
        }
        let points = spread(&favourites, POINTS);
        // How many favourites each picked one stands for: itself and those closest to it.
        let mut stands_for = vec![0usize; points.len()];
        for f in &favourites {
            stands_for[closest(&points, |p| Some(dot(f, p))).map_or(0, |(at, _)| at)] += 1;
        }
        // Each sound with the picked favourite it's most like, and how like.
        let scored: Vec<(usize, usize, f64)> = places
            .par_iter()
            .filter(|&&i| sounds[i].2 == Role::Other)
            .filter_map(|&i| closest(&points, |p| index.dot_with(sounds[i].0, p)).map(|(at, like)| (i, at, like)))
            .collect();
        let mut by_point: Vec<Vec<(usize, f64)>> = vec![Vec::new(); points.len()];
        for (i, at, like) in scored {
            by_point[at].push((i, like));
        }
        for (group, each) in by_point.iter_mut().zip(&stands_for) {
            // The closest first; equal ones by id, so the same sounds always come out the same.
            group.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(sounds[a.0].0.cmp(&sounds[b.0].0)));
            for (rank, (i, _)) in group.iter().enumerate() {
                out[*i] = u32::try_from(rank / (*each).max(1)).unwrap_or(NEVER - 1);
            }
        }
        for &i in places.iter().filter(|&&i| sounds[i].2 == Role::Favourite) {
            out[i] = 0;
        }
    }
    out
}

/// The one of `points` that `like` finds most alike (its place and how alike); none when `like` knows none of them.
fn closest(points: &[&[f32]], like: impl Fn(&[f32]) -> Option<f64>) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for (at, p) in points.iter().enumerate() {
        if let Some(l) = like(p)
            && best.is_none_or(|(_, b)| l > b)
        {
            best = Some((at, l));
        }
    }
    best
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
        use Role::{Favourite as F, Other as O};
        let sounds = [
            (1, "Kick", F),
            (2, "Kick", O),
            (3, "kick", O),
            (4, "KICK", O),
            (5, "Kick", F),
            (6, "Snare", O),
            (7, "Snare", O),
            (8, "Kick", O),
        ];
        let r = reach(&index, &sounds);
        // 2 is the closest to favourite 1, 4 to favourite 5: each the first of its own. 3 is as far from either, so
        // it goes with the first (1), after 2. Favourites 0; snares and the kick without a fingerprint never.
        assert_eq!(r, [0, 0, 1, 0, 0, NEVER, NEVER, NEVER]);
        // A copy of a favourite (the same audio) is never one like it.
        let mut copied = sounds;
        copied[1].2 = Role::Out;
        assert_eq!(reach(&index, &copied), [0, NEVER, 0, 0, 0, NEVER, NEVER, NEVER]);
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
    fn each_favourite_gets_its_own_share_not_the_crowded_one_all() {
        // Favourite 1 sits in a crowd (2, 3, 4 all very close), favourite 5 alone with one sound (6) further off.
        let index = index(&[
            (1, [1.0, 0.0]),
            (2, [0.99, 0.01]),
            (3, [0.98, 0.02]),
            (4, [0.97, 0.03]),
            (5, [0.0, 1.0]),
            (6, [0.3, 0.7]),
        ]);
        use Role::{Favourite as F, Other as O};
        let sounds = [(1, "Kick", F), (2, "Kick", O), (3, "Kick", O), (4, "Kick", O), (5, "Kick", F), (6, "Kick", O)];
        // "1 like each": 2 for the first, 6 for the second, though 3 and 4 are closer to a favourite than 6 is.
        assert_eq!(reach(&index, &sounds), [0, 0, 1, 2, 0, 0]);
    }

    #[test]
    fn nothing_to_compare_with_reaches_nothing() {
        let index = index(&[(1, [1.0, 0.0])]);
        assert_eq!(reach(&index, &[(1, "Kick", Role::Other)]), [NEVER]);
        assert!(reach(&index, &[]).is_empty());
    }
}
