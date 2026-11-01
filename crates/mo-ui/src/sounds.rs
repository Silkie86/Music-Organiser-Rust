//! Fills the Sounds page from the sound library and plays the picked sound. The library (`sounds.db`, the C# app's,
//! read-only) is read once on a background thread; each change of the filters or order is worked out on its own
//! low-priority thread, newest ask first (rule 12), across every core (rule 21). Tiles and lines are only made when
//! the list draws them, so 357,000 sounds cost what's on screen. Sounds are read and played off the window's thread.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::log;
use mo_app::shortcuts::Shortcuts;
use mo_app::sounds::{self as list, Filters, Length, Order, SoundList};
use mo_audio::play::Player;
use mo_sounds::{Sound, SoundsDb, music};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer, TimerMode, VecModel};

use crate::{AppWindow, MenuItem, PickedSound, SoundFact, SoundLine, SoundTile, SoundsData, TileRow, TypeChipInfo};

/// The C# app's settings in `sounds.db` for the view shown last and autoplay, read (not written: it's read-only).
const VIEW_KEY: &str = "sounds_view";
const AUTOPLAY_KEY: &str = "sound_lab_autoplay";

/// The views, in the order of the page's view buttons.
const TILES: i32 = 0;
const HISTORY: i32 = 3;
const VIEWS: [&str; 4] = ["Tiles", "List", "Galaxy", "History"];

/// How many type chips show; the other types go in the More types menu.
const TYPE_CHIPS: usize = 9;

/// The Tempo chip's choices, and the tempos each lets through (lowest, highest).
const TEMPOS: [(&str, Option<f64>, Option<f64>); 5] = [
    ("Up to 90", None, Some(90.0)),
    ("90 to 110", Some(90.0), Some(110.0)),
    ("110 to 130", Some(110.0), Some(130.0)),
    ("130 to 150", Some(130.0), Some(150.0)),
    ("150 and up", Some(150.0), None),
];

/// The page's sounds, filters and what they let through, kept on the window's thread.
struct Page {
    list: Arc<SoundList>,
    filters: Filters,
    order: Order,
    /// Picks the shuffled order; a new one on each open and each Shuffle.
    seed: u64,
    view: i32,
    /// What the filters let through, in order, as places in `list`.
    shown: Vec<usize>,
    /// What History shows: the sounds played, newest first, as places in `list`, with when.
    history: Vec<(usize, NaiveDateTime)>,
    /// How many tiles fit side by side.
    columns: usize,
    /// The picked sound, as its place in `list`.
    picked: Option<usize>,
    autoplay: bool,
    /// Counts the asks for a new list, so an older answer that arrives late is dropped.
    asked: u64,
    /// The type chips' types, in chip order, then the rest (the More types menu).
    types: Vec<String>,
    now: NaiveDateTime,
    /// The sound playing (its place in `list`) and how long the part shown in its waveform lasts.
    playing: Option<(usize, f64)>,
}

impl Default for Page {
    fn default() -> Page {
        Page {
            list: Arc::default(),
            filters: Filters::default(),
            order: Order::default(),
            seed: new_seed(),
            view: TILES,
            shown: Vec::new(),
            history: Vec::new(),
            columns: 4,
            picked: None,
            autoplay: true,
            asked: 0,
            types: Vec::new(),
            now: Local::now().naive_local(),
            playing: None,
        }
    }
}

impl Page {
    /// The places in `list` of what the view on show lists, in order.
    fn entry(&self, row: usize) -> Option<usize> {
        if self.view == HISTORY { self.history.get(row).map(|h| h.0) } else { self.shown.get(row).copied() }
    }

    fn entries(&self) -> usize {
        if self.view == HISTORY { self.history.len() } else { self.shown.len() }
    }

    /// The row of the view on show that holds the sound at `at` in `list`.
    fn row_of(&self, at: usize) -> Option<usize> {
        if self.view == HISTORY {
            self.history.iter().position(|h| h.0 == at)
        } else {
            self.shown.iter().position(|&i| i == at)
        }
    }
}

