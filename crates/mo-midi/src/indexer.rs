//! Fills `midi-index.db` (the C# `MidiIndexer`): every pattern or clip in the projects it's given and every `.mid`
//! file in your MIDI folders, read where it is (nothing is moved, copied or changed). Only new and changed files are
//! read, newest first, and files that failed are only tried again when asked.
//!
//! Reading runs on the threads of a pool the caller gives (the app's low-priority background pool, rule 21); saving
//! happens on the calling thread, a batch at a time, so the database is written from one place. A newer run wanted
//! stops this one at the next file (rule 12).
//!
//! Which projects and folders to read is the caller's: the newest save of each song from the library, and the MIDI
//! folders' files (found with the library's folder walk).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use mo_formats::NoteEvent;
use mo_formats::case;
use mo_formats::dates::RealDates;
use mo_harmony::{KeyFact, KeyReader};

use crate::analyser::FEATURE_COUNT;
use crate::extract::{self, Unread};
use crate::index::{MidiError, SourceKind};
use crate::keys;
use crate::made::made;
use crate::roles::Role;
use crate::store::{FileRead, MidiStore, Outcome, ReadKey, Stamp};

/// A file the index should hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub path: String,
    pub kind: SourceKind,
    /// Unix seconds, from the folder listing or the library.
    pub mtime: i64,
    pub size: i64,
}

impl Wanted {
    /// What reading it now would read it with; `None` for a file no reader takes.
    pub fn version(&self) -> Option<u32> {
        match self.kind {
            SourceKind::File => Some(extract::FILE_VERSION),
            SourceKind::Project => extract::project_version(&self.path),
        }
    }
}

/// What a run has to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// New and changed files (and failed ones, when asked), newest first: what you made lately shows up first. A file
    /// whose date isn't real ([`RealDates`]) comes after the rest, as it does in the page's list.
    pub todo: Vec<Wanted>,
    /// Files the index holds that aren't wanted any more, as the index spells them.
    pub forget: Vec<String>,
}

/// Whether a file found in a MIDI folder is worth reading: not shorter than a single note needs (an empty or
/// never-finished download, skipped rather than failed), and not a "._" copy a Mac leaves beside each file.
pub fn worth_reading(path: &str, size: i64) -> bool {
    size >= extract::MIN_MIDI_BYTES as i64 && !mo_formats::paths::file_name(path).starts_with("._")
}

/// Works out what to read and what to forget. `away` are folders that can't be reached now (a drive not plugged in):
/// what was read from them is kept, not forgotten, since reading all of it again when it's back would take hours. A
/// file listed twice (folders inside each other, or two spellings) is read once.
pub fn plan(wanted: &[Wanted], stamps: &[Stamp], away: &[String], retry_failed: bool) -> Plan {
    // Wanted, once each, letter case aside.
    let mut seen = HashSet::new();
    let wanted: Vec<&Wanted> = wanted
        .iter()
        .filter(|w| w.kind == SourceKind::Project || worth_reading(&w.path, w.size))
        .filter(|w| seen.insert(case::key(&w.path)))
        .collect();
    let wanted_keys: HashMap<String, &str> = wanted.iter().map(|w| (case::key(&w.path), w.path.as_str())).collect();
    // What the index holds, one row per file: where two rows are one file (a name whose letters changed case beyond
    // what SQLite folds), the one spelled as wanted stays and the other goes.
    let mut known: HashMap<String, &Stamp> = HashMap::new();
    let mut forget = Vec::new();
    for s in stamps {
        let key = case::key(&s.path);
        let away_now = away.iter().any(|f| mo_library::scan::is_inside(&s.path, f));
        match wanted_keys.get(&key) {
            None if !away_now => forget.push(s.path.clone()),
            None => {}
            Some(spelled) => match known.get(&key) {
                Some(other) if other.path == *spelled => forget.push(s.path.clone()),
                Some(other) => {
                    forget.push(other.path.clone());
                    known.insert(key, s);
                }
                None => {
                    known.insert(key, s);
                }
            },
        }
    }
    let mut todo: Vec<Wanted> = wanted
        .into_iter()
        .filter(|w| {
            let Some(version) = w.version() else { return false };
            match known.get(&case::key(&w.path)) {
                None => true,
                Some(s) => s.mtime != w.mtime || s.size != w.size || s.version != version || (retry_failed && s.failed),
            }
        })
        .cloned()
        .collect();
    let real = RealDates::now();
    todo.sort_by_key(|w| (!real.seconds(w.mtime), std::cmp::Reverse(w.mtime)));
    Plan { todo, forget }
}

