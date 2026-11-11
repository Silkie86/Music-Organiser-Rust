//! Favourites from Bitwig, on the Sounds page ([`mo_app::bitwig_favourites`]): a card in the sound card's place that
//! lists the multisamples in your Bitwig Library folders, each with a tick (kept in the app's settings, so it can be run
//! again), and Mark as favourites, which finds every ticked multisample's samples in the library and makes those
//! sounds favourites, through the one writer of your say about sounds, as the card's heart does ([`super::yours`]).
//!
//! The folders are listed and the multisamples read off the window's thread (rule 12), on the low-priority pool for
//! work over a disk (rule 21). The card says what it found: how many sounds became favourites, and each sample it
//! couldn't find (the session log has the same list).

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_app::bitwig_favourites::{self as favs, Multisample};
use mo_app::{app_settings, background, log, space_bitwig};
use mo_sounds::compact::yours::{Change, Yours};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{FEED, PAGE};
use crate::{AppWindow, FilterChoice, SoundsData};

/// The card's state, on the window's thread.
#[derive(Default)]
struct Card {
    /// The multisamples found, in order (`None` until they're listed).
    list: Option<Vec<Multisample>>,
    /// The ticked ones, as the setting keeps them.
    ticked: HashSet<String>,
    /// Counts the listings, so an older one's is dropped.
    listing: u64,
    /// Counts the looks, so word from an older one is dropped.
    ask: u64,
    /// Stops the look under way.
    stop: Option<Arc<AtomicBool>>,
    /// What the last look said, and the samples it couldn't find (a line each).
    said: String,
    missing: Vec<String>,
}

thread_local! {
    static CARD: RefCell<Card> = RefCell::default();
}

/// The activity card's headline while a look is on.
const LOOKING: &str = "Finding your Bitwig favourites";

/// Bitwig's Library folders: Settings' Bitwig Studio folder's (then the one in Documents) and the Library folders
/// added on Settings › DAWs, as Space looks in them.
fn libraries() -> Vec<PathBuf> {
    let chosen =
        crate::settings::value(app_settings::BITWIG_FOLDER_KEY).filter(|f| !f.trim().is_empty()).map(PathBuf::from);
    let extra = crate::settings::value(app_settings::BITWIG_LIBRARIES_KEY)
        .map(|v| app_settings::folder_list(&v))
        .unwrap_or_default();
    let studios: Vec<PathBuf> =
        chosen.into_iter().chain(mo_app::bitwig_convert::documents().map(|d| d.join("Bitwig Studio"))).collect();
    space_bitwig::library_folders(&[], &studios, &extra)
}

/// Opens the card (or closes it, when it's open), listing the multisamples afresh: one may have been saved since.
pub(super) fn toggle(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    if data.get_bitwig_shown() {
        close(app);
        return;
    }
    data.set_bitwig_shown(true);
    let ask = CARD.with(|c| {
        let mut c = c.borrow_mut();
        c.ticked = favs::picks_of(&crate::settings::value(favs::PICKS_KEY).unwrap_or_default());
        c.listing += 1;
        c.listing
    });
    show(app);
    if super::HERE.with(|h| *h.borrow()) {
        // The screenshots never read folders.
        return;
    }
    let libraries = libraries();
    let weak = app.as_weak();
    let started = background::spawn("Bitwig favourites list", move || {
        let found = background::on_bulk_pool(|| favs::multisamples(&libraries));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                listed(&app, ask, found);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't list the Bitwig multisamples: {e}"));
    }
}

/// The multisamples, as listed by the listing numbered `listing`.
fn listed(app: &AppWindow, listing: u64, found: Vec<Multisample>) {
    let now = CARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.listing != listing {
            return false;
        }
        c.list = Some(found);
        true
    });
    if now {
        show(app);
    }
}

/// The screenshots' card: these multisamples, ticked as `ticked` says. Never reads a folder.
pub(super) fn show_for_screens(
    app: &AppWindow,
    found: Vec<Multisample>,
    ticked: &[&str],
    said: &str,
    missing: &[&str],
) {
    app.global::<SoundsData>().set_bitwig_shown(true);
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        c.ticked = found
            .iter()
            .filter(|m| ticked.contains(&m.name.as_str()))
            .map(|m| m.path.to_string_lossy().to_lowercase())
            .collect();
        c.list = Some(found);
        c.said = said.to_owned();
        c.missing = missing.iter().map(|m| (*m).to_owned()).collect();
    });
    show(app);
}

/// Closes the card. A look under way goes on (its activity card says when it's done).
pub(super) fn close(app: &AppWindow) {
    app.global::<SoundsData>().set_bitwig_shown(false);
}

/// Ticks or unticks the multisample at `at` (every one with -1 for all, -2 for none), and keeps the ticks.
pub(super) fn tick(app: &AppWindow, at: i32) {
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        let Some(list) = c.list.clone() else { return };
        let keys: Vec<String> = match at {
            -1 | -2 => list.iter().map(key).collect(),
            _ => usize::try_from(at).ok().and_then(|i| list.get(i)).map(key).into_iter().collect(),
        };
        for k in keys {
            let on = match at {
                -1 => true,
                -2 => false,
                _ => !c.ticked.contains(&k),
            };
            if on {
                c.ticked.insert(k);
            } else {
                c.ticked.remove(&k);
            }
        }
        // Only the ones listed now: one gone from the folder is let go of.
        let ticked: Vec<&Multisample> = list.iter().filter(|m| c.ticked.contains(&key(m))).collect();
        crate::settings::remember(favs::PICKS_KEY, &favs::picks_setting(ticked.iter().map(|m| m.path.as_path())));
    });
    show(app);
}

