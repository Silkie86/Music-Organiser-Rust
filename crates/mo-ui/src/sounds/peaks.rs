//! The waveforms of the sounds on show. The app's own sound library keeps them in a table of their own and the page's
//! list is read without them (a third less to read, and about 150 MB less to hold for half a million sounds), so each
//! one is read as its sound shows: on a tile, on the picked sound's card, or carried out in a drag. Those asked for in
//! one turn of the event loop are read together, away from the window's thread, and kept for the next time they show.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use mo_app::{background, log};
use mo_sounds::{Peaks, Sound, SoundsDb};
use slint::ComponentHandle;

use super::PAGE;
use crate::AppWindow;

/// How many waveforms are kept: many screens of tiles, a megabyte or two. Past that the one kept longest goes first.
const KEEP: usize = 4096;

/// How long asks wait after a read failed, so a library that can't be read isn't tried again in every frame.
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// The waveforms kept, and those asked for.
#[derive(Default)]
struct Kept {
    /// The library they're read from: another starts again.
    db: Option<PathBuf>,
    /// Each sound's waveform by id, with when its file was last changed as the list said then (a file changed since is
    /// analysed again, so its waveform is read again). Empty for a sound that has none.
    waves: HashMap<i64, (NaiveDateTime, Peaks)>,
    /// The ids kept, the one kept longest first.
    order: VecDeque<i64>,
    /// Asked for and not back yet, each with when its file was last changed.
    asked: HashMap<i64, NaiveDateTime>,
    /// Asked for in this turn of the event loop, all read together once it ends.
    queued: Vec<Asked>,
    /// When a read last failed, while asks wait ([`RETRY_AFTER`]).
    failed: Option<Instant>,
}

impl Kept {
    /// The waveform kept for `s` (empty when it has none), unless its file has changed since.
    fn kept(&self, s: &Sound) -> Option<&Peaks> {
        self.waves.get(&s.id).filter(|(at, _)| *at == s.added).map(|(_, wave)| wave)
    }

    /// Asks for `s`'s waveform, unless it's asked for already (or a read failed a moment ago): true when it's the
    /// first ask of this turn of the event loop, whose end then sends them all.
    fn ask(&mut self, s: &Sound) -> bool {
        if self.failed.is_some_and(|at| at.elapsed() < RETRY_AFTER) || self.asked.get(&s.id) == Some(&s.added) {
            return false;
        }
        self.asked.insert(s.id, s.added);
        self.queued.push((s.id, s.added));
        self.queued.len() == 1
    }

    /// Reads from `db` from now on: what was kept from another library goes, and what was asked of it, but for
    /// `keeping`.
    fn read_from(&mut self, db: &Path, keeping: &[Asked]) {
        if self.db.as_deref() == Some(db) {
            return;
        }
        self.db = Some(db.to_owned());
        self.waves.clear();
        self.order.clear();
        let keeping: HashSet<i64> = keeping.iter().map(|a| a.0).collect();
        self.asked.retain(|id, _| keeping.contains(id));
        self.failed = None;
    }

    /// What was asked for in this turn, to be read from `db`.
    fn take_queued(&mut self, db: &Path) -> Vec<Asked> {
        let asked = std::mem::take(&mut self.queued);
        self.read_from(db, &asked);
        asked
    }

    /// Lets go of what was asked for in this turn: nothing would read it.
    fn drop_queued(&mut self) {
        for (id, _) in std::mem::take(&mut self.queued) {
            self.asked.remove(&id);
        }
    }

