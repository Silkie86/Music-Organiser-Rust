//! Sounds that are copies of each other, grouped, with the one to keep first: what the Space page's Duplicates list
//! shows. A port of the C# app's `DuplicateFinder.Groups`, checked by the sounds twin test.
//!
//! The copies themselves are found by the C# app's duplicate finder (which keeps a print of each sound in sounds.db);
//! this reads what it found. Nothing here moves, deletes or hides anything: hiding is the app's own list, and the
//! files always stay where they are.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mo_formats::case;

use crate::rediscover::used_in;
use crate::usage::Usage;
use crate::{Sound, SoundMatch, SoundsDb, SoundsError};

/// How a sound is a copy of another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CopyKind {
    /// The very same audio, whatever the file is called.
    Exact,
    /// The same sound in another file: a different format, sample rate, bit depth or level.
    Converted,
    /// One is cut from the other: shorter at the start or the end, or a part of a longer sound.
    Trimmed,
}

impl CopyKind {
    fn of(kind: &str) -> CopyKind {
        match kind {
            "Exact" => CopyKind::Exact,
            "Trimmed" => CopyKind::Trimmed,
            _ => CopyKind::Converted,
        }
    }
}

/// One sound in a group of copies.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateMember {
    pub id: i64,
    /// The file's size in bytes.
    pub size: i64,
    /// The one suggested to keep (the first in its group).
    pub keep: bool,
    /// How it's a copy (`None` for the one to keep).
    pub kind: Option<CopyKind>,
    /// Why, in words: "Exact copy, renamed", "Best to keep: full quality, not compressed".
    pub why: String,
    /// The same sound as the one it's a copy of, not just very like it: only these are offered for hiding.
    pub same: bool,
    /// Which sounds in the group are the same sound as each other (same number, same sound); in each such set one
    /// stays showing and the rest can be hidden.
    pub same_set: i64,
}

/// Sounds that are copies of each other, the one to keep first.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateGroup {
    pub members: Vec<DuplicateMember>,
    /// The furthest kind of copy in the group.
    pub kind: CopyKind,
    /// The space the copies that are the same sound take (very similar and trimmed ones may differ on purpose, so
    /// they don't count).
    pub spare_bytes: i64,
}

impl DuplicateGroup {
    /// How many of the copies are the same sound as the one to keep.
    pub fn same_copies(&self) -> usize {
        self.members.iter().filter(|m| !m.keep && m.same).count()
    }

    /// Whether some copies are only very similar (or trimmed), not the same sound.
    pub fn has_similar(&self) -> bool {
        self.members.iter().any(|m| !m.keep && !m.same)
    }
}

/// Converted copies are the same sound when their whole sounds are at most this far apart, in dB; an MP3 or similar
/// shifts the sound a little, so it gets the second. From Solomon's library (2026-10-16): real copies pile up below
/// 0.1.
const SAME_DB: f64 = 0.1;
const SAME_DB_LOSSY: f64 = 1.2;
/// A match whose sounds are typically further apart than this, in dB, isn't a copy at all (matched on a rhythm or an
/// onset only).
const NOT_A_COPY_DB: f64 = 3.0;
/// The duplicate finder's exact hash covers this many seconds of audio.
const READ_SECONDS: f64 = 20.0;

const LOSSY: [&str; 6] = ["mp3", "m4a", "aac", "ogg", "opus", "wma"];
const LOSSLESS: [&str; 5] = ["wav", "aif", "aiff", "aifc", "flac"];

/// The extension without its dot, as .NET's `GetExtension` finds it (after the last dot of the file name).
fn extension(path: &str) -> &str {
    let name = crate::file_name(path);
    name.rfind('.').map_or("", |i| &name[i + 1..])
}

fn is_lossy(path: Option<&str>) -> bool {
    path.is_some_and(|p| LOSSY.iter().any(|e| e.eq_ignore_ascii_case(extension(p))))
}

