//! Fills the Presets page from `mo_app::presets` and answers it. Everything slow runs on low-priority threads (rules 12
//! and 21) and only what they find comes back to the window:
//!
//! - **Sounds**, read once the page is first shown: the library (read-only), then the newest save of each song, a few
//!   at a time over every core, each batch shown as it's read; then the preset files in the plugins' and Bitwig's
//!   folders. Your part fixes are read from and written to the app's own `presets.db`.
//! - **Curves**, read once the section is first opened: what `curves.db` already holds at once, then every save that
//!   changed since is read again and the list shown afresh.
//!
//! The lists are models that make each line only when the list asks for it, so a long list costs nothing to show
//! (rule 12: virtualised lists).
//!
//! Send to Bitwig makes the sound's Bitwig preset and puts it into the project open in Bitwig through the Bitwig link
//! (`mo-dawlink`, [`send_to_bitwig`]); when Bitwig isn't there, the preset is shown in its folder to drag in.
//! Audition in Bitwig does the same on a track of its own with a short part playing on it
//! (`mo_app::preset_audition`), to hear it first: Stop in Bitwig takes the track out again, Keep in Bitwig leaves it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::{Feed, Job};
use mo_app::background::{self, Newest};
use mo_app::bitwig_convert as bitwig;
use mo_app::finder::Kind;
use mo_app::log;
use mo_app::places::Place;
use mo_app::presets::{self as logic, CurveFilters, Filters, Item, Section, act, curve_act};
use mo_app::shortcuts::{CURVES_ACTIONS, PRESETS_ACTIONS, Shortcuts, what_of};
use mo_app::year_strip;
use mo_library::Library;
use mo_presets::bitwig::{self as bw, Templates};
use mo_presets::curves::{self, Curve, CurvesDb};
use mo_presets::export;
use mo_presets::file_store;
use mo_presets::filter::Choice;
use mo_presets::fixes::{self, RoleFixes};
use mo_presets::library::{self as plib, Source};
use mo_presets::role::{Role, Thresholds};
use mo_presets::sound_store::{self, SoundStore};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel, Weak};

use crate::filters::FilterModels;
use crate::{
    ActionKey, AppWindow, CardAction, CurveCardInfo, CurveLineInfo, CurvePlace, MenuItem, PartChip, PresetCard,
    PresetFact, PresetLine, PresetsData, YearBar,
};

/// How many saves are read before what they hold is shown.
const BATCH: usize = 24;

/// What the page knows, on the window's thread.
#[derive(Default)]
struct Page {
    section: Section,
    /// The Sounds section: every line, those shown (places in `items`), and the picked one (a place in `items`).
    /// Shared with the filtering worker, which works out what's shown off the window's thread.
    /// Never copied on the window's thread (100k lines or more): part fixes are laid over it (`fixed_now`), and lines
    /// read while a filtering holds it wait in `pending` until it lets go ([`take_waiting`]).
    items: Arc<Vec<Item>>,
    pending: Vec<Item>,
    /// Whether Ctrl+K has been handed the lines (once they're all read).
    finder_has: bool,
    shown: Vec<usize>,
    /// Counts the filterings asked for, so an older one finishing late is dropped (newest wins).
    asked: u64,
    /// The last filtering taken: while it's behind `asked`, what's shown is still being worked out.
    answered: u64,
    /// Filter here, not on the worker (the screenshots, which show the page at once).
    filter_here: bool,
    filters: Filters,
    picked: Option<usize>,
    years: Vec<(i32, usize)>,
    /// The plugins in the list, most used first, each with how many lines it has.
    plugins: Vec<(String, usize)>,
    /// The filters as their chips and menus show them now.
    chips: logic::FilterChips,
    songs: usize,
    files: usize,
    started: bool,
    reading: Option<(usize, usize)>,
    failed: Option<String>,
    /// The name of the song the Song filter picks, as Songs gave it.
    song_name: String,
    /// Every song's newest save the last reading looked at, with its time then.
    looked_at: HashMap<String, i64>,
    exporting: bool,
    message: String,
    /// The line being auditioned in Bitwig ([`logic::audition_id`]) and how far it's got.
    hearing: Option<(String, logic::Hearing)>,
    /// The auditioned line's name, for the strip under the list that shows the audition whatever line is picked.
    hearing_name: String,
    /// The Curves section.
    curves: Vec<Curve>,
    /// What the search and the order go by, one for each of `curves`, worked out once (H-5).
    curve_keys: Vec<logic::CurveKey>,
    /// The curve whose places the card shows: they're only made again when another curve is picked or it changes.
    curve_places_of: Option<String>,
    curve_shown: Vec<usize>,
    curve_filters: CurveFilters,
    /// The Found in filter as its chips show it now.
    curve_found_in: mo_app::filters::Filter,
    /// The picked curve, by its id (a rescan may move it in the list).
    curve_picked: Option<String>,
    curves_started: bool,
    curves_reading: Option<(usize, usize)>,
    curves_failed: Option<String>,
    curve_message: String,
    /// Where things are: the library (exports read the export folder from it as they start), the app's data folder,
    /// Bitwig's folders.
    library: PathBuf,
    data_dir: PathBuf,
    bitwig: Vec<PathBuf>,
    /// The Bitwig presets that Send to Bitwig builds from, once found.
    templates: Option<Arc<Templates>>,
    feed: Feed,
    shortcuts: Shortcuts,
    /// The last time the list was shown afresh while sounds are still coming in.
    shown_at: Option<Instant>,
    /// Part fixes made since the sounds started being read, by fingerprint (None: the fix was taken away): laid over
    /// the lines wherever they're shown, filtered or found, and given to the sounds read after one was made.
    fixed_now: logic::Fixes,
    /// A line Ctrl+K asked for, picked once the filtering asked for with it is done; and that line, to pick once the
    /// list is on show.
    to_pick: Option<usize>,
    pick_now: Option<usize>,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::default();
    static LINES: Rc<Lines> = Rc::new(Lines::default());
    static CURVE_LINES: Rc<CurveLines> = Rc::new(CurveLines::default());
    static FILTER_MODELS: ChipModels = ChipModels::default();
    static APP: RefCell<Option<Weak<AppWindow>>> = const { RefCell::new(None) };
    /// Works out what the filters show, off the window's thread, newest ask first.
    static FILTERER: RefCell<Option<Newest<FilterAsk>>> = const { RefCell::new(None) };
}

/// What the filtering worker is asked: the lines, the filters, and the plugins when they're to be counted again
/// (`None`: the ones the page has).
struct FilterAsk {
    id: u64,
    items: Arc<Vec<Item>>,
    fixes: logic::Fixes,
    filters: Filters,
    plugins: Option<Vec<(String, usize)>>,
}

/// What the filters show, worked out together, so the list and its chips change at once (a click on a chip is
/// turned back into its choice through the chips on show).
struct FilterAnswer {
    id: u64,
    shown: Vec<usize>,
    years: Vec<(i32, usize)>,
    chips: logic::FilterChips,
    plugins: Vec<(String, usize)>,
}

impl FilterAsk {
    fn answer(self) -> FilterAnswer {
        let plugins = self.plugins.unwrap_or_else(|| logic::plugins(&self.items));
        let found = logic::filtered_with(&self.items, &self.filters, &self.fixes);
        let chips = logic::filter_chips(&found.counts, &plugins, &self.filters);
        FilterAnswer { id: self.id, shown: found.shown, years: found.years, chips, plugins }
    }
}

/// The filters' chips and menus on screen, kept from one showing to the next.
#[derive(Default)]
struct ChipModels {
    part: FilterModels,
    plugin: FilterModels,
    format: FilterModels,
    kind: FilterModels,
    from: FilterModels,
    found_in: FilterModels,
}

/// The sounds list, made a line at a time as the list asks.
#[derive(Default)]
struct Lines {
    notify: ModelNotify,
}

impl Model for Lines {
    type Data = PresetLine;

    fn row_count(&self) -> usize {
        PAGE.with(|p| p.try_borrow().map_or(0, |p| p.shown.len()))
    }

