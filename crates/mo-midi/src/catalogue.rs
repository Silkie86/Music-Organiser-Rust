//! Every MIDI part in the index, held in memory for instant searching (the C# `MidiCatalogue`).
//!
//! Parts with the same notes are one entry, the newest copy, with the other files that have them ("in 20 files"). A
//! file's date that isn't real (one still to come, or before 1981: [`RealDates`]) is no clue to when it was saved, so
//! such a copy comes after the copies with a real date.
//! Search words must all appear in a part's names, file, folders, kind, instrument or key. "More like this" ranks by
//! the part's shape. Each search is one pass over the list, spread over the cores, quick even for hundreds of
//! thousands of parts; the caller runs it off the UI thread at low priority (rules 12 and 21).
//!
//! Not here yet: fitting a song's harmony and finding chords by name (they need `mo-harmony`, phase 6), and searching
//! by description with CLaMP 3 (its model comes later).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use mo_formats::dates::RealDates;
use mo_formats::{case, paths};
use rayon::prelude::*;

use crate::index::{Source, SourceKind, StoredPhrase};
use crate::instruments::{self, Family, InstrumentGuess};
use crate::member::PhraseMember;
use crate::roles::Role;

/// Parts sorted into sets of the same notes (the same [`StoredPhrase::hash`]): the newest of each set, in the
/// order [`Catalogue::new`] wants its parts in (newest file first, a date that isn't real last), and beside each
/// (`others` has one list for each of `firsts`) every other file with those notes, newest first.
#[derive(Debug, Default)]
pub struct Grouped {
    pub firsts: Vec<StoredPhrase>,
    pub others: Vec<Vec<Arc<Source>>>,
}

/// One part as the page lists it: the newest copy of some notes, with every other file that has the same notes.
#[derive(Debug)]
pub struct Entry {
    pub phrase: StoredPhrase,
    /// The other files with the same notes (older saves of the song, or the same pattern in another project), newest
    /// first.
    pub other_sources: Box<[Arc<Source>]>,
    /// The instrument it's for, when its names, plugin, channel or General MIDI sound say. How the app knows is
    /// worked out when it's asked for ([`Entry::instrument_why`]), not kept for every part.
    pub instrument: Option<Family>,
    /// For a phrase joined from clips that play back to back: those clips, and where each sits in it.
    pub members: Box<[PhraseMember]>,
    /// The longer phrases these notes are part of (as places in [`Catalogue::entries`]), longest first.
    pub within: Box<[usize]>,
    /// [`Entry::key_group`], or [`NO_GROUP`].
    key_group: u32,
    /// [`Entry::in_keys`]: how many, or 0 for none; and whether they're octaves.
    in_keys: u32,
    in_octaves: bool,
    /// Where its search text is in the catalogue's one text, and how long: see [`Catalogue::matches`].
    text_at: usize,
    text_len: u32,
}

/// How many entries' search texts [`Catalogue::new`] makes into one text at a time.
const TEXT_RUN: usize = 4_096;

/// [`Entry::key_group`] when there's none.
const NO_GROUP: u32 = u32::MAX;

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

    /// Every file that has these notes, the newest first.
    pub fn sources(&self) -> impl Iterator<Item = &Arc<Source>> {
        std::iter::once(&self.phrase.source).chain(&self.other_sources)
    }

    /// Whether this song (as [`song_of`] gives it) has these notes, in any of its saves. Works each file's song out
    /// again: a search that leaves songs out goes by the catalogue's list of them instead.
    pub fn is_in_song(&self, song: &str) -> bool {
        self.sources().any(|s| song_of(&s.path) == song)
    }

    /// The newest of the same notes moved up or down in the same folder of MIDI files (the same
    /// [`StoredPhrase::shape`]: a pack's loop in every key), as a place in [`Catalogue::entries`]; it stands for them
    /// all in a list. None when there's no other. See [`Catalogue::new`] for what's grouped.
    pub fn key_group(&self) -> Option<usize> {
        (self.key_group != NO_GROUP).then_some(self.key_group as usize)
    }

    /// How many keys (or octaves) that set of notes is in.
    pub fn in_keys(&self) -> Option<InKeys> {
        let count = self.in_keys as usize;
        (count > 0).then_some(if self.in_octaves { InKeys::Octaves(count) } else { InKeys::Keys(count) })
    }

    /// The instrument it's for with how the app knows ("Keyscape is a piano plugin"), worked out now.
    pub fn instrument_why(&self) -> Option<InstrumentGuess> {
        instrument_of(&self.phrase)
    }
}