/// How a run went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub read: usize,
    pub failed: usize,
    pub forgotten: usize,
    pub parts: usize,
    /// Sets of notes whose key was read again, as the key finder got better.
    pub rekeyed: usize,
    /// Sets of notes given their shape (the same in any key) after the reading: kept before the index kept shapes,
    /// or saved without as theirs tripped ([`MidiStore::save`]).
    pub shaped: usize,
    /// Sets of notes given their slots (what a search for parts that fit a song looks at first) after the reading:
    /// kept before the index kept slots or by another version of them, or saved without as theirs tripped.
    pub slots: usize,
    /// Sets of notes kept in the index that tripped what reads their key again or makes their shape or slots after
    /// the reading (a bug in it; the log has the first in full and a line for each): passed over, so the others
    /// still got theirs, and asked for again at every run until it's put right ([`made`]). A new part's that
    /// tripped as it was saved are among them: those steps ask for them again in the same run.
    pub tripped: usize,
    /// Stopped before the end, because a newer run was wanted.
    pub stopped: bool,
}

/// Saved this often, or every this many files, whichever comes first.
const SAVE_EVERY: Duration = Duration::from_secs(1);
const SAVE_FILES: usize = 100;

/// Carries out a [`plan`]: reads the files with `readers` threads of `pool` and saves what they hold. `progress` hears
/// how many files are done, of how many, and the last one's path. `stop` set (by a newer run) ends it at the next
/// file; what's been read so far is saved.
///
/// Called from a thread that isn't one of `pool`'s (it waits for them while it saves).
pub fn run(
    store: &mut MidiStore,
    plan: &Plan,
    midi_folders: &[String],
    pool: &rayon::ThreadPool,
    readers: usize,
    stop: &AtomicBool,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<Summary, MidiError> {
    run_saving(store, plan, midi_folders, pool, readers, stop, progress, &MidiStore::save)
}

/// What saves a batch of files just read: [`MidiStore::save`].
type Save = dyn Fn(&mut MidiStore, &[FileRead]) -> Result<(), MidiError>;

/// [`run`], with what saves each batch handed in (a test hands in one whose new parts trip).
#[allow(clippy::too_many_arguments)]
fn run_saving(
    store: &mut MidiStore,
    plan: &Plan,
    midi_folders: &[String],
    pool: &rayon::ThreadPool,
    readers: usize,
    stop: &AtomicBool,
    progress: &dyn Fn(usize, usize, &str),
    save: &Save,
) -> Result<Summary, MidiError> {
    if pool.current_thread_index().is_some() {
        return Err(MidiError::Open {
            path: store.path().to_owned(),
            why: "the indexer was started on one of its own reading threads".into(),
        });
    }
    let mut summary =
        Summary { forgotten: store.forget(plan.forget.iter().map(String::as_str))?, ..Summary::default() };
    let total = plan.todo.len();
    if total == 0 {
        after_reading(store, pool, readers, stop, progress, &mut summary)?;
        return Ok(summary);
    }
    // A small bounded queue: readers wait when the saver is behind, so read files never pile up in memory.
    let (tx, rx) = sync_channel::<FileRead>(16);
    let next = AtomicUsize::new(0);
    let readers = readers.clamp(1, pool.current_num_threads().max(1));
    // A save that failed stops the readers too; the caller's `stop` is only read.
    let failed = AtomicBool::new(false);
    let mut saved: Result<(), MidiError> = Ok(());
    std::thread::scope(|scope| {
        let (next, failed) = (&next, &failed);
        let todo = &plan.todo;
        scope.spawn(move || {
            pool.scope(|s| {
                for _ in 0..readers {
                    let tx = tx.clone();
                    // Each thread takes the next file in order, so the newest are read first.
                    s.spawn(move |_| {
                        while !stop.load(Ordering::Relaxed) && !failed.load(Ordering::Relaxed) {
                            let Some(w) = todo.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                            let outcome = match extract::read(Path::new(&w.path), midi_folders) {
                                Ok(found) => Outcome::Parts(found),
                                Err(Unread::InUse(why)) => Outcome::InUse(why),
                                Err(Unread::Failed(why)) => Outcome::Failed(why),
                            };
                            let read = FileRead {
                                path: w.path.clone(),
                                kind: w.kind,
                                mtime: w.mtime,
                                size: w.size,
                                version: w.version().unwrap_or(0),
                                outcome,
                            };
                            // The saver stopped (a save failed): nothing more to do.
                            if tx.send(read).is_err() {
                                break;
                            }
                        }
                    });
                }
                drop(tx);
            });
        });

        let mut batch: Vec<FileRead> = Vec::new();
        let mut last = Instant::now();
        for read in rx.iter() {
            summary.read += 1;
            match &read.outcome {
                Outcome::Parts(found) => summary.parts += found.len(),
                Outcome::Failed(_) | Outcome::InUse(_) => summary.failed += 1,
            }
            let name = read.path.clone();
            batch.push(read);
            if batch.len() >= SAVE_FILES || last.elapsed() >= SAVE_EVERY {
                if let Err(e) = save(store, &batch) {
                    saved = Err(e);
                    failed.store(true, Ordering::Relaxed);
                    break;
                }
                batch.clear();
                last = Instant::now();
                progress(summary.read, total, &name);
            }
        }
        // Dropping the receiver lets readers still waiting to hand a file over stop too.
        drop(rx);
        if saved.is_ok()
            && let Err(e) = save(store, &batch)
        {
            saved = Err(e);
        }
    });
    saved?;
    summary.stopped = summary.read < total;
    progress(summary.read, total, "");
    if !summary.stopped {
        after_reading(store, pool, readers, stop, progress, &mut summary)?;
    }
    Ok(summary)
}

/// What's done once the files are read: notes no file has any more go, keys are read again where the key finder got
/// better, and notes with no shape or slots get theirs (kept before the index kept those, or saved without as
/// theirs tripped).
fn after_reading(
    store: &mut MidiStore,
    pool: &rayon::ThreadPool,
    readers: usize,
    stop: &AtomicBool,
    progress: &dyn Fn(usize, usize, &str),
    summary: &mut Summary,
) -> Result<(), MidiError> {
    store.tidy()?;
    let keys = rekey(store, pool, readers, stop, progress, &keys::reread)?;
    summary.rekeyed = keys.made;
    summary.tripped += keys.tripped;
    summary.stopped = !keys.finished;
    if !summary.stopped {
        let shapes = fill_shapes(store, pool, stop)?;
        summary.shaped = shapes.made;
        summary.tripped += shapes.tripped;
        summary.stopped = !shapes.finished;
    }
    if !summary.stopped {
        let slots = fill_slots(store, pool, stop)?;
        summary.slots = slots.made;
        summary.tripped += slots.tripped;
        summary.stopped = !slots.finished;
    }
    Ok(())
}

/// How one of the steps after reading went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Went {
    /// Sets of notes that got what the step makes.
    made: usize,
    /// Sets of notes passed over, as what makes it tripped on them ([`made`]).
    tripped: usize,
    /// Whether it got to the end (`stop` wasn't set).
    finished: bool,
}

