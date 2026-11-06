//! The Plugins page's Uninstall, on the side card's On disk tab, in Clean up's place ([`super::clean`]): the plugin's
//! own uninstaller, picked for you unless it looks like another version's, then a wait for the plugin's files to go
//! and Clean up offered for what it left behind. Without one, its plugin files to the Recycle Bin instead, with Put
//! back. The rules are `mo_app::plugins::uninstall`'s; the looking, the running and the waiting are on workers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_app::log;
use mo_app::plugins::clean::Watched;
use mo_app::plugins::uninstall::{self, Plan, Waited};
use mo_space::Stopped;
use mo_space::clean::SystemBin;
use mo_space::plugin_files;
use mo_space::plugin_space::SpaceKind;
use mo_space::uninstallers;
use slint::ComponentHandle;

use super::clean::{self, Cleaning, Outcome, Stage, back, fill};
use super::{PAGE, fill_panel, refresh};
use crate::{AppWindow, PluginsData};

/// Whether the plugin at `row` can be uninstalled here ([`mo_app::plugins::Plugins::can_uninstall`]).
pub(super) fn can_uninstall(row: usize) -> bool {
    PAGE.with(|p| p.borrow().plugins.as_ref().is_some_and(|pl| pl.can_uninstall(row)))
}

/// Uninstall for the picked plugin: shows the On disk tab and looks for its uninstaller on a worker.
pub(super) fn start(app: &AppWindow) {
    let started = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if clean::held(&p) {
            return None;
        }
        let ask = p.plugins.as_ref()?.uninstall_ask(p.picked?)?;
        if let Some(old) = p.clean.take() {
            old.stop.store(true, Ordering::Relaxed);
        }
        p.clean_asked += 1;
        let stop = Arc::new(AtomicBool::new(false));
        p.clean = Some(Cleaning {
            row_key: ask.row_key.clone(),
            name: ask.name.clone(),
            title: ask.title(),
            warning: ask.warning(),
            id: p.clean_asked,
            stop: stop.clone(),
            stage: Stage::Finding,
        });
        Some((ask, p.clean_asked, stop))
    });
    let Some((ask, id, stop)) = started else { return };
    app.global::<PluginsData>().set_tab(1);
    fill(app);
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins uninstall look", move || {
        let plan =
            mo_app::background::guarded(|| mo_app::background::on_bulk_pool(|| ask.plan(&uninstallers::all(), &stop)));
        back(&weak, move |app| found(app, id, ask, plan));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start looking for an uninstaller: {e}"));
        PAGE.with(|p| p.borrow_mut().clean = None);
        fill(app);
    }
}

fn found(app: &AppWindow, id: u64, ask: uninstall::Ask, plan: Result<Result<Plan, Stopped>, String>) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(c) = p.clean.as_mut().filter(|c| c.id == id) else { return };
        let ask = Arc::new(ask);
        c.stage = match plan {
            Ok(Ok(plan)) => {
                let intro = ask.intro(&plan);
                match plan {
                    Plan::Run { found, must_pick, files } => {
                        log::info(
                            "Plugins",
                            &format!("Uninstall {}: {} uninstallers look like its", ask.name, found.len()),
                        );
                        let picked = (!must_pick).then_some(0);
                        let others = found.iter().map(|u| ask.is_other(u)).collect();
                        Stage::Pick { intro, found, others, files, picked }
                    }
                    Plan::Files(lines) => {
                        let ticks = lines.iter().map(|l| l.stays.is_none()).collect();
                        Stage::Files { intro, lines, ticks }
                    }
                    Plan::Waves | Plan::Nothing => Stage::done(intro),
                }
            }
            Ok(Err(Stopped)) => Stage::done("Stopped.".to_owned()),
            Err(why) => {
                log::warning("Plugins", &format!("Uninstall {}: couldn't look for its uninstaller: {why}", c.name));
                Stage::done(format!("Couldn't look for {}'s uninstaller: {why}. Nothing was removed.", c.name))
            }
        };
    });
    fill(app);
}

