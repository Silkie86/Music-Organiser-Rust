//! Fills the MIDI page from the MIDI index and answers its chips, list and card. Everything slow runs off the window's
//! thread at low priority (rules 12 and 21): reading the index once, the first time the page is shown; working out
//! each list (newest ask wins); and reading the picked part's notes for its piano roll and its key. Lines are only
//! made when the list draws them, so 300,000 parts cost what's on screen.
//!
//! Where the parts come from: the Rust app's own `midi-index.db` when it holds any (its keys are the new key
//! finder's), else the C# app's `midi.db`, read-only (its keys are the old app's; the card reads the new key finder's
//! from the part's notes). Nothing here writes either.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::log;
use mo_app::midi::{self as page, Filters, Order, Roll, Target};
use mo_app::shortcuts::{Shortcuts, what_of};
use mo_formats::NoteEvent;
use mo_harmony::KeyFact;
use mo_library::songs::thousands;
use mo_midi::store::MidiStore;
use mo_midi::{Catalogue, Hit, MidiIndex, Role, StoredPhrase};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::{AppWindow, CardAction, KindChip, MenuItem, MidiData, MidiFact, MidiLine, PickedPart, RollNote, YearBar};

/// Where a part's notes are read from.
#[derive(Clone)]
pub enum NotesFrom {
    /// The Rust app's `midi-index.db`.
    Store(PathBuf),
    /// The C# app's `midi.db`.
    Old(PathBuf),
    /// Made-up notes by hash, for the screenshots.
    Made(Arc<HashMap<String, Vec<NoteEvent>>>),
}

/// The index as read: every part, and where it came from.
pub struct Loaded {
    pub cat: Catalogue,
    /// From the C# app's `midi.db`: the list's keys are the old app's.
    pub old: bool,
    pub notes: NotesFrom,
}

#[derive(Default)]
struct Page {
    cat: Arc<Catalogue>,
    old: bool,
    notes: Option<NotesFrom>,
    filters: Filters,
    order: Order,
    seed: Option<u64>,
    target: Option<Target>,
    shown: Arc<Vec<Hit>>,
    years: Vec<(i32, usize)>,
    /// The picked part, as its place in the catalogue's entries.
    picked: Option<usize>,
    asked: u64,
    notes_asked: u64,
    /// The index is being read, or has been: it's read once, the first time the page is shown.
    started: bool,
    library: PathBuf,
    feed: Option<Feed>,
    shortcuts: Shortcuts,
}

impl Page {
    fn row_of(&self, at: usize) -> Option<usize> {
        self.shown.iter().position(|h| h.entry == at)
    }
}

struct Ask {
    id: u64,
    cat: Arc<Catalogue>,
    filters: Filters,
    order: Order,
    target: Option<Target>,
    seed: Option<u64>,
}

/// An ask for a part's notes: the roll, and its key when the index has no reading of the new key finder.
struct NotesAsk {
    id: u64,
    from: NotesFrom,
    phrase_id: i64,
    hash: String,
    role: Role,
    cut: bool,
    bars: i32,
    read_key: bool,
}

struct NotesRead {
    roll: Roll,
    fact: Option<KeyFact>,
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
    static WORKER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    static NOTES: RefCell<Option<Newest<NotesAsk>>> = const { RefCell::new(None) };
    /// Works everything out in place, for the screenshots, which have no event loop to send answers back to.
    static HERE: RefCell<bool> = const { RefCell::new(false) };
}

struct Lines {
    page: Rc<RefCell<Page>>,
    notify: ModelNotify,
}

impl Model for Lines {
    type Data = MidiLine;

    fn row_count(&self) -> usize {
        self.page.borrow().shown.len()
    }

