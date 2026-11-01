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

use mo_formats::case;

use crate::extract::{self, Unread};
use crate::index::{MidiError, SourceKind};
use crate::keys;
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
    /// New and changed files (and failed ones, when asked), newest first: what you made lately shows up first.
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
    todo.sort_by_key(|w| std::cmp::Reverse(w.mtime));
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
        store.tidy()?;
        let (rekeyed, finished) = rekey(store, pool, readers, stop, progress)?;
        summary.rekeyed = rekeyed;
        summary.stopped = !finished;
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
                if let Err(e) = store.save(&batch) {
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
            && let Err(e) = store.save(&batch)
        {
            saved = Err(e);
        }
    });
    saved?;
    summary.stopped = summary.read < total;
    progress(summary.read, total, "");
    if !summary.stopped {
        store.tidy()?;
        let (rekeyed, finished) = rekey(store, pool, readers, stop, progress)?;
        summary.rekeyed = rekeyed;
        summary.stopped = !finished;
    }
    Ok(summary)
}

/// Sets of notes read again at once, for their key.
const REKEY_BATCH: usize = 500;

/// Reads again the key of every set of notes that needs it (an older key finder read it, or its kind changed), from
/// the notes kept in the index: no file is opened. On `readers` threads of `pool`, a batch at a time; `progress` hears
/// how many are done, of how many, with no path. `stop` ends it after the batch it's on. Gives how many were read and
/// whether it got to the end.
fn rekey(
    store: &mut MidiStore,
    pool: &rayon::ThreadPool,
    readers: usize,
    stop: &AtomicBool,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<(usize, bool), MidiError> {
    let total = store.stale_key_count()?;
    let readers = readers.clamp(1, pool.current_num_threads().max(1));
    let mut done = 0;
    while done < total {
        if stop.load(Ordering::Relaxed) {
            return Ok((done, false));
        }
        let stale = store.stale_keys(REKEY_BATCH)?;
        if stale.is_empty() {
            break;
        }
        let share = stale.len().div_ceil(readers);
        let mut read: Vec<Vec<ReadKey>> = Vec::new();
        pool.scope(|s| {
            let mut slots: Vec<&mut Vec<ReadKey>> = Vec::new();
            read.resize_with(readers, Vec::new);
            slots.extend(read.iter_mut());
            for (chunk, slot) in stale.chunks(share).zip(slots) {
                s.spawn(move |_| {
                    keys::with_reader(|reader| {
                        for k in chunk {
                            let (fact, features) = keys::reread(reader, &k.notes, k.role, k.cut);
                            slot.push(ReadKey { notes_id: k.notes_id, role: k.role, fact, features });
                        }
                    });
                });
            }
        });
        let read: Vec<ReadKey> = read.into_iter().flatten().collect();
        store.save_keys(&read)?;
        done += read.len();
        progress(done, total, "");
    }
    Ok((done, true))
}