/// An ask for a new list: everything the worker needs, so the page can go on changing meanwhile.
struct Ask {
    id: u64,
    list: Arc<SoundList>,
    filters: Filters,
    order: Order,
    seed: u64,
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    static TILE_ROWS: Rc<TileRows> = PAGE.with(|p| Rc::new(TileRows { page: p.clone(), notify: ModelNotify::default() }));
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
    static WORKER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
    /// Moves the picked tile's waveform along while it plays.
    static TICKER: Timer = Timer::default();
    /// Works out lists in place, for the screenshots, which have no event loop to send answers back to.
    static HERE: RefCell<bool> = const { RefCell::new(false) };
}

/// The one player, made the first time something plays (nothing opens before).
static PLAYER: OnceLock<Player> = OnceLock::new();

/// The tile rows, made from the page's list one row at a time as the list draws them.
struct TileRows {
    page: Rc<RefCell<Page>>,
    notify: ModelNotify,
}

impl Model for TileRows {
    type Data = TileRow;

    fn row_count(&self) -> usize {
        let p = self.page.borrow();
        if p.view != TILES { 0 } else { p.entries().div_ceil(p.columns.max(1)) }
    }

    fn row_data(&self, row: usize) -> Option<TileRow> {
        let p = self.page.borrow();
        let columns = p.columns.max(1);
        let first = row * columns;
        if first >= p.entries() {
            return None;
        }
        let tiles: Vec<SoundTile> =
            (first..(first + columns).min(p.entries())).filter_map(|r| p.entry(r).map(|at| tile(&p, r, at))).collect();
        Some(TileRow { tiles: ModelRc::new(VecModel::from(tiles)) })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The List and History lines, made one at a time as the list draws them.
struct Lines {
    page: Rc<RefCell<Page>>,
    notify: ModelNotify,
}

impl Model for Lines {
    type Data = SoundLine;

    fn row_count(&self) -> usize {
        let p = self.page.borrow();
        if p.view == TILES { 0 } else { p.entries() }
    }

    fn row_data(&self, row: usize) -> Option<SoundLine> {
        let p = self.page.borrow();
        let at = p.entry(row)?;
        Some(line(&p, row, at))
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

fn tile(p: &Page, row: usize, at: usize) -> SoundTile {
    let Some(s) = p.list.sound(at) else { return SoundTile::default() };
    SoundTile {
        row: row as i32,
        name: s.name().into(),
        facts: list::facts_line(s).into(),
        colour: list::type_colour(&s.sound_type),
        favourite: s.favourite,
        liked: p.list.liked_share(at).map_or(-1.0, |l| l as f32),
        wave: wave(s).into(),
        points: points(s),
        picked: p.picked == Some(at),
        playing: p.playing.is_some_and(|(i, _)| i == at),
    }
}

fn line(p: &Page, row: usize, at: usize) -> SoundLine {
    let Some(s) = p.list.sound(at) else { return SoundLine::default() };
    let a = s.analysis.as_ref();
    SoundLine {
        row: row as i32,
        name: s.name().into(),
        sound_type: s.sound_type.clone().into(),
        colour: list::type_colour(&s.sound_type),
        key: list::key_text(s).into(),
        bpm: a.and_then(|a| a.bpm).map(list::bpm_text).unwrap_or_default().into(),
        length: a.map(|a| list::length_text(a.seconds)).unwrap_or_default().into(),
        used: list::used_text(p.list.used_in(at)).into(),
        folder: s.folder().into(),
        when: p.history.get(row).filter(|_| p.view == HISTORY).map(|h| when(h.1, p.now)).unwrap_or_default().into(),
        favourite: s.favourite,
        picked: p.picked == Some(at),
        playing: p.playing.is_some_and(|(i, _)| i == at),
    }
}

/// When a sound was played, short: "14:05" today, "Tue" this week, "3 Oct" before.
fn when(at: NaiveDateTime, now: NaiveDateTime) -> String {
    let days = (now.date() - at.date()).num_days();
    if days == 0 {
        at.format("%H:%M").to_string()
    } else if (1..7).contains(&days) {
        at.format("%a").to_string()
    } else {
        at.format("%-d %b").to_string()
    }
}

/// A sound's waveform for the screens: every stored peak, one thin line each.
fn wave(s: &Sound) -> String {
    list::wave_path(s.analysis.as_ref().map(|a| a.waveform.as_slice()).unwrap_or_default())
}

fn points(s: &Sound) -> i32 {
    s.analysis.as_ref().map_or(0, |a| a.waveform.len() as i32)
}

fn new_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(|s| SharedString::from(s.as_ref())).collect::<Vec<_>>()))
}

/// Every key the Key chip offers: the twelve major keys, then the twelve minor ones.
fn key_choices() -> Vec<String> {
    [false, true].iter().flat_map(|&minor| (0..12).map(move |t| music::key_name(t, minor, false))).collect()
}

/// For the screenshots: work lists out on the window's thread, as there's no event loop to send them back with.
pub fn compute_here() {
    HERE.with(|h| *h.borrow_mut() = true);
}

/// Wires the page and starts reading the sound library at `sounds_db` in the background. Show in folder and Copy path
/// report on `feed`'s cards.
pub fn connect(app: &AppWindow, sounds_db: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    wire(app, feed, shortcuts);
    let data = app.global::<SoundsData>();
    data.set_busy("Reading your sounds".into());
    if HERE.with(|h| *h.borrow()) {
        loaded(app, read_library(&sounds_db));
        return;
    }
    let weak = app.as_weak();
    let started = background::spawn("Sounds library", move || {
        let read = read_library(&sounds_db);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                loaded(&app, read);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start reading the library: {e}"));
    }
}

/// What reading the library gives: the sounds, what was played, and the view and autoplay you had last.
struct Read {
    sounds: Vec<Sound>,
    history: Vec<(i64, NaiveDateTime)>,
    view: Option<String>,
    autoplay: Option<String>,
}

fn read_library(path: &Path) -> Result<Read, String> {
    if !path.exists() {
        return Err(String::new());
    }
    let db = SoundsDb::open_read_only(path).map_err(|e| e.to_string())?;
    let now = Local::now().naive_local();
    let sounds = db.all(now).map_err(|e| e.to_string())?;
    let history = db.history(500).unwrap_or_default();
    let view = db.setting(VIEW_KEY).ok().flatten();
    let autoplay = db.setting(AUTOPLAY_KEY).ok().flatten();
    Ok(Read { sounds, history, view, autoplay })
}

fn loaded(app: &AppWindow, read: Result<Read, String>) {
    let data = app.global::<SoundsData>();
    data.set_busy("".into());
    let read = match read {
        Ok(read) => read,
        Err(why) => {
            if !why.is_empty() {
                log::warning("Sounds", &format!("Couldn't read the sound library: {why}"));
            }
            data.set_empty_title("No sound library yet".into());
            data.set_empty_text(
                if why.is_empty() {
                    "The C# app's Sound Lab makes it when it scans your sample folders. This page reads it.".to_string()
                } else {
                    format!("The sound library couldn't be read: {why}")
                }
                .into(),
            );
            return;
        }
    };
    show(app, SoundList::new(read.sounds), read.history, read.view.as_deref(), read.autoplay.as_deref());
}

/// Shows `sounds` on the page, with what was `played` (id, when), opening on the view and autoplay setting given.
pub fn show(
    app: &AppWindow,
    sounds: SoundList,
    played: Vec<(i64, NaiveDateTime)>,
    view: Option<&str>,
    autoplay: Option<&str>,
) {
    let data = app.global::<SoundsData>();
    data.set_empty_title("".into());
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let ids: std::collections::HashMap<i64, usize> =
            (0..sounds.len()).filter_map(|i| sounds.sound(i).map(|s| (s.id, i))).collect();
        p.history = played.into_iter().filter_map(|(id, at)| ids.get(&id).map(|&i| (i, at))).collect();
        p.list = Arc::new(sounds);
        p.view = view.and_then(|v| VIEWS.iter().position(|n| *n == v)).map_or(TILES, |i| i as i32);
        if let Some(a) = autoplay {
            p.autoplay = a != "0" && !a.eq_ignore_ascii_case("false");
        }
        p.types = p.list.type_counts().into_iter().map(|(t, _)| t).collect();
        p.picked = None;
        data.set_tags(strings(p.list.tags()));
    });
    refresh(app);
}