    fn row_data(&self, row: usize) -> Option<MidiLine> {
        let p = self.page.borrow();
        let hit = p.shown.get(row)?;
        let e = p.cat.entry(hit.entry)?;
        let mut details = page::details(e);
        if let Some(l) = hit.likeness {
            details = format!("{:.0}% alike · {details}", l * 100.0);
        }
        Some(MidiLine {
            row: row as i32,
            title: page::title(e).into(),
            from: page::from_text(e).into(),
            details: details.into(),
            phrase: page::phrase_text(e).into(),
            key: page::key_text(e).into(),
            date: page::date_text(e.phrase.source.mtime).into(),
            colour: colour(e.role()),
            why: why_text(&e.phrase).into(),
            picked: p.picked == Some(hit.entry),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// A kind's colour, the Sounds page's colour for the same kind of sound (Keys, Lead, Bass, Vocal, Kick), so the two
/// pages agree; grey for something else.
fn colour(role: Role) -> i32 {
    match role {
        Role::Chords => 11,
        Role::Melody => 9,
        Role::Bass => 8,
        Role::Arp => 14,
        Role::Drums => 0,
        Role::Other => -1,
    }
}

fn why_text(p: &StoredPhrase) -> String {
    if p.your_role.is_some() { "You said so".to_owned() } else { p.role_why.clone() }
}

/// Wires the page up; the index is read the first time the page is shown ([`shown`]).
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
        p.shortcuts = shortcuts.clone();
    });
    wire(app);
    let data = app.global::<MidiData>();
    data.set_empty_title("Reading your MIDI…".into());
    data.set_empty_text("".into());
    if app.get_place() == mo_app::places::Place::Midi.index() as i32 {
        shown(app);
    }
}

fn wire(app: &AppWindow) {
    let data = app.global::<MidiData>();
    data.set_lines(ModelRc::from(LINES.with(Rc::clone) as Rc<dyn Model<Data = MidiLine>>));
    data.set_froms(strings(["Your projects", "MIDI files"]));
    data.set_keys(strings(page::key_choices()));
    data.set_orders(strings(Order::ALL.map(Order::name)));
    data.set_order(Order::default().name().into());
    show_kinds(app);

    let weak = app.as_weak();
    data.on_search_edited(move |text| {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.filters.text = text.to_string();
                p.seed = None;
            });
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_toggle_kind(move |i| {
        if let (Some(app), Some(role)) = (weak.upgrade(), usize::try_from(i).ok().and_then(|i| Role::ALL.get(i))) {
            PAGE.with(|p| p.borrow_mut().filters.toggle_role(*role));
            show_kinds(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clear_kinds(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().filters.roles.clear());
            show_kinds(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_pick(move |what, value| {
        let Some(app) = weak.upgrade() else { return };
        let data = app.global::<MidiData>();
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            match what.as_str() {
                "from" => p.filters.from = page::from_of_name(&value),
                "key" => p.filters.key = page::key_of_choice(&value),
                "order" => {
                    p.order = Order::from_name(&value);
                    p.seed = None;
                }
                _ => {}
            }
        });
        match what.as_str() {
            "from" => data.set_from(value),
            "key" => data.set_key(value),
            "order" => data.set_order(value),
            _ => {}
        }
        relist(&app);
    });
    let weak = app.as_weak();
    data.on_shuffle(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().seed = Some(new_seed()));
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clear_target(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().target = None);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_years_picked(move |a, b| {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.filters.years = mo_app::year_strip::year_span(&p.years, a.min(b), a.max(b));
            });
            show_years(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_years_cleared(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().filters.years = None);
            show_years(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clear(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.filters = Filters::default();
                p.target = None;
            });
            let data = app.global::<MidiData>();
            data.set_search("".into());
            data.set_from("".into());
            data.set_key("".into());
            show_kinds(&app);
            show_years(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_select(move |row| {
        if let (Some(app), Ok(row)) = (weak.upgrade(), usize::try_from(row)) {
            select_row(&app, row);
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
    data.on_panel_resized(|_| {});
    let weak = app.as_weak();
    data.on_menu_chosen(move |id| {
        if let Some(app) = weak.upgrade() {
            run_action(&app, &id);
        }
    });
}

fn strings<S: Into<SharedString>>(list: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(list.into_iter().map(Into::into).collect::<Vec<SharedString>>()))
}

fn new_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64)
}

/// The page is on show: the first time, the index is read.
pub fn shown(app: &AppWindow) {
    let library = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // Before `connect` (the app opening on this page) there's no library yet: `connect` asks again.
        if p.started || p.library.as_os_str().is_empty() {
            return None;
        }
        p.started = true;
        Some(p.library.clone())
    });
    let Some(library) = library else { return };
    let weak = app.as_weak();
    let started = background::spawn("MIDI index", move || {
        let loaded = load(&library);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                show(&app, loaded);
            }
        });
    });
    if let Err(e) = started {
        show(app, Err(format!("its reader couldn't start ({e})")));
    }
}

/// Reads the index beside the library: the Rust app's when it holds parts, else the C# app's. Both read-only.
pub fn load(library: &Path) -> Result<Loaded, String> {
    let old_path = MidiIndex::path_beside(library);
    let old = old_path.exists().then(|| MidiIndex::open_read_only(&old_path)).transpose().map_err(|e| e.to_string())?;
    let store_path = MidiStore::path_beside(library);
    if store_path.exists() {
        match MidiStore::open_read_only(&store_path) {
            Ok(store) => {
                // Your corrections made in the C# app count until the Rust page takes over.
                let roles = old.as_ref().and_then(|o| o.your_roles().ok()).unwrap_or_default();
                let octaves = old.as_ref().and_then(|o| o.your_octaves().ok()).unwrap_or_default();
                let phrases = store.all(&roles, &octaves).map_err(|e| e.to_string())?;
                if !phrases.is_empty() {
                    let cat = background::on_pool(|| Catalogue::new(phrases));
                    return Ok(Loaded { cat, old: false, notes: NotesFrom::Store(store_path) });
                }
            }
            Err(e) => log::warning("MIDI", &format!("The Rust MIDI index couldn't be opened: {e}")),
        }
    }
    let Some(old) = old else {
        return Ok(Loaded { cat: Catalogue::default(), old: true, notes: NotesFrom::Old(old_path) });
    };
    let phrases = old.all().map_err(|e| e.to_string())?;
    let cat = background::on_pool(|| Catalogue::new(phrases));
    Ok(Loaded { cat, old: true, notes: NotesFrom::Old(old_path) })
}

/// The index has been read: the page shows it.
pub fn show(app: &AppWindow, loaded: Result<Loaded, String>) {
    let data = app.global::<MidiData>();
    let loaded = match loaded {
        Ok(l) => l,
        Err(e) => {
            log::warning("MIDI", &format!("The MIDI index couldn't be read: {e}"));
            data.set_summary("".into());
            data.set_empty_title("The MIDI couldn't be read".into());
            data.set_empty_text(format!("The index said: {e}").into());
            return;
        }
    };
    let summary = page::summary(&loaded.cat, loaded.old);
    let empty = loaded.cat.entries.is_empty();
    log::info("MIDI", &format!("{summary} ({})", if loaded.old { "the C# app's midi.db" } else { "midi-index.db" }));
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.years = page::year_counts(&loaded.cat);
        p.cat = Arc::new(loaded.cat);
        p.old = loaded.old;
        p.notes = Some(loaded.notes);
        p.picked = None;
        p.target = None;
    });
    data.set_summary(summary.into());
    if empty {
        data.set_empty_title("No MIDI here yet".into());
        data.set_empty_text(
            "Once the MIDI in your projects and MIDI folders has been read, every part shows up here. Reading it in \
             this app comes next; until then this page shows what the old app read."
                .into(),
        );
    } else {
        data.set_empty_title("".into());
        data.set_empty_text("".into());
    }
    show_kinds(app);
    show_years(app);
    relist(app);
}