fn is_lossless(path: &str) -> bool {
    LOSSLESS.iter().any(|e| e.eq_ignore_ascii_case(extension(path)))
}

/// Whether a match is a real copy: one whose sounds are far apart all through only shares a rhythm, or an onset.
fn is_copy(m: &SoundMatch) -> bool {
    m.typical.is_none_or(|t| t <= NOT_A_COPY_DB)
}

/// A converted copy close enough to be the same sound.
fn is_same(m: &SoundMatch, strict: bool) -> bool {
    m.kind == "Converted" && m.apart.is_some_and(|a| a <= if strict { SAME_DB } else { SAME_DB_LOSSY })
}

/// Sets that join, keeping the order things were first met in (the C# dictionary's order, which decides ties).
#[derive(Default)]
struct Sets {
    parent: HashMap<i64, i64>,
    order: Vec<i64>,
}

impl Sets {
    fn find(&mut self, x: i64) -> i64 {
        let Some(&p) = self.parent.get(&x) else {
            self.parent.insert(x, x);
            self.order.push(x);
            return x;
        };
        if p == x {
            return x;
        }
        let root = self.find(p);
        self.parent.insert(x, root);
        root
    }

    fn join(&mut self, a: i64, b: i64) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent.insert(ra, rb);
        }
    }
}

/// .NET's `Math.Round(x, digits)` (halves to even).
fn round(x: f64, digits: i32) -> f64 {
    crate::analyse::round_to(x, digits)
}

/// A number as .NET's "0.##" or "0.#" format writes it: up to `places` decimals, halves away from zero, no trailing
/// zeros.
fn short(x: f64, places: i32) -> String {
    let p = 10f64.powi(places);
    let r = (x * p).round() / p;
    let s = format!("{r:.*}", places as usize);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    s.to_owned()
}

/// Reads the copies the C# app's duplicate finder found from `db` and groups them, biggest saving first. `sounds` is
/// the whole library (hidden copies included: they're listed, so they can be shown again), `usage` from
/// [`crate::usage::usage`].
pub fn read(db: &SoundsDb, sounds: &[Sound], usage: &HashMap<i64, Usage>) -> Result<Vec<DuplicateGroup>, SoundsError> {
    Ok(groups(sounds, usage, &db.matches()?, &db.exact_copies()?, &db.sizes()?))
}