    fn row_data(&self, row: usize) -> Option<PresetLine> {
        PAGE.with(|p| {
            let p = p.try_borrow().ok()?;
            let at = *p.shown.get(row)?;
            let l = logic::line(&p.items.get(at)?.as_fixed(&p.fixed_now));
            Some(PresetLine {
                row: row as i32,
                name: l.name.into(),
                under: l.under.into(),
                part: l.part.into(),
                colour: l.colour,
                part_tip: l.part_tip.into(),
                fixed: l.fixed,
                from: l.from.into(),
                kind: l.kind.into(),
                format: l.format.into(),
                picked: p.picked == Some(at),
            })
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

#[derive(Default)]
struct CurveLines {
    notify: ModelNotify,
}

impl Model for CurveLines {
    type Data = CurveLineInfo;

    fn row_count(&self) -> usize {
        PAGE.with(|p| p.try_borrow().map_or(0, |p| p.curve_shown.len()))
    }

    fn row_data(&self, row: usize) -> Option<CurveLineInfo> {
        PAGE.with(|p| {
            let p = p.try_borrow().ok()?;
            let c = p.curves.get(*p.curve_shown.get(row)?)?;
            let l = logic::curve_line(c);
            Some(CurveLineInfo {
                row: row as i32,
                name: l.name.into(),
                under: l.under.into(),
                found: l.found.into(),
                length: l.length.into(),
                hidden: l.hidden,
                shape: l.shape.into(),
                picked: p.curve_picked.as_deref() == Some(c.id.as_str()),
            })
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// Wires the page. `library` is the C# app's `library.db`, only ever read.
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.data_dir = mo_app::paths::data_dir();
        p.feed = feed;
        p.shortcuts = shortcuts.clone();
        // The filters show from the start, before any song is read (with no lines, each choice counts none).
        refilter(&mut p);
        (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_keys, &p.curve_filters);
    });
    wire(app);
    finder_opens();
    // Ctrl+K finds your presets before this page is first shown: they're read once, in the background.
    crate::finder::when_opening(|app| {
        let start = PAGE.with(|p| !std::mem::replace(&mut p.borrow_mut().started, true));
        if start {
            read_sounds(app);
        }
    });
    // The app may start on this page: it was shown before the page knew where the library is.
    if app.get_place() == Place::Presets.index() as i32 {
        shown(app);
    }
}

/// Says how Ctrl+K opens one of the Sounds section's lines: on this page, picked.
fn finder_opens() {
    crate::finder::on_open(Kind::Preset, |app, hit, _| {
        // Found by what it is, not where it was: a read again puts the lines in another order (M-1).
        let at = PAGE.with(|p| p.borrow().items.iter().position(|i| logic::audition_id(i) == hit.id));
        let Some(at) = at else { return false };
        crate::go_to(app, Place::Presets.index() as i32);
        show_preset(app, at);
        true
    });
}

/// Shows one line of the Sounds section, by its place in the lines (Ctrl+K): when the filters hide it, they go.
fn show_preset(app: &AppWindow, at: usize) {
    if PAGE.with(|p| p.borrow().section) != Section::Sounds {
        pick_section(app, Section::Sounds);
    }
    if PAGE.with(|p| p.borrow().shown.contains(&at)) {
        pick(app, Some(at));
        return;
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // A synth that plays nothing is hidden unless that's shown: keep it shown, or the line stays hidden (H-1).
        let plays_nothing = p.items.get(at).is_some_and(|i| i.row.plays_nothing);
        p.filters = Filters { unused: p.filters.unused || plays_nothing, ..p.filters.only_order() };
        p.to_pick = Some(at);
        refilter(&mut p);
    });
    show_all(app);
}

/// A key was changed in Settings: the buttons, card and menu show the new keys at once.
pub fn show_keys(app: &AppWindow, shortcuts: &Shortcuts) {
    PAGE.with(|p| p.borrow_mut().shortcuts = shortcuts.clone());
    let key = shortcuts.key_of(act::EXPORT_SHOWN).to_owned();
    let data = app.global::<PresetsData>();
    data.set_export_shown_key(ActionKey { what: what_of(act::EXPORT_SHOWN).into(), key: key.into() });
    data.set_keep_audition_key(shortcuts.key_of(act::KEEP).into());
    show_card(app);
    show_curve_card(app);
}

fn wire(app: &AppWindow) {
    APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
    let data = app.global::<PresetsData>();
    data.set_lines(ModelRc::from(LINES.with(|m| m.clone()) as Rc<dyn Model<Data = PresetLine>>));
    data.set_curve_lines(ModelRc::from(CURVE_LINES.with(|m| m.clone()) as Rc<dyn Model<Data = CurveLineInfo>>));
    let key = PAGE.with(|p| p.borrow().shortcuts.key_of(act::EXPORT_SHOWN).to_owned());
    data.set_export_shown_key(ActionKey { what: what_of(act::EXPORT_SHOWN).into(), key: key.into() });

    let weak = app.as_weak();
    data.on_section_picked(move |i| {
        if let (Some(app), Some(s)) = (weak.upgrade(), usize::try_from(i).ok().and_then(Section::from_index)) {
            pick_section(&app, s);
        }
    });
    let weak = app.as_weak();
    data.on_search_edited(move |t| {
        let Some(app) = weak.upgrade() else { return };
        match PAGE.with(|p| p.borrow().section) {
            Section::Sounds => change(&app, |f| f.search = t.to_string()),
            Section::Curves => change_curves(&app, |f| f.search = t.to_string()),
        }
    });
    let weak = app.as_weak();
    // A filter's choice was picked or unpicked, by its words.
    data.on_toggle(move |chip, choice| {
        let Some(app) = weak.upgrade() else { return };
        if chip == logic::FOUND_IN {
            change_curves(&app, |f| f.toggle_kind(&choice));
        } else {
            change(&app, |f| f.toggle(&chip, &choice));
        }
    });
    let weak = app.as_weak();
    data.on_clear_chip(move |chip| {
        let Some(app) = weak.upgrade() else { return };
        if chip == logic::FOUND_IN {
            change_curves(&app, |f| f.kinds.clear());
        } else {
            change(&app, |f| f.clear_chip(&chip));
        }
    });
    let weak = app.as_weak();
    data.on_order_picked(move |column| {
        if let (Some(app), Some(order)) = (weak.upgrade(), logic::Order::from_column(&column)) {
            change(&app, |f| f.order_by(order));
        }
    });
    let weak = app.as_weak();
    data.on_clear(move || {
        if let Some(app) = weak.upgrade() {
            change(&app, |f| *f = f.cleared());
        }
    });
    let weak = app.as_weak();
    data.on_clear_song(move || {
        if let Some(app) = weak.upgrade() {
            change(&app, |f| f.song = None);
        }
    });
    let weak = app.as_weak();
    data.on_unused_toggled(move |on| {
        if let Some(app) = weak.upgrade() {
            change(&app, |f| f.unused = on);
        }
    });
    let weak = app.as_weak();
    data.on_years_picked(move |a, b| {
        let span = PAGE.with(|p| year_strip::year_span(&p.borrow().years, a, b));
        if let (Some(app), Some(span)) = (weak.upgrade(), span) {
            change(&app, |f| f.years = Some(span));
        }
    });
    let weak = app.as_weak();
    data.on_years_cleared(move || {
        if let Some(app) = weak.upgrade() {
            change(&app, |f| f.years = None);
        }
    });
    let weak = app.as_weak();
    data.on_select(move |row| {
        if let Some(app) = weak.upgrade() {
            let at = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().shown.get(r).copied()));
            pick(&app, at);
        }
    });
    let weak = app.as_weak();
    data.on_step(move |by| {
        if let Some(app) = weak.upgrade() {
            step(&app, by);
        }
    });
    let weak = app.as_weak();
    data.on_close_panel(move || {
        if let Some(app) = weak.upgrade() {
            pick(&app, None);
            // Closing the card stops the audition, as on the MIDI page (the 6 Nov review's X3-1).
            stop_audition_now(&app);
        }
    });
    let weak = app.as_weak();
    data.on_stop_audition(move || {
        if let Some(app) = weak.upgrade() {
            stop_audition_now(&app);
        }
    });
    let weak = app.as_weak();
    data.on_keep_audition(move || {
        if let Some(app) = weak.upgrade() {
            keep_audition(&app);
        }
    });
    data.on_panel_resized(|w| log::info("Presets", &format!("Panel width {w:.0}")));
    let weak = app.as_weak();
    data.on_menu_chosen(move |id| {
        if let Some(app) = weak.upgrade() {
            run_sound_action(&app, &id);
        }
    });
    let weak = app.as_weak();
    data.on_export_shown(move || {
        if let Some(app) = weak.upgrade() {
            export_shown(&app);
        }
    });

    let weak = app.as_weak();
    data.on_curve_hidden_toggled(move |on| {
        if let Some(app) = weak.upgrade() {
            change_curves(&app, |f| f.hidden = on);
        }
    });
    let weak = app.as_weak();
    data.on_curve_select(move |row| {
        if let Some(app) = weak.upgrade() {
            let id = PAGE.with(|p| {
                let p = p.borrow();
                usize::try_from(row).ok().and_then(|r| p.curve_shown.get(r)).map(|&i| p.curves[i].id.clone())
            });
            pick_curve(&app, id);
        }
    });
    let weak = app.as_weak();
    data.on_curve_step(move |by| {
        if let Some(app) = weak.upgrade() {
            step_curve(&app, by);
        }
    });
    let weak = app.as_weak();
    data.on_curve_drag(move || {
        if let Some(app) = weak.upgrade() {
            carry_curve(&app);
        }
    });
    let weak = app.as_weak();
    data.on_curve_drag_let_go(move || {
        if let Some(app) = weak.upgrade() {
            crate::drag::let_go(&app);
        }
    });
    let weak = app.as_weak();
    data.on_curve_chosen(move |id| {
        if let Some(app) = weak.upgrade() {
            run_curve_action(&app, &id);
        }
    });
    let weak = app.as_weak();
    data.on_rename_done(move |name| {
        if let Some(app) = weak.upgrade() {
            app.global::<PresetsData>().set_renaming(false);
            rename_curve(&app, name.to_string());
        }
    });
}

/// The page came into view: the sounds are read the first time.
pub fn shown(app: &AppWindow) {
    // Not connected yet (the app starting on this page, before `connect`): `connect` shows it once it's wired.
    if APP.with(|a| a.borrow().is_none()) {
        return;
    }
    let start = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        !std::mem::replace(&mut p.started, true)
    });
    if start {
        read_sounds(app);
    }
    show_all(app);
}

/// Opens the page on one song's sounds (a song's Presets tab), by its save's path; `name` is the song's, for its chip.
/// A save the page hasn't read (saved since the page read your songs) has the songs read again (H-4).
/// `modified` is the save's time as Songs has it: a save looked at with that time (even one that gave no sounds, or a
/// copy in the export folder) isn't read again (U-2).
pub fn show_song(app: &AppWindow, save: &str, name: &str, modified: i64) {
    let again = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.section = Section::Sounds;
        p.song_name = name.to_owned();
        p.started && p.reading.is_none() && logic::keeps_settings(save) && p.looked_at.get(save) != Some(&modified)
    });
    if again {
        read_again(app);
    }
    change(app, |f| *f = Filters { song: Some(save.to_owned()), unused: f.unused, ..f.only_order() });
    // The page may have been left on Curves: the key sheet lists the Sounds keys now (H-12).
    crate::refresh_keys(app);
}

fn pick_section(app: &AppWindow, s: Section) {
    // The search box is shared, but each section's filters keep their own words (the box always edits the section on
    // show), so a switch only puts the other section's words in the box (`show_all`).
    let first_curves = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.section = s;
        s == Section::Curves && !std::mem::replace(&mut p.curves_started, true)
    });
    if first_curves {
        read_curves(app);
    }
    show_all(app);
    // The key sheet lists the section's own keys.
    crate::refresh_keys(app);
}

/// Changes the sounds' filters and shows the list again.
fn change(app: &AppWindow, f: impl FnOnce(&mut Filters)) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        f(&mut p.filters);
        refilter(&mut p);
    });
    show_all(app);
}

fn change_curves(app: &AppWindow, f: impl FnOnce(&mut CurveFilters)) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        f(&mut p.curve_filters);
        (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_keys, &p.curve_filters);
    });
    show_all(app);
}

/// Works out what's shown from the filters, off the window's thread: the list as it is stays on show until the new
/// one is ready (a moment), and a newer ask wins over an older one still going. `count_plugins`: the lines changed,
/// so the plugins are counted again too.
fn refilter(p: &mut Page) {
    ask_filter(p, false);
}

fn ask_filter(p: &mut Page, count_plugins: bool) {
    // Lines left waiting (a filtering that never answered) go in now, if nothing holds the list.
    take_waiting(p);
    p.asked += 1;
    let ask = FilterAsk {
        id: p.asked,
        items: Arc::clone(&p.items),
        fixes: p.fixed_now.clone(),
        filters: p.filters.clone(),
        plugins: (!count_plugins).then(|| p.plugins.clone()),
    };
    if p.filter_here || p.items.is_empty() {
        take_filtered(p, ask.answer());
        return;
    }
    let ask = FILTERER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            match Newest::start("Presets filter", |ask: FilterAsk| {
                // The ask (and its hold on the lines) is let go here, before the window takes the answer: the
                // window can then put in the lines read meanwhile.
                let answer = background::on_pool(move || ask.answer());
                on_window(move |app| {
                    let current = PAGE.with(|p| {
                        let mut p = p.borrow_mut();
                        let current = take_filtered(&mut p, answer);
                        // The filtering has let go of the lines: those read meanwhile go in, filtered next. An older
                        // answer leaves that to the newer filtering, which holds the lines still.
                        if current && take_waiting(&mut p) && (p.reading.is_none() || due(&p)) {
                            p.shown_at = Some(Instant::now());
                            ask_filter(&mut p, true);
                        }
                        current
                    });
                    give_finder(app);
                    if current {
                        show_all(app);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error(
                    "Presets",
                    &format!("Couldn't start the list's filtering thread, so it filters here: {e}"),
                ),
            }
        }
        match w.as_ref() {
            Some(worker) => {
                worker.ask(ask);
                None
            }
            None => Some(ask),
        }
    });
    if let Some(ask) = ask {
        take_filtered(p, ask.answer());
    }
}

/// Whether the list may be filtered afresh while reading: at most four times a second (the plugin list and the
/// filtering go over every line).
fn due(p: &Page) -> bool {
    p.shown_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(250))
}

/// Puts the lines read while a filtering held the list ([`Page::pending`]) in, in place, once nothing else holds it.
/// Whether any went in.
fn take_waiting(p: &mut Page) -> bool {
    if p.pending.is_empty() {
        return false;
    }
    let Page { items, pending, .. } = p;
    let Some(all) = Arc::get_mut(items) else { return false };
    // Room for at least twice the lines when it grows, so a big batch (every preset file at once) moves the list
    // rarely.
    if all.capacity() - all.len() < pending.len() {
        all.reserve(pending.len().max(all.len()));
    }
    all.append(pending);
    true
}

/// Ctrl+K gets the lines once they're all read and in: handed over while reading, it would hold the list.
fn give_finder(app: &AppWindow) {
    let items = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let ready = p.started && p.reading.is_none() && p.pending.is_empty() && !p.finder_has;
        p.finder_has |= ready;
        ready.then(|| Arc::clone(&p.items))
    });
    if let Some(items) = items {
        crate::finder::add_source(app, "presets", Arc::new(mo_app::finder::Presets(items)));
    }
}

/// A filtering is done: the page takes it, unless a newer one has been asked for since. Whether it took it.
fn take_filtered(p: &mut Page, a: FilterAnswer) -> bool {
    if a.id != p.asked {
        return false;
    }
    p.answered = a.id;
    p.shown = a.shown;
    p.years = a.years;
    p.chips = a.chips;
    p.plugins = a.plugins;
    if let Some(at) = p.to_pick.take()
        && p.shown.contains(&at)
    {
        p.pick_now = Some(at);
    }
    true
}

/// Puts everything the page knows on screen, then picks the line Ctrl+K asked for once the list shows it.
fn show_all(app: &AppWindow) {
    show_everything(app);
    let pick_now = PAGE.with(|p| p.borrow_mut().pick_now.take());
    if let Some(at) = pick_now {
        pick(app, Some(at));
    }
}

