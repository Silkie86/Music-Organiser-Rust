//! Every MIDI part in the index, held in memory for instant searching (the C# `MidiCatalogue`).
//!
//! Parts with the same notes are one entry, the newest copy, with the other files that have them ("in 20 files").
//! Search words must all appear in a part's names, file, folders, kind, instrument or key. "More like this" ranks by
//! the part's shape. Each search is one pass over the list, spread over the cores, quick even for hundreds of
//! thousands of parts; the caller runs it off the UI thread at low priority (rules 12 and 21).
//!
//! Not here yet: fitting a song's harmony and finding chords by name (they need `mo-harmony`, phase 6), and searching
//! by description with CLaMP 3 (its model comes later).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use mo_formats::{case, paths};
use rayon::prelude::*;

use crate::index::{Source, SourceKind, StoredPhrase};
use crate::instruments::{self, Family, InstrumentGuess};
use crate::member::PhraseMember;
use crate::roles::Role;

/// One part as the page lists it: the newest copy of some notes, with every other file that has the same notes.
#[derive(Debug)]
pub struct Entry {
    pub phrase: StoredPhrase,
    /// The other files with the same notes (older saves of the song, or the same pattern in another project), newest
    /// first.
    pub other_sources: Vec<Arc<Source>>,
    /// The instrument it's for, when its names, plugin, channel or General MIDI sound say.
    pub instrument: Option<InstrumentGuess>,
    /// For a phrase joined from clips that play back to back: those clips, and where each sits in it.
    pub members: Vec<PhraseMember>,
    /// The longer phrases these notes are part of (as places in [`Catalogue::entries`]), longest first.
    pub within: Vec<usize>,
    /// Everything search looks in, in small letters.
    search_text: String,
    /// The songs this part is in (see [`song_of`]), worked out the first time they're asked for.
    songs: OnceLock<Vec<String>>,
}

impl Entry {
    /// How many files have these notes.
    pub fn copies(&self) -> usize {
        1 + self.other_sources.len()
    }

    /// What it is: what you said, or else the guess.
    pub fn role(&self) -> Role {
        self.phrase.role()
    }

    pub fn is_project(&self) -> bool {
        self.phrase.source.kind == SourceKind::Project
    }

    pub fn is_joined(&self) -> bool {
        !self.members.is_empty()
    }

    /// How many different clips a joined phrase was made from.
    pub fn clip_count(&self) -> usize {
        let mut hashes: Vec<&str> = self.members.iter().map(|m| m.hash.as_str()).collect();
        hashes.sort_unstable();
        hashes.dedup();
        hashes.len()
    }

    /// The file's name without its ending: the song save or the `.mid` file.
    pub fn source_name(&self) -> &str {
        file_stem(&self.phrase.source.path)
    }

    /// "Bass · Pattern 3" for a project's part; "Dark keys 01" (or "Dark keys 01 · Piano") for a file.
    pub fn title(&self) -> String {
        let (a, b) = if self.is_project() {
            (&self.phrase.part, &self.phrase.clip)
        } else {
            (&self.phrase.clip, &self.phrase.part)
        };
        [a.as_str(), b.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
    }

    /// The key, when the reading is sure enough to show ("Ebm"), else "".
    pub fn key_text(&self) -> String {
        self.phrase.key.filter(|k| k.is_sure()).map(|k| k.short()).unwrap_or_default()
    }

    /// Whether this song (as [`song_of`] gives it) has these notes, in any of its saves.
    pub fn is_in_song(&self, song: &str) -> bool {
        self.songs
            .get_or_init(|| {
                std::iter::once(&self.phrase.source).chain(&self.other_sources).map(|s| song_of(&s.path)).collect()
            })
            .iter()
            .any(|s| s == song)
    }

    /// Whether every word (already in small letters) is in its names, folders, kind, instrument or key.
    pub fn matches(&self, words: &[String]) -> bool {
        words.iter().all(|w| self.search_text.contains(w.as_str()))
    }
}

/// Where parts come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    Projects,
    Files,
}

