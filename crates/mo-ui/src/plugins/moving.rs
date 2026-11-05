//! The Plugins page's Move to another drive, on the side card's On disk tab, in Clean up's place
//! ([`super::clean`]): pick one of the plugin's preset or sound folders, then where on another drive it goes. It's
//! checked there first, then copied, every file checked, and swapped for a link (`mo_space::folder_move`), all on a
//! worker. Then the move is added to the library's moved folders (`mo_library::moves`), so the old app and Space know
//! of it and its old copy.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

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
            stage: Stage::MovePick {
                intro,
                picked: (!ask.folders.is_empty()).then_some(0),
                choosing: false,
                ask: Arc::new(ask),
            },
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
    enum Next {
        Choose(u64, Arc<Ask>, usize),
        Go(Arc<Ask>, usize, u64, String),
    }
    let next = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let c = p.clean.as_mut()?;
        let id = c.id;
        match &mut c.stage {
            Stage::MovePick { ask, picked: Some(at), choosing, .. } if !*choosing => {
                // One picker at a time, and the folder stays picked while it's open.
                *choosing = true;
                Some(Next::Choose(id, ask.clone(), *at))
            }
            Stage::MoveReady { ask, folder, bytes, destination, .. } => {
                Some(Next::Go(ask.clone(), *folder, *bytes, destination.clone()))
            }
            _ => None,
        }
    });
    match next {
        Some(Next::Choose(id, ask, at)) => {
            fill(app);
            let weak = app.as_weak();
            choose_folder(move |path| {
                let Some(app) = weak.upgrade() else { return };
                match path {
                    Some(path) => check(&app, id, ask, at, path.to_string_lossy().into_owned()),
                    None => chosen_none(&app, id),
                }
            });
        }
        Some(Next::Go(ask, at, bytes, destination)) => go(app, ask, at, bytes, destination),
        None => {}
    }
}

/// The folder picker; `then` hears the folder, or `None` when it closed without one (or couldn't open).
fn choose_folder(then: impl FnOnce(Option<PathBuf>) + 'static) {
    let then = std::rc::Rc::new(std::cell::RefCell::new(Some(then)));
    let later = then.clone();
    let asked = slint::spawn_local(async move {
        let folder = rfd::AsyncFileDialog::new()
            .set_title("Pick where the folder goes (on another drive)")
            .pick_folder()
            .await
            .map(|f| f.path().to_path_buf());
        if let Some(then) = later.borrow_mut().take() {
            then(folder);
        }
    });
    if let Err(e) = asked {
        log::error("Plugins", &format!("Couldn't open the folder picker: {e}"));
        if let Some(then) = then.borrow_mut().take() {
            then(None);
        }
    }
}

/// The picker closed without a folder: the card can pick again.
fn chosen_none(app: &AppWindow, id: u64) {
    PAGE.with(|p| {
        if let Some(Cleaning { stage: Stage::MovePick { choosing, .. }, .. }) =
            p.borrow_mut().clean.as_mut().filter(|c| c.id == id)
        {
            *choosing = false;
        }
    });
    fill(app);
}

/// Reads your folders and the moves made already, as the library keeps them now. Each error is a whole sentence.
fn library_now(library: &Path) -> Result<(Vec<String>, Vec<mo_space::folder_move::MovedFolder>), String> {
    let sentence = |e: mo_library::LibraryError| {
        let t = e.to_string();
        if t.ends_with('.') { t } else { format!("{t}.") }
    };
    let lib = Library::open_read_only(library).map_err(sentence)?;
    let protected = lib.protected_roots().map_err(sentence)?;
    let moved = moving::moved_now(lib.strict_setting(MOVED_FOLDERS).map_err(sentence)?.as_deref())?;
    Ok((protected, moved))
}