/// Sets of notes given something made from them (their shape, their slots) at once.
const FILL_BATCH: usize = 2_000;

/// Makes something from each set of notes that has none of it yet, from the notes kept in the index: no file is
/// opened. `without` gives the next of those after a set of notes (by id) with their notes, `make` makes one's (on
/// `pool`), `save` keeps a batch's. `stop` ends it after the batch it's on.
///
/// Notes that trip `make` are passed over ([`made`]) and keep none of it, so every later run asks for them again
/// (each run then goes over the table once to find them, which is quick). `tripped` is whether it has tripped
/// already since the index was opened, as new parts were saved ([`MidiStore::tripped`]): the bug is in the log in
/// full then, and the same notes asked for again here get a line each, not a second crash log.
fn fill<T: Send>(
    store: &mut MidiStore,
    pool: &rayon::ThreadPool,
    stop: &AtomicBool,
    tripped: bool,
    without: impl Fn(&MidiStore, i64, usize) -> Result<Vec<(i64, Vec<NoteEvent>)>, MidiError>,
    make: impl Fn(&[NoteEvent]) -> T + Sync,
    save: impl Fn(&mut MidiStore, &[(i64, T)]) -> Result<(), MidiError>,
) -> Result<Went, MidiError> {
    let mut went = Went::default();
    let tripped = AtomicBool::new(tripped);
    let mut after = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(went);
        }
        let batch = without(store, after, FILL_BATCH)?;
        let Some(&(last, _)) = batch.last() else {
            return Ok(Went { finished: true, ..went });
        };
        after = last;
        let kept: Vec<(i64, T)> = pool.install(|| {
            use rayon::prelude::*;
            batch.par_iter().filter_map(|(id, notes)| Some((*id, made(&tripped, || make(notes))?))).collect()
        });
        save(store, &kept)?;
        went.made += kept.len();
        went.tripped += batch.len() - kept.len();
    }
}

