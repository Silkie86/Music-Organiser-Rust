//! The Space page: the drives your projects, samples, backup and the app's own files are on, how full each is, what
//! the app's own files take (part 1), and Bitwig's copies of Library multisamples in your projects (part 2). mo_app's
//! `space` and `space_bitwig` decide every word. It looks the first time the page shows, on low-priority threads, and
//! again when it shows after five minutes or on Measure again, so nothing on the window's thread waits for a disk
//! (rule 12). Nothing here changes a file, and the library is only read.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::places::Place;
use mo_app::shortcuts::{Shortcuts, what_of};
use mo_app::space::{self, Drive, Fill, Holds};
use mo_app::space_bitwig::{self, BITWIG_COPIES, Copies};
use mo_app::{backup, log, paths};
use mo_space::app_files::{self, AppFilesPart};
use mo_space::bitwig::LibraryIndex;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AppWindow, CardAction, CardPart, DriveCard, RoomLine, SpaceData};

/// How long what the page shows stays fresh: shown again after this, it looks again (the C# app looked every five
/// minutes).
const FRESH: Duration = Duration::from_secs(5 * 60);

/// The buttons' ids, from the one shortcut list (rule 15): Measure again and Show in folder are the page's own
/// (`mo_app::shortcuts::SPACE_ACTIONS`); the Recycle Bin is the app-wide action, so its button shows the same key.
const MEASURE: &str = "space.measure";
const REVEAL: &str = "space.reveal";
const RECYCLE_BIN: &str = "app.recycleBin";

/// One look at the drives and the app's files.
struct Ask {
    id: u64,
    library: PathBuf,
    app_folder: PathBuf,
    stop: Arc<AtomicBool>,
}

/// One look for Bitwig copies. It reads every copy, far more than a look at the drives, so it has a thread of its own
/// and is only asked for on the page's first look, on Measure again, and while no look has finished: never on the
/// five-minute look.
struct BitwigAsk {
    id: u64,
    library: PathBuf,
    stop: Arc<AtomicBool>,
    feed: Feed,
}

/// What a look brings back, a step at a time: the drives first (quick), then the app's files once they're measured;
/// and from the look for Bitwig copies, how far it is, then what it found.
enum Step {
    Drives(u64, Vec<Drive>),
    AppFiles(u64, Vec<AppFilesPart>),
    BitwigLooking(u64, (usize, usize)),
    Bitwig(u64, Copies),
}

#[derive(Default)]
struct Page {
    library: PathBuf,
    /// Set once the page is wired; it looks at nothing before.
    feed: Option<Feed>,
    shortcuts: Shortcuts,
    worker: Option<Newest<Ask>>,
    /// When the newest look was asked for: none before the page first shows.
    looked: Option<Instant>,
    /// Counts the looks, so an answer to an older one that arrives late is dropped.
    asked: u64,
    /// Stops the look still measuring when a newer one is asked for.
    stop: Arc<AtomicBool>,
    /// Whether a look is still measuring the app's files: Measure again waits for it.
    measuring: bool,
    drives: Vec<Drive>,
    /// The app's own files, once measured. A new look keeps the last sizes on show until its own are in.
    parts: Option<Vec<AppFilesPart>>,
    /// The look for Bitwig copies, as the look at the drives above: its thread, how many were asked for, and its stop.
    bitwig_worker: Option<Newest<BitwigAsk>>,
    bitwig_asked: u64,
    bitwig_stop: Arc<AtomicBool>,
    /// What the last look for Bitwig copies found; it stays on show while the next one looks.
    bitwig: Option<Copies>,
    /// While a look for Bitwig copies is on: the project folders looked through, of how many (0 while they're still
    /// being found). Measure again waits for it too.
    bitwig_looking: Option<(usize, usize)>,
    /// The picked line's id, or "" when none is.
    picked: String,
}

impl Page {
    /// Whether a look is still on, so Measure again waits.
    fn busy(&self) -> bool {
        self.measuring || self.bitwig_looking.is_some()
    }
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::new(Page::default());
}

/// Wires the page. It reads nothing until it first shows ([`shown`]): the library at `library` (read-only) for your
/// project and sample folders, and the backup's settings for its drive. Opening a folder or the Recycle Bin reports
/// on `feed`'s cards if it can't, and the look for Bitwig copies shows how far it is there.
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
        p.shortcuts = shortcuts.clone();
    });
    let data = app.global::<SpaceData>();
    let weak = app.as_weak();
    data.on_pick(move |id| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().picked = id.to_string());
        show_card(&app);
    });
    let weak = app.as_weak();
    data.on_close_card(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().picked.clear());
        show_card(&app);
    });
    let weak = app.as_weak();
    data.on_chosen(move |id| {
        let Some(app) = weak.upgrade() else { return };
        chosen(&app, &id);
    });
    show(app);
    // The app may start on this page, before it was wired.
    if app.get_place() == Place::Space.index() as i32 {
        shown(app);
    }
}

