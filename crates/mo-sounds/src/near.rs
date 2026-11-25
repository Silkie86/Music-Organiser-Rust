//! "Sounds like my favourites": how close each sound is to the favourites of its own type, so the Favourites filter can
//! show the sounds most like them too, a few or many (Solomon, 12 Nov 2026: "show similar to favourites so its not just
//! limited to favourites and make that something you can adjust how adventurous").
//!
//! Each sound is compared with the favourites of its type (a kick with your favourite kicks), in the fingerprints
//! "Sounds like this" uses ([`SoundIndex::active`]): CLAP's when deep listening is on, for each sound and favourite
//! CLAP heard. This app never makes CLAP's fingerprints, so the sounds its own scan added have only the analyser's:
//! those are compared in the analyser's, which every analysed sound has, as "Sounds like this" falls back to them for
//! a sound CLAP never heard (reviews SO-1 and SN-1), and how many were is said by the slider. Each sound goes with the
//! favourite it's most like. Each favourite's sounds
//! are then put in order, the closest first, and each is given its reach ([`reach`]): how many sounds like each
//! favourite must be let through for it to show. So "3 like each" shows the 3 sounds most like every favourite, not
//! 30 piled round one of them in a crowded corner while the others get none.
//!
//! A type with many favourites is compared through at most [`POINTS`] of them, picked to be as unlike each other as
//! they can be (each one is the favourite least like those picked so far): the favourites close to a picked one share
//! its sounds, as many more as there are of them. So a big library is gone through in a moment. Every sound is scored
//! on rayon's threads: call it on the app's low-priority pool, never the window's thread.

use crate::index::{SoundIndex, VectorIndex, dot};

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

/// How close each sound is to the favourites of its type ([`reach`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// For each sound, in the order given: how many sounds like each favourite of its type must be let through for it
    /// to show ([`NEVER`] when it can't be). A favourite's is 0.
    pub each: Vec<u32>,
    /// The sounds (places in the order given) placed by the analyser's fingerprints while deep listening is on: ones
    /// CLAP never heard, or nearer a favourite it never heard. None with deep listening off.
    pub by_analyser: Vec<usize>,
}

/// How close each of `sounds` (its id, its type as shown, and what it is to the favourites) is to the favourites of
/// its type. Types are matched in any case ("kick" is Kick), as the Type filter matches them.
///
/// With deep listening on, a sound and a favourite CLAP both heard are compared in CLAP's fingerprints; a sound CLAP
/// never heard (this app's own scan adds only the analyser's) is compared in the analyser's, which every analysed
/// sound has, with every favourite of its type, and placed among all the type's sounds compared that way; and a sound
/// CLAP heard is also compared in the analyser's with the favourites CLAP never heard, keeping whichever puts it
/// nearer. So a new sound, or a new favourite, never hides the type's others (review SN-1). A sound with no
/// fingerprint at all, every sound of a type with no favourite, and every [`Role::Out`], is [`NEVER`].
pub fn reach(whole: &SoundIndex, sounds: &[(i64, &str, Role)]) -> Reach {
    let mut out = Reach { each: vec![NEVER; sounds.len()], by_analyser: Vec::new() };
    // Each type's places. A library has a few dozen types at most, so a list is quicker than lowering each name.
    let mut by_type: Vec<(&str, Vec<usize>)> = Vec::new();
    for (i, (_, t, _)) in sounds.iter().enumerate() {
        match by_type.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(t)) {
            Some((_, places)) => places.push(i),
            None => by_type.push((t, vec![i])),
        }
    }
    let active = whole.active();
    let deep = !std::ptr::eq(active, &whole.features);
    // Each type with a favourite: its favourites and its others.
    let types: Vec<(Vec<usize>, Vec<usize>)> = by_type
        .into_iter()
        .filter_map(|(_, places)| {
            let favourites: Vec<usize> = places.iter().copied().filter(|&i| sounds[i].2 == Role::Favourite).collect();
            if favourites.is_empty() {
                return None;
            }
            let others: Vec<usize> = places.iter().copied().filter(|&i| sounds[i].2 == Role::Other).collect();
            Some((others, favourites))
        })
        .collect();
    for (_, favourites) in &types {
        for &i in favourites {
            out.each[i] = 0;
        }
    }
    // Every type is ranked in one go through an index ([`rank`]), so numbers kept in a file are read once, not once
    // for each type.
    if !deep {
        for ranked in rank(&whole.features, sounds, &types) {
            for (i, r) in ranked {
                out.each[i] = r;
            }
        }
        return out;
    }
    let heard = |&i: &usize| active.contains(sounds[i].0);
    // Each type's others and favourites CLAP heard, and its favourites CLAP never heard.
    let mut by_clap: Vec<(Vec<usize>, Vec<usize>)> = Vec::with_capacity(types.len());
    let mut unheard: Vec<(bool, Vec<usize>)> = Vec::with_capacity(types.len());
    for (others, favourites) in &types {
        let (heard_favourites, unheard_favourites): (Vec<usize>, Vec<usize>) =
            favourites.iter().copied().partition(heard);
        let (heard_others, unheard_others): (Vec<usize>, Vec<usize>) = others.iter().copied().partition(heard);
        unheard.push((!unheard_others.is_empty() || !unheard_favourites.is_empty(), unheard_favourites));
        by_clap.push((heard_others, heard_favourites));
    }
    for ranked in rank(active, sounds, &by_clap) {
        for (i, r) in ranked {
            out.each[i] = r;
        }
    }
    // The types with a sound or a favourite CLAP never heard, compared in the analyser's.
    let nothing = || (Vec::new(), Vec::new());
    let all: Vec<(Vec<usize>, Vec<usize>)> =
        types.iter().zip(&unheard).map(|(t, (any, _))| if *any { t.clone() } else { nothing() }).collect();
    let against_unheard: Vec<(Vec<usize>, Vec<usize>)> = types
        .iter()
        .zip(&unheard)
        .map(|((others, _), (any, favourites))| if *any { (others.clone(), favourites.clone()) } else { nothing() })
        .collect();
    let all = rank(&whole.features, sounds, &all);
    let against_unheard = rank(&whole.features, sounds, &against_unheard);
    for (all, against_unheard) in all.into_iter().zip(against_unheard) {
        // The ones CLAP never heard: placed among all the type's sounds, every favourite in the analyser's.
        for (i, r) in all {
            if !heard(&i) {
                out.each[i] = r;
                out.by_analyser.push(i);
            }
        }
        // The ones CLAP heard, against the favourites it didn't: nearer that way, they show sooner.
        for (i, r) in against_unheard {
            if heard(&i) && r < out.each[i] {
                out.each[i] = r;
                out.by_analyser.push(i);
            }
        }
    }
    out
}

