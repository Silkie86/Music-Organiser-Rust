//! The panel's Presets tab: the preset files the app's Presets page lists (`crate::presets`), its filter column,
//! orders and year strip, and a preset dragged out onto a track or a device. The files are found the first time the
//! tab is shown, and the list is worked out on background threads, the newest asking's answer shown, as the other
//! tabs' are.

use super::{Carried, Choices, Slots};
use crate::presets::{self, ORDERS, Presets, Read};
use crate::threads::{background, lock};
use crate::{Panel, PresetLine, YearBar};
use mo_app::presets::{self as page, ChoiceCounts, Filtered, Filters, Order};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The panel's view number for the Presets tab (Sounds 0, MIDI 1, Presets 2, DAW check 3).
const VIEW: i32 = 2;

/// What background threads hand the tab: the newest of each wins.
#[derive(Default)]
struct PresetSlots {
    read: Mutex<Option<Read>>,
    /// What the list shows for what was asked (with the year strip's and the filter column's counts), and the
    /// asking's number.
    found: Mutex<Option<(u64, Filtered)>>,
    /// Counts askings: only the newest one's answer is shown.
    searches: AtomicU64,
}

/// The list's lines, made only for the rows shown.
struct PresetLines {
    presets: RefCell<Option<Arc<Presets>>>,
    shown: RefCell<Arc<Vec<usize>>>,
    notify: ModelNotify,
}

impl PresetLines {
    fn show(&self, presets: Option<Arc<Presets>>, shown: Arc<Vec<usize>>) {
        *self.presets.borrow_mut() = presets;
        *self.shown.borrow_mut() = shown;
        self.notify.reset();
    }

    /// The row showing the preset at place `at` in the list, or -1.
    fn row_of(&self, at: usize) -> i32 {
        let row = self.shown.borrow().iter().position(|&a| a == at);
        row.and_then(|r| i32::try_from(r).ok()).unwrap_or(-1)
    }

    /// The preset on `row`, as a place in the list.
    fn at(&self, row: usize) -> Option<usize> {
        self.shown.borrow().get(row).copied()
    }
}

impl Model for PresetLines {
    type Data = PresetLine;

