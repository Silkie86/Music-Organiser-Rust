//! Fills the Plugins page from `mo_app::plugins` and answers its search box, chips, cards and side card. The library
//! is read on a low-priority thread when the page is shown (rules 12 and 21), when anything was written to it since
//! the last read, Space's Recycle Bin changed or the day did; until it's back the page shows what it read before. The picked plugin's installed copies are
//! read on a thread of their own, newest pick wins. Nothing here writes to library.db; Clean up (`clean`) sends
//! folders you tick to the Recycle Bin, and nowhere else.

mod clean;
mod folder_fixes;
mod moving;
mod uninstall;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Local;
use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::finder::Kind;
use mo_app::log;
use mo_app::places::Place;
use mo_app::plugins::{
    self, Chip, Filters, Group, NEEDED_COUNT_CHOICES, Plugins, Sort, State, UNUSED_YEARS_CHOICES, copy_lines,
    no_copies_note, search_url,
};
use mo_app::shell;
use mo_app::shortcuts::{PLUGINS_ACTIONS, Shortcuts};
use mo_app::year_strip;
use mo_library::Library;
use mo_library::plugins::ScannedPlugin;
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel, Weak};

use crate::filters::FilterModels;
use crate::{
    AppWindow, CardAction, CopyRow, MenuItem, NeededCard, PluginListLine, PluginsData, RoomBar, RoomFolder, UseRow,
    YearBar,
};

/// The side card's tabs.
const TABS: [&str; 3] = ["Songs", "On disk", "Installed copies"];

/// What the page knows, on the window's thread.
#[derive(Default)]
struct Page {
    library: PathBuf,
    plugins: Option<Plugins>,
    /// The picked plugin's row.
    picked: Option<usize>,
    /// The side card's DAW chip ("" for any) and Every save.
    use_daw: String,
    every_save: bool,
    /// The card's song lines as shown, for opening one.
    uses: Vec<i64>,
    years: Vec<(i32, usize)>,
    /// Counts the reads of copies, so one for a plugin picked before is dropped.
    copies_asked: u64,
    shortcuts: Shortcuts,
    feed: Feed,
    /// Clean up for the picked plugin, if one is open, and what this session's clean-ups sent to the Recycle Bin.
    clean: Option<clean::Cleaning>,
    binned: Vec<clean::Binned>,
    /// The plugin files this session's uninstalls removed: left out of every read until the old app looks again.
    uninstalled: Vec<String>,
    /// What this session's clean-ups removed that isn't in your Recycle Bin (no Put back for it): left out too.
    removed_for_good: Vec<String>,
    /// The folders this session's moves swapped for a link: their room is on the other drive now, so they're left out
    /// of every read too.
    moved: Vec<String>,
    /// How many of this session's moves have finished, whatever came of them: each leaves an old copy beside its link,
    /// or, cut short, what it had copied, for Space to look for ([`moves_this_session`]).
    moves: u64,
    clean_asked: u64,
    /// The picked plugin's folder lines' paths, as the On disk tab shows them.
    room_paths: Vec<String>,
    /// This session's "Not its", "Take off" and "Add a folder": shown over every read until the old app measures.
    folder_fixes: Vec<mo_app::plugins::folder_fixes::FolderFix>,
    /// What another page asked this one to show (Space's Show on Plugins page) before the library was first read.
    wanted: Option<Wanted>,
    /// Ctrl+K asked for the library to be read before the page was first shown.
    finder_asked: bool,
}

/// What another page can ask this one to show.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Wanted {
    /// One plugin's card, by its line's name key and own, and its name (looked for when it isn't there by those).
    Plugin { key: String, own: String, name: String },
    /// Every plugin that takes room and no song has used lately ("Big and unused").
    Unused,
}

/// The library, whether to read it whatever ([`read_again`]), and what Space's clean-ups have in the Recycle Bin then
/// (a change there is shown by a fresh read, as the library itself doesn't change).
type ReadJob = (PathBuf, bool, Vec<String>);
type CopiesJob = (u64, PathBuf, String);

thread_local! {
    static PAGE: RefCell<Page> = RefCell::default();
    static READER: RefCell<Option<Newest<ReadJob>>> = const { RefCell::new(None) };
    static COPIES: RefCell<Option<Newest<CopiesJob>>> = const { RefCell::new(None) };
    static FILTER_MODELS: FilterChips = FilterChips::default();
    static LINES: Rc<Lines> = Rc::new(Lines::default());
}

/// The list's lines, made only for the rows in sight as the list scrolls (the list view asks for them): a search or
/// filter only tells the list it changed, rather than making a line for every plugin shown.
#[derive(Default)]
struct Lines {
    notify: ModelNotify,
}

impl Model for Lines {
    type Data = PluginListLine;

