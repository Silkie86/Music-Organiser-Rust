//! The Plugins page's Clean up, on the side card's On disk tab: the picked plugin's preset and sound folders, which
//! may go to the Recycle Bin and why the others stay, then what went, with Put back. The checks are
//! `mo_space::plugin_clean`'s, run on worker threads (rule 12): the folders are read for samples your songs use before
//! the list shows, and every picked folder is checked again just before it goes. Nothing goes anywhere but the
//! Recycle Bin: what went is kept in the app's list of what's there (`mo_app::binned`, shared with Space), and every
//! read leaves out what isn't there now (nothing is written to library.db).
//! Uninstall's card ([`super::uninstall`]) is shown the same way, in the same place.

use mo_app::space_bin::BIN;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mo_app::binned::Kept;
use mo_app::log;
use mo_app::plugins::clean::{self, Inputs, Watched};
use mo_app::plugins::uninstall;
use mo_app::space_bin::{PLUGIN_CLEAN, PLUGIN_UNINSTALL};
use mo_library::Library;
use mo_space::clean::{Done, SystemBin, undo};
use mo_space::plugin_clean::{CleanLine, apply, lines};
use mo_space::plugin_files::FileLine;
use mo_space::size::size_text;
use mo_space::uninstallers::Uninstaller;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use super::{PAGE, fill_panel, refresh};
use crate::{AppWindow, CleanRow, PluginsData};

/// A clean-up or uninstall under way, for the plugin with this row key (name key and own).
pub(super) struct Cleaning {
    pub(super) row_key: (String, String),
    pub(super) name: String,
    /// The card's title, and what it warns of first ("" for nothing).
    pub(super) title: String,
    pub(super) warning: String,
    /// Counts the cards, so a result for one closed meanwhile is dropped.
    pub(super) id: u64,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) stage: Stage,
}

pub(super) enum Stage {
    Checking,
    Ready {
        inputs: Arc<Inputs>,
        lines: Vec<CleanLine>,
        ticks: Vec<bool>,
    },
    Removing,
    /// What happened; `offer_clean`: Clean up is offered next (for what an uninstaller left).
    Done {
        head: String,
        problems: Vec<String>,
        offer_clean: bool,
    },
    PuttingBack,
    /// Uninstall: looking for its uninstaller.
    Finding,
    /// Uninstall: which uninstaller to run (`None` until you pick, when they look like another version's).
    Pick {
        intro: String,
        found: Vec<Uninstaller>,
        /// Which of them look like another version's.
        others: Vec<bool>,
        files: Vec<String>,
        picked: Option<usize>,
    },
    /// Uninstall without an uninstaller: its plugin files, to tick.
    Files {
        intro: String,
        lines: Vec<FileLine>,
        ticks: Vec<bool>,
    },
    /// Uninstall: its uninstaller is open, and the app waits for the plugin's files to go.
    Waiting,
    /// Move: which folder to move.
    MovePick {
        intro: String,
        ask: Arc<mo_app::plugins::moving::Ask>,
        picked: Option<usize>,
        /// The folder picker is open: the folder picked stays as it is until it closes.
        choosing: bool,
    },
    /// Move: checking the folder can go where you picked.
    MoveChecking,
    /// Move: ready to move `folder` (`bytes`, measured just now) to `destination`.
    MoveReady {
        text: String,
        ask: Arc<mo_app::plugins::moving::Ask>,
        folder: usize,
        bytes: u64,
        destination: String,
    },
    /// Move: copying, checking, then swapping in the link.
    Moving,
}

impl Stage {
    pub(super) fn done(head: String) -> Stage {
        Stage::Done { head, problems: Vec::new(), offer_clean: false }
    }
}

/// What a clean-up sent to the Recycle Bin this session and wasn't put back: the page leaves it out of every read.
pub(super) struct Binned {
    pub(super) row_key: (String, String),
    pub(super) done: Done,
    /// What it's kept under in the app's list of what's in the bin ([`record`]): its kind and when, if it was kept.
    pub(super) kept: Option<(String, i64)>,
}

/// The Plugins page's clean-ups and uninstalls sending things to the Recycle Bin now, each by what stops it: as the app
/// closes, each is stopped and given a little time to keep what went ([`finish_sending`], review P-1).
static SENDING: Mutex<Vec<Arc<AtomicBool>>> = Mutex::new(Vec::new());
static SENDING_ENDED: Condvar = Condvar::new();

/// The longest the app waits, as it closes, for a clean-up it stopped to keep what went (as Space's Removes).
const CLOSE_WAIT: Duration = Duration::from_secs(15);

/// What a clean-up's things in the bin are kept under (kind and when), and the app's list of them now.
type KeptAs = ((String, i64), Kept);

/// One clean-up or uninstall sending to the bin, in [`SENDING`] until it's done with, however it ends.
struct OnItsWay(Arc<AtomicBool>);

impl OnItsWay {
    fn new(stop: Arc<AtomicBool>) -> Self {
        SENDING.lock().unwrap_or_else(PoisonError::into_inner).push(stop.clone());
        OnItsWay(stop)
    }
}

