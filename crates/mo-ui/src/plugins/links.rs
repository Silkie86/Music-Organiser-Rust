//! The Plugins page's broken links (the whole-app review of 19 Nov, PM-2): the moved folders whose link doesn't work
//! (`mo_app::plugins::links`), looked for on a worker at each read of the library and after each move ([`look`]), and
//! named in a line above the list, with Link again ([`link_again`]: on a worker, with Stop while it runs). What it sets
//! aside is kept in the library's folders left behind (`mo_library::moves::keep_old_homes`, Solomon's OK of 21 Nov
//! 2026), so the Space page reminds you of it until it goes to the Recycle Bin.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mo_app::log;
use mo_app::plugins::links::{self, Broken, Fixed, NotKept, Records};
use mo_app::plugins::moving::moved_now;
use mo_library::Library;
use mo_library::moves::{MOVED_FOLDERS, OLD_HOMES, add_moved_folder, keep_old_homes};
use mo_space::after_move::local_drives;
use mo_space::folder_move::{MovedFolder, SystemDrives};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::clean::{self, back};
use super::moving::{Running, kept_when_free};
use super::{PAGE, fill_panel, refresh};
use crate::{AppWindow, LinkTrouble, PluginsData};

/// Counts the looks: only the newest one's finding is shown (rule 12, newest wins).
static LOOKS: AtomicU64 = AtomicU64::new(0);

/// What Link again is known by among the moves running ([`Running`]): closing the app stops it and waits, as for a
/// move.
const LINKING: &str = "Link again";

/// The library's moved folders, its folders left behind and your folders (project, sample and MIDI folders) as it
/// keeps them now, read strictly (one that can't be read is an error, never none); with no moved folders, nothing
/// else is read. Each error is a whole sentence.
fn records_now(library: &Path) -> Result<Records, String> {
    let sentence = |e: mo_library::LibraryError| {
        let t = e.to_string();
        if t.ends_with('.') { t } else { format!("{t}.") }
    };
    let lib = Library::open_read_only(library).map_err(sentence)?;
    let moved = moved_now(lib.strict_setting(MOVED_FOLDERS).map_err(sentence)?.as_deref())?;
    if moved.is_empty() {
        return Ok(Records::default());
    }
    let homes = links::homes_now(lib.strict_setting(OLD_HOMES).map_err(sentence)?.as_deref())?;
    let yours = lib.protected_roots().map_err(sentence)?;
    Ok(Records { moved, homes, yours })
}

/// Looks, on a worker, for the moved folders whose link doesn't work, and shows them: at each read of the library,
/// each time the page shows (a drive plugged in, or a link put right by hand, changes nothing in the library), and
/// after each move. Each one is said in the log once it's found, not at every look.
pub(super) fn look(app: &AppWindow) {
    let look = LOOKS.fetch_add(1, Ordering::Relaxed) + 1;
    // The ones already found were said in the log with what the computer says of them: only new ones are looked at
    // that way.
    let (library, known) = PAGE.with(|p| {
        let p = p.borrow();
        (p.library.clone(), p.broken.clone())
    });
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins broken links", move || {
        let found = mo_app::background::guarded(|| -> Result<Vec<(Broken, Option<String>)>, String> {
            // No library (the screenshots, a first start): no moved folders.
            if !library.exists() {
                return Ok(Vec::new());
            }
            let records = records_now(&library)?;
            // The drives are only asked for when a lost copy is looked for. What the computer says of each new link
            // that doesn't lead to its copy goes in the log with it, to tell why (Solomon's log of 25 Nov 2026).
            let found = links::look(&records, &local_drives);
            Ok(found
                .into_iter()
                .map(|b| {
                    let seen = if known.contains(&b) { None } else { links::seen(&b) };
                    (b, seen)
                })
                .collect())
        })
        .and_then(|r| r);
        match found {
            Ok(found) => back(&weak, move |app| {
                if LOOKS.load(Ordering::Relaxed) != look {
                    return;
                }
                let new: Vec<(Broken, Option<String>)> = PAGE.with(|p| {
                    let mut page = p.borrow_mut();
                    let p = &mut *page;
                    let new = found.iter().filter(|(b, _)| !p.broken.contains(b)).cloned().collect();
                    p.broken = found.into_iter().map(|(b, _)| b).collect();
                    // What Link again said of one whose link works now is old: it mustn't show if it breaks again.
                    p.link_tried.retain(|at, _| p.broken.iter().any(|b| mo_space::key(&b.moved.from) == *at));
                    new
                });
                for (b, seen) in &new {
                    log::warning("Plugins", &links::log_line(b, seen.as_deref()));
                }
                fill(app);
            }),
            Err(why) => log::warning("Plugins", &format!("Couldn't look at the moved folders' links: {why}")),
        }
    });
    if let Err(e) = spawned {
        log::warning("Plugins", &format!("Couldn't look at the moved folders' links: {e}"));
    }
}