    fn row_count(&self) -> usize {
        PAGE.with(|p| p.try_borrow().ok().and_then(|p| p.plugins.as_ref().map(|pl| pl.shown.len())).unwrap_or(0))
    }

    fn row_data(&self, row: usize) -> Option<PluginListLine> {
        PAGE.with(|p| {
            let p = p.try_borrow().ok()?;
            let pl = p.plugins.as_ref()?;
            Some(line(pl, *pl.shown.get(row)?))
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The filters' chips and menus on screen, kept from one showing to the next.
#[derive(Default)]
struct FilterChips {
    show: FilterModels,
    kind: FilterModels,
    format: FilterModels,
    daw: FilterModels,
    /// The card's songs tab: one DAW or every one.
    use_daw: FilterModels,
}

/// Wires the page. The library at `library` is read when the page is first shown.
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = feed;
        p.shortcuts = shortcuts.clone();
    });
    let data = app.global::<PluginsData>();
    data.set_lines(ModelRc::from(LINES.with(|m| m.clone()) as Rc<dyn Model<Data = PluginListLine>>));
    data.set_sorts(strings(Sort::ALL.iter().map(|s| s.name())));
    data.set_tabs(strings(TABS));
    data.set_card_counts(strings(
        NEEDED_COUNT_CHOICES.iter().map(|&n| if n == 0 { "Off".to_owned() } else { n.to_string() }),
    ));
    data.set_unused_choices(strings(
        UNUSED_YEARS_CHOICES.iter().map(|&y| if y == 1 { "1 year".to_owned() } else { format!("{y} years") }),
    ));
    fill_actions(app);
    crate::finder::on_open(Kind::Plugin, |app, hit, _| {
        let (key, own) = mo_app::finder::plugin_of(&hit.id);
        crate::go_to(app, Place::Plugins.index() as i32);
        show_plugin(app, key, own, &hit.title);
        true
    });
    // Ctrl+K finds your plugins before this page is first shown: the library is read once, in the background.
    crate::finder::when_opening(|_| {
        let first = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.plugins.is_none() && !std::mem::replace(&mut p.finder_asked, true)
        });
        if first {
            shown();
        }
    });

    let weak = app.as_weak();
    data.on_search_edited(move |t| change(&weak, |f| f.search = t.to_string()));
    let weak = app.as_weak();
    data.on_show_unused(move || change(&weak, |f| f.show(Chip::BigUnused)));
    let weak = app.as_weak();
    data.on_toggle(move |filter, c| {
        if filter == "daw" {
            change(&weak, |f| f.pick_daw(&c));
        } else if let Some(chip) = Chip::from_name(&c) {
            change(&weak, |f| f.pick(chip));
        }
    });
    let weak = app.as_weak();
    data.on_clear_filter(move |filter| {
        change(&weak, |f| match group_named(&filter) {
            Some(g) => f.clear_group(g),
            None => f.daws.clear(),
        });
    });
    let weak = app.as_weak();
    data.on_sort_picked(move |s| {
        if let Some(sort) = Sort::from_name(&s) {
            change(&weak, |f| f.sort = sort);
        }
    });
    let weak = app.as_weak();
    data.on_clear(move || change(&weak, |f| *f = Filters { sort: f.sort, ..Filters::default() }));
    let weak = app.as_weak();
    data.on_years_picked(move |a, b| {
        let span = PAGE.with(|p| year_strip::year_span(&p.borrow().years, a, b));
        if let Some(span) = span {
            change(&weak, |f| f.years = Some(span));
        }
    });
    let weak = app.as_weak();
    data.on_years_cleared(move || change(&weak, |f| f.years = None));
    let weak = app.as_weak();
    data.on_select(move |row| {
        if let Some(app) = weak.upgrade() {
            pick(&app, usize::try_from(row).ok());
        }
    });
    let weak = app.as_weak();
    data.on_close_panel(move || {
        if let Some(app) = weak.upgrade() {
            pick(&app, None);
        }
    });
    let weak = app.as_weak();
    data.on_menu_chosen(move |id| {
        if let Some(app) = weak.upgrade() {
            run_action(&app, &id);
        }
    });
    let weak = app.as_weak();
    data.on_use_daw_picked(move |d| {
        let Some(app) = weak.upgrade() else { return };
        // One DAW or every one: picking the one that's on shows every DAW again.
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let mut picked: Vec<String> = if p.use_daw.is_empty() { Vec::new() } else { vec![p.use_daw.clone()] };
            mo_app::filters::toggle(&mut picked, &d, false);
            p.use_daw = picked.pop().unwrap_or_default();
        });
        fill_uses(&app);
    });
    let weak = app.as_weak();
    data.on_every_save_toggled(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.every_save = !p.every_save;
        });
        fill_uses(&app);
    });
    let weak = app.as_weak();
    data.on_open_use(move |at| {
        let Some(app) = weak.upgrade() else { return };
        let save = PAGE.with(|p| p.borrow().uses.get(usize::try_from(at).ok()?).copied());
        if let Some(save) = save {
            crate::go_to(&app, mo_app::places::Place::Songs.index() as i32);
            if !crate::songs::show_save(&app, save) {
                log::info("Plugins", "The song isn't on the Songs page yet (its list is still being read)");
            }
        }
    });
    let weak = app.as_weak();
    data.on_card_count_picked(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&n) = usize::try_from(i).ok().and_then(|i| NEEDED_COUNT_CHOICES.get(i)) else { return };
        PAGE.with(|p| {
            if let Some(pl) = p.borrow_mut().plugins.as_mut() {
                pl.needed_count = n;
            }
        });
        fill_cards(&app);
    });
    let weak = app.as_weak();
    data.on_unused_picked(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let Some(&y) = usize::try_from(i).ok().and_then(|i| UNUSED_YEARS_CHOICES.get(i)) else { return };
        PAGE.with(|p| {
            if let Some(pl) = p.borrow_mut().plugins.as_mut() {
                pl.set_unused_years(y);
            }
        });
        refresh(&app);
        fill_panel(&app);
    });
    let weak = app.as_weak();
    data.on_clean_ticked(move |at, on| {
        if let (Some(app), Ok(at)) = (weak.upgrade(), usize::try_from(at)) {
            clean::tick(&app, at, on);
        }
    });
    let weak = app.as_weak();
    data.on_clean_confirm(move || {
        if let Some(app) = weak.upgrade() {
            clean::confirm(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clean_close(move || {
        if let Some(app) = weak.upgrade() {
            clean::close(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clean_stop(move || {
        if let Some(app) = weak.upgrade() {
            clean::stop(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clean_leftovers(move || {
        if let Some(app) = weak.upgrade() {
            clean::start(&app);
        }
    });
    let weak = app.as_weak();
    data.on_folder_not_its(move |at| {
        if let (Some(app), Ok(at)) = (weak.upgrade(), usize::try_from(at)) {
            folder_fixes::not_its(&app, at);
        }
    });
    let weak = app.as_weak();
    data.on_add_folder(move || {
        if let Some(app) = weak.upgrade() {
            folder_fixes::add(&app);
        }
    });
    let weak = app.as_weak();
    data.on_put_back(move || {
        if let Some(app) = weak.upgrade() {
            clean::put_back(&app);
        }
    });
    data.on_panel_resized(|w| log::info("Plugins", &format!("Panel width {w:.0}")));

    let weak = app.as_weak();
    // What the last read was of: the library, its files then ([`mo_library::list_copy::stamp_of`]), Space's Recycle
    // Bin and the day (what counts as unused moves with it). A visit while all are the same keeps the list on the page
    // rather than read the whole library again.
    type Of = (PathBuf, String, Vec<String>, chrono::NaiveDate);
    let mut last: Option<Of> = None;
    let reader = Newest::start("Plugins read", move |(path, whatever, in_bin): ReadJob| {
        let started = std::time::Instant::now();
        let stamp = mo_library::list_copy::stamp_of(&path);
        let today = Local::now().date_naive();
        let same = |(p, s, b, d): &Of| *p == path && Some(s) == stamp.as_ref() && *b == in_bin && *d == today;
        if !whatever && last.as_ref().is_some_and(same) {
            log::info("Plugins", "Nothing was written to the library since the last read: the list stays as it is");
            return;
        }
        last = stamp.map(|s| (path.clone(), s, in_bin, today));
        let read = background::on_pool(|| {
            let lib = Library::open_read_only(&path).map_err(|e| e.to_string())?;
            plugins::read(&lib).map_err(|e| e.to_string())
        });
        if read.is_err() {
            last = None;
        }
        let built = read.map(|r| Plugins::new(r, Local::now().naive_local()));
        if let Ok(p) = &built {
            log::info("Plugins", &format!("{} plugins read in {:.1} s", p.rows.len(), started.elapsed().as_secs_f32()));
        }
        // What Ctrl+K finds, made here rather than on the window's thread.
        let found = built.as_ref().ok().map(|p| Arc::new(mo_app::finder::plugins(p)));
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                if let Some(found) = found {
                    crate::finder::add_source(&app, "plugins", found);
                }
                arrived(&app, built);
            }
        });
    });
    match reader {
        Ok(r) => READER.with(|s| *s.borrow_mut() = Some(r)),
        Err(e) => log::error("Plugins", &format!("Couldn't start reading the library: {e}")),
    }

    let weak = app.as_weak();
    let mut lib: Option<(PathBuf, Library)> = None;
    let copies = Newest::start("Plugin copies", move |(asked, path, name): CopiesJob| {
        if lib.as_ref().is_none_or(|(p, _)| *p != path) {
            lib = match Library::open_read_only(&path) {
                Ok(l) => Some((path.clone(), l)),
                Err(e) => {
                    log::warning("Plugins", &format!("Couldn't open the library for installed copies: {e}"));
                    None
                }
            };
        }
        let found: Vec<ScannedPlugin> = match lib.as_ref().map(|(_, l)| l.scanned_named(&name)) {
            Some(Ok(found)) => found,
            Some(Err(e)) => {
                log::warning("Plugins", &format!("Couldn't read the installed copies of {name}: {e}"));
                Vec::new()
            }
            None => Vec::new(),
        };
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                copies_arrived(&app, asked, &found);
            }
        });
    });
    match copies {
        Ok(c) => COPIES.with(|s| *s.borrow_mut() = Some(c)),
        Err(e) => log::error("Plugins", &format!("Couldn't start reading installed copies: {e}")),
    }
    // The app may start on this page, before its reader was there to ask.
    if app.get_place() == mo_app::places::Place::Plugins.index() as i32 {
        shown();
    }
}

/// This session's "Not its", "Take off" and "Add a folder", for Space's own read of the plugins to show too
/// ([`mo_app::plugins::Plugins::apply_folder_fixes`]).
pub fn folder_fixes_this_session() -> Vec<mo_app::plugins::folder_fixes::FolderFix> {
    PAGE.with(|p| p.borrow().folder_fixes.clone())
}

/// The page came into view: its library is read again, unless nothing was written to it since the last read.
pub fn shown() {
    ask_read(false);
}

/// The library is read again even if it hasn't changed (Put back: what came back shows again).
pub(crate) fn read_again() {
    ask_read(true);
}

fn ask_read(whatever: bool) {
    let path = PAGE.with(|p| p.borrow().library.clone());
    let in_bin = crate::space::plugin_files_in_bin();
    READER.with(|r| {
        if let Some(r) = r.borrow().as_ref() {
            r.ask((path, whatever, in_bin));
        }
    });
}

/// Runs one of the page's actions on the picked plugin (a button, the right-click menu or its key). Returns whether
/// there was a plugin to do it for.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let picked = PAGE.with(|p| {
        let p = p.borrow();
        let pl = p.plugins.as_ref()?;
        let row = p.picked?;
        let r = pl.rows.get(row)?;
        Some((r.name.clone(), r.vendor.clone(), pl.saves_using(row, &p.use_daw), p.use_daw.clone(), p.feed.clone()))
    });
    let Some((name, vendor, saves, daw, feed)) = picked else { return false };
    match id {
        "plugins.projects" => {
            let label = if daw.is_empty() { format!("Using {name}") } else { format!("Using {name} in {daw}") };
            crate::go_to(app, mo_app::places::Place::Songs.index() as i32);
            crate::songs::show_only(app, &label, saves);
        }
        "plugins.cleanUp" => clean::start(app),
        "plugins.uninstall" => uninstall::start(app),
        "plugins.move" => moving::start(app),
        "plugins.searchOnline" => {
            let url = search_url(&name, &vendor);
            let started = background::spawn("Search online", move || {
                if let Err(e) = shell::run(&shell::open(&url, None, shell::Os::THIS)) {
                    log::warning("Plugins", &format!("Search online: {e}"));
                    feed.start("Couldn't open the web search", "").failed(&e.to_string());
                }
            });
            if let Err(e) = started {
                log::error("Plugins", &format!("Couldn't start the web search: {e}"));
            }
        }
        _ => return false,
    }
    true
}

/// Whether a plugin is picked, for the page's keys.
pub fn has_pick() -> bool {
    PAGE.with(|p| p.borrow().picked.is_some())
}

fn arrived(app: &AppWindow, built: Result<Plugins, String>) {
    let _doing = crate::watchdog::doing("Plugins: show the list");
    let data = app.global::<PluginsData>();
    match built {
        Err(e) => {
            log::warning("Plugins", &format!("Couldn't read the library: {e}"));
            PAGE.with(|p| p.borrow_mut().wanted = None);
            // What was read before stays; only a page with nothing yet says why.
            if PAGE.with(|p| p.borrow().plugins.is_none()) {
                data.set_message(format!("Couldn't read your plugins: {e}").into());
            }
        }
        Ok(mut fresh) => {
            // What another page asked for before the library was read (a read that fails drops it too).
            let wanted = PAGE.with(|p| p.borrow_mut().wanted.take());
            // The filters and the pick carry over a fresh read.
            let kept = PAGE.with(|p| {
                let p = p.borrow();
                let old = p.plugins.as_ref()?;
                fresh.filters = old.filters.clone();
                fresh.needed_count = old.needed_count;
                if old.unused_years != fresh.unused_years {
                    fresh.set_unused_years(old.unused_years);
                }
                p.picked.and_then(|i| old.rows.get(i)).map(|r| (r.key.clone(), r.own.clone()))
            });
            // This session's folder fixes first, then what went: a folder added and binned or moved since stays out, as
            // Space has it (review E2-4).
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.folder_fixes = fresh.apply_folder_fixes(&p.folder_fixes);
            });
            fresh.forget_gone();
            clean::forget_binned(&mut fresh);
            // And the plugin files Space's clean-ups sent to the Recycle Bin, as Space leaves them out.
            fresh.forget_paths(&crate::space::plugin_files_in_bin());
            fresh.refilter();
            // What another page asked for before the first read comes first; with every unused plugin asked for, the
            // plugin picked before stays picked only when it's among them.
            let kept = kept.and_then(|(k, o)| fresh.find(&k, &o));
            let picked = match wanted.map(|w| show_in(&mut fresh, &w)) {
                Some(Some(row)) => Some(row),
                Some(None) => kept.filter(|row| fresh.shown.contains(row)),
                None => kept,
            };
            let empty = fresh.rows.is_empty();
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.years = fresh.year_counts();
                background::free_later(p.plugins.replace(fresh));
                // The same plugin at its new place, so its tab, DAW and "Every save" stay as they were.
                p.picked = picked;
            });
            data.set_message(
                if empty { "No plugins yet: the old app reads them when it scans your projects." } else { "" }.into(),
            );
            refresh(app);
            fill_cards(app);
            pick(app, picked);
        }
    }
}

