//! The Plugins page's Move to another drive, on the side card's On disk tab, in Clean up's place
//! ([`super::clean`]): pick one of the plugin's preset or sound folders, then where on another drive it goes. It's
//! checked there first, then copied, every file checked, and swapped for a link (`mo_space::folder_move`), all on a
//! worker. Then the move is added to the library's moved folders (`mo_library::moves`), so the old app and Space know
//! of it and its old copy.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_app::log;
use mo_app::plugins::moving::{self, Ask};
use mo_library::Library;
use mo_library::moves::{MOVED_FOLDERS, add_moved_folder};
use mo_space::folder_move::{CantMove, SystemDrives, move_folder, problem};
use slint::ComponentHandle;

use super::clean::{self, Cleaning, Stage, back, fill};
use super::{PAGE, fill_panel, refresh};
use crate::{AppWindow, PluginsData};

/// Whether the plugin at `row` has a folder that could move ([`mo_app::plugins::Plugins::can_move`]).
pub(super) fn can_move(row: usize) -> bool {
    PAGE.with(|p| p.borrow().plugins.as_ref().is_some_and(|pl| pl.can_move(row)))
}

/// Move for the picked plugin: its folders on the On disk tab, the biggest picked.
pub(super) fn start(app: &AppWindow) {
    let started = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if clean::held(&p) {
            return None;
        }
        let ask = p.plugins.as_ref()?.move_ask(p.picked?)?;
        if let Some(old) = p.clean.take() {
            old.stop.store(true, Ordering::Relaxed);
        }
        p.clean_asked += 1;
        let intro = ask.intro();
        p.clean = Some(Cleaning {
            row_key: ask.row_key.clone(),
            name: ask.name.clone(),
            title: ask.title(),
            warning: String::new(),
            id: p.clean_asked,
            stop: Arc::new(AtomicBool::new(false)),
            stage: Stage::MovePick { intro, picked: (!ask.folders.is_empty()).then_some(0), ask: Arc::new(ask) },
        });
        Some(())
    });
    if started.is_some() {
        app.global::<PluginsData>().set_tab(1);
        fill(app);
    }
}

/// The card's main button: pick where the folder goes, or move it.
pub(super) fn confirm(app: &AppWindow) {
    let what = PAGE.with(|p| match p.borrow().clean.as_ref().map(|c| &c.stage) {
        Some(Stage::MovePick { ask, picked: Some(at), .. }) => Some((true, ask.clone(), *at, String::new())),
        Some(Stage::MoveReady { ask, folder, destination, .. }) => {
            Some((false, ask.clone(), *folder, destination.clone()))
        }
        _ => None,
    });
    let Some((picking, ask, at, destination)) = what else { return };
    if picking {
        let weak = app.as_weak();
        crate::backup::pick_folder("Pick where the folder goes (on another drive)", move |path| {
            if let Some(app) = weak.upgrade() {
                check(&app, ask, at, path.to_string_lossy().into_owned());
            }
        });
    } else {
        go(app, ask, at, destination);
    }
}

/// Reads your folders and the moves made already, as the library keeps them now.
fn library_now(library: &Path) -> Result<(Vec<String>, Vec<mo_space::folder_move::MovedFolder>), String> {
    let lib = Library::open_read_only(library).map_err(|e| e.to_string())?;
    let protected = lib.protected_roots().map_err(|e| e.to_string())?;
    let moved = moving::moved_now(lib.setting(MOVED_FOLDERS).map_err(|e| e.to_string())?.as_deref())?;
    Ok((protected, moved))
}

/// Checks the folder can move where you picked, on a worker.
fn check(app: &AppWindow, ask: Arc<Ask>, at: usize, destination: String) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let library = p.library.clone();
        let c = p.clean.as_mut().filter(|c| matches!(c.stage, Stage::MovePick { .. }))?;
        c.stage = Stage::MoveChecking;
        Some((c.id, c.stop.clone(), library))
    });
    let Some((id, stop, library)) = job else { return };
    fill(app);
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins move check", move || {
        let found = mo_app::background::guarded(|| -> Result<Option<CantMove>, String> {
            let (protected, moved) = library_now(&library)?;
            let Some(f) = ask.folders.get(at) else { return Ok(Some(CantMove::Gone)) };
            Ok(problem(&f.path, &destination, f.bytes, &SystemDrives, &moved, &protected, &stop))
        })
        .and_then(|r| r);
        back(&weak, move |app| checked(app, id, ask, at, destination, found));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start the move check: {e}"));
        PAGE.with(|p| p.borrow_mut().clean = None);
        fill(app);
    }
}

