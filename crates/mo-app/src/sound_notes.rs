//! The one writer of what you do with sounds (stage 3 of "the sound library grows again", review finding X1-own1):
//! deliberate plays, drags out of the app and sends into a DAW are noted on the window's thread, which never waits.
//! A thread of their own, at low priority (rule 21), keeps them in the app's own sound library
//! ([`mo_sounds::compact::did`]), all those noted meanwhile in one short write.
//!
//! Each note has a number, in the order noted. A thread that reads the library holds the writer off while it reads
//! ([`Noter::hold`]): what it reads holds every note up to the number it's told, and none after, so the page knows
//! which of what it showed at once the sounds it reads now already count.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use mo_sounds::compact::did::{self, Did};

use crate::background;

/// Notes what's done with sounds, for the thread that keeps them in the library beside `sounds_db` ([`Noter::start`]).
pub struct Noter {
    sounds_db: PathBuf,
    state: Mutex<State>,
    /// Held by the writer while it writes, and by a reader while it reads ([`Noter::hold`]): the number of the last
    /// note written (or that failed: it won't be written later either).
    written: Arc<Mutex<u64>>,
}

#[derive(Default)]
struct State {
    /// The writer: started at the first note (so a page that's never used starts no thread).
    writer: Option<Writer>,
    /// The writer couldn't be started: said once, and nothing is kept from then on.
    no_writer: bool,
    /// The last note's number.
    noted: u64,
}

struct Writer {
    /// Where notes go to be written, each with its number.
    to: Sender<(u64, Did)>,
    /// Closes as the writer ends.
    ended: Receiver<()>,
    thread: JoinHandle<()>,
}

/// The library held still for a read ([`Noter::hold`]): nothing is written into it until this goes.
pub struct Held<'a>(MutexGuard<'a, u64>);

impl Held<'_> {
    /// The number of the last note in the library as it's read: every one up to it is there (or failed), none after.
    pub fn upto(&self) -> u64 {
        *self.0
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
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.writer.is_none() && !state.no_writer {
            state.writer = self.writer_thread();
            state.no_writer = state.writer.is_none();
        }
        state.noted += 1;
        let number = state.noted;
        let to = &state.writer.as_ref()?.to;
        let mut seen = HashSet::new();
        for &id in ids.iter().filter(|&&id| seen.insert(id)) {
            // The writer went (it can't, short of a crash it logged): nothing more to keep.
            to.send((number, Did { id, kind, at })).ok()?;
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

    /// Starts the writer: each time something's noted, it keeps all that's been noted meanwhile in one write.
    fn writer_thread(&self) -> Option<Writer> {
        let (to, from) = mpsc::channel::<(u64, Did)>();
        let (ending, ended) = mpsc::channel::<()>();
        let sounds_db = self.sounds_db.clone();
        let written = self.written.clone();
        let started = background::spawn("Keep plays", move || {
            // Dropped as the thread ends, however it ends: whoever waits for it hears so.
            let _ending = ending;
            // Each reason it couldn't keep them is said once: a library that isn't the app's own yet says so at
            // every play otherwise.
            let mut said: HashSet<String> = HashSet::new();
            while let Ok(first) = from.recv() {
                let mut notes = vec![first];
                notes.extend(from.try_iter());
                let last = notes.iter().map(|n| n.0).max().unwrap_or_default();
                let did: Vec<Did> = notes.into_iter().map(|n| n.1).collect();
                let mut written = written.lock().unwrap_or_else(PoisonError::into_inner);
                let kept = mo_catch::catch(|| did::keep(&sounds_db, &did));
                *written = (*written).max(last);
                drop(written);
                let why = match kept {
                    Ok(Ok(_)) => continue,
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => "keeping them tripped up".to_owned(),
                };
                if said.insert(why.clone()) {
                    crate::log::warning("Sounds", &format!("Couldn't keep {} plays, drags or sends: {why}", did.len()));
                }
            }
        });
        match started {
            Ok(thread) => Some(Writer { to, ended, thread }),
            Err(e) => {
                crate::log::warning("Sounds", &format!("Couldn't start keeping plays, drags and sends: {e}"));
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