impl Drop for OnItsWay {
    fn drop(&mut self) {
        let mut sending = SENDING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = sending.iter().position(|stop| Arc::ptr_eq(stop, &self.0)) {
            sending.swap_remove(at);
        }
        SENDING_ENDED.notify_all();
    }
}

/// As the window closes, first: a clean-up or uninstall still sending to the bin stops after what's going now.
pub(super) fn stop_sending() {
    for stop in SENDING.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        stop.store(true, Ordering::Relaxed);
    }
}

/// As the app closes: waits a little for a clean-up told to stop ([`stop_sending`]) to keep what went, for Put back.
/// One that doesn't end in time (Windows' administrator step may still be waiting for you) noted what went as it went
/// ([`Sent::note`]): the next look keeps it.
pub(super) fn finish_sending() {
    // Again: one may have started since the window closed.
    stop_sending();
    let mut sending = SENDING.lock().unwrap_or_else(PoisonError::into_inner);
    if sending.is_empty() {
        return;
    }
    log::warning(
        "Plugins",
        &format!("The app closed while a clean-up was sending to the {BIN}: it stops (what went stays there)"),
    );
    let deadline = Instant::now() + CLOSE_WAIT;
    while !sending.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            log::warning(
                "Plugins",
                &format!("A clean-up didn't stop in time: what went to the {BIN} was noted as it went, for Put back"),
            );
            return;
        }
        sending = SENDING_ENDED.wait_timeout(sending, left).unwrap_or_else(PoisonError::into_inner).0;
    }
}

/// A clean-up or uninstall of the plugin at `row_key` on its way to the bin: when it started, and its note of each thing
/// the moment it goes (`mo_app::binned::Going`), so a close or a crash part way still leaves what went on the disk, to
/// be kept by the next look with its Put back (review P-1).
pub(super) struct Sent {
    kind: &'static str,
    row_key: (String, String),
    when: i64,
    note: mo_app::binned::Going,
    _on_its_way: OnItsWay,
}

impl Sent {
    /// Starts one, stopped by `stop` (as the app closes too).
    pub(super) fn start(kind: &'static str, row_key: &(String, String), stop: &Arc<AtomicBool>) -> Sent {
        let when = mo_app::binned::now();
        let note = mo_app::binned::Going::start_for(&mo_app::paths::data_dir(), kind, when, Some(row_key.clone()));
        Sent { kind, row_key: row_key.clone(), when, note, _on_its_way: OnItsWay::new(stop.clone()) }
    }

    /// Runs `work` with the bin to send through: the system's, each thing noted as it goes.
    pub(super) fn send<T>(&self, work: impl FnOnce(&Watched<'_>) -> T) -> T {
        let goes = |removed: &mo_space::clean::Removed| self.note.went(removed);
        let hear = |_: &std::path::Path, _: &Result<(), String>| {};
        let heard = mo_space::clean::Heard::new(&SystemBin, &hear).noting(&goes);
        work(&Watched::new(&heard))
    }

    /// What it noted going so far, as the bin has each: what went when it stopped on a bug, the same as its note (so
    /// the next look doesn't take the note in as another clean-up).
    pub(super) fn went_so_far(&self) -> Done {
        Done { removed: self.note.went_so_far(), ..Done::default() }
    }

    /// It ended (`outcome`): what's in the bin is kept in the app's list ([`record`]), and its note goes once that's
    /// kept. After a crash the note stays for the next look, as Windows' administrator step goes on by itself.
    pub(super) fn keep(self, outcome: &Outcome) -> Option<KeptAs> {
        let (kept, keeps) = record(self.kind, &self.row_key, outcome, self.when);
        let crashed = matches!(outcome, Outcome::Stopped(_));
        self.note.finish(!crashed && keeps);
        kept
    }
}

/// Keeps what a clean-up or an uninstall of the plugin at `row_key` sent to the Recycle Bin in the app's list, as Space's
/// clean-ups are (`mo_app::binned`), under `when` (as its note: [`Sent`]): Space's Recycle Bin line counts it, and its
/// Put back is here after a restart too (review X1-3). What it's kept under and the list now, or `None` when nothing
/// went to the bin or it couldn't be kept (said in the log); and whether it's all kept, so its note may go. Looks in
/// the bin and writes a file: call it off the window's thread.
fn record(kind: &str, row_key: &(String, String), outcome: &Outcome, when: i64) -> (Option<KeptAs>, bool) {
    let (Outcome::Done(done) | Outcome::Stopped(done)) = outcome else { return (None, true) };
    let in_bin = clean::in_the_bin(done);
    if in_bin.removed.is_empty() {
        return (None, true);
    }
    let folder = mo_app::paths::data_dir();
    let kept = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
        mo_app::binned::record_for(&folder, &SystemBin, kind, when, &in_bin, Some(row_key.clone()))
    }));
    match kept {
        Ok((kept, at)) => {
            let noted = kept.noted;
            if !noted {
                log::warning(
                    "Plugins",
                    &format!(
                        "What went to the {BIN} couldn't be kept in its list: Put back works until the app closes"
                    ),
                );
            }
            (Some(((kind.to_owned(), at), kept)), noted)
        }
        Err(_) => (None, false),
    }
}