/// The page came on show: it looks the first time, and again once what it shows is five minutes old. It looks for
/// Bitwig copies the first time, and again only while no look for them has finished.
pub fn shown(app: &AppWindow) {
    let (stale, bitwig) = PAGE.with(|p| {
        let p = p.borrow();
        let wired = p.feed.is_some();
        let stale = wired && p.looked.is_none_or(|at| at.elapsed() >= FRESH);
        (stale, wired && p.bitwig.is_none() && p.bitwig_looking.is_none())
    });
    if stale {
        look_again(app);
    }
    if bitwig {
        look_for_copies(app);
    }
}

/// Runs one of the page's own shortcuts ([`mo_app::shortcuts::SPACE_ACTIONS`]). Returns whether it did anything,
/// so an unused key goes on.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let (busy, picked) = PAGE.with(|p| {
        let p = p.borrow();
        (p.busy(), p.picked.clone())
    });
    match id {
        MEASURE if !busy => chosen(app, MEASURE),
        REVEAL if picked == space::APP_FILES => chosen(app, REVEAL),
        _ => return false,
    }
    true
}

/// A button on the page or its card.
fn chosen(app: &AppWindow, id: &str) {
    let feed = PAGE.with(|p| p.borrow().feed.clone());
    match (id, feed) {
        (MEASURE, _) => {
            look_again(app);
            look_for_copies(app);
        }
        (REVEAL, Some(feed)) => crate::files::open_folder(&feed, &paths::data_dir().to_string_lossy()),
        (RECYCLE_BIN, Some(feed)) => crate::files::open_recycle_bin(&feed),
        _ => {}
    }
}

/// The buttons with their words, each with its tooltip and key from the shortcut list (rule 15). Measure again
/// waits while a look is on.
fn actions(p: &Page, buttons: &[(&str, &str)]) -> ModelRc<CardAction> {
    let actions: Vec<CardAction> = buttons
        .iter()
        .map(|&(id, text)| CardAction {
            text: text.into(),
            tip: what_of(id).into(),
            key: p.shortcuts.key_of(id).into(),
            id: id.into(),
            enabled: !(id == MEASURE && p.busy()),
        })
        .collect();
    ModelRc::new(VecModel::from(actions))
}

/// Asks for a new look at the drives and the app's files, on the page's own low-priority thread. One still measuring
/// stops: the newest wins (rule 12).
fn look_again(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.stop.store(true, Ordering::Relaxed);
        p.stop = Arc::new(AtomicBool::new(false));
        p.asked += 1;
        p.looked = Some(Instant::now());
        p.measuring = true;
        Ask { id: p.asked, library: p.library.clone(), app_folder: paths::data_dir(), stop: p.stop.clone() }
    });
    let weak = app.as_weak();
    let started = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.worker.is_none() {
            p.worker = Some(Newest::start("Space", move |ask: Ask| look(ask, &weak))?);
        }
        if let Some(worker) = &p.worker {
            worker.ask(ask);
        }
        Ok::<(), std::io::Error>(())
    });
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking at the drives: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.looked = None;
            p.measuring = false;
        });
    }
    show(app);
}

/// One look, off the window's thread: the drives your folders are on, then the app's own files.
fn look(ask: Ask, weak: &slint::Weak<AppWindow>) {
    let folders = folders_of(&ask.library, &ask.app_folder);
    let drives = space::drives(&folders, space::look);
    arrive(weak, Step::Drives(ask.id, drives));
    let data = &ask.app_folder;
    let databases = app_files::databases(data.join("library.db"), data.join("sounds.db"), data.join("midi.db"));
    // Spread over the low-priority pool's threads (rule 21). A look stopped for a newer one says nothing: the newer
    // one's sizes follow.
    if let Ok(parts) = background::on_pool(|| app_files::measure(data, &databases, &ask.stop)) {
        arrive(weak, Step::AppFiles(ask.id, parts));
    }
}