    /// Keeps what was read from `db` for the sounds `asked`: each one's waveform, or none for one it hasn't (never
    /// analysed, or gone). An answer to an ask made before its file changed is let go: the newer ask's answer
    /// follows. True when there's something new to show; when the read failed, they're asked for again as they show,
    /// once [`RETRY_AFTER`] has passed.
    fn arrived(&mut self, db: &Path, asked: &[Asked], read: Result<Vec<(i64, Peaks)>, String>) -> bool {
        if self.db.as_deref() != Some(db) {
            return false;
        }
        let still = |k: &Kept, (id, added): &Asked| k.asked.get(id) == Some(added);
        let mut waves: HashMap<i64, Peaks> = match read {
            Ok(waves) => waves.into_iter().collect(),
            Err(e) => {
                // Said once, until a read works again.
                if self.failed.is_none() {
                    log::warning("Sounds", &format!("Couldn't read the waveforms of the sounds on show: {e}"));
                }
                self.failed = Some(Instant::now());
                for a in asked {
                    if still(self, a) {
                        self.asked.remove(&a.0);
                    }
                }
                return false;
            }
        };
        self.failed = None;
        for a in asked {
            if still(self, a) {
                self.asked.remove(&a.0);
                self.keep(a.0, a.1, waves.remove(&a.0).unwrap_or_default());
            }
        }
        true
    }

    /// Keeps the waveforms `sounds` (read from `db`) came with, so they show on once their sounds come without.
    fn keep_own(&mut self, db: &Path, sounds: &[Sound]) {
        self.read_from(db, &[]);
        for s in sounds {
            if let Some(a) = s.analysis.as_ref().filter(|a| !a.waveform.is_empty()) {
                self.keep(s.id, s.added, a.waveform.clone());
            }
        }
    }

    fn keep(&mut self, id: i64, added: NaiveDateTime, wave: Peaks) {
        if self.waves.insert(id, (added, wave)).is_some() {
            return;
        }
        self.order.push_back(id);
        while self.order.len() > KEEP {
            if let Some(oldest) = self.order.pop_front() {
                self.waves.remove(&oldest);
            }
        }
    }
}

/// A sound whose waveform is asked for: its id, and when its file was last changed as the list said then.
type Asked = (i64, NaiveDateTime);

/// An ask for the waveforms of these sounds, from the library at `db`.
struct Ask {
    db: PathBuf,
    asked: Vec<Asked>,
}

thread_local! {
    static KEPT: RefCell<Kept> = RefCell::default();
    /// Hands asks to the thread that reads them, started at the first.
    static READER: RefCell<Option<mpsc::Sender<Ask>>> = const { RefCell::new(None) };
    static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// The window the waveforms show in once they're back.
pub(super) fn connect(app: &AppWindow) {
    APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
}

/// `s`'s waveform, given to `with`: its own, or the one read for it. `None` when it has none, or while it's read:
/// it's asked for now, and the page shows it once it's back ([`super::waves_arrived`]).
pub(super) fn with<T>(s: &Sound, with: impl FnOnce(&Peaks) -> T) -> Option<T> {
    let a = s.analysis.as_ref()?;
    if !a.waveform.is_empty() {
        return Some(with(&a.waveform));
    }
    KEPT.with(|k| {
        let mut k = k.borrow_mut();
        if let Some(wave) = k.kept(s) {
            return (!wave.is_empty()).then(|| with(wave));
        }
        // Sent once this turn of the event loop ends, with the others asked for meanwhile. Without an event loop
        // (nothing would ever come back), not asked for.
        if k.ask(s) && slint::invoke_from_event_loop(send_queued).is_err() {
            k.drop_queued();
        }
        None
    })
}

/// Keeps the waveforms the first sounds shown came with (read from `db`): the rest come without them, those sounds
/// among them, and they show on.
pub(super) fn keep_own(db: &Path, sounds: &[Sound]) {
    KEPT.with(|k| k.borrow_mut().keep_own(db, sounds));
}

/// Sends the waveforms asked for in this turn of the event loop to be read, from the library the page reads.
fn send_queued() {
    let db = PAGE.with(|p| p.borrow().galaxy.db.clone());
    let Some(db) = db else {
        KEPT.with(|k| k.borrow_mut().drop_queued());
        return;
    };
    let asked = KEPT.with(|k| k.borrow_mut().take_queued(&db));
    if asked.is_empty() {
        return;
    }
    let unsent = READER.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            *r = start_reader();
        }
        let ask = Ask { db: db.clone(), asked };
        match r.as_ref() {
            Some(reader) => reader.send(ask).err().map(|e| e.0.asked),
            None => Some(ask.asked),
        }
    });
    if let Some(asked) = unsent {
        // Nothing reads them: asked for again in a moment, as after a failed read.
        READER.with(|r| *r.borrow_mut() = None);
        arrived(&db, &asked, Err("its reader stopped".to_owned()));
    }
}