/// What the app keeps of the Recycle Bin now, from any thread: `mo_app::binned::refresh`, or `None` when it stopped on
/// a crash (said in the log). Looks in the bin: call it off the window's thread.
fn kept_now() -> Option<Kept> {
    let folder = mo_app::paths::data_dir();
    mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| mo_app::binned::refresh(&folder, &SystemBin))).ok()
}

/// Once a run, on a worker: the Plugins page's clean-ups and uninstalls from earlier runs that are still in the Recycle
/// Bin, as the app's list keeps them, so their Put back is on their plugin's card (review X1-3).
pub(super) fn load_binned(app: &AppWindow) {
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins binned", move || {
        let Some(kept) = kept_now() else { return };
        let mine = mine(&kept.list);
        back(&weak, move |app| {
            crate::space::bin_changed(app, kept);
            if mine.is_empty() {
                return;
            }
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                for b in mine {
                    if !p.binned.iter().any(|o| o.kept.is_some() && o.kept == b.kept) {
                        if let Some(pl) = p.plugins.as_mut() {
                            pl.forget_removed(&b.done);
                        }
                        p.binned.push(b);
                    }
                }
            });
            refresh(app);
            fill(app);
        });
    });
    if let Err(e) = spawned {
        log::warning("Plugins", &format!("Couldn't read what earlier clean-ups put in the Recycle Bin: {e}"));
    }
}

/// This page's clean-ups and uninstalls in the app's list of what's in the bin (`list`), each under its plugin.
fn mine(list: &[mo_app::binned::Binned]) -> Vec<Binned> {
    list.iter()
        .filter(|b| b.kind == PLUGIN_CLEAN || b.kind == PLUGIN_UNINSTALL)
        .filter_map(|b| {
            let row_key = b.plugin.clone()?;
            Some(Binned { row_key, done: b.done.clone(), kept: Some((b.kind.clone(), b.when)) })
        })
        .collect()
}

/// This page's clean-ups and uninstalls in the app's list of what's in the bin (`kept`), when that list can be gone by
/// for what's still there: it was read from its file and the bin could be looked in. Otherwise `None` (the list may
/// hold only some of them, or all of them whatever came back), and the page keeps what it knows.
pub(crate) fn trusted(kept: &Kept) -> Option<Vec<mo_app::binned::Binned>> {
    (kept.noted && kept.in_bin.looked)
        .then(|| kept.list.iter().filter(|b| b.kind == PLUGIN_CLEAN || b.kind == PLUGIN_UNINSTALL).cloned().collect())
}

/// Brings the page's list of what its clean-ups sent to the bin in step with the app's list of what's there (`list`,
/// just looked at, after a Put back here or on Space): each one kept there now holds only what's still in the bin, and
/// goes once nothing of it is, so what came back shows on the page again (review P-5). One that couldn't be kept in
/// that list stays as it is.
pub(super) fn sync_binned(binned: &mut Vec<Binned>, list: &[mo_app::binned::Binned]) {
    binned.retain_mut(|b| {
        let Some((kind, when)) = &b.kept else { return true };
        match list.iter().find(|l| &l.kind == kind && l.when == *when && !l.done.removed.is_empty()) {
            Some(l) => {
                b.done = l.done.clone();
                true
            }
            None => false,
        }
    });
}

/// What's left of the plugin at `row_key` in the bin after its Put back: what the app's list (`list`) still keeps of
/// it, under the same keys (so a later Put back on Space finds them, review P-5), and anything else of `still` (what
/// didn't come back) that list doesn't hold. Without the list, all of `still`.
fn left_after(row_key: &(String, String), still: Option<Done>, list: Option<&[mo_app::binned::Binned]>) -> Vec<Binned> {
    let Some(list) = list else {
        return still.into_iter().map(|done| Binned { row_key: row_key.clone(), done, kept: None }).collect();
    };
    let mut left: Vec<Binned> =
        mine(list).into_iter().filter(|b| &b.row_key == row_key && !b.done.removed.is_empty()).collect();
    if let Some(mut rest) = still {
        let held: std::collections::HashSet<&str> =
            left.iter().flat_map(|b| b.done.removed.iter().map(|r| r.path.as_str())).collect();
        rest.removed.retain(|r| !held.contains(r.path.as_str()));
        if !rest.removed.is_empty() {
            left.push(Binned { row_key: row_key.clone(), done: rest, kept: None });
        }
    }
    left
}

impl Cleaning {
    pub(super) fn busy(&self) -> bool {
        matches!(self.stage, Stage::Removing | Stage::PuttingBack | Stage::Waiting | Stage::Moving)
    }
}

/// How a clean-up went.
pub(super) enum Outcome {
    Done(Done),
    /// The library couldn't be read again just before: nothing went.
    NotChecked(String),
    /// It stopped part way on a bug: what had gone by then.
    Stopped(Done),
}

/// Whether the plugin at `row` has anything Clean up could look at ([`mo_app::plugins::Plugins::can_clean`]).
pub(super) fn can_clean(row: usize) -> bool {
    PAGE.with(|p| p.borrow().plugins.as_ref().is_some_and(|pl| pl.can_clean(row)))
}

/// What Clean up looks at: the data folder is the app's own, so it never goes.
fn kept() -> Vec<String> {
    vec![mo_app::paths::data_dir().to_string_lossy().into_owned()]
}