/// Checks the folder can move where you picked, on a worker, and measures it again: the page's size for it may be
/// old, and a maker's folder is measured without the plugin folders inside it, which move with it.
fn check(app: &AppWindow, id: u64, ask: Arc<Ask>, at: usize, destination: String) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let library = p.library.clone();
        // Still the card, and the folder, the picker was opened for.
        let c = p.clean.as_mut().filter(|c| {
            c.id == id && matches!(c.stage, Stage::MovePick { picked: Some(now), choosing: true, .. } if now == at)
        })?;
        c.stage = Stage::MoveChecking;
        Some((c.stop.clone(), library))
    });
    let Some((stop, library)) = job else { return };
    fill(app);
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins move check", move || {
        let found = mo_app::background::guarded(|| -> Result<Result<u64, CantMove>, String> {
            let (protected, moved) = library_now(&library)?;
            let Some(f) = ask.folders.get(at) else { return Ok(Err(CantMove::Gone)) };
            Ok(mo_app::background::on_bulk_pool(|| {
                let bytes = match mo_space::size::measure(Path::new(&f.path), &stop) {
                    Ok(m) if m.bytes > 0 => m.bytes,
                    Ok(_) => f.bytes,
                    Err(_) => return Err(CantMove::Stopped),
                };
                match problem(&f.path, &destination, bytes, &SystemDrives, &moved, &protected, &stop) {
                    Some(why) => Err(why),
                    None => Ok(bytes),
                }
            }))
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
    found: Result<Result<u64, CantMove>, String>,
) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(c) = p.clean.as_mut().filter(|c| c.id == id) else { return };
        c.stage = match (found, ask.folders.get(at)) {
            (Ok(Ok(bytes)), Some(f)) => {
                let to = mo_space::folder_move::target(&f.path, &destination).to_string_lossy().into_owned();
                let text = moving::ready_text(&f.path, &to, bytes);
                Stage::MoveReady { text, ask, folder: at, bytes, destination }
            }
            (Ok(Err(why)), _) => Stage::done(why.text()),
            (Ok(Ok(_)), None) => Stage::done(CantMove::Gone.text()),
            (Err(why), _) => Stage::done(format!("Couldn't check it can move there: {why}")),
        };
    });
    fill(app);
}

/// Moves the folder on a worker, then adds the move to the library's moved folders.
fn go(app: &AppWindow, ask: Arc<Ask>, at: usize, bytes: u64, destination: String) {
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
            // Heard for every file, from several threads: the card hears about every half percent (16 MB at least).
            let step = (bytes / 200).max(16 << 20);
            let next = AtomicU64::new(0);
            let progress = |copied: u64| {
                let due = next.load(Ordering::Relaxed);
                if copied < due
                    || next
                        .compare_exchange(due, copied.saturating_add(step), Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                {
                    return;
                }
                let fraction = if bytes == 0 { 0.0 } else { copied as f32 / bytes as f32 };
                card.progress(fraction, Some(&format!("{} copied", mo_space::size_text(copied))));
            };
            mo_app::background::on_bulk_pool(|| {
                move_folder(
                    &folder.path,
                    &destination,
                    bytes,
                    (&ask.key, folder.kind, &folder.shared_with),
                    &SystemDrives,
                    &moved,
                    &protected,
                    &progress,
                    &stop,
                )
            })
        }));
        let gone = matches!(moved, Ok(Ok(_))).then(|| folder.path.clone());
        let head = match moved {
            Ok(Ok(moved)) => {
                let noted = moving::moved_entry(&moved).and_then(|entry| note(&library, &entry));
                if let Err(why) = &noted {
                    // In full, so it can be added by hand.
                    let entry = moving::moved_entry(&moved).unwrap_or_default();
                    log::error(
                        "Plugins",
                        &format!("Move: {} wasn't added to the moved folders: {why}. {entry}", moved.from),
                    );
                }
                let head = moving::moved_text(&moved, &noted);
                card.done(&head);
                head
            }
            Ok(Err(why)) => {
                let head = why.text();
                log::warning("Plugins", &format!("Move {}: {head}", ask.name));
                card.failed(&head);
                head
            }
            Err(_) => {
                log::error("Plugins", &format!("Move {}: stopped on a crash part way", ask.name));
                let head = "The move stopped on something it didn't expect (the log says what). Look at the folder \
                            and the other drive before trying again."
                    .to_owned();
                card.failed(&head);
                head
            }
        };
        back(&weak, move |app| moved_done(app, id, head, gone));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start the move: {e}"));
        PAGE.with(|p| p.borrow_mut().clean = None);
        fill(app);
    }
}

/// Adds the move to the library's moved folders ([`kept_when_free`]): the move is done by now, so it's worth the wait.
fn note(library: &Path, entry: &str) -> Result<(), String> {
    kept_when_free(|| add_moved_folder(library, entry))
}

/// Makes one change to library.db, trying again a few times while the C# app holds it (a scan being saved). Call it
/// away from the window's thread.
pub(super) fn kept_when_free(change: impl Fn() -> Result<(), mo_library::LibraryError>) -> Result<(), String> {
    let mut tries = 0;
    loop {
        match change() {
            Ok(()) => return Ok(()),
            Err(e) if tries < 5 && e.is_busy() => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(3));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn moved_done(app: &AppWindow, id: u64, head: String, gone: Option<String>) {
    log::info("Plugins", &format!("Move: {head}"));
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // A link now: its room is on the other drive, and it can't move again. Kept, so the next read (and Space)
        // leaves it out too.
        if let Some(gone) = gone {
            if let Some(pl) = p.plugins.as_mut() {
                pl.forget_paths(std::slice::from_ref(&gone));
            }
            p.moved.push(gone);
        }
        if let Some(c) = p.clean.as_mut().filter(|c| c.id == id) {
            c.stage = Stage::done(head);
        }
    });
    refresh(app);
    fill_panel(app);
}