/// Starts the thread that reads the waveforms asked for, at low priority (rule 21).
fn start_reader() -> Option<mpsc::Sender<Ask>> {
    let (reader, asks) = mpsc::channel::<Ask>();
    match background::spawn("Sound waveforms", move || read_asked(&asks)) {
        Ok(_) => Some(reader),
        Err(e) => {
            log::warning("Sounds", &format!("Couldn't start reading the waveforms of the sounds on show: {e}"));
            None
        }
    }
}

/// Reads the waveforms asked for, each ask together with those waiting behind it, and sends them back to the window.
fn read_asked(asks: &mpsc::Receiver<Ask>) {
    while let Ok(ask) = asks.recv() {
        let mut together: Vec<Ask> = Vec::new();
        for ask in std::iter::once(ask).chain(asks.try_iter()) {
            match together.iter_mut().find(|t| t.db == ask.db) {
                Some(t) => t.asked.extend(ask.asked),
                None => together.push(ask),
            }
        }
        for Ask { db, asked } in together {
            let ids: Vec<i64> = asked.iter().map(|a| a.0).collect();
            // A damaged waveform stops only this read (one warning line in the log).
            let read = mo_catch::catch(|| read(&db, &ids)).unwrap_or_else(|_| Err("a waveform is damaged".to_owned()));
            let _ = slint::invoke_from_event_loop(move || arrived(&db, &asked, read));
        }
    }
}

/// The waveforms of the sounds with these ids in the library at `db`, opened for them alone (read-only, and closed
/// again at once: nothing holds it open while it's laid out anew or made again).
pub(super) fn read(db: &Path, ids: &[i64]) -> Result<Vec<(i64, Peaks)>, String> {
    SoundsDb::open_read_only(db).and_then(|db| db.waves(ids)).map_err(|e| e.to_string())
}