/// Clean up for the picked plugin: shows the On disk tab and checks its folders on a worker.
pub(super) fn start(app: &AppWindow) {
    let started = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if held(&p) {
            return None;
        }
        let ask = p.plugins.as_ref()?.clean_ask(p.picked?)?;
        if let Some(old) = p.clean.take() {
            old.stop.store(true, Ordering::Relaxed);
        }
        p.clean_asked += 1;
        let stop = Arc::new(AtomicBool::new(false));
        p.clean = Some(Cleaning {
            row_key: ask.row_key.clone(),
            name: ask.name.clone(),
            title: format!("Clean up {}", ask.name),
            warning: String::new(),
            id: p.clean_asked,
            stop: stop.clone(),
            stage: Stage::Checking,
        });
        Some((ask, p.clean_asked, stop, p.library.clone()))
    });
    let Some((ask, id, stop, library)) = started else { return };
    app.global::<PluginsData>().set_tab(1);
    fill(app);
    let weak = app.as_weak();
    let kept = kept();
    let spawned = mo_app::background::spawn("Plugins clean-up check", move || {
        // A long walk of the disk: on the bulk pool, so the pages' quick jobs don't wait behind it.
        let found = mo_app::background::guarded(|| {
            mo_app::background::on_bulk_pool(|| -> Result<(Inputs, Vec<CleanLine>), String> {
                let lib = Library::open_read_only(&library).map_err(|e| e.to_string())?;
                let inputs = ask.read(&lib, kept).map_err(|e| e.to_string())?;
                drop(lib);
                let found = inputs
                    .with(|l| lines(&inputs.ask.space, &inputs.ask.all, l, &stop))
                    .map_err(|_| "Stopped.".to_owned())?;
                Ok((inputs, found))
            })
        })
        .and_then(|r| r);
        back(&weak, move |app| checked(app, id, found));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start the clean-up check: {e}"));
        PAGE.with(|p| p.borrow_mut().clean = None);
        fill(app);
    }
}

pub(super) fn back(weak: &Weak<AppWindow>, f: impl FnOnce(&AppWindow) + Send + 'static) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            f(&app);
        }
    });
}

fn checked(app: &AppWindow, id: u64, found: Result<(Inputs, Vec<CleanLine>), String>) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(c) = p.clean.as_mut().filter(|c| c.id == id) else { return };
        match found {
            Ok((inputs, lines)) => {
                let ticks = lines.iter().map(|l| l.ticked).collect();
                c.stage = Stage::Ready { inputs: Arc::new(inputs), lines, ticks };
            }
            Err(why) => {
                log::warning("Plugins", &format!("Clean up {}: couldn't check its folders: {why}", c.name));
                c.stage = Stage::done(clean::check_failed_text(&c.name, &why));
            }
        }
    });
    fill(app);
}

/// A line ticked or unticked (for Uninstall's uninstallers, the one picked).
pub(super) fn tick(app: &AppWindow, at: usize, on: bool) {
    let refill = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(c) = p.clean.as_mut() else { return false };
        match &mut c.stage {
            Stage::Ready { lines, ticks, .. } => {
                set_tick(ticks, at, on, lines.get(at).is_some_and(|l| l.stays.is_none()));
                false
            }
            Stage::Files { lines, ticks, .. } => {
                set_tick(ticks, at, on, lines.get(at).is_some_and(|l| l.stays.is_none()));
                false
            }
            // One at a time: the others untick.
            Stage::Pick { found, picked, .. } => pick_one(picked, at, on, found.len()),
            Stage::MovePick { ask, picked, choosing: false, .. } => pick_one(picked, at, on, ask.folders.len()),
            _ => false,
        }
    });
    if refill {
        fill(app);
        return;
    }
    // Only the words under the lines: the lines stay as they are, so the box clicked keeps its place.
    let data = app.global::<PluginsData>();
    PAGE.with(|p| {
        if let Some(c) = p.borrow().clean.as_ref()
            && let Some((freed, can_go)) = go_state(&c.stage)
        {
            data.set_clean_freed(freed.into());
            data.set_clean_can_go(can_go);
        }
    });
}

/// Ticks or unticks line `at`, when it may go.
fn set_tick(ticks: &mut [bool], at: usize, on: bool, may_go: bool) {
    if let Some(t) = ticks.get_mut(at).filter(|_| may_go) {
        *t = on;
    }
}

/// The words under the lines and whether the main button may be pressed, for a card with lines.
fn go_state(stage: &Stage) -> Option<(String, bool)> {
    match stage {
        Stage::Ready { lines, ticks, .. } => {
            Some((clean::freed_text(lines, ticks), !clean::picked(lines, ticks).is_empty()))
        }
        Stage::Files { lines, ticks, .. } => {
            Some((uninstall::files_freed_text(lines, ticks), uninstall::ticked_files(lines, ticks).next().is_some()))
        }
        Stage::Pick { picked, .. } => Some((String::new(), picked.is_some())),
        Stage::MovePick { picked, choosing, .. } => Some((String::new(), picked.is_some() && !choosing)),
        Stage::MoveReady { .. } => Some((String::new(), true)),
        _ => None,
    }
}