/// Shows one plugin's card (by its line's name key and `own`), for another page (Space's Show on Plugins page): when
/// the filters hide it, they go (the sort stays); when it isn't in the list, the list shows what's found by its `name`.
/// Before the library is first read, it's shown once the read is back. The caller shows this page.
pub fn show_plugin(app: &AppWindow, key: &str, own: &str, name: &str) {
    show_wanted(app, Wanted::Plugin { key: key.to_owned(), own: own.to_owned(), name: name.to_owned() });
}

/// Shows only the plugins that take room and no song has used lately ("Big and unused", as the list's "Show them"
/// link does), for another page (Space's Show them on Plugins page). The caller shows this page.
pub fn show_unused(app: &AppWindow) {
    show_wanted(app, Wanted::Unused);
}

fn show_wanted(app: &AppWindow, wanted: Wanted) {
    let now = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        match p.plugins.as_mut() {
            Some(pl) => Some(show_in(pl, &wanted)),
            None => {
                p.wanted = Some(wanted);
                None
            }
        }
    });
    let Some(row) = now else { return };
    refresh(app);
    match row {
        Some(row) => {
            pick(app, Some(row));
            app.global::<PluginsData>().set_tab(0);
        }
        // The plugin picked before stays picked only when it's among those shown now.
        None => {
            let hidden = PAGE.with(|p| {
                let p = p.borrow();
                p.picked.is_some_and(|r| p.plugins.as_ref().is_some_and(|pl| !pl.shown.contains(&r)))
            });
            if hidden {
                pick(app, None);
            }
        }
    }
}

