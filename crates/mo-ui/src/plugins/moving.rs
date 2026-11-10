//! The Plugins page's Move to another drive, on the side card's On disk tab, in Clean up's place
//! ([`super::clean`]): pick one of the plugin's preset or sound folders, then where on another drive it goes. It's
//! checked there first, then copied, every file checked, and swapped for a link (`mo_space::folder_move`), all on a
//! worker. Then the move is added to the library's moved folders (`mo_library::moves`), so Space and this page know of
//! it and its old copy.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use mo_app::log;
use mo_app::plugins::moves_under_way;
use mo_app::plugins::moving::{self, Ask};
use mo_library::Library;
use mo_library::moves::{MOVED_FOLDERS, add_moved_folder};
use mo_space::folder_move::{CantMove, SystemDrives, move_folder, problem};
use mo_space::half_copy::{self, Cleared, State};
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
    let running = Running::start(&folder.path, &stop);
    let spawned = mo_app::background::spawn("Plugins move", move || {
        let _running = running;
        // Not `guarded`: this changes things, so a stop part way is said as it is.
        let moved = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| -> Result<_, CantMove> {
            // As the library is now: a move made meanwhile counts.
            let (protected, moved) = library_now(&library).map_err(CantMove::Failed)?;
            // Noted before anything is copied, so a half copy a close or a crash leaves is found next time (review
            // E2-3).
            if let Err(e) = moves_under_way::start(&moves_under_way::file(), &folder.path, &destination) {
                log::warning("Plugins", &format!("Move: the move under way couldn't be noted: {e}"));
            }
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
        // Done: its copy is in use now. A move that stopped or failed cleared its copy, or left a half copy for the next
        // look to find; that look drops the note when there's nothing.
        if gone.is_some()
            && let Err(e) = moves_under_way::finished(&moves_under_way::file(), &folder.path, &destination)
        {
            log::warning("Plugins", &format!("Move: the move under way's note couldn't be taken out: {e}"));
        }
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
        p.moves += 1;
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
    // The old copy it left shows on Space; what a move cut short left shows here.
    crate::space::plugin_moved(app);
    look_for_half_copies(app);
}

/// The folders being moved in this run, with their Stop, from any thread: a move's note isn't dropped while it runs, and
/// closing the app stops it and waits ([`finish_up`]). [`ENDED`] hears each one end.
static RUNNING: Mutex<Vec<(String, Arc<AtomicBool>)>> = Mutex::new(Vec::new());
static ENDED: Condvar = Condvar::new();

/// One move in [`RUNNING`], taken out when dropped (however the move ends).
struct Running(String);

impl Running {
    fn start(from: &str, stop: &Arc<AtomicBool>) -> Running {
        RUNNING.lock().unwrap_or_else(PoisonError::into_inner).push((from.to_owned(), stop.clone()));
        Running(from.to_owned())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = running.iter().position(|(f, _)| f == &self.0) {
            running.remove(at);
        }
        ENDED.notify_all();
    }
}

/// When the window has closed: a move still copying is told to stop (its copy is cleared, as Stop does). Quick: the
/// waiting is [`finish_up`]'s, once the app's other jobs have been told to stop too.
pub(super) fn stop_moves() {
    let running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    if running.is_empty() {
        return;
    }
    log::info("Plugins", "Move: the app is closing, so the move under way stops");
    for (_, stop) in running.iter() {
        stop.store(true, Ordering::Relaxed);
    }
}

/// Waits for the moves [`stop_moves`] stopped, a minute at most; one cut off even so is found at the next start
/// (review E2-3).
pub(super) fn finish_up() {
    stop_moves();
    let running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    let (running, waited) = ENDED
        .wait_timeout_while(running, Duration::from_secs(60), |r| !r.is_empty())
        .unwrap_or_else(PoisonError::into_inner);
    if waited.timed_out() && !running.is_empty() {
        log::warning("Plugins", "Move: still stopping after a minute; what it left is found at the next start");
    }
}

/// Counts the looks for half copies: only the newest one's finding is shown (rule 12, newest wins).
static LOOKS: AtomicU64 = AtomicU64::new(0);

/// Looks, on a worker, for the half copies moves cut off part way left ([`moves_under_way::cut_off`]), and shows them:
/// at the page's first read, and after each move and each clear (not at every read: a half copy is read in full).
pub(super) fn look_for_half_copies(app: &AppWindow) {
    let look = LOOKS.fetch_add(1, Ordering::Relaxed) + 1;
    let file = moves_under_way::file();
    let library = PAGE.with(|p| p.borrow().library.clone());
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins half copies", move || {
        let found = mo_app::background::guarded(|| -> Result<_, String> {
            if moves_under_way::under_way(&file).is_empty() {
                return Ok(Vec::new());
            }
            let (_, moved) = library_now(&library)?;
            let running: Vec<String> =
                RUNNING.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(f, _)| f.clone()).collect();
            let never = AtomicBool::new(false);
            mo_app::background::on_bulk_pool(|| {
                moves_under_way::cut_off(&file, &moved, &running, &SystemDrives, &never)
            })
            .map_err(|_| "Stopped.".to_owned())
        })
        .and_then(|r| r);
        match found {
            Ok(found) => back(&weak, move |app| {
                if LOOKS.load(Ordering::Relaxed) != look {
                    return;
                }
                PAGE.with(|p| p.borrow_mut().half_copies = found);
                fill_half_copies(app);
            }),
            Err(why) => log::warning("Plugins", &format!("Couldn't look for half copies: {why}")),
        }
    });
    if let Err(e) = spawned {
        log::warning("Plugins", &format!("Couldn't look for half copies: {e}"));
    }
}