/// How many keys the same notes are in, moved up or down: 12 for a pack's loop in every key; or, when they're only
/// moved by octaves, how many octaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InKeys {
    Keys(usize),
    Octaves(usize),
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

/// The song each file is a save of, by where the file's record is in memory (with the record, so that place stays its).
type Songs = HashMap<usize, (Arc<Source>, Box<str>)>;

/// Every part, one entry per set of notes, newest first.
#[derive(Debug, Default)]
pub struct Catalogue {
    pub entries: Vec<Entry>,
    /// How many parts the index holds, copies counted.
    pub phrase_count: usize,
    /// How many lines a list of every part has: the same loop in other keys is one ([`Entry::key_group`]).
    pub line_count: usize,
    /// Which files' dates are real, as of when the catalogue was made: the entries' order, the dates the page shows
    /// and the years it counts all go by it, so they agree for as long as the catalogue lasts.
    pub dates: RealDates,
    /// The entries' places, in the order of their hashes: [`Catalogue::find`] looks a hash up in it, so no hash is
    /// kept a second time.
    by_hash: Vec<u32>,
    /// Every entry's search text (everything search looks in, in small letters), one after the other: one piece of
    /// memory, not one for each part.
    text: String,
    /// The search texts of the parts relabelled since ([`Catalogue::relabel`]), in the same way: an entry's place
    /// past the end of `text` is a place in here.
    relabelled: String,
    /// The song each file is a save of, by where the file's record is in memory, worked out the first time a search
    /// leaves songs out. Each record is held here too, so its place can't be given to another while this lasts.
    songs: OnceLock<Songs>,
}

impl Catalogue {
    /// The catalogue of `phrases`, which come newest file first (as [`crate::store::MidiStore::all`] and
    /// [`crate::MidiIndex::all`] give them), so the first of each set of notes is the newest copy. The phrases whose
    /// file's date isn't real go after the rest first (the store's list has them there already), keeping their order.
    pub fn new(mut phrases: Vec<StoredPhrase>) -> Catalogue {
        let dates = RealDates::now();
        let undated = |p: &StoredPhrase| !dates.seconds(p.source.mtime);
        if !phrases.is_sorted_by_key(undated) {
            phrases.sort_by_key(undated);
        }
        let phrase_count = phrases.len();
        // Which entry each phrase goes to: the first with its notes makes one, and the rest are its copies. Found
        // from the hashes where they are, before anything is moved.
        let mut seen: HashMap<&str, usize> = HashMap::with_capacity(phrases.len());
        let goes_to: Vec<usize> = phrases
            .iter()
            .map(|p| {
                let next = seen.len();
                *seen.entry(p.hash.as_str()).or_insert(next)
            })
            .collect();
        let count = seen.len();
        drop(seen);
        // The first of each set of notes stays where it is in the list, moved up over the copies (the list isn't
        // copied: a second list of every part would double what the reading holds at its height). A copy keeps only
        // its file; its names are held in the part itself, almost always, so letting it go here is quick.
        let mut others: Vec<Vec<Arc<Source>>> = vec![Vec::new(); count];
        let mut entry_of = goes_to.into_iter();
        let mut kept = 0;
        phrases.retain(|p| {
            let at = entry_of.next().unwrap_or(usize::MAX);
            if at == kept {
                kept += 1;
                return true;
            }
            if let Some(list) = others.get_mut(at) {
                list.push(Arc::clone(&p.source));
            }
            false
        });
        drop(entry_of);
        phrases.shrink_to_fit();
        Catalogue::of_grouped(Grouped { firsts: phrases, others }, dates, phrase_count)
    }