fn wire(app: &AppWindow, feed: Feed, shortcuts: &Shortcuts) {
    FEED.with(|f| *f.borrow_mut() = Some(feed));
    let data = app.global::<SoundsData>();
    data.set_tile_rows(ModelRc::from(TILE_ROWS.with(|m| m.clone()) as Rc<dyn Model<Data = TileRow>>));
    data.set_lines(ModelRc::from(LINES.with(|m| m.clone()) as Rc<dyn Model<Data = SoundLine>>));
    data.set_views(strings(VIEWS));
    data.set_orders(strings(Order::ALL.map(Order::name)));
    data.set_lengths(strings([Length::Hits.name(), Length::Loops.name()]));
    data.set_keys(strings(key_choices()));
    data.set_bpms(strings(TEMPOS.map(|t| t.0)));
    let key = |id: &str| shortcuts.key_of(id);
    let item =
        |what: &str, key: &str, id: &str| MenuItem { what: what.into(), key: key.into(), id: id.into(), enabled: true };
    data.set_menu(ModelRc::new(VecModel::from(vec![
        item("Play or stop", key("app.play"), "play"),
        item("", "", ""),
        item("Show in folder", key("sounds.reveal"), "sounds.reveal"),
        item("Copy path", key("sounds.copyPath"), "sounds.copyPath"),
    ])));
    data.set_play_tip("Play or stop the picked sound".into());

    let weak = app.as_weak();
    data.on_search_edited(move |text| change(&weak, |f| f.search = text.to_string()));
    let weak = app.as_weak();
    data.on_pick_type(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let ty = PAGE.with(|p| p.borrow().types.get(i as usize).cloned());
        change(&app.as_weak(), |f| f.sound_type = if f.sound_type == ty { None } else { ty });
    });
    let weak = app.as_weak();
    data.on_pick(move |kind, value| {
        let value = value.to_string();
        change(&weak, |f| match kind.as_str() {
            "length" => {
                f.length = Length::ALL.into_iter().find(|l| l.name() == value).unwrap_or_default();
            }
            "key" => f.key = music::parse_key(&value).filter(|_| !value.is_empty()),
            "bpm" => {
                let t = TEMPOS.iter().find(|t| t.0 == value);
                f.bpm_from = t.and_then(|t| t.1);
                f.bpm_to = t.and_then(|t| t.2);
            }
            "tag" => f.tag = (!value.is_empty()).then_some(value),
            "type" => f.sound_type = (!value.is_empty()).then_some(value),
            _ => {}
        });
    });
    let weak = app.as_weak();
    data.on_toggle(move |kind| {
        change(&weak, |f| match kind.as_str() {
            "mix-well" => f.mix_well = !f.mix_well,
            "unused" => {
                f.unused = !f.unused;
                f.used &= !f.unused;
            }
            "used" => {
                f.used = !f.used;
                f.unused &= !f.used;
            }
            "favourites" => f.favourites = !f.favourites,
            _ => {}
        });
    });
    let weak = app.as_weak();
    data.on_clear(move || change(&weak, |f| *f = Filters { search: f.search.clone(), ..Filters::default() }));
    let weak = app.as_weak();
    data.on_pick_order(move |name| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().order = Order::from_name(&name));
        refresh(&app);
    });
    let weak = app.as_weak();
    data.on_shuffle(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.seed = new_seed();
            p.order = Order::Shuffled;
        });
        refresh(&app);
    });
    let weak = app.as_weak();
    data.on_toggle_autoplay(move || {
        let Some(app) = weak.upgrade() else { return };
        let on = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.autoplay = !p.autoplay;
            p.autoplay
        });
        app.global::<SoundsData>().set_autoplay(on);
    });
    let weak = app.as_weak();
    data.on_pick_view(move |v| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().view = v);
        show_lists(&app);
        bring_picked_into_view(&app);
    });
    let weak = app.as_weak();
    data.on_columns_changed(move |n| {
        let Some(app) = weak.upgrade() else { return };
        let n = n.max(1) as usize;
        let changed = PAGE.with(|p| std::mem::replace(&mut p.borrow_mut().columns, n) != n);
        if changed {
            TILE_ROWS.with(|m| m.notify.reset());
            bring_picked_into_view(&app);
        }
    });
    let weak = app.as_weak();
    data.on_select(move |row| {
        if let Some(app) = weak.upgrade() {
            select_row(&app, row, true);
        }
    });
    let weak = app.as_weak();
    data.on_play_row(move |row| {
        let Some(app) = weak.upgrade() else { return };
        select_row(&app, row, false);
        play_picked(&app, 0.0);
    });
    let weak = app.as_weak();
    data.on_step(move |d| {
        if let Some(app) = weak.upgrade() {
            step(&app, d);
        }
    });
    let weak = app.as_weak();
    data.on_play(move || {
        if let Some(app) = weak.upgrade() {
            toggle_play(&app);
        }
    });
    let weak = app.as_weak();
    data.on_seek(move |at| {
        let Some(app) = weak.upgrade() else { return };
        let (picked, playing) = PAGE.with(|p| {
            let p = p.borrow();
            (p.picked, p.playing)
        });
        match (picked, playing) {
            (Some(i), Some((j, seconds))) if i == j => {
                if let Some(player) = PLAYER.get() {
                    player.seek(f64::from(at) * seconds);
                }
            }
            (Some(_), _) => play_picked(&app, f64::from(at)),
            _ => {}
        }
    });
    let weak = app.as_weak();
    data.on_close_panel(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().picked = None);
        app.global::<SoundsData>().set_panel_shown(false);
        redraw();
    });
    let weak = app.as_weak();
    data.on_menu_chosen(move |id| {
        if let Some(app) = weak.upgrade() {
            if id == "play" {
                toggle_play(&app);
            } else {
                run_action(&app, &id);
            }
        }
    });
    data.on_panel_resized(|w| log::info("Sounds", &format!("Panel width {w:.0}")));
}