/// For each of `types` (its others and its favourites, as places in `sounds`): each of the others that `index` has,
/// with its reach to the favourites it has. Each goes with the (picked) favourite it's most like, the closest first in
/// each one's group, and its reach is its place there over the favourites that picked one stands for (so each
/// favourite gets its own share). Every type's sounds are asked of the index together ([`VectorIndex::each`]).
fn rank(
    index: &VectorIndex,
    sounds: &[(i64, &str, Role)],
    types: &[(Vec<usize>, Vec<usize>)],
) -> Vec<Vec<(usize, u32)>> {
    // Every type's favourites' fingerprints, those the index has.
    let ids: Vec<i64> = types.iter().flat_map(|(_, favourites)| favourites.iter().map(|&i| sounds[i].0)).collect();
    let mut found = index.each(&ids, |_, row| row.to_vec()).into_iter();
    let favourites: Vec<Vec<Vec<f32>>> = types
        .iter()
        .map(|(others, favourites)| {
            let have: Vec<Vec<f32>> = found.by_ref().take(favourites.len()).flatten().collect();
            if others.is_empty() { Vec::new() } else { have }
        })
        .collect();
    let points: Vec<Vec<&[f32]>> = favourites.iter().map(|f| spread(f, POINTS)).collect();
    // Each sound of a type with a favourite to compare with, with the picked favourite it's most like, and how like.
    let asked: Vec<(usize, usize)> = types
        .iter()
        .enumerate()
        .filter(|(t, _)| !points[*t].is_empty())
        .flat_map(|(t, (others, _))| others.iter().map(move |&i| (t, i)))
        .collect();
    let ids: Vec<i64> = asked.iter().map(|&(_, i)| sounds[i].0).collect();
    let scored = index.each(&ids, |k, row| closest(&points[asked[k].0], |p| Some(row.dot(p))));
    let mut by_point: Vec<Vec<Vec<(usize, f64)>>> = points.iter().map(|p| vec![Vec::new(); p.len()]).collect();
    for (&(t, i), found) in asked.iter().zip(scored) {
        if let Some((at, like)) = found.flatten() {
            by_point[t][at].push((i, like));
        }
    }
    by_point
        .into_iter()
        .enumerate()
        .map(|(t, mut groups)| {
            // How many favourites each picked one stands for: itself and those closest to it.
            let mut stands_for = vec![0usize; groups.len()];
            for f in &favourites[t] {
                stands_for[closest(&points[t], |p| Some(dot(f, p))).map_or(0, |(at, _)| at)] += 1;
            }
            let mut out = Vec::with_capacity(groups.iter().map(Vec::len).sum());
            for (group, each) in groups.iter_mut().zip(&stands_for) {
                // The closest first; equal ones by id, so the same sounds always come out the same.
                group.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(sounds[a.0].0.cmp(&sounds[b.0].0)));
                for (place, (i, _)) in group.iter().enumerate() {
                    out.push((*i, u32::try_from(place / (*each).max(1)).unwrap_or(NEVER - 1)));
                }
            }
            out
        })
        .collect()
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
        let r = reach(&index, &sounds).each;
        // 2 is the closest to favourite 1, 4 to favourite 5: each the first of its own. 3 is as far from either, so
        // it goes with the first (1), after 2. Favourites 0; snares and the kick without a fingerprint never.
        assert_eq!(r, [0, 0, 1, 0, 0, NEVER, NEVER, NEVER]);
        // A copy of a favourite (the same audio) is never one like it.
        let mut copied = sounds;
        copied[1].2 = Role::Out;
        assert_eq!(reach(&index, &copied).each, [0, NEVER, 0, 0, 0, NEVER, NEVER, NEVER]);
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
        assert_eq!(reach(&index, &sounds), Reach { each: vec![0, 0, 1, 2, 0, 0], by_analyser: vec![] });
    }

    #[test]
    fn a_sound_clap_has_not_heard_is_compared_in_the_analysers_fingerprints() {
        use Role::{Favourite as F, Other as O};
        use std::sync::Arc;
        let list = |l: &[(i64, [f32; 2])]| l.iter().map(|(id, v)| (*id, v.to_vec())).collect();
        // The analyser heard every sound; CLAP only 1 to 4 (the scan's sounds, 5 to 9, it never hears).
        let features = list(&[
            (1, [1.0, 0.0]),
            (2, [0.9, 0.1]),
            (3, [0.0, 1.0]),
            (4, [0.1, 0.9]),
            (5, [1.0, 0.0]),
            (6, [0.95, 0.05]),
            (7, [0.0, 1.0]),
            (8, [0.05, 0.95]),
            (9, [0.0, 1.0]),
        ]);
        // CLAP puts 2 nearest 1 the other way round from the analyser, so which one is used shows.
        let clap = list(&[(1, [1.0, 0.0]), (2, [0.0, 1.0]), (3, [0.9, 0.1]), (4, [0.5, 0.5])]);
        let deep = SoundIndex {
            features: VectorIndex::clap(&features),
            clap: Some(Arc::new(VectorIndex::clap(&clap))),
            deep: true,
        };
        let reach = |s: &[(i64, &str, Role)]| {
            let r = reach(&deep, s);
            (r.each, r.by_analyser.len())
        };
        // Kicks 1 to 4, all heard by CLAP: CLAP's, so 3 is the closest to favourite 1.
        let kicks = [(1, "Kick", F), (2, "Kick", O), (3, "Kick", O), (4, "Kick", O)];
        assert_eq!(reach(&kicks), (vec![0, 2, 0, 1], 0));
        // Snares 5 to 8, none heard by CLAP, the favourite among them: the analyser's, and every one has its reach.
        let snares = [(5, "Snare", F), (6, "Snare", O), (7, "Snare", O), (8, "Snare", O)];
        assert_eq!(reach(&snares), (vec![0, 0, 2, 1], 3));
        // A favourite CLAP heard among sounds it didn't: they're compared with it in the analyser's.
        let mixed = [(1, "Hat", F), (6, "Hat", O), (7, "Hat", O), (8, "Hat", O)];
        assert_eq!(reach(&mixed), (vec![0, 0, 2, 1], 3));
        // CLAP heard the sounds but not the favourite: the analyser's too, so the favourite still counts.
        let unheard = [(5, "Perc", F), (2, "Perc", O), (3, "Perc", O), (4, "Perc", O)];
        assert_eq!(reach(&unheard), (vec![0, 0, 2, 1], 3));
        // Review SN-1: one new sound (6) among kicks CLAP heard keeps CLAP's order for them, and has its own reach,
        // placed among them all in the analyser's: the closest to favourite 1 there.
        let one_new = [(1, "Kick", F), (2, "Kick", O), (3, "Kick", O), (4, "Kick", O), (6, "Kick", O)];
        assert_eq!(reach(&one_new), (vec![0, 2, 0, 1, 0], 1));
        // And a new favourite (9) CLAP never heard leaves CLAP's order for the others to favourite 1, each one heard
        // taking the nearer of that and its place by the new favourite; the new sounds go with whichever is closer.
        let new_favourite = [
            (1, "Kick", F),
            (2, "Kick", O),
            (3, "Kick", O),
            (4, "Kick", O),
            (6, "Kick", O),
            (7, "Kick", O),
            (9, "Kick", F),
        ];
        assert_eq!(reach(&new_favourite), (vec![0, 2, 0, 1, 0, 1, 0], 2));
        // With deep listening off it's all the analyser's, said by no note.
        let off = SoundIndex { deep: false, ..deep.clone() };
        assert!(super::reach(&off, &kicks).by_analyser.is_empty());
    }

    #[test]
    fn nothing_to_compare_with_reaches_nothing() {
        let index = index(&[(1, [1.0, 0.0])]);
        assert_eq!(reach(&index, &[(1, "Kick", Role::Other)]).each, [NEVER]);
        assert!(reach(&index, &[]).each.is_empty());
    }
}
