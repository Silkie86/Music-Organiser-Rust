//! What the app learns from your library: each sound's type when its name doesn't say, the sounds whose folder name
//! the classifier disagrees with, and tags suggested for sounds like the ones you've tagged. A port of the C# app's
//! `SoundOrganiser.UpdateTypes`, `ResolveType`, `Disagreements` and `TrainTags`.
//!
//! Everything here works out what should change and hands it back; nothing writes to `sounds.db` (that comes once the
//! Sounds page is trusted). Finding a type from CLAP's own words ("model", zero-shot) waits for CLAP in Rust.
//!
//! The vectors are the ones [`crate::SoundsDb::vectors`] reads, in the space [`space`] picks. Heavy steps run on
//! rayon's threads: call these from a low-priority pool, never the window's thread.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;

use crate::analyse::round_to;
use crate::classify::{Classifier, Example, Options, train};
use crate::dotnet_random::DotnetRandom;
use crate::types::UNKNOWN;
use crate::{Sound, TagCount};

/// Each sound's vector by id.
pub type Vectors = HashMap<i64, Vec<f32>>;

/// The space the app learns in: "clap" once deep listening has heard at least 9 in 10 analysed sounds, else
/// "features" (the analyser's own numbers).
pub fn space(clap_vectors: usize, analysed: usize) -> &'static str {
    if clap_vectors > 0 && clap_vectors as f64 >= analysed as f64 * 0.9 { "clap" } else { "features" }
}

/// A sound's vector, if it has one (an empty one counts as none, as in the C# app).
fn vector(vectors: &Vectors, id: i64) -> Option<&[f32]> {
    vectors.get(&id).map(Vec::as_slice).filter(|v| !v.is_empty())
}

/// A sound's type and where it came from, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeChoice {
    pub sound_type: String,
    /// "you", "name", "learned", or "" when nothing said.
    pub source: &'static str,
    /// How sure: 1 for yours and the name's, the classifier's chance (to 2 places) for a learned one.
    pub confidence: f64,
}

/// The type `s` gets: your own label, else its name's, else the classifier's best guess when it's at least 45% sure,
/// else "Unknown".
pub fn resolve_type(s: &Sound, vector: Option<&[f32]>, model: Option<&Classifier>) -> TypeChoice {
    let choice = |t: &str, source, confidence| TypeChoice { sound_type: t.to_owned(), source, confidence };
    if let Some(u) = &s.user_type {
        return choice(u, "you", 1.0);
    }
    if let Some(n) = &s.name_type {
        return choice(n, "name", 1.0);
    }
    if let (Some(v), Some(m)) = (vector, model)
        && let Some((label, p)) = m.top(v)
        && p >= 0.45
    {
        return choice(label, "learned", round_to(p, 2));
    }
    choice(UNKNOWN, "", 0.0)
}

/// What [`update_types`] works out.
#[derive(Debug)]
pub struct TypeUpdate {
    /// The new model, or `None` when there weren't enough labelled sounds (the saved one is then cleared, as in C#).
    pub model: Option<Classifier>,
    /// The sounds whose type, source or confidence changes, in library order.
    pub changes: Vec<(i64, TypeChoice)>,
}

/// Trains the type classifier on your library and works out every sound's type, as the C# app's `UpdateTypes`
/// (before it saves): your own labels count three times as much as names. It needs at least 20 labelled sounds of
/// at least two types.
pub fn update_types(sounds: &[Sound], vectors: &Vectors, space: &str) -> TypeUpdate {
    let examples: Vec<Example> = sounds
        .iter()
        .filter_map(|s| {
            let label = s.user_type.as_ref().or(s.name_type.as_ref())?;
            let x = vector(vectors, s.id)?;
            Some(Example { x: x.to_vec(), label: label.clone(), weight: if s.user_type.is_some() { 3.0 } else { 1.0 } })
        })
        .collect();
    let kinds: HashSet<&str> = examples.iter().map(|e| e.label.as_str()).collect();
    let model =
        if examples.len() >= 20 && kinds.len() >= 2 { train(&examples, space, &Options::default()).ok() } else { None };
    let changes = sounds
        .par_iter()
        .filter_map(|s| {
            let c = resolve_type(s, vector(vectors, s.id), model.as_ref());
            let same = s.sound_type == c.sound_type && s.type_source == c.source && s.type_confidence == c.confidence;
            (!same).then_some((s.id, c))
        })
        .collect();
    TypeUpdate { model, changes }
}