    /// The catalogue of parts already sorted into sets of the same notes ([`Grouped`], as
    /// [`crate::store::MidiStore::grouped`] reads them): the same catalogue [`Catalogue::new`] makes from every part,
    /// without the copies' rows ever being read in full.
    pub fn from_grouped(grouped: Grouped) -> Catalogue {
        let phrase_count = grouped.firsts.len() + grouped.others.iter().map(Vec::len).sum::<usize>();
        Catalogue::of_grouped(grouped, RealDates::now(), phrase_count)
    }

    fn of_grouped(Grouped { firsts, others }: Grouped, dates: RealDates, phrase_count: usize) -> Catalogue {
        // Names and instruments are worked out on every core: hundreds of thousands of parts.
        let mut entries: Vec<Entry> = firsts
            .into_par_iter()
            .zip(others)
            .map(|(phrase, other_sources)| {
                let instrument = instrument_of(&phrase).map(|i| i.family);
                let members = PhraseMember::decode(&phrase.members).into_boxed_slice();
                Entry {
                    phrase,
                    other_sources: other_sources.into_boxed_slice(),
                    instrument,
                    members,
                    within: Box::default(),
                    key_group: NO_GROUP,
                    in_keys: 0,
                    in_octaves: false,
                    text_at: 0,
                    text_len: 0,
                }
            })
            .collect();
        // The search texts go into one text, each entry keeping where its own is. Made a run of entries at a time on
        // every core, each run's into a text of its own, so no part's text is ever a piece of memory of its own: a
        // few hundred thousand small ones at once would stay the app's after they're let go (Windows keeps them).
        let runs: Vec<String> = entries
            .par_chunks_mut(TEXT_RUN)
            .map(|run| {
                let mut text = String::new();
                for e in run {
                    let words = search_text_of(&e.phrase, e.phrase.role(), e.instrument);
                    (e.text_at, e.text_len) = (text.len(), text_len(&words));
                    text.push_str(&words);
                }
                text
            })
            .collect();
        let mut text = String::with_capacity(runs.iter().map(String::len).sum());
        for (run, words) in entries.chunks_mut(TEXT_RUN).zip(&runs) {
            for e in run {
                e.text_at += text.len();
            }
            text.push_str(words);
        }
        drop(runs);
        // The places in the order of their hashes, sorted with each hash beside its place (the entries themselves
        // are big and far apart), and only the places kept.
        let mut hashed: Vec<(&str, u32)> = entries
            .iter()
            .enumerate()
            .filter_map(|(at, e)| Some((e.phrase.hash.as_str(), u32::try_from(at).ok()?)))
            .collect();
        hashed.par_sort_unstable();
        let by_hash: Vec<u32> = hashed.iter().map(|&(_, at)| at).collect();
        drop(hashed);
        // While the catalogue is made, the many look-ups of joined phrases' clips go by a table of the hashes where
        // they are, let go before the entries change.
        let places: HashMap<&str, usize> =
            entries.iter().enumerate().map(|(at, e)| (e.phrase.hash.as_str(), at)).collect();

        // Each clip learns which longer phrases it's part of.
        let mut within: Vec<(usize, usize)> = Vec::new();
        for (at, e) in entries.iter().enumerate().filter(|(_, e)| e.is_joined()) {
            let mut hashes: Vec<&str> = e.members.iter().map(|m| m.hash.as_str()).collect();
            hashes.sort_unstable();
            hashes.dedup();
            for hash in hashes {
                if let Some(&member) = places.get(hash).filter(|m| **m != at) {
                    within.push((member, at));
                }
            }
        }
        drop(places);
        // Each clip's phrases in one piece, in the order they were found (newest first).
        within.sort_by_key(|&(member, _)| member);
        for run in within.chunk_by(|a, b| a.0 == b.0) {
            if let Some(e) = run.first().and_then(|&(member, _)| entries.get_mut(member)) {
                e.within = run.iter().map(|&(_, phrase)| phrase).collect();
            }
        }
        // The same loop in other keys: a pack's files in one folder with the same shape. Only `.mid` files (a song
        // playing its riff on another root is another part of the song), only side by side in one folder (a short,
        // plain shape, such as a held chord, is in many places), never drums (a drum pattern moved is other drums).
        // Gathered by a number made from the folder's name rather than by the name itself, so no file's folder is
        // held while the gathering lasts; the few gathered together are told apart by the name itself after.
        let mut by_shape: HashMap<(u64, i64), Vec<usize>> = HashMap::new();
        for (at, e) in entries.iter().enumerate() {
            let p = &e.phrase;
            let in_keys = !e.is_project() && !p.drums && e.role() != Role::Drums;
            if let Some(shape) = p.shape.filter(|_| in_keys) {
                by_shape.entry((number_of(&folder_key(&p.source.path)), shape)).or_default().push(at);
            }
        }
        let folder_of = |at: usize| entries.get(at).map(|e| folder_key(&e.phrase.source.path)).unwrap_or_default();
        let by_shape: Vec<Vec<usize>> = by_shape
            .into_values()
            .filter(|g| g.len() > 1)
            .flat_map(|g| {
                // Almost always one folder; two folders whose names make the same number are split again here.
                let mut folders: Vec<(String, Vec<usize>)> = Vec::new();
                for at in g {
                    let folder = folder_of(at);
                    match folders.iter_mut().find(|(f, _)| *f == folder) {
                        Some((_, list)) => list.push(at),
                        None => folders.push((folder, vec![at])),
                    }
                }
                folders.into_iter().map(|(_, list)| list)
            })
            .collect();
        let mut line_count = entries.len();
        // One loop saved as several files: two tracks of one file (a tune doubled, a riff played again higher) are two
        // parts, so each group holds one part a file. A file's next track with the shape starts or joins the next
        // group: the "Lead" of each key's file together, and the "Lead high" of each together.
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for g in by_shape.into_iter().filter(|g| g.len() > 1) {
            let mut split: Vec<Vec<usize>> = Vec::new();
            for at in g {
                let Some(src) = entries.get(at).map(|e| &e.phrase.source) else { continue };
                let has = |s: &Vec<usize>| {
                    s.iter().any(|&o| entries.get(o).is_some_and(|e| Arc::ptr_eq(&e.phrase.source, src)))
                };
                match split.iter_mut().find(|s| !has(s)) {
                    Some(s) => s.push(at),
                    None => split.push(vec![at]),
                }
            }
            groups.extend(split.into_iter().filter(|s| s.len() > 1));
        }
        for group in groups {
            // In entries' order, newest first: the first stands for them.
            let mut keys: Vec<u8> =
                group.iter().filter_map(|&at| entries.get(at)).map(|e| e.phrase.lowest % 12).collect();
            keys.sort_unstable();
            keys.dedup();
            let (count, in_octaves) = if keys.len() > 1 { (keys.len(), false) } else { (group.len(), true) };
            line_count -= group.len() - 1;
            let first = group.first().and_then(|&at| u32::try_from(at).ok()).unwrap_or(NO_GROUP);
            for &at in &group {
                if let Some(e) = entries.get_mut(at) {
                    e.key_group = first;
                    e.in_keys = u32::try_from(count).unwrap_or(u32::MAX);
                    e.in_octaves = in_octaves;
                }
            }
        }
        let bars: Vec<i32> = entries.iter().map(|e| e.phrase.bars).collect();
        for e in &mut entries {
            // Longest first; equal lengths keep their order (newest first), as the C# stable sort does.
            e.within.sort_by_key(|&p| std::cmp::Reverse(bars.get(p).copied().unwrap_or(0)));
        }
        Catalogue {
            entries,
            phrase_count,
            line_count,
            dates,
            by_hash,
            text,
            relabelled: String::new(),
            songs: OnceLock::new(),
        }
    }