/// Sends the ticked lines to the Recycle Bin on a worker. Just before, it reads again what the library knows (your
/// folders, the samples your projects use) and takes how the page sees the plugins now (the years you picked, a
/// new read), so each folder is checked against things as they are when it goes, not when the card opened.
pub(super) fn confirm(app: &AppWindow) {
    let (uninstalling, moving) = PAGE.with(|p| {
        let p = p.borrow();
        let stage = p.clean.as_ref().map(|c| &c.stage);
        (
            matches!(stage, Some(Stage::Pick { .. } | Stage::Files { .. })),
            matches!(stage, Some(Stage::MovePick { .. } | Stage::MoveReady { .. })),
        )
    });
    if uninstalling {
        super::uninstall::confirm(app);
        return;
    }
    if moving {
        super::moving::confirm(app);
        return;
    }
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let feed = p.feed.clone();
        let library = p.library.clone();
        let ask = p.clean.as_ref().and_then(|c| {
            let pl = p.plugins.as_ref()?;
            pl.clean_ask(pl.row_of(&c.row_key)?)
        });
        let c = p.clean.as_mut()?;
        let Stage::Ready { lines, ticks, .. } = &c.stage else { return None };
        let picked = clean::picked(lines, ticks);
        if picked.is_empty() {
            return None;
        }
        let Some(ask) = ask else {
            c.stage = Stage::done(format!("{} isn't on the page as it was any more. Nothing was removed.", c.name));
            return None;
        };
        c.stage = Stage::Removing;
        let card = feed.start(&format!("Cleaning up {}", c.name), &format!("Sending to the {BIN}"));
        Some((ask, library, picked, c.id, c.stop.clone(), c.row_key.clone(), c.name.clone(), card))
    });
    fill(app);
    let Some((ask, library, picked, id, stop, row_key, name, card)) = job else { return };
    log::info("Plugins", &format!("Clean up {name}: sending {} folders to the Recycle Bin", picked.len()));
    let weak = app.as_weak();
    let kept = kept();
    // Started before its worker, so the window closing straight after still finds it, to stop and wait for.
    let sent = Sent::start(PLUGIN_CLEAN, &row_key, &stop);
    let spawned = mo_app::background::spawn("Plugins clean-up", move || {
        let inputs = mo_app::background::guarded(|| {
            Library::open_read_only(&library).and_then(|lib| ask.read(&lib, kept)).map_err(|e| e.to_string())
        })
        .and_then(|r| r);
        let outcome = match inputs {
            Err(why) => Outcome::NotChecked(clean::recheck_failed_text(&name, &why)),
            Ok(inputs) => sent.send(|bin| {
                // Not `guarded`: this changes things, so a stop part way says what went, not "nothing happened".
                let done = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
                    mo_app::background::on_bulk_pool(|| {
                        inputs.with(|l| apply(&picked, &inputs.ask.space, &inputs.ask.all, l, bin, &stop))
                    })
                }));
                match done {
                    Ok(d) => Outcome::Done(d),
                    Err(_) => Outcome::Stopped(sent.went_so_far()),
                }
            }),
        };
        report(card, &outcome);
        let kept = sent.keep(&outcome);
        back(&weak, move |app| removed(app, id, row_key, outcome, kept));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start the clean-up: {e}"));
        PAGE.with(|p| p.borrow_mut().clean = None);
        fill(app);
    }
}

/// Says on its activity card how sending to the Recycle Bin went.
pub(super) fn report(card: mo_app::activity::Job, outcome: &Outcome) {
    match outcome {
        Outcome::Done(d) if !d.removed.is_empty() => card.done(&clean::done_text(d).0),
        Outcome::Done(d) => card.failed(d.problems.first().map_or("Nothing was removed.", |p| p.why.as_str())),
        Outcome::NotChecked(why) => card.failed(why),
        Outcome::Stopped(_) => card.failed(&clean::stopped_part_way()),
    }
}

/// Picks line `at` of `len` (the others untick), or unpicks it. Says the lines need drawing again.
fn pick_one(picked: &mut Option<usize>, at: usize, on: bool, len: usize) -> bool {
    if on && at < len {
        *picked = Some(at);
    } else if *picked == Some(at) {
        *picked = None;
    }
    true
}

/// Whether a card that's working (sending, putting back, waiting for an uninstaller) holds the page: then nothing new
/// starts, and an activity card says why.
pub(super) fn held(p: &super::Page) -> bool {
    let Some(c) = p.clean.as_ref().filter(|c| c.busy()) else { return false };
    p.feed.start(&format!("{} is still under way", c.title.trim_end_matches('?')), "").failed(
        "One clean-up, uninstall or move at a time: this starts once it's done (or once you press Stop on it).",
    );
    true
}