/// Sounds whose name gives a type but the classifier is at least 70% sure it's another: worth a listen and a
/// correction. Most sure first, then by id; at most `max`. Hidden copies and sounds you've labelled are left out.
pub fn disagreements<'a>(
    sounds: &'a [Sound],
    vectors: &Vectors,
    model: &Classifier,
    max: usize,
) -> Vec<(&'a Sound, String, f64)> {
    let mut found: Vec<(&Sound, String, f64)> = sounds
        .par_iter()
        .filter(|s| !s.hidden && s.user_type.is_none())
        .filter_map(|s| {
            let name_type = s.name_type.as_deref()?;
            let (label, p) = model.top(vector(vectors, s.id)?)?;
            (label != name_type && p >= 0.7).then(|| (s, label.to_owned(), round_to(p, 2)))
        })
        .collect();
    found.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.id.cmp(&b.0.id)));
    found.truncate(max);
    found
}

/// One tag's suggestions from [`learn_tags`].
#[derive(Clone, Debug, PartialEq)]
pub struct TagSuggestions {
    pub tag: String,
    /// The sounds the tag is suggested for, with how sure (at least 85%), most sure first; at most 300.
    pub suggested: Vec<(i64, f64)>,
    /// How often the model was right on examples it hadn't seen, when there were enough to tell.
    pub accuracy: Option<f64>,
}