fn show_kinds(app: &AppWindow) {
    let (chips, any) = PAGE.with(|p| {
        let p = p.borrow();
        let counts = p.cat.role_counts();
        let chips: Vec<KindChip> = Role::ALL
            .iter()
            .map(|r| {
                let n = counts.get(r).copied().unwrap_or(0);
                let text = if n > 0 { format!("{} {}", r.plural(), thousands(n)) } else { r.plural().to_owned() };
                KindChip { text: text.into(), on: p.filters.roles.contains(r) }
            })
            .collect();
        (chips, !p.filters.roles.is_empty())
    });
    let data = app.global::<MidiData>();
    data.set_kinds(ModelRc::new(VecModel::from(chips)));
    data.set_any_kind(any);
}

fn show_years(app: &AppWindow) {
    let bars = PAGE.with(|p| {
        let p = p.borrow();
        let most = p.years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        p.years
            .iter()
            .map(|&(year, n)| YearBar {
                year: year.to_string().into(),
                share: n as f32 / most,
                picked: p.filters.years.is_some_and(|(a, b)| (a..=b).contains(&year)),
            })
            .collect::<Vec<_>>()
    });
    app.global::<MidiData>().set_years(ModelRc::new(VecModel::from(bars)));
}

/// Works the list out again for the filters now: off the window's thread, newest ask first.
fn relist(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.asked += 1;
        Ask {
            id: p.asked,
            cat: p.cat.clone(),
            filters: p.filters.clone(),
            order: p.order,
            target: p.target,
            seed: p.seed,
        }
    });
    app.global::<MidiData>().set_any_filter(PAGE.with(|p| p.borrow().filters.any()));
    if HERE.with(|h| *h.borrow()) {
        let hits = page::view(&ask.cat, &ask.filters, ask.order, ask.target, ask.seed);
        listed(app, ask.id, hits);
        return;
    }
    WORKER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("MIDI list", move |ask: Ask| {
                let hits = background::on_pool(|| page::view(&ask.cat, &ask.filters, ask.order, ask.target, ask.seed));
                let weak = weak.clone();
                let id = ask.id;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        listed(&app, id, hits);
                    }
                });
            }) {
                Ok(n) => *w = Some(n),
                Err(e) => log::error("MIDI", &format!("The list's worker couldn't start: {e}")),
            }
        }
        if let Some(w) = w.as_ref() {
            w.ask(ask);
        }
    });
}