    /// The part with these notes. Entries are made again on every reload, so the page holds on to its picks by
    /// their notes and finds them again here.
    pub fn find(&self, hash: &str) -> Option<usize> {
        let hash_of = |at: &u32| self.entries.get(*at as usize).map_or("", |e| e.phrase.hash.as_str());
        let found = self.by_hash.binary_search_by(|at| hash_of(at).cmp(hash)).ok()?;
        self.by_hash.get(found).map(|&at| at as usize)
    }

    /// Whether every word (already in small letters, as [`words_of`] gives them) is in the names, folders, kind,
    /// instrument or key of `e`. For one of this catalogue's own entries that's a look in the text kept; for any
    /// other (one from a catalogue made before, say) its text is worked out again, which is slower but the same.
    pub fn matches(&self, e: &Entry, words: &[String]) -> bool {
        if !self.entries.as_ptr_range().contains(&std::ptr::from_ref(e)) {
            let text = search_text_of(&e.phrase, e.phrase.role(), e.instrument);
            return words.iter().all(|w| text.contains(w.as_str()));
        }
        let (from, at) = match e.text_at.checked_sub(self.text.len()) {
            Some(at) if e.text_len > 0 => (&self.relabelled, at),
            _ => (&self.text, e.text_at),
        };
        let text = from.get(at..at.saturating_add(e.text_len as usize)).unwrap_or("");
        words.iter().all(|w| text.contains(w.as_str()))
    }