fn change(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut Filters)) {
    let Some(app) = weak.upgrade() else { return };
    PAGE.with(|p| f(&mut p.borrow_mut().filters));
    refresh(&app);
}

/// Asks for the list again with the filters and order as they are now, and shows the chips meanwhile.
fn refresh(app: &AppWindow) {
    show_chips(app);
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.asked += 1;
        Ask { id: p.asked, list: p.list.clone(), filters: p.filters.clone(), order: p.order, seed: p.seed }
    });
    if HERE.with(|h| *h.borrow()) {
        let shown = ask.list.view(&ask.filters, ask.order, ask.seed);
        listed(app, ask.id, shown);
        return;
    }
    WORKER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Sounds list", move |ask: Ask| {
                let shown = ask.list.view(&ask.filters, ask.order, ask.seed);
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        listed(&app, ask.id, shown);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error("Sounds", &format!("Couldn't start the list's thread: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

/// A new list has been worked out: shows it, unless a newer one has been asked for since.
fn listed(app: &AppWindow, id: u64, shown: Vec<usize>) {
    let current = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.asked != id {
            return false;
        }
        p.shown = shown;
        true
    });
    if current {
        show_lists(app);
    }
}

fn show_lists(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_view(p.view);
        data.set_shown(p.entries() as i32);
        let total = p.list.len();
        let summary = if p.list.is_empty() {
            String::new()
        } else if p.view != HISTORY && (p.filters.any() || !p.filters.search.trim().is_empty()) {
            format!("{} of {} sounds", thousands(p.shown.len()), thousands(total))
        } else {
            format!("{} sounds", thousands(total))
        };
        data.set_summary(summary.into());
    });
    redraw();
}