/// What to look for.
#[derive(Clone, Debug)]
pub struct Query {
    /// Words that must all appear.
    pub text: String,
    /// Only this kind of part.
    pub role: Option<Role>,
    pub from: Option<From>,
    /// Rank by how alike each part is to this one (a place in [`Catalogue::entries`]).
    pub like: Option<usize>,
    /// Only the longer phrases this part is in.
    pub within: Option<usize>,
    /// Leave out every part in these songs (each from [`song_of`]): a song's own parts say nothing about it.
    pub leave_songs: Vec<String>,
    /// The most hits to give.
    pub limit: usize,
    /// A random pick from everything that matches, in an order this number decides, instead of newest first.
    pub shuffle: Option<u64>,
}

impl Default for Query {
    fn default() -> Query {
        Query {
            text: String::new(),
            role: None,
            from: None,
            like: None,
            within: None,
            leave_songs: Vec::new(),
            limit: 500,
            shuffle: None,
        }
    }
}

/// A part found: its place in [`Catalogue::entries`], and how alike it is for "more like this".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub entry: usize,
    pub likeness: Option<f64>,
}

/// Every part, one entry per set of notes, newest first.
#[derive(Debug, Default)]
pub struct Catalogue {
    pub entries: Vec<Entry>,
    /// How many parts the index holds, copies counted.
    pub phrase_count: usize,
    by_hash: HashMap<String, usize>,
}

impl Catalogue {
    /// The catalogue of `phrases`, which come newest file first (as [`crate::MidiIndex::all`] gives them), so the
    /// first of each set of notes is the newest copy.
    pub fn new(phrases: Vec<StoredPhrase>) -> Catalogue {
        let phrase_count = phrases.len();
        let mut by_hash: HashMap<String, usize> = HashMap::with_capacity(phrases.len());
        let mut firsts = Vec::new();
        let mut others: Vec<Vec<Arc<Source>>> = Vec::new();
        for p in phrases {
            if let Some(&at) = by_hash.get(&p.hash) {
                if let Some(list) = others.get_mut(at) {
                    list.push(p.source);
                }
                continue;
            }
            by_hash.insert(p.hash.clone(), firsts.len());
            firsts.push(p);
            others.push(Vec::new());
        }
        // Names, instruments and search text are worked out on every core: hundreds of thousands of parts.
        let mut entries: Vec<Entry> = firsts
            .into_par_iter()
            .zip(others)
            .map(|(phrase, other_sources)| {
                let instrument = instrument_of(&phrase);
                let search_text = search_text_of(&phrase, phrase.role(), instrument.as_ref());
                let members = PhraseMember::decode(&phrase.members);
                Entry {
                    phrase,
                    other_sources,
                    instrument,
                    members,
                    within: Vec::new(),
                    search_text,
                    songs: OnceLock::new(),
                }
            })
            .collect();

        // Each clip learns which longer phrases it's part of.
        let mut within: Vec<(usize, usize)> = Vec::new();
        for (at, e) in entries.iter().enumerate().filter(|(_, e)| e.is_joined()) {
            let mut hashes: Vec<&str> = e.members.iter().map(|m| m.hash.as_str()).collect();
            hashes.sort_unstable();
            hashes.dedup();
            for hash in hashes {
                if let Some(&member) = by_hash.get(hash).filter(|m| **m != at) {
                    within.push((member, at));
                }
            }
        }
        for (member, phrase) in within {
            if let Some(e) = entries.get_mut(member) {
                e.within.push(phrase);
            }
        }
        let bars: Vec<i32> = entries.iter().map(|e| e.phrase.bars).collect();
        for e in &mut entries {
            // Longest first; equal lengths keep their order (newest first), as the C# stable sort does.
            e.within.sort_by_key(|&p| std::cmp::Reverse(bars.get(p).copied().unwrap_or(0)));
        }
        Catalogue { entries, phrase_count, by_hash }
    }

    /// The part with these notes. Entries are made again on every reload, so the page holds on to its picks by
    /// their notes and finds them again here.
    pub fn find(&self, hash: &str) -> Option<usize> {
        self.by_hash.get(hash).copied()
    }