/// Uninstall pressed: runs the uninstaller picked, or sends the plugin files ticked to the Recycle Bin.
pub(super) fn confirm(app: &AppWindow) {
    let run = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let feed = p.feed.clone();
        let c = p.clean.as_mut()?;
        let Stage::Pick { found, files, picked: Some(at), .. } = &c.stage else { return None };
        let u = found.get(*at)?.clone();
        let files = files.clone();
        c.stage = Stage::Waiting;
        let card = feed.start(&format!("Uninstalling {}", u.name), "Its uninstaller is open in its own window.");
        Some((u, files, c.id, c.stop.clone(), c.row_key.clone(), c.name.clone(), card))
    });
    if let Some((u, files, id, stop, row_key, name, card)) = run {
        fill(app);
        log::info("Plugins", &format!("Uninstall {name}: running {} ({})", u.name, u.command));
        let weak = app.as_weak();
        let spawned = mo_app::background::spawn("Plugins uninstaller", move || {
            // The files there before it runs: one already gone says nothing about how it went.
            let watched = uninstall::there_now(&files);
            let until = std::time::Instant::now() + uninstall::WAIT;
            let ran = mo_app::background::guarded(|| uninstallers::run_until(&u, &stop, Some(until)))
                .unwrap_or_else(uninstallers::Ran::Unclear);
            let waited = uninstall::after_run(&ran, &watched, uninstall::GRACE, uninstall::POLL, &stop);
            let head = uninstall::ran_text(&name, &ran, waited);
            if waited == Waited::Gone {
                card.done(&head)
            } else {
                card.failed(&head)
            }
            let gone = uninstall::gone_now(&watched);
            back(&weak, move |app| ran_done(app, id, row_key, head, gone, waited == Waited::Gone));
        });
        if let Err(e) = spawned {
            log::error("Plugins", &format!("Couldn't start the uninstaller: {e}"));
            PAGE.with(|p| p.borrow_mut().clean = None);
            fill(app);
        }
        return;
    }
    remove_files(app);
}

/// The plugin files ticked go to the Recycle Bin, each checked again just before against the plugin as the page
/// knows it now.
fn remove_files(app: &AppWindow) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let feed = p.feed.clone();
        let ask = p.clean.as_ref().and_then(|c| {
            let pl = p.plugins.as_ref()?;
            pl.uninstall_ask(pl.row_of(&c.row_key)?)
        });
        let c = p.clean.as_mut()?;
        let Stage::Files { lines, ticks, .. } = &c.stage else { return None };
        let picked: Vec<_> = uninstall::ticked_files(lines, ticks).cloned().collect();
        if picked.is_empty() {
            return None;
        }
        let Some(ask) = ask else {
            c.stage = Stage::done(format!("{} isn't on the page as it was any more. Nothing was removed.", c.name));
            return None;
        };
        c.stage = Stage::Removing;
        let card = feed.start(&format!("Removing {}'s plugin files", c.name), "Sending to the Recycle Bin");
        Some((ask, picked, c.id, c.stop.clone(), c.row_key.clone(), card))
    });
    fill(app);
    let Some((ask, picked, id, stop, row_key, card)) = job else { return };
    log::info("Plugins", &format!("Uninstall {}: sending {} plugin files to the Recycle Bin", ask.name, picked.len()));
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins uninstall files", move || {
        let bin = Watched::new(&SystemBin);
        // Not `guarded`: this changes things, so a stop part way says what went.
        let done = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
            plugin_files::remove(&picked, &ask.space, &|k| ask.name_of(k), &bin, &stop)
        }));
        let outcome = match done {
            Ok(d) => Outcome::Done(d),
            Err(_) => {
                let lines: Vec<(&str, u64, SpaceKind)> =
                    picked.iter().map(|l| (l.path.as_str(), l.bytes, SpaceKind::Program)).collect();
                Outcome::Stopped(bin.went_of(&lines))
            }
        };
        clean::report(card, &outcome);
        let kept = clean::record(mo_app::space_bin::PLUGIN_UNINSTALL, &row_key, &outcome);
        back(&weak, move |app| clean::removed(app, id, row_key, outcome, kept));
    });
    if let Err(e) = spawned {
        log::error("Plugins", &format!("Couldn't start removing the plugin files: {e}"));
        PAGE.with(|p| p.borrow_mut().clean = None);
        fill(app);
    }
}

fn ran_done(app: &AppWindow, id: u64, row_key: (String, String), head: String, gone: Vec<String>, uninstalled: bool) {
    log::info("Plugins", &format!("Uninstall: {head}"));
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(pl) = p.plugins.as_mut() {
            pl.forget_paths(&gone);
        }
        p.uninstalled.extend(gone);
        let offer =
            uninstalled && p.plugins.as_ref().is_some_and(|pl| pl.row_of(&row_key).is_some_and(|r| pl.can_clean(r)));
        if let Some(c) = p.clean.as_mut().filter(|c| c.id == id) {
            c.stage = Stage::Done { head, problems: Vec::new(), offer_clean: offer };
        }
    });
    refresh(app);
    fill_panel(app);
}