/// The tiles and lines again (a pick or what's playing changed what they show).
fn redraw() {
    TILE_ROWS.with(|m| m.notify.reset());
    LINES.with(|m| m.notify.reset());
}

fn show_chips(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let f = &p.filters;
        let counts = p.list.type_counts();
        let chips: Vec<TypeChipInfo> = counts
            .iter()
            .take(TYPE_CHIPS)
            .map(|(t, n)| TypeChipInfo {
                name: t.clone().into(),
                count: list::short_count(*n).into(),
                colour: list::type_colour(t),
                on: f.sound_type.as_deref() == Some(t),
            })
            .collect();
        data.set_types(ModelRc::new(VecModel::from(chips)));
        data.set_more_types(strings(counts.iter().skip(TYPE_CHIPS).map(|(t, _)| t.as_str())));
        let more = f.sound_type.as_deref().filter(|t| counts.iter().skip(TYPE_CHIPS).any(|(c, _)| c == t));
        data.set_more_type(more.unwrap_or_default().into());
        data.set_length(if f.length == Length::All { "" } else { f.length.name() }.into());
        data.set_key(f.key.map(|(t, m)| music::key_name(t, m, false)).unwrap_or_default().into());
        data.set_mix_well(f.mix_well);
        let tempo = TEMPOS.iter().find(|t| t.1 == f.bpm_from && t.2 == f.bpm_to && (t.1.is_some() || t.2.is_some()));
        data.set_bpm(tempo.map_or("", |t| t.0).into());
        data.set_tag(f.tag.clone().unwrap_or_default().into());
        data.set_unused(f.unused);
        data.set_used(f.used);
        data.set_knows_use(p.list.knows_use());
        data.set_favourites(f.favourites);
        data.set_any_filter(f.any());
        data.set_order(p.order.name().into());
        data.set_autoplay(p.autoplay);
    });
}

