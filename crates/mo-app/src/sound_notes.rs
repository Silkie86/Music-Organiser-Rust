//! The one writer of what you do with sounds and your own say about them (stages 3 and 5 of "the sound library grows
//! again", review finding X1-own1): deliberate plays, drags out of the app and sends into a DAW, and favourites, your
//! own types and tags, are noted on the window's thread, which never waits. A thread of their own, at low
//! priority (rule 21), keeps them in the app's own sound library ([`mo_sounds::compact::did`],
//! [`mo_sounds::compact::yours`]), all those noted meanwhile in one short write of each.
//!
//! Each note has a number, in the order noted. A thread that reads the library holds the writer off while it reads
//! ([`Noter::hold`]): what it reads holds every note up to the number it's told, and none after, so the page knows
//! which of what it showed at once the sounds it reads now already count. Your say that couldn't be kept (the library
//! busy, say) is tried again with each later write, and until it's kept the page goes on showing it ([`Holds`]).

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use mo_sounds::compact::did::{self, Did};
use mo_sounds::compact::yours::{self, Change, Yours};

use crate::background;

/// Notes what's done with sounds and your own say about them, for the thread that keeps them in the library beside
/// `sounds_db` ([`Noter::start`]).
pub struct Noter {
    sounds_db: PathBuf,
    state: Mutex<State>,
    /// Held by the writer while it writes, and by a reader while it reads ([`Noter::hold`]): what the library holds.
    written: Arc<Mutex<Holds>>,
}

/// Which notes the library holds as it's read ([`Held::holds`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holds {
    /// The number of the last note written (or that failed for good: it won't be written later either).
    pub upto: u64,
    /// Your say up to it that couldn't be kept yet: tried again with the next write ([`MOST_TRIES`] times at most).
    pub waiting: BTreeSet<u64>,
}

impl Holds {
    /// Whether the library holds the note numbered `number` (or it failed for good).
    pub fn has(&self, number: u64) -> bool {
        number <= self.upto && !self.waiting.contains(&number)
    }
}

/// How many writes a change you made is tried in before it's given up (said in the log): a library that isn't the
/// app's own yet, or is damaged, refuses every one.
pub const MOST_TRIES: u32 = 10;

#[derive(Default)]
struct State {
    /// The writer: started at the first note (so a page that's never used starts no thread).
    writer: Option<Writer>,
    /// The writer couldn't be started: said once, and nothing is kept from then on.
    no_writer: bool,
    /// The last note's number.
    noted: u64,
}

/// One note, for the writer.
enum Note {
    Did(Did),
    Yours(Yours),
}

struct Writer {
    /// Where notes go to be written, each with its number.
    to: Sender<(u64, Note)>,
    /// Closes as the writer ends.
    ended: Receiver<()>,
    thread: JoinHandle<()>,
}

/// The library held still for a read ([`Noter::hold`]): nothing is written into it until this goes.
pub struct Held<'a>(MutexGuard<'a, Holds>);

impl Held<'_> {
    /// The number of the last note in the library as it's read: every one up to it is there (or failed), none after,
    /// short of your say still waiting to be kept ([`Held::holds`]).
    pub fn upto(&self) -> u64 {
        self.0.upto
    }

    /// Which notes the library holds as it's read.
    pub fn holds(&self) -> Holds {
        self.0.clone()
    }
}

impl Noter {
    /// A noter for the app's own sound library beside `sounds_db`.
    pub fn start(sounds_db: PathBuf) -> Noter {
        Noter { sounds_db, state: Mutex::default(), written: Arc::default() }
    }

    /// Notes that `kind` ([`mo_sounds::event`]) was done just now with the sounds numbered `ids` (each once). Returns
    /// at once, with the note's number: they're kept in the background, and a failure is logged (once for each
    /// reason). `None` when they can't be kept (the writer couldn't start).
    pub fn note(&self, ids: &[i64], kind: &'static str) -> Option<u64> {
        let at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or_default();
        let mut seen = HashSet::new();
        self.send(ids.iter().filter(|&&id| seen.insert(id)).map(|&id| Note::Did(Did { id, kind, at })))
    }

    /// Notes `change`, which you just made to the sound numbered `id`, as [`Noter::note`] does: kept after every
    /// change noted before it.
    pub fn note_yours(&self, id: i64, change: Change) -> Option<u64> {
        self.send(std::iter::once(Note::Yours(Yours { id, change })))
    }

    /// Notes many changes at once under one number, as [`Noter::note_yours`] does each: kept together, in one write
    /// (Favourites from Bitwig marks thousands of sounds). `None` when they can't be kept, or there are none.
    pub fn note_all_yours(&self, yours: Vec<Yours>) -> Option<u64> {
        if yours.is_empty() {
            return None;
        }
        self.send(yours.into_iter().map(Note::Yours))
    }