/// Asks for a new look for Bitwig copies, on the page's second low-priority thread. One still looking stops: the
/// newest wins (rule 12).
fn look_for_copies(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let feed = p.feed.clone()?;
        p.bitwig_stop.store(true, Ordering::Relaxed);
        p.bitwig_stop = Arc::new(AtomicBool::new(false));
        p.bitwig_asked += 1;
        p.bitwig_looking = Some((0, 0));
        Some(BitwigAsk { id: p.bitwig_asked, library: p.library.clone(), stop: p.bitwig_stop.clone(), feed })
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    let started = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.bitwig_worker.is_none() {
            // The Library as the last look read it, kept on the thread so a file isn't read again while it's unchanged.
            let mut index = None;
            let work = move |ask: BitwigAsk| look_for_copies_now(ask, &mut index, &weak);
            p.bitwig_worker = Some(Newest::start("Space Bitwig", work)?);
        }
        if let Some(worker) = &p.bitwig_worker {
            worker.ask(ask);
        }
        Ok::<(), std::io::Error>(())
    });
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking for Bitwig copies: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().bitwig_looking = None);
    }
    show(app);
}

/// One look for Bitwig copies, off the window's thread: the project folders from the library (read-only), then each
/// project's copies, with an activity card saying how far it is.
fn look_for_copies_now(ask: BitwigAsk, index: &mut Option<LibraryIndex>, weak: &slint::Weak<AppWindow>) {
    let roots: Vec<PathBuf> = project_roots(&ask.library).into_iter().map(PathBuf::from).collect();
    let documents = space_bitwig::documents();
    let job = ask.feed.start(space_bitwig::LOOKING, "");
    // Spread over the low-priority pool's threads (rule 21).
    let found = background::on_pool(|| {
        space_bitwig::look(&roots, documents.as_deref(), index, &ask.stop, |done, of| {
            if of > 0 {
                job.progress(done as f32 / of as f32, Some(&format!("{done} of {of} projects")));
            }
            arrive(weak, Step::BitwigLooking(ask.id, (done, of)));
        })
    });
    match found {
        Ok(copies) => {
            job.done(&space_bitwig::found_words(&copies));
            arrive(weak, Step::Bitwig(ask.id, copies));
        }
        // Stopped for a newer look, whose own card follows.
        Err(_) => job.done_as("Stopped looking for Bitwig copies", "A newer look took its place"),
    }
}

/// The library, opened read-only for the folders it knows; None when there's none yet or it can't be read (said in
/// the log).
fn open_library(library: &Path) -> Option<mo_library::Library> {
    if !library.exists() {
        return None;
    }
    mo_library::Library::open_read_only(library)
        .map_err(|e| log::warning("Space", &format!("Couldn't read the library for its folders: {e}")))
        .ok()
}

/// Your project folders, as the library lists them.
fn roots_in(lib: &mo_library::Library) -> Vec<String> {
    lib.roots().unwrap_or_else(|e| {
        log::warning("Space", &format!("Couldn't read the project folders: {e}"));
        Vec::new()
    })
}

/// Your project folders, from the library (read-only).
fn project_roots(library: &Path) -> Vec<String> {
    open_library(library).as_ref().map(roots_in).unwrap_or_default()
}

/// The folders to look at: your project and sample folders from the library (read-only), the backup drive from the
/// backup's settings, and the app's own folder.
fn folders_of(library: &Path, app_folder: &Path) -> Vec<(Holds, PathBuf)> {
    let lib = open_library(library);
    let roots = lib.as_ref().map(roots_in).unwrap_or_default();
    let samples = lib
        .as_ref()
        .map(|lib| {
            lib.list_setting("sample_folders").unwrap_or_else(|e| {
                log::warning("Space", &format!("Couldn't read the sample folders: {e}"));
                Vec::new()
            })
        })
        .unwrap_or_default();
    let backup_drive = backup::Settings::load(&app_folder.join(backup::SETTINGS_FILE)).drive;
    space::folders(&roots, &samples, backup_drive.as_deref(), app_folder)
}

/// A look's step reaches the window's thread, unless a newer look has been asked for since.
fn arrive(weak: &slint::Weak<AppWindow>, step: Step) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        let _doing = crate::watchdog::doing("Space: a look arrived");
        // How much to show again: all of it, only the Bitwig copies line and card (how far a look is), or nothing
        // (an older look's step).
        let (all, wins) = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            match step {
                Step::Drives(id, drives) if id == p.asked => p.drives = drives,
                Step::AppFiles(id, parts) if id == p.asked => {
                    p.parts = Some(parts);
                    p.measuring = false;
                }
                Step::BitwigLooking(id, at) if id == p.bitwig_asked => {
                    p.bitwig_looking = Some(at);
                    return (false, true);
                }
                Step::Bitwig(id, copies) if id == p.bitwig_asked => {
                    // A card whose line has gone (none found this time) closes.
                    if copies.projects.is_empty() && p.picked == BITWIG_COPIES {
                        p.picked.clear();
                    }
                    p.bitwig = Some(copies);
                    p.bitwig_looking = None;
                }
                _ => return (false, false),
            }
            (true, false)
        });
        if all {
            show(&app);
        } else if wins {
            show_wins(&app);
        }
    });
}