    pub fn entry(&self, at: usize) -> Option<&Entry> {
        self.entries.get(at)
    }

    /// Changes what kind of part this is (`None`: back to the app's guess), for searching and the chips too. Only
    /// in memory: saving it is the index's job once the Rust app writes one.
    pub fn relabel(&mut self, at: usize, role: Option<Role>) {
        if let Some(e) = self.entries.get_mut(at) {
            e.phrase.your_role = role;
            e.search_text = search_text_of(&e.phrase, e.phrase.role(), e.instrument.as_ref());
        }
    }

    /// How many parts of each kind there are, for the chips.
    pub fn role_counts(&self) -> HashMap<Role, usize> {
        let mut counts = HashMap::new();
        for e in &self.entries {
            *counts.entry(e.role()).or_insert(0) += 1;
        }
        counts
    }

    /// The parts that match `q`, best or newest first, with how many matched in all.
    pub fn search(&self, q: &Query) -> (Vec<Hit>, usize) {
        if let Some(part) = q.within {
            let phrases = self.entries.get(part).map(|e| e.within.as_slice()).unwrap_or_default();
            let hits = phrases.iter().take(q.limit).map(|&entry| Hit { entry, likeness: None }).collect();
            return (hits, phrases.len());
        }
        let words: Vec<String> = q.text.split(' ').filter(|w| !w.is_empty()).map(case::to_lower).collect();
        let pool: Vec<usize> = self
            .entries
            .par_iter()
            .enumerate()
            .filter(|(at, e)| {
                // Cheapest first: working out a part's songs is the slowest test, so it comes last.
                q.role.is_none_or(|r| e.role() == r)
                    && q.from.is_none_or(|f| (f == From::Projects) == e.is_project())
                    && q.like != Some(*at)
                    && e.matches(&words)
                    && !q.leave_songs.iter().any(|s| e.is_in_song(s))
            })
            .map(|(at, _)| at)
            .collect();
        let matching = pool.len();

        if let Some(like) = q.like.and_then(|at| self.entries.get(at)) {
            let mine = &like.phrase.features;
            let mut alike: Vec<Hit> = pool
                .par_iter()
                .map(|&entry| {
                    let theirs = self.entries.get(entry).map(|e| e.phrase.features.as_slice()).unwrap_or_default();
                    Hit { entry, likeness: Some(likeness(mine, theirs)) }
                })
                .collect();
            // Most alike first; equal ones keep their order (newest first).
            alike.sort_by(|a, b| b.likeness.unwrap_or(0.0).total_cmp(&a.likeness.unwrap_or(0.0)));
            alike.truncate(q.limit);
            if let Some(seed) = q.shuffle {
                shuffle(&mut alike, seed);
            }
            return (alike, matching);
        }
        let mut pool = pool;
        if let Some(seed) = q.shuffle {
            // A random few hundred from everything that matches, not just the newest: only the places kept are drawn.
            pick(&mut pool, q.limit, seed);
        }
        let hits = pool.into_iter().take(q.limit).map(|entry| Hit { entry, likeness: None }).collect();
        (hits, matching)
    }
}

/// How alike two parts' shapes are, 0 to 1 (the C# `PhraseAnalyser.Likeness`): the first six numbers (the kind of
/// part) count three times as much as the finer detail. Stored as bytes, each 0 to 255 for 0 to 1.
pub fn likeness(a: &[u8], b: &[u8]) -> f64 {
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (i, (&x, &y)) in a.iter().zip(b).enumerate() {
        let (x, y) = (f64::from(x) / 255.0, f64::from(y) / 255.0);
        let w = if i < 6 { 3.0 } else { 1.0 };
        ab += w * x * y;
        aa += w * x * x;
        bb += w * y * y;
    }
    if aa <= 0.0 || bb <= 0.0 { 0.0 } else { ab / (aa * bb).sqrt() }
}