fn listed(app: &AppWindow, id: u64, hits: Vec<Hit>) {
    let (shown, all, result, target) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.asked != id {
            return (None, 0, String::new(), String::new());
        }
        p.shown = Arc::new(hits);
        let target = p.target.and_then(|t| {
            let at = match t {
                Target::Like(at) | Target::Within(at) => at,
            };
            p.cat.entry(at).map(|e| (t, page::title(e)))
        });
        let target_text = match &target {
            Some((Target::Like(_), title)) => format!("Parts shaped like {title}"),
            Some((Target::Within(_), title)) => format!("Longer phrases with {title} in them"),
            None => String::new(),
        };
        let shown = p.shown.len();
        let all = p.cat.entries.len();
        (Some(shown), all, page::result_text(shown, all, p.order, p.target, p.seed.is_some()), target_text)
    });
    let Some(shown) = shown else { return };
    LINES.with(|l| l.notify.reset());
    let data = app.global::<MidiData>();
    data.set_shown(shown as i32);
    data.set_result(result.into());
    data.set_target(target.into());
    let _ = all;
}

fn select_row(app: &AppWindow, row: usize) {
    let at = PAGE.with(|p| p.borrow().shown.get(row).map(|h| h.entry));
    if let Some(at) = at {
        pick(app, Some(at));
    }
}

fn step(app: &AppWindow, by: i32) {
    let next = PAGE.with(|p| {
        let p = p.borrow();
        if p.shown.is_empty() {
            return None;
        }
        let row = p.picked.and_then(|at| p.row_of(at));
        let next = match row {
            None => 0,
            Some(r) => (r as i64 + i64::from(by)).clamp(0, p.shown.len() as i64 - 1) as usize,
        };
        Some(next)
    });
    if let Some(row) = next {
        select_row(app, row);
        let data = app.global::<MidiData>();
        data.set_show_row(row as i32);
        data.set_show_ask(data.get_show_ask() + 1);
    }
}

