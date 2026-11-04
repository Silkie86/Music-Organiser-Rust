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

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::background;
use mo_app::bitwig_convert as bitwig;
use mo_app::log;
use mo_app::presets::{self as logic, CurveFilters, Filters, Item, Section, act, curve_act};
use mo_app::shortcuts::{CURVES_ACTIONS, PRESETS_ACTIONS, Shortcuts, what_of};
use mo_app::year_strip;
use mo_library::Library;
use mo_presets::bitwig::{self as bw, Templates};
use mo_presets::curves::{self, Curve, CurvesDb};
use mo_presets::export;
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
    items: Vec<Item>,
    shown: Vec<usize>,
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
    exporting: bool,
    message: String,
    /// The Curves section.
    curves: Vec<Curve>,
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
    /// Where things are: the library, the app's data folder, the export folder, Bitwig's folders.
    library: PathBuf,
    data_dir: PathBuf,
    export_root: Option<PathBuf>,
    bitwig: Vec<PathBuf>,
    /// The Bitwig presets that Send to Bitwig builds from, once found.
    templates: Option<Arc<Templates>>,
    feed: Feed,
    shortcuts: Shortcuts,
    /// The last time the list was shown afresh while sounds are still coming in.
    shown_at: Option<Instant>,
    /// Part fixes made since the sounds started being read, by fingerprint (None: the fix was taken away), so the
    /// sounds read after one was made get it too.
    fixed_now: HashMap<String, Option<Role>>,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::default();
    static LINES: Rc<Lines> = Rc::new(Lines::default());
    static CURVE_LINES: Rc<CurveLines> = Rc::new(CurveLines::default());
    static FILTER_MODELS: ChipModels = ChipModels::default();
    static APP: RefCell<Option<Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// The filters' chips and menus on screen, kept from one showing to the next.
#[derive(Default)]
struct ChipModels {
    part: FilterModels,
    plugin: FilterModels,
    maker: FilterModels,
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
            let l = logic::line(p.items.get(at)?);
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
        (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_filters);
    });
    wire(app);
}

/// A key was changed in Settings: the buttons, card and menu show the new keys at once.
pub fn show_keys(app: &AppWindow, shortcuts: &Shortcuts) {
    PAGE.with(|p| p.borrow_mut().shortcuts = shortcuts.clone());
    let key = shortcuts.key_of(act::EXPORT_SHOWN).to_owned();
    app.global::<PresetsData>()
        .set_export_shown_key(ActionKey { what: what_of(act::EXPORT_SHOWN).into(), key: key.into() });
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
    let start = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        !std::mem::replace(&mut p.started, true)
    });
    if start {
        read_sounds(app);
    }
    show_all(app);
}

/// Opens the page on one song's sounds (a song's Presets tab), by its save's path.
pub fn show_song(app: &AppWindow, save: &str) {
    PAGE.with(|p| p.borrow_mut().section = Section::Sounds);
    change(app, |f| *f = Filters { song: Some(save.to_owned()), unused: f.unused, ..Filters::default() });
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
        (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_filters);
    });
    show_all(app);
}

/// Works out what's shown from the filters. Quick (each line's search words are made once), so it runs here.
fn refilter(p: &mut Page) {
    p.shown = logic::shown(&p.items, &p.filters);
    p.years = logic::year_counts(&p.items, &p.filters);
    p.chips = logic::filter_chips(&logic::counts(&p.items, &p.filters), &p.plugins, &p.filters);
}

/// Puts everything the page knows on screen.
fn show_all(app: &AppWindow) {
    let data = app.global::<PresetsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_section(p.section.index() as i32);
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
            data.set_maker_filter(m.maker.view(&p.chips.maker));
            data.set_type_filter(m.kind.view(&p.chips.kind));
            data.set_from_filter(m.from.view(&p.chips.from));
            data.set_curve_found_in(m.found_in.view(&p.curve_found_in));
        });
        data.set_song(
            f.song
                .as_ref()
                .map(|s| p.items.iter().find(|i| i.path() == s).map_or_else(|| s.clone(), |i| i.song.clone()))
                .unwrap_or_default()
                .into(),
        );
        data.set_any_filter(f.any());
        data.set_unused(f.unused);
        data.set_shown(p.shown.len() as i32);
        let reading = p.reading.is_some();
        let (title, text) = logic::empty_words(p.items.len(), p.shown.len(), reading, f.any(), p.failed.as_deref());
        data.set_empty_title(title.into());
        data.set_empty_text(text.into());
        data.set_format_note(logic::format_note(&p.items, &p.shown).into());
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
                logic::summary(p.items.len(), p.shown.len(), p.songs, p.files, reading),
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
        let offers = logic::offers(item, p.exporting);
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
        let p = p.borrow();
        let Some(c) = p.curve_picked.as_deref().and_then(|id| p.curves.iter().find(|c| c.id == id)) else {
            data.set_curve_panel_shown(false);
            return;
        };
        let card = logic::curve_card(c);
        data.set_curve_panel_shown(true);
        data.set_curve_card(CurveCardInfo { name: card.name.into(), shape: card.shape.into(), hidden: card.hidden });
        data.set_curve_facts(ModelRc::new(VecModel::from(
            card.facts.into_iter().map(|(n, v)| PresetFact { name: n.into(), value: v.into() }).collect::<Vec<_>>(),
        )));
        data.set_curve_places(ModelRc::new(VecModel::from(
            card.places.into_iter().map(|(t, a)| CurvePlace { title: t.into(), about: a.into() }).collect::<Vec<_>>(),
        )));
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
                refilter(&mut p);
            });
            show_all(app);
        });
    });
    if let Err(e) = started {
        log::error("Presets", &format!("Couldn't start reading the sounds: {e}"));
    }
}

