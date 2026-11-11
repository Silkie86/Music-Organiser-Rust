//! The parts of the library that fit the song being worked on: what the MIDI page asks for.
//!
//! A search ([`Target::search`]) knows candidates and their places; the page has a [`Catalogue`] and a saved song.
//! This goes between the two:
//!
//! 1. [`Pick::usual`]: what of the song is fitted to when nothing is picked by hand (its chords and bass for a tune,
//!    its melodies for chords, over the bars where the most of them play together).
//! 2. [`Own::of`]: the song's own parts in the library (every save's, and a pack's loop it uses in the pack's other
//!    keys), which are left out. They'd fit best of all and say nothing.
//! 3. [`tried`]: the parts to try, as the page's filters let them through.
//! 4. [`Tried::search`]: the search, its answer as places in the catalogue.
//!
//! Reading the song ([`crate::playing::read`]) and the index's slots ([`crate::store::MidiStore::slots`]) is the
//! caller's, off the window's thread, and so is keeping them between searches.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mo_formats::{NoteEvent, case};
use rayon::prelude::*;

use super::{Candidate, Search, Target, Wants};
use crate::catalogue::{Catalogue, Entry, folder_key};
use crate::index::{Source, SourceKind};
use crate::playing::Song;
use crate::roles::Role;
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
/// chords like the song's own tune). A pack's loop the song uses is its own in the pack's other keys too: the same
/// shape in the same folder of `.mid` files, never drums, which is what the catalogue takes for one loop in several
/// keys ([`Entry::key_group`]). The same shape anywhere else is not: a plain shape (a held note, a bassline on one
/// pitch) is in many places, and those are parts worth offering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Own {
    /// Places in [`Catalogue::entries`] of the parts in one of the song's saves.
    in_song: HashSet<usize>,
    /// Places of the same loops in other keys.
    other_keys: HashSet<usize>,
}

impl Own {
    /// The own parts of the song whose saves are at `saves`: every save's path, as the song library groups them
    /// (`mo_library::Song::saves` and its `copies`; that grouping knows the saves you joined or split by hand, which
    /// a file's name alone doesn't). Two passes over the catalogue, the second on rayon's threads; keep it for as
    /// long as the catalogue and the song's saves stay.
    pub fn of<'a>(cat: &Catalogue, saves: impl IntoIterator<Item = &'a str>) -> Own {
        let saves: HashSet<String> = saves.into_iter().map(case::to_lower).collect();
        if saves.is_empty() {
            return Own::default();
        }
        // Each file is asked about once: every part of a file shares its source.
        let mut asked: HashMap<*const Source, bool> = HashMap::new();
        let mut is_save = |source: &Arc<Source>| {
            *asked.entry(Arc::as_ptr(source)).or_insert_with(|| saves.contains(&case::to_lower(&source.path)))
        };
        let mut in_song = HashSet::new();
        // The loops among them that a folder of `.mid` files has: that folder, and the loop's shape.
        let mut loops: HashSet<(String, i64)> = HashSet::new();
        for (at, e) in cat.entries.iter().enumerate() {
            if !sources(e).any(&mut is_save) {
                continue;
            }
            in_song.insert(at);
            if let Some(shape) = loop_shape(e) {
                loops.extend(files(e).map(|file| (folder_key(&file.path), shape)));
            }
        }
        // The same loops in other keys: the shape first, which few parts share, then the folder.
        let shapes: HashSet<i64> = loops.iter().map(|(_, shape)| *shape).collect();
        let other_keys = if loops.is_empty() {
            HashSet::new()
        } else {
            cat.entries
                .par_iter()
                .enumerate()
                .filter(|(at, e)| {
                    !in_song.contains(at)
                        && loop_shape(e)
                            .filter(|shape| shapes.contains(shape))
                            .is_some_and(|shape| files(e).any(|file| loops.contains(&(folder_key(&file.path), shape))))
                })
                .map(|(at, _)| at)
                .collect()
        };
        Own { in_song, other_keys }
    }

    /// Whether the part at `at` (a place in [`Catalogue::entries`]) is one of the song's own, or one of its loops in
    /// another key.
    pub fn has(&self, at: usize) -> bool {
        self.in_song.contains(&at) || self.other_keys.contains(&at)
    }

    /// How many of the catalogue's parts are in one of the song's saves.
    pub fn len(&self) -> usize {
        self.in_song.len()
    }

    pub fn is_empty(&self) -> bool {
        self.in_song.is_empty()
    }
}

/// Every file a part's notes are in, the newest first.
fn sources(e: &Entry) -> impl Iterator<Item = &Arc<Source>> {
    std::iter::once(&e.phrase.source).chain(&e.other_sources)
}

/// The `.mid` files among them.
fn files(e: &Entry) -> impl Iterator<Item = &Arc<Source>> {
    sources(e).filter(|source| source.kind == SourceKind::File)
}