/// Picks a part (or none): its card, then its notes for the roll and its key.
fn pick(app: &AppWindow, at: Option<usize>) {
    let before = PAGE.with(|p| std::mem::replace(&mut p.borrow_mut().picked, at));
    // Only the lines that changed are drawn again.
    PAGE.with(|p| {
        let p = p.borrow();
        for r in [before, at].into_iter().flatten().filter_map(|a| p.row_of(a)) {
            LINES.with(|l| l.notify.row_changed(r));
        }
    });
    let data = app.global::<MidiData>();
    let Some(at) = at else {
        data.set_panel_shown(false);
        return;
    };
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.notes_asked += 1;
        let id = p.notes_asked;
        let notes = p.notes.clone()?;
        let old = p.old;
        let shortcuts = p.shortcuts.clone();
        let e = p.cat.entry(at)?;
        let card = card(e, old);
        let facts: Vec<MidiFact> =
            page::facts(e).into_iter().map(|(n, v)| MidiFact { name: n.into(), value: v.into() }).collect();
        let (actions, menu) = actions(e, &shortcuts);
        data.set_picked(card);
        data.set_facts(ModelRc::new(VecModel::from(facts)));
        data.set_actions(ModelRc::new(VecModel::from(actions)));
        data.set_menu(ModelRc::new(VecModel::from(menu)));
        data.set_roll(ModelRc::new(VecModel::from(Vec::<RollNote>::new())));
        data.set_roll_bars(ModelRc::new(VecModel::from(Vec::<f32>::new())));
        Some(NotesAsk {
            id,
            from: notes,
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            role: e.role(),
            cut: page::is_cut(&e.phrase.clip),
            bars: e.phrase.bars,
            read_key: e.phrase.key_fact.is_none(),
        })
    });
    data.set_panel_shown(true);
    let Some(ask) = ask else { return };
    if HERE.with(|h| *h.borrow()) {
        let id = ask.id;
        let read = read_notes(&ask, &mut None);
        notes_read(app, id, read);
        return;
    }
    NOTES.with(|n| {
        let mut n = n.borrow_mut();
        if n.is_none() {
            let weak = app.as_weak();
            // The connection stays open from one part to the next.
            let mut open: Option<(PathBuf, Reader)> = None;
            match Newest::start("MIDI notes", move |ask: NotesAsk| {
                let id = ask.id;
                let read = read_notes(&ask, &mut open);
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        notes_read(&app, id, read);
                    }
                });
            }) {
                Ok(w) => *n = Some(w),
                Err(e) => log::error("MIDI", &format!("The notes reader couldn't start: {e}")),
            }
        }
        if let Some(n) = n.as_ref() {
            n.ask(ask);
        }
    });
}

enum Reader {
    Store(MidiStore),
    Old(MidiIndex),
}

fn read_notes(ask: &NotesAsk, open: &mut Option<(PathBuf, Reader)>) -> Result<NotesRead, String> {
    let notes = match &ask.from {
        NotesFrom::Made(made) => made.get(&ask.hash).cloned().unwrap_or_default(),
        NotesFrom::Store(path) | NotesFrom::Old(path) => {
            if open.as_ref().is_none_or(|(p, _)| p != path) {
                let reader = match &ask.from {
                    NotesFrom::Store(_) => Reader::Store(MidiStore::open_read_only(path).map_err(|e| e.to_string())?),
                    _ => Reader::Old(MidiIndex::open_read_only(path).map_err(|e| e.to_string())?),
                };
                *open = Some((path.clone(), reader));
            }
            match open.as_ref().map(|(_, r)| r) {
                Some(Reader::Store(s)) => s.notes(ask.phrase_id, &ask.hash).map_err(|e| e.to_string())?,
                Some(Reader::Old(o)) => o.notes(ask.phrase_id, Some(&ask.hash)).map_err(|e| e.to_string())?,
                None => Vec::new(),
            }
        }
    };
    let fact = if ask.read_key {
        mo_midi::keys::with_reader(|r| mo_midi::keys::read(r, &notes, ask.role, ask.cut)).1
    } else {
        None
    };
    Ok(NotesRead { roll: page::roll(&notes, ask.bars), fact })
}