pub(super) fn removed(
    app: &AppWindow,
    id: u64,
    row_key: (String, String),
    outcome: Outcome,
    recorded: Option<((String, i64), Kept)>,
) {
    let (kept, list) = recorded.map_or((None, None), |(at, list)| (Some(at), Some(list)));
    if let Some(list) = list {
        crate::space::bin_changed(app, list);
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let (head, problems, done) = match outcome {
            Outcome::Done(d) => {
                let (head, problems) = clean::done_text(&d);
                (head, problems, d)
            }
            Outcome::NotChecked(why) => (why, Vec::new(), Done::default()),
            Outcome::Stopped(d) => {
                let problems = d.problems.iter().map(|p| format!("{}: {}", p.path, p.why)).collect();
                (clean::stopped_part_way(), problems, d)
            }
        };
        log::info("Plugins", &format!("Clean up: {head}"));
        for line in &problems {
            log::warning("Plugins", &format!("Clean up kept {line}"));
        }
        if !done.removed.is_empty() {
            log::info(
                "Plugins",
                &format!("Clean up: {} folders in the {BIN}, {}", done.removed.len(), size_text(done.bytes())),
            );
            if let Some(pl) = p.plugins.as_mut() {
                pl.forget_removed(&done);
            }
            p.removed_for_good.extend(clean::removed_for_good(&done));
            let in_bin = clean::in_the_bin(&done);
            if !in_bin.removed.is_empty() {
                p.binned.push(Binned { row_key, done: in_bin, kept });
            }
        }
        if let Some(c) = p.clean.as_mut().filter(|c| c.id == id) {
            c.stage = Stage::Done { head, problems, offer_clean: false };
        }
    });
    refresh(app);
    fill_panel(app);
}

/// Puts back everything this session's clean-ups of the picked plugin sent to the Recycle Bin. What doesn't come back
/// stays listed, to try again.
pub(super) fn put_back(app: &AppWindow) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if held(&p) {
            return None;
        }
        let r = p.plugins.as_ref()?.rows.get(p.picked?)?;
        let (row_key, name) = ((r.key.clone(), r.own.clone()), r.name.clone());
        let (theirs, others): (Vec<Binned>, Vec<Binned>) =
            std::mem::take(&mut p.binned).into_iter().partition(|b| b.row_key == row_key);
        p.binned = others;
        if theirs.is_empty() {
            return None;
        }
        let done = clean::joined(theirs.iter().map(|b| b.done.clone()).collect());
        p.clean_asked += 1;
        p.clean = Some(Cleaning {
            row_key: row_key.clone(),
            name: name.clone(),
            title: format!("Put back what went from {name}"),
            warning: String::new(),
            id: p.clean_asked,
            stop: Arc::new(AtomicBool::new(false)),
            stage: Stage::PuttingBack,
        });
        let card = p.feed.start(&format!("Putting back what went from {name}"), &format!("From the {BIN}"));
        Some((done, theirs, row_key, p.clean_asked, card))
    });
    let Some((done, theirs, row_key, id, card)) = job else { return };
    app.global::<PluginsData>().set_tab(1);
    fill(app);
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins put back", move || {
        let problems = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| undo(&done, &SystemBin)));
        // What came back isn't in the bin any more: the app's list of what's there drops it (Space's line too).
        let kept = kept_now();
        let list = kept.as_ref().and_then(trusted);
        let (head, lines, still) = match &problems {
            Ok(problems) => {
                let (head, lines) = clean::undone_text(&done, problems);
                (head, lines, clean::still_binned(&done, problems))
            }
            // What came back before it stopped isn't known: all of it stays listed, and a later Put back says which
            // aren't in the bin any more.
            Err(_) => (clean::put_back_stopped(), Vec::new(), Some(done)),
        };
        if still.is_none() {
            card.done(&head)
        } else {
            card.failed(&head)
        }
        back(&weak, move |app| {
            if let Some(kept) = kept {
                crate::space::bin_changed(app, kept);
            }
            let left = left_after(&row_key, still, list.as_deref());
            put_back_done(app, id, head, lines, left);
        });
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start putting back: {e}"));
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            // Nothing was tried: they stay listed as they were, under their keys.
            p.binned.extend(theirs);
            p.clean = None;
        });
        fill(app);
    }
}

fn put_back_done(app: &AppWindow, id: u64, head: String, problems: Vec<String>, left: Vec<Binned>) {
    log::info("Plugins", &format!("Put back: {head}"));
    for line in &problems {
        log::warning("Plugins", &format!("Put back: {line}"));
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // A look that came back meanwhile ([`load_binned`]) may have listed some again already.
        for b in left {
            if b.kept.is_none() || !p.binned.iter().any(|o| o.kept == b.kept) {
                p.binned.push(b);
            }
        }
        if let Some(c) = p.clean.as_mut().filter(|c| c.id == id) {
            c.stage = Stage::Done { head, problems, offer_clean: false };
        }
    });
    fill(app);
    // The library still lists them: a fresh read shows them again (less what's still binned).
    super::read_again();
}

/// Stop: a check closes; sending to the Recycle Bin stops after the folder going now (what went stays in the bin, with
/// Put back); waiting for an uninstaller stops waiting (it carries on in its own window).
pub(super) fn stop(app: &AppWindow) {
    let checking = PAGE.with(|p| {
        let p = p.borrow();
        let Some(c) = p.clean.as_ref() else { return false };
        c.stop.store(true, Ordering::Relaxed);
        matches!(c.stage, Stage::Checking | Stage::Finding | Stage::MoveChecking)
    });
    if checking {
        close(app);
    }
}

