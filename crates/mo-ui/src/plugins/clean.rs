//! The Plugins page's Clean up, on the side card's On disk tab: the picked plugin's preset and sound folders, which
//! may go to the Recycle Bin and why the others stay, then what went, with Put back. The checks are
//! `mo_space::plugin_clean`'s, run on worker threads (rule 12): the folders are read for samples your songs use before
//! the list shows, and every picked folder is checked again just before it goes. Nothing goes anywhere but the
//! Recycle Bin, and nothing is written to library.db: the page leaves out what went until the old app measures again.
//! Uninstall's card ([`super::uninstall`]) is shown the same way, in the same place.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_app::log;
use mo_app::plugins::clean::{self, Inputs, Watched};
use mo_app::plugins::uninstall;
use mo_library::Library;
use mo_space::clean::{Done, SystemBin, undo};
use mo_space::plugin_clean::{CleanLine, apply, lines};
use mo_space::plugin_files::FileLine;
use mo_space::size::size_text;
use mo_space::uninstallers::Uninstaller;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use super::{PAGE, fill_panel, refresh, shown};
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
        let card = feed.start(&format!("Cleaning up {}", c.name), "Sending to the Recycle Bin");
        Some((ask, library, picked, c.id, c.stop.clone(), c.row_key.clone(), c.name.clone(), card))
    });
    fill(app);
    let Some((ask, library, picked, id, stop, row_key, name, card)) = job else { return };
    log::info("Plugins", &format!("Clean up {name}: sending {} folders to the Recycle Bin", picked.len()));
    let weak = app.as_weak();
    let kept = kept();
    let spawned = mo_app::background::spawn("Plugins clean-up", move || {
        let inputs = mo_app::background::guarded(|| {
            Library::open_read_only(&library).and_then(|lib| ask.read(&lib, kept)).map_err(|e| e.to_string())
        })
        .and_then(|r| r);
        let outcome = match inputs {
            Err(why) => Outcome::NotChecked(clean::recheck_failed_text(&name, &why)),
            Ok(inputs) => {
                let bin = Watched::new(&SystemBin);
                // Not `guarded`: this changes things, so a stop part way says what went, not "nothing happened".
                let done = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
                    mo_app::background::on_bulk_pool(|| {
                        inputs.with(|l| apply(&picked, &inputs.ask.space, &inputs.ask.all, l, &bin, &stop))
                    })
                }));
                match done {
                    Ok(d) => Outcome::Done(d),
                    Err(_) => Outcome::Stopped(bin.went(&picked)),
                }
            }
        };
        report(card, &outcome);
        back(&weak, move |app| removed(app, id, row_key, outcome));
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
        Outcome::Stopped(_) => card.failed(clean::STOPPED_PART_WAY),
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

pub(super) fn removed(app: &AppWindow, id: u64, row_key: (String, String), outcome: Outcome) {
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
                (clean::STOPPED_PART_WAY.to_owned(), problems, d)
            }
        };
        log::info("Plugins", &format!("Clean up: {head}"));
        for line in &problems {
            log::warning("Plugins", &format!("Clean up kept {line}"));
        }
        if !done.removed.is_empty() {
            log::info(
                "Plugins",
                &format!("Clean up: {} folders in the Recycle Bin, {}", done.removed.len(), size_text(done.bytes())),
            );
            if let Some(pl) = p.plugins.as_mut() {
                pl.forget_removed(&done);
            }
            p.binned.push(Binned { row_key, done });
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
        let done = clean::joined(theirs.into_iter().map(|b| b.done).collect());
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
        let card = p.feed.start(&format!("Putting back what went from {name}"), "From the Recycle Bin");
        Some((done, row_key, p.clean_asked, card))
    });
    let Some((done, row_key, id, card)) = job else { return };
    app.global::<PluginsData>().set_tab(1);
    fill(app);
    let weak = app.as_weak();
    let (kept_done, kept_key) = (done.clone(), row_key.clone());
    let spawned = mo_app::background::spawn("Plugins put back", move || {
        let problems = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| undo(&done, &SystemBin)));
        let (head, lines, still) = match &problems {
            Ok(problems) => {
                let (head, lines) = clean::undone_text(&done, problems);
                (head, lines, clean::still_binned(&done, problems))
            }
            // What came back before it stopped isn't known: all of it stays listed, and a later Put back says which
            // aren't in the bin any more.
            Err(_) => (clean::PUT_BACK_STOPPED.to_owned(), Vec::new(), Some(done)),
        };
        if still.is_none() {
            card.done(&head)
        } else {
            card.failed(&head)
        }
        back(&weak, move |app| put_back_done(app, id, row_key, head, lines, still));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start putting back: {e}"));
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.binned.push(Binned { row_key: kept_key, done: kept_done });
            p.clean = None;
        });
        fill(app);
    }
}

fn put_back_done(
    app: &AppWindow,
    id: u64,
    row_key: (String, String),
    head: String,
    problems: Vec<String>,
    still: Option<Done>,
) {
    log::info("Plugins", &format!("Put back: {head}"));
    for line in &problems {
        log::warning("Plugins", &format!("Put back: {line}"));
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(done) = still {
            p.binned.push(Binned { row_key, done });
        }
        if let Some(c) = p.clean.as_mut().filter(|c| c.id == id) {
            c.stage = Stage::Done { head, problems, offer_clean: false };
        }
    });
    fill(app);
    // The library still lists them: a fresh read shows them again (less what's still binned).
    shown();
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
            Stage::Removing => ("Sending to the Recycle Bin…", true),
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
                data.set_clean_go_text("Send to the Recycle Bin".into());
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
                data.set_clean_go_text("Send to the Recycle Bin".into());
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