/// The song a file is a save of, as one piece of text: its folder (a Bitwig project's own folder counts as the one
/// it's in) and its name without the save number, in small letters (the C# `MidiEntry.SongOf`).
pub fn song_of(path: &str) -> String {
    let name = file_stem(path);
    let (folder, separator) = match path.rfind(['\\', '/']) {
        Some(cut) => (path.get(..cut).unwrap_or(""), path.get(cut..=cut).unwrap_or("/")),
        None => ("", "/"),
    };
    let song_folder = mo_library::songs::song_folder(folder, name);
    let song = mo_library::songs::split(name).0;
    let joined = if song_folder.is_empty() { song } else { format!("{song_folder}{separator}{song}") };
    case::to_lower(&joined)
}

/// The instrument a part is for, from what's attached to it: its track (or channel) name and its clip; for a `.mid`
/// file also the file's name and its folders from the nearest up (packs sort into folders like "Piano"), though in a
/// project those name the song, not the part; channel 10 or a drum plugin; the General MIDI sound a `.mid` file sets.
pub fn instrument_of(p: &StoredPhrase) -> Option<InstrumentGuess> {
    let is_file = p.source.kind == SourceKind::File;
    let mut names: Vec<&str> = vec![&p.part];
    if is_file {
        names.extend([p.track_name.as_str(), p.clip.as_str(), file_stem(&p.source.path)]);
        let mut folders = folders_of(&p.source.path);
        let keep = folders.len().saturating_sub(3);
        folders.drain(..keep);
        names.extend(folders.into_iter().rev());
    } else {
        names.push(&p.clip);
    }
    let guess = instruments::guess(names, p.drums && is_file, p.program);
    // A project track flagged as drums plays an unpitched plugin (a drum machine or sampler of hits).
    guess.or_else(|| {
        p.drums.then(|| InstrumentGuess { family: Family::Drums, why: format!("“{}” is a drum plugin", p.part) })
    })
}

/// A file's name without its ending, with either separator before it: the files come from Windows, wherever this runs.
fn file_stem(path: &str) -> &str {
    paths::stem(paths::file_name(path))
}

/// The folders a file sits in, from the top down.
fn folders_of(path: &str) -> Vec<&str> {
    let folder = path.rfind(['\\', '/']).and_then(|cut| path.get(..cut)).unwrap_or("");
    folder.split(['\\', '/']).filter(|f| !f.is_empty()).collect()
}

fn search_text_of(p: &StoredPhrase, role: Role, instrument: Option<&InstrumentGuess>) -> String {
    let mut parts: Vec<&str> =
        vec![&p.part, &p.clip, file_stem(&p.source.path), role.name(), role.plural(), &p.source.daw];
    if let Some(i) = instrument {
        parts.push(i.family.search_words());
    }
    // The General MIDI sound it asks for, except the default piano (what files write when nobody chose) and on drums,
    // where the number picks a drum kit, not one of these sounds.
    if let Some(program) = p.program.filter(|p| (1..128).contains(p)).filter(|_| !p.drums) {
        parts.push(instruments::general_midi(program));
    }
    // The folders it's in (the last few): MIDI packs are sorted into folders like "Chords" and "Trap Melodies".
    let folders = folders_of(&p.source.path);
    parts.extend(folders.iter().skip(folders.len().saturating_sub(4)));
    let key = p.key.filter(|k| k.is_sure()).map(|k| [k.name(), k.short()]);
    if let Some([name, short]) = &key {
        parts.extend([name.as_str(), short.as_str()]);
    }
    if role == Role::Bass {
        parts.push("bassline");
    }
    if role == Role::Chords {
        parts.push("progression");
    }
    case::to_lower(&parts.join(" "))
}