fn notes_read(app: &AppWindow, id: u64, read: Result<NotesRead, String>) {
    if PAGE.with(|p| p.borrow().notes_asked) != id {
        return;
    }
    let data = app.global::<MidiData>();
    let mut picked = data.get_picked();
    match read {
        Ok(read) => {
            let roll = &read.roll;
            let notes: Vec<RollNote> =
                roll.notes.iter().map(|n| RollNote { x: n.x, y: n.y, w: n.w, h: n.h, strength: n.strength }).collect();
            if notes.is_empty() {
                picked.roll_note = "Its notes aren't in the index any more.".into();
            } else {
                picked.roll_low = page::note_name(roll.low).into();
                picked.roll_high = page::note_name(roll.high).into();
                picked.roll_note = if roll.left_off > 0 {
                    format!(
                        "The first {} notes shown, of {}",
                        thousands(notes.len()),
                        thousands(notes.len() + roll.left_off)
                    )
                    .into()
                } else {
                    "".into()
                };
            }
            data.set_roll(ModelRc::new(VecModel::from(notes)));
            data.set_roll_bars(ModelRc::new(VecModel::from(roll.bars.clone())));
            if let Some(fact) = read.fact {
                let drums = picked.kind == page::role_label(Role::Drums);
                let words = page::key_card(Some(&fact), drums);
                picked.key = words.answer.into();
                picked.key_sure = words.sure.into();
                picked.key_why = words.why.into();
                picked.key_from = "Read just now from its notes by the new key finder.".into();
            }
        }
        Err(e) => {
            log::warning("MIDI", &format!("A part's notes couldn't be read: {e}"));
            picked.roll_note = format!("Its notes couldn't be read: {e}").into();
        }
    }
    data.set_picked(picked);
}

/// The card for a part, before its notes are read (the roll and, without a stored reading, the key follow).
fn card(e: &mo_midi::Entry, old: bool) -> PickedPart {
    let p = &e.phrase;
    let words = page::key_card(p.key_fact.as_ref(), p.drums || e.role() == Role::Drums);
    let (key, sure, why, from) = if p.key_fact.is_some() || p.drums || e.role() == Role::Drums {
        (words.answer, words.sure, words.why, if p.key_fact.is_some() { "From the new key finder." } else { "" })
    } else {
        ("Reading…".to_owned(), String::new(), String::new(), "")
    };
    let old_key = match (old, p.key) {
        (true, Some(k)) => format!("The old app said {} ({:.0}% sure).", k.name(), k.confidence * 100.0),
        (true, None) => "The old app found no key.".to_owned(),
        _ => String::new(),
    };
    let (others, more) = page::other_files(e, 6);
    PickedPart {
        kind: page::role_label(e.role()).into(),
        kind_why: why_text(p).into(),
        colour: colour(e.role()),
        title: page::title(e).into(),
        from: page::from_text(e).into(),
        path: mo_app::shell::wrappable_path(&p.source.path).into(),
        key: key.into(),
        key_sure: sure.into(),
        key_why: why.into(),
        key_from: from.into(),
        old_key: old_key.into(),
        instrument: page::instrument_text(e).into(),
        phrase: page::phrase_text(e).into(),
        roll_low: "".into(),
        roll_high: "".into(),
        roll_note: "".into(),
        others: strings(others),
        others_more: if more > 0 { format!("and {} more", thousands(more)).into() } else { "".into() },
    }
}

/// The card's buttons and the line's right-click menu: one list (rule 15: each shows its key).
fn actions(e: &mo_midi::Entry, shortcuts: &Shortcuts) -> (Vec<CardAction>, Vec<MenuItem>) {
    let mut list: Vec<(&str, &str, bool)> = vec![("midi.moreLike", "More like this", true)];
    if !e.within.is_empty() {
        list.push(("midi.within", "Show the whole phrase", true));
    }
    list.push(("midi.source", "Show in folder", true));
    list.push(("midi.copyPath", "Copy path", true));
    let card = list
        .iter()
        .map(|&(id, text, enabled)| CardAction {
            text: text.into(),
            tip: what_of(id).into(),
            key: shortcuts.key_of(id).into(),
            id: id.into(),
            enabled,
        })
        .collect();
    let menu = list
        .iter()
        .map(|&(id, text, enabled)| MenuItem {
            what: text.into(),
            key: shortcuts.key_of(id).into(),
            id: id.into(),
            enabled,
        })
        .collect();
    (card, menu)
}