/// Shows what moves cut off left, with Clear (not while a clear runs).
pub(super) fn fill_half_copies(app: &AppWindow) {
    let (text, can) = PAGE.with(|p| {
        let p = p.borrow();
        let text = if p.clearing_half_copies {
            "Clearing what cut-off moves left…".to_owned()
        } else {
            moves_under_way::found_text(&p.half_copies)
        };
        (text, !p.clearing_half_copies && moves_under_way::can_clear(&p.half_copies))
    });
    let data = app.global::<PluginsData>();
    data.set_half_copy_text(text.into());
    data.set_can_clear_half_copies(can);
}

/// Clears the half copies moves cut off left, on a worker: each looked at again first (`half_copy::clear`), so only
/// a copy nothing uses goes for good; one in use only loses its marker. One clear at a time. Closing the app part way
/// leaves what's left of a copy marked, for the next start.
pub(super) fn clear_half_copies(app: &AppWindow) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.clearing_half_copies {
            return None;
        }
        let found: Vec<_> = p.half_copies.iter().filter(|h| !matches!(h.state, State::Stays(_))).cloned().collect();
        if found.is_empty() {
            return None;
        }
        p.clearing_half_copies = true;
        Some((p.library.clone(), p.feed.clone(), found))
    });
    let Some((library, feed, found)) = job else { return };
    // A look already under way is older than the clear: what it finds isn't shown.
    LOOKS.fetch_add(1, Ordering::Relaxed);
    fill_half_copies(app);
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins clear half copies", move || {
        let card = feed.start("Clearing what cut-off moves left", "Checking each copy first");
        let cleared = mo_app::background::guarded(|| -> Result<(u64, Vec<String>), String> {
            let (_, moved) = library_now(&library)?;
            let never = AtomicBool::new(false);
            let (mut freed, mut stays) = (0u64, Vec::new());
            for h in &found {
                let tried = mo_app::background::on_bulk_pool(|| {
                    half_copy::clear(Path::new(&h.path), &moved, &SystemDrives, &never)
                });
                match tried {
                    Ok(Cleared::Copy(bytes)) => {
                        log::info("Plugins", &format!("Cleared the half copy {}", h.path));
                        freed = freed.saturating_add(bytes);
                    }
                    Ok(_) => log::info("Plugins", &format!("Took the marker out of {}: it's in use", h.path)),
                    Err(why) => {
                        log::warning("Plugins", &format!("The half copy {} stays: {why}", h.path));
                        stays.push(format!("{}: {why}", h.path));
                    }
                }
            }
            Ok((freed, stays))
        });
        match cleared {
            Ok(Ok((freed, stays))) if stays.is_empty() => {
                card.done(&format!("Cleared, freeing {}.", mo_space::size_text(freed)))
            }
            Ok(Ok((_, stays))) => card.failed(&format!("Some stay: {}", stays.join(" "))),
            Ok(Err(why)) | Err(why) => card.failed(&format!("Nothing was cleared: {why}")),
        }
        back(&weak, |app| {
            PAGE.with(|p| p.borrow_mut().clearing_half_copies = false);
            look_for_half_copies(app);
        });
    });
    if let Err(e) = spawned {
        log::warning("Plugins", &format!("Couldn't clear the half copies: {e}"));
        PAGE.with(|p| p.borrow_mut().clearing_half_copies = false);
        fill_half_copies(app);
    }
}