/// Shows the moved folders whose link doesn't work, each as a chip with a sign and all of it in its tooltip, with Link
/// again when it can do anything (not while a clean-up, uninstall or move is under way: one at a time), or that Link
/// again is running, with Stop.
pub(super) fn fill(app: &AppWindow) {
    let (head, chips, tip, can, linking) = PAGE.with(|p| {
        let p = p.borrow();
        if p.linking.is_some() {
            return ("Linking moved folders again…".to_owned(), Vec::new(), String::new(), false, true);
        }
        let can = p.broken.iter().any(Broken::can_link_again) && !p.clean.as_ref().is_some_and(|c| c.busy());
        let chips = links::chips(&p.broken, &p.link_tried);
        (links::head(&p.broken), chips, links::link_again_tip(&p.broken), can, false)
    });
    // Each sign a chip can have is one the page can draw (`Icons.named`); mo-screens runs this.
    #[cfg(debug_assertions)]
    for sign in links::SIGNS {
        let drawn = app.global::<crate::Icons>().invoke_named(sign.into());
        assert!(!drawn.is_empty(), "Icons.named in studio.slint has no {sign}");
    }
    let data = app.global::<PluginsData>();
    let changed = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let changed = p.link_chips != chips;
        if changed {
            p.link_chips.clone_from(&chips);
        }
        changed
    });
    if changed {
        let rows: Vec<LinkTrouble> = chips
            .into_iter()
            .map(|c| LinkTrouble { name: c.name.into(), icon: c.icon.into(), tip: c.tip.into() })
            .collect();
        data.set_links(ModelRc::new(VecModel::from(rows)));
    }
    data.set_link_head(head.into());
    data.set_link_tip(tip.into());
    data.set_can_link_again(can);
    data.set_linking(linking);
}

/// Link again, on a worker: each moved folder that needs it, looked at again first, as the library keeps them now
/// (`mo_app::plugins::links::link_again`). Then the moved folders whose copy is somewhere else now, and the folders it
/// set aside, are kept in the library, and the page looks again.
pub(super) fn link_again(app: &AppWindow) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.linking.is_some() || !p.broken.iter().any(Broken::can_link_again) || clean::held(&p) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        p.linking = Some(stop.clone());
        Some((p.library.clone(), p.feed.clone(), stop))
    });
    let Some((library, feed, stop)) = job else { return };
    // A look under way is older than this: what it finds isn't shown.
    LOOKS.fetch_add(1, Ordering::Relaxed);
    // The card's own button waits too (one at a time).
    clean::fill(app);
    log::info("Plugins", "Link again");
    let weak = app.as_weak();
    let running = Running::start(LINKING, &stop);
    let spawned = mo_app::background::spawn("Plugins link again", move || {
        let _running = running;
        let card = feed.start("Linking moved folders again", "Looking at each link first");
        // Heard for every file, from several threads: the card hears about it at most every quarter of a second.
        let last = Mutex::new(Instant::now());
        let progress = |m: &MovedFolder, copied: u64, of: u64| {
            let Ok(mut last) = last.try_lock() else { return };
            if last.elapsed() < Duration::from_millis(250) {
                return;
            }
            *last = Instant::now();
            let fraction = if of == 0 { 0.0 } else { (copied as f32 / of as f32).min(1.0) };
            let name = mo_formats::paths::file_name(&m.from);
            card.progress(fraction, Some(&format!("{name}: {} copied", mo_space::size_text(copied))));
        };
        // Not `guarded`: this changes things, so a stop part way is said as it is.
        let fixed = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| -> Result<Fixed, String> {
            let records = records_now(&library)?;
            Ok(mo_app::background::on_bulk_pool(|| {
                links::link_again(&records, &local_drives, &SystemDrives, &progress, &stop)
            }))
        }));
        let linked = match fixed {
            Ok(Ok(fixed)) => {
                let moved_kept = keep_moved(&library, &fixed);
                let homes_kept = keep_homes(&library, &fixed);
                // The log keeps all it said; the card names the folders (Solomon, 24 Nov 2026: nothing
                // over-explained), and each chip still on the line says what it said of that one.
                let head = links::done_text(&fixed, &moved_kept, &homes_kept);
                let short = links::card_text(&fixed, &moved_kept, &homes_kept);
                if links::all_done(&fixed, &moved_kept, &homes_kept) {
                    log::info("Plugins", &format!("Link again: {head}"));
                    card.done(&short);
                } else {
                    log::warning("Plugins", &format!("Link again: {head}"));
                    card.failed(&short);
                }
                let linked = fixed.done.iter().filter(|d| d.said.is_ok()).map(|d| d.from.clone()).collect();
                (linked, links::tried(&fixed))
            }
            Ok(Err(why)) => {
                let head = format!("Nothing was linked again: {why}");
                log::warning("Plugins", &format!("Link again: {head}"));
                card.failed(&head);
                (Vec::new(), BTreeMap::new())
            }
            Err(_) => {
                log::error("Plugins", "Link again: stopped on a crash part way");
                let head = "Link again stopped on something it didn't expect (the log says what). Look at the moved \
                            folders before trying again."
                    .to_owned();
                card.failed(&head);
                (Vec::new(), BTreeMap::new())
            }
        };
        let (linked, tried) = linked;
        back(&weak, move |app| linked_again(app, linked, tried));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start Link again: {e}"));
        PAGE.with(|p| p.borrow_mut().linking = None);
        fill(app);
    }
}