/// What was read for the sounds `asked` is back: the page shows their waveforms. When the read failed, it asks again
/// once [`RETRY_AFTER`] has passed, for what's on show then.
fn arrived(db: &Path, asked: &[Asked], read: Result<Vec<(i64, Peaks)>, String>) {
    let _doing = crate::watchdog::doing("Sounds: waveforms arrived");
    let failed = read.is_err();
    let ids: Vec<i64> = asked.iter().map(|a| a.0).collect();
    let show = move || {
        if let Some(app) = APP.with(|a| a.borrow().as_ref().and_then(slint::Weak::upgrade)) {
            super::waves_arrived(&app, &ids);
        }
    };
    if KEPT.with(|k| k.borrow_mut().arrived(db, asked, read)) {
        show();
    } else if failed {
        slint::Timer::single_shot(RETRY_AFTER + Duration::from_millis(50), show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_sounds::SoundAnalysis;

    fn sound(id: i64, wave: &[f32]) -> Sound {
        let mut s = Sound::new(id, &format!("D:\\Samples\\hit {id}.wav"));
        s.analysis = Some(SoundAnalysis { waveform: Peaks::new(wave), ..SoundAnalysis::default() });
        s
    }

    fn asked(sounds: &[&Sound]) -> Vec<Asked> {
        sounds.iter().map(|s| (s.id, s.added)).collect()
    }

    #[test]
    fn a_waveform_is_asked_for_once_and_kept_once_its_back() {
        let db = Path::new("sounds-rust.db");
        let mut k = Kept::default();
        let (a, b) = (sound(1, &[]), sound(2, &[]));
        assert!(k.kept(&a).is_none());
        // The first ask of the turn sends them all at its end; asking again changes nothing.
        assert!(k.ask(&a));
        assert!(!k.ask(&b) && !k.ask(&a));
        assert_eq!(k.take_queued(db), asked(&[&a, &b]));
        assert!(!k.ask(&a), "asked for already");
        // Back: the one with a waveform has it, the one without has none, and neither is asked for again.
        assert!(k.arrived(db, &asked(&[&a, &b]), Ok(vec![(1, Peaks::new(&[0.5, 1.0]))])));
        assert_eq!(k.kept(&a), Some(&Peaks::new(&[0.5, 1.0])));
        assert_eq!(k.kept(&b), Some(&Peaks::default()));
        assert!(k.asked.is_empty() && k.queued.is_empty());
        // Its file changed since (analysed again): read again.
        let mut changed = a.clone();
        changed.added += chrono::Duration::seconds(5);
        assert!(k.kept(&changed).is_none());
        assert!(k.ask(&changed));
        assert_eq!(k.take_queued(db), asked(&[&changed]));
        assert!(k.arrived(db, &asked(&[&changed]), Ok(vec![(1, Peaks::new(&[0.75]))])));
        assert_eq!(k.kept(&changed), Some(&Peaks::new(&[0.75])));
        // Another library's answer is no answer; a new library starts again.
        assert!(!k.arrived(Path::new("other.db"), &asked(&[&a]), Ok(vec![(1, Peaks::new(&[0.25]))])));
        assert!(k.ask(&a));
        assert_eq!(k.take_queued(Path::new("other.db")), asked(&[&a]));
        assert!(k.kept(&b).is_none());
        assert!(k.arrived(Path::new("other.db"), &asked(&[&a]), Ok(vec![(1, Peaks::new(&[0.25]))])));
        assert_eq!(k.kept(&a), Some(&Peaks::new(&[0.25])));
    }

    #[test]
    fn an_answer_to_an_ask_made_before_the_file_changed_is_let_go() {
        let db = Path::new("sounds-rust.db");
        let mut k = Kept::default();
        let before = sound(1, &[]);
        let mut after = before.clone();
        after.added += chrono::Duration::seconds(5);
        assert!(k.ask(&before));
        let first = k.take_queued(db);
        // Its file changed while the first read was under way: asked for again.
        assert!(k.ask(&after));
        let again = k.take_queued(db);
        assert!(k.arrived(db, &first, Ok(vec![(1, Peaks::new(&[0.5]))])));
        assert!(k.kept(&after).is_none() && k.kept(&before).is_none(), "the first answer is of the file before");
        assert!(k.arrived(db, &again, Ok(vec![(1, Peaks::new(&[1.0]))])));
        assert_eq!(k.kept(&after), Some(&Peaks::new(&[1.0])));
    }

    #[test]
    fn a_read_that_failed_is_tried_again_only_after_a_moment() {
        let db = Path::new("sounds-rust.db");
        let mut k = Kept::default();
        let a = sound(1, &[]);
        assert!(k.ask(&a));
        assert_eq!(k.take_queued(db), asked(&[&a]));
        assert!(!k.arrived(db, &asked(&[&a]), Err("locked".to_owned())));
        assert!(k.kept(&a).is_none());
        assert!(!k.ask(&a), "not straight away");
        k.failed = k.failed.and_then(|at| at.checked_sub(RETRY_AFTER));
        assert!(k.ask(&a));
        assert_eq!(k.take_queued(db), asked(&[&a]));
        assert!(k.arrived(db, &asked(&[&a]), Ok(vec![(1, Peaks::new(&[1.0]))])));
        assert!(k.failed.is_none());
        // Nothing to send them with: let go, so they're asked for again.
        let b = sound(2, &[]);
        assert!(k.ask(&b));
        k.drop_queued();
        assert!(k.ask(&b));
    }

    #[test]
    fn the_first_sounds_waveforms_are_kept_and_the_oldest_go_past_the_limit() {
        let db = Path::new("sounds-rust.db");
        let mut k = Kept::default();
        let first: Vec<Sound> = (0..KEEP as i64 + 10).map(|id| sound(id, &[0.5])).collect();
        k.keep_own(db, &first);
        assert_eq!((k.waves.len(), k.order.len()), (KEEP, KEEP));
        let without = |s: &Sound| {
            let mut s = s.clone();
            s.analysis.as_mut().unwrap().waveform = Peaks::default();
            s
        };
        assert!(k.kept(&without(&first[5])).is_none(), "among the first kept, so the first to go");
        assert_eq!(k.kept(&without(&first[KEEP + 9])), Some(&Peaks::new(&[0.5])));
    }
}