    /// Gives the writer `notes`, under the next number (starting it at the first note, so a page that's never used
    /// starts no thread).
    fn send(&self, notes: impl Iterator<Item = Note>) -> Option<u64> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.writer.is_none() && !state.no_writer {
            state.writer = self.writer_thread();
            state.no_writer = state.writer.is_none();
        }
        state.noted += 1;
        let number = state.noted;
        let to = &state.writer.as_ref()?.to;
        for note in notes {
            // The writer went (it can't, short of a crash it logged): nothing more to keep.
            to.send((number, note)).ok()?;
        }
        Some(number)
    }

    /// Holds the writer off while the library is read, until what's returned goes. Waits for a write under way to end
    /// (a moment), so never on the window's thread.
    pub fn hold(&self) -> Held<'_> {
        Held(self.written.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Waits for everything noted so far to be kept (or to have failed), and lets the writer go; the next note starts
    /// it again.
    pub fn finish(&self) {
        self.finish_within(None);
    }

    /// As the app closes: what's noted is kept, waiting `longest` at most (the library may be busy with its backup
    /// copy, or being read). Whether it all was. One cut short loses only what wasn't written: each write is whole or
    /// not there.
    pub fn finish_within(&self, longest: Option<Duration>) -> bool {
        let writer = self.state.lock().unwrap_or_else(PoisonError::into_inner).writer.take();
        let Some(Writer { to, ended, thread }) = writer else { return true };
        drop(to);
        if let Some(longest) = longest
            && ended.recv_timeout(longest) == Err(RecvTimeoutError::Timeout)
        {
            return false;
        }
        let _ = thread.join();
        true
    }

    /// Starts the writer: each time something's noted, it keeps all that's been noted meanwhile, in one write of
    /// what's done with sounds and one of your own say about them (in the order noted).
    fn writer_thread(&self) -> Option<Writer> {
        let (to, from) = mpsc::channel::<(u64, Note)>();
        let (ending, ended) = mpsc::channel::<()>();
        let sounds_db = self.sounds_db.clone();
        let written = self.written.clone();
        let started = background::spawn("Keep what's done with sounds", move || {
            // Dropped as the thread ends, however it ends: whoever waits for it hears so.
            let _ending = ending;
            // Each reason it couldn't keep them is said once: a library that isn't the app's own yet says so at
            // every play otherwise.
            let mut said: HashSet<(bool, String)> = HashSet::new();
            let mut say = |what: String, why: String, given_up: bool| {
                if said.insert((given_up, why.clone())) {
                    crate::log::warning("Sounds", &format!("Couldn't keep {what}: {why}"));
                }
            };
            // Your say not kept yet, in the order noted, each with its number and the writes it's been tried in.
            let mut waiting: Vec<(u64, Yours, u32)> = Vec::new();
            let mut more = true;
            while more {
                let mut notes = Vec::new();
                match from.recv() {
                    Ok(first) => {
                        notes.push(first);
                        notes.extend(from.try_iter());
                    }
                    // Closing: what's still waiting is tried once more.
                    Err(_) if !waiting.is_empty() => more = false,
                    Err(_) => break,
                }
                let last = notes.iter().map(|n| n.0).max().unwrap_or_default();
                let mut did = Vec::new();
                for (number, note) in notes {
                    match note {
                        Note::Did(d) => did.push(d),
                        Note::Yours(y) => waiting.push((number, y, 0)),
                    }
                }
                let mut written = written.lock().unwrap_or_else(PoisonError::into_inner);
                // They only write the app's own notes, so a panic in them is a bug: logged as a crash, in full (review
                // finding S-7).
                let kept_did = (!did.is_empty()).then(|| mo_catch::catch_crash(|| did::keep(&sounds_db, &did)));
                let kept_yours = (!waiting.is_empty()).then(|| {
                    let yours: Vec<Yours> = waiting.iter().map(|(_, y, _)| y.clone()).collect();
                    mo_catch::catch_crash(|| yours::keep(&sounds_db, &yours))
                });
                let why = |kept: Result<Result<usize, mo_sounds::SoundsError>, _>| match kept {
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(e.to_string()),
                    Err(_) => Some("keeping them tripped up".to_owned()),
                };
                let yours_failed = kept_yours.and_then(why);
                let tried = waiting.len();
                let mut given_up = 0;
                if yours_failed.is_some() {
                    waiting.retain_mut(|(_, _, tries)| {
                        *tries += 1;
                        given_up += usize::from(*tries >= MOST_TRIES || !more);
                        *tries < MOST_TRIES && more
                    });
                } else {
                    waiting.clear();
                }
                written.upto = written.upto.max(last);
                written.waiting = waiting.iter().map(|(number, _, _)| *number).collect();
                drop(written);
                if let Some(why) = kept_did.and_then(why) {
                    say(format!("{} plays, drags or sends", did.len()), why, false);
                }
                if let Some(why) = yours_failed {
                    let what = if given_up > 0 {
                        format!("{given_up} changes to favourites, types or tags (gave up)")
                    } else {
                        format!("{tried} changes to favourites, types or tags yet (tried again later)")
                    };
                    say(what, why, given_up > 0);
                }
            }
        });
        match started {
            Ok(thread) => Some(Writer { to, ended, thread }),
            Err(e) => {
                crate::log::warning("Sounds", &format!("Couldn't start keeping what's done with sounds: {e}"));
                None
            }
        }
    }
}

impl Drop for Noter {
    fn drop(&mut self) {
        self.finish();
    }
}
