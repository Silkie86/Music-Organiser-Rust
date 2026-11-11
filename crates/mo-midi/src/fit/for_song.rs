//! The parts of the library that fit the song being worked on: what the MIDI page asks for.
//!
//! A search ([`Target::search`]) knows candidates and their places; the page has a [`Catalogue`] and a saved song.
//! This goes between the two:
//!
//! 1. [`Pick::usual`]: what of the song is fitted to when nothing is picked by hand (its chords and bass for a tune,
//!    its melodies for chords, over the bars where the most of them play together).
//! 2. [`Own::of`]: the song's own parts in the library, which are left out. They'd fit best of all and say nothing.
//! 3. [`tried`]: the parts to try, as the page's filters let them through.
//! 4. [`Tried::search`]: the search, its answer as places in the catalogue.
//!
//! Reading the song ([`crate::playing::read`]) and the index's slots ([`crate::store::MidiStore::slots`]) is the
//! caller's, off the window's thread, and so is keeping them between searches.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::atomic::AtomicBool;

use mo_formats::NoteEvent;
use rayon::prelude::*;

use super::{Candidate, Search, Target, Wants};
use crate::catalogue::{Catalogue, Entry, song_of};
use crate::playing::Song;
use crate::store::SlotBook;

/// The most bars of a song fitted to when none are picked: a loop's length, twice over.
pub const USUAL_BARS: i64 = 8;

/// What of a song is fitted to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    /// The parts, as places in [`Song::parts`].
    pub parts: Vec<usize>,
    /// The bars, counted from 0.
    pub bars: Range<i64>,
}

impl Pick {
    /// What's fitted to when nothing is picked by hand. Looking for a tune: the song's chords, arps and basslines
    /// ([`Song::harmony_parts`]). Looking for chords: its melodies ([`Song::melody_parts`]). Over up to
    /// [`USUAL_BARS`] bars from the first bar where the most of them play at once ([`Song::busiest_bars`]), and of
    /// those parts only the ones that play in those bars, so the pick names what's heard. None when the song has no
    /// such part.
    pub fn usual(song: &Song, wants: Wants) -> Option<Pick> {
        let mut parts = match wants {
            Wants::Tune => song.harmony_parts(),
            Wants::Chords => song.melody_parts(),
        };
        let bars = song.busiest_bars(&parts, USUAL_BARS)?;
        parts.retain(|&part| song.plays_in(part, &bars));
        Some(Pick { parts, bars })
    }

    /// What a part from the library is fitted to ([`Target::of_song`]): a tune to find chords for when every picked
    /// part is a melody, else chords to find a tune for. None when the picked parts play nothing pitched in the bars.
    pub fn target(&self, song: &Song) -> Option<Target> {
        Target::of_song(song, &self.parts, &self.bars)
    }
}

/// A song's own parts in the library: what a search for parts that fit it leaves out.
///
/// The library has every part of the song's saves, and they'd come first in any list (nothing sits with the song's
/// chords like the song's own tune). The same notes in other keys are its own too: a pack's loop the song uses is in
/// the library in every key the pack has it in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Own {
    /// Places in [`Catalogue::entries`] of the parts in one of the song's saves.
    places: HashSet<usize>,
    /// Their notes' shapes in any key ([`crate::index::StoredPhrase::shape`]).
    shapes: HashSet<i64>,
}

impl Own {
    /// The own parts of the song `save` is a save of (its path: every save of the song counts, as
    /// [`song_of`] tells them). One pass over the catalogue on rayon's threads; keep it for as long as the catalogue
    /// and the song stay. Nothing is kept on the entries for it ([`Entry::is_in_song`] keeps each part's songs, which
    /// one pass has no use for).
    pub fn of(cat: &Catalogue, save: &str) -> Own {
        let song = song_of(save);
        let in_song = |e: &Entry| {
            std::iter::once(&e.phrase.source).chain(&e.other_sources).any(|source| song_of(&source.path) == song)
        };
        let own: Vec<(usize, Option<i64>)> = cat
            .entries
            .par_iter()
            .enumerate()
            .filter(|(_, e)| in_song(e))
            .map(|(at, e)| (at, e.phrase.shape))
            .collect();
        Own { places: own.iter().map(|(at, _)| *at).collect(), shapes: own.iter().filter_map(|(_, s)| *s).collect() }
    }