/// Picks the sound on `row` of the view on show, opens its panel, and plays it when autoplay is on and `autoplay` asks.
fn select_row(app: &AppWindow, row: i32, autoplay: bool) {
    let Some(at) = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().entry(r))) else { return };
    pick(app, at);
    let on = PAGE.with(|p| p.borrow().autoplay);
    if autoplay && on {
        play_picked(app, 0.0);
    }
}

fn pick(app: &AppWindow, at: usize) {
    let data = app.global::<SoundsData>();
    let picked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.picked = Some(at);
        picked_sound(&p, at)
    });
    data.set_picked(picked);
    data.set_panel_shown(true);
    data.set_playing(PAGE.with(|p| p.borrow().playing.is_some_and(|(i, _)| i == at)));
    redraw();
    bring_picked_into_view(app);
}

/// Scrolls the list so the picked sound shows, when it's out of sight.
fn bring_picked_into_view(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let Some(row) = PAGE.with(|p| {
        let p = p.borrow();
        let row = p.picked.and_then(|at| p.row_of(at))?;
        Some(if p.view == TILES { row / p.columns.max(1) } else { row })
    }) else {
        return;
    };
    data.set_show_row(row as i32);
    data.set_show_ask(data.get_show_ask().wrapping_add(1));
}

fn picked_sound(p: &Page, at: usize) -> PickedSound {
    let Some(s) = p.list.sound(at) else { return PickedSound::default() };
    let a = s.analysis.as_ref();
    let mut facts = vec![
        ("Key", list::key_text(s)),
        ("Tempo", a.and_then(|a| a.bpm).map(list::bpm_text).unwrap_or_default()),
        ("Length", a.map(|a| list::length_text(a.seconds)).unwrap_or_default()),
    ];
    if let Some(used) = p.list.used_in(at) {
        facts.push(("Used", list::used_text(Some(used))));
    }
    let facts: Vec<SoundFact> = facts
        .into_iter()
        .map(|(name, value)| SoundFact {
            name: name.into(),
            value: if value.is_empty() { "–".into() } else { value.into() },
        })
        .collect();
    let type_from = match s.type_source.as_str() {
        "you" => "· you set this",
        "name" => "· from its name",
        "learned" => "· learned from your library",
        "model" => "· guessed from how it sounds",
        _ => "",
    };
    PickedSound {
        name: s.name().into(),
        sound_type: s.sound_type.clone().into(),
        colour: list::type_colour(&s.sound_type),
        type_from: type_from.into(),
        path: s.path.clone().into(),
        wave: wave(s).into(),
        points: points(s),
        seconds: a.map_or(0.0, |a| a.seconds.min(mo_audio::MAX_SECONDS)) as f32,
        facts: ModelRc::new(VecModel::from(facts)),
        tags: strings(&s.tags),
        favourite: s.favourite,
        character: a.map(|a| a.character.join(", ")).unwrap_or_default().into(),
    }
}