fn show_everything(app: &AppWindow) {
    let data = app.global::<PresetsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_section(p.section.index() as i32);
        // The heading of the column the list goes by is lit, with which way (the UX review of 10 Nov, B5).
        data.set_order_column(p.filters.order.column().into());
        data.set_order_reversed(p.filters.reversed);
        let (hearing, playing) = match &p.hearing {
            Some((_, logic::Hearing::Starting)) => {
                (format!("Putting {} into Bitwig to hear it\u{2026}", p.hearing_name), false)
            }
            Some((_, logic::Hearing::Playing)) => {
                (format!("Hearing {} in Bitwig, on a track of its own.", p.hearing_name), true)
            }
            _ => (String::new(), false),
        };
        data.set_hearing(hearing.into());
        data.set_hearing_playing(playing);
        let search = match p.section {
            Section::Sounds => &p.filters.search,
            Section::Curves => &p.curve_filters.search,
        };
        if data.get_search().as_str() != search {
            data.set_search(search.as_str().into());
        }
        let f = &p.filters;
        FILTER_MODELS.with(|m| {
            data.set_part_filter(m.part.view(&p.chips.part));
            data.set_plugin_filter(m.plugin.view(&p.chips.plugin));
            data.set_format_filter(m.format.view(&p.chips.format));
            data.set_type_filter(m.kind.view(&p.chips.kind));
            data.set_from_filter(m.from.view(&p.chips.from));
            data.set_curve_found_in(m.found_in.view(&p.curve_found_in));
        });
        // The song's name as Songs gave it: no looking through every line on each show.
        data.set_song(
            f.song
                .as_ref()
                .map(|s| if p.song_name.is_empty() { s.as_str() } else { &p.song_name })
                .unwrap_or("")
                .into(),
        );
        data.set_any_filter(f.any());
        data.set_unused(f.unused);
        data.set_shown(p.shown.len() as i32);
        let reading = p.reading.is_some();
        let (mut title, mut text) = logic::empty_words(
            p.items.len() + p.pending.len(),
            p.shown.len(),
            reading,
            p.answered != p.asked,
            f.any(),
            p.failed.as_deref(),
        );
        // A song with nothing read from it says why, not "No sounds match" (only looked for when nothing shows).
        if !title.is_empty()
            && !reading
            && let Some(save) = &f.song
            && !p.items.iter().any(|i| i.path() == save)
        {
            (title, text) = logic::song_not_read(save);
        }
        data.set_empty_title(title.into());
        data.set_empty_text(text.into());
        let most = p.years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        let bars: Vec<YearBar> = p
            .years
            .iter()
            .map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most })
            .collect();
        data.set_years(ModelRc::new(VecModel::from(bars)));
        let (from, to) = year_strip::picked_places(&p.years, f.years).unwrap_or((-1, -1));
        data.set_years_from(from);
        data.set_years_to(to);
        data.set_exporting(p.exporting);

        // The curves.
        data.set_curve_hidden(p.curve_filters.hidden);
        data.set_curves_shown(p.curve_shown.len() as i32);
        let (title, text) = logic::curves_empty(
            p.curves.len(),
            p.curve_shown.len(),
            p.curves_reading.is_some() || !p.curves_started,
            p.curves_failed.as_deref(),
        );
        data.set_curve_empty_title(title.into());
        data.set_curve_empty_text(text.into());
    });
    show_header(app);
    show_card(app);
    show_curve_card(app);
    LINES.with(|m| m.notify.reset());
    CURVE_LINES.with(|m| m.notify.reset());
}

/// The page's header: the line under the title and the work going on, for the section on show.
fn show_header(app: &AppWindow) {
    let data = app.global::<PresetsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let reading = p.reading.is_some();
        // The header, for the section on show.
        let hidden = p.curves.iter().filter(|c| c.hidden).count();
        let (summary, busy, done) = match p.section {
            Section::Sounds => (
                logic::summary(p.items.len() + p.pending.len(), p.shown.len(), p.songs, p.files, reading),
                p.reading.map(|(d, t)| logic::sounds_busy(d, t)).unwrap_or_default(),
                p.reading.map_or(0.0, |(d, t)| if t == 0 { -1.0 } else { d as f32 / t as f32 }),
            ),
            Section::Curves => (
                logic::curves_summary(p.curves.len(), p.curve_shown.len(), hidden),
                p.curves_reading.map(|(d, t)| logic::curves_busy(d, t)).unwrap_or_default(),
                p.curves_reading.map_or(0.0, |(d, t)| if t == 0 { -1.0 } else { d as f32 / t as f32 }),
            ),
        };
        data.set_summary(summary.into());
        data.set_busy(busy.into());
        data.set_done(done);
    });
}

/// The picked sound's card, its buttons and its right-click menu (the same lines, with their keys: rule 15).
fn show_card(app: &AppWindow) {
    let data = app.global::<PresetsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let Some(item) = p.picked.and_then(|i| p.items.get(i)) else {
            data.set_panel_shown(false);
            return;
        };
        let item = &*item.as_fixed(&p.fixed_now);
        let c = logic::card(item);
        data.set_panel_shown(true);
        data.set_card(PresetCard {
            over: c.over.into(),
            name: c.name.into(),
            plugin: c.plugin.into(),
            part: c.part.into(),
            colour: c.colour,
            reason: c.reason.into(),
            fixed: c.fixed.into(),
            hint: c.hint.into(),
            path: c.path.into(),
            fixable: c.fixable,
        });
        data.set_facts(ModelRc::new(VecModel::from(
            c.facts.into_iter().map(|(n, v)| PresetFact { name: n.into(), value: v.into() }).collect::<Vec<_>>(),
        )));
        let offers = logic::offers(item, p.exporting, hearing_of(&p, item));
        let key = |id: &str| SharedString::from(p.shortcuts.key_of(id));
        data.set_actions(ModelRc::new(VecModel::from(
            offers
                .iter()
                .map(|o| CardAction {
                    text: o.text.as_str().into(),
                    tip: what_of(o.id).into(),
                    key: key(o.id),
                    id: o.id.into(),
                    enabled: o.enabled,
                    main: o.id == act::AUDITION,
                    danger: false,
                })
                .collect::<Vec<_>>(),
        )));
        let parts = logic::part_offers(item);
        let shown_role = item.role();
        data.set_parts(ModelRc::new(VecModel::from(
            parts
                .iter()
                .map(|(text, id, _)| PartChip {
                    text: text.as_str().into(),
                    id: id.as_str().into(),
                    on: logic::part_of(id).is_some_and(|r| Some(r) == shown_role),
                })
                .collect::<Vec<_>>(),
        )));
        let mut menu: Vec<MenuItem> = offers
            .iter()
            .map(|o| MenuItem { what: o.text.as_str().into(), key: key(o.id), id: o.id.into(), enabled: o.enabled })
            .collect();
        if !item.row.hash.is_empty() {
            menu.push(MenuItem { enabled: true, ..MenuItem::default() });
            for (text, id, on) in parts {
                let what = if on { format!("{text} (set)") } else { format!("It's {}", text.to_lowercase()) };
                let what = if id == act::USE_GUESS { text.clone() } else { what };
                menu.push(MenuItem { what: what.into(), key: key(&id), id: id.into(), enabled: !on });
            }
        }
        data.set_menu(ModelRc::new(VecModel::from(menu)));
        data.set_message(p.message.as_str().into());
    });
}

fn show_curve_card(app: &AppWindow) {
    let data = app.global::<PresetsData>();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(c) = p.curve_picked.as_deref().and_then(|id| p.curves.iter().find(|c| c.id == id)) else {
            data.set_curve_panel_shown(false);
            p.curve_places_of = None;
            return;
        };
        let card = logic::curve_card(c);
        let new_places = p.curve_places_of.as_deref() != Some(c.id.as_str());
        let id = c.id.clone();
        data.set_curve_panel_shown(true);
        data.set_curve_card(CurveCardInfo { name: card.name.into(), shape: card.shape.into(), hidden: card.hidden });
        data.set_curve_facts(ModelRc::new(VecModel::from(
            card.facts.into_iter().map(|(n, v)| PresetFact { name: n.into(), value: v.into() }).collect::<Vec<_>>(),
        )));
        if new_places {
            data.set_curve_places(ModelRc::new(VecModel::from(
                card.places
                    .into_iter()
                    .map(|(t, a)| CurvePlace { title: t.into(), about: a.into() })
                    .collect::<Vec<_>>(),
            )));
        }
        let offers = logic::curve_offers(c, logic::bitwig_curves_folder(&p.bitwig).is_some());
        let key = |id: &str| SharedString::from(p.shortcuts.key_of(id));
        data.set_curve_actions(ModelRc::new(VecModel::from(
            offers
                .iter()
                .map(|o| CardAction {
                    text: o.text.as_str().into(),
                    tip: what_of(o.id).into(),
                    key: key(o.id),
                    id: o.id.into(),
                    enabled: o.enabled,
                    main: false,
                    danger: false,
                })
                .collect::<Vec<_>>(),
        )));
        data.set_curve_menu(ModelRc::new(VecModel::from(
            offers
                .iter()
                .map(|o| MenuItem { what: o.text.as_str().into(), key: key(o.id), id: o.id.into(), enabled: o.enabled })
                .collect::<Vec<_>>(),
        )));
        data.set_curve_message(p.curve_message.as_str().into());
        p.curve_places_of = Some(id);
    });
}

/// Picks a sound (a place in the page's lines), or none.
fn pick(app: &AppWindow, at: Option<usize>) {
    let row = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.picked != at {
            p.message.clear();
        }
        p.picked = at;
        at.and_then(|a| p.shown.iter().position(|&s| s == a))
    });
    if let Some(row) = row {
        let data = app.global::<PresetsData>();
        data.set_show_row(row as i32);
        data.set_show_ask(data.get_show_ask() + 1);
    }
    show_card(app);
    LINES.with(|m| m.notify.reset());
}

fn step(app: &AppWindow, by: i32) {
    let next = PAGE.with(|p| {
        let p = p.borrow();
        let now = p.picked.and_then(|a| p.shown.iter().position(|&s| s == a));
        let row = match now {
            Some(r) => (r as i64 + i64::from(by)).clamp(0, p.shown.len() as i64 - 1),
            None => 0,
        };
        usize::try_from(row).ok().and_then(|r| p.shown.get(r).copied())
    });
    if next.is_some() {
        pick(app, next);
    }
}

fn pick_curve(app: &AppWindow, id: Option<String>) {
    let row = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.curve_picked != id {
            p.curve_message.clear();
        }
        p.curve_picked = id.clone();
        let id = id?;
        p.curve_shown.iter().position(|&i| p.curves[i].id == id)
    });
    let data = app.global::<PresetsData>();
    data.set_renaming(false);
    if let Some(row) = row {
        data.set_curve_show_row(row as i32);
        data.set_curve_show_ask(data.get_curve_show_ask() + 1);
    }
    show_curve_card(app);
    CURVE_LINES.with(|m| m.notify.reset());
}

fn step_curve(app: &AppWindow, by: i32) {
    let next = PAGE.with(|p| {
        let p = p.borrow();
        let now = p.curve_picked.as_deref().and_then(|id| p.curve_shown.iter().position(|&i| p.curves[i].id == id));
        let row = match now {
            Some(r) => (r as i64 + i64::from(by)).clamp(0, p.curve_shown.len() as i64 - 1),
            None => 0,
        };
        usize::try_from(row).ok().and_then(|r| p.curve_shown.get(r)).map(|&i| p.curves[i].id.clone())
    });
    if next.is_some() {
        pick_curve(app, next);
    }
}

/// Runs `f` on the window's thread with the app, from any thread.
fn on_window(f: impl FnOnce(&AppWindow) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = APP.with(|a| a.borrow().as_ref().and_then(Weak::upgrade)) {
            f(&app);
        }
    });
}

// ---------------------------------------------------------------------------------------------------------------------
// Reading the sounds

/// What the reader needs from the window's side.
struct Where {
    library: PathBuf,
    data_dir: PathBuf,
}

fn read_sounds(_app: &AppWindow) {
    let at = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.reading = Some((0, 0));
        Where { library: p.library.clone(), data_dir: p.data_dir.clone() }
    });
    let started = background::spawn("Presets read", move || {
        let result = read_all(&at);
        if let Err(e) = &result {
            log::warning("Presets", e);
        }
        on_window(move |app| {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.reading = None;
                if let Err(e) = &result {
                    p.failed = Some(e.clone());
                }
                take_waiting(&mut p);
                ask_filter(&mut p, true);
            });
            give_finder(app);
            show_all(app);
        });
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start reading the sounds: {e}"));
    }
}

/// Reads every song and preset file again, from the start: what's unchanged comes from the stores, so it's quick.
/// Ctrl+K forgets the old lines until the new ones are all in (it finds a pick by what it is, so one in Recent still
/// opens the same sound).
fn read_again(app: &AppWindow) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        background::free_later(std::mem::take(&mut p.items));
        background::free_later(std::mem::take(&mut p.pending));
        p.shown.clear();
        p.picked = None;
        p.to_pick = None;
        p.songs = 0;
        p.files = 0;
        p.failed = None;
        p.finder_has = false;
        refilter(&mut p);
    });
    crate::finder::add_source(app, "presets", Arc::new(mo_app::finder::Presets(Arc::new(Vec::new()))));
    read_sounds(app);
}

