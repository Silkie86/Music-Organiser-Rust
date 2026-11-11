//! Favourites from Bitwig, on the Sounds page ([`mo_app::bitwig_favourites`]): a card in the sound card's place that
//! lists the multisamples in your Bitwig Library folders, each with a tick (kept in the app's settings, so it can be run
//! again), and Mark as favourites, which finds every ticked multisample's samples in the library and makes those
//! sounds favourites, through the one writer of your say about sounds, as the card's heart does ([`super::yours`]).
//!
//! The folders are listed and the multisamples read off the window's thread (rule 12), on the low-priority pool for
//! work over a disk (rule 21). The card says what it found: how many sounds became favourites, and each sample it
//! couldn't find (the session log has the same list).

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_app::bitwig_favourites::{self as favs, Found, Marked, Multisample, Picks};
use mo_app::{app_settings, background, log, space_bitwig};
use mo_catch::catch_crash;
use mo_sounds::compact::yours::{Change, Yours};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::{FEED, PAGE};
use crate::{AppWindow, FilterChoice, SoundsData};

/// The card's state, on the window's thread.
#[derive(Default)]
struct Card {
    /// The multisamples found, in order (`None` until they're listed).
    list: Option<Vec<Multisample>>,
    /// The ticked ones, as the setting keeps them (with ticks for ones not listed now).
    ticked: Picks,
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
    /// The multisamples' lines, changed in place so a tick doesn't turn the list back to its top.
    static LINES: Rc<VecModel<FilterChoice>> = Rc::new(VecModel::default());
}

/// The activity card's headline while a look is on.
const LOOKING: &str = "Finding your Bitwig favourites";

/// What the card and its activity card say when the look itself tripped up.
const TRIPPED: &str = "Looking tripped up: the crash log says where.";

/// Where Bitwig's Library folders are, as the settings say (read on the window's thread, where they live): your Bitwig
/// Studio folder (Settings › DAWs), then the one in Documents, and the Library folders added there.
fn bitwig_settings() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let chosen =
        crate::settings::value(app_settings::BITWIG_FOLDER_KEY).filter(|f| !f.trim().is_empty()).map(PathBuf::from);
    let extra = crate::settings::value(app_settings::BITWIG_LIBRARIES_KEY)
        .map(|v| app_settings::folder_list(&v))
        .unwrap_or_default();
    let studios = chosen.into_iter().chain(mo_app::bitwig_convert::documents().map(|d| d.join("Bitwig Studio")));
    (studios.collect(), extra)
}

/// Whether the page holds every sound (not just the first few, nor none): a look needs them all.
fn sounds_whole() -> bool {
    PAGE.with(|p| {
        let p = p.borrow();
        p.read && p.partial.is_none() && !p.list.is_empty()
    })
}