fn key(m: &Multisample) -> String {
    m.path.to_string_lossy().to_lowercase()
}

/// Mark as favourites: the ticked multisamples' samples looked for in the library, off the window's thread, and the
/// sounds found made favourites. Stop, while a look is on.
pub(super) fn mark(app: &AppWindow) {
    if let Some(stop) = CARD.with(|c| c.borrow_mut().stop.take()) {
        stop.store(true, Ordering::Relaxed);
        show(app);
        return;
    }
    let picked: Vec<Multisample> = CARD.with(|c| {
        let c = c.borrow();
        c.list.iter().flatten().filter(|m| c.ticked.contains(&key(m))).cloned().collect()
    });
    let (list, whole) = PAGE.with(|p| {
        let p = p.borrow();
        (p.list.clone(), p.read && p.partial.is_none() && !p.list.is_empty())
    });
    if picked.is_empty() || !whole || super::HERE.with(|h| *h.borrow()) {
        return;
    }
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let stop = Arc::new(AtomicBool::new(false));
    let ask = CARD.with(|c| {
        let mut c = c.borrow_mut();
        c.ask += 1;
        c.stop = Some(stop.clone());
        c.said = format!("Looking through {}…", mo_app::songs::plural(picked.len(), "multisample"));
        c.missing.clear();
        c.ask
    });
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Bitwig favourites", move || {
        let job = feed.start(LOOKING, "Each ticked multisample's samples, looked for in your sound library.");
        let temp = std::env::temp_dir();
        let found = background::on_bulk_pool(|| {
            favs::look(&picked, list.sounds(), &temp, &stop, |done, of| {
                job.progress(done as f32 / of.max(1) as f32, Some(&format!("{done} of {of} multisamples")));
            })
        });
        let sent = slint::invoke_from_event_loop(move || {
            let newly = weak.upgrade().map_or(0, |app| looked(&app, ask, &found));
            let summary = favs::summary(&found, newly);
            log::info("Sounds", &format!("Favourites from Bitwig: {summary}"));
            for line in favs::missing_lines(&found) {
                log::info("Sounds", &format!("Favourites from Bitwig, not found: {line}"));
            }
            job.done(&summary);
        });
        if sent.is_err() {
            log::warning("Sounds", "Favourites from Bitwig: the window closed before the sounds could be marked.");
        }
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start looking for Bitwig favourites: {e}"));
        CARD.with(|c| {
            let mut c = c.borrow_mut();
            c.stop = None;
            c.said = "It couldn't start: the log says why.".to_owned();
        });
        show(app);
    }
}

/// What the look numbered `ask` found: the sounds not favourites yet made so, at once on the page and through the
/// writer. How many were made favourites.
fn looked(app: &AppWindow, ask: u64, found: &favs::Found) -> usize {
    let _doing = crate::watchdog::doing("Sounds: Bitwig favourites found");
    let newly = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let ids: Vec<i64> = found
            .sounds
            .iter()
            .copied()
            .filter(|&id| p.places.get(&id).and_then(|&at| p.list.now(at)).is_some_and(|now| !now.favourite))
            .collect();
        if ids.is_empty() {
            return 0;
        }
        let yours: Vec<Yours> = ids.iter().map(|&id| Yours { id, change: Change::Favourite(true) }).collect();
        let number = p.noter.as_ref().and_then(|n| n.note_all_yours(yours)).unwrap_or(super::did::NOT_KEPT);
        for &id in &ids {
            super::yours::take(&mut p, id, Change::Favourite(true), number);
        }
        ids.len()
    });
    if newly > 0 {
        super::yours::shown_again(app);
    }
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.ask == ask {
            c.stop = None;
            c.said = favs::summary(found, newly);
            c.missing = favs::missing_rows(found);
        }
    });
    show(app);
    newly
}

/// The card as it is now.
fn show(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let (lines, ticked, listed, busy, said, missing) = CARD.with(|c| {
        let c = c.borrow();
        let lines: Vec<FilterChoice> = c
            .list
            .iter()
            .flatten()
            .map(|m| FilterChoice {
                text: SharedString::from(m.name.as_str()),
                count: SharedString::default(),
                on: c.ticked.contains(&key(m)),
            })
            .collect();
        let ticked = lines.iter().filter(|l| l.on).count();
        (lines, ticked, c.list.is_some(), c.stop.is_some(), c.said.clone(), c.missing.clone())
    });
    let whole = PAGE.with(|p| {
        let p = p.borrow();
        p.read && p.partial.is_none() && !p.list.is_empty()
    });
    let of = lines.len();
    data.set_bitwig_listed(listed);
    data.set_bitwig_multisamples(ModelRc::new(VecModel::from(lines)));
    data.set_bitwig_ticked(SharedString::from(format!("{ticked} of {of} ticked")));
    data.set_bitwig_busy(busy);
    data.set_bitwig_can_mark(busy || (ticked > 0 && whole));
    data.set_bitwig_said(SharedString::from(if !whole && !busy && said.is_empty() {
        "Your sounds are still being read: Mark as favourites works once they're in.".to_owned()
    } else {
        said
    }));
    let missing: Vec<SharedString> = missing.into_iter().map(SharedString::from).collect();
    data.set_bitwig_missing(ModelRc::new(VecModel::from(missing)));
}

/// The page's sounds were read (or read again): Mark as favourites may work now.
pub(super) fn sounds_read(app: &AppWindow) {
    if app.global::<SoundsData>().get_bitwig_shown() {
        show(app);
    }
}