/// Works out the shape (the same in every key) of the notes with none: saved before the index kept shapes, or
/// saved without as theirs tripped ([`fill`]). Quick: a hash a set of notes.
fn fill_shapes(store: &mut MidiStore, pool: &rayon::ThreadPool, stop: &AtomicBool) -> Result<Went, MidiError> {
    if store.unshaped_count()? == 0 {
        return Ok(Went { finished: true, ..Went::default() });
    }
    let tripped = store.tripped().0;
    fill(store, pool, stop, tripped, MidiStore::unshaped, crate::analyser::shape_of, MidiStore::save_shapes)
}

/// Works out the slots (for a search's first look) of the notes with none of this [`crate::fit::SLOTS_VERSION`]:
/// saved before the index kept slots, or by another version, or saved without as theirs tripped ([`fill`]). Quick:
/// a few microseconds a set of notes.
fn fill_slots(store: &mut MidiStore, pool: &rayon::ThreadPool, stop: &AtomicBool) -> Result<Went, MidiError> {
    if store.without_slots_count()? == 0 {
        return Ok(Went { finished: true, ..Went::default() });
    }
    let tripped = store.tripped().1;
    fill(store, pool, stop, tripped, MidiStore::without_slots, crate::fit::slots_of, MidiStore::save_slots)
}

/// Sets of notes read again at once, for their key.
const REKEY_BATCH: usize = 500;