/// Reads the library, every song's newest save and the preset files, handing each batch to the window.
fn read_all(at: &Where) -> Result<(), String> {
    let lib = Library::open_read_only(&at.library).map_err(|e| format!("Couldn't open your library: {e}"))?;
    let saves = lib.saves().map_err(|e| format!("Couldn't read your library: {e}"))?;
    let choices = lib.song_choices().unwrap_or_default();
    let export_root = bitwig::export_left_out(&lib).ok().flatten();
    drop(lib);
    let songs = mo_library::songs::group_songs(&saves, &choices);
    let newest = logic::newest_saves(&songs, export_root.as_deref());
    let bitwig_folders = logic::bitwig_folders(&plib::default_folders(), saves.iter().map(|s| s.path.as_str()));
    // Every song's newest save as it is now, read below or left out: Songs' link reads again only for another (U-2).
    let looked_at: HashMap<String, i64> = songs.iter().map(|s| (s.latest.path.clone(), s.latest.mtime)).collect();
    drop(songs);
    drop(saves);
    let total = newest.len();
    {
        let folders = bitwig_folders.clone();
        on_window(move |app| {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.bitwig = folders;
                background::free_later(std::mem::replace(&mut p.looked_at, looked_at));
                p.reading = Some((0, total));
            });
            show_all(app);
        });
    }
    // Your fixes win over every guess.
    let fixes = match RoleFixes::open(&at.data_dir.join(fixes::FILE_NAME)).and_then(|f| f.all()) {
        Ok(f) => f,
        Err(e) => {
            log::warning("Presets", &format!("Couldn't read your part fixes: {}", e.0));
            HashMap::new()
        }
    };
    let limits = Thresholds::default();
    // What was found in each save last time: only new and changed saves are read from their files.
    let mut store = match SoundStore::open(&at.data_dir.join(sound_store::FILE_NAME), &limits) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warning("Presets", &format!("Couldn't open what the saves held last time, so all are read: {}", e.0));
            None
        }
    };
    let read_now = |paths: &[String]| background::on_pool(|| plib::read_saves(paths, &limits));
    let mut done = 0;
    let mut failed = 0;
    let mut from_files = 0;
    for batch in newest.chunks(BATCH) {
        let paths: Vec<String> = batch.iter().map(|s| s.path.clone()).collect();
        let kept = store.as_mut().map(|st| background::on_pool(|| st.saves(&paths, |p| plib::read_saves(p, &limits))));
        let read = match kept {
            Some(Ok(batch)) => {
                from_files += batch.read;
                if let Some(why) = batch.not_kept {
                    log::warning("Presets", &format!("Couldn't keep what the saves hold for next time: {why}"));
                    store = None;
                }
                batch.saves
            }
            other => {
                if let Some(Err(e)) = other {
                    log::warning("Presets", &format!("Couldn't read what the saves held last time: {}", e.0));
                    store = None;
                }
                from_files += paths.len();
                read_now(&paths)
            }
        };
        let mut items = Vec::new();
        let mut songs = 0;
        for mut s in read {
            if s.error.is_some() {
                failed += 1;
            }
            mo_presets::list::apply_fixes(&mut s.sounds, &fixes);
            let Some(song) = batch.iter().find(|b| b.path == s.path) else { continue };
            songs += usize::from(!s.sounds.is_empty());
            items.extend(logic::song_items(plib::rows(std::slice::from_ref(&s), &[]), song));
        }
        done += batch.len();
        add(items, songs, 0, Some((done, total)));
    }
    if failed > 0 {
        log::info("Presets", &format!("{failed} of {total} saves couldn't be read for their sounds"));
    }
    log::info(
        "Presets",
        &format!("{from_files} of {total} saves read from their files, the rest as they were last time"),
    );
    if let Some(store) = store.as_mut() {
        let all: Vec<String> = newest.iter().map(|s| s.path.clone()).collect();
        if let Err(e) = store.keep_only(&all) {
            log::warning("Presets", &format!("Couldn't forget saves no longer listed: {}", e.0));
        }
    }
    // The preset files, and the Bitwig presets among them that Send to Bitwig builds from.
    let roots = logic::preset_folders(&plib::default_folders(), &bitwig_folders);
    // Read only the preset files that are new or changed since the last run; the rest are as they were kept.
    let started = std::time::Instant::now();
    let (kept, why) = background::on_pool(|| file_store::find_every(&roots, &at.data_dir.join(file_store::FILE_NAME)));
    if let Some(why) = why {
        log::warning("Presets", &format!("Every preset file is read, none kept for next time: {why}"));
    }
    if let Some(why) = &kept.not_kept {
        log::warning("Presets", &format!("Couldn't keep the preset files read for next time: {why}"));
    }
    log::info(
        "Presets",
        &format!(
            "{} of {} preset files read from their files, the rest as they were last time ({:.1} s)",
            kept.read,
            kept.files.len(),
            started.elapsed().as_secs_f64()
        ),
    );
    let (found, templates) = (kept.files, kept.templates);
    // Each file's year from the time the store looked at already: no second look at every file (X2-4).
    let years: HashMap<String, Option<i32>> =
        found.iter().zip(&kept.modified).map(|(f, at)| (f.path.clone(), at.and_then(year_of_time))).collect();
    let mut items: Vec<Item> = plib::rows(&[], &found)
        .into_iter()
        .map(|row| {
            let year = match &row.source {
                Source::File { path } => years.get(path).copied().flatten(),
                Source::Song { .. } => None,
            };
            Item::new(row, String::new(), year)
        })
        .collect();
    // In the list's order, so filtering needn't sort them.
    logic::sort_files(&mut items);
    let files = items.len();
    let templates = Arc::new(templates);
    on_window(move |app| {
        PAGE.with(|p| p.borrow_mut().templates = Some(templates));
        let _ = app;
    });
    add(items, 0, files, None);
    Ok(())
}

fn year_of_time(m: std::time::SystemTime) -> Option<i32> {
    logic::year_of(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// Adds lines read on another thread to the page. While reading goes on the list is shown afresh at most four times
/// a second, so a library of thousands of songs doesn't keep the window busy.
fn add(items: Vec<Item>, songs: usize, files: usize, reading: Option<(usize, usize)>) {
    on_window(move |app| {
        let show = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            // Fixes made since the reading began are laid over these lines too (`fixed_now`): they're never written
            // into them. While a filtering holds the lines, the new ones wait: its answer puts them in (`take_waiting`).
            // Moved in when nothing waits, not copied (H-8).
            if p.pending.is_empty() {
                p.pending = items;
            } else {
                p.pending.extend(items);
            }
            take_waiting(&mut p);
            p.songs += songs;
            p.files += files;
            if reading.is_some() {
                p.reading = reading;
            }
            // At most four times a second while reading: the plugin list and the filtering go over every line.
            let due = due(&p) || reading.is_none();
            if due {
                p.shown_at = Some(Instant::now());
                // Every plugin, counted again with the filtering: the menu scrolls, and only makes the lines in sight.
                ask_filter(&mut p, true);
            }
            due
        });
        if show {
            show_all(app);
        } else {
            // The header's count still moves on.
            show_header(app);
        }
    });
}

// ---------------------------------------------------------------------------------------------------------------------
// What can be done with a sound

/// Space on the page: auditions the picked sound in Bitwig, or stops it (as Audition in Bitwig does). Whether it did.
pub fn toggle_play(app: &AppWindow) -> bool {
    PAGE.with(|p| p.borrow().section) == Section::Sounds && run_sound_action(app, act::AUDITION)
}

/// Runs one of the Sounds section's actions (from its key, its card or its menu). Returns whether it did anything.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    match PAGE.with(|p| p.borrow().section) {
        Section::Sounds => run_sound_action(app, id),
        Section::Curves => run_curve_action(app, id),
    }
}

/// The page's own key lists: the Sounds section's or the Curves section's, as is on show.
pub fn page_actions() -> &'static [mo_app::shortcuts::Action] {
    match PAGE.with(|p| p.borrow().section) {
        Section::Sounds => PRESETS_ACTIONS,
        Section::Curves => CURVES_ACTIONS,
    }
}

fn run_sound_action(app: &AppWindow, id: &str) -> bool {
    if id == act::EXPORT_SHOWN {
        return export_shown(app);
    }
    // Keep goes to the audition playing, whatever line is picked (the strip under the list has it too).
    if id == act::KEEP {
        let playing = PAGE.with(|p| matches!(p.borrow().hearing, Some((_, logic::Hearing::Playing))));
        if playing {
            keep_audition(app);
        }
        return playing;
    }
    let Some(item) = PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|i| p.items.get(i)).map(|i| i.as_fixed(&p.fixed_now).into_owned())
    }) else {
        return false;
    };
    let feed = PAGE.with(|p| p.borrow().feed.clone());
    if let Some(role) = logic::part_of(id) {
        set_fix(app, &item, Some(role));
        return true;
    }
    let enabled = |id: &str| {
        PAGE.with(|p| {
            let p = p.borrow();
            logic::offers(&item, p.exporting, hearing_of(&p, &item)).iter().any(|o| o.id == id && o.enabled)
        })
    };
    match id {
        act::USE_GUESS if item.row.fixed.is_some() => set_fix(app, &item, None),
        act::REVEAL => crate::files::reveal(&feed, item.path(), mo_app::shell::folder_of(item.path())),
        act::SHOW_IN_SONGS if !item.is_file() => show_in_songs(app, item.path()),
        act::COPY_PATH => crate::files::copy_path(&feed, item.path()),
        act::EXPORT if enabled(id) => export_one(app, item, false),
        act::TO_BITWIG if enabled(id) => {
            if item.is_file() {
                send_file(item.path().to_owned());
            } else {
                export_one(app, item, true);
            }
        }
        act::AUDITION if enabled(id) => toggle_audition(app, item),
        _ => return false,
    }
    true
}

/// Sets (or takes away) your part for a sound, in `presets.db`, then on every copy of it in the list.
fn set_fix(app: &AppWindow, item: &Item, role: Option<Role>) {
    let hash = item.row.hash.clone();
    if hash.is_empty() {
        return;
    }
    let (db, feed) = PAGE.with(|p| {
        let p = p.borrow();
        (p.data_dir.join(fixes::FILE_NAME), p.feed.clone())
    });
    // Shown at once; written to the database behind it, and put back if that fails.
    let before = item.row.fixed;
    apply(app, &hash, role);
    let started = background::spawn("Preset part fix", move || {
        let written = RoleFixes::open(&db).and_then(|f| match role {
            Some(r) => f.set(&hash, r),
            None => f.clear(&hash),
        });
        if let Err(e) = written {
            log::warning("Presets", &format!("Couldn't keep the part fix: {}", e.0));
            feed.start("Couldn't keep the part", "").failed(&e.0);
            on_window(move |app| apply(app, &hash, before));
        }
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start keeping the part fix: {e}"));
    }
}

fn apply(app: &AppWindow, hash: &str, role: Option<Role>) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // Laid over the lines, not written into them: they're shared, and one change would copy them all.
        p.fixed_now.insert(hash.to_owned(), role);
        // A part filter may drop it from the list; the card stays on it.
        refilter(&mut p);
    });
    show_all(app);
}

/// What an export needs: the library to read the export folder from, and the templates (found on the export's thread
/// if the page hasn't yet: a click before the preset files were read).
fn export_setup() -> (PathBuf, Option<Arc<Templates>>, Vec<PathBuf>, Feed) {
    PAGE.with(|p| {
        let p = p.borrow();
        (p.library.clone(), p.templates.clone(), p.bitwig.clone(), p.feed.clone())
    })
}

/// Ends an export that can't start (its export folder isn't there, say): its card and the page say why.
fn export_stopped(job: Job, why: String) {
    log::warning("Presets", &format!("Nothing was exported: {why}"));
    job.failed(&why);
    set_exporting(false, Some(why));
}

/// The Bitwig presets to make others from. The page's own, unless they don't have what `enough` asks for: then the
/// preset folders are looked through again (only new and changed files are read), so 'save a preset in Bitwig, then
/// try again' works without a restart (H-2), and the page keeps what was found.
fn templates_now(
    have: Option<Arc<Templates>>,
    bitwig: &[PathBuf],
    enough: impl Fn(&Templates) -> bool,
) -> Arc<Templates> {
    if let Some(t) = have.filter(|t| enough(t)) {
        return t;
    }
    let roots = logic::preset_folders(&plib::default_folders(), bitwig);
    // The same folder the page keeps the store in (`Page::data_dir` is set from it).
    let store = mo_app::paths::data_dir().join(file_store::FILE_NAME);
    let (found, why) = background::on_pool(|| file_store::find(&roots, &store));
    if let Some(why) = why.or(found.not_kept) {
        log::warning("Presets", &format!("The preset files read for Send to Bitwig weren't kept: {why}"));
    }
    let fresh = Arc::new(found.templates);
    let kept = Arc::clone(&fresh);
    on_window(move |_| PAGE.with(|p| p.borrow_mut().templates = Some(kept)));
    fresh
}

fn set_exporting(on: bool, message: Option<String>) {
    on_window(move |app| {
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.exporting = on;
            if let Some(m) = message {
                p.message = m;
            }
        });
        show_all(app);
    });
}