    fn row_count(&self) -> usize {
        self.shown.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<PresetLine> {
        let presets = self.presets.borrow();
        let line = page::line(presets.as_ref()?.item(self.at(row)?)?);
        let details = [line.under.as_str(), line.kind.as_str(), line.format.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("  ·  ");
        Some(PresetLine {
            name: line.name.into(),
            details: details.into(),
            part: line.part.into(),
            colour: line.colour,
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The filter column's lines, kept from one showing to the next, and the counts of the list last worked out.
#[derive(Default)]
struct PresetColumn {
    part: Choices,
    kind: Choices,
    format: Choices,
    plugin: Choices,
    counts: RefCell<Option<ChoiceCounts>>,
    /// The year strip's years for the list last worked out (with every filter but the years on, as the app's page
    /// counts them), newest first, with how many each.
    years: RefCell<Vec<(i32, usize)>>,
}

impl PresetColumn {
    /// The year strip shows `years` from now on.
    fn show_years(&self, panel: &Panel, years: Vec<(i32, usize)>) {
        if *self.years.borrow() == years {
            return;
        }
        let most = years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        let bars: Vec<YearBar> =
            years.iter().map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most }).collect();
        panel.set_preset_years(ModelRc::new(VecModel::from(bars)));
        *self.years.borrow_mut() = years;
    }

    fn show(&self, panel: &Panel, presets: Option<&Presets>, filters: &Filters) {
        let counts = self.counts.borrow();
        let chips = match presets {
            Some(p) => p.chips(counts.as_ref(), filters),
            None => page::filter_chips(&ChoiceCounts::default(), &[], filters),
        };
        panel.set_preset_part_filter(self.part.view(&chips.part));
        panel.set_preset_kind_filter(self.kind.view(&chips.kind));
        panel.set_preset_format_filter(self.format.view(&chips.format));
        panel.set_preset_plugin_filter(self.plugin.view(&chips.plugin));
        let (from, to) = mo_app::year_strip::picked_places(&self.years.borrow(), filters.years).unwrap_or((-1, -1));
        panel.set_preset_years_from(from);
        panel.set_preset_years_to(to);
        let order = ORDERS.iter().position(|&(o, _)| o == filters.order).and_then(|i| i32::try_from(i).ok());
        panel.set_preset_order(order.unwrap_or(0));
    }
}

pub(super) struct PresetsTab {
    lines: Rc<PresetLines>,
    slots: Arc<PresetSlots>,
    filters: Rc<RefCell<Filters>>,
    column: Rc<PresetColumn>,
    /// The files have been asked for and not yet answered, or they were read: not asked for again.
    read_asked: Cell<bool>,
    /// The tab was showing at the last tick: a reading that failed is tried again when it's shown again.
    was_shown: Cell<bool>,
}

impl PresetsTab {
    pub(super) fn new() -> PresetsTab {
        PresetsTab {
            lines: Rc::new(PresetLines {
                presets: RefCell::new(None),
                shown: RefCell::new(Arc::default()),
                notify: ModelNotify::default(),
            }),
            slots: Arc::default(),
            filters: Rc::default(),
            column: Rc::default(),
            read_asked: Cell::new(false),
            was_shown: Cell::new(false),
        }
    }

    /// The tab's list and what its filters, orders and lines do. A dragged preset's file is checked to be there on a
    /// background thread and handed to the window's drag (`slots.drag`).
    pub(super) fn fill_in(&self, panel: &Panel, window_slots: &Arc<Slots>) {
        panel.set_presets(ModelRc::from(Rc::clone(&self.lines) as Rc<dyn Model<Data = PresetLine>>));
        panel.set_preset_status("Looking for your preset files".into());
        panel.set_preset_orders(ModelRc::new(VecModel::from(
            ORDERS.iter().map(|(_, words)| SharedString::from(*words)).collect::<Vec<_>>(),
        )));
        let changed = {
            let lines = Rc::clone(&self.lines);
            let slots = Arc::clone(&self.slots);
            let filters = Rc::clone(&self.filters);
            let column = Rc::clone(&self.column);
            let weak = panel.as_weak();
            Rc::new(move |change: &dyn Fn(&mut Filters, &PresetColumn)| {
                let presets = lines.presets.borrow().clone();
                change(&mut filters.borrow_mut(), &column);
                let filters = filters.borrow().clone();
                if let Some(panel) = weak.upgrade() {
                    column.show(&panel, presets.as_deref(), &filters);
                }
                if let Some(presets) = presets {
                    view(presets, filters, Arc::clone(&slots));
                }
            })
        };
        let change = Rc::clone(&changed);
        panel.on_preset_search(move |typed| change(&|f, _| f.search = typed.trim().to_owned()));
        let change = Rc::clone(&changed);
        panel.on_preset_pick(move |filter, choice| change(&|f, _| f.toggle(&filter, &choice)));
        let change = Rc::clone(&changed);
        panel.on_preset_clear(move |filter| change(&|f, _| f.clear_chip(&filter)));
        let change = Rc::clone(&changed);
        panel.on_preset_years_picked(move |a, b| {
            change(&|f, column| {
                f.years = mo_app::year_strip::year_span(&column.years.borrow(), a.min(b), a.max(b));
            });
        });
        let change = Rc::clone(&changed);
        panel.on_preset_years_cleared(move || change(&|f, _| f.years = None));
        let change = changed;
        panel.on_preset_pick_order(move |i| {
            let Some(&(order, _)) = usize::try_from(i).ok().and_then(|i| ORDERS.get(i)) else { return };
            // As the app's page picks a column's order: a second pick turns it round, a third goes back to the list
            // by folder.
            change(&|f, _| match order {
                Order::Found => (f.order, f.reversed) = (Order::Found, false),
                _ => f.order_by(order),
            });
        });
        let lines = Rc::clone(&self.lines);
        let weak = panel.as_weak();
        panel.on_preset_click(move |row| {
            let Some(panel) = weak.upgrade() else { return };
            let presets = lines.presets.borrow().clone();
            let item = usize::try_from(row).ok().and_then(|r| lines.at(r)).and_then(|at| presets.as_ref()?.item(at));
            panel.set_preset_said(item.map(presets::hint).unwrap_or_default().into());
        });
        let lines = Rc::clone(&self.lines);
        let slots = Arc::clone(window_slots);
        let weak = panel.as_weak();
        panel.on_preset_drag_out(move |row| {
            let Some(row) = usize::try_from(row).ok() else { return };
            // Carried by its place in the list: a new list may land before the file is checked.
            let (Some(presets), Some(at)) = (lines.presets.borrow().clone(), lines.at(row)) else { return };
            if let Some(panel) = weak.upgrade() {
                panel.set_preset_said(SharedString::new());
            }
            let slots = Arc::clone(&slots);
            // Checked to be there off the window's thread (a drive may be asleep).
            background("mo-plugin-drag", move || {
                let file = presets.file(at);
                *lock(&slots.drag) = Some((Carried::Preset(at), file));
            });
        });
        self.column.show(panel, None, &self.filters.borrow());
    }

    /// The row showing the preset at place `at` in the list (-1 for none): the one being carried.
    pub(super) fn row_of(&self, at: usize) -> i32 {
        self.lines.row_of(at)
    }

    /// Takes in what background threads found; the files are found the first time the tab shows.
    pub(super) fn tick(&self, panel: &Panel) {
        let shown = panel.get_view() == VIEW;
        let just_shown = !self.was_shown.replace(shown) && shown;
        if just_shown && !self.read_asked.replace(true) {
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-presets", move || *lock(&slots.read) = Some(presets::read()));
        }
        if let Some(read) = lock(&self.slots.read).take() {
            match read {
                Read::Ready(presets) => {
                    self.lines.show(Some(Arc::clone(&presets)), Arc::default());
                    panel.set_preset_status("Getting the list ready".into());
                    self.column.show(panel, Some(&presets), &self.filters.borrow());
                    view(presets, self.filters.borrow().clone(), Arc::clone(&self.slots));
                }
                Read::Not(why) => {
                    panel.set_preset_status(why.into());
                    self.read_asked.set(false);
                }
            }
        }
        if let Some((number, Filtered { shown: found, years, counts })) = lock(&self.slots.found).take()
            && number == self.slots.searches.load(Ordering::Acquire)
        {
            let presets = self.lines.presets.borrow().clone();
            let total = presets.as_ref().map_or(0, |p| p.items.len());
            let narrowed = self.filters.borrow().any() || !self.filters.borrow().search.is_empty();
            panel.set_preset_status(presets::count_line(total, narrowed.then_some(found.len())).into());
            panel.set_preset_picked(-1);
            panel.set_preset_said(SharedString::new());
            let empty = if found.is_empty() { "No presets match: take a filter or a word away" } else { "" };
            panel.set_preset_empty(empty.into());
            *self.column.counts.borrow_mut() = Some(counts);
            self.column.show_years(panel, years);
            self.column.show(panel, presets.as_deref(), &self.filters.borrow());
            self.lines.show(presets, Arc::new(found));
        }
    }
}

/// Works out what `filters` show of `presets` on a background thread; it lands in `slots.found` with this asking's
/// number, and only the newest asking's answer is shown.
fn view(presets: Arc<Presets>, filters: Filters, slots: Arc<PresetSlots>) {
    let number = slots.searches.fetch_add(1, Ordering::AcqRel) + 1;
    background("mo-plugin-presets-view", move || {
        if slots.searches.load(Ordering::Acquire) != number {
            return;
        }
        let started = Instant::now();
        let found = presets.view(&filters);
        crate::log::took_if_slow("Presets", &format!("Worked out a list of {} presets", found.shown.len()), started);
        *lock(&slots.found) = Some((number, found));
    });
}
