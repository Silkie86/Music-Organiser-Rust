//! "Sounds you haven't used much": learns your taste from the sounds your projects use (plus favourites, ratings and
//! what you play), then digs out unused sounds close to that taste, kept varied so the list isn't ten copies of one
//! kick. A port of the C# app's `Rediscover`, checked by the sounds twin test.
//!
//! Scoring every unused sound is the long part on a big library (spread over rayon's threads): call it on the app's
//! low-priority pool (`mo_app::background::on_pool`), never the window's thread.

use std::collections::HashMap;

use chrono::Datelike;
use mo_formats::case;
use rayon::prelude::*;

use crate::dotnet_random::DotnetRandom;
use crate::index::{SoundIndex, VectorIndex, dot, unit};
use crate::usage::Usage;
use crate::{Sound, analyse::round_to, directory};

/// What to dig out.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Only sounds of this type; `None` for any.
    pub sound_type: Option<String>,
    /// How many to suggest.
    pub count: usize,
    /// 0 to 1: how much chance counts against fitting your taste.
    pub adventure: f64,
    /// The same seed gives the same list; 0 gives a new list each time (Shuffle).
    pub seed: i32,
    /// Sounds used in up to this many projects count as unused (0, or 1 for "Include rarely used").
    pub max_uses: u32,
    /// Only sounds whose file is dated that year (when the pack was made, or when you got it).
    pub year: Option<i32>,
}

impl Default for Options {
    fn default() -> Self {
        Options { sound_type: None, count: 30, adventure: 0.3, seed: 0, max_uses: 0, year: None }
    }
}

/// One sound suggested, with why in words and its score (to 3 places).
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub id: i64,
    pub reason: String,
    pub score: f64,
}

/// A sample folder and how much of it your projects use.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderUse {
    pub folder: String,
    pub sounds: usize,
    pub used: usize,
    /// `used / sounds`, to 3 places.
    pub share: f64,
}

/// How many projects use a sound.
pub(crate) fn used_in(usage: &HashMap<i64, Usage>, id: i64) -> u32 {
    usage.get(&id).map_or(0, |u| u.projects)
}

/// The years the sounds' files are dated, newest first, for the page's year filter. Hidden copies are left out.
pub fn years(sounds: &[Sound]) -> Vec<i32> {
    let mut y: Vec<i32> = sounds.iter().filter(|s| !s.hidden).map(|s| s.added.year()).filter(|y| *y > 1980).collect();
    y.sort_unstable_by(|a, b| b.cmp(a));
    y.dedup();
    y
}