/// Exports one sound (and its Bitwig preset where its plugin has a template); with `send`, sends the Bitwig preset.
fn export_one(app: &AppWindow, item: Item, send: bool) {
    let Source::Song { save, key } = item.row.source.clone() else { return };
    let (library, templates, bitwig_folders, feed) = export_setup();
    PAGE.with(|p| p.borrow_mut().exporting = true);
    show_all(app);
    let job = feed.start(if send { "Making a Bitwig preset" } else { "Exporting a preset" }, &item.row.name);
    let started = background::spawn("Preset export", move || {
        // Checked here, as every export checks it: it looks at the disk.
        let root = match bitwig::export_folder_in(&library) {
            Ok(root) => root,
            Err(why) => return export_stopped(job, why),
        };
        let templates = templates_now(templates, &bitwig_folders, |t| without_template(&[&item], t).is_empty());
        let done = background::on_pool(|| {
            export::export_from_save(
                &save,
                Choice::default(),
                Some(std::slice::from_ref(&key)),
                &root,
                Some(&templates),
            )
        });
        match done {
            Ok(e) => {
                let without = without_template(&[&item], &templates);
                let mut text = logic::export_summary(std::slice::from_ref(&e), &without, &[]);
                let bwpreset =
                    e.files.iter().find(|f| f.to_lowercase().ends_with(".bwpreset")).map(|f| e.folder.join(f));
                // What's shown in its folder after: the file exported; for a send, the Bitwig preset only when it
                // didn't go into Bitwig (to drag in by hand).
                let mut shown = e.files.first().map(|f| e.folder.join(f));
                let mut went = true;
                if send {
                    (text, went) = match &bwpreset {
                        Some(p) => match send_to_bitwig(p) {
                            Ok(words) => (words, true),
                            Err(words) => (words, false),
                        },
                        None => (no_bitwig_preset(&item), false),
                    };
                    shown = if went { None } else { bwpreset.clone().or(shown) };
                }
                if went {
                    job.done(&text);
                } else {
                    job.failed(&text);
                }
                if let Some(path) = shown {
                    let path = path.to_string_lossy().into_owned();
                    crate::files::reveal(&feed, &path, mo_app::shell::folder_of(&path));
                }
                set_exporting(false, Some(text));
            }
            Err(e) => {
                log::warning("Presets", &format!("Export of {save}: {e}"));
                let text = format!("Export failed: {e}");
                job.failed(&text);
                set_exporting(false, Some(text));
            }
        }
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start the export: {e}"));
        PAGE.with(|p| p.borrow_mut().exporting = false);
    }
}

/// Why no Bitwig preset came out of an export of `item`.
fn no_bitwig_preset(item: &Item) -> String {
    match bw::state_format(&item.row.extension) {
        // FL Studio's own plugins' settings only FL Studio reads.
        "" => format!("{} is one of FL Studio's own plugins: Bitwig can't load it.", item.row.plugin),
        format => format!(
            "No Bitwig preset could be made for {}: save a preset of any {format} plugin once in Bitwig (any plugin \
             will do), then try again.",
            item.row.plugin
        ),
    }
}

/// The plugins among `items` that could get a Bitwig preset, but have no template yet.
fn without_template(items: &[&Item], templates: &Templates) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        let format = bw::state_format(&i.row.extension);
        if format.is_empty() {
            continue;
        }
        // Its own template, or another plugin's of the same format to stand in for it.
        let has = bw::key(&i.row.plugin, format).is_some_and(|k| templates.contains_key(&k))
            || templates.values().any(|t| t.format() == format);
        if !has && !out.iter().any(|o| o.eq_ignore_ascii_case(&i.row.plugin)) {
            out.push(i.row.plugin.clone());
        }
    }
    out
}

/// Export these: every sound shown, save by save, into the export folder. Preset files are left (they're files).
fn export_shown(app: &AppWindow) -> bool {
    // A key still reaches here while an export runs (the button is off): it says so rather than anything else (H-11).
    if PAGE.with(|p| p.borrow().exporting) {
        set_message(app, "An export is running: wait for it to finish, then try again.");
        return false;
    }
    // Only counted here: the plan and the plugins with no template are made on the export's thread (H-3).
    let (items, shown, files) = PAGE.with(|p| {
        let p = p.borrow();
        let files = p.shown.iter().filter(|&&i| p.items.get(i).is_some_and(Item::is_file)).count();
        (Arc::clone(&p.items), p.shown.clone(), files)
    });
    if files == shown.len() {
        drop(items);
        if files > 0 {
            set_message(app, "The list shows only preset files, which are files already: nothing to export.");
        }
        return false;
    }
    let (library, templates, bitwig_folders, feed) = export_setup();
    PAGE.with(|p| p.borrow_mut().exporting = true);
    show_all(app);
    let job = feed.start("Exporting presets", &format!("{} sounds", logic::thousands(shown.len() - files)));
    let started = background::spawn("Preset export", move || {
        let root = match bitwig::export_folder_in(&library) {
            Ok(root) => root,
            Err(why) => return export_stopped(job, why),
        };
        // The lines are let go of straight after, so lines read during the export join the list.
        let (plan, without, templates) = {
            let (plan, _) = logic::export_plan(&items, &shown);
            let songs: Vec<&Item> = shown.iter().filter_map(|&i| items.get(i)).filter(|i| !i.is_file()).collect();
            let templates = templates_now(templates, &bitwig_folders, |t| without_template(&songs, t).is_empty());
            (plan, without_template(&songs, &templates), templates)
        };
        drop((items, shown));
        let (mut done, mut failed) = (Vec::new(), Vec::new());
        let total = plan.len();
        for (n, (save, keys)) in plan.iter().enumerate() {
            match background::on_pool(|| {
                export::export_from_save(save, Choice::default(), Some(keys), &root, Some(&templates))
            }) {
                Ok(e) => done.push(e),
                Err(e) => failed.push(format!("{}: {e}", mo_formats::paths::stem(save))),
            }
            job.progress((n + 1) as f32 / total as f32, Some(&format!("{} of {}", n + 1, total)));
        }
        let mut text = logic::export_summary(&done, &without, &failed);
        if files > 0 {
            text += &format!(
                " The {} preset files shown are files already, so they were left as they are.",
                logic::thousands(files)
            );
        }
        job.done(&text);
        let folder = if done.len() == 1 { done[0].folder.clone() } else { root.clone() };
        let folder = folder.to_string_lossy().into_owned();
        crate::files::reveal(&feed, &folder, &folder);
        set_exporting(false, Some(text));
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start the export: {e}"));
        PAGE.with(|p| p.borrow_mut().exporting = false);
    }
    true
}

fn set_message(app: &AppWindow, text: &str) {
    PAGE.with(|p| p.borrow_mut().message = text.to_owned());
    show_card(app);
}

/// A Bitwig preset file sent into Bitwig; when it can't go, it's shown in its folder to drag onto a track.
fn send_file(path: String) {
    // One send or export at a time from this page: the button is off until it's done.
    let feed = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.exporting = true;
        p.feed.clone()
    });
    on_window(show_all);
    let started = background::spawn("Send to Bitwig", move || {
        let text = match send_to_bitwig(Path::new(&path)) {
            Ok(words) => words,
            Err(words) => {
                crate::files::reveal(&feed, &path, mo_app::shell::folder_of(&path));
                words
            }
        };
        set_exporting(false, Some(text));
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start sending to Bitwig: {e}"));
    }
}

/// Sends a Bitwig preset into the project open in Bitwig, through the Bitwig link (`mo-dawlink`): a new track named
/// after the preset, with the preset loaded on it, as if it were dropped there. Waits for Bitwig, so only ever on a
/// thread away from the window. `Ok` says what happened in Bitwig; `Err` says why it didn't go, and the caller shows
/// the file in its folder instead, to drag onto a track.
fn send_to_bitwig(preset: &Path) -> Result<String, String> {
    let name = preset.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // The link's own send: it waits its turn behind sounds and kits sent from the Sounds page, checks the file is
    // still there, and keeps the change to the project open as it began.
    match mo_dawlink::bitwig::send_preset(preset, &name) {
        Ok(sent) => match sent.problem {
            None => Ok(logic::sent_words(&name)),
            Some(problem) => Err(logic::send_failed(&problem)),
        },
        Err(e) => Err(logic::send_failed(&e.to_string())),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Audition in Bitwig

/// The audition playing in Bitwig, once it's in (the window's page knows which line it is).
static AUDITION: std::sync::Mutex<Option<mo_dawlink::bitwig::Auditioning>> = std::sync::Mutex::new(None);
/// The newest Audition, Stop or Keep: an audition that finishes starting after a newer one is taken out again.
static AUDITION_ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn audition_now() -> std::sync::MutexGuard<'static, Option<mo_dawlink::bitwig::Auditioning>> {
    AUDITION.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn audition_asked() -> u64 {
    AUDITION_ASKED.load(Ordering::SeqCst)
}

/// Whether `item` is the line being auditioned, and how far it's got.
fn hearing_of(p: &Page, item: &Item) -> logic::Hearing {
    match &p.hearing {
        Some((id, hearing)) if *id == logic::audition_id(item) => *hearing,
        _ => logic::Hearing::Off,
    }
}

/// Audition in Bitwig, and Stop in Bitwig: stops the audition playing (taking its track out again); unless it was this
/// line's, puts this line's preset on a new track in Bitwig with a part for its kind playing on it.
fn toggle_audition(app: &AppWindow, item: Item) {
    let id = logic::audition_id(&item);
    let was = PAGE.with(|p| p.borrow().hearing.as_ref().map(|(i, _)| i.clone()));
    let before = stop_audition(app);
    if was.as_deref() == Some(id.as_str()) {
        if let (Some(before), Some(was)) = (before, was) {
            stop_in_bitwig(before, Some(was));
        }
        return;
    }
    // One thing plays in Bitwig at a time: a part playing from the MIDI page stops (the 6 Nov review's D-8).
    crate::midi::stop_bitwig_for_another(app);
    let asked = audition_asked();
    let name = item.row.name.clone();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.hearing = Some((id, logic::Hearing::Starting));
        p.hearing_name.clone_from(&name);
        p.message.clear();
    });
    show_all(app);
    let (_, templates, bitwig_folders, _) = export_setup();
    // Its place in Bitwig's line, taken now, behind the stop just asked for on the MIDI page.
    let ticket = mo_dawlink::bitwig::ticket();
    let on_its_way = crate::in_flight::Start::begin();
    // Held while it waits its turn, so the app closing behind a long job still stops it (the 10 Nov review's D-2).
    let before = before.map(|b| HELD.hold(b));
    let started = background::spawn("Audition in Bitwig", move || {
        let _on_its_way = on_its_way;
        let turn = ticket.wait();
        // Not there when the app took it as it closed, and stopped it.
        let before = before.and_then(|id| HELD.release(id));
        // The one playing before goes first, in the same turn, so the two never cross in Bitwig (whether Bitwig was
        // playing, and stopping it, belong to each in turn). If it can't be stopped, this one doesn't start: the one
        // before is put back, so Stop can be pressed again (the review's D-2).
        if let (Some(before), Some(was)) = (before, was.clone())
            && !stop_audition_in(before, Some(was), &turn)
        {
            return;
        }
        let wanted = move || audition_asked() == asked;
        let held = std::cell::Cell::new(None);
        let made = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
            audition(&item, templates, &bitwig_folders, &turn, &wanted, &held)
        }));
        // A track it made is let go of once it has answered (it took the track out again, or says it may be left),
        // but kept for the app to take out as it closes when it stopped part way.
        if let Ok(Err(_)) = made
            && let Some(id) = held.take()
        {
            HELD.release(id);
        }
        let made = made.unwrap_or_else(|_| Err(Some("It stopped before it started playing.".to_owned())));
        let made = match made {
            // Something newer was asked for while it waited: it took back what it put in, and says nothing.
            Err(None) => return,
            Ok(auditioning) => {
                // Checked and kept under the lock a Stop takes, so a Stop either finds it or is seen here.
                let mut slot = audition_now();
                // Let go of only now, under the slot's lock, which the app takes too as it closes (with all that's
                // held), so it's always in one or the other. Gone when the app took it, and took it out, as it closed.
                if let Some(id) = held.take()
                    && HELD.release(id).is_none()
                {
                    return;
                }
                if audition_asked() != asked {
                    drop(slot);
                    // Stopped (or another line asked for) just as it went in: stopped inside its own turn.
                    stop_audition_in(auditioning, Some(logic::audition_id(&item)), &turn);
                    return;
                }
                *slot = Some(auditioning);
                Ok(())
            }
            Err(Some(why)) => {
                log::warning("Presets", &format!("{name} couldn't be auditioned in Bitwig: {why}"));
                Err(why)
            }
        };
        drop(turn);
        on_window(move |app| {
            // A Stop since cleared the card itself.
            if audition_asked() != asked {
                return;
            }
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                match made {
                    // The strip under the list says it plays (the 6 Nov review's D-4): the card's words stay the
                    // picked line's.
                    Ok(()) => {
                        if let Some((_, hearing)) = &mut p.hearing {
                            *hearing = logic::Hearing::Playing;
                        }
                    }
                    Err(why) => {
                        // Named when another line's card is on show.
                        let picked = p.picked.and_then(|i| p.items.get(i)).map(logic::audition_id);
                        let this = p.hearing.as_ref().map(|(id, _)| id.clone());
                        p.message = if picked.is_some() && picked == this { why } else { format!("{name}: {why}") };
                        p.hearing = None;
                    }
                }
            });
            show_all(app);
        });
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start an audition: {e}"));
        PAGE.with(|p| p.borrow_mut().hearing = None);
        show_all(app);
    }
}