/// Reads again the key of every set of notes that needs it (an older key finder read it, or its kind changed), from
/// the notes kept in the index: no file is opened. On `readers` threads of `pool`, a batch at a time, in the order of
/// the notes' ids; `progress` hears how many are done, of how many, with no path. `stop` ends it after the batch it's
/// on. `reread` is [`keys::reread`] (a test hands in one that trips).
///
/// Notes that trip `reread` are passed over ([`made`]) and keep the key they had, so every later run asks for them
/// again.
fn rekey(
    store: &mut MidiStore,
    pool: &rayon::ThreadPool,
    readers: usize,
    stop: &AtomicBool,
    progress: &dyn Fn(usize, usize, &str),
    reread: &Reread,
) -> Result<Went, MidiError> {
    let mut went = Went::default();
    // A quick count from a small index: mostly there are none, and the keys aren't gone over at all.
    let total = store.stale_key_count()?;
    if total == 0 {
        return Ok(Went { finished: true, ..went });
    }
    let readers = readers.clamp(1, pool.current_num_threads().max(1));
    let tripped = AtomicBool::new(false);
    let mut after = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(went);
        }
        // From the last set given on, not from the top: one that trips stays as it was, and would be given again.
        let stale = store.stale_keys(after, REKEY_BATCH)?;
        let Some(last) = stale.last() else {
            return Ok(Went { finished: true, ..went });
        };
        after = last.notes_id;
        let share = stale.len().div_ceil(readers);
        let mut read: Vec<Vec<ReadKey>> = Vec::new();
        pool.scope(|s| {
            let mut slots: Vec<&mut Vec<ReadKey>> = Vec::new();
            read.resize_with(readers, Vec::new);
            slots.extend(read.iter_mut());
            let tripped = &tripped;
            for (chunk, slot) in stale.chunks(share).zip(slots) {
                s.spawn(move |_| {
                    keys::with_reader(|reader| {
                        for k in chunk {
                            match made(tripped, || reread(reader, &k.notes, k.role, k.cut)) {
                                Some((fact, features)) => {
                                    slot.push(ReadKey { notes_id: k.notes_id, role: k.role, fact, features });
                                }
                                // What it was working on may be left half done: the next set gets a new one.
                                None => *reader = KeyReader::new(),
                            }
                        }
                    });
                });
            }
        });
        let read: Vec<ReadKey> = read.into_iter().flatten().collect();
        store.save_keys(&read)?;
        went.made += read.len();
        went.tripped += stale.len() - read.len();
        progress((went.made + went.tripped).min(total), total, "");
    }
}