/// Reads the library, every song's newest save and the preset files, handing each batch to the window.
fn read_all(at: &Where) -> Result<(), String> {
    let lib = Library::open_read_only(&at.library).map_err(|e| format!("Couldn't open your library: {e}"))?;
    let saves = lib.saves().map_err(|e| format!("Couldn't read your library: {e}"))?;
    let choices = lib.song_choices().unwrap_or_default();
    let export_root = bitwig::export_root(
        lib.setting(bitwig::EXPORT_FOLDER_KEY).ok().flatten().as_deref(),
        bitwig::home().as_deref(),
    );
    drop(lib);
    let songs = mo_library::songs::group_songs(&saves, &choices);
    let newest = logic::newest_saves(&songs, export_root.as_deref());
    let bitwig_folders = logic::bitwig_folders(&plib::default_folders(), saves.iter().map(|s| s.path.as_str()));
    drop(songs);
    drop(saves);
    let total = newest.len();
    {
        let (root, folders) = (export_root.clone(), bitwig_folders.clone());
        on_window(move |app| {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.export_root = root;
                p.bitwig = folders;
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
        let kept = store.as_mut().map(|st| st.saves(&paths, read_now));
        let read = match kept {
            Some(Ok((read, from))) => {
                from_files += from;
                read
            }
            other => {
                if let Some(Err(e)) = other {
                    log::warning("Presets", &format!("Couldn't keep what the saves hold, so all are read: {}", e.0));
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
    let (found, templates) = background::on_pool(|| {
        let found = plib::find_preset_files(&roots);
        let templates = bw::templates(&found);
        (found, templates)
    });
    let years: HashMap<String, Option<i32>> = background::on_pool(|| {
        use rayon::prelude::*;
        found.par_iter().map(|f| (f.path.clone(), file_year(&f.path))).collect()
    });
    let items: Vec<Item> = plib::rows(&[], &found)
        .into_iter()
        .map(|row| {
            let year = match &row.source {
                Source::File { path } => years.get(path).copied().flatten(),
                Source::Song { .. } => None,
            };
            Item::new(row, String::new(), year)
        })
        .collect();
    let files = items.len();
    let templates = Arc::new(templates);
    on_window(move |app| {
        PAGE.with(|p| p.borrow_mut().templates = Some(templates));
        let _ = app;
    });
    add(items, 0, files, None);
    Ok(())
}

fn file_year(path: &str) -> Option<i32> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    logic::year_of(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// Adds lines read on another thread to the page. While reading goes on the list is shown afresh at most four times
/// a second, so a library of thousands of songs doesn't keep the window busy.
fn add(items: Vec<Item>, songs: usize, files: usize, reading: Option<(usize, usize)>) {
    on_window(move |app| {
        let show = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let mut items = items;
            for i in &mut items {
                if let Some(fix) = p.fixed_now.get(&i.row.hash) {
                    i.row.fixed = *fix;
                }
            }
            p.items.extend(items);
            p.songs += songs;
            p.files += files;
            if reading.is_some() {
                p.reading = reading;
            }
            // At most four times a second while reading: the plugin list and the filtering go over every line.
            let due = p.shown_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) || reading.is_none();
            if due {
                p.shown_at = Some(Instant::now());
                // Every plugin: the menu scrolls, and only makes the lines in sight.
                p.plugins = logic::plugins(&p.items);
                refilter(&mut p);
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
    let Some(item) = PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|i| p.items.get(i).cloned())
    }) else {
        return false;
    };
    let feed = PAGE.with(|p| p.borrow().feed.clone());
    if let Some(role) = logic::part_of(id) {
        set_fix(app, &item, Some(role));
        return true;
    }
    let enabled =
        |id: &str| logic::offers(&item, PAGE.with(|p| p.borrow().exporting)).iter().any(|o| o.id == id && o.enabled);
    match id {
        act::USE_GUESS if item.row.fixed.is_some() => set_fix(app, &item, None),
        act::REVEAL => crate::files::reveal(&feed, item.path(), mo_app::shell::folder_of(item.path())),
        act::COPY_PATH => crate::files::copy_path(&feed, item.path()),
        act::EXPORT if enabled(id) => export_one(app, item, false),
        act::TO_BITWIG if enabled(id) => {
            if item.is_file() {
                send_file(item.path().to_owned());
            } else {
                export_one(app, item, true);
            }
        }
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
        logic::apply_fix(&mut p.items, hash, role);
        p.fixed_now.insert(hash.to_owned(), role);
        // A part filter may drop it from the list; the card stays on it.
        refilter(&mut p);
    });
    show_all(app);
}

/// The export folder and the templates, found now if the page hasn't yet (a click before the preset files were read).
fn export_setup() -> (Option<PathBuf>, Option<Arc<Templates>>, Vec<PathBuf>, Feed) {
    PAGE.with(|p| {
        let p = p.borrow();
        (p.export_root.clone(), p.templates.clone(), p.bitwig.clone(), p.feed.clone())
    })
}

fn templates_now(have: Option<Arc<Templates>>, bitwig: &[PathBuf]) -> Arc<Templates> {
    have.unwrap_or_else(|| {
        let roots = logic::preset_folders(&plib::default_folders(), bitwig);
        Arc::new(background::on_pool(|| bw::templates(&plib::find_preset_files(&roots))))
    })
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
    let (root, templates, bitwig_folders, feed) = export_setup();
    let Some(root) = root else {
        set_message(app, "There's no export folder. Pick one in Settings.");
        return;
    };
    PAGE.with(|p| p.borrow_mut().exporting = true);
    show_all(app);
    let job = feed.start(if send { "Making a Bitwig preset" } else { "Exporting a preset" }, &item.row.name);
    let started = background::spawn("Preset export", move || {
        let templates = templates_now(templates, &bitwig_folders);
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
                        None => (
                            format!(
                                "No Bitwig preset could be made for {}: save any preset of it once in Bitwig, then \
                                 try again.",
                                item.row.plugin
                            ),
                            false,
                        ),
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

/// The plugins among `items` that could get a Bitwig preset, but have no template yet.
fn without_template(items: &[&Item], templates: &Templates) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        let format = bw::state_format(&i.row.extension);
        if format.is_empty() {
            continue;
        }
        let has = bw::key(&i.row.plugin, format).is_some_and(|k| templates.contains_key(&k));
        if !has && !out.iter().any(|o| o.eq_ignore_ascii_case(&i.row.plugin)) {
            out.push(i.row.plugin.clone());
        }
    }
    out
}

/// Export these: every sound shown, save by save, into the export folder. Preset files are left (they're files).
fn export_shown(app: &AppWindow) -> bool {
    let (plan, files, items) = PAGE.with(|p| {
        let p = p.borrow();
        let (plan, files) = logic::export_plan(&p.items, &p.shown);
        let items: Vec<Item> =
            p.shown.iter().filter_map(|&i| p.items.get(i).cloned()).filter(|i| !i.is_file()).collect();
        (plan, files, items)
    });
    if plan.is_empty() || PAGE.with(|p| p.borrow().exporting) {
        if files > 0 {
            set_message(app, "The list shows only preset files, which are files already: nothing to export.");
        }
        return false;
    }
    let (root, templates, bitwig_folders, feed) = export_setup();
    let Some(root) = root else {
        set_message(app, "There's no export folder. Pick one in Settings.");
        return true;
    };
    PAGE.with(|p| p.borrow_mut().exporting = true);
    show_all(app);
    let job = feed.start("Exporting presets", &format!("{} songs", logic::thousands(plan.len())));
    let started = background::spawn("Preset export", move || {
        let templates = templates_now(templates, &bitwig_folders);
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
        let refs: Vec<&Item> = items.iter().collect();
        let mut text = logic::export_summary(&done, &without_template(&refs, &templates), &failed);
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
    p.curves = list;
    p.curves_failed = None;
    (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_filters);
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
    let export_root = bitwig::export_root(
        lib.setting(bitwig::EXPORT_FOLDER_KEY).ok().flatten().as_deref(),
        bitwig::home().as_deref(),
    );
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
                    if let Some(old) = p.curves.iter_mut().find(|o| o.id == c.id) {
                        *old = c;
                    }
                    (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_filters);
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

/// Shows made-up sounds and curves, for mo-screens (no library, nothing read).
pub fn show_sample(app: &AppWindow) {
    wire(app);
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.started = true;
        p.curves_started = true;
        p.items = sample_items();
        p.songs = 3;
        p.files = p.items.iter().filter(|i| i.is_file()).count();
        p.plugins = logic::plugins(&p.items);
        p.curves = sample_curves();
        p.bitwig = vec![PathBuf::from(r"K:\Bitwig Studio")];
        refilter(&mut p);
        (p.curve_shown, p.curve_found_in) = logic::shown_curves(&p.curves, &p.curve_filters);
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
