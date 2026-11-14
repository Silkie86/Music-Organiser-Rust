//! The panel's Curves tab: the curves the app's Presets › Curves keeps (`crate::curves`), searched and filtered as the
//! app's list is, and a curve dragged out as a Bitwig curve file. The curves are read the first time the tab is shown
//! (and again each time it's shown after that, so a curve the app found since is there), on a background thread.

use super::curve_bench::Bench;
use super::{Carried, Choices, Slots};
use crate::curves::{self, Curves, Read};
use crate::threads::{background, lock};
use crate::{CurveLine, Panel};
use mo_app::presets::{self as page, CurveFilters};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// The panel's view number for the Curves tab (shown beside Presets).
const VIEW: i32 = 5;

/// The list's lines, made only for the rows shown.
#[derive(Default)]
struct CurveLines {
    curves: RefCell<Option<Arc<Curves>>>,
    shown: RefCell<Vec<usize>>,
    notify: ModelNotify,
}

impl CurveLines {
    /// The row showing the curve at place `at`, or -1.
    fn row_of(&self, at: usize) -> i32 {
        let row = self.shown.borrow().iter().position(|&a| a == at);
        row.and_then(|r| i32::try_from(r).ok()).unwrap_or(-1)
    }

    fn at(&self, row: usize) -> Option<usize> {
        self.shown.borrow().get(row).copied()
    }
}

impl Model for CurveLines {
    type Data = CurveLine;

    fn row_count(&self) -> usize {
        self.shown.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<CurveLine> {
        let curves = self.curves.borrow();
        let line = page::curve_line(curves.as_ref()?.curves.get(self.at(row)?)?);
        Some(CurveLine {
            name: line.name.into(),
            under: line.under.into(),
            found: line.found.into(),
            length: line.length.into(),
            shape: line.shape.into(),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// What the tab shows, kept from one change to the next.
#[derive(Default)]
struct Shown {
    lines: Rc<CurveLines>,
    filters: RefCell<CurveFilters>,
    from: Choices,
}

impl Shown {
    /// Works the list out again for the filters (quick: each curve's words were worked out once, as the app's list
    /// does it with each key typed), and shows it.
    fn show(&self, panel: &Panel) {
        let curves = self.lines.curves.borrow().clone();
        let filters = self.filters.borrow();
        let (shown, from) = match &curves {
            Some(c) => c.view(&filters),
            None => (Vec::new(), mo_app::filters::Filter::default()),
        };
        panel.set_curve_from_filter(self.from.view(&from));
        panel.set_curve_hidden(filters.hidden);
        if let Some(c) = &curves {
            let narrowed = filters.any() || !filters.search.trim().is_empty() || filters.hidden;
            panel.set_curve_status(curves::count_line(c.curves.len(), narrowed.then_some(shown.len())).into());
            let empty = if shown.is_empty() { "No curves match: take a filter or a word away" } else { "" };
            panel.set_curve_empty(empty.into());
        }
        panel.set_curve_picked(-1);
        panel.set_curve_said(SharedString::new());
        *self.lines.shown.borrow_mut() = shown;
        self.lines.notify.reset();
    }
}

pub(super) struct CurvesTab {
    shown: Rc<Shown>,
    /// The editor a clicked curve opens in.
    bench: Rc<Bench>,
    read: Arc<Mutex<Option<Read>>>,
    /// Asked for and not yet answered: not asked again meanwhile.
    reading: Cell<bool>,
    was_shown: Cell<bool>,
}

impl CurvesTab {
    pub(super) fn new(player: Arc<crate::audition::Player>) -> CurvesTab {
        CurvesTab {
            shown: Rc::default(),
            bench: Bench::new(player),
            read: Arc::default(),
            reading: Cell::new(false),
            was_shown: Cell::new(false),
        }
    }

    /// The tab's list and what its search, filter and lines do. A dragged curve's file is written on a background
    /// thread and handed to the window's drag (`slots.drag`).
    pub(super) fn fill_in(&self, panel: &Panel, window_slots: &Arc<Slots>) {
        panel.set_curves(ModelRc::from(Rc::clone(&self.shown.lines) as Rc<dyn Model<Data = CurveLine>>));
        panel.set_curve_status("Looking for your curves".into());
        let change = |f: fn(&mut CurveFilters, &str)| {
            let shown = Rc::clone(&self.shown);
            let weak = panel.as_weak();
            move |text: SharedString| {
                f(&mut shown.filters.borrow_mut(), &text);
                if let Some(panel) = weak.upgrade() {
                    shown.show(&panel);
                }
            }
        };
        panel.on_curve_search(change(|f, t| f.search = t.trim().to_owned()));
        panel.on_curve_pick(change(|f, t| f.toggle_kind(t)));
        let clear = change(|f, _| f.kinds.clear());
        panel.on_curve_clear(move || clear(SharedString::new()));
        let hidden = change(|f, t| f.hidden = t == "on");
        panel.on_curve_hidden_toggled(move |on| hidden(if on { "on" } else { "" }.into()));
        self.bench.fill_in(panel, window_slots);
        let (shown, bench, weak) = (Rc::clone(&self.shown), Rc::clone(&self.bench), panel.as_weak());
        panel.on_curve_open(move |row| {
            let Some(row) = usize::try_from(row).ok() else { return };
            let (Some(curves), Some(at)) = (shown.lines.curves.borrow().clone(), shown.lines.at(row)) else { return };
            if let (Some(curve), Some(panel)) = (curves.curves.get(at), weak.upgrade()) {
                bench.open(&panel, at, curve);
            }
        });
        let shown = Rc::clone(&self.shown);
        let slots = Arc::clone(window_slots);
        let weak = panel.as_weak();
        panel.on_curve_drag_out(move |row| {
            let Some(row) = usize::try_from(row).ok() else { return };
            let (Some(curves), Some(at)) = (shown.lines.curves.borrow().clone(), shown.lines.at(row)) else { return };
            if let Some(panel) = weak.upgrade() {
                panel.set_curve_said(SharedString::new());
            }
            let slots = Arc::clone(&slots);
            background("mo-plugin-drag", move || {
                let file = curves.to_drag(at, &std::env::temp_dir());
                *lock(&slots.drag) = Some((Carried::Curve(at), file));
            });
        });
    }

    /// The row showing the curve at place `at` (-1 for none): the one being carried.
    pub(super) fn row_of(&self, at: usize) -> i32 {
        self.shown.lines.row_of(at)
    }

    /// Reads the curves each time the tab is shown, and takes in what was read.
    pub(super) fn tick(&self, panel: &Panel) {
        let shown = panel.get_view() == VIEW;
        if !self.was_shown.replace(shown) && shown && !self.reading.replace(true) {
            let read = Arc::clone(&self.read);
            background("mo-plugin-curves", move || *lock(&read) = Some(curves::read()));
        }
        self.bench.tick(panel);
        let Some(read) = lock(&self.read).take() else { return };
        self.reading.set(false);
        match read {
            Read::Ready(curves) => {
                self.bench.read_again(panel, &curves.curves);
                *self.shown.lines.curves.borrow_mut() = Some(Arc::new(curves));
                self.shown.show(panel);
            }
            Read::Not(why) => {
                self.bench.read_again(panel, &[]);
                *self.shown.lines.curves.borrow_mut() = None;
                self.shown.show(panel);
                panel.set_curve_status(why.into());
                panel.set_curve_empty(SharedString::new());
            }
        }
    }
}