/// Puts the line's Bitwig preset on a new track in Bitwig with its part playing. A sound in a song gets its Bitwig
/// preset made first, in Temp ([`audition_preset_of`]), and it goes again once Bitwig has loaded it. Waits for Bitwig,
/// so never on the window's thread. `Err(None)` when it was no longer `wanted` (it put nothing in, or took it back).
fn audition(
    item: &Item,
    templates: Option<Arc<Templates>>,
    bitwig_folders: &[PathBuf],
    turn: &mo_dawlink::bitwig::Turn<'_>,
    wanted: &dyn Fn() -> bool,
    held: &std::cell::Cell<Option<u64>>,
) -> Result<mo_dawlink::bitwig::Auditioning, Option<String>> {
    // Nothing is made for one that's no longer wanted (the review's D-5).
    if !wanted() {
        return Err(None);
    }
    // A song's sound gets its Bitwig preset made in Temp, under a name the sweep knows, and it goes once Bitwig has
    // loaded it: never in the export folder, among the presets you exported (the 6 Nov review's D-fp5, rule 28).
    let (preset, name, made) = match &item.row.source {
        Source::File { .. } => {
            let preset = PathBuf::from(item.path());
            let name = preset.file_stem().map(|n| n.to_string_lossy().into_owned());
            (preset, name, false)
        }
        Source::Song { save, key } => {
            let (preset, name) = audition_preset_of(item, save, key, templates, bitwig_folders)?;
            (preset, Some(name), true)
        }
    };
    // The preset made for it goes however this ends.
    let _made = made.then(|| Gone(preset.clone()));
    let name = name.unwrap_or_else(|| item.row.name.clone());
    let clip = mo_app::preset_audition::clip(&name, item.role());
    let bytes = mo_formats::midi::write::write_clip(&clip)
        .map_err(|e| format!("Its part's MIDI file couldn't be made: {e}."))?
        .bytes;
    let part = std::env::temp_dir().join(mo_app::midi::play_file_name());
    std::fs::write(&part, &bytes).map_err(|e| format!("Its part's MIDI file couldn't be saved in Temp: {e}."))?;
    // Bitwig has read both files once this answers, so the part goes at once (rule 28).
    // Its track is held (under `held`) from the moment it's made until the caller has it in the slot, so the app
    // closing while Bitwig loads a heavy preset still takes it out (the 10 Nov review's D-2).
    let made = |a: &mo_dawlink::bitwig::Auditioning| held.set(Some(HELD.hold(a.clone())));
    let heard =
        mo_dawlink::bitwig::Bitwig::default().audition_preset_telling(&preset, &part, &name, turn, wanted, &made);
    let _ = std::fs::remove_file(&part);
    heard.map_err(|e| (e != mo_dawlink::bitwig::Error::Cancelled).then(|| e.to_string()))
}

/// A file the app made in Temp, removed when this goes.
struct Gone(PathBuf);

impl Drop for Gone {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Makes a song's sound into a Bitwig preset for an audition, written straight to a play file's name in Temp
/// ([`mo_app::midi::play_preset_name`]), which start-up's tidy clears if the app stops before it's removed: no export
/// folder is made there (the 10 Nov review's D-4). Gives the preset and the sound's name for the track.
fn audition_preset_of(
    item: &Item,
    save: &str,
    key: &str,
    templates: Option<Arc<Templates>>,
    bitwig_folders: &[PathBuf],
) -> Result<(PathBuf, String), Option<String>> {
    let to = std::env::temp_dir().join(mo_app::midi::play_preset_name());
    let templates = templates_now(templates, bitwig_folders, |t| without_template(&[item], t).is_empty());
    let made = background::on_pool(|| export::bitwig_preset_from_save(save, key, &templates, &to));
    match made {
        Ok(Some(name)) => Ok((to, name)),
        Ok(None) => Err(Some(no_bitwig_preset(item))),
        Err(e) => Err(Some(format!("Its Bitwig preset couldn't be made: {e}"))),
    }
}

/// Takes an audition's track out of Bitwig on a thread of its own, without the window waiting, in its place in the
/// line taken now, behind what was asked for before it (the review's D-fp2 and D-fp3). When it can't, and `line` names
/// the line it was of, the card says why and the audition is put back so Stop can be pressed again (D-2).
fn stop_in_bitwig(auditioning: mo_dawlink::bitwig::Auditioning, line: Option<String>) {
    let ticket = mo_dawlink::bitwig::ticket();
    // Counted, so the app waits a moment for it as it closes (the review's D-fp4).
    let on_its_way = crate::in_flight::Start::begin();
    // Held while it waits its turn, so the app closing behind a long job still stops it (the 10 Nov review's D-2).
    let held = HELD.hold(auditioning);
    let started = background::spawn("Stop audition", move || {
        let _on_its_way = on_its_way;
        let turn = ticket.wait();
        // Not there when the app took it as it closed, and stopped it.
        let Some(auditioning) = HELD.release(held) else { return };
        stop_audition_in(auditioning, line, &turn);
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start stopping the audition: {e}"));
    }
}

/// Stops an audition inside `turn`. Whether it stopped; when it didn't and `line` names its line, it's put back
/// before the turn ends (see [`put_back`]).
fn stop_audition_in(
    auditioning: mo_dawlink::bitwig::Auditioning,
    line: Option<String>,
    turn: &mo_dawlink::bitwig::Turn<'_>,
) -> bool {
    let Err(e) = mo_dawlink::bitwig::Bitwig::default().stop_audition(&auditioning, turn) else { return true };
    log::warning("Presets", &format!("The audition in Bitwig couldn't be stopped: {e}"));
    // Put back once, so Stop can be pressed again once its project is in front; let go when the stop can never work
    // (Bitwig is gone) or it failed again after being put back, so nothing stays blocked behind it and Esc goes on to
    // its next step (the 10 Nov review's D-1).
    let again = PUT_BACK.failed_before(&auditioning);
    let Some(line) = line else { return true };
    if e.bitwig_gone() || again {
        let_go(&auditioning.track, &e);
        return true;
    }
    !put_back(auditioning, line, e)
}

/// An audition put back once already, or that can never be stopped, or with another in its place: it's let go (what
/// comes next goes ahead), and the card says to take its track out in Bitwig if it's still there.
fn let_go(track: &str, why: &mo_dawlink::bitwig::Error) {
    log::info("Presets", &format!("The app let go of the audition of {track}, which couldn't be stopped"));
    message_later(format!(
        "{track} couldn't be stopped in Bitwig: {why} The app has let it go: if its track is still in your song, take \
         it out in Bitwig."
    ));
}

/// The audition put back after a failed stop, if one is: a second failure lets it go.
static PUT_BACK: crate::in_flight::PutBack<mo_dawlink::bitwig::Auditioning> = crate::in_flight::PutBack::new();

/// Auditions only a thread on its way knows of (a start's track while its preset loads, one a stop took out of the
/// slot and is waiting its turn to stop), for [`finish_up`] to stop as the app closes (the 10 Nov review's D-2).
static HELD: crate::in_flight::Held<mo_dawlink::bitwig::Auditioning> = crate::in_flight::Held::new();

/// An audition that couldn't be stopped: back in its place, unless another plays by now (then it's let go), and a start
/// in Bitwig still on its way behind it (from this page or the MIDI page) is called off, so one thing plays there at a
/// time (the review's D-8). Run inside the stop's turn, so the start behind sees it. The card says why and what to do.
/// Whether it was put back.
fn put_back(auditioning: mo_dawlink::bitwig::Auditioning, line: String, why: mo_dawlink::bitwig::Error) -> bool {
    let track = auditioning.track.clone();
    let put = {
        let mut slot = audition_now();
        if slot.is_none() {
            PUT_BACK.note(auditioning.clone());
            *slot = Some(auditioning);
            AUDITION_ASKED.fetch_add(1, Ordering::SeqCst);
            Some((audition_asked(), crate::midi::call_off_start()))
        } else {
            None
        }
    };
    let Some((at, midi)) = put else {
        let_go(&track, &why);
        return false;
    };
    let words = format!(
        "{track} couldn't be stopped in Bitwig: {why} Bring its project to the front in Bitwig (with changes on), then \
         press Stop in Bitwig again. If it fails again, the app lets it go."
    );
    on_window(move |app| {
        if audition_asked() == at {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.hearing = Some((line, logic::Hearing::Playing));
                p.hearing_name = track;
            });
        }
        if let Some(midi) = midi {
            crate::midi::called_off(app, midi);
        }
        PAGE.with(|p| p.borrow_mut().message = words);
        show_all(app);
    });
    true
}

/// Calls off an audition still on its way, for a part from the MIDI page that couldn't be stopped in Bitwig. Run
/// inside that stop's turn; gives back the ask to hand to [`called_off`] on the window's thread. None when an
/// audition plays already (so none was starting).
pub(crate) fn call_off_start() -> Option<u64> {
    let slot = audition_now();
    if slot.is_some() {
        return None;
    }
    AUDITION_ASKED.fetch_add(1, Ordering::SeqCst);
    Some(audition_asked())
}

/// The card after [`call_off_start`]: an audition called off as it started says why, unless something newer was asked.
pub(crate) fn called_off(app: &AppWindow, at: u64) {
    let starting = PAGE.with(|p| matches!(p.borrow().hearing, Some((_, logic::Hearing::Starting))));
    if audition_asked() != at || !starting {
        return;
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.hearing = None;
        p.message =
            "The audition didn't start: the part from the MIDI page couldn't be stopped in Bitwig (see there).".into();
    });
    show_all(app);
}

/// As the app closes: an audition still on its way sees it's no longer wanted (the MIDI page's finish_up calls this, so
/// the app waits once for both pages).
pub(crate) fn call_off_for_closing() {
    AUDITION_ASKED.fetch_add(1, Ordering::SeqCst);
}

/// Stops the audition playing or starting, whatever line is picked: the strip's Stop in Bitwig, closing the card, and
/// leaving the page.
fn stop_audition_now(app: &AppWindow) {
    let line = PAGE.with(|p| p.borrow().hearing.as_ref().map(|(id, _)| id.clone()));
    if let Some(before) = stop_audition(app) {
        stop_in_bitwig(before, line);
    }
}

/// Esc on the page (`app.escape`, in the search box or anywhere else), the same order on every page, one step a press
/// once nothing plays ([`stop_on_escape`]; the whole-app review of 6 Nov, X3-12): the search is cleared; else the
/// picked line's card closes (a sound's, or a curve's on Curves). Says whether it did either (when not, the key goes
/// on).
pub fn escape(app: &AppWindow) -> bool {
    let data = app.global::<PresetsData>();
    if !data.get_search().is_empty() {
        data.set_search(SharedString::new());
        data.invoke_search_edited(SharedString::new());
        return true;
    }
    let (section, sound, curve) = PAGE.with(|p| {
        let p = p.borrow();
        (p.section, p.picked.is_some(), p.curve_picked.is_some())
    });
    match section {
        Section::Sounds if sound => pick(app, None),
        Section::Curves if curve => pick_curve(app, None),
        _ => return false,
    }
    true
}

/// Esc's first step (`app.escape`): the audition in Bitwig stops. Whether one was playing (only while its page is on
/// show: it stops as the page goes, [`left`]).
pub fn stop_on_escape(app: &AppWindow) -> bool {
    let hearing = PAGE.with(|p| p.borrow().hearing.is_some());
    if hearing {
        stop_audition_now(app);
    }
    hearing
}

/// When another page comes to the front: the audition stops, so nothing plays in Bitwig that this page can't show (the
/// 6 Nov review's X3-1).
pub fn left(app: &AppWindow) {
    if PAGE.with(|p| p.borrow().hearing.is_some()) {
        stop_audition_now(app);
    }
}

/// Stops an audition playing or starting, for Play in Bitwig on the MIDI page taking Bitwig over (the review's
/// D-8): one thing plays there at a time.
pub fn stop_audition_for_another(app: &AppWindow) {
    if PAGE.with(|p| p.borrow().hearing.is_none()) {
        return;
    }
    stop_audition_now(app);
    PAGE.with(|p| p.borrow_mut().message = "The audition stopped: Play in Bitwig on the MIDI page took over.".into());
    show_all(app);
}

/// The audition playing, taken (none when none is), and a newer ask counted under the same lock: one still starting
/// sees it and takes its track out as soon as it's in.
fn take_audition() -> Option<mo_dawlink::bitwig::Auditioning> {
    let mut slot = audition_now();
    AUDITION_ASKED.fetch_add(1, Ordering::SeqCst);
    slot.take()
}

/// Stops the audition playing or starting as far as the card goes, at once, and gives back the one playing for the
/// caller to take out of Bitwig.
fn stop_audition(app: &AppWindow) -> Option<mo_dawlink::bitwig::Auditioning> {
    let taken = take_audition();
    let had = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let had = p.hearing.take().is_some();
        if had {
            p.message.clear();
        }
        had
    });
    if had {
        show_all(app);
    }
    taken
}