/// A part's shape, when it can be a loop a pack has in several keys: never drums (a drum pattern moved is other
/// drums).
fn loop_shape(e: &Entry) -> Option<i64> {
    e.phrase.shape.filter(|_| !e.phrase.drums && e.role() != Role::Drums)
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
        .filter(|(at, e)| !own.has(*at) && keep(e))
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
    use super::super::tests::{bass, chords, lane, note, song, tune, wandering};
    use super::*;
    use crate::catalogue::tests::{phrase, source};
    use crate::fit::slots_of;
    use crate::index::StoredPhrase;
    use crate::{PPQ, analyser, playing};
    use mo_formats::arrangement::Arrangement;

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
                notes: notes.len() as i32,
                lowest: notes.iter().map(|n| n.key).min().unwrap_or(0),
                highest: notes.iter().map(|n| n.key).max().unwrap_or(0),
                drums: role == Role::Drums,
                shape: analyser::shape_of(&notes),
                cut: Some(false),
                ..phrase(self.phrases.len() as i64, from, "", hash, role, hash)
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

    /// A bar of sixteenths on one key: a hi-hat row, or a bassline that stays on its note.
    fn sixteenths(key: u8) -> Vec<NoteEvent> {
        (0..16).map(|at| note(at * PPQ / 4, PPQ / 4, key)).collect()
    }

    /// The song's saves, as the song library has them: two by its name, and one joined to it by hand.
    const SAVES: [&str; 3] = [
        r"C:\Projects\Didums\Didums_70.flp",
        r"C:\Projects\Didums\Didums_69.flp",
        r"C:\Projects\Old ideas\Riff final.flp",
    ];

    /// The song's parts from its three saves. The pack its lead came from, with the lead in two more keys. A folder
    /// of exports with the song's bassline (saved after the song), the same in another key, its hi-hats and a
    /// bassline of the hi-hats' shape. And what isn't the song's: the bassline's shape in another folder, a pack's
    /// tune in two keys, another song with that tune, and tunes that go nowhere.
    fn library() -> Index {
        let [new_save, old_save, joined_save] = SAVES.map(|path| source(path, SourceKind::Project, 300));
        let other_song = source(r"C:\Projects\Other\Other_3.flp", SourceKind::Project, 250);
        let file = |path: &str, mtime: i64| source(path, SourceKind::File, mtime);
        let mut index = Index::default();
        // The newest copy of the song's bassline isn't the song's: the export was saved after it.
        index.add("song bass", &file(r"D:\Exports\Sub.mid", 400), Role::Bass, bass());
        index.add("song chords", &new_save, Role::Chords, chords(0));
        index.add("song bass", &new_save, Role::Bass, bass());
        index.add("song lead", &new_save, Role::Melody, tune(0));
        index.add("song hats", &new_save, Role::Drums, sixteenths(42));
        index.add("song lead", &old_save, Role::Melody, tune(0));
        index.add("old lead", &old_save, Role::Melody, wandering(3));
        index.add("joined riff", &joined_save, Role::Melody, wandering(4));
        // The pack the lead came from.
        index.add("song lead", &file(r"D:\Packs\Leads\Lead 01 C.mid", 100), Role::Melody, tune(0));
        index.add("pack lead in F", &file(r"D:\Packs\Leads\Lead 01 F.mid", 100), Role::Melody, up(&tune(0), 5));
        index.add("pack lead in D", &file(r"D:\Packs\Leads\Lead 01 D.mid", 100), Role::Melody, up(&tune(0), 2));
        // Beside the exported bassline.
        index.add("sub in D", &file(r"D:\Exports\Sub in D.mid", 100), Role::Bass, up(&bass(), 2));
        index.add("song hats", &file(r"D:\Exports\Hats.mid", 100), Role::Drums, sixteenths(42));
        index.add("pedal bass", &file(r"D:\Exports\Pedal.mid", 100), Role::Bass, sixteenths(33));
        // Not the song's.
        index.add("bass elsewhere", &file(r"D:\Packs\Bass\Roots.mid", 100), Role::Bass, up(&bass(), 7));
        // Another project in the song's folder that plays its lead a minor third up: a riff played on another root
        // in a project is another part, not a pack's loop in another key.
        let sketch = source(r"C:\Projects\Didums\Sketch.flp", SourceKind::Project, 100);
        index.add("sketch riff", &sketch, Role::Melody, up(&tune(0), 3));
        // A tune that sits with the chords: the song's an octave down, without its last two notes (so it's another
        // loop, not the song's in another key).
        let mut own_tune = tune(0);
        own_tune.iter_mut().for_each(|n| n.key -= 12);
        own_tune.truncate(10);
        index.add("pack tune", &file(r"D:\Packs\Tunes\Tune A.mid", 100), Role::Melody, own_tune.clone());
        index.add("pack tune in Eb", &file(r"D:\Packs\Tunes\Tune Eb.mid", 100), Role::Melody, up(&own_tune, 3));
        index.add("other song's tune", &other_song, Role::Melody, up(&own_tune, 12));
        for seed in 0..6 {
            let path = format!(r"D:\Packs\Tunes\Wandering {seed}.mid");
            index.add(&format!("wandering {seed}"), &file(&path, 100), Role::Melody, wandering(seed + 10));
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
    fn the_songs_own_parts_are_every_saves_and_its_loops_in_other_keys() {
        let index = library();
        let cat = index.catalogue();
        // The saves as the library lists them, whatever the letters' case.
        let own = Own::of(&cat, [SAVES[0], &SAVES[1].to_uppercase(), SAVES[2]]);
        let has = |hash: &str| own.has(cat.find(hash).unwrap());
        // Its parts from every save, the one joined by hand too.
        assert!(has("song chords") && has("song lead") && has("song hats") && has("old lead") && has("joined riff"));
        // And the bassline, whose newest copy is in another file.
        assert!(has("song bass") && !cat.entries[cat.find("song bass").unwrap()].is_project());
        assert_eq!(own.len(), 6);
        // The pack's copies of its lead in other keys, and the export's of its bassline, are the same loops.
        assert!(has("pack lead in F") && has("pack lead in D") && has("sub in D"));
        // The bassline's shape in a folder the song has nothing from is another part: a plain shape is in many
        // places. Nor does a drum part's shape make a bassline the song's.
        assert!(!has("bass elsewhere") && !has("pedal bass"));
        // Nor is the lead's shape in a project beside the song's: only a folder of `.mid` files has a loop's keys.
        assert!(!has("sketch riff"));
        // The rest of the library isn't the song's.
        assert!(!has("pack tune") && !has("pack tune in Eb") && !has("other song's tune") && !has("wandering 0"));

        // Without the save joined by hand, its riff isn't the song's.
        let two = Own::of(&cat, [SAVES[0], SAVES[1]]);
        assert!(!two.has(cat.find("joined riff").unwrap()) && two.len() == 5);
        // Another song's own parts are its own; the pack's tune isn't one of its loops (it has no file there).
        let other = Own::of(&cat, [r"C:\Projects\Other\Other_3.flp"]);
        assert_eq!(other.len(), 1);
        assert!(!other.has(cat.find("pack tune").unwrap()));
        // A song the library has nothing of, or one with no saves, leaves nothing out.
        assert!(Own::of(&cat, [r"C:\Projects\New\New_1.flp"]).is_empty());
        assert_eq!(Own::of(&cat, []), Own::default());
    }

    #[test]
    fn a_search_gives_the_parts_that_fit_as_places_in_the_catalogue() {
        let index = library();
        let cat = index.catalogue();
        let song = song();
        let target = Pick::usual(&song, Wants::Tune).unwrap().target(&song).unwrap();
        let own = Own::of(&cat, SAVES);

        let tried = tried(&cat, &own, |_| true);
        // 21 sets of notes, less the song's six and its three loops in other keys.
        assert_eq!((cat.entries.len(), tried.len()), (21, 12));
        let (search, found) = index.search(&tried, &target, 20);
        // The hi-hats' shape as a bassline is tried, as is every key's copy of the pack's tune.
        assert_eq!((search.looked, search.not_ready, search.unread, search.stopped), (12, 0, 0, false));
        // The pack's tune is found as the copy that fits as it is (not the one in Eb, nor the other song's an octave
        // up: the list has a loop once), and the bassline from another folder with the move that puts it on the
        // chords' roots. None of the song's own parts is there.
        assert!(found.contains(&("pack tune".to_owned(), 0)), "{found:?}");
        assert!(found.contains(&("bass elsewhere".to_owned(), 5)), "{found:?}");
        let listed = |hash: &str| found.iter().any(|(found, _)| found == hash);
        assert!(!listed("pack tune in Eb") && !listed("other song's tune"));
        assert!(found.iter().all(|(hash, _)| !own.has(cat.find(hash).unwrap())), "{found:?}");
        // The best first.
        let sits: Vec<f64> = search.found.iter().map(|f| f.sits.judged()).collect();
        assert!(sits.is_sorted_by(|a, b| a >= b), "{sits:?}");

        // What the page's filters hide isn't tried: without the files only the other projects' two are left. The
        // other song's tune is found with nothing moved (octaves don't count), the sketch's riff moved back down.
        let from_projects = super::tried(&cat, &own, |e| e.is_project());
        assert_eq!(from_projects.len(), 2);
        let (_, mut found) = index.search(&from_projects, &target, 3);
        found.sort();
        assert_eq!(found, [("other song's tune".to_owned(), 0), ("sketch riff".to_owned(), -3)]);

        // Without the song's own parts left out they'd be found, and first.
        let nothing_left_out = super::tried(&cat, &Own::default(), |_| true);
        assert_eq!(nothing_left_out.len(), 21);
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