/// Runs one of the page's actions on the picked part; false when there's nothing for it to do.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let (picked, feed) = PAGE.with(|p| {
        let p = p.borrow();
        (p.picked.and_then(|at| p.cat.entry(at).map(|e| (at, e.phrase.source.path.clone()))), p.feed.clone())
    });
    match (id, picked) {
        ("midi.back", _) => {
            let had = PAGE.with(|p| p.borrow_mut().target.take().is_some());
            if had {
                relist(app);
            }
            had
        }
        ("midi.moreLike", Some((at, _))) => {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.target = Some(Target::Like(at));
                p.seed = None;
            });
            relist(app);
            true
        }
        ("midi.within", Some((at, _))) => {
            PAGE.with(|p| p.borrow_mut().target = Some(Target::Within(at)));
            relist(app);
            true
        }
        ("midi.source", Some((_, path))) => {
            if let Some(feed) = feed {
                crate::files::reveal(&feed, &path, mo_app::shell::folder_of(&path));
            }
            true
        }
        ("midi.copyPath", Some((_, path))) => {
            if let Some(feed) = feed {
                crate::files::copy_path(&feed, &path);
            }
            true
        }
        _ => false,
    }
}

/// Whether a part is picked, so the page's keys have something to act on.
pub fn has_pick() -> bool {
    PAGE.with(|p| p.borrow().picked.is_some())
}

/// For the screenshots: everything is worked out in place.
pub fn work_here() {
    HERE.with(|h| *h.borrow_mut() = true);
}

/// For the screenshots: picks the line at `row` of the list on show.
pub fn pick_row(app: &AppWindow, row: usize) {
    select_row(app, row);
}