/// Keep in Bitwig: the audition stops playing, and its track (the preset, and the part's clip) stays in the project.
fn keep_audition(app: &AppWindow) {
    let Some(auditioning) = take_audition() else { return };
    let track = auditioning.track.clone();
    let words = format!("Kept: {track} stays in Bitwig, on its own track with the part.");
    let ticket = mo_dawlink::bitwig::ticket();
    let on_its_way = crate::in_flight::Start::begin();
    let started = background::spawn("Keep audition", move || {
        let _on_its_way = on_its_way;
        let kept = mo_dawlink::bitwig::Bitwig::default().keep_audition(auditioning, &ticket.wait());
        if let Err(e) = kept {
            log::warning("Presets", &format!("The audition in Bitwig couldn't be stopped as it was kept: {e}"));
            message_later(format!("{track} stays in Bitwig, but it couldn't be stopped there: {e}"));
        }
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start keeping the audition: {e}"));
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.hearing = None;
        p.message = words;
    });
    show_all(app);
}

/// Shows words on the card from a thread away from the window.
fn message_later(text: String) {
    on_window(move |app| {
        PAGE.with(|p| p.borrow_mut().message = text);
        show_all(app);
    });
}

/// In the crash hook, when the crash ends the app: the audition's track comes out of Bitwig, a second at most. Never
/// waits for the lock: the thread that crashed may hold it.
pub fn after_crash() {
    // A start still on its way takes its track out, as when the app closes.
    call_off_for_closing();
    let mut slot = match AUDITION.try_lock() {
        Ok(slot) => slot,
        Err(std::sync::TryLockError::Poisoned(slot)) => slot.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    // With what a thread still on its way holds (the 10 Nov review's D-2).
    let to = crate::in_flight::closing_link(Duration::from_millis(500));
    for auditioning in slot.take().into_iter().chain(HELD.try_take_all()) {
        if to.stop_audition_as_app_closes(&auditioning).is_err() {
            break;
        }
    }
}

/// When the app closes: takes an audition's track out of Bitwig, so it doesn't loop on. Waits a moment at most.
pub fn finish_up() {
    // A start still on its way sees it's no longer wanted and takes its track out inside its own turn: waited for a
    // moment (the review's D-fp4).
    let first = take_audition();
    crate::in_flight::wait_for_all(Duration::from_secs(3));
    // With those a thread still on its way holds, behind a job longer than the wait (the 10 Nov review's D-2): taken
    // under the slot's lock, as a start lets go of its track only under it once it's in the slot.
    let all: Vec<_> = {
        let mut slot = audition_now();
        first.or_else(|| slot.take()).into_iter().chain(HELD.take_all()).collect()
    };
    let to = crate::in_flight::closing_link(Duration::from_secs(3));
    for auditioning in all {
        // Without waiting its turn: what would go first is closing too, and the extension still makes the stop once
        // the app has gone (3.21).
        if let Err(e) = to.stop_audition_as_app_closes(&auditioning) {
            log::warning("Presets", &format!("The audition in Bitwig couldn't be stopped as the app closed: {e}"));
            // Bitwig gone or not answering: the rest would only make the app slower to close.
            if e.bitwig_gone() || e == mo_dawlink::bitwig::Error::TimedOut {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The curves

fn read_curves(_app: &AppWindow) {
    let at = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.curves_reading = Some((0, 0));
        Where { library: p.library.clone(), data_dir: p.data_dir.clone() }
    });
    let started = background::spawn("Curves read", move || {
        let result = scan_curves(&at);
        on_window(move |app| {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.curves_reading = None;
                match result {
                    Ok(list) => set_curves(&mut p, list),
                    Err(e) => {
                        log::warning("Curves", &e);
                        p.curves_failed = Some(e);
                    }
                }
            });
            show_all(app);
        });
    });
    if let Err(e) = started {
        log::error("Curves", &format!("Couldn't start reading the curves: {e}"));
    }
}

fn set_curves(p: &mut Page, list: Vec<Curve>) {
    // Worked out once here, then each key typed in the search makes no copies; the old list goes on its own thread.
    p.curve_keys = logic::CurveKey::all(&list);
    background::free_later(std::mem::replace(&mut p.curves, list));
    p.curve_places_of = None;
    p.curves_failed = None;
    (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_keys, &p.curve_filters);
    if let Some(id) = &p.curve_picked
        && !p.curves.iter().any(|c| &c.id == id)
    {
        p.curve_picked = None;
    }
}

/// Shows what `curves.db` holds at once, then reads the saves that changed and gives the list as it is now.
fn scan_curves(at: &Where) -> Result<Vec<Curve>, String> {
    let mut db = CurvesDb::open(&at.data_dir.join(curves::FILE_NAME))
        .map_err(|e| format!("Couldn't open the curves: {}", e.0))?;
    let first = db.list().map_err(|e| format!("Couldn't read the curves: {}", e.0))?;
    on_window(move |app| {
        PAGE.with(|p| set_curves(&mut p.borrow_mut(), first));
        show_all(app);
    });
    let lib = Library::open_read_only(&at.library).map_err(|e| format!("Couldn't open your library: {e}"))?;
    let saves = lib.saves().map_err(|e| format!("Couldn't read your library: {e}"))?;
    let export_root = bitwig::export_left_out(&lib).ok().flatten();
    drop(lib);
    let mut paths = logic::curve_saves(&saves, export_root.as_deref());
    let bitwig_folders = logic::bitwig_folders(&plib::default_folders(), saves.iter().map(|s| s.path.as_str()));
    drop(saves);
    let library_roots: Vec<PathBuf> = bitwig_folders.iter().map(|b| b.join("Library")).collect();
    paths.extend(background::on_pool(|| logic::curve_files(&library_roots)));
    {
        let folders = bitwig_folders.clone();
        on_window(move |_| PAGE.with(|p| p.borrow_mut().bitwig = folders));
    }
    let (changed, gone) = db.out_of_date(&paths).map_err(|e| format!("Couldn't read the curves: {}", e.0))?;
    if changed.is_empty() && !gone {
        return db.list().map_err(|e| format!("Couldn't read the curves: {}", e.0));
    }
    // How far it's got, on the header, while the saves are read.
    let progress = Arc::new(curves::Progress::default());
    let finished = Arc::new(AtomicBool::new(false));
    {
        let (progress, finished) = (progress.clone(), finished.clone());
        let _ = background::spawn("Curves progress", move || {
            while !finished.load(Ordering::Relaxed) {
                let (d, t) = (progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
                on_window(move |app| {
                    PAGE.with(|p| p.borrow_mut().curves_reading = Some((d, t)));
                    show_header(app);
                });
                std::thread::sleep(Duration::from_millis(400));
            }
        });
    }
    let scanned = background::on_pool(|| db.scan(&paths, &progress));
    finished.store(true, Ordering::Relaxed);
    match scanned {
        Ok(n) => log::info("Curves", &format!("Read {n} saves for their curves")),
        Err(e) => log::warning("Curves", &format!("Couldn't read every save for its curves: {}", e.0)),
    }
    db.list().map_err(|e| format!("Couldn't read the curves: {}", e.0))
}

fn run_curve_action(app: &AppWindow, id: &str) -> bool {
    let (curve, data_dir, bitwig_folders, feed) = PAGE.with(|p| {
        let p = p.borrow();
        let c = p.curve_picked.as_deref().and_then(|id| p.curves.iter().find(|c| c.id == id)).cloned();
        (c, p.data_dir.clone(), p.bitwig.clone(), p.feed.clone())
    });
    let Some(curve) = curve else { return false };
    match id {
        curve_act::RENAME => {
            let data = app.global::<PresetsData>();
            data.set_rename_text(curve.name.as_str().into());
            data.set_renaming(true);
        }
        curve_act::HIDE => {
            let hidden = !curve.hidden;
            curve_db_change(curve.id.clone(), move |db, id| db.set_hidden(id, hidden));
        }
        curve_act::OPEN_SONG => {
            let Some(s) = curve.sources.iter().find(|s| s.kind != curves::SourceKind::BitwigCurve) else {
                return false;
            };
            crate::files::reveal(&feed, &s.path, mo_app::shell::folder_of(&s.path));
        }
        curve_act::SHOW_IN_SONGS => {
            let Some(s) = curve.sources.iter().find(|s| s.kind != curves::SourceKind::BitwigCurve) else {
                return false;
            };
            show_in_songs(app, &s.path);
        }
        curve_act::REVEAL => {
            let folder = data_dir.join(curves::FOLDER);
            let started = background::spawn("Curve file", move || match curves::file(&curve, &folder) {
                Ok(path) => {
                    let path = path.to_string_lossy().into_owned();
                    crate::files::reveal(&feed, &path, mo_app::shell::folder_of(&path));
                    curve_message(
                        "The curve file is in the app's Curves folder: drag it onto an automation lane.".into(),
                    );
                }
                Err(e) => {
                    feed.start("Couldn't make the curve file", "").failed(&e);
                    curve_message(e);
                }
            });
            if let Err(e) = started {
                log::error("Curves", &format!("Couldn't start making the curve file: {e}"));
            }
        }
        curve_act::TO_BITWIG => {
            let Some(folder) = logic::bitwig_curves_folder(&bitwig_folders) else { return false };
            let started = background::spawn("Curve to Bitwig", move || match curves::save_to_bitwig(&curve, &folder) {
                Ok(path) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let text = format!("Saved {name} in Bitwig's curves. It shows in Bitwig's browser under Curves.");
                    feed.start("Saved a curve to Bitwig", "").done(&name);
                    curve_message(text);
                }
                Err(e) => {
                    feed.start("Couldn't save the curve to Bitwig", "").failed(&e);
                    curve_message(e);
                }
            });
            if let Err(e) = started {
                log::error("Curves", &format!("Couldn't start saving the curve: {e}"));
            }
        }
        _ => return false,
    }
    true
}

/// Drags the picked curve's file (in the app's Curves folder, made on a thread of its own as the drag gets going) out,
/// onto an automation lane in Bitwig (X3-15).
fn carry_curve(app: &AppWindow) {
    let Some((curve, folder)) = PAGE.with(|p| {
        let p = p.borrow();
        let c = p.curve_picked.as_deref().and_then(|id| p.curves.iter().find(|c| c.id == id)).cloned()?;
        Some((c, p.data_dir.join(curves::FOLDER)))
    }) else {
        return;
    };
    let under_way = crate::drag::start(
        app,
        crate::drag::Drag {
            make: Box::new(move || {
                let path = curves::file(&curve, &folder)?;
                let card = mo_app::drag_out::Card {
                    title: curve.name.clone(),
                    detail: ["Curve".to_owned(), logic::beats_words(curve.length_beats)]
                        .into_iter()
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · "),
                    ..Default::default()
                };
                Ok(mo_app::drag_out::Payload {
                    files: vec![path],
                    cards: vec![card],
                    caption: "1 curve".into(),
                    numbered: false,
                })
            }),
            done: Box::new(|app, outcome| {
                app.global::<PresetsData>().set_curve_lifted(false);
                if outcome == crate::drag::Outcome::Landed {
                    crate::drag::glow(app, |app, on| app.global::<PresetsData>().set_curve_landed(on));
                }
            }),
        },
    );
    if under_way {
        app.global::<PresetsData>().set_curve_lifted(true);
    }
}

/// Opens the Songs page on the song a save belongs to (X3-10, as Plugins' Show in Songs does).
fn show_in_songs(app: &AppWindow, save: &str) {
    crate::go_to(app, Place::Songs.index() as i32);
    if !crate::songs::show_save_path(app, save) {
        log::info("Presets", "The song isn't on the Songs page yet (its list is still being read)");
    }
}

fn curve_message(text: String) {
    on_window(move |app| {
        PAGE.with(|p| p.borrow_mut().curve_message = text);
        show_curve_card(app);
    });
}

fn rename_curve(_app: &AppWindow, name: String) {
    let Some(id) = PAGE.with(|p| p.borrow().curve_picked.clone()) else { return };
    curve_db_change(id, move |db, id| db.rename(id, &name));
}

/// Changes one curve in `curves.db` (your name, hidden), then puts it in the list as it now is.
fn curve_db_change(
    id: String,
    change: impl FnOnce(&CurvesDb, &str) -> Result<Option<Curve>, curves::CurvesError> + Send + 'static,
) {
    let (db, feed) = PAGE.with(|p| {
        let p = p.borrow();
        (p.data_dir.join(curves::FILE_NAME), p.feed.clone())
    });
    let started = background::spawn("Curve change", move || {
        let changed = CurvesDb::open(&db).and_then(|db| change(&db, &id));
        match changed {
            Ok(Some(c)) => on_window(move |app| {
                PAGE.with(|p| {
                    let mut p = p.borrow_mut();
                    if let Some(at) = p.curves.iter().position(|o| o.id == c.id) {
                        p.curve_keys[at] = logic::CurveKey::of(&c);
                        p.curve_places_of = None;
                        p.curves[at] = c;
                    }
                    (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_keys, &p.curve_filters);
                });
                show_all(app);
            }),
            Ok(None) => {}
            Err(e) => {
                log::warning("Curves", &e.0);
                feed.start("Couldn't change the curve", "").failed(&e.0);
            }
        }
    });
    if let Err(e) = started {
        log::error("Curves", &format!("Couldn't start changing the curve: {e}"));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// For the screenshots

/// Shows made-up sounds and curves, with Ctrl+K opening them, for mo-screens (no library, nothing read).
pub fn show_sample(app: &AppWindow) {
    wire(app);
    finder_opens();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.started = true;
        p.curves_started = true;
        p.filter_here = true;
        p.items = Arc::new(sample_items());
        p.songs = 3;
        p.files = p.items.iter().filter(|i| i.is_file()).count();
        p.plugins = logic::plugins(&p.items);
        p.curves = sample_curves();
        p.curve_keys = logic::CurveKey::all(&p.curves);
        p.bitwig = vec![PathBuf::from(r"K:\Bitwig Studio")];
        refilter(&mut p);
        (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_keys, &p.curve_filters);
    });
    show_all(app);
}

/// Hands the sample's lines to Ctrl+K, as the page does once its lines are read (screenshots).
pub fn sample_in_finder(app: &AppWindow) {
    give_finder(app);
}

/// Shows the picked line as auditioned in Bitwig, playing (screenshots: nothing reaches Bitwig); `false` puts it back.
pub fn show_audition_sample(app: &AppWindow, on: bool) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let id = p.picked.and_then(|i| p.items.get(i)).map(logic::audition_id);
        p.hearing = if on { id.map(|id| (id, logic::Hearing::Playing)) } else { None };
        p.hearing_name = if on { "Atmos".into() } else { String::new() };
    });
    show_all(app);
}