/// Opens the card (or closes it, when it's open), listing the multisamples afresh: one may have been saved since.
pub(super) fn toggle(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    if data.get_bitwig_shown() {
        close(app);
        return;
    }
    data.set_bitwig_shown(true);
    let listing = CARD.with(|c| {
        let mut c = c.borrow_mut();
        c.ticked = Picks::of(&crate::settings::value(favs::PICKS_KEY).unwrap_or_default());
        c.listing += 1;
        c.listing
    });
    show(app);
    if super::HERE.with(|h| *h.borrow()) {
        // The screenshots never read folders.
        return;
    }
    let (studios, extra) = bitwig_settings();
    let weak = app.as_weak();
    let started = background::spawn("Bitwig favourites list", move || {
        // Every folder is looked at here, never on the window's thread: K: may be asleep.
        let found = catch_crash(AssertUnwindSafe(|| {
            background::on_bulk_pool(|| favs::multisamples(&space_bitwig::library_folders(&[], &studios, &extra)))
        }))
        .unwrap_or_default();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                listed(&app, listing, found);
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
        c.ticked = Picks::default();
        for m in found.iter().filter(|m| ticked.contains(&m.name.as_str())) {
            c.ticked.set(m, true);
        }
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

/// Ticks or unticks the multisample at `at` (every one listed with -1 for all, -2 for none), and keeps the ticks.
pub(super) fn tick(app: &AppWindow, at: i32) {
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        let Card { list: Some(list), ticked, .. } = &mut *c else { return };
        match at {
            -1 | -2 => list.iter().for_each(|m| ticked.set(m, at == -1)),
            _ => {
                if let Some(m) = usize::try_from(at).ok().and_then(|i| list.get(i)) {
                    let on = !ticked.has(m);
                    ticked.set(m, on);
                }
            }
        }
        crate::settings::remember(favs::PICKS_KEY, &ticked.setting());
    });
    show(app);
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
        c.list.iter().flatten().filter(|m| c.ticked.has(m)).cloned().collect()
    });
    if picked.is_empty() || !sounds_whole() || super::HERE.with(|h| *h.borrow()) {
        return;
    }
    let list = PAGE.with(|p| p.borrow().list.clone());
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
        // A bug in the look is logged as a crash, in full; the card and its activity card still end.
        let found = catch_crash(AssertUnwindSafe(|| {
            let temp = std::env::temp_dir();
            background::on_bulk_pool(|| {
                favs::look(&picked, list.sounds(), &temp, &stop, |done, of| {
                    job.progress(done as f32 / of.max(1) as f32, Some(&format!("{done} of {of} multisamples")));
                })
            })
        }));
        let Ok(found) = found else {
            job.failed(TRIPPED);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    ended(&app, ask, TRIPPED.to_owned(), Vec::new());
                }
            });
            return;
        };
        let sent = slint::invoke_from_event_loop(move || {
            let summary = match weak.upgrade() {
                Some(app) => favs::summary(&found, looked(&app, ask, &found)),
                None => "The window closed before the sounds could be marked.".to_owned(),
            };
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
        ended(app, ask, "It couldn't start: the log says why.".to_owned(), Vec::new());
    }
}

/// What the look numbered `ask` found: the sounds the page lists that aren't favourites yet made so, at once on the
/// page and through the writer, all in one note.
fn looked(app: &AppWindow, ask: u64, found: &Found) -> Marked {
    let _doing = crate::watchdog::doing("Sounds: Bitwig favourites found");
    let marked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let mut marked = Marked::default();
        let mut ids = Vec::new();
        for &id in &found.sounds {
            match p.places.get(&id).and_then(|&at| p.list.now(at)).map(|now| now.favourite) {
                Some(true) => marked.already += 1,
                Some(false) => ids.push(id),
                // Not on the page's list now (read again meanwhile without it): left alone.
                None => {}
            }
        }
        marked.newly = ids.len();
        if !ids.is_empty() {
            let change = Change::Favourite(true);
            let yours: Vec<Yours> = ids.iter().map(|&id| Yours { id, change: change.clone() }).collect();
            let number = p.noter.as_ref().and_then(|n| n.note_all_yours(yours)).unwrap_or(super::did::NOT_KEPT);
            super::yours::take_all(&mut p, &ids, &change, number);
        }
        marked
    });
    if marked.newly > 0 {
        super::yours::shown_again(app);
    }
    ended(app, ask, favs::summary(found, marked), favs::missing_rows(found));
    marked
}

/// The look numbered `ask` is over: the card says `said`, and lists `missing`.
fn ended(app: &AppWindow, ask: u64, said: String, missing: Vec<String>) {
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.ask == ask {
            c.stop = None;
            c.said = said;
            c.missing = missing;
        }
    });
    show(app);
}

/// The card as it is now.
fn show(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let (lines, listed, busy, said, missing) = CARD.with(|c| {
        let c = c.borrow();
        let lines: Vec<FilterChoice> = c
            .list
            .iter()
            .flatten()
            .map(|m| FilterChoice {
                text: SharedString::from(m.name.as_str()),
                count: SharedString::default(),
                on: c.ticked.has(m),
            })
            .collect();
        (lines, c.list.is_some(), c.stop.is_some(), c.said.clone(), c.missing.clone())
    });
    let whole = sounds_whole();
    let (ticked, of) = (lines.iter().filter(|l| l.on).count(), lines.len());
    data.set_bitwig_listed(listed);
    LINES.with(|model| {
        if model.row_count() == lines.len() {
            for (i, line) in lines.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&line) {
                    model.set_row_data(i, line);
                }
            }
        } else {
            model.set_vec(lines);
        }
        data.set_bitwig_multisamples(ModelRc::from(model.clone() as Rc<dyn Model<Data = FilterChoice>>));
    });
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