    /// The song each file in the catalogue is a save of ([`song_of`]), by where the file's record is in memory.
    fn songs(&self) -> &Songs {
        self.songs.get_or_init(|| {
            // On this thread alone: work shared out from inside a once-only step could wait on a second search
            // that wants the same step, for good. It's once for each file, not each part, so it's quick.
            let mut songs = Songs::new();
            for s in self.entries.iter().flat_map(Entry::sources) {
                songs
                    .entry(Arc::as_ptr(s).addr())
                    .or_insert_with(|| (Arc::clone(s), song_of(&s.path).into_boxed_str()));
            }
            songs
        })
    }

    /// Whether the entry at `at` has a line of its own in a list: it isn't the same loop in another key as a newer one.
    pub fn is_line(&self, at: usize) -> bool {
        self.entries.get(at).is_some_and(|e| e.key_group().is_none_or(|g| g == at))
    }

    pub fn entry(&self, at: usize) -> Option<&Entry> {
        self.entries.get(at)
    }

    /// Changes what kind of part this is (`None`: back to the app's guess), for searching and the chips too. Only
    /// in memory: saving it is the index's job once the Rust app writes one.
    pub fn relabel(&mut self, at: usize, role: Option<Role>) {
        if let Some(e) = self.entries.get_mut(at) {
            e.phrase.your_role = role;
            // Its new text goes in the small text of relabelled parts (the one big text is never made bigger, which
            // would copy all of it); the old stays where it is, unused (a relabel is rare).
            let words = search_text_of(&e.phrase, e.phrase.role(), e.instrument);
            (e.text_at, e.text_len) = (self.text.len() + self.relabelled.len(), text_len(&words));
            self.relabelled.push_str(&words);
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
            let phrases = self.entries.get(part).map(|e| &e.within[..]).unwrap_or_default();
            let hits = phrases.iter().take(q.limit).map(|&entry| Hit { entry, likeness: None }).collect();
            return (hits, phrases.len());
        }
        let words = words_of(&q.text);
        // The songs to leave out, against each file's song: worked out once for every file, not for every part.
        let left = (!q.leave_songs.is_empty()).then(|| self.songs());
        let is_left = |s: &Arc<Source>| match left.and_then(|songs| songs.get(&Arc::as_ptr(s).addr())) {
            Some((_, song)) => q.leave_songs.iter().any(|l| l.as_str() == &**song),
            None => {
                let song = song_of(&s.path);
                q.leave_songs.contains(&song)
            }
        };
        let pool: Vec<usize> = self
            .entries
            .par_iter()
            .enumerate()
            .filter(|(at, e)| {
                // Cheapest first: working out a part's songs is the slowest test, so it comes last.
                q.role.is_none_or(|r| e.role() == r)
                    && q.from.is_none_or(|f| (f == From::Projects) == e.is_project())
                    && q.like != Some(*at)
                    && self.matches(e, &words)
                    && !(left.is_some() && e.sources().any(is_left))
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

/// The folder a file is in, in small letters: what tells one pack's folder of `.mid` files from another's when
/// the same loop in other keys is looked for.
pub(crate) fn folder_key(path: &str) -> String {
    case::to_lower(path.rfind(['\\', '/']).and_then(|cut| path.get(..cut)).unwrap_or(""))
}

/// A number for a text that's the same each time (`std`'s hasher, with its fixed keys): for gathering by it.
fn number_of(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// What's typed into a search, as the words [`Catalogue::matches`] takes: split at spaces, in small letters.
pub fn words_of(text: &str) -> Vec<String> {
    text.split(' ').filter(|w| !w.is_empty()).map(case::to_lower).collect()
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

/// A search text's length as an entry keeps it.
fn text_len(text: &str) -> u32 {
    u32::try_from(text.len()).unwrap_or(u32::MAX)
}

fn search_text_of(p: &StoredPhrase, role: Role, instrument: Option<Family>) -> String {
    let mut parts: Vec<&str> =
        vec![&p.part, &p.clip, file_stem(&p.source.path), role.name(), role.plural(), &p.source.daw];
    if let Some(family) = instrument {
        parts.push(family.search_words());
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
pub(crate) mod tests {
    use super::*;
    use crate::index::Key;

    pub(crate) fn source(path: &str, kind: SourceKind, mtime: i64) -> Arc<Source> {
        let daw = if kind == SourceKind::File { "MIDI" } else { "FL Studio" };
        Arc::new(Source { path: path.into(), kind, daw: daw.into(), mtime, size: 1, tempo: None })
    }

    pub(crate) fn phrase(id: i64, src: &Arc<Source>, part: &str, clip: &str, role: Role, hash: &str) -> StoredPhrase {
        StoredPhrase {
            id,
            source: Arc::clone(src),
            part: part.into(),
            clip: clip.into(),
            role,
            role_why: crate::roles::RoleWhy::NONE,
            your_role: None,
            bars: 4,
            notes: 8,
            lowest: 40,
            highest: 60,
            key: None,
            key_fact: None,
            chroma: [0; 12],
            chords: Vec::new(),
            features: [0; 48],
            hash: hash.into(),
            members: Default::default(),
            program: None,
            drums: false,
            track_name: Default::default(),
            your_octave: None,
            shape: None,
            cut: None,
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
    fn the_same_loop_in_other_keys_is_grouped_only_across_files() {
        let a = source(r"L:\Pack\Loop Am.mid", SourceKind::File, 300);
        let b = source(r"L:\Pack\Loop Dm.mid", SourceKind::File, 200);
        let shaped = |p: StoredPhrase, lowest: u8| StoredPhrase { shape: Some(7), lowest, ..p };
        let cat = Catalogue::new(vec![
            // One file with the tune doubled an octave up on a second track: two parts.
            shaped(phrase(1, &a, "Lead", "Loop Am", Role::Melody, "A1"), 57),
            shaped(phrase(2, &a, "Lead high", "Loop Am", Role::Melody, "A2"), 69),
            // The same loop in another file, in D minor: grouped with the first.
            shaped(phrase(3, &b, "Lead", "Loop Dm", Role::Melody, "D1"), 62),
        ]);
        assert_eq!(cat.entries[0].key_group(), Some(0));
        assert_eq!(cat.entries[1].key_group(), None, "a second track of the same file stays a line");
        assert_eq!(cat.entries[2].key_group(), Some(0));
        assert_eq!(cat.entries[0].in_keys(), Some(InKeys::Keys(2)));
        assert_eq!(cat.line_count, 2);
        // Both files with both tracks: two lines, each track's copies together.
        let cat = Catalogue::new(vec![
            shaped(phrase(1, &a, "Lead", "Loop Am", Role::Melody, "A1"), 57),
            shaped(phrase(2, &a, "Lead high", "Loop Am", Role::Melody, "A2"), 69),
            shaped(phrase(3, &b, "Lead", "Loop Dm", Role::Melody, "D1"), 62),
            shaped(phrase(4, &b, "Lead high", "Loop Dm", Role::Melody, "D2"), 74),
        ]);
        assert_eq!(cat.line_count, 2);
        assert_eq!((cat.entries[1].key_group(), cat.entries[3].key_group()), (Some(1), Some(1)));
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
    fn a_copy_with_a_real_date_stands_for_its_notes() {
        // As the C# app's index lists them, newest date first: a pack's copy dated 2099 (a date still to come, as one
        // of Solomon's says 31 Dec 2027), then the song saved in 2024 with the same notes, then a file from 2019.
        let pack = source(r"L:\New Midi\Pack\16BTFILL.mid", SourceKind::File, 4_102_401_600);
        let song = source(r"C:\Projects\Didums\Didums_70.flp", SourceKind::Project, 1_709_294_400);
        let old = source(r"L:\New Midi\Old.mid", SourceKind::File, 1_559_390_400);
        let c = Catalogue::new(vec![
            phrase(1, &pack, "", "16BTFILL", Role::Drums, "A"),
            phrase(2, &pack, "", "16BTFILL", Role::Drums, "B"),
            phrase(3, &song, "Drums", "Fill", Role::Drums, "A"),
            phrase(4, &old, "", "Old", Role::Melody, "C"),
        ]);
        let ids: Vec<i64> = c.entries.iter().map(|e| e.phrase.id).collect();
        assert_eq!(ids, [3, 4, 2], "the real dates first, newest first, then the rest");
        let fill = &c.entries[c.find("A").unwrap()];
        assert_eq!(fill.other_sources.iter().map(|s| s.mtime).collect::<Vec<_>>(), [4_102_401_600]);
        assert!(c.dates.seconds(1_709_294_400) && !c.dates.seconds(4_102_401_600));
    }

    #[test]
    fn clips_know_the_phrases_they_are_in() {
        let c = sample();
        let joined = c.find("J").unwrap();
        assert_eq!(c.entries[c.find("A").unwrap()].within[..], [joined]);
        assert_eq!(c.entries[c.find("B").unwrap()].within[..], [joined]);
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
        assert_eq!(c.search(&Query { text: "trap keys".into(), ..Query::default() }).1, 1, "its names still match");
        // A second part relabelled: each reads its own words from the relabelled text.
        let bass = c.find("B").unwrap();
        c.relabel(bass, Some(Role::Chords));
        assert_eq!(c.search(&Query { text: "arps".into(), ..Query::default() }).1, 1);
        assert_eq!(c.search(&Query { text: "bass hook progression".into(), ..Query::default() }).1, 1);
        assert_eq!(c.search(&Query { text: "bassline".into(), ..Query::default() }).1, 0);
        c.relabel(keys, None);
        assert_eq!(c.entries[keys].role(), Role::Melody);
        assert_eq!(c.search(&Query { text: "arps".into(), ..Query::default() }).1, 0, "its old words are gone");
    }

    #[test]
    fn every_part_is_found_by_its_hash_and_matches_its_own_words() {
        let pack = source(r"D:\Packs\Loops\Many.mid", SourceKind::File, 100);
        // Hashes out of order, as they come: the newest file first, not the smallest hash.
        let hashes: Vec<String> = (0..300u32).map(|n| format!("{:08X}", n.wrapping_mul(2_654_435_761))).collect();
        let phrases =
            hashes.iter().enumerate().map(|(n, h)| phrase(n as i64, &pack, "", &format!("Loop{n}x"), Role::Melody, h));
        let c = Catalogue::new(phrases.collect());
        for (n, h) in hashes.iter().enumerate() {
            let at = c.find(h).expect("found");
            assert_eq!(c.entries[at].phrase.id, n as i64);
            // Its own words and nobody else's: each entry reads its own stretch of the one text.
            assert!(c.matches(&c.entries[at], &words_of(&format!("LOOP{n}x many"))));
            assert!(!c.matches(&c.entries[at], &words_of(&format!("loop{}x", n + 1))));
        }
        // A part of another catalogue (one made before a reload, say) is matched by its own words too.
        let other = Catalogue::new(vec![phrase(1, &pack, "", "Elsewhere", Role::Bass, "Z")]);
        assert!(c.matches(&other.entries[0], &words_of("elsewhere bassline")));
        assert!(!c.matches(&other.entries[0], &words_of("loop0x")));
        assert_eq!(c.find("no such hash"), None);
        assert_eq!(c.find(""), None);
        assert_eq!(Catalogue::default().find("A"), None);
    }

    #[test]
    fn leaving_songs_out_is_the_same_the_second_time() {
        let c = sample();
        let q = Query { leave_songs: vec![song_of(r"C:\Projects\Didums\Didums_1.flp")], ..Query::default() };
        let first = c.search(&q);
        assert_eq!(titles(&c, &first.0), ["Dark keys 01"]);
        assert_eq!(c.search(&q), first, "from the songs kept the first time");
        // A song only an older copy is in still leaves the part out.
        let old = source(r"C:\Projects\Other\Other_1.flp", SourceKind::Project, 50);
        let new = source(r"C:\Projects\Didums\Didums_70.flp", SourceKind::Project, 300);
        let c = Catalogue::new(vec![
            phrase(1, &new, "Piano", "Verse", Role::Chords, "A"),
            phrase(2, &old, "Piano", "Verse", Role::Chords, "A"),
        ]);
        let other = Query { leave_songs: vec![song_of(r"C:\Projects\Other\Other_9.flp")], ..Query::default() };
        assert_eq!(c.search(&other).1, 0);
        let neither = Query { leave_songs: vec![song_of(r"C:\Projects\Else\Else_9.flp")], ..Query::default() };
        assert_eq!(c.search(&neither).1, 1);
    }

    #[test]
    fn an_entry_is_no_bigger_than_it_was_made() {
        // Hundreds of thousands are held at once: a field added here costs that many times its size.
        assert!(std::mem::size_of::<Entry>() <= 360, "{}", std::mem::size_of::<Entry>());
    }

    #[test]
    fn more_like_this_ranks_by_shape() {
        let mut c = sample();
        let shape = |v: &[u8]| -> Vec<u8> { v.iter().copied().chain(std::iter::repeat(0)).take(48).collect() };
        let (a, b, k) = (c.find("A").unwrap(), c.find("B").unwrap(), c.find("C").unwrap());
        c.entries[a].phrase.features = crate::features_of(&shape(&[255, 200, 10]));
        c.entries[b].phrase.features = crate::features_of(&shape(&[0, 0, 255]));
        c.entries[k].phrase.features = crate::features_of(&shape(&[250, 190, 20]));
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