fn room_line(line: space::Line) -> RoomLine {
    RoomLine { id: line.id.into(), title: line.title.into(), why: line.why.into(), size: line.size.into() }
}

/// Shows what the page knows now: the summary, the drives, the lists and the picked line's card.
pub fn show(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let app_bytes = p.parts.as_deref().map_or(0, space::app_bytes);
        data.set_summary(space::summary(&p.drives, app_bytes).into());
        let cards: Vec<DriveCard> = p
            .drives
            .iter()
            .map(|d| DriveCard {
                name: d.name.as_str().into(),
                room: space::room_line(d).into(),
                used: d.used(),
                holds: space::holds_line(d).into(),
                note: space::fill_note(d, app_bytes).into(),
                fill: match d.fill(app_bytes) {
                    Fill::Roomy => 0,
                    Fill::Nearly => 1,
                    Fill::Almost => 2,
                },
                here: d.here,
            })
            .collect();
        data.set_drives(ModelRc::new(VecModel::from(cards)));
        let app_drive = space::app_drive(&p.drives);
        let also = vec![room_line(space::app_files_line(p.parts.as_deref(), app_drive.map(|d| d.name.as_str())))];
        data.set_also(ModelRc::new(VecModel::from(also)));
        data.set_actions(actions(&p, &[(MEASURE, "Measure again"), (RECYCLE_BIN, "Open Recycle Bin")]));
    });
    show_wins(app);
}

/// Shows Biggest wins first and the picked line's card: all that changes while a look for Bitwig copies goes on.
fn show_wins(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let wins: Vec<RoomLine> =
            space_bitwig::line(p.bitwig.as_ref(), p.bitwig_looking).into_iter().map(room_line).collect();
        data.set_wins(ModelRc::new(VecModel::from(wins)));
        data.set_wins_note(space_bitwig::wins_note(p.bitwig.as_ref()).into());
    });
    show_card(app);
}

/// Shows which line is picked and its card; picking a line or closing its card changes only this.
fn show_card(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_picked(p.picked.as_str().into());
        let (card, buttons): (space::Card, &[(&str, &str)]) = match p.picked.as_str() {
            space::APP_FILES => (
                space::app_files_card(p.parts.as_deref(), space::app_drive(&p.drives)),
                &[(REVEAL, "Show in folder"), (MEASURE, "Measure again")],
            ),
            BITWIG_COPIES => (space_bitwig::card(p.bitwig.as_ref(), p.bitwig_looking), &[(MEASURE, "Measure again")]),
            _ => return,
        };
        data.set_card_kind(card.kind.into());
        data.set_card_title(card.title.into());
        data.set_card_big(card.big.into());
        data.set_card_big_note(card.big_note.into());
        data.set_card_lead(card.lead.into());
        let parts: Vec<CardPart> = card
            .parts
            .into_iter()
            .map(|(name, what, size)| CardPart { name: name.into(), what: what.into(), size: size.into() })
            .collect();
        data.set_card_parts(ModelRc::new(VecModel::from(parts)));
        data.set_card_note(card.note.into());
        data.set_card_room(card.room.into());
        data.set_card_warning(card.warning.into());
        data.set_card_actions(actions(&p, buttons));
    });
}

/// Shows made-up drives, app files and Bitwig copies, without reading any disk: for the screenshots.
pub fn show_sample(app: &AppWindow, drives: Vec<Drive>, parts: Vec<AppFilesPart>, bitwig: Copies, picked: &str) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.drives = drives;
        p.parts = Some(parts);
        p.bitwig = Some(bitwig);
        p.picked = picked.to_owned();
        // Shown now, so showing the page doesn't look at the real drives or projects.
        p.looked = Some(Instant::now());
    });
    show(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_is_in_the_shortcut_list() {
        for id in [MEASURE, REVEAL, RECYCLE_BIN] {
            assert_ne!(what_of(id), "", "{id} isn't in the shortcut list");
        }
        let keys = Shortcuts::default();
        assert_eq!(keys.page_action_for("F5", mo_app::shortcuts::SPACE_ACTIONS), Some(MEASURE));
        assert_eq!(keys.page_action_for("Ctrl+Shift+E", mo_app::shortcuts::SPACE_ACTIONS), Some(REVEAL));
    }
}