/// Sets the list up for what another page asked for: the plugin's row to pick (the filters go when they hide it, the
/// sort stays), or only "Big and unused" on (nothing to pick).
fn show_in(pl: &mut Plugins, wanted: &Wanted) -> Option<usize> {
    match wanted {
        Wanted::Plugin { key, own, name } => pl.show_one(key, own, name),
        Wanted::Unused => {
            pl.show_only_unused();
            None
        }
    }
}

/// How many years make a plugin unused on this page (its pick can change while the app runs), once the library is
/// read: Space counts the same ones.
pub fn unused_years() -> Option<u32> {
    PAGE.with(|p| p.borrow().plugins.as_ref().map(|pl| pl.unused_years))
}

/// How many moves have finished this session, whatever came of them: Space looks for old copies again after each.
pub fn moves_this_session() -> u64 {
    PAGE.with(|p| p.borrow().moves)
}

/// Where this session's Clean ups, uninstalls and moves took things from: Space leaves them out of the plugins it
/// counts, as this page does, until the old app measures again.
pub fn gone_this_session() -> Vec<String> {
    PAGE.with(|p| {
        let p = p.borrow();
        let binned = p.binned.iter().flat_map(|b| b.done.removed.iter().map(|r| r.path.clone()));
        let left_out = p.uninstalled.iter().chain(&p.removed_for_good).chain(&p.moved).cloned();
        binned.chain(left_out).collect()
    })
}