/// The arrow keys: across a row of tiles (-1, 1) or down and up the list (-2, 2), a row of tiles at a time.
fn step(app: &AppWindow, d: i32) {
    let next = PAGE.with(|p| {
        let p = p.borrow();
        let n = p.entries();
        if n == 0 {
            return None;
        }
        let by = match (d, p.view) {
            (-1 | 1, TILES) => d as isize,
            (-2 | 2, TILES) => d.signum() as isize * p.columns.max(1) as isize,
            (-2 | 2, _) => d.signum() as isize,
            _ => return None,
        };
        let row = match p.picked.and_then(|at| p.row_of(at)) {
            Some(r) => (r as isize + by).clamp(0, n as isize - 1) as usize,
            None => 0,
        };
        Some(row)
    });
    if let Some(row) = next {
        select_row(app, row as i32, true);
    }
}

/// Space and the Play button: stops the picked sound if it's playing, else plays it.
pub fn toggle_play(app: &AppWindow) -> bool {
    let (picked, playing) = PAGE.with(|p| {
        let p = p.borrow();
        (p.picked, p.playing)
    });
    match (picked, playing) {
        (Some(i), Some((j, _))) if i == j => stop(app),
        (Some(_), _) => play_picked(app, 0.0),
        (None, Some(_)) => stop(app),
        (None, None) => return false,
    }
    true
}

/// Plays the picked sound from `from` (0 to 1 of its waveform). The file is read off the window's thread.
fn play_picked(app: &AppWindow, from: f64) {
    let Some((at, path, seconds)) = PAGE.with(|p| {
        let p = p.borrow();
        let at = p.picked?;
        let s = p.list.sound(at)?;
        let seconds = s.analysis.as_ref().map_or(0.0, |a| a.seconds).min(mo_audio::MAX_SECONDS);
        Some((at, s.path.clone(), seconds))
    }) else {
        return;
    };
    PAGE.with(|p| p.borrow_mut().playing = Some((at, seconds.max(0.001))));
    playing_changed(app);
    // The screenshots show a sound playing without a sound card.
    if HERE.with(|h| *h.borrow()) {
        return;
    }
    let weak = app.as_weak();
    let started =
        background::spawn("Read sound", move || match mo_audio::read(Path::new(&path), mo_audio::MAX_SECONDS) {
            Ok(audio) => {
                let player = PLAYER.get_or_init(|| {
                    Player::new(|key| {
                        let key = key.to_owned();
                        let _ = slint::invoke_from_event_loop(move || ended(&key));
                    })
                });
                player.play(Arc::new(audio), path, from * seconds);
            }
            Err(e) => {
                log::warning("Sounds", &format!("Couldn't play {path}: {e}"));
                let why = e.to_string();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(feed) = FEED.with(|f| f.borrow().clone()) {
                        feed.start("Couldn't play the sound", "").failed(&why);
                    }
                    if let Some(app) = weak.upgrade() {
                        stop(&app);
                    }
                });
            }
        });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start reading the sound: {e}"));
    }
}