/// Groups the copies found, biggest saving first: `matches` and `exact` (sounds sharing an audio hash) as the
/// duplicate finder kept them, `sizes` each file's size.
pub fn groups(
    sounds: &[Sound],
    usage: &HashMap<i64, Usage>,
    matches: &[SoundMatch],
    exact: &[(i64, String)],
    sizes: &HashMap<i64, i64>,
) -> Vec<DuplicateGroup> {
    let by_id: HashMap<i64, &Sound> = sounds.iter().map(|s| (s.id, s)).collect();
    let size = |id: i64| sizes.get(&id).copied().unwrap_or(0);
    let mut copies = Sets::default();
    // Sounds that are the same sound as each other: exact copies, and converted copies whose whole sounds match.
    let mut same = Sets::default();

    // The exact hash covers the first 20 seconds of audio, so two long files are exact copies only if their lengths
    // (and, past that, their sizes) match too.
    let exact_key = |id: i64, hash: &str| {
        let secs = by_id.get(&id).map_or(0.0, |s| s.seconds());
        if secs > READ_SECONDS - 0.5 { format!("{hash}:{secs:.3}:{}", size(id)) } else { format!("{hash}:{secs:.3}") }
    };
    let hash_of: HashMap<i64, String> = exact.iter().map(|(id, h)| (*id, exact_key(*id, h))).collect();
    let mut by_hash: Vec<(&str, Vec<i64>)> = Vec::new();
    let mut at: HashMap<&str, usize> = HashMap::new();
    for (id, _) in exact {
        let key = hash_of[id].as_str();
        let i = *at.entry(key).or_insert_with(|| {
            by_hash.push((key, Vec::new()));
            by_hash.len() - 1
        });
        by_hash[i].1.push(*id);
    }
    for (_, ids) in &by_hash {
        for e in &ids[1..] {
            copies.join(ids[0], *e);
            same.join(ids[0], *e);
        }
    }

    let mut links: HashMap<i64, Vec<(i64, CopyKind)>> = HashMap::new();
    // An MP3 is let off a little more (its encoder shifts the sound), but only as a leaf: two different files that
    // are each close to the same MP3 aren't the same as each other (round robins, say).
    let mut lossy_links: Vec<(i64, i64, f64)> = Vec::new();
    for m in matches.iter().filter(|m| is_copy(m)) {
        copies.join(m.a, m.b);
        let (pa, pb) = (by_id.get(&m.a).map(|s| s.path.as_str()), by_id.get(&m.b).map(|s| s.path.as_str()));
        if is_same(m, true) {
            same.join(m.a, m.b);
        } else if is_same(m, false) && (is_lossy(pa) || is_lossy(pb)) {
            let apart = m.apart.unwrap_or(0.0);
            if is_lossy(pa) && !is_lossy(pb) {
                lossy_links.push((m.a, m.b, apart));
            } else if is_lossy(pb) && !is_lossy(pa) {
                lossy_links.push((m.b, m.a, apart));
            } else {
                // Both compressed: the lossy limit is the only fair one, and neither is an original.
                same.join(m.a, m.b);
            }
        }
        let kind = CopyKind::of(&m.kind);
        links.entry(m.a).or_default().push((m.b, kind));
        links.entry(m.b).or_default().push((m.a, kind));
    }

    // Each compressed file joins the one set it's closest to (if it isn't in one already).
    let mut set_sizes: HashMap<i64, usize> = HashMap::new();
    for x in same.order.clone() {
        *set_sizes.entry(same.find(x)).or_default() += 1;
    }
    let in_set: HashSet<i64> = same.order.clone().into_iter().filter(|x| set_sizes[&same.find(*x)] > 1).collect();
    // Stable, as C#'s OrderBy.
    lossy_links.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut attached: HashMap<i64, i64> = HashMap::new();
    for (file, to, _) in &lossy_links {
        if !in_set.contains(file) && !attached.contains_key(file) {
            let root = same.find(*to);
            attached.insert(*file, root);
        }
    }
    let mut set_of = |id: i64| match attached.get(&id) {
        Some(to) => same.find(*to),
        None => same.find(id),
    };

    // Each group of copies, in the order its sounds were first met.
    let mut grouped: Vec<(i64, Vec<i64>)> = Vec::new();
    let mut group_at: HashMap<i64, usize> = HashMap::new();
    for x in copies.order.clone() {
        let root = copies.find(x);
        let i = *group_at.entry(root).or_insert_with(|| {
            grouped.push((root, Vec::new()));
            grouped.len() - 1
        });
        grouped[i].1.push(x);
    }

    let mut out = Vec::new();
    for (_, ids) in grouped {
        let members: Vec<&Sound> = ids.iter().filter_map(|id| by_id.get(id).copied()).collect();
        if members.len() < 2 {
            continue;
        }
        let sets: HashMap<i64, i64> = members.iter().map(|s| (s.id, set_of(s.id))).collect();
        let mut set_size: HashMap<i64, usize> = HashMap::new();
        for s in &members {
            *set_size.entry(sets[&s.id]).or_default() += 1;
        }
        let used = |s: &Sound| used_in(usage, s.id);
        let better = |a: &&Sound, b: &&Sound| -> Ordering {
            a.hidden
                .cmp(&b.hidden)
                .then(used(b).cmp(&used(a)))
                // One of the copies that are the same sound, so the others can go.
                .then((set_size[&sets[&b.id]] > 1).cmp(&(set_size[&sets[&a.id]] > 1)))
                .then(is_lossless(&b.path).cmp(&is_lossless(&a.path)))
                .then(round(b.seconds(), 2).total_cmp(&round(a.seconds(), 2)))
                .then(a.added.cmp(&b.added))
                // A turned-down copy is rarely the original.
                .then(peak(b).total_cmp(&peak(a)))
                .then(case::key(&a.path).cmp(&case::key(&b.path)))
        };
        // The best of `of` (stable, as C#'s OrderBy).
        let best = |of: &[&Sound]| -> i64 {
            let mut ranked: Vec<&Sound> = of.to_vec();
            ranked.sort_by(better);
            ranked[0].id
        };
        let keep = by_id[&best(&members)];
        let keep_set = sets[&keep.id];
        // In each other set of the same sound, the best one stays and the rest are copies of it.
        let mut lead: HashMap<i64, &Sound> = HashMap::new();
        let mut by_set: Vec<(i64, Vec<&Sound>)> = Vec::new();
        for s in &members {
            let set = sets[&s.id];
            match by_set.iter_mut().find(|(k, _)| *k == set) {
                Some((_, v)) => v.push(s),
                None => by_set.push((set, vec![s])),
            }
        }
        for (set, list) in &by_set {
            lead.insert(*set, if *set == keep_set { keep } else { by_id[&best(list)] });
        }
        let kind_of = |s: &Sound, of: &Sound| -> CopyKind {
            if let Some(h) = hash_of.get(&s.id)
                && hash_of.get(&of.id) == Some(h)
            {
                return CopyKind::Exact;
            }
            let own = links.get(&s.id).map_or(&[][..], Vec::as_slice);
            if let Some((_, k)) = own.iter().find(|(other, _)| *other == of.id) {
                return *k;
            }
            own.iter().map(|(_, k)| *k).max().unwrap_or(CopyKind::Exact)
        };

        struct Copy<'a> {
            sound: &'a Sound,
            kind: CopyKind,
            same: bool,
            of: &'a Sound,
            set: i64,
        }
        let mut rest: Vec<Copy> = members
            .iter()
            .filter(|s| s.id != keep.id)
            .map(|s| {
                let set = sets[&s.id];
                let of = if set == keep_set { keep } else { lead[&set] };
                let same = s.id != of.id;
                let mut kind = kind_of(s, if same { of } else { keep });
                if same && kind == CopyKind::Trimmed {
                    // The same via another copy.
                    kind = CopyKind::Converted;
                }
                Copy { sound: s, kind, same, of, set }
            })
            .collect();
        rest.sort_by(|a, b| {
            (b.same && b.set == keep_set)
                .cmp(&(a.same && a.set == keep_set))
                .then((a.set != keep_set).cmp(&(b.set != keep_set)))
                .then(a.set.cmp(&b.set))
                .then(a.same.cmp(&b.same))
                .then(a.kind.cmp(&b.kind))
                .then(case::key(&a.sound.path).cmp(&case::key(&b.sound.path)))
        });
        let mut list = vec![DuplicateMember {
            id: keep.id,
            size: size(keep.id),
            keep: true,
            kind: None,
            why: keep_reason(keep, &members, usage),
            same: true,
            same_set: keep_set,
        }];
        list.extend(rest.iter().map(|c| DuplicateMember {
            id: c.sound.id,
            size: size(c.sound.id),
            keep: false,
            kind: Some(c.kind),
            // A copy that's only like the others is described against the one to keep (C# used its own set's lead,
            // itself when alone, so a trimmed copy always read "Longer version").
            why: describe(c.kind, c.same, c.sound, if c.same { c.of } else { keep }, c.same && c.of.id != keep.id),
            same: c.same,
            same_set: c.set,
        }));
        let spare_bytes = list.iter().filter(|m| !m.keep && m.same).map(|m| m.size).sum();
        let kind = list.iter().filter_map(|m| m.kind).max().unwrap_or(CopyKind::Exact);
        out.push(DuplicateGroup { members: list, kind, spare_bytes });
    }
    // Stable, as C#'s OrderBy: the biggest saving first, then the biggest group.
    out.sort_by(|a, b| b.spare_bytes.cmp(&a.spare_bytes).then(b.members.len().cmp(&a.members.len())));
    out
}