fn change(weak: &Weak<AppWindow>, f: impl FnOnce(&mut Filters)) {
    let _doing = crate::watchdog::doing("Plugins: search or filter");
    let Some(app) = weak.upgrade() else { return };
    PAGE.with(|p| {
        if let Some(pl) = p.borrow_mut().plugins.as_mut() {
            f(&mut pl.filters);
            pl.refilter();
        }
    });
    refresh(&app);
}

/// Shows the list as the filters leave it.
fn refresh(app: &AppWindow) {
    let data = app.global::<PluginsData>();
    // The lines are made as the list asks for them ([`Lines`]), once the page is let go below.
    PAGE.with(|p| {
        let p = p.borrow();
        let Some(pl) = p.plugins.as_ref() else { return };
        data.set_summary(pl.summary().into());
        data.set_count(pl.count_text().into());
        FILTER_MODELS.with(|m| {
            data.set_show_filter(m.show.view(&pl.filter(Group::Show)));
            data.set_kind_filter(m.kind.view(&pl.filter(Group::Kind)));
            data.set_format_filter(m.format.view(&pl.filter(Group::Format)));
            data.set_daw_filter(m.daw.view(&pl.daw_filter()));
        });
        data.set_sort(pl.filters.sort.name().into());
        data.set_sort_index(Sort::ALL.iter().position(|&s| s == pl.filters.sort).map_or(-1, |i| i as i32));
        data.set_has_sizes(pl.has_sizes());
        data.set_any_filter(pl.filters.any());
        if data.get_search().as_str() != pl.filters.search {
            data.set_search(pl.filters.search.as_str().into());
        }
        data.set_freeable(pl.freeable().into());
        data.set_unused_index(UNUSED_YEARS_CHOICES.iter().position(|&y| y == pl.unused_years).map_or(-1, |i| i as i32));
        let most = p.years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        let bars: Vec<YearBar> = p
            .years
            .iter()
            .map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most })
            .collect();
        data.set_years(ModelRc::new(VecModel::from(bars)));
        let (from, to) = year_strip::picked_places(&p.years, pl.filters.years).unwrap_or((-1, -1));
        data.set_years_from(from);
        data.set_years_to(to);
    });
    LINES.with(|m| m.notify.reset());
}