/// Keeps the moved folders whose copy is somewhere else now in the library's moved folders: each replaces the one for
/// the same folder (`mo_library::moves::add_moved_folder`). Their links lead there already, so each is worth the wait.
/// Stops at the first that can't be kept: that one and the ones after it aren't ([`NotKept`]).
fn keep_moved(library: &Path, fixed: &Fixed) -> Result<(), NotKept> {
    let entries = links::moved_entries(fixed).map_err(|why| NotKept { first: 0, why })?;
    for (at, entry) in entries.iter().enumerate() {
        kept_when_free(|| add_moved_folder(library, entry)).map_err(|why| {
            // In full, so it can be put right by hand.
            log::error("Plugins", &format!("Link again: a moved folder wasn't kept: {why}. {entry}"));
            NotKept { first: at, why }
        })?;
    }
    Ok(())
}

/// Keeps the folders Link again set aside, and the ones that moved with their drive, in the library's folders left
/// behind, so the Space page reminds you of them (`mo_library::moves::keep_old_homes`, all or none).
fn keep_homes(library: &Path, fixed: &Fixed) -> Result<(), String> {
    let entries = links::home_entries(fixed)?;
    if entries.is_empty() {
        return Ok(());
    }
    kept_when_free(|| keep_old_homes(library, &entries)).inspect_err(|why| {
        let all: Vec<&str> = entries.iter().map(|(e, _)| e.as_str()).collect();
        log::error("Plugins", &format!("Link again: the folders set aside weren't kept: {why}. {}", all.join(" ")));
    })
}

/// Link again is over: the folders linked again are links now (their room is on the other drive), the Space page
/// looks for what it set aside when it next shows, and the page looks at the links again, each one still needing a
/// look saying in its tooltip what Link again said of it (`tried`).
fn linked_again(app: &AppWindow, linked: Vec<String>, tried: BTreeMap<String, String>) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.linking = None;
        p.link_tried = tried;
        // What it found before is old now: the line waits for the look below.
        p.broken.clear();
        p.moves += 1;
        if let Some(pl) = p.plugins.as_mut() {
            pl.forget_paths(&linked);
        }
        p.moved.extend(linked);
    });
    refresh(app);
    fill_panel(app);
    crate::space::plugin_moved(app);
    look(app);
}

/// Stop on the line while Link again runs: the folder it's on is left as it was, and so are the rest.
pub(super) fn stop(_app: &AppWindow) {
    PAGE.with(|p| {
        if let Some(stop) = &p.borrow().linking {
            stop.store(true, Ordering::Relaxed);
        }
    });
}

/// Whether Link again is running: one clean-up, uninstall, move or Link again at a time ([`clean::held`]).
pub(super) fn running(p: &super::Page) -> bool {
    p.linking.is_some()
}

/// Whether Link again is running, said on a card when it is: nothing else starts meanwhile (one at a time).
pub(super) fn holds(p: &super::Page) -> bool {
    if !running(p) {
        return false;
    }
    p.feed.start("Link again is still under way", "").failed(
        "One clean-up, uninstall, move or Link again at a time: try this again once it's done (or once you press \
         Stop on its line).",
    );
    true
}

/// For the screenshots: the line as it shows for these, with what the last Link again said of some (`tried`).
pub(super) fn show_sample(app: &AppWindow, broken: Vec<Broken>, tried: BTreeMap<String, String>) {
    LOOKS.fetch_add(1, Ordering::Relaxed);
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.broken = broken;
        p.link_tried = tried;
    });
    fill(app);
}