/// Suggests unused sounds you might like, best first. `sounds` is the whole library (hidden copies are left out
/// here), `usage` comes from [`crate::usage::usage`], `index` from [`SoundIndex::read`].
pub fn suggest(sounds: &[Sound], usage: &HashMap<i64, Usage>, index: &SoundIndex, o: &Options) -> Vec<Suggestion> {
    let mut rng = DotnetRandom::new(if o.seed == 0 { fresh_seed() } else { o.seed });
    let index = index.active();
    let sounds: Vec<&Sound> = sounds
        .iter()
        .filter(|s| {
            !s.hidden
                && s.analysis.is_some()
                && o.sound_type.as_ref().is_none_or(|t| *t == s.sound_type)
                && index.contains(s.id)
        })
        .collect();

    // Your taste: what you've used, starred, rated or keep playing.
    let taste: Vec<(&Sound, f64)> = sounds
        .iter()
        .filter(|s| used_in(usage, s.id) > 0 || s.favourite || s.rating >= 4 || s.plays >= 3)
        .map(|s| {
            let whole = i64::from(used_in(usage, s.id))
                + if s.favourite { 3 } else { 0 }
                + i64::from((s.rating - 3).max(0)) * 2;
            (*s, whole as f64 + f64::from(s.plays.min(10)) * 0.2)
        })
        .collect();
    // The year narrows what's suggested, not what your taste is learned from.
    let candidates: Vec<&Sound> = sounds
        .iter()
        .filter(|s| used_in(usage, s.id) <= o.max_uses && o.year.is_none_or(|y| s.added.year() == y))
        .copied()
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }

    // Each type's taste on its own, so a hat is compared with hats you like.
    let mut by_type: HashMap<&str, Vec<(&Sound, f64)>> = HashMap::new();
    for t in &taste {
        by_type.entry(t.0.sound_type.as_str()).or_default().push(*t);
    }
    let centroids: HashMap<&str, Vec<f32>> = by_type.into_iter().map(|(k, g)| (k, centroid(index, &g))).collect();
    let overall = (!taste.is_empty()).then(|| centroid(index, &taste));

    let noise: Vec<f64> = candidates.iter().map(|_| rng.next_double()).collect();
    let scored: Vec<(&Sound, f64)> = candidates
        .par_iter()
        .zip(&noise)
        .map(|(c, noise)| {
            let centroid = centroids.get(c.sound_type.as_str()).or(overall.as_ref());
            let v = index.get(c.id).unwrap_or_default();
            let fit = centroid.map_or(0.5, |m| (dot(v, m) + 1.0) / 2.0);
            let fresh = if c.plays == 0 { 1.0 } else { 1.0 / f64::from(1 + c.plays) };
            (*c, (1.0 - o.adventure) * fit + 0.15 * fresh + o.adventure * noise)
        })
        .collect();

    // Pick for variety (maximal marginal relevance). Each sound's likeness to its closest pick so far is kept up to
    // date as picks are made, rather than compared with every pick each round.
    // The best max(count × 20, 200), best first; equal scores keep the library's order, as C#'s stable sort does.
    // Only the kept ones are sorted: a big library has hundreds of thousands of scores.
    let mut pool: Vec<(usize, &Sound, f64)> = scored.into_iter().enumerate().map(|(i, (s, x))| (i, s, x)).collect();
    let order = |a: &(usize, &Sound, f64), b: &(usize, &Sound, f64)| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0));
    let keep = o.count.saturating_mul(20).max(200);
    if pool.len() > keep {
        pool.select_nth_unstable_by(keep - 1, order);
        pool.truncate(keep);
    }
    pool.sort_unstable_by(order);
    let pool: Vec<(&Sound, f64, &[f32])> =
        pool.into_iter().map(|(_, s, score)| (s, score, index.get(s.id).unwrap_or_default())).collect();
    let mut redundancy = vec![0f64; pool.len()];
    let mut taken = vec![false; pool.len()];
    let mut picked: Vec<usize> = Vec::new();
    while picked.len() < o.count && picked.len() < pool.len() {
        let mut best = usize::MAX;
        let mut best_val = f64::MIN;
        for (i, p) in pool.iter().enumerate() {
            if taken[i] {
                continue;
            }
            let val = 0.75 * p.1 - 0.25 * if picked.is_empty() { 0.0 } else { redundancy[i] };
            if val > best_val {
                best_val = val;
                best = i;
            }
        }
        if best == usize::MAX {
            break;
        }
        taken[best] = true;
        picked.push(best);
        let v = pool[best].2;
        for (i, p) in pool.iter().enumerate() {
            if !taken[i] {
                let d = dot(v, p.2);
                redundancy[i] = if picked.len() == 1 { d } else { redundancy[i].max(d) };
            }
        }
    }

    picked
        .into_iter()
        .map(|i| {
            let (s, score, v) = pool[i];
            Suggestion { id: s.id, reason: reason(index, usage, s, v, &taste), score: round_to(score, 3) }
        })
        .collect()
}

/// A seed for a new list each time.
pub(crate) fn fresh_seed() -> i32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
    (nanos % i32::MAX as u32) as i32 | 1
}

/// The weighted middle of `items`, made unit length.
fn centroid(index: &VectorIndex, items: &[(&Sound, f64)]) -> Vec<f32> {
    let mut c: Option<Vec<f32>> = None;
    for (s, w) in items {
        let Some(v) = index.get(s.id) else { continue };
        let c = c.get_or_insert_with(|| vec![0f32; v.len()]);
        for (x, y) in c.iter_mut().zip(v) {
            *x += (f64::from(*y) * w) as f32;
        }
    }
    let mut c = c.unwrap_or_default();
    unit(&mut c);
    c
}

/// Why a sound was suggested: its own use first, then the liked sound it's most like (when it's like one).
fn reason(index: &VectorIndex, usage: &HashMap<i64, Usage>, s: &Sound, v: &[f32], taste: &[(&Sound, f64)]) -> String {
    let mut nearest: Option<(&Sound, f64)> = None;
    for (t, _) in taste {
        // Not itself: a starred sound you haven't used yet would be "like" itself (C# names it).
        if t.sound_type != s.sound_type || t.id == s.id {
            continue;
        }
        let Some(tv) = index.get(t.id) else { continue };
        let sim = dot(v, tv);
        // The first of the closest, as C#'s stable sort keeps it.
        if nearest.is_none_or(|(_, best)| sim > best) {
            nearest = Some((t, sim));
        }
    }
    let used = used_in(usage, s.id);
    let own = if used == 0 {
        "Never used".to_owned()
    } else {
        format!("Used in {used} project{}", if used == 1 { "" } else { "s" })
    };
    if let Some((n, sim)) = nearest
        && sim > 0.5
    {
        let n_used = used_in(usage, n.id);
        let why = if n_used > 0 {
            format!("which is in {n_used} of your projects")
        } else if n.favourite {
            "a favourite".to_owned()
        } else {
            "one you like".to_owned()
        };
        return format!("{own}. Sounds like {}, {why}", n.name());
    }
    format!("{own}, from {}", s.folder())
}

