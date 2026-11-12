//! Favourites from Bitwig, on the Sounds page ([`mo_app::bitwig_favourites`]): a card in the sound card's place that
//! lists the multisamples in your Bitwig Library folders, each with a tick (kept in the app's settings, so it can be run
//! again), and Mark as favourites, which finds every ticked multisample's samples in the library and makes those
//! sounds favourites, through the one writer of your say about sounds, as the card's heart does ([`super::yours`]).
//!
//! The folders are listed and the multisamples read off the window's thread (rule 12), on the low-priority pool for
//! work over a disk (rule 21). The card says what it found: how many sounds became favourites, and each sample it
//! couldn't find and why (the session log has the same list).
//!
//! The samples it couldn't find can be saved as sounds (Solomon, 12 Nov 2026: "Yes, into a From Bitwig folder"):
//! copied out of the multisamples into the first sample folder › From Bitwig › the multisample's name
//! ([`favs::save_missing`], never over a file), the sample folders scanned again, and once the page has the new
//! sounds, those become favourites too.

use std::cell::RefCell;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_app::bitwig_favourites::{self as favs, Found, Marked, Missing, Multisample, Picks, Why};
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
    /// The samples the last look couldn't find that can be saved as sounds, and the sample folder they'd go in.
    saveable: Vec<Missing>,
    save_into: Option<PathBuf>,
    /// Saving them is under way.
    saving: bool,
    /// Samples saved into the From Bitwig folder, with the type their multisample names, to make favourites once the
    /// page has them as sounds.
    pending: Vec<(PathBuf, Option<&'static str>)>,
    /// Looking for those among the page's sounds is under way, and whether the sounds were read again meanwhile.
    resolving: bool,
    again: bool,
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
    saveable: usize,
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
        let made_up = Missing {
            multisample: String::new(),
            from: PathBuf::new(),
            sample: String::new(),
            entry: 0,
            why: Why::NoSuchName,
        };
        c.saveable = vec![made_up; saveable];
        c.save_into = (saveable > 0).then(|| PathBuf::from(r"I:\Samples"));
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
    if picked.is_empty() || !sounds_whole() || CARD.with(|c| c.borrow().saving) || super::HERE.with(|h| *h.borrow()) {
        return;
    }
    let (list, library_db) = PAGE.with(|p| {
        let p = p.borrow();
        (p.list.clone(), p.kit.library.clone())
    });
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let stop = Arc::new(AtomicBool::new(false));
    let ask = CARD.with(|c| {
        let mut c = c.borrow_mut();
        c.ask += 1;
        c.stop = Some(stop.clone());
        c.said = format!("Looking through {}…", mo_app::songs::plural(picked.len(), "multisample"));
        c.missing.clear();
        c.saveable.clear();
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
        let into = library_db.as_deref().and_then(first_sample_folder);
        let sent = slint::invoke_from_event_loop(move || {
            let summary = match weak.upgrade() {
                Some(app) => favs::summary(&found, looked(&app, ask, &found, into)),
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

/// The first sample folder there is (Settings › Sounds), to save the samples not found into. Opens the song library:
/// never on the window's thread.
fn first_sample_folder(library_db: &Path) -> Option<PathBuf> {
    favs::sample_folders(library_db).into_iter().find(|f| f.is_dir())
}

/// What the look numbered `ask` found: the sounds the page lists that aren't favourites yet made so ([`favourites`]),
/// and the samples it couldn't find kept to be saved into `into` (the first sample folder).
fn looked(app: &AppWindow, ask: u64, found: &Found, into: Option<PathBuf>) -> Marked {
    let _doing = crate::watchdog::doing("Sounds: Bitwig favourites found");
    let marked = favourites(app, &found.sounds, &found.types);
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.ask == ask {
            c.saveable = found.missing.iter().filter(|m| m.why != Why::Unplayable).cloned().collect();
            c.save_into = into;
        }
    });
    ended(app, ask, favs::summary(found, marked), favs::missing_rows(found));
    marked
}

/// The sounds `ids` the page lists that aren't favourites yet made so, at once on the page and through the writer,
/// all in one note.
fn favourites(app: &AppWindow, sounds: &[i64], types: &[(i64, &'static str)]) -> Marked {
    let marked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let mut marked = Marked::default();
        let mut ids = Vec::new();
        for &id in sounds {
            match p.places.get(&id).and_then(|&at| p.list.now(at)).map(|now| now.favourite) {
                Some(true) => marked.already += 1,
                Some(false) => ids.push(id),
                // Not on the page's list now (read again meanwhile without it): left alone.
                None => {}
            }
        }
        marked.newly = ids.len();
        // The type its multisample names, for each sound that takes it ([`favs::takes_type`]), a list a type.
        let mut typed: Vec<(&'static str, Vec<i64>)> = Vec::new();
        for &(id, kind) in types {
            let takes = p
                .places
                .get(&id)
                .and_then(|&at| p.list.now(at))
                .is_some_and(|now| favs::takes_type(kind, now.sound_type, now.type_source));
            if takes {
                match typed.iter_mut().find(|(k, _)| *k == kind) {
                    Some((_, ids)) => ids.push(id),
                    None => typed.push((kind, vec![id])),
                }
            }
        }
        marked.typed = typed.iter().map(|(_, ids)| ids.len()).sum();
        let favourite = Change::Favourite(true);
        let mut yours: Vec<Yours> = ids.iter().map(|&id| Yours { id, change: favourite.clone() }).collect();
        for (kind, ids) in &typed {
            let change = Change::Type(Some((*kind).to_owned()));
            yours.extend(ids.iter().map(|&id| Yours { id, change: change.clone() }));
        }
        if !yours.is_empty() {
            // All in one note, so one undo takes it all back.
            let number = p.noter.as_ref().and_then(|n| n.note_all_yours(yours)).unwrap_or(super::did::NOT_KEPT);
            if !ids.is_empty() {
                super::yours::take_all(&mut p, &ids, &favourite, number);
            }
            for (kind, ids) in &typed {
                super::yours::take_all(&mut p, ids, &Change::Type(Some((*kind).to_owned())), number);
            }
        }
        marked
    });
    if marked.newly + marked.typed > 0 {
        super::yours::shown_again(app);
    }
    marked
}

/// Save as sounds: the samples the last look couldn't find copied into the first sample folder › From Bitwig, off the
/// window's thread, then the sample folders scanned again; they become favourites once the page has them
/// ([`sounds_read`]).
pub(super) fn save(app: &AppWindow) {
    if super::HERE.with(|h| *h.borrow()) {
        return;
    }
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let Some((missing, into)) = CARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.saving || c.stop.is_some() || c.saveable.is_empty() {
            return None;
        }
        let into = c.save_into.clone()?;
        c.saving = true;
        Some((std::mem::take(&mut c.saveable), into))
    }) else {
        return;
    };
    show(app);
    // Kept to put back if the thread can't start, so Save as sounds is still there.
    let kept = missing.clone();
    let weak = app.as_weak();
    let started = background::spawn("Bitwig favourites save", move || {
        let job = feed
            .start(SAVING, "Copied out of the multisamples into your sample folder: Bitwig's are left as they are.");
        let stop = AtomicBool::new(false);
        let saved =
            catch_crash(AssertUnwindSafe(|| background::on_bulk_pool(|| favs::save_missing(&missing, &into, &stop))));
        let said = match &saved {
            Ok(saved) => {
                let words = favs::saved_summary(saved, &into);
                log::info("Sounds", &format!("Favourites from Bitwig: {words}"));
                for (what, why) in &saved.failed {
                    log::warning("Sounds", &format!("Favourites from Bitwig, couldn't save {what}: {why}"));
                }
                job.done(&words);
                words
            }
            Err(_) => {
                job.failed(TRIPPED_SAVING);
                TRIPPED_SAVING.to_owned()
            }
        };
        let paths = saved.map(|s| s.paths.into_iter().zip(s.types).collect()).unwrap_or_default();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                saved_as_files(&app, said, paths);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start saving the Bitwig samples: {e}"));
        CARD.with(|c| {
            let mut c = c.borrow_mut();
            c.saving = false;
            c.saveable = kept;
        });
        show(app);
    }
}

/// The activity card's headline while samples are saved.
const SAVING: &str = "Saving the Bitwig samples not found as sounds";

/// What the card says when saving itself tripped up.
const TRIPPED_SAVING: &str = "Saving tripped up: the crash log says where.";

/// The samples are saved as the files `paths`: the sample folders are scanned again, and the card waits for them.
fn saved_as_files(app: &AppWindow, said: String, paths: Vec<(PathBuf, Option<&'static str>)>) {
    let scanning = !paths.is_empty() && super::sample_folders_changed();
    let said = if paths.is_empty() {
        said
    } else if scanning {
        format!("{said} They become favourites once the app has scanned the folder.")
    } else {
        format!(
            "{said} The app can't scan your sample folders just now: once they're scanned, Mark as favourites finds \
             them."
        )
    };
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        c.saving = false;
        c.said = said;
        c.missing.clear();
        if scanning {
            for path in paths {
                if !c.pending.iter().any(|(p, _)| *p == path.0) {
                    c.pending.push(path);
                }
            }
        }
    });
    show(app);
    resolve(app);
}

/// The samples saved and waiting ([`saved_as_files`]) looked for among the page's sounds, off the window's thread; the
/// ones there become favourites, the rest wait for the next time the sounds are read.
fn resolve(app: &AppWindow) {
    let Some(pending) = CARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.resolving {
            // The sounds were read again meanwhile: looked through again once this look is done.
            c.again = true;
            return None;
        }
        if c.pending.is_empty() || !sounds_whole() {
            return None;
        }
        c.resolving = true;
        Some(c.pending.clone())
    }) else {
        return;
    };
    let list = PAGE.with(|p| p.borrow().list.clone());
    let weak = app.as_weak();
    let started = background::spawn("Bitwig favourites saved", move || {
        let paths: Vec<PathBuf> = pending.iter().map(|(p, _)| p.clone()).collect();
        let ids = catch_crash(AssertUnwindSafe(|| favs::sounds_at(&paths, list.sounds()))).unwrap_or_default();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                resolved(&app, &pending, &ids);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't look for the saved Bitwig samples: {e}"));
        CARD.with(|c| c.borrow_mut().resolving = false);
    }
}

/// The saved samples `pending` are the sounds `ids` (`None`: not read yet): those become favourites, and take the
/// type their multisample names.
fn resolved(app: &AppWindow, pending: &[(PathBuf, Option<&'static str>)], ids: &[Option<i64>]) {
    let mut seen = HashSet::new();
    let found: Vec<i64> = ids.iter().flatten().copied().filter(|&id| seen.insert(id)).collect();
    let types: Vec<(i64, &'static str)> =
        pending.iter().zip(ids).filter_map(|((_, kind), id)| Some(((*id)?, (*kind)?))).collect();
    let marked = if found.is_empty() { Marked::default() } else { favourites(app, &found, &types) };
    CARD.with(|c| {
        let mut c = c.borrow_mut();
        c.resolving = false;
        let there: HashSet<&PathBuf> =
            pending.iter().zip(ids).filter(|(_, id)| id.is_some()).map(|((p, _), _)| p).collect();
        c.pending.retain(|(p, _)| !there.contains(p));
        if !found.is_empty() {
            let words = format!(
                "{} saved from Bitwig {} favourites now.",
                mo_app::songs::plural(found.len(), "sound"),
                if found.len() == 1 { "is" } else { "are" },
            );
            let words = if marked.typed > 0 { format!("{words} {}", favs::typed_words(marked.typed)) } else { words };
            log::info("Sounds", &format!("Favourites from Bitwig: {words} ({} newly)", marked.newly));
            c.said = if c.pending.is_empty() {
                words
            } else {
                format!("{words} {} still to be scanned.", mo_app::songs::plural(c.pending.len(), "sample"))
            };
        }
    });
    show(app);
    // The sounds were read again while this look went on: the samples still waiting may be among them now.
    if CARD.with(|c| std::mem::take(&mut c.borrow_mut().again)) {
        resolve(app);
    }
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
    let (lines, listed, busy, saving, said, missing, save) = CARD.with(|c| {
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
        let save = match (&c.save_into, c.saveable.len()) {
            (_, 0) => String::new(),
            (Some(into), n) => format!(
                "Save as sounds copies {} into {}, a folder for each multisample, and makes {} favourites once \
                 scanned. Your multisamples stay as they are.",
                if n == 1 {
                    "the sample not found".to_owned()
                } else {
                    format!("the {} not found", mo_app::songs::plural(n, "sample"))
                },
                into.join(favs::FROM_BITWIG).display(),
                if n == 1 { "it" } else { "them" },
            ),
            (None, _) => String::new(),
        };
        (lines, c.list.is_some(), c.stop.is_some(), c.saving, c.said.clone(), c.missing.clone(), save)
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
    data.set_bitwig_can_mark(busy || (ticked > 0 && whole && !saving));
    data.set_bitwig_said(SharedString::from(if !whole && !busy && said.is_empty() {
        "Your sounds are still being read: Mark as favourites works once they're in.".to_owned()
    } else {
        said
    }));
    let missing: Vec<SharedString> = missing.into_iter().map(SharedString::from).collect();
    data.set_bitwig_missing(ModelRc::new(VecModel::from(missing)));
    data.set_bitwig_save_note(SharedString::from(save));
}

/// The page's sounds were read (or read again): Mark as favourites may work now.
/// Samples saved from Bitwig may be among them now: those become favourites.
pub(super) fn sounds_read(app: &AppWindow) {
    if app.global::<SoundsData>().get_bitwig_shown() {
        show(app);
    }
    resolve(app);
}