/// The loudest point, to the whole dB (-99 when not analysed).
fn peak(s: &Sound) -> f64 {
    round(s.analysis.as_ref().map_or(-99.0, |a| a.peak_db), 0)
}

fn keep_reason(keep: &Sound, all: &[&Sound], usage: &HashMap<i64, Usage>) -> String {
    let used = used_in(usage, keep.id);
    if used > 0 && all.iter().filter(|s| used_in(usage, s.id) > 0).count() == 1 {
        return if used == 1 {
            "Best to keep: your projects use this one".to_owned()
        } else {
            format!("Best to keep: {used} of your projects use this one")
        };
    }
    if used > 0 {
        return "Best to keep: your projects use it most".to_owned();
    }
    if is_lossless(&keep.path) && all.iter().any(|s| !is_lossless(&s.path)) {
        return "Best to keep: full quality, not compressed".to_owned();
    }
    if all.iter().any(|s| s.seconds() < keep.seconds() - 0.05) {
        return "Best to keep: the full-length one".to_owned();
    }
    "Best to keep: the oldest copy".to_owned()
}

fn describe(kind: CopyKind, same: bool, s: &Sound, keep: &Sound, of_another: bool) -> String {
    match kind {
        CopyKind::Exact if same && of_another => format!("Exact copy of {}", keep.name()),
        CopyKind::Converted if same && of_another => format!("Same sound as {}", keep.name()),
        CopyKind::Exact if same => {
            if s.name() == keep.name() {
                "Exact copy".to_owned()
            } else {
                "Exact copy, renamed".to_owned()
            }
        }
        CopyKind::Converted if same => match changes(s, keep) {
            c if !c.is_empty() => format!("Same sound, {c}"),
            _ => "Same sound, another file".to_owned(),
        },
        CopyKind::Exact | CopyKind::Converted => "Very similar, but not the same sound all the way through".to_owned(),
        CopyKind::Trimmed => {
            if s.seconds() < keep.seconds() {
                format!("Trimmed: {} s of {} s", short(s.seconds(), 2), short(keep.seconds(), 2))
            } else {
                format!("Longer version ({} s)", short(s.seconds(), 2))
            }
        }
    }
}

