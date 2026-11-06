//! The On disk tab's "Not its", "Take off" and "Add a folder…" ([`mo_app::plugins::folder_fixes`]): each is kept in
//! library.db on a worker (`mo_library::plugin_folders`), and read back with every read of the page.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use mo_app::log;
use mo_app::plugins::folder_fixes::{self, FolderFix, Say};
use mo_library::Library;
use mo_library::plugin_folders::{add_plugin_folder, rule_out_plugin_folder, take_off_added_folder};
use slint::ComponentHandle;

use super::clean::{self, back};
use super::moving::kept_when_free;
use super::{PAGE, fill_panel, refresh};
use crate::AppWindow;

/// "Not its" (or "Take off") on the picked plugin's folder line `at`.
pub(super) fn not_its(app: &AppWindow, at: usize) {
    let job = PAGE.with(|p| {
        let p = p.borrow();
        // Not while a clean-up or move is under way: its list was made from the folders as they were.
        if clean::held(&p) {
            return None;
        }
        let path = p.room_paths.get(at)?.clone();
        let row = p.picked?;
        let pl = p.plugins.as_ref()?;
        let (key, say) = pl.not_its(row, &path)?;
        let measured = pl.measured_of(&key);
        Some((p.library.clone(), p.feed.clone(), pl.rows.get(row)?.name.clone(), key, say, path, measured))
    });
    let Some((library, feed, name, key, say, path, measured)) = job else { return };
    log::info("Plugins", &format!("{name}: {say:?} {path}"));
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins folder fix", move || {
        let kept = kept_when_free(|| match say {
            Say::RuleOut => rule_out_plugin_folder(&library, &key, &path),
            Say::TakeOff => take_off_added_folder(&library, &key, &path),
        });
        let fix = FolderFix::Removed { key, path, measured };
        report(&feed, &name, &fix, &kept);
        if kept.is_ok() {
            back(&weak, move |app| fixed(app, fix));
        }
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start the folder change: {e}"));
    }
}

/// "Add a folder…": you pick a folder of the picked plugin's; it's checked and measured on a worker, then kept.
pub(super) fn add(app: &AppWindow) {
    let job = PAGE.with(|p| {
        let p = p.borrow();
        if clean::held(&p) {
            return None;
        }
        let room = p.plugins.as_ref()?.add_folder_room(p.picked?)?;
        Some((p.library.clone(), p.feed.clone(), room))
    });
    let Some((library, feed, room)) = job else { return };
    let weak = app.as_weak();
    let title = "Pick a folder of the plugin's (its presets or sounds)";
    crate::backup::pick_folder(title, move |folder| {
        let path = folder.to_string_lossy().into_owned();
        // As the page knows the plugins now: other plugins' folders inside it, and when this one was measured.
        let known = PAGE.with(|p| {
            let p = p.borrow();
            p.plugins.as_ref().map(|pl| (pl.folders_inside(&path), pl.measured_of(&room.key)))
        });
        let Some((inside, measured)) = known else { return };
        let spawned = mo_app::background::spawn("Plugins add folder", move || {
            let card = feed.start(&format!("Adding a folder to {}", room.name), "Measuring it");
            let found = mo_app::background::guarded(|| -> Result<FolderFix, String> {
                let lib = Library::open_read_only(&library).map_err(|e| e.to_string())?;
                let mut protected = lib.protected_roots().map_err(|e| e.to_string())?;
                protected.extend(lib.strict_list_setting("midi_folders").map_err(|e| e.to_string())?);
                let mut plugin_roots = lib.strict_list_setting("plugin_folders").map_err(|e| e.to_string())?;
                if plugin_roots.is_empty() {
                    let defaults = mo_space::plugins::default_folders();
                    plugin_roots = defaults.iter().map(|d| d.to_string_lossy().into_owned()).collect();
                }
                if let Some(why) = folder_fixes::add_problem(&path, &room, &protected, &plugin_roots) {
                    return Err(why);
                }
                let stop = AtomicBool::new(false);
                let measured_now =
                    mo_app::background::on_bulk_pool(|| mo_space::size::measure(Path::new(&path), &stop))
                        .map_err(|_| "Measuring it stopped.".to_owned())?;
                // Before the library hears of it: the read that follows uses this measure, not one of its own.
                folder_fixes::remember_measure(&path, measured_now);
                let folder = folder_fixes::added_folder(&path, measured_now, &inside);
                kept_when_free(|| add_plugin_folder(&library, &room.key, &folder.path))?;
                Ok(FolderFix::Added { key: room.key.clone(), folder, measured })
            })
            .and_then(|r| r);
            match found {
                Ok(fix) => {
                    let head = folder_fixes::done_text(&room.name, &fix);
                    log::info("Plugins", &head);
                    card.done(&head);
                    back(&weak, move |app| fixed(app, fix));
                }
                Err(why) => {
                    log::warning("Plugins", &format!("{}: couldn't add {path}: {why}", room.name));
                    card.failed(&format!("Nothing was added: {why}"));
                }
            }
        });
        if let Err(e) = spawned {
            log::error("Plugins", &format!("Couldn't start adding the folder: {e}"));
        }
    });
}

fn report(feed: &mo_app::activity::Feed, name: &str, fix: &FolderFix, kept: &Result<(), String>) {
    let card = feed.start(&format!("{name}'s folders"), "");
    match kept {
        Ok(()) => {
            let head = folder_fixes::done_text(name, fix);
            log::info("Plugins", &head);
            card.done(&head);
        }
        Err(why) => {
            log::warning("Plugins", &format!("{name}: a folder change wasn't kept: {why}"));
            card.failed(&format!("It wasn't changed: {why}"));
        }
    }
}

/// Shows a change that was kept, now and over every read this session. A Clean up or Move card open for a plugin
/// closes (it isn't working, or the change wasn't let start): its list was made from the folders as they were.
fn fixed(app: &AppWindow, fix: FolderFix) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(pl) = p.plugins.as_mut() {
            pl.apply_folder_fixes(std::slice::from_ref(&fix));
        }
        p.folder_fixes.push(fix);
        if p.clean.as_ref().is_some_and(|c| !c.busy()) {
            p.clean = None;
        }
    });
    refresh(app);
    fill_panel(app);
}
