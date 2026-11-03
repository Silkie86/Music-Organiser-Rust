//! The Space page, part 1: the drives your projects, samples, backup and the app's own files are on, how full each is,
//! and what the app's own files take (mo_app::space decides every word). It looks the first time the page shows, on a
//! low-priority thread, and again when it shows after five minutes or on Measure again, so nothing on the window's
//! thread waits for a disk (rule 12). Nothing here changes a file, and the library is only read.

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
use mo_app::{backup, log, paths};
use mo_space::app_files::{self, AppFilesPart};
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

/// What a look brings back, in two steps: the drives first (quick), then the app's files once they're measured.
enum Step {
    Drives(u64, Vec<Drive>),
    AppFiles(u64, Vec<AppFilesPart>),
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
    /// The picked line's id, or "" when none is.
    picked: String,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::new(Page::default());
}

/// Wires the page. It reads nothing until it first shows ([`shown`]): the library at `library` (read-only) for your
/// project and sample folders, and the backup's settings for its drive. Opening a folder or the Recycle Bin reports
/// on `feed`'s cards if it can't.
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
        p.shortcuts = shortcuts.clone();
    });
    let data = app.global::<SpaceData>();
    data.set_wins_note(space::WINS_COMING.into());
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

/// The page came on show: it looks the first time, and again once what it shows is five minutes old.
pub fn shown(app: &AppWindow) {
    let stale = PAGE.with(|p| {
        let p = p.borrow();
        p.feed.is_some() && p.looked.is_none_or(|at| at.elapsed() >= FRESH)
    });
    if stale {
        look_again(app);
    }
}

/// Runs one of the page's own shortcuts ([`mo_app::shortcuts::SPACE_ACTIONS`]). Returns whether it did anything,
/// so an unused key goes on.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let (measuring, picked) = PAGE.with(|p| {
        let p = p.borrow();
        (p.measuring, p.picked.clone())
    });
    match id {
        MEASURE if !measuring => chosen(app, MEASURE),
        REVEAL if picked == space::APP_FILES => chosen(app, REVEAL),
        _ => return false,
    }
    true
}

/// A button on the page or its card.
fn chosen(app: &AppWindow, id: &str) {
    let feed = PAGE.with(|p| p.borrow().feed.clone());
    match (id, feed) {
        (MEASURE, _) => look_again(app),
        (REVEAL, Some(feed)) => crate::files::open_folder(&feed, &paths::data_dir().to_string_lossy()),
        (RECYCLE_BIN, Some(feed)) => crate::files::open_recycle_bin(&feed),
        _ => {}
    }
}

/// The buttons with their words, each with its tooltip and key from the shortcut list (rule 15). Measure again
/// waits while a look is measuring.
fn actions(p: &Page, buttons: &[(&str, &str)]) -> ModelRc<CardAction> {
    let actions: Vec<CardAction> = buttons
        .iter()
        .map(|&(id, text)| CardAction {
            text: text.into(),
            tip: what_of(id).into(),
            key: p.shortcuts.key_of(id).into(),
            id: id.into(),
            enabled: !(id == MEASURE && p.measuring),
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

/// The folders to look at: your project and sample folders from the library (read-only), the backup drive from the
/// backup's settings, and the app's own folder.
fn folders_of(library: &Path, app_folder: &Path) -> Vec<(Holds, PathBuf)> {
    let (mut roots, mut samples) = (Vec::new(), Vec::new());
    if library.exists() {
        match mo_library::Library::open_read_only(library) {
            Ok(lib) => {
                roots = lib.roots().unwrap_or_else(|e| {
                    log::warning("Space", &format!("Couldn't read the project folders: {e}"));
                    Vec::new()
                });
                samples = lib.list_setting("sample_folders").unwrap_or_else(|e| {
                    log::warning("Space", &format!("Couldn't read the sample folders: {e}"));
                    Vec::new()
                });
            }
            Err(e) => log::warning("Space", &format!("Couldn't read the library for its folders: {e}")),
        }
    }
    let backup_drive = backup::Settings::load(&app_folder.join(backup::SETTINGS_FILE)).drive;
    space::folders(&roots, &samples, backup_drive.as_deref(), app_folder)
}

/// A look's step reaches the window's thread, unless a newer look has been asked for since.
fn arrive(weak: &slint::Weak<AppWindow>, step: Step) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        let _doing = crate::watchdog::doing("Space: a look arrived");
        let current = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            match step {
                Step::Drives(id, drives) if id == p.asked => p.drives = drives,
                Step::AppFiles(id, parts) if id == p.asked => {
                    p.parts = Some(parts);
                    p.measuring = false;
                }
                _ => return false,
            }
            true
        });
        if current {
            show(&app);
        }
    });
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
        let line = space::app_files_line(p.parts.as_deref(), app_drive.map(|d| d.name.as_str()));
        let also = vec![RoomLine {
            id: line.id.into(),
            title: line.title.into(),
            why: line.why.into(),
            size: line.size.into(),
        }];
        data.set_also(ModelRc::new(VecModel::from(also)));
        data.set_actions(actions(&p, &[(MEASURE, "Measure again"), (RECYCLE_BIN, "Open Recycle Bin")]));
    });
    show_card(app);
}

/// Shows which line is picked and its card; picking a line or closing its card changes only this.
fn show_card(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_picked(p.picked.as_str().into());
        if p.picked != space::APP_FILES {
            return;
        }
        let card = space::app_files_card(p.parts.as_deref(), space::app_drive(&p.drives));
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
        data.set_card_room(card.room.into());
        data.set_card_warning(card.warning.into());
        data.set_card_actions(actions(&p, &[(REVEAL, "Show in folder"), (MEASURE, "Measure again")]));
    });
}

/// Shows made-up drives and app files, without reading any disk: for the screenshots.
pub fn show_sample(app: &AppWindow, drives: Vec<Drive>, parts: Vec<AppFilesPart>, picked: &str) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.drives = drives;
        p.parts = Some(parts);
        p.picked = picked.to_owned();
        // Shown now, so showing the page doesn't look at the real drives.
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