/// Made-up parts for the screenshots, with their notes: a few songs' parts and a pack's.
pub fn sample() -> Loaded {
    use mo_midi::{Key, Source, SourceKind};
    let now = 1_762_100_000;
    let day = 86_400;
    let source = |path: &str, mtime: i64| {
        let file = path.ends_with(".mid");
        Arc::new(Source {
            path: path.to_owned(),
            kind: if file { SourceKind::File } else { SourceKind::Project },
            daw: if file {
                "MIDI".into()
            } else if path.ends_with(".flp") {
                "FL Studio".into()
            } else {
                "Bitwig Studio".into()
            },
            mtime,
            size: 2000,
            tempo: None,
        })
    };
    let mut phrases = Vec::new();
    let mut notes = HashMap::new();
    // Path, part, clip, kind, key (tonic, minor), bars, days old, keys played.
    type Made<'a> = (&'a str, &'a str, &'a str, Role, u8, bool, i32, i64, &'a [u8]);
    let parts: [Made; 9] = [
        (
            "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 12.flp",
            "Keys",
            "Pattern 3",
            Role::Chords,
            9,
            true,
            8,
            0,
            &[57, 60, 64, 53, 57, 60, 48, 52, 55, 55, 59, 62],
        ),
        (
            "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 12.flp",
            "Sub",
            "Pattern 4",
            Role::Bass,
            9,
            true,
            8,
            0,
            &[33, 33, 29, 29, 24, 24, 31, 31],
        ),
        (
            "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 12.flp",
            "Lead",
            "Pattern 7 · bars 9–16",
            Role::Melody,
            9,
            true,
            8,
            0,
            &[69, 72, 76, 74, 72, 71, 69, 67, 69],
        ),
        (
            "K:\\Bitwig Studio\\Projects\\Glass\\Glass.bwproject",
            "Arp",
            "Arp clip",
            Role::Arp,
            2,
            false,
            4,
            2,
            &[62, 66, 69, 74, 69, 66],
        ),
        (
            "L:\\New Midi\\Dark Keys\\Dark keys 01 Cm.mid",
            "Piano",
            "Dark keys 01 Cm",
            Role::Chords,
            0,
            true,
            8,
            5,
            &[60, 63, 67, 56, 60, 63, 55, 58, 62],
        ),
        (
            "L:\\New Midi\\Dark Keys\\Dark keys 02 Fm.mid",
            "Piano",
            "Dark keys 02 Fm",
            Role::Chords,
            5,
            true,
            8,
            5,
            &[53, 56, 60, 49, 53, 56],
        ),
        (
            "L:\\New Midi\\Trap Bass\\808 slide 03.mid",
            "Bass",
            "808 slide 03",
            Role::Bass,
            7,
            false,
            4,
            40,
            &[31, 31, 34, 29],
        ),
        (
            "L:\\New Midi\\Drums\\Boom bap 90.mid",
            "Drums",
            "Boom bap 90",
            Role::Drums,
            0,
            false,
            2,
            60,
            &[36, 42, 38, 42, 36, 36, 38, 42],
        ),
        (
            "L:\\New Midi\\Melodies\\Flute hook A.mid",
            "Flute",
            "Flute hook A",
            Role::Melody,
            9,
            true,
            4,
            400,
            &[69, 72, 74, 76, 74, 72],
        ),
    ];
    for (i, (path, part, clip, role, tonic, minor, bars, age, keys)) in parts.into_iter().enumerate() {
        let hash = format!("{i:016X}");
        let step = (i64::from(bars) * mo_midi::BAR) / keys.len().max(1) as i64;
        let made: Vec<NoteEvent> = keys
            .iter()
            .enumerate()
            .map(|(j, &key)| NoteEvent {
                start: j as i64 * step,
                length: step * 9 / 10,
                key,
                velocity: 30_000 + (j as u16 % 4) * 8_000,
                muted: false,
                step: false,
                attribute: mo_formats::NoteAttribute::NONE,
            })
            .collect();
        notes.insert(hash.clone(), made);
        phrases.push(StoredPhrase {
            id: i as i64,
            source: source(path, now - age * day),
            part: part.to_owned(),
            clip: clip.to_owned(),
            role,
            role_why: format!("Its notes play like {}", page::role_label(role).to_lowercase()),
            your_role: None,
            bars,
            notes: keys.len() as i32,
            lowest: keys.iter().copied().min().unwrap_or(60),
            highest: keys.iter().copied().max().unwrap_or(60),
            key: (role != Role::Drums).then_some(Key { tonic, minor, confidence: 0.6 }),
            key_fact: None,
            chroma: [0; 12],
            chords: Vec::new(),
            features: (0..48).map(|j| ((j * 37 + i * 53) % 200 + 40) as u8).collect(),
            hash,
            members: String::new(),
            program: None,
            drums: role == Role::Drums,
            track_name: String::new(),
            your_octave: None,
        });
    }
    // An older save of the song with the same keys part: one line, "in 2 files".
    if let Some(first) = phrases.first().cloned() {
        phrases.push(StoredPhrase {
            source: source("I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 11.flp", now - 3 * day),
            ..first
        });
    }
    Loaded { cat: Catalogue::new(phrases), old: true, notes: NotesFrom::Made(Arc::new(notes)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_holds_every_kind_and_a_copy() {
        let s = sample();
        assert_eq!(s.cat.entries.len(), 9);
        assert_eq!(s.cat.phrase_count, 10);
        assert!(Role::ALL.iter().filter(|r| **r != Role::Other).all(|r| s.cat.role_counts().contains_key(r)));
    }

    #[test]
    fn notes_are_read_and_keyed_for_the_card() {
        let s = sample();
        let e = &s.cat.entries[0];
        let ask = NotesAsk {
            id: 1,
            from: s.notes.clone(),
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            role: e.role(),
            cut: false,
            bars: e.phrase.bars,
            read_key: true,
        };
        let Ok(read) = read_notes(&ask, &mut None) else { panic!("the made-up notes should read") };
        assert_eq!(read.roll.notes.len(), 12);
        assert!(read.fact.is_some());
    }

    #[test]
    fn a_missing_index_is_an_empty_list_not_an_error() {
        let dir = std::env::temp_dir().join("mo-ui-midi-test-empty");
        let _ = std::fs::create_dir_all(&dir);
        let Ok(l) = load(&dir.join("library.db")) else { panic!("no index should read as empty") };
        assert!(l.cat.entries.is_empty());
    }
}