/// Why [`suggest`] found nothing, in words for the page.
pub fn why_nothing(sounds: &[Sound], index: &SoundIndex, o: &Options) -> String {
    let visible: Vec<&Sound> = sounds.iter().filter(|s| !s.hidden).collect();
    if visible.is_empty() {
        return "No sounds yet. Add your sample folders in Settings and let the scan finish.".to_owned();
    }
    let analysed: Vec<&Sound> = visible.into_iter().filter(|s| s.analysis.is_some()).collect();
    if analysed.is_empty() || index.active().is_empty() {
        return "None of your sounds have been listened to yet. Let the scan of your sample folders finish, then \
                shuffle again."
            .to_owned();
    }
    let of_type: Vec<&Sound> = match &o.sound_type {
        None => analysed,
        Some(t) => analysed.into_iter().filter(|s| s.sound_type == *t).collect(),
    };
    let kind = o.sound_type.as_ref().map_or(String::new(), |t| format!("{t} "));
    if of_type.is_empty() {
        return format!(
            "No sounds are sorted as {} yet. Pick another type, or Any type.",
            o.sound_type.as_deref().unwrap_or("")
        );
    }
    // Only sounds the index has can be suggested (deep listening may not have heard these yet).
    let mut of_type: Vec<&Sound> = of_type.into_iter().filter(|s| index.active().contains(s.id)).collect();
    if of_type.is_empty() {
        return format!(
            "None of your {kind}sounds have been listened to yet. Let deep listening or the scan finish, then shuffle \
             again."
        );
    }
    if let Some(year) = o.year {
        of_type.retain(|s| s.added.year() == year);
        if of_type.is_empty() {
            return format!("No {kind}sounds are dated {year}. Pick another year, or Any year.");
        }
    }
    if o.max_uses == 0 {
        format!("Every {kind}sound is used in a project already. Tick \"Include rarely used\" to see ones used once.")
    } else {
        format!("Every {kind}sound is used in more than one project already.")
    }
}