fn line(pl: &Plugins, i: usize) -> PluginListLine {
    let r = &pl.rows[i];
    let kind = match r.kind {
        mo_library::PluginKind::Instrument => "Instrument",
        mo_library::PluginKind::Effect => "Effect",
        mo_library::PluginKind::NoteEffect => "Note effect",
        mo_library::PluginKind::Unknown => "",
    };
    // A DAW's own device says its DAW already.
    let daws = if r.own.is_empty() { r.daws.join(", ") } else { String::new() };
    let details = [r.format_text(), kind.to_owned(), r.vendor.clone(), daws]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    PluginListLine {
        row: i as i32,
        name: r.name.as_str().into(),
        details: details.into(),
        songs: match r.songs {
            0 => "No songs".into(),
            1 => "1 song".into(),
            n => format!("{n} songs").into(),
        },
        last: r.last_used.map_or_else(|| "Never".to_owned(), |t| t.format("%b %Y").to_string()).into(),
        size: if r.bytes() > 0 { mo_space::size::size_text(r.bytes()).into() } else { SharedString::new() },
        state: if !r.installed {
            1
        } else if r.unused {
            2
        } else {
            0
        },
    }
}

fn fill_cards(app: &AppWindow) {
    let data = app.global::<PluginsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let Some(pl) = p.plugins.as_ref() else { return };
        let cards = pl.needed();
        data.set_cards_summary(pl.needed_summary(&cards).into());
        data.set_all_installed(pl.needed_count > 0 && pl.all_installed());
        data.set_has_missing(pl.any_missing());
        data.set_card_count(NEEDED_COUNT_CHOICES.iter().position(|&n| n == pl.needed_count).map_or(-1, |i| i as i32));
        let tiles: Vec<NeededCard> = cards
            .iter()
            .map(|c| NeededCard {
                row: c.row as i32,
                count: c.count.to_string().into(),
                label: c.label.into(),
                name: pl.rows[c.row].name.as_str().into(),
                details: c.details.as_str().into(),
                other: c.other_version.as_str().into(),
            })
            .collect();
        data.set_cards(ModelRc::new(VecModel::from(tiles)));
    });
}

/// Picks the plugin at `row` (or none) and fills its card.
fn pick(app: &AppWindow, row: Option<usize>) {
    let _doing = crate::watchdog::doing("Plugins: pick a plugin");
    let changed = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let row = row.filter(|&r| p.plugins.as_ref().is_some_and(|pl| r < pl.rows.len()));
        let changed = p.picked != row;
        p.picked = row;
        if changed {
            p.use_daw.clear();
            p.every_save = false;
        }
        changed
    });
    clean::picked_now(PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|r| p.plugins.as_ref()?.rows.get(r)).map(|r| (r.key.clone(), r.own.clone()))
    }));
    let data = app.global::<PluginsData>();
    data.set_selected(PAGE.with(|p| p.borrow().picked.map_or(-1, |r| r as i32)));
    if changed {
        data.set_tab(0);
    }
    fill_panel(app);
    ask_copies(app);
}