fn checked(
    app: &AppWindow,
    id: u64,
    ask: Arc<Ask>,
    at: usize,
    destination: String,
    found: Result<Option<CantMove>, String>,
) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(c) = p.clean.as_mut().filter(|c| c.id == id) else { return };
        c.stage = match (found, ask.folders.get(at)) {
            (Ok(None), Some(f)) => {
                let to = mo_space::folder_move::target(&f.path, &destination).to_string_lossy().into_owned();
                Stage::MoveReady { text: moving::ready_text(&f.path, &to, f.bytes), ask, folder: at, destination }
            }
            (Ok(Some(why)), _) => Stage::done(moving::cant_text(&why)),
            (Ok(None), None) => Stage::done(moving::cant_text(&CantMove::Gone)),
            (Err(why), _) => Stage::done(format!("Couldn't check it can move there: {why}")),
        };
    });
    fill(app);
}

/// Moves the folder on a worker, then adds the move to the library's moved folders.
fn go(app: &AppWindow, ask: Arc<Ask>, at: usize, destination: String) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let library = p.library.clone();
        let feed = p.feed.clone();
        let c = p.clean.as_mut().filter(|c| matches!(c.stage, Stage::MoveReady { .. }))?;
        let f = ask.folders.get(at)?.clone();
        c.stage = Stage::Moving;
        let card = feed.start(&format!("Moving {}", mo_formats::paths::file_name(&f.path)), "To the other drive");
        Some((c.id, c.stop.clone(), library, f, card))
    });
    let Some((id, stop, library, folder, card)) = job else { return };
    fill(app);
    log::info("Plugins", &format!("Move {}: {} to {destination}", ask.name, folder.path));
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins move", move || {
        // Not `guarded`: this changes things, so a stop part way is said as it is.
        let moved = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| -> Result<_, CantMove> {
            // As the library is now: a move made meanwhile counts.
            let (protected, moved) = library_now(&library).map_err(CantMove::Failed)?;
            let progress = |copied: u64| {
                let fraction = if folder.bytes == 0 { 0.0 } else { copied as f32 / folder.bytes as f32 };
                card.progress(fraction, Some(&format!("{} copied", mo_space::size_text(copied))));
            };
            mo_app::background::on_bulk_pool(|| {
                move_folder(
                    &folder.path,
                    &destination,
                    folder.bytes,
                    (&ask.key, folder.kind, &folder.shared_with),
                    &SystemDrives,
                    &moved,
                    &protected,
                    &progress,
                    &stop,
                )
            })
        }));
        let head = match moved {
            Ok(Ok(moved)) => {
                let noted = moving::moved_entry(&moved)
                    .and_then(|entry| add_moved_folder(&library, &entry).map_err(|e| e.to_string()));
                if let Err(why) = &noted {
                    log::error("Plugins", &format!("Move: {} wasn't added to the moved folders: {why}", moved.from));
                }
                let head = moving::moved_text(&moved, &noted);
                card.done(&head);
                head
            }
            Ok(Err(why)) => {
                let head = moving::cant_text(&why);
                card.failed(&head);
                head
            }
            Err(_) => {
                let head = "The move stopped on something it didn't expect (the log says what). Look at the folder \
                            and the other drive before trying again."
                    .to_owned();
                card.failed(&head);
                head
            }
        };
        back(&weak, move |app| moved_done(app, id, head));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start the move: {e}"));
        PAGE.with(|p| p.borrow_mut().clean = None);
        fill(app);
    }
}

fn moved_done(app: &AppWindow, id: u64, head: String) {
    log::info("Plugins", &format!("Move: {head}"));
    PAGE.with(|p| {
        if let Some(c) = p.borrow_mut().clean.as_mut().filter(|c| c.id == id) {
            c.stage = Stage::done(head);
        }
    });
    refresh(app);
    fill_panel(app);
}