/// For every tag you've given at least five sounds, learns what those sounds have in common and suggests it for other
/// sounds that share it, as the C# app's `TrainTags`. `tags` is [`crate::SoundsDb::tag_counts`]; `yours` gives the
/// ids you tagged with a tag ([`crate::SoundsDb::tagged`]).
///
/// Untagged sounds aren't truly "not" the tag, so they're split in two: train against one half, suggest within the
/// other, then swap. No sound is judged by a model that was told it's a negative.
///
/// Every tag's old suggestions are to be cleared, whether or not it gets new ones; the tags given back are the ones
/// that were trained.
pub fn learn_tags(
    tags: &[TagCount],
    yours: impl Fn(&str) -> Vec<i64>,
    sounds: &[Sound],
    vectors: &Vectors,
    space: &str,
) -> Vec<TagSuggestions> {
    let known: HashSet<i64> = sounds.iter().map(|s| s.id).collect();
    let mut rng = DotnetRandom::new(11);
    let mut results = Vec::new();
    for TagCount { tag, yours: count, .. } in tags {
        if *count < 5 {
            continue;
        }
        let positives: Vec<i64> =
            yours(tag).into_iter().filter(|id| known.contains(id) && vector(vectors, *id).is_some()).collect();
        let pos_ids: HashSet<i64> = positives.iter().copied().collect();
        // C#'s `OrderBy(_ => rng.Next())`: each sound draws a number in library order; ties keep that order.
        let pool: Vec<i64> =
            sounds.iter().map(|s| s.id).filter(|id| !pos_ids.contains(id) && vector(vectors, *id).is_some()).collect();
        let draws: Vec<i32> = pool.iter().map(|_| rng.next()).collect();
        let mut order: Vec<usize> = (0..pool.len()).collect();
        order.sort_by_key(|&i| draws[i]);
        let unlabelled: Vec<i64> = order.into_iter().map(|i| pool[i]).collect();
        if positives.len() < 5 || unlabelled.len() < 10 {
            continue;
        }
        let fold = |fold: usize| -> (Vec<(i64, f64)>, Option<f64>) {
            let neg_pool = unlabelled.iter().enumerate().filter(|(i, _)| i % 2 != fold).map(|(_, id)| *id);
            let to_score: Vec<i64> =
                unlabelled.iter().enumerate().filter(|(i, _)| i % 2 == fold).map(|(_, id)| *id).collect();
            let negatives: Vec<i64> = neg_pool.take(20.max(positives.len() * 4)).collect();
            let example = |id: &i64, label: &str| Example {
                x: vector(vectors, *id).unwrap_or_default().to_vec(),
                label: label.to_owned(),
                weight: 1.0,
            };
            let examples: Vec<Example> = positives
                .iter()
                .map(|id| example(id, "yes"))
                .chain(negatives.iter().map(|id| example(id, "no")))
                .collect();
            // Two kinds and at least 25 examples, so it always trains (unless the vectors differ in length).
            let Ok(model) = train(&examples, space, &Options { epochs: 60, ..Options::default() }) else {
                return (Vec::new(), None);
            };
            let suggested = to_score
                .par_iter()
                .filter_map(|id| {
                    let p = model.predict(vector(vectors, *id)?).into_iter().find(|(l, _)| *l == "yes")?.1;
                    (p >= 0.85).then_some((*id, p))
                })
                .collect();
            (suggested, model.held_out_accuracy)
        };
        let ((first, first_accuracy), (second, second_accuracy)) = rayon::join(|| fold(0), || fold(1));
        let mut suggested: Vec<(i64, f64)> = first.into_iter().chain(second).collect();
        suggested.sort_by(|a, b| b.1.total_cmp(&a.1));
        suggested.truncate(300);
        results.push(TagSuggestions { tag: tag.clone(), suggested, accuracy: first_accuracy.or(second_accuracy) });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(id: i64, user: Option<&str>, name: Option<&str>) -> Sound {
        Sound {
            id,
            path: format!("C:\\Samples\\{id}.wav"),
            error: None,
            analysis: None,
            sound_type: UNKNOWN.into(),
            type_source: String::new(),
            type_confidence: 0.0,
            user_type: user.map(Into::into),
            name_type: name.map(Into::into),
            tags: Vec::new(),
            favourite: false,
            hidden: false,
            rating: 0,
            plays: 0,
            added: chrono::NaiveDateTime::default(),
            activity: Default::default(),
        }
    }

    /// A point near `centre` on the first two numbers, from the id (so every run is the same).
    fn near(centre: (f32, f32), id: i64) -> Vec<f32> {
        let wobble = |k: i64| ((id * 7919 + k * 104_729) % 1000) as f32 / 1000.0 - 0.5;
        vec![centre.0 + wobble(1), centre.1 + wobble(2), wobble(3)]
    }

    #[test]
    fn the_space_follows_deep_listening() {
        assert_eq!(space(0, 0), "features");
        assert_eq!(space(89, 100), "features");
        assert_eq!(space(90, 100), "clap");
    }

    #[test]
    fn yours_beats_the_name_which_beats_the_classifier() {
        let m = Classifier {
            classes: vec!["Kick".into(), "Pad".into(), "Hat".into()],
            mean: vec![0.0],
            scale: vec![1.0],
            w: vec![4.0, -4.0, 0.0],
            b: vec![0.0, 0.0, 0.0],
            ..Classifier::default()
        };
        let v: &[f32] = &[1.0];
        assert_eq!(resolve_type(&sound(1, Some("Pad"), Some("Kick")), Some(v), Some(&m)).source, "you");
        assert_eq!(resolve_type(&sound(1, None, Some("Pad")), Some(v), Some(&m)).sound_type, "Pad");
        let learned = resolve_type(&sound(1, None, None), Some(v), Some(&m));
        assert_eq!((learned.sound_type.as_str(), learned.source), ("Kick", "learned"));
        // e^4 / (e^4 + e^-4 + 1), to 2 places.
        assert_eq!(learned.confidence, 0.98);
        // Not sure enough (a third each), no vector, or no model: Unknown.
        assert_eq!(resolve_type(&sound(1, None, None), Some(&[0.0]), Some(&m)).sound_type, UNKNOWN);
        assert_eq!(resolve_type(&sound(1, None, None), None, Some(&m)).source, "");
        assert_eq!(resolve_type(&sound(1, None, None), Some(v), None).confidence, 0.0);
    }

    fn library() -> (Vec<Sound>, Vectors) {
        let mut sounds = Vec::new();
        let mut vectors = Vectors::new();
        // Enough sounds for several rounds of nudges per pass (training goes 64 at a time), as in a real library.
        for id in 0..300 {
            let (name, centre) = if id % 2 == 0 { ("Kick", (5.0, 0.0)) } else { ("Hat", (0.0, 5.0)) };
            // A third have no name type: the classifier gives them one.
            let named = id % 3 != 0;
            sounds.push(sound(id, None, named.then_some(name)));
            vectors.insert(id, near(centre, id));
        }
        // A Kick-named sound that sounds like a hat, and one you've labelled (left out of disagreements).
        sounds.push(sound(1000, None, Some("Kick")));
        vectors.insert(1000, near((0.0, 5.0), 1000));
        sounds.push(sound(1001, Some("Kick"), Some("Kick")));
        vectors.insert(1001, near((0.0, 5.0), 1001));
        (sounds, vectors)
    }

    #[test]
    fn types_are_learned_and_only_changes_come_back() {
        let (mut sounds, vectors) = library();
        let u = update_types(&sounds, &vectors, "features");
        let model = u.model.expect("enough labelled sounds");
        let by_id: HashMap<i64, &TypeChoice> = u.changes.iter().map(|(id, c)| (*id, c)).collect();
        assert_eq!(by_id[&0].source, "learned");
        assert_eq!(by_id[&0].sound_type, "Kick");
        assert_eq!(by_id[&3].sound_type, "Hat");
        assert_eq!(by_id[&2].source, "name");
        assert_eq!(by_id[&1001].source, "you");
        // Once applied, nothing changes.
        for s in &mut sounds {
            if let Some(c) = by_id.get(&s.id) {
                s.sound_type = c.sound_type.clone();
                s.type_source = c.source.to_owned();
                s.type_confidence = c.confidence;
            }
        }
        let again = update_types(&sounds, &vectors, "features").changes;
        assert!(again.is_empty(), "{again:?}");

        let odd = disagreements(&sounds, &vectors, &model, 10);
        assert_eq!(odd.iter().map(|d| (d.0.id, d.1.as_str())).collect::<Vec<_>>(), [(1000, "Hat")]);
    }

    #[test]
    fn too_few_labels_mean_no_model() {
        let (sounds, vectors) = library();
        let u = update_types(&sounds[..10], &vectors, "features");
        assert!(u.model.is_none());
        assert!(u.changes.iter().all(|(_, c)| c.source != "learned"));
    }

    #[test]
    fn tags_are_suggested_for_sounds_like_the_ones_you_tagged() {
        let mut sounds = Vec::new();
        let mut vectors = Vectors::new();
        for id in 0..300 {
            sounds.push(sound(id, None, None));
            // A fifth sound "dark" (near one point); the rest are spread elsewhere.
            let centre = if id % 5 == 0 { (6.0, 6.0) } else { ((id % 7) as f32 - 3.0, (id % 4) as f32 - 6.0) };
            vectors.insert(id, near(centre, id));
        }
        let tagged: Vec<i64> = (0..150).filter(|id| id % 5 == 0).collect();
        let tags = vec![
            TagCount { tag: "dark".into(), yours: tagged.len() as i64, learned: 0 },
            TagCount { tag: "rare".into(), yours: 2, learned: 0 },
        ];
        let out = learn_tags(&tags, |t| if t == "dark" { tagged.clone() } else { vec![1, 2] }, &sounds, &vectors, "x");
        assert_eq!(out.len(), 1, "a tag on fewer than five sounds isn't learned");
        let dark = &out[0];
        let ids: HashSet<i64> = dark.suggested.iter().map(|s| s.0).collect();
        // Nearly all the untagged dark ones (150..300, every fifth) are suggested, and nothing else: none you tagged,
        // none that isn't dark.
        assert!((150..300).filter(|id| id % 5 == 0 && ids.contains(id)).count() >= 25, "{ids:?}");
        assert!(ids.iter().all(|id| id % 5 == 0 && *id >= 150), "{ids:?}");
        assert!(tagged.iter().all(|id| !ids.contains(id)));
        assert!(dark.suggested.windows(2).all(|w| w[0].1 >= w[1].1));
        assert!(dark.suggested.iter().all(|s| s.1 >= 0.85));
        // The same library gives the same suggestions.
        let again = learn_tags(&tags, |t| if t == "dark" { tagged.clone() } else { vec![] }, &sounds, &vectors, "x");
        assert_eq!(again, out);
    }
}