/// The card's buttons and the right-click menu: Clean up only for a plugin with folders it could look at.
fn fill_actions(app: &AppWindow) {
    let data = app.global::<PluginsData>();
    let (shortcuts, picked) = PAGE.with(|p| (p.borrow().shortcuts.clone(), p.borrow().picked));
    let (can_clean, can_uninstall, can_move) = (
        picked.is_some_and(clean::can_clean),
        picked.is_some_and(uninstall::can_uninstall),
        picked.is_some_and(moving::can_move),
    );
    let enabled = |id: &str| match id {
        "plugins.cleanUp" => can_clean,
        "plugins.uninstall" => can_uninstall,
        "plugins.move" => can_move,
        _ => true,
    };
    let menu: Vec<MenuItem> = PLUGINS_ACTIONS
        .iter()
        .map(|a| MenuItem {
            what: a.what.into(),
            key: shortcuts.key_of(a.id).into(),
            id: a.id.into(),
            enabled: enabled(a.id),
        })
        .collect();
    data.set_menu(ModelRc::new(VecModel::from(menu)));
    let actions: Vec<CardAction> = PLUGINS_ACTIONS
        .iter()
        .map(|a| CardAction {
            text: match a.id {
                "plugins.projects" => "Show in Songs",
                "plugins.cleanUp" => "Clean up…",
                "plugins.uninstall" => "Uninstall…",
                "plugins.move" => "Move to another drive…",
                _ => "Search online",
            }
            .into(),
            tip: a.what.into(),
            key: shortcuts.key_of(a.id).into(),
            id: a.id.into(),
            enabled: enabled(a.id),
            main: false,
            danger: a.id == "plugins.uninstall",
        })
        .collect();
    data.set_actions(ModelRc::new(VecModel::from(actions)));
}

fn fill_panel(app: &AppWindow) {
    let data = app.global::<PluginsData>();
    let card = PAGE.with(|p| {
        let p = p.borrow();
        let pl = p.plugins.as_ref()?;
        Some((pl.card(p.picked?)?, pl.sizes_note(), pl.scanner_note()))
    });
    let Some((card, sizes, scanner)) = card else {
        data.set_has_pick(false);
        return;
    };
    data.set_has_pick(true);
    data.set_name(card.name.into());
    data.set_facts(card.facts.into());
    data.set_state(match card.state {
        State::Installed => 0,
        State::Missing => 1,
        State::Unused => 2,
    });
    data.set_state_text(card.state.text().into());
    data.set_other(card.other_version.into());
    data.set_room_summary(card.room.summary.into());
    data.set_room_shared(card.room.shared.into());
    let bars: Vec<RoomBar> = card
        .room
        .parts
        .iter()
        .map(|b| RoomBar { name: b.name.into(), size: b.size.as_str().into(), share: b.share, kind: i32::from(b.kind) })
        .collect();
    data.set_room_bars(ModelRc::new(VecModel::from(bars)));
    let folders: Vec<RoomFolder> = card
        .room
        .folders
        .iter()
        .enumerate()
        .map(|(i, f)| RoomFolder {
            kind: f.kind.into(),
            path: shell::wrappable_path(&f.path).into(),
            size: f.size.as_str().into(),
            shared: f.shared.as_str().into(),
            added: f.added,
            can_say: f.can_say,
            at: i as i32,
        })
        .collect();
    PAGE.with(|p| p.borrow_mut().room_paths = card.room.folders.iter().map(|f| f.path.clone()).collect());
    data.set_can_add_folder(PAGE.with(|p| {
        let p = p.borrow();
        p.picked.is_some_and(|r| p.plugins.as_ref().is_some_and(|pl| pl.can_add_folder(r)))
    }));
    data.set_room_folders(ModelRc::new(VecModel::from(folders)));
    data.set_sizes_note(sizes.into());
    data.set_scanner_note(scanner.into());
    fill_actions(app);
    clean::fill(app);
    fill_uses(app);
}

fn fill_uses(app: &AppWindow) {
    let data = app.global::<PluginsData>();
    let uses = PAGE.with(|p| {
        let p = p.borrow();
        Some((p.plugins.as_ref()?.songs_using(p.picked?, p.every_save, &p.use_daw), p.use_daw.clone(), p.every_save))
    });
    let Some((uses, daw, every)) = uses else { return };
    PAGE.with(|p| p.borrow_mut().uses = uses.lines.iter().map(|l| l.save).collect());
    let choices: Vec<(String, String)> = uses.daws.iter().map(|d| (d.clone(), String::new())).collect();
    let picked: Vec<String> = if daw.is_empty() { Vec::new() } else { vec![daw.clone()] };
    FILTER_MODELS.with(|m| data.set_use_daws(m.use_daw.view(&mo_app::filters::show(&choices, &picked))));
    data.set_use_daw(daw.into());
    data.set_every_save(every);
    data.set_uses_note(uses.note.into());
    // The tab says how many songs, like "Used in 12 songs".
    let tabs = [uses.title.as_str(), TABS[1], TABS[2]];
    data.set_tabs(strings(tabs));
    let rows: Vec<UseRow> = uses
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| UseRow {
            name: l.name.as_str().into(),
            daw: l.daw.as_str().into(),
            date: l.date.as_str().into(),
            saves: l.saves_text().into(),
            at: i as i32,
        })
        .collect();
    data.set_uses(ModelRc::new(VecModel::from(rows)));
}