/// Your sample folders by how little of them your projects use, then the biggest first: find forgotten packs.
/// Hidden copies are left out. A folder spelt two ways (by case) counts once, under the first spelling met.
/// As in C#, a project's use of a hidden copy isn't credited to the copy that's shown.
pub fn folders(sounds: &[Sound], usage: &HashMap<i64, Usage>) -> Vec<FolderUse> {
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<FolderUse> = Vec::new();
    for s in sounds.iter().filter(|s| !s.hidden) {
        let dir = directory(&s.path);
        let dir = if cfg!(windows) { dir.replace('/', "\\") } else { dir.to_owned() };
        let i = *at.entry(case::key(&dir)).or_insert_with(|| {
            out.push(FolderUse { folder: dir.clone(), sounds: 0, used: 0, share: 0.0 });
            out.len() - 1
        });
        out[i].sounds += 1;
        if used_in(usage, s.id) > 0 {
            out[i].used += 1;
        }
    }
    for f in &mut out {
        f.share = round_to(f.used as f64 / f.sounds as f64, 3);
    }
    // Stable: equal folders keep the order they were met in, as C#.
    out.sort_by(|a, b| a.share.total_cmp(&b.share).then(b.sounds.cmp(&a.sounds)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SoundAnalysis;
    use crate::learn::Vectors;
    use crate::test_support::sound;

    fn analysed(id: i64, path: &str, kind: &str) -> Sound {
        let mut s = sound(id, path);
        s.analysis = Some(SoundAnalysis::default());
        s.sound_type = kind.to_owned();
        s
    }

    /// Kicks near (1, 0), hats near (0, 1); sound 1 (a kick) is used in 5 projects.
    fn library() -> (Vec<Sound>, HashMap<i64, Usage>, SoundIndex) {
        let mut sounds = Vec::new();
        let mut v = Vectors::new();
        for id in 1..=20 {
            let kick = id <= 10;
            sounds.push(analysed(
                id,
                &format!(r"D:\Packs\{}\{id}.wav", if kick { "Kicks" } else { "Hats" }),
                if kick { "Kick" } else { "Hat" },
            ));
            let a = id as f32 * 0.01;
            v.insert(id, if kick { vec![1.0, a] } else { vec![a, 1.0] });
        }
        let usage = HashMap::from([(1, Usage { projects: 5, ..Usage::default() })]);
        let index = SoundIndex { features: VectorIndex::clap(&v), clap: None, deep: false };
        (sounds, usage, index)
    }

    #[test]
    fn suggests_unused_sounds_like_the_ones_you_use() {
        let (sounds, usage, index) = library();
        let o = Options { sound_type: Some("Kick".into()), count: 3, seed: 5, ..Options::default() };
        let s = suggest(&sounds, &usage, &index, &o);
        assert_eq!(s.len(), 3);
        assert!(s.iter().all(|x| x.id != 1 && x.id <= 10), "{s:?}");
        assert!(
            s[0].reason.starts_with("Never used. Sounds like 1, which is in 5 of your projects"),
            "{}",
            s[0].reason
        );
        // The same seed gives the same list.
        assert_eq!(s, suggest(&sounds, &usage, &index, &o));
    }

    #[test]
    fn with_no_taste_the_reason_names_the_folder() {
        let (sounds, _, index) = library();
        let o = Options { sound_type: Some("Hat".into()), count: 2, seed: 9, ..Options::default() };
        let s = suggest(&sounds, &HashMap::new(), &index, &o);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].reason, "Never used, from Hats");
    }

    #[test]
    fn a_starred_unused_sound_is_not_like_itself() {
        let (mut sounds, _, index) = library();
        // Sound 12, a hat, starred but never used: the only hat you like.
        sounds[11].favourite = true;
        let o = Options { sound_type: Some("Hat".into()), count: 10, seed: 4, ..Options::default() };
        let s = suggest(&sounds, &HashMap::new(), &index, &o);
        let twelve = s.iter().find(|x| x.id == 12).unwrap();
        assert_eq!(twelve.reason, "Never used, from Hats");
        assert!(s.iter().any(|x| x.reason == "Never used. Sounds like 12, a favourite"), "{s:?}");
    }

    #[test]
    fn why_nothing_says_what_is_missing() {
        let (mut sounds, usage, index) = library();
        let o = Options { sound_type: Some("Snare".into()), ..Options::default() };
        assert!(suggest(&sounds, &usage, &index, &o).is_empty());
        assert_eq!(
            why_nothing(&sounds, &index, &o),
            "No sounds are sorted as Snare yet. Pick another type, or Any type."
        );
        let o = Options { year: Some(1999), ..Options::default() };
        assert_eq!(why_nothing(&sounds, &index, &o), "No sounds are dated 1999. Pick another year, or Any year.");
        // Pads the index hasn't heard yet.
        sounds.push(analysed(40, r"D:\Packs\Pads\40.wav", "Pad"));
        let o = Options { sound_type: Some("Pad".into()), ..Options::default() };
        assert!(suggest(&sounds, &usage, &index, &o).is_empty());
        assert!(why_nothing(&sounds, &index, &o).starts_with("None of your Pad sounds have been listened to yet"));
        for s in &mut sounds {
            s.hidden = true;
        }
        assert!(why_nothing(&sounds, &index, &Options::default()).starts_with("No sounds yet"));
        assert!(why_nothing(&[], &SoundIndex::default(), &Options::default()).starts_with("No sounds yet"));
    }

    #[test]
    fn folders_least_used_first() {
        let (mut sounds, usage, _) = library();
        sounds.push(analysed(30, r"d:\packs\kicks\30.wav", "Kick"));
        sounds.push(analysed(31, r"E:\x.wav", "Kick"));
        let f = folders(&sounds, &usage);
        assert_eq!(
            f.iter().map(|f| f.folder.as_str()).collect::<Vec<_>>(),
            [r"D:\Packs\Hats", r"E:\", r"D:\Packs\Kicks"]
        );
        assert_eq!((f[2].sounds, f[2].used, f[2].share), (11, 1, 0.091));
    }

    #[test]
    fn years_newest_first() {
        let (mut sounds, ..) = library();
        sounds[0].added = chrono::NaiveDate::from_ymd_opt(2019, 5, 1).unwrap().into();
        sounds[1].added = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap().into();
        assert_eq!(years(&sounds), [2019]);
    }
}
