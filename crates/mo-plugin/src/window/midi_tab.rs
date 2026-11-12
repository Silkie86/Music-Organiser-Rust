//! The panel's MIDI tab: the parts of the app's MIDI index (`crate::midi`), the MIDI page's filter column, orders
//! and year strip, and a part dragged out onto a track. The index is read the first time the tab is shown, and the
//! list is worked out on background threads, the newest asking's answer shown, as the Sounds tab's is.

use super::{Carried, Choices, Slots};
use crate::midi::{self, Asked, ORDERS, Parts, Read};
use crate::threads::{background, lock};
use crate::{Panel, PartLine, Theme, YearBar};
use mo_app::midi::{self as page, ChoiceCounts};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The panel's view number for the MIDI tab (Sounds 0, MIDI 1, DAW check 2).
const VIEW: i32 = 1;

/// What background threads hand the tab: the newest of each wins.
#[derive(Default)]
struct PartSlots {
    read: Mutex<Option<Read>>,
    /// What the list shows for what was asked, with the filter column's counts and the asking's number.
    found: Mutex<Option<(u64, Vec<usize>, ChoiceCounts)>>,
    /// Counts askings: only the newest one's answer is shown.
    searches: AtomicU64,
    /// The piano roll drawn for the part picked, with its asking's number (none: it couldn't be).
    roll: Mutex<Option<(u64, Option<midi::RollPixels>)>>,
    /// Counts piano rolls asked for: only the newest is shown.
    rolls: AtomicU64,
    /// The part the piano roll shows (its place in the catalogue), with the parts and the scale it was drawn at:
    /// drawn again when following the playing is turned on or off, and the play head shows only over its own part.
    rolled: Mutex<Option<(Arc<Parts>, usize, f64)>>,
}

/// The list's lines, made only for the rows shown.
struct PartLines {
    parts: RefCell<Option<Arc<Parts>>>,
    shown: RefCell<Arc<Vec<usize>>>,
    notify: ModelNotify,
}

impl PartLines {
    fn show(&self, parts: Option<Arc<Parts>>, shown: Arc<Vec<usize>>) {
        *self.parts.borrow_mut() = parts;
        *self.shown.borrow_mut() = shown;
        self.notify.reset();
    }

    /// The row showing the part at place `at` in the catalogue, or -1.
    fn row_of(&self, at: usize) -> i32 {
        let row = self.shown.borrow().iter().position(|&a| a == at);
        row.and_then(|r| i32::try_from(r).ok()).unwrap_or(-1)
    }

    /// The part on `row`, as a place in the catalogue.
    fn at(&self, row: usize) -> Option<usize> {
        self.shown.borrow().get(row).copied()
    }
}

impl Model for PartLines {
    type Data = PartLine;