fn ask_copies(app: &AppWindow) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.copies_asked += 1;
        let name = p.picked.and_then(|r| p.plugins.as_ref()?.rows.get(r)).map(|r| r.name.clone())?;
        Some((p.copies_asked, p.library.clone(), name))
    });
    let data = app.global::<PluginsData>();
    data.set_copies(ModelRc::new(VecModel::from(Vec::<CopyRow>::new())));
    data.set_copies_note("Reading…".into());
    if let Some(job) = job {
        COPIES.with(|c| {
            if let Some(c) = c.borrow().as_ref() {
                c.ask(job);
            }
        });
    }
}

fn copies_arrived(app: &AppWindow, asked: u64, found: &[ScannedPlugin]) {
    let note = PAGE.with(|p| {
        let p = p.borrow();
        if p.copies_asked != asked {
            return None;
        }
        let pl = p.plugins.as_ref()?;
        Some(if found.is_empty() { no_copies_note(pl, p.picked?) } else { String::new() })
    });
    let Some(note) = note else { return };
    let data = app.global::<PluginsData>();
    data.set_copies_note(note.into());
    let rows: Vec<CopyRow> = copy_lines(found)
        .into_iter()
        .map(|c| CopyRow {
            format: c.format.into(),
            name: c.name.into(),
            details: c.details.into(),
            id: c.id.into(),
            path: shell::wrappable_path(&c.path).into(),
            note: c.note.into(),
        })
        .collect();
    data.set_copies(ModelRc::new(VecModel::from(rows)));
}

/// The chip filter a page name means ("show", "kind", "format"); `None` for the DAWs.
fn group_named(name: &str) -> Option<Group> {
    Group::ALL.into_iter().find(|g| g.name().eq_ignore_ascii_case(name))
}

fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(|s| SharedString::from(s.as_ref())).collect::<Vec<_>>()))
}

/// For the screenshots, with no event loop to send work back: shows `plugins` as if just read, picks `row` on tab
/// `tab`, and shows `copies` as its installed copies.
pub fn show_sample(app: &AppWindow, plugins: Plugins, row: Option<usize>, tab: i32, copies: &[ScannedPlugin]) {
    PAGE.with(|p| p.borrow_mut().plugins = None);
    arrived(app, Ok(plugins));
    pick(app, row);
    app.global::<PluginsData>().set_tab(tab);
    let asked = PAGE.with(|p| p.borrow().copies_asked);
    copies_arrived(app, asked, copies);
}

/// For the screenshots: the picked plugin's Clean up list as it would show, from made-up lines (path, kind, size, why
/// it stays, can go, ticked).
pub fn show_clean_sample(
    app: &AppWindow,
    name: &str,
    intro: &str,
    rows: &[(&str, &str, &str, &str, bool, bool)],
    freed: &str,
) {
    show_card_sample(
        app,
        &format!("Clean up {name}"),
        "",
        intro,
        rows,
        freed,
        ("Send to the Recycle Bin", "Keep everything"),
    );
}

/// For the screenshots: the On disk tab's card (Clean up's or Uninstall's) with made-up lines (path, kind, size, why it
/// stays, may go, ticked) and its two buttons' words.
pub fn show_card_sample(
    app: &AppWindow,
    title: &str,
    warning: &str,
    intro: &str,
    rows: &[(&str, &str, &str, &str, bool, bool)],
    freed: &str,
    (go, keep): (&str, &str),
) {
    let data = app.global::<PluginsData>();
    data.set_tab(1);
    data.set_clean_shown(true);
    data.set_clean_stage(1);
    data.set_clean_title(title.into());
    data.set_clean_warning(warning.into());
    data.set_clean_intro(intro.into());
    let rows: Vec<crate::CleanRow> = rows
        .iter()
        .map(|(path, kind, size, why, can, ticked)| crate::CleanRow {
            path: shell::wrappable_path(path).into(),
            kind: (*kind).into(),
            size: (*size).into(),
            why: (*why).into(),
            can: *can,
            ticked: *ticked,
        })
        .collect();
    data.set_clean_can_go(rows.iter().any(|r| r.ticked));
    data.set_clean_rows(ModelRc::new(VecModel::from(rows)));
    data.set_clean_freed(freed.into());
    data.set_clean_go_text(go.into());
    data.set_clean_keep_text(keep.into());
}