/// Picks the line at `row` of the sounds or the curve at `row` of the curves (screenshots).
pub fn pick_row(app: &AppWindow, row: usize) {
    match PAGE.with(|p| p.borrow().section) {
        Section::Sounds => {
            let at = PAGE.with(|p| p.borrow().shown.get(row).copied());
            pick(app, at);
        }
        Section::Curves => {
            let id = PAGE.with(|p| {
                let p = p.borrow();
                p.curve_shown.get(row).map(|&i| p.curves[i].id.clone())
            });
            pick_curve(app, id);
        }
    }
}

/// Shows a section (screenshots).
pub fn show_section(app: &AppWindow, s: Section) {
    pick_section(app, s);
}

fn sample_items() -> Vec<Item> {
    use mo_formats::PluginKind;
    use mo_presets::library::PresetRow;
    use mo_presets::role::{Basis, Guess};
    let guess = |role: Option<Role>, basis: Basis, reason: &str, weak: bool| Guess {
        role,
        basis,
        reason: reason.into(),
        octave_checked: true,
        weak,
    };
    let song =
        |name: &str, plugin: &str, place: &str, kind: PluginKind, native: bool, ext: &str, g: Guess, save: &str| {
            PresetRow {
                source: Source::Song { save: save.into(), key: format!("{place}|{plugin}").to_lowercase() },
                name: name.into(),
                plugin: plugin.into(),
                kind,
                native,
                place: place.into(),
                extension: ext.into(),
                guess: g,
                plays_nothing: false,
                hash: format!("{save}{name}{plugin}"),
                fixed: None,
            }
        };
    let d = r"I:\Fl Studio Data\FL Studio\Projects\Didums\Didums_77.flp";
    let s = r"I:\Fl Studio Data\FL Studio\Projects\Sly\sly_17.flp";
    let b = r"K:\Bitwig Studio\Projects\Night Bus\Night Bus.bwproject";
    let synth = PluginKind::Instrument;
    let fx = PluginKind::Effect;
    let low = "Plays low (around C2), one note at a time";
    let rows = vec![
        (
            song("Sub", "Serum", "Sub", synth, false, ".fxp", guess(Some(Role::Bass), Basis::Notes, low, false), d),
            "Didums",
            2026,
        ),
        (
            song(
                "Hook",
                "Serum",
                "Hook",
                synth,
                false,
                ".fxp",
                guess(Some(Role::Lead), Basis::Notes, "One note at a time, high (around C5), moving in steps", false),
                d,
            ),
            "Didums",
            2026,
        ),
        (
            song(
                "Keys",
                "FL Keys",
                "Keys",
                synth,
                true,
                ".flstate",
                guess(Some(Role::Chords), Basis::Notes, "Three or more notes together most of the time", false),
                d,
            ),
            "Didums",
            2026,
        ),
        (
            song(
                "Atmos",
                "Vital",
                "Atmos",
                synth,
                false,
                ".vstpreset",
                guess(Some(Role::Pad), Basis::Name, "Its name says pad (\"atmos\"); its notes are long", true),
                d,
            ),
            "Didums",
            2026,
        ),
        (
            song(
                "Fruity Limiter",
                "Fruity Limiter",
                "Master",
                fx,
                true,
                ".flstate",
                guess(None, Basis::Track, "A master effect plays no part of its own", false),
                d,
            ),
            "Didums",
            2026,
        ),
        (
            song(
                "Arp",
                "Sylenth1",
                "Arp",
                synth,
                false,
                ".fxp",
                guess(Some(Role::Arp), Basis::Notes, "Even steps through the notes of a chord", false),
                s,
            ),
            "Sly",
            2025,
        ),
        (
            song(
                "Drums",
                "FPC",
                "Drums",
                synth,
                true,
                ".flstate",
                guess(Some(Role::Drums), Basis::Notes, "A few fixed pitches on the grid, played together", false),
                s,
            ),
            "Sly",
            2025,
        ),
        (
            song(
                "Valhalla Room",
                "ValhallaRoom",
                "Insert 3 · Vocals",
                fx,
                false,
                ".fxp",
                guess(Some(Role::Lead), Basis::Track, "On the insert the Hook channel feeds", false),
                s,
            ),
            "Sly",
            2025,
        ),
        (
            song(
                "Reese",
                "Phase Plant",
                "Bass",
                synth,
                false,
                ".vstpreset",
                guess(Some(Role::Bass), Basis::Notes, low, false),
                b,
            ),
            "Night Bus",
            2024,
        ),
        (
            song(
                "Polysynth",
                "Polysynth",
                "Stabs",
                synth,
                true,
                ".bwpreset",
                guess(Some(Role::Stabs), Basis::Notes, "Short chords on the off-beats", false),
                b,
            ),
            "Night Bus",
            2024,
        ),
    ];
    let mut out: Vec<Item> = rows.into_iter().map(|(r, song, y)| Item::new(r, song.into(), Some(y))).collect();
    // One you fixed.
    if let Some(i) = out.get_mut(3) {
        i.row.fixed = Some(Role::Pad);
    }
    let file = |name: &str, plugin: &str, folder: &str, ext: &str, role: Option<Role>, y: i32| {
        let path = format!(r"C:\Users\silki\Documents\VST3 Presets\{folder}\{name}{ext}");
        Item::new(
            PresetRow {
                source: Source::File { path },
                name: name.into(),
                plugin: plugin.into(),
                kind: synth,
                native: false,
                place: folder.into(),
                extension: ext.into(),
                guess: match role {
                    Some(r) => guess(
                        Some(r),
                        Basis::Name,
                        &format!("A preset file: its name says {}", r.label().to_lowercase()),
                        true,
                    ),
                    None => Guess::none("A preset file: it never played in a song"),
                },
                plays_nothing: false,
                hash: String::new(),
                fixed: None,
            },
            String::new(),
            Some(y),
        )
    };
    out.push(file("Warm Pad", "Vital", "Vital", ".vstpreset", Some(Role::Pad), 2023));
    out.push(file("Growl 3", "Serum", "Serum Presets", ".fxp", None, 2022));
    // More plugins than fit as chips, so the Plugin filter is a menu that scrolls, as it is with a real library.
    out.push(file("Glass Keys", "Pigments", "Pigments", ".vstpreset", Some(Role::Chords), 2025));
    out.push(file("Dusty Lead", "Diva", "Diva", ".fxp", Some(Role::Lead), 2021));
    out.push(file("Wide Choir", "Omnisphere", "Omnisphere", ".fxb", Some(Role::Pad), 2020));
    out.push(file("Space Arp", "Massive X", "Massive X", ".vstpreset", Some(Role::Arp), 2025));
    out
}

fn sample_curves() -> Vec<Curve> {
    use mo_formats::bitwig::curve::CurvePoint;
    use mo_presets::curves::{CurveSource, SourceKind};
    let point = |x: f64, y: f64, bend: f64| CurvePoint { x, y, bend };
    let source = |kind, song: &str, device: &str, setting: &str, at: f64, modified: i64| CurveSource {
        kind,
        path: format!(r"I:\Projects\{song}.flp"),
        song: song.into(),
        name: String::new(),
        device: device.into(),
        setting: setting.into(),
        at,
        modified,
    };
    let curve = |name: &str, points: Vec<CurvePoint>, beats: f64, sources: Vec<CurveSource>| Curve {
        id: format!("{name:0>16}"),
        name: name.into(),
        points,
        length_beats: beats,
        hidden: false,
        sources,
    };
    vec![
        curve(
            "Didums_77 - Serum - Cutoff",
            vec![point(0.0, 0.1, 0.6), point(0.75, 0.95, 0.0), point(1.0, 0.2, 0.0)],
            16.0,
            vec![
                source(SourceKind::FlKnobMoves, "Didums_77", "Serum", "Cutoff", 64.0, 30),
                source(SourceKind::FlKnobMoves, "Didums_70", "Serum", "Cutoff", 64.0, 20),
            ],
        ),
        curve(
            "Riser",
            vec![point(0.0, 0.0, 0.8), point(1.0, 1.0, 0.0)],
            32.0,
            vec![source(SourceKind::BitwigClip, "Night Bus", "Filter+", "Frequency", 96.0, 25)],
        ),
        curve(
            "Wobble",
            vec![
                point(0.0, 0.5, 0.0),
                point(0.25, 1.0, -0.5),
                point(0.5, 0.5, 0.0),
                point(0.75, 0.0, 0.5),
                point(1.0, 0.5, 0.0),
            ],
            4.0,
            vec![source(SourceKind::BitwigCurve, "Wobble", "", "", 0.0, 10)],
        ),
        curve(
            "sly_17 - Sylenth1 - Mix",
            vec![point(0.0, 0.8, 0.0), point(0.5, 0.8, 0.0), point(0.5, 0.3, 0.0), point(1.0, 0.3, 0.0)],
            8.0,
            vec![source(SourceKind::FlKnobMoves, "sly_17", "Sylenth1", "Mix A/B", 32.0, 15)],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_read_while_a_filtering_holds_the_list_wait_and_are_never_copied_in() {
        let mut p = Page { items: Arc::new(sample_items()), ..Page::default() };
        let first = p.items.len();
        // A filtering holds the list.
        let held = Arc::clone(&p.items);
        p.pending.extend(sample_items());
        assert!(!take_waiting(&mut p), "the held list isn't copied to add lines");
        assert_eq!(p.items.len(), first);
        assert!(Arc::ptr_eq(&p.items, &held));
        // Once it lets go, they go in, in the order read, into the very same list.
        drop(held);
        let at = Arc::as_ptr(&p.items);
        assert!(take_waiting(&mut p));
        assert_eq!(p.items.len(), first * 2);
        assert!(p.pending.is_empty());
        assert_eq!(Arc::as_ptr(&p.items), at);
        assert_eq!(p.items[first..], sample_items()[..]);
    }

    #[test]
    fn a_part_fix_is_laid_over_the_shared_lines_not_written_into_them() {
        let items = Arc::new(sample_items());
        let shared = Arc::clone(&items);
        let at = items.iter().position(|i| !i.row.hash.is_empty() && i.row.fixed.is_none()).unwrap();
        let hash = items[at].row.hash.clone();
        let mut fixes = logic::Fixes::new();
        fixes.insert(hash, Some(Role::Pad));
        // The lines aren't touched (nor copied); the line, the card and the filter all see the fix.
        assert_eq!(items[at].row.fixed, None);
        assert!(Arc::ptr_eq(&items, &shared));
        let shown = items[at].as_fixed(&fixes);
        assert_eq!(shown.row.fixed, Some(Role::Pad));
        assert!(logic::line(&shown).fixed);
        let pads = Filters { parts: vec![logic::PartPick::Role(Role::Pad)], ..Filters::default() };
        assert!(logic::filtered_with(&items, &pads, &fixes).shown.contains(&at));
        // A fix taken away goes back to the guess.
        let mut taken = logic::Fixes::new();
        taken.insert(items[at].row.hash.clone(), None);
        assert!(matches!(items[at].as_fixed(&taken), std::borrow::Cow::Borrowed(_)));
    }
}