/// The random numbers shuffling uses: SplitMix64, from `seed`.
fn random(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Puts the list in a random order that `seed` decides (Fisher–Yates).
fn shuffle<T>(list: &mut [T], seed: u64) {
    let mut next = random(seed);
    for i in (1..list.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        list.swap(i, j);
    }
}

/// Moves a random `count` of the list (that `seed` decides) to its front, in random order, and drops the rest: as
/// [`shuffle`] and keeping the first `count`, but only drawing for the places kept.
fn pick<T>(list: &mut Vec<T>, count: usize, seed: u64) {
    let mut next = random(seed);
    let n = count.min(list.len());
    for i in 0..n {
        let j = i + (next() % (list.len() - i) as u64) as usize;
        list.swap(i, j);
    }
    list.truncate(n);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Key;

    fn source(path: &str, kind: SourceKind, mtime: i64) -> Arc<Source> {
        let daw = if kind == SourceKind::File { "MIDI" } else { "FL Studio" };
        Arc::new(Source { path: path.into(), kind, daw: daw.into(), mtime, size: 1, tempo: None })
    }

    fn phrase(id: i64, src: &Arc<Source>, part: &str, clip: &str, role: Role, hash: &str) -> StoredPhrase {
        StoredPhrase {
            id,
            source: Arc::clone(src),
            part: part.into(),
            clip: clip.into(),
            role,
            role_why: String::new(),
            your_role: None,
            bars: 4,
            notes: 8,
            lowest: 40,
            highest: 60,
            key: None,
            key_fact: None,
            chroma: [0; 12],
            chords: Vec::new(),
            features: vec![0; 48],
            hash: hash.into(),
            members: String::new(),
            program: None,
            drums: false,
            track_name: String::new(),
            your_octave: None,
        }
    }

    fn sample() -> Catalogue {
        let new_save = source(r"C:\Projects\Didums\Didums_70.flp", SourceKind::Project, 300);
        let old_save = source(r"C:\Projects\Didums\Didums_69.flp", SourceKind::Project, 200);
        let pack = source(r"D:\Packs\Trap Melodies\Dark keys 01.mid", SourceKind::File, 100);
        let mut joined = phrase(5, &new_save, "Piano", "Verse → Hook", Role::Chords, "J");
        joined.members = "A@0+768 B@768+768".into();
        joined.bars = 8;
        let mut keys = phrase(4, &pack, "", "Dark keys 01", Role::Melody, "C");
        keys.key = Some(Key { tonic: 3, minor: true, confidence: 0.3 });
        Catalogue::new(vec![
            phrase(1, &new_save, "Piano", "Verse", Role::Chords, "A"),
            phrase(2, &new_save, "Bass", "Hook", Role::Bass, "B"),
            joined,
            phrase(3, &old_save, "Piano", "Verse", Role::Chords, "A"),
            keys,
        ])
    }

    fn titles(c: &Catalogue, hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| c.entries[h.entry].title()).collect()
    }

    #[test]
    fn copies_are_one_entry() {
        let c = sample();
        assert_eq!(c.phrase_count, 5);
        assert_eq!(c.entries.len(), 4);
        let verse = &c.entries[c.find("A").unwrap()];
        assert_eq!(verse.copies(), 2);
        assert_eq!(verse.phrase.id, 1, "the newest copy is the one kept");
        assert!(verse.is_in_song(&song_of(r"C:\Projects\Didums\Didums_12.flp")));
        assert!(!verse.is_in_song(&song_of(r"C:\Projects\Other\Didums_12.flp")));
    }

    #[test]
    fn clips_know_the_phrases_they_are_in() {
        let c = sample();
        let joined = c.find("J").unwrap();
        assert_eq!(c.entries[c.find("A").unwrap()].within, vec![joined]);
        assert_eq!(c.entries[c.find("B").unwrap()].within, vec![joined]);
        assert_eq!(c.entries[joined].clip_count(), 2);
        let (hits, n) = c.search(&Query { within: c.find("A"), ..Query::default() });
        assert_eq!((n, hits[0].entry), (1, joined));
    }

    #[test]
    fn words_must_all_match() {
        let c = sample();
        let find = |text: &str| titles(&c, &c.search(&Query { text: text.into(), ..Query::default() }).0);
        assert_eq!(find("bassline"), ["Bass · Hook"]);
        assert_eq!(find("TRAP keys"), ["Dark keys 01"]);
        assert_eq!(find("ebm"), ["Dark keys 01"], "a sure key is searchable");
        assert_eq!(find("piano progression").len(), 2);
        assert!(find("piano bass hook").is_empty());
        assert_eq!(find("").len(), 4);
    }

    #[test]
    fn filters_and_leaving_a_song_out() {
        let c = sample();
        let (hits, n) = c.search(&Query { from: Some(From::Files), ..Query::default() });
        assert_eq!((n, titles(&c, &hits)), (1, vec!["Dark keys 01".to_owned()]));
        let (_, n) = c.search(&Query { role: Some(Role::Chords), ..Query::default() });
        assert_eq!(n, 2);
        let didums = song_of(r"C:\Projects\Didums\Didums_1.flp");
        let (hits, _) = c.search(&Query { leave_songs: vec![didums], ..Query::default() });
        assert_eq!(titles(&c, &hits), ["Dark keys 01"]);
    }

    #[test]
    fn relabelling_changes_counts_and_search() {
        let mut c = sample();
        assert_eq!(c.role_counts()[&Role::Melody], 1);
        let keys = c.find("C").unwrap();
        c.relabel(keys, Some(Role::Arp));
        assert_eq!(c.role_counts().get(&Role::Melody), None);
        assert_eq!(c.search(&Query { text: "arps".into(), ..Query::default() }).1, 1);
        c.relabel(keys, None);
        assert_eq!(c.entries[keys].role(), Role::Melody);
    }

    #[test]
    fn more_like_this_ranks_by_shape() {
        let mut c = sample();
        let shape = |v: &[u8]| -> Vec<u8> { v.iter().copied().chain(std::iter::repeat(0)).take(48).collect() };
        let (a, b, k) = (c.find("A").unwrap(), c.find("B").unwrap(), c.find("C").unwrap());
        c.entries[a].phrase.features = shape(&[255, 200, 10]);
        c.entries[b].phrase.features = shape(&[0, 0, 255]);
        c.entries[k].phrase.features = shape(&[250, 190, 20]);
        let (hits, _) = c.search(&Query { like: Some(a), ..Query::default() });
        assert_eq!(hits[0].entry, k);
        assert!(hits[0].likeness.unwrap() > 0.99);
        assert!(hits.iter().all(|h| h.entry != a), "not the part itself");
    }

    #[test]
    fn shuffle_is_the_same_for_the_same_seed() {
        let c = sample();
        let q = Query { shuffle: Some(7), ..Query::default() };
        assert_eq!(c.search(&q), c.search(&q));
        let mut a: Vec<u32> = (0..50).collect();
        shuffle(&mut a, 7);
        assert_ne!(a, (0..50).collect::<Vec<_>>());
        a.sort_unstable();
        assert_eq!(a, (0..50).collect::<Vec<_>>());
        let mut b: Vec<u32> = (0..50).collect();
        pick(&mut b, 10, 7);
        assert_eq!(b.len(), 10);
        b.sort_unstable();
        b.dedup();
        assert_eq!(b.len(), 10);
        let mut few = vec![1, 2];
        pick(&mut few, 10, 7);
        assert_eq!(few.len(), 2);
    }

    #[test]
    fn songs_and_instruments() {
        assert_eq!(
            song_of(r"C:\Projects\bored_58\bored_58.bwproject"),
            song_of(r"C:\Projects\bored_57\bored_57.bwproject")
        );
        assert_eq!(song_of("/music/Didums_70.flp"), "/music/didums");
        let pack = source(r"D:\Packs\Piano\Loops\Pack 1\Track.mid", SourceKind::File, 1);
        let p = phrase(1, &pack, "", "Track", Role::Chords, "x");
        assert_eq!(instrument_of(&p).unwrap().family, Family::Piano);
        let song = source(r"C:\Piano songs\Song.flp", SourceKind::Project, 1);
        let mut p = phrase(1, &song, "Sampler 3", "Pattern 1", Role::Chords, "x");
        assert_eq!(instrument_of(&p), None, "a project's folders name the song, not the part");
        p.drums = true;
        assert_eq!(instrument_of(&p).unwrap().why, "“Sampler 3” is a drum plugin");
    }
}