/// Closes the clean-up (Keep everything, Done). One under way keeps going: only its words go.
pub(super) fn close(app: &AppWindow) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.clean.as_ref().is_some_and(Cleaning::busy) {
            return;
        }
        if let Some(c) = p.clean.take() {
            c.stop.store(true, Ordering::Relaxed);
        }
    });
    fill(app);
}

/// A new plugin was picked: a clean-up for another one that isn't under way closes.
pub(super) fn picked_now(key: Option<(String, String)>) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.clean.as_ref().is_some_and(|c| !c.busy() && Some(&c.row_key) != key.as_ref())
            && let Some(c) = p.clean.take()
        {
            c.stop.store(true, Ordering::Relaxed);
        }
    });
}

/// Leaves out of a fresh read what this session's clean-ups sent to the Recycle Bin, and the plugin files this
/// session's uninstalls removed.
pub(super) fn forget_binned(plugins: &mut mo_app::plugins::Plugins) {
    PAGE.with(|p| {
        let p = p.borrow();
        for b in &p.binned {
            plugins.forget_removed(&b.done);
        }
        plugins.forget_paths(&p.uninstalled);
        plugins.forget_paths(&p.removed_for_good);
        plugins.forget_paths(&p.moved);
    });
}

/// Shows the clean-up (or none) for the picked plugin.
pub(super) fn fill(app: &AppWindow) {
    let data = app.global::<PluginsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let picked_key = p.picked.and_then(|r| p.plugins.as_ref()?.rows.get(r)).map(|r| (r.key.clone(), r.own.clone()));
        let binned: Vec<&Binned> = p.binned.iter().filter(|b| Some(&b.row_key) == picked_key.as_ref()).collect();
        data.set_binned_text(clean::binned_text(&binned.iter().map(|b| &b.done).collect::<Vec<_>>()).into());
        let c = p.clean.as_ref().filter(|c| Some(&c.row_key) == picked_key.as_ref());
        data.set_clean_shown(c.is_some());
        let Some(c) = c else { return };
        data.set_clean_title(c.title.as_str().into());
        data.set_clean_warning(c.warning.as_str().into());
        data.set_clean_stage(match c.stage {
            Stage::Checking | Stage::Finding | Stage::MoveChecking => 0,
            Stage::Ready { .. } | Stage::Pick { .. } | Stage::Files { .. } => 1,
            Stage::MovePick { .. } | Stage::MoveReady { .. } => 1,
            Stage::Removing | Stage::Waiting | Stage::Moving => 2,
            Stage::Done { .. } => 3,
            Stage::PuttingBack => 4,
        });
        let (busy, can_stop) = match c.stage {
            Stage::Checking => ("Checking the folders for samples your songs use…", true),
            Stage::Finding => ("Looking for its uninstaller…", true),
            Stage::Removing => {
                (if cfg!(windows) { "Sending to the Recycle Bin…" } else { "Sending to the Trash…" }, true)
            }
            Stage::Waiting => ("Its uninstaller is open in its own window. Waiting for its files to go…", true),
            Stage::PuttingBack => ("Putting back…", false),
            Stage::MoveChecking => ("Checking it can move there…", true),
            Stage::Moving => (
                "Copying to the other drive and checking every file, then leaving a link where it was. Stop works \
                 until the link goes in; nothing changes here before then.",
                true,
            ),
            _ => ("", false),
        };
        data.set_clean_busy_text(busy.into());
        data.set_clean_can_stop(can_stop);
        let model = |rows: Vec<CleanRow>| ModelRc::new(VecModel::from(rows));
        match &c.stage {
            Stage::Ready { inputs, lines, ticks } => {
                data.set_clean_intro(clean::intro(inputs, lines).into());
                let rows: Vec<CleanRow> = clean::rows(lines, inputs)
                    .into_iter()
                    .zip(ticks)
                    .map(|(r, t)| CleanRow {
                        path: mo_app::shell::wrappable_path(&r.path).into(),
                        kind: r.kind.into(),
                        size: r.size.into(),
                        why: r.why.into(),
                        can: r.can,
                        ticked: *t && r.can,
                    })
                    .collect();
                data.set_clean_rows(model(rows));
                data.set_clean_go_text(format!("Send to the {BIN}").into());
                data.set_clean_keep_text("Keep everything".into());
            }
            Stage::Files { intro, lines, ticks, .. } => {
                data.set_clean_intro(intro.as_str().into());
                let rows: Vec<CleanRow> = lines
                    .iter()
                    .zip(ticks)
                    .map(|(l, t)| CleanRow {
                        path: mo_app::shell::wrappable_path(&l.path).into(),
                        kind: uninstall::file_kind(l).into(),
                        size: size_text(l.bytes).into(),
                        why: l.stays.clone().unwrap_or_default().into(),
                        can: l.stays.is_none(),
                        ticked: *t && l.stays.is_none(),
                    })
                    .collect();
                data.set_clean_rows(model(rows));
                data.set_clean_go_text(format!("Send to the {BIN}").into());
                data.set_clean_keep_text("Keep it".into());
            }
            Stage::Pick { intro, found, others, picked, .. } => {
                data.set_clean_intro(intro.as_str().into());
                let rows: Vec<CleanRow> = found
                    .iter()
                    .enumerate()
                    .map(|(i, u)| {
                        let (name, detail) = uninstall::uninstaller_text(u);
                        // The maker and size under the name, where a long one wraps.
                        CleanRow {
                            path: detail.into(),
                            kind: name.into(),
                            size: SharedString::new(),
                            why: if others.get(i).copied().unwrap_or(false) {
                                uninstall::OTHER_VERSION.into()
                            } else {
                                SharedString::new()
                            },
                            can: true,
                            ticked: *picked == Some(i),
                        }
                    })
                    .collect();
                data.set_clean_rows(model(rows));
                data.set_clean_go_text("Uninstall".into());
                data.set_clean_keep_text("Keep it".into());
            }
            Stage::MovePick { intro, ask, picked, .. } => {
                data.set_clean_intro(intro.as_str().into());
                let rows: Vec<CleanRow> = ask
                    .folders
                    .iter()
                    .enumerate()
                    .map(|(i, f)| CleanRow {
                        path: mo_app::shell::wrappable_path(&f.path).into(),
                        kind: mo_app::plugins::moving::folder_kind(f).into(),
                        size: size_text(f.bytes).into(),
                        why: SharedString::new(),
                        can: true,
                        ticked: *picked == Some(i),
                    })
                    .collect();
                data.set_clean_rows(model(rows));
                data.set_clean_go_text("Pick where it goes…".into());
                data.set_clean_keep_text("Keep it here".into());
            }
            Stage::MoveReady { text, .. } => {
                data.set_clean_intro(text.as_str().into());
                data.set_clean_rows(model(Vec::new()));
                data.set_clean_go_text("Move".into());
                data.set_clean_keep_text("Keep it here".into());
            }
            Stage::Done { head, problems, offer_clean } => {
                data.set_clean_intro(SharedString::new());
                data.set_clean_result(head.as_str().into());
                data.set_clean_problems(ModelRc::new(VecModel::from(
                    problems.iter().map(|s| SharedString::from(s.as_str())).collect::<Vec<_>>(),
                )));
                data.set_clean_can_put_back(!binned.is_empty());
                data.set_clean_offer_clean(*offer_clean);
            }
            _ => data.set_clean_intro(SharedString::new()),
        }
        if let Some((freed, can_go)) = go_state(&c.stage) {
            data.set_clean_freed(freed.into());
            data.set_clean_can_go(can_go);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_space::clean::{Removed, What};
    use mo_space::plugin_space::SpaceKind;

    fn gone(path: &str) -> Removed {
        Removed {
            path: path.to_owned(),
            real: path.to_owned(),
            bytes: 1,
            what: What::Plugin(SpaceKind::Presets),
            when: 5,
            bin_id: Some(String::new()),
            binned: None,
            binned_zone: None,
            stamp: None,
        }
    }

    fn done(paths: &[&str]) -> Done {
        Done { removed: paths.iter().map(|p| gone(p)).collect(), ..Done::default() }
    }

    fn row() -> (String, String) {
        ("omnisphere".to_owned(), String::new())
    }

    fn listed(when: i64, paths: &[&str]) -> mo_app::binned::Binned {
        mo_app::binned::Binned { kind: PLUGIN_CLEAN.to_owned(), when, done: done(paths), plugin: Some(row()) }
    }

    fn paths(b: &Binned) -> Vec<&str> {
        b.done.removed.iter().map(|r| r.path.as_str()).collect()
    }

    #[test]
    fn a_put_back_on_space_that_brought_some_back_shows_those_again() {
        let mut binned = vec![
            Binned { row_key: row(), done: done(&["a", "b", "c"]), kept: Some((PLUGIN_CLEAN.to_owned(), 5)) },
            Binned { row_key: row(), done: done(&["d"]), kept: Some((PLUGIN_CLEAN.to_owned(), 6)) },
            Binned { row_key: row(), done: done(&["e"]), kept: None },
        ];
        // a and b came back from the first; all of the second came back.
        sync_binned(&mut binned, &[listed(5, &["c"])]);
        assert_eq!(binned.len(), 2);
        assert_eq!(paths(&binned[0]), ["c"]);
        assert_eq!(binned[0].kept, Some((PLUGIN_CLEAN.to_owned(), 5)));
        assert_eq!(paths(&binned[1]), ["e"], "one the app's list never held stays as it was");
    }

    #[test]
    fn what_stays_after_a_put_back_here_keeps_its_keys() {
        let list = [listed(5, &["c"]), listed(7, &[])];
        let left = left_after(&row(), Some(done(&["c", "x"])), Some(&list));
        assert_eq!(left.len(), 2, "an emptied one goes");
        assert_eq!(left[0].kept, Some((PLUGIN_CLEAN.to_owned(), 5)));
        assert_eq!(paths(&left[0]), ["c"]);
        assert_eq!(paths(&left[1]), ["x"], "what didn't come back and the list doesn't hold stays, without a key");
        assert_eq!(left[1].kept, None);
        let left = left_after(&row(), Some(done(&["c"])), None);
        assert_eq!((left.len(), left[0].kept.clone()), (1, None), "without the list, all of what's left, as before");
        assert!(left_after(&row(), None, Some(&[])).is_empty());
    }
}