    fn row_count(&self) -> usize {
        self.shown.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<PartLine> {
        let parts = self.parts.borrow();
        let e = parts.as_ref()?.entry(self.at(row)?)?;
        Some(PartLine {
            title: page::title(e).into(),
            from: page::from_text(e).into(),
            details: page::details(e).into(),
            colour: page::role_colour(e.role()),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The filter column's lines, kept from one showing to the next, and the counts of the list last worked out.
#[derive(Default)]
struct PartColumn {
    kind: Choices,
    from: Choices,
    key: Choices,
    counts: RefCell<Option<ChoiceCounts>>,
    /// How many years the year strip was last given.
    years: Cell<usize>,
}

impl PartColumn {
    fn show(&self, panel: &Panel, parts: Option<&Parts>, asked: &Asked) {
        let chips = midi::chips(self.counts.borrow().as_ref(), asked);
        panel.set_kind_filter(self.kind.view(&chips.kind));
        panel.set_from_filter(self.from.view(&chips.from));
        panel.set_part_key_filter(self.key.view(&chips.key));
        let years = parts.map_or(&[][..], |p| &p.years[..]);
        let most = years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        if self.years.replace(years.len()) != years.len() {
            let bars: Vec<YearBar> = years
                .iter()
                .map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most })
                .collect();
            panel.set_midi_years(ModelRc::new(VecModel::from(bars)));
        }
        let (from, to) = mo_app::year_strip::picked_places(years, asked.filters.years).unwrap_or((-1, -1));
        panel.set_midi_years_from(from);
        panel.set_midi_years_to(to);
        let order = ORDERS.iter().position(|&o| o == asked.order).and_then(|i| i32::try_from(i).ok());
        panel.set_midi_order(order.unwrap_or(0));
    }
}

pub(super) struct MidiTab {
    lines: Rc<PartLines>,
    slots: Arc<PartSlots>,
    asked: Rc<RefCell<Asked>>,
    column: Rc<PartColumn>,
    /// The index has been asked for and not yet answered, or it was read: not asked for again.
    read_asked: Cell<bool>,
    /// The tab was showing at the last tick: a reading that failed is tried again when it's shown again (the app
    /// may have made the index since).
    was_shown: Cell<bool>,
    /// The play head's place last shown (-1 for none): the panel is told only when it moves.
    head: Cell<f32>,
    /// The track's name has picked the list's first filter (once only: the filters are the user's after that).
    track_picked: Cell<bool>,
}

impl MidiTab {
    pub(super) fn new() -> MidiTab {
        MidiTab {
            lines: Rc::new(PartLines {
                parts: RefCell::new(None),
                shown: RefCell::new(Arc::default()),
                notify: ModelNotify::default(),
            }),
            slots: Arc::default(),
            asked: Rc::default(),
            column: Rc::default(),
            read_asked: Cell::new(false),
            was_shown: Cell::new(false),
            head: Cell::new(-1.0),
            track_picked: Cell::new(false),
        }
    }

    /// The tab's list and what its filters, orders and lines do. A dragged part's file is written on a background
    /// thread and handed to the window's drag (`slots.drag`).
    pub(super) fn fill_in(&self, panel: &Panel, window_slots: &Arc<Slots>) {
        panel.set_parts(ModelRc::from(Rc::clone(&self.lines) as Rc<dyn Model<Data = PartLine>>));
        panel.set_midi_status("Reading your MIDI".into());
        panel.set_midi_orders(ModelRc::new(VecModel::from(
            ORDERS.iter().map(|o| SharedString::from(order_words(*o))).collect::<Vec<_>>(),
        )));
        let changed = {
            let lines = Rc::clone(&self.lines);
            let slots = Arc::clone(&self.slots);
            let asked = Rc::clone(&self.asked);
            let column = Rc::clone(&self.column);
            let weak = panel.as_weak();
            Rc::new(move |change: &dyn Fn(&mut Asked, Option<&Parts>)| {
                let parts = lines.parts.borrow().clone();
                change(&mut asked.borrow_mut(), parts.as_deref());
                let asked = asked.borrow().clone();
                if let Some(panel) = weak.upgrade() {
                    column.show(&panel, parts.as_deref(), &asked);
                }
                if let Some(parts) = parts {
                    view(parts, asked, Arc::clone(&slots));
                }
            })
        };
        let change = Rc::clone(&changed);
        panel.on_midi_search(move |typed| change(&|a, _| a.filters.text = typed.trim().to_owned()));
        let change = Rc::clone(&changed);
        panel.on_midi_pick(move |filter, choice| change(&|a, _| a.filters.toggle(&filter, &choice)));
        let change = Rc::clone(&changed);
        panel.on_midi_clear(move |filter| change(&|a, _| a.filters.clear(&filter)));
        let change = Rc::clone(&changed);
        panel.on_midi_years_picked(move |a, b| {
            change(&|asked, parts| {
                let years = parts.map_or(&[][..], |p| &p.years[..]);
                asked.filters.years = mo_app::year_strip::year_span(years, a.min(b), a.max(b));
            });
        });
        let change = Rc::clone(&changed);
        panel.on_midi_years_cleared(move || change(&|asked, _| asked.filters.years = None));
        let change = changed;
        panel.on_midi_pick_order(move |i| {
            let Some(&order) = usize::try_from(i).ok().and_then(|i| ORDERS.get(i)) else { return };
            change(&|a, _| a.order = order);
        });
        // Following the playing turned on or off: the roll is drawn again for it.
        let slots = Arc::clone(&self.slots);
        let weak = panel.as_weak();
        panel.on_midi_follow_changed(move || {
            let rolled = lock(&slots.rolled).clone();
            if let (Some(panel), Some((parts, at, scale))) = (weak.upgrade(), rolled) {
                draw_roll(&panel, &slots, parts, at, scale);
            }
        });
        let lines = Rc::clone(&self.lines);
        let slots = Arc::clone(window_slots);
        let weak = panel.as_weak();
        panel.on_midi_drag_out(move |row| {
            let Some(row) = usize::try_from(row).ok() else { return };
            // Carried by its place in the catalogue: a new list may land before the file is written.
            let (Some(parts), Some(at)) = (lines.parts.borrow().clone(), lines.at(row)) else { return };
            if let Some(panel) = weak.upgrade() {
                panel.set_midi_said(SharedString::new());
            }
            let slots = Arc::clone(&slots);
            // Its notes are read from the index and written to a file off the window's thread.
            background("mo-plugin-drag", move || {
                let started = Instant::now();
                let file = parts.file(at, &std::env::temp_dir());
                crate::log::took_if_slow("Drag", "Wrote a MIDI part to drag", started);
                *lock(&slots.drag) = Some((Carried::Part(at), file));
            });
        });
        self.column.show(panel, None, &self.asked.borrow());
    }

    /// Draws a part's piano roll (the parts, its place in them and the screen's pixels to a point), as
    /// [`draw_roll`] does.
    pub(super) fn roll_drawer(&self, panel: &Panel) -> impl Fn(Arc<Parts>, usize, f64) + 'static {
        let slots = Arc::clone(&self.slots);
        let weak = panel.as_weak();
        move |parts, at, scale| {
            if let Some(panel) = weak.upgrade() {
                draw_roll(&panel, &slots, parts, at, scale);
            }
        }
    }

    /// The piano roll goes (the list changed): a roll still being drawn isn't shown.
    fn forget_roll(&self, panel: &Panel) {
        self.slots.rolls.fetch_add(1, Ordering::AcqRel);
        *lock(&self.slots.rolled) = None;
        panel.set_midi_roll(slint::Image::default());
        panel.set_midi_roll_points(0.0);
    }

    /// The play head over the piano roll: `playing` is the part playing now (its place in the catalogue) and how far
    /// through its loop it is (0 to 1); it shows only while that's the part the roll shows.
    pub(super) fn show_head(&self, panel: &Panel, playing: Option<(usize, f32)>) {
        let rolled = lock(&self.slots.rolled).as_ref().map(|r| r.1);
        let head = match playing {
            Some((at, place)) if rolled == Some(at) => place,
            _ => -1.0,
        };
        if (head - self.head.replace(head)).abs() > f32::EPSILON {
            panel.set_midi_head(head);
        }
    }

    /// Finds the parts and the place in them of the part on a row, as the list shows it then.
    pub(super) fn part_finder(&self) -> impl Fn(usize) -> Option<(Arc<Parts>, usize)> + 'static {
        let lines = Rc::clone(&self.lines);
        move |row| Some((lines.parts.borrow().clone()?, lines.at(row)?))
    }

    /// The row showing the part at place `at` in the catalogue (-1 for none): the one being carried.
    pub(super) fn row_of(&self, at: usize) -> i32 {
        self.lines.row_of(at)
    }

    /// Takes in what background threads found; the index is read the first time the tab shows.
    pub(super) fn tick(&self, panel: &Panel) {
        let shown = panel.get_view() == VIEW;
        let just_shown = !self.was_shown.replace(shown) && shown;
        if just_shown && !self.read_asked.replace(true) {
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-midi", move || *lock(&slots.read) = Some(midi::read()));
        }
        if let Some(read) = lock(&self.slots.read).take() {
            match read {
                Read::Ready(parts) => {
                    // On a track named for a kind of part, the list opens on it ("Bass": basslines).
                    let kinds = mo_midi::roles::Role::ALL.map(mo_midi::roles::Role::plural);
                    let named = if self.track_picked.replace(true) {
                        Vec::new()
                    } else {
                        crate::track::named(&panel.get_track(), kinds)
                    };
                    for kind in named {
                        self.asked.borrow_mut().filters.toggle(mo_app::midi::KIND, &kind);
                    }
                    self.lines.show(Some(Arc::clone(&parts)), Arc::default());
                    panel.set_midi_status("Getting the list ready".into());
                    self.column.show(panel, Some(&parts), &self.asked.borrow());
                    view(parts, self.asked.borrow().clone(), Arc::clone(&self.slots));
                }
                Read::Not(why) => {
                    panel.set_midi_status(why.into());
                    self.read_asked.set(false);
                }
            }
        }
        if let Some((number, drawn)) = lock(&self.slots.roll).take()
            && number == self.slots.rolls.load(Ordering::Acquire)
        {
            let image = drawn.map(|d| {
                let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(d.width, d.height);
                for (to, from) in pixels.make_mut_bytes().iter_mut().zip(&d.rgba) {
                    *to = *from;
                }
                panel.set_midi_roll_points(d.points as f32);
                panel.set_midi_roll_from(d.bars.0 as f32);
                panel.set_midi_roll_to(d.bars.1 as f32);
                slint::Image::from_rgba8_premultiplied(pixels)
            });
            if image.is_none() {
                panel.set_midi_roll_points(0.0);
            }
            panel.set_midi_roll(image.unwrap_or_default());
        }
        if let Some((number, found, counts)) = lock(&self.slots.found).take()
            && number == self.slots.searches.load(Ordering::Acquire)
        {
            let parts = self.lines.parts.borrow().clone();
            // Worked out with the list, off this thread.
            let total = counts.lines;
            let narrowed = self.asked.borrow().filters.any();
            panel.set_midi_status(midi::count_line(total, narrowed.then_some(found.len())).into());
            panel.set_midi_picked(-1);
            self.forget_roll(panel);
            let empty = if found.is_empty() && narrowed { "No parts match: take a filter or a word away" } else { "" };
            panel.set_midi_empty(empty.into());
            *self.column.counts.borrow_mut() = Some(counts);
            self.column.show(panel, parts.as_deref(), &self.asked.borrow());
            self.lines.show(parts, Arc::new(found));
        }
    }
}

/// An order's words on its button, as the app's MIDI page says them, with a capital: "Newest first".
fn order_words(order: mo_app::midi::Order) -> String {
    let words = order.words(false);
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Draws the piano roll of the part at `at` on a background thread, for the room the panel has for it at `scale`
/// pixels to a point, in the panel's colours; it shows once drawn, if it's still the newest asked for.
fn draw_roll(panel: &Panel, slots: &Arc<PartSlots>, parts: Arc<Parts>, at: usize, scale: f64) {
    let number = slots.rolls.fetch_add(1, Ordering::AcqRel) + 1;
    *lock(&slots.rolled) = Some((Arc::clone(&parts), at, scale));
    // The room the roll shows in once a part is picked (its height is set; its width is the list's).
    let (width, height) = (f64::from(panel.get_midi_roll_width()), ROLL_HEIGHT);
    let follow = panel.get_midi_follow();
    let colours = colours(panel);
    let slots = Arc::clone(slots);
    background("mo-plugin-roll", move || {
        if slots.rolls.load(Ordering::Acquire) != number {
            return;
        }
        let started = Instant::now();
        let drawn = parts.roll(at, (width, height, scale), &colours, follow);
        if let Err(why) = &drawn {
            crate::log::line("MIDI", &format!("A part's piano roll couldn't be drawn: {why}"));
        }
        crate::log::took_if_slow("MIDI", "Drew a piano roll", started);
        *lock(&slots.roll) = Some((number, drawn.ok()));
    });
}

/// The piano roll's height on the panel, in points (panel.slint's `roll-well`).
const ROLL_HEIGHT: f64 = 120.0;

/// The piano roll's colours, as the app's MIDI page takes them from its theme.
fn colours(panel: &Panel) -> mo_app::piano_roll::Colours {
    use mo_app::shapes::Colour;
    let t = panel.global::<Theme>();
    let rgb = |c: slint::Color| (u32::from(c.red()) << 16) | (u32::from(c.green()) << 8) | u32::from(c.blue());
    let colour = |c: slint::Color| Colour::rgb(rgb(c), f64::from(c.alpha()) / 255.0);
    let (white_key, black_key) =
        if t.get_light() { (t.get_surface(), t.get_text()) } else { (t.get_muted(), t.get_background()) };
    mo_app::piano_roll::Colours {
        well: rgb(t.get_well()),
        shine: colour(t.get_shine()),
        edge: colour(t.get_edge()),
        white_key: colour(white_key),
        black_key: colour(black_key),
        accent: colour(t.get_accent()),
        accent_text: colour(t.get_accent_text()),
    }
}

/// Works out what `asked` shows of `parts` on a background thread; it lands in `slots.found` with this asking's
/// number, and only the newest asking's answer is shown.
fn view(parts: Arc<Parts>, asked: Asked, slots: Arc<PartSlots>) {
    let number = slots.searches.fetch_add(1, Ordering::AcqRel) + 1;
    background("mo-plugin-midi-view", move || {
        if slots.searches.load(Ordering::Acquire) != number {
            return;
        }
        let started = Instant::now();
        let (shown, counts) = parts.view(&asked);
        crate::log::took_if_slow("MIDI", &format!("Worked out a list of {} parts", shown.len()), started);
        *lock(&slots.found) = Some((number, shown, counts));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_words_start_with_a_capital() {
        assert_eq!(order_words(mo_app::midi::Order::Newest), "Newest first");
    }
}