fn changes(s: &Sound, keep: &Sound) -> String {
    let mut bits = Vec::new();
    if !extension(&s.path).eq_ignore_ascii_case(extension(&keep.path)) {
        bits.push(format!("converted to {}", extension(&s.path).to_uppercase()));
    }
    if let (Some(a), Some(k)) = (&s.analysis, &keep.analysis) {
        if a.sample_rate != k.sample_rate {
            bits.push(format!("{} kHz", short(f64::from(a.sample_rate) / 1000.0, 1)));
        }
        if (a.peak_db - k.peak_db).abs() >= 1.0 {
            bits.push(if a.peak_db > k.peak_db { "louder" } else { "quieter" }.to_owned());
        }
    }
    if !case::eq_ignore_case(s.name(), keep.name()) {
        bits.push("renamed".to_owned());
    }
    bits.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SoundAnalysis;
    use crate::test_support::sound;

    fn analysed(id: i64, path: &str, seconds: f64, rate: i32, peak: f64) -> Sound {
        let mut s = sound(id, path);
        s.analysis = Some(SoundAnalysis { seconds, sample_rate: rate, peak_db: peak, ..SoundAnalysis::default() });
        s
    }

    fn matched(a: i64, b: i64, kind: &str, apart: Option<f64>, typical: Option<f64>) -> SoundMatch {
        SoundMatch { a, b, kind: kind.to_owned(), score: 1.0, offset: 0.0, apart, typical }
    }

    #[test]
    fn exact_and_converted_copies_group_with_the_best_to_keep_first() {
        let sounds = vec![
            analysed(1, "/a/Kick.wav", 1.0, 44_100, -1.0),
            analysed(2, "/b/Kick copy.wav", 1.0, 44_100, -1.0),
            analysed(3, "/c/Kick.mp3", 1.02, 44_100, -1.0),
            analysed(4, "/d/Snare.wav", 2.0, 48_000, -3.0),
            analysed(5, "/d/Snare short.wav", 0.5, 48_000, -3.0),
        ];
        let usage = HashMap::from([(2, Usage { projects: 3, ..Usage::default() })]);
        let exact = vec![(1, "AB".to_owned()), (2, "AB".to_owned())];
        let matches = vec![
            matched(1, 3, "Converted", Some(0.5), Some(0.4)),
            matched(4, 5, "Trimmed", Some(0.2), Some(0.3)),
            // Only a shared rhythm: not a copy.
            matched(1, 4, "Converted", Some(9.0), Some(8.0)),
        ];
        let sizes = HashMap::from([(1, 100), (2, 100), (3, 40), (4, 300), (5, 80)]);
        let g = groups(&sounds, &usage, &matches, &exact, &sizes);
        assert_eq!(g.len(), 2);
        let kick = &g[0];
        assert_eq!(kick.members.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 1, 3]);
        assert_eq!(kick.members[0].why, "Best to keep: 3 of your projects use this one");
        assert_eq!(kick.members[1].why, "Exact copy, renamed");
        assert_eq!(kick.members[1].kind, Some(CopyKind::Exact));
        // The MP3 is close enough for a compressed copy: the same sound.
        assert_eq!(kick.members[2].why, "Same sound, converted to MP3, renamed");
        assert!(kick.members[2].same);
        assert_eq!(kick.spare_bytes, 140);
        assert_eq!(kick.same_copies(), 2);
        let snare = &g[1];
        assert_eq!(snare.members[0].id, 4);
        assert_eq!(snare.members[0].why, "Best to keep: the full-length one");
        assert_eq!(snare.members[1].why, "Trimmed: 0.5 s of 2 s");
        assert!(snare.has_similar());
        assert_eq!(snare.spare_bytes, 0);
    }

    #[test]
    fn a_converted_copy_too_far_apart_is_only_very_similar() {
        let sounds = vec![analysed(1, "/a/Pad.wav", 3.0, 44_100, -1.0), analysed(2, "/b/Pad.flac", 3.0, 48_000, -6.0)];
        let g = groups(
            &sounds,
            &HashMap::new(),
            &[matched(1, 2, "Converted", Some(0.4), None)],
            &[],
            &HashMap::from([(1, 10), (2, 10)]),
        );
        let m = &g[0].members;
        assert_eq!(m[1].why, "Very similar, but not the same sound all the way through");
        assert!(!m[1].same);
        // Same sound, when close enough: what changed is listed.
        let g = groups(
            &sounds,
            &HashMap::new(),
            &[matched(1, 2, "Converted", Some(0.05), None)],
            &[],
            &HashMap::from([(1, 10), (2, 10)]),
        );
        assert_eq!(g[0].members[1].why, "Same sound, converted to FLAC, 48 kHz, quieter");
    }

    #[test]
    fn numbers_as_dotnet_writes_them() {
        assert_eq!(short(2.0, 2), "2");
        assert_eq!(short(0.125, 2), "0.13");
        assert_eq!(short(44.1, 1), "44.1");
        assert_eq!(short(48.0, 1), "48");
        assert_eq!(extension("/a/b.c/Kick"), "");
        assert_eq!(extension("/a/Kick.WAV"), "WAV");
    }
}