/// What reads a set of notes' key again: [`keys::reread`].
type Reread = dyn Fn(&mut KeyReader, &[NoteEvent], Role, bool) -> (Option<KeyFact>, [f32; FEATURE_COUNT]) + Sync;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::fit::slots_of;

    /// The first notes of the three made-up files: a tune, one a fourth up, and a bassline.
    const TUNE: u8 = 60;
    const HIGHER: u8 = 65;
    const BASS: u8 = 36;

    /// A one-track MIDI file that plays these keys one after another, a quarter note each.
    fn smf(keys: &[u8]) -> Vec<u8> {
        let mut track = Vec::new();
        for &key in keys {
            track.extend([0x00, 0x90, key, 100, 0x60, 0x80, key, 0]);
        }
        track.extend([0x00, 0xFF, 0x2F, 0x00]);
        let mut out = b"MThd".to_vec();
        out.extend([0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
        out.extend(b"MTrk");
        out.extend((track.len() as u32).to_be_bytes());
        out.extend(track);
        out
    }

    fn pool() -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    /// A folder of a test's own under the temp folder, by this run of the tests (two runs at once don't share one);
    /// gone again when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Folder {
            let dir = std::env::temp_dir().join(format!("mo-midi-indexer-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Folder(dir)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Three files written into `folder`, one part each: the same tune from [`TUNE`], [`HIGHER`] and [`BASS`].
    fn three_files(folder: &Folder) -> Vec<Wanted> {
        let tune = [0, 4, 7, 12, 11, 7, 5, 4, 2, -1, 0, 7];
        let files = [("Tune.mid", TUNE), ("Higher.mid", HIGHER), ("Bass.mid", BASS)];
        files
            .iter()
            .map(|(file, first)| {
                let keys: Vec<u8> = tune.iter().map(|step| (i32::from(*first) + step) as u8).collect();
                let path = folder.0.join(file);
                std::fs::write(&path, smf(&keys)).unwrap();
                Wanted {
                    path: path.to_string_lossy().into_owned(),
                    kind: SourceKind::File,
                    mtime: 1_760_000_000,
                    size: std::fs::metadata(&path).unwrap().len() as i64,
                }
            })
            .collect()
    }

    /// An index of three files, one part each, in `folder`.
    fn index(folder: &Folder) -> (MidiStore, PathBuf) {
        let dir = &folder.0;
        let wanted = three_files(folder);
        let db = dir.join("midi-index.db");
        let mut store = MidiStore::open(&db).unwrap();
        let plan = plan(&wanted, &[], &[], false);
        let folders = [dir.to_string_lossy().into_owned()];
        let read = run(&mut store, &plan, &folders, &pool(), 2, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
        // Nothing trips the app's own readers.
        assert_eq!((read.read, read.parts, read.failed, read.tripped, read.stopped), (3, 3, 0, 0, false));
        (store, db)
    }

    /// Each part's key and "more like this" numbers, by its file's name.
    fn keys_of(store: &MidiStore) -> HashMap<String, (Option<KeyFact>, Vec<u8>)> {
        let all = store.all(&HashMap::new(), &HashMap::new()).unwrap();
        all.into_iter().map(|p| (p.clip.to_string(), (p.key_fact, p.features.to_vec()))).collect()
    }

    /// The key finder, but for notes that start on one of `bad`: there it stops, as a bug in it would.
    fn trips_on(
        bad: &[u8],
    ) -> impl Fn(&mut KeyReader, &[NoteEvent], Role, bool) -> (Option<KeyFact>, [f32; FEATURE_COUNT]) + Sync {
        move |reader, notes, role, cut| {
            assert!(
                !notes.first().is_some_and(|n| bad.contains(&n.key)),
                "these notes trip the key finder (on purpose)"
            );
            keys::reread(reader, notes, role, cut)
        }
    }

    /// The 14 November 2026 review's MI-15.
    #[test]
    fn notes_that_trip_the_key_finder_are_passed_over_and_the_others_get_their_keys() {
        let folder = Folder::new("keys");
        let (mut store, db) = index(&folder);
        let before = keys_of(&store);
        assert!(before.values().all(|(fact, _)| fact.is_some()), "{before:?}");
        // As an older key finder left them: to read again, each with the key that one read.
        let an_older_finder_read_them = || {
            rusqlite::Connection::open(&db).unwrap().execute("UPDATE keys SET version = 1", []).unwrap();
        };
        let go = AtomicBool::new(false);
        let said = std::sync::Mutex::new(Vec::new());
        let progress = |done: usize, total: usize, _: &str| said.lock().unwrap().push((done, total));

        an_older_finder_read_them();
        assert_eq!(store.stale_key_count().unwrap(), 3);
        let went = rekey(&mut store, &pool(), 2, &go, &progress, &trips_on(&[HIGHER])).unwrap();
        assert_eq!(went, Went { made: 2, tripped: 1, finished: true });
        // The other two are read; the one that tripped keeps the key it had, and is still to read.
        assert_eq!(keys_of(&store), before);
        assert_eq!(store.stale_key_count().unwrap(), 1);
        assert_eq!(*said.lock().unwrap(), [(3, 3)]);
        // The next run asks for it again, and it alone.
        let again = rekey(&mut store, &pool(), 2, &go, &progress, &trips_on(&[HIGHER])).unwrap();
        assert_eq!(again, Went { made: 0, tripped: 1, finished: true });

        // Every set of notes tripping still ends: each is given once a run, though none is ever read.
        an_older_finder_read_them();
        let all = rekey(&mut store, &pool(), 2, &go, &progress, &trips_on(&[TUNE, HIGHER, BASS])).unwrap();
        assert_eq!(all, Went { made: 0, tripped: 3, finished: true });
        assert_eq!((store.stale_key_count().unwrap(), keys_of(&store)), (3, before.clone()));

        // With the key finder put right they're all read, by the same threads' readers as tripped before.
        let right = rekey(&mut store, &pool(), 2, &go, &progress, &keys::reread).unwrap();
        assert_eq!(right, Went { made: 3, tripped: 0, finished: true });
        assert_eq!((store.stale_key_count().unwrap(), keys_of(&store)), (0, before));
        // And a run told to stop says it didn't get to the end.
        an_older_finder_read_them();
        let stopped = rekey(&mut store, &pool(), 2, &AtomicBool::new(true), &progress, &keys::reread).unwrap();
        assert_eq!(stopped, Went::default());
    }

    /// The keys to read again come a batch at a time from where the last batch ended, so more than one batch of
    /// notes that trip is gone through once and the reading ends.
    #[test]
    fn keys_to_read_again_are_given_in_order_from_the_last_one_on() {
        let folder = Folder::new("order");
        let (store, db) = index(&folder);
        rusqlite::Connection::open(&db).unwrap().execute("UPDATE keys SET version = 1 WHERE notes != 2", []).unwrap();
        let ids = |stale: Vec<crate::store::StaleKey>| stale.iter().map(|k| k.notes_id).collect::<Vec<_>>();
        assert_eq!(ids(store.stale_keys(0, 1).unwrap()), [1]);
        assert_eq!(ids(store.stale_keys(1, 1).unwrap()), [3]);
        assert_eq!(ids(store.stale_keys(0, 10).unwrap()), [1, 3]);
        assert!(store.stale_keys(3, 10).unwrap().is_empty());
    }

    /// MI-15 again, for what's made from the notes after the keys: their shape, their slots.
    #[test]
    fn notes_that_trip_a_fill_are_passed_over_and_asked_for_again_next_time() {
        let folder = Folder::new("fill");
        let (mut store, db) = index(&folder);
        let go = AtomicBool::new(false);
        assert_eq!(store.without_slots_count().unwrap(), 0);
        rusqlite::Connection::open(&db).unwrap().execute("DELETE FROM slots", []).unwrap();
        assert_eq!(store.without_slots_count().unwrap(), 3);

        let trips = |notes: &[NoteEvent]| {
            assert!(notes.first().is_none_or(|n| n.key != BASS), "these notes trip the slots (on purpose)");
            slots_of(notes)
        };
        let went =
            fill(&mut store, &pool(), &go, false, MidiStore::without_slots, trips, MidiStore::save_slots).unwrap();
        assert_eq!(went, Went { made: 2, tripped: 1, finished: true });
        // The two that didn't trip have theirs; the one that did has none, so a search doesn't take it for a part
        // with nothing in it.
        assert_eq!(store.without_slots_count().unwrap(), 1);
        // The next run asks for it again, and with the slots put right it gets them.
        let next = fill_slots(&mut store, &pool(), &go).unwrap();
        assert_eq!(next, Went { made: 1, tripped: 0, finished: true });
        assert_eq!(store.without_slots_count().unwrap(), 0);

        // A run that's told to stop says it didn't get to the end.
        rusqlite::Connection::open(&db).unwrap().execute("DELETE FROM slots", []).unwrap();
        let stopped = fill_slots(&mut store, &pool(), &AtomicBool::new(true)).unwrap();
        assert_eq!(stopped, Went::default());
    }

    /// The 19 November 2026 review's MP-11: MI-15 once more, for a part just read from a file. Its shape and its
    /// slots are made as it's saved, and nothing caught them there: a bug in either on one new loop ended the
    /// reading at that loop's file, the batch it was in unsaved, and again at every start.
    #[test]
    fn a_new_part_that_trips_its_shape_or_its_slots_is_saved_without_and_asked_for_again() {
        let folder = Folder::new("fresh");
        let batch: Vec<FileRead> = three_files(&folder)
            .into_iter()
            .map(|w| FileRead {
                outcome: Outcome::Parts(extract::read(Path::new(&w.path), &[]).unwrap()),
                version: w.version().unwrap(),
                path: w.path,
                kind: w.kind,
                mtime: w.mtime,
                size: w.size,
            })
            .collect();
        let mut store = MidiStore::open(&folder.0.join("midi-index.db")).unwrap();
        // The bassline's notes trip the shape, the higher tune's the slots, as a bug in either would.
        let shape = |notes: &[NoteEvent]| {
            assert!(notes.first().is_none_or(|n| n.key != BASS), "these notes trip the shape (on purpose)");
            crate::analyser::shape_of(notes)
        };
        let slots = |notes: &[NoteEvent]| {
            assert!(notes.first().is_none_or(|n| n.key != HIGHER), "these notes trip the slots (on purpose)");
            slots_of(notes)
        };
        for _ in 0..2 {
            // Twice: the second time the notes are there already (the same files read again), and trip again.
            store.save_making(&batch, shape, slots).unwrap();
            // Every file is saved and stamped, and every part is there with its notes.
            assert_eq!(store.stamps().unwrap().iter().filter(|s| !s.failed && s.mtime > 0).count(), 3);
            let parts = store.all(&HashMap::new(), &HashMap::new()).unwrap();
            assert_eq!(parts.len(), 3);
            // Only what tripped is missing: one shape, one set of slots.
            assert_eq!((store.unshaped_count().unwrap(), store.without_slots_count().unwrap()), (1, 1));
            let first_key = |list: Vec<(i64, Vec<NoteEvent>)>| list.iter().map(|(_, n)| n[0].key).collect::<Vec<_>>();
            assert_eq!(first_key(store.unshaped(0, 10).unwrap()), [BASS]);
            assert_eq!(first_key(store.without_slots(0, 10).unwrap()), [HIGHER]);
        }
        // The steps after the reading ask for them again, and with the bug put right they get theirs.
        let go = AtomicBool::new(false);
        assert_eq!(fill_shapes(&mut store, &pool(), &go).unwrap(), Went { made: 1, tripped: 0, finished: true });
        assert_eq!(fill_slots(&mut store, &pool(), &go).unwrap(), Went { made: 1, tripped: 0, finished: true });
        assert_eq!((store.unshaped_count().unwrap(), store.without_slots_count().unwrap()), (0, 0));
        // And a file read again then keeps them.
        store.save(&batch).unwrap();
        assert_eq!((store.unshaped_count().unwrap(), store.without_slots_count().unwrap()), (0, 0));

        // A whole run, as the page starts one: it gets to its end with every file read and saved, where it ended at
        // the first batch with such a part in it. The steps after the reading (which don't trip here) make what the
        // saving passed over.
        let folder = Folder::new("fresh-run");
        let wanted = three_files(&folder);
        let mut store = MidiStore::open(&folder.0.join("midi-index.db")).unwrap();
        let tripping = move |store: &mut MidiStore, batch: &[FileRead]| store.save_making(batch, shape, slots);
        let plan = plan(&wanted, &[], &[], false);
        let read = run_saving(&mut store, &plan, &[], &pool(), 2, &go, &|_, _, _| {}, &tripping).unwrap();
        assert_eq!((read.read, read.parts, read.failed, read.stopped), (3, 3, 0, false));
        assert_eq!((read.shaped, read.slots, read.tripped), (1, 1, 0));
        assert_eq!(store.tripped(), (true, true));
        assert_eq!(store.stamps().unwrap().iter().filter(|s| !s.failed && s.mtime > 0).count(), 3);
        assert_eq!((store.unshaped_count().unwrap(), store.without_slots_count().unwrap()), (0, 0));
    }
}