fn stop(app: &AppWindow) {
    if let Some(player) = PLAYER.get() {
        player.stop();
    }
    PAGE.with(|p| p.borrow_mut().playing = None);
    playing_changed(app);
}

/// A sound played to its end by itself.
fn ended(key: &str) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let ours = p.playing.and_then(|(i, _)| p.list.sound(i)).is_some_and(|s| s.path == key);
        if ours {
            p.playing = None;
        }
    });
    TICKER.with(Timer::stop);
    redraw();
    // The panel's Play button and waveform follow on the next tick of `playing_changed`'s timer, which has stopped,
    // so they're put right here through the page's own window.
    WINDOW.with(|w| {
        if let Some(app) = w.borrow().as_ref().and_then(slint::Weak::upgrade) {
            let data = app.global::<SoundsData>();
            data.set_playing(false);
            data.set_played(1.0);
        }
    });
}

thread_local! {
    /// The window, for a sound ending by itself (the player's thread only knows the sound's key).
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Shows what's playing, and moves its waveform along while it plays.
fn playing_changed(app: &AppWindow) {
    WINDOW.with(|w| *w.borrow_mut() = Some(app.as_weak()));
    let data = app.global::<SoundsData>();
    let (picked_plays, any) = PAGE.with(|p| {
        let p = p.borrow();
        (p.playing.is_some() && p.playing.map(|(i, _)| i) == p.picked, p.playing.is_some())
    });
    data.set_playing(picked_plays);
    data.set_played(0.0);
    redraw();
    // Nothing really plays for the screenshots, so nothing moves the waveform along there.
    if !any || HERE.with(|h| *h.borrow()) {
        TICKER.with(Timer::stop);
        return;
    }
    let weak = app.as_weak();
    TICKER.with(|t| {
        t.start(TimerMode::Repeated, Duration::from_millis(40), move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(seconds) = PAGE.with(|p| p.borrow().playing.map(|(_, s)| s)) else { return };
            let at = PLAYER.get().and_then(Player::position).unwrap_or(0.0);
            app.global::<SoundsData>().set_played((at / seconds).clamp(0.0, 1.0) as f32);
        });
    });
}

/// Runs one of the page's own actions (from its key or its menu). Returns whether it did anything, so an unused key
/// goes on.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let picked = PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|at| p.list.sound(at)).map(|s| (s.path.clone(), mo_app::shell::folder_of(&s.path).to_owned()))
    });
    let feed = FEED.with(|f| f.borrow().clone());
    match (id, picked, feed) {
        ("sounds.stop", _, _) => {
            if PAGE.with(|p| p.borrow().playing.is_none()) {
                return false;
            }
            stop(app);
        }
        ("sounds.reveal", Some((path, folder)), Some(feed)) => crate::files::reveal(&feed, &path, &folder),
        ("sounds.copyPath", Some((path, _)), Some(feed)) => crate::files::copy_path(&feed, &path),
        _ => return false,
    }
    true
}

/// A count with thousands marked: "357,412".
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_times_read_well() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(357_412), "357,412");
        assert_eq!(thousands(1_000_000), "1,000,000");
        let now =
            chrono::NaiveDate::from_ymd_opt(2026, 11, 1).and_then(|d| d.and_hms_opt(12, 0, 0)).unwrap_or_default();
        assert_eq!(when(now - chrono::Duration::minutes(5), now), "11:55");
        assert_eq!(when(now - chrono::Duration::days(2), now), "Fri");
        assert_eq!(when(now - chrono::Duration::days(30), now), "2 Oct");
        assert_eq!(key_choices().len(), 24);
    }
}