    /// Whether the part at `at` is one of the song's own, or the same notes in another key.
    pub fn has(&self, at: usize, e: &Entry) -> bool {
        self.places.contains(&at) || e.phrase.shape.is_some_and(|shape| self.shapes.contains(&shape))
    }

    /// How many of the catalogue's parts are in one of the song's saves.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }
}

/// Parts of a catalogue for a search to try.
#[derive(Clone, Debug)]
pub struct Tried<'a> {
    cat: &'a Catalogue,
    /// Their places in [`Catalogue::entries`], in the catalogue's order.
    places: Vec<usize>,
}

/// The parts of `cat` to try against a song: every part `keep` lets through (the page's filters: a part the page
/// hides shouldn't come up), without the song's `own`. Every copy of a loop in other keys is among them, so that the
/// search can give the copy that needs moving least. One pass on rayon's threads.
pub fn tried<'a>(cat: &'a Catalogue, own: &Own, keep: impl Fn(&Entry) -> bool + Sync) -> Tried<'a> {
    let places = cat
        .entries
        .par_iter()
        .enumerate()
        // The cheap test first.
        .filter(|(at, e)| !own.has(*at, e) && keep(e))
        .map(|(at, _)| at)
        .collect();
    Tried { cat, places }
}

impl Tried<'_> {
    /// How many parts there are to try.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// The search ([`Target::search`]) of these parts for the ones that go best with `target`, `most` at the most.
    /// In the answer [`super::Found::at`] is a place in [`Catalogue::entries`]. `book` has the index's slots and
    /// `notes` reads one part's notes from it; `stop` set ends it early. Call it off the window's thread.
    pub fn search(
        &self,
        target: &Target,
        book: &SlotBook,
        notes: &mut dyn FnMut(i64, &str) -> Option<Vec<NoteEvent>>,
        most: usize,
        pool: &rayon::ThreadPool,
        stop: &AtomicBool,
    ) -> Search {
        // A place the catalogue doesn't have (it never gives one) is left out of both lists alike.
        let (places, parts): (Vec<usize>, Vec<Candidate>) =
            self.places.iter().filter_map(|&at| Some((at, Candidate::of(&self.cat.entry(at)?.phrase)))).unzip();
        let mut search = target.search(&parts, book, notes, most, pool, stop);
        search.found.retain_mut(|found| match places.get(found.at) {
            Some(&at) => {
                found.at = at;
                true
            }
            None => false,
        });
        search
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::super::tests::{bass, chords, lane, song, tune, wandering};
    use super::*;
    use crate::fit::slots_of;
    use crate::index::{Source, SourceKind, StoredPhrase};
    use crate::roles::Role;
    use crate::{analyser, playing};
    use mo_formats::arrangement::Arrangement;

    fn source(path: &str, kind: SourceKind, mtime: i64) -> Arc<Source> {
        let daw = if kind == SourceKind::File { "MIDI" } else { "FL Studio" };
        Arc::new(Source { path: path.into(), kind, daw: daw.into(), mtime, size: 1, tempo: None })
    }

    /// A made-up index: the catalogue of its parts, their slots and their notes.
    #[derive(Default)]
    struct Index {
        phrases: Vec<StoredPhrase>,
        book: SlotBook,
        notes: HashMap<String, Vec<NoteEvent>>,
    }

    impl Index {
        /// Adds a part with these notes, from this file. Its note hash is `hash`.
        fn add(&mut self, hash: &str, from: &Arc<Source>, role: Role, notes: Vec<NoteEvent>) {
            self.phrases.push(StoredPhrase {
                id: self.phrases.len() as i64,
                source: Arc::clone(from),
                part: String::new(),
                clip: hash.into(),
                role,
                role_why: String::new(),
                your_role: None,
                bars: 4,
                notes: notes.len() as i32,
                lowest: notes.iter().map(|n| n.key).min().unwrap_or(0),
                highest: notes.iter().map(|n| n.key).max().unwrap_or(0),
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
                shape: analyser::shape_of(&notes),
                cut: Some(false),
            });
            self.book.add(hash, &slots_of(&notes));
            self.notes.insert(hash.into(), notes);
        }

        fn catalogue(&self) -> Catalogue {
            Catalogue::new(self.phrases.clone())
        }

        /// A search of `tried`, with the hashes found in order and each one's best move.
        fn search(&self, tried: &Tried, target: &Target, most: usize) -> (Search, Vec<(String, i32)>) {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
            let mut notes = |_: i64, hash: &str| self.notes.get(hash).cloned();
            let search = tried.search(target, &self.book, &mut notes, most, &pool, &AtomicBool::new(false));
            let found =
                search.found.iter().map(|f| (tried.cat.entries[f.at].phrase.hash.clone(), f.sits.best().0)).collect();
            (search, found)
        }
    }

    fn arrangement(lanes: Vec<mo_formats::arrangement::PreviewLane>) -> Arrangement {
        Arrangement { view: "Test", tempo: 120.0, beats_per_bar: 4, lanes, markers: Vec::new(), notes: Vec::new() }
    }

    fn up(notes: &[NoteEvent], semitones: u8) -> Vec<NoteEvent> {
        notes.iter().map(|n| NoteEvent { key: n.key + semitones, ..*n }).collect()
    }

    /// The song of the tests' saves (its chords, bass and lead are in the library from its two saves), a pack with
    /// the song's tune in three keys and a tune of its own in two, and a second song with a tune that fits.
    fn library() -> Index {
        let new_save = source(r"C:\Projects\Didums\Didums_70.flp", SourceKind::Project, 300);
        let old_save = source(r"C:\Projects\Didums\Didums_69.flp", SourceKind::Project, 200);
        let other_song = source(r"C:\Projects\Other\Other_3.flp", SourceKind::Project, 250);
        let pack = source(r"D:\Packs\Leads\Lead 01.mid", SourceKind::File, 100);
        // A file saved after the song has its bassline too: the newest copy of those notes isn't the song's.
        let exported = source(r"D:\Exports\Sub.mid", SourceKind::File, 400);
        let mut index = Index::default();
        index.add("song bass", &exported, Role::Bass, bass());
        index.add("song chords", &new_save, Role::Chords, chords(0));
        index.add("song bass", &new_save, Role::Bass, bass());
        index.add("song lead", &new_save, Role::Melody, tune(0));
        index.add("song lead", &old_save, Role::Melody, tune(0));
        index.add("old lead", &old_save, Role::Melody, wandering(3));
        // The pack the song's lead came from has it in other keys too.
        index.add("pack lead in F", &pack, Role::Melody, up(&tune(0), 5));
        index.add("pack lead in D", &pack, Role::Melody, up(&tune(0), 2));
        // A tune of the pack's own that sits with the chords: the song's an octave down, without its last two
        // notes (so it's another loop, not the song's in another key).
        let mut own_tune = tune(0);
        own_tune.iter_mut().for_each(|n| n.key -= 12);
        own_tune.truncate(10);
        index.add("pack tune", &pack, Role::Melody, own_tune.clone());
        index.add("pack tune in Eb", &pack, Role::Melody, up(&own_tune, 3));
        index.add("other song's tune", &other_song, Role::Melody, up(&own_tune, 12));
        for seed in 0..6 {
            index.add(&format!("wandering {seed}"), &pack, Role::Melody, wandering(seed + 10));
        }
        index
    }

    #[test]
    fn nothing_picked_a_tune_is_fitted_to_the_chords_and_bass_where_they_play_together() {
        let song = song();
        // The chords and the bass come in at bar 9 (counted from 0: 8) and run 8 and 4 bars.
        let pick = Pick::usual(&song, Wants::Tune).unwrap();
        assert_eq!(pick, Pick { parts: vec![0, 1], bars: 8..16 });
        assert_eq!(pick.target(&song).unwrap().wants(), Wants::Tune);
        // Chords are fitted to the lead, where it plays.
        let pick = Pick::usual(&song, Wants::Chords).unwrap();
        assert_eq!(pick, Pick { parts: vec![2], bars: 12..16 });
        assert_eq!(pick.target(&song).unwrap().wants(), Wants::Chords);
        // A part that only plays somewhere else in the song isn't one of the picked: a pad that comes in later.
        let mut later = song.clone();
        later.parts.push(playing::of(&arrangement(vec![lane("Pad", 40, &chords(0))])).parts.remove(0));
        assert_eq!(later.harmony_parts(), [0, 1, 4]);
        assert_eq!(Pick::usual(&later, Wants::Tune).unwrap(), Pick { parts: vec![0, 1], bars: 8..16 });
        // A song with no melody has nothing to fit chords to, and one with drums alone nothing at all.
        let no_tune = playing::of(&arrangement(vec![lane("Keys", 0, &chords(0)), lane("FPC", 0, &bass())]));
        assert!(Pick::usual(&no_tune, Wants::Chords).is_none());
        assert_eq!(Pick::usual(&no_tune, Wants::Tune).unwrap(), Pick { parts: vec![0], bars: 0..4 });
        // Parts picked by hand that play nothing in the bars picked give nothing to fit to.
        assert!(Pick { parts: vec![0], bars: 0..4 }.target(&song).is_none());
    }

    #[test]
    fn the_songs_own_parts_are_left_out_in_every_key_and_every_save() {
        let index = library();
        let cat = index.catalogue();
        // Any save of the song names it.
        let own = Own::of(&cat, r"C:\Projects\Didums\Didums_12.flp");
        let has = |hash: &str| {
            let at = cat.find(hash).unwrap();
            own.has(at, &cat.entries[at])
        };
        // Its parts from the newest save and from an older one, and one whose newest copy is in another file.
        assert!(has("song chords") && has("song bass") && has("song lead") && has("old lead"));
        assert!(!cat.entries[cat.find("song bass").unwrap()].is_project());
        assert_eq!(own.len(), 4);
        // The pack's copies of its lead in other keys are the same loop.
        assert!(has("pack lead in F") && has("pack lead in D"));
        // The rest of the pack and the other song aren't.
        assert!(!has("pack tune") && !has("pack tune in Eb") && !has("other song's tune") && !has("wandering 0"));
        // Another song's own parts are its own.
        let other = Own::of(&cat, r"C:\Projects\Other\Other_1.flp");
        assert_eq!(other.len(), 1);
        // "other song's tune" and the pack's are the same notes octaves apart: the same loop.
        let at = cat.find("pack tune").unwrap();
        assert!(other.has(at, &cat.entries[at]));
        // A song the library has nothing of leaves nothing out.
        assert!(Own::of(&cat, r"C:\Projects\New\New_1.flp").is_empty());
    }

    #[test]
    fn a_search_gives_the_parts_that_fit_as_places_in_the_catalogue() {
        let index = library();
        let cat = index.catalogue();
        let song = song();
        let target = Pick::usual(&song, Wants::Tune).unwrap().target(&song).unwrap();
        let own = Own::of(&cat, r"C:\Projects\Didums\Didums_70.flp");

        let tried = tried(&cat, &own, |_| true);
        // 15 sets of notes, less the song's four and the pack's two copies of its lead.
        assert_eq!((cat.entries.len(), tried.len()), (15, 9));
        let (search, found) = index.search(&tried, &target, 3);
        assert_eq!((search.looked, search.not_ready, search.unread, search.stopped), (9, 0, 0, false));
        // The pack's tune comes first, as the copy that fits as it is (not the one in Eb, nor the other song's an
        // octave up, which the list has once); none of the song's own parts is there.
        assert_eq!(found[0], ("pack tune".to_owned(), 0));
        assert!(found.iter().all(|(hash, _)| hash.starts_with("wandering") || hash == "pack tune"), "{found:?}");
        assert_eq!(found.len(), 3);

        // What the page's filters hide isn't tried: without the pack's files only the other song's tune is left,
        // and it's found with nothing moved (octaves don't count).
        let from_projects = super::tried(&cat, &own, |e| e.is_project());
        assert_eq!(from_projects.len(), 1);
        let (_, found) = index.search(&from_projects, &target, 3);
        assert_eq!(found, [("other song's tune".to_owned(), 0)]);

        // Without the song's own parts left out they'd be what's found.
        let nothing_left_out = super::tried(&cat, &Own::default(), |_| true);
        assert_eq!(nothing_left_out.len(), 15);
        let (_, found) = index.search(&nothing_left_out, &target, 1);
        assert!(found[0].0.starts_with("song"), "{found:?}");
    }

    #[test]
    fn nothing_to_try_finds_nothing() {
        let index = library();
        let cat = index.catalogue();
        let song = song();
        let target = Pick::usual(&song, Wants::Tune).unwrap().target(&song).unwrap();
        let none = tried(&cat, &Own::default(), |_| false);
        assert!(none.is_empty());
        let (search, found) = index.search(&none, &target, 10);
        assert!(found.is_empty());
        assert_eq!((search.looked, search.close), (0, 0));
    }
}
