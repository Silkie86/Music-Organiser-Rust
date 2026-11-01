//! Fills the Sounds page from the sound library and plays the picked sound. The library (`sounds.db`, the C# app's,
//! read-only) is read once on a background thread; each change of the filters or order is worked out on its own
//! low-priority thread, newest ask first (rule 12), across every core (rule 21). Tiles and lines are only made when
//! the list draws them, so 357,000 sounds cost what's on screen. Sounds are read and played off the window's thread.
//! Rediscover (unused sounds that fit your taste) is dug out on its own low-priority thread too.

mod galaxy;
mod kit;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::log;
use mo_app::shortcuts::Shortcuts;
use mo_app::sounds::{self as list, Filters, Length, Order, SoundList};
use mo_audio::play::Player;
use mo_library::UsageRules;
use mo_library::songs::thousands;
use mo_sounds::index::SoundIndex;
use mo_sounds::liking::{self, LIKED_WEIGHTS_KEY, LikedWeights};
use mo_sounds::rediscover::{self, FolderUse};
use mo_sounds::usage::{self, Usage};
use mo_sounds::{Sound, SoundsDb, music};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer, TimerMode, VecModel};

use crate::{
    AlikeLine, AppWindow, FolderLine, MenuItem, PickedSound, SoundFact, SoundLine, SoundTile, SoundsData, TileRow,
    TypeChipInfo,
};

/// The C# app's settings in `library.db` for the view shown last and autoplay, read (not written: it's read-only).
const VIEW_KEY: &str = "sounds_view";
const AUTOPLAY_KEY: &str = "sound_lab_autoplay";
/// The C# app's Settings in `library.db` for which songs count as using a sound.
const USED_NEEDS_NOTES_KEY: &str = "used_in_needs_notes";
const USED_NOT_MUTED_KEY: &str = "used_in_not_muted_newest";
/// The C# app's setting in `library.db` for how many sounds Rediscover digs out.
const COUNT_KEY: &str = "rediscover_count";

/// The views, in the order of the page's view buttons.
const TILES: i32 = 0;
const LIST: i32 = 1;
const GALAXY: i32 = 2;
const HISTORY: i32 = 3;
const REDISCOVER: i32 = 4;
const KITS: i32 = 5;
const VIEWS: [&str; 6] = ["Tiles", "List", "Galaxy", "History", "Rediscover", "Kits"];
/// The C# app's export folder setting in `library.db` (kits go in its Kits folder).
const EXPORT_FOLDER_KEY: &str = "export_folder";

/// Rediscover's How many choices (the C# app's), and the one it starts on.
const COUNTS: [usize; 4] = [30, 60, 120, 250];
const COUNT: usize = 60;
/// How many forgotten folders Rediscover lists, as in the C# app.
const FOLDERS: usize = 40;

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
    shown: Arc<Vec<usize>>,
    /// What History shows: the sounds played, newest first, as places in `list`, with when.
    history: Vec<(usize, NaiveDateTime)>,
    /// How many tiles fit side by side.
    columns: usize,
    /// The picked sound, as its place in `list`.
    picked: Option<usize>,
    autoplay: bool,
    /// Counts the asks for a new list, so an older answer that arrives late is dropped.
    asked: u64,
    /// The type chips' types with their counts, in chip order, then the rest (the More types menu).
    types: Vec<(String, usize)>,
    now: NaiveDateTime,
    /// The sound playing (its place in `list`) and how long the part shown in its waveform lasts.
    playing: Option<(usize, f64)>,
    /// Each sound's place in `list`, by id.
    places: Arc<HashMap<i64, usize>>,
    /// Every sound's fingerprint, once read, for Sounds like this and Rediscover.
    index: Option<Arc<SoundIndex>>,
    /// What Rediscover dug out, as places in `list` with why, best first.
    rediscover: Vec<(usize, String)>,
    /// What Rediscover digs out next time.
    re: rediscover::Options,
    /// Counts Rediscover's asks, so an older answer that arrives late is dropped.
    re_asked: u64,
    /// Rediscover has run on this library (so changing a choice picks again).
    re_started: bool,
    /// The answer to Rediscover's newest ask hasn't come yet.
    re_busy: bool,
    /// The kit builder's pads and choices.
    kit: kit::Kit,
    /// The galaxy's map and the part of it on show.
    galaxy: galaxy::Galaxy,
}

impl Default for Page {
    fn default() -> Page {
        Page {
            list: Arc::default(),
            filters: Filters::default(),
            order: Order::default(),
            seed: new_seed(),
            view: TILES,
            shown: Arc::default(),
            history: Vec::new(),
            columns: 4,
            picked: None,
            autoplay: false,
            asked: 0,
            types: Vec::new(),
            now: Local::now().naive_local(),
            playing: None,
            places: Arc::default(),
            index: None,
            rediscover: Vec::new(),
            re: rediscover::Options { count: COUNT, ..rediscover::Options::default() },
            re_asked: 0,
            re_started: false,
            re_busy: false,
            kit: kit::Kit::default(),
            galaxy: galaxy::Galaxy::default(),
        }
    }
}

impl Page {
    /// The places in `list` of what the view on show lists, in order.
    fn entry(&self, row: usize) -> Option<usize> {
        match self.view {
            HISTORY => self.history.get(row).map(|h| h.0),
            REDISCOVER => self.rediscover.get(row).map(|r| r.0),
            KITS => None,
            _ => self.shown.get(row).copied(),
        }
    }

    fn entries(&self) -> usize {
        match self.view {
            HISTORY => self.history.len(),
            REDISCOVER => self.rediscover.len(),
            KITS => 0,
            _ => self.shown.len(),
        }
    }

    /// The row of the view on show that holds the sound at `at` in `list`.
    fn row_of(&self, at: usize) -> Option<usize> {
        match self.view {
            HISTORY => self.history.iter().position(|h| h.0 == at),
            REDISCOVER => self.rediscover.iter().position(|r| r.0 == at),
            KITS => None,
            _ => self.shown.iter().position(|&i| i == at),
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
    static ALIKE: RefCell<Option<Newest<AlikeAsk>>> = const { RefCell::new(None) };
    static REDISCOVERING: RefCell<Option<Newest<RediscoverAsk>>> = const { RefCell::new(None) };
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
    /// Moves the picked tile's waveform along while it plays.
    static TICKER: Timer = Timer::default();
    /// Works out lists in place, for the screenshots, which have no event loop to send answers back to.
    static HERE: RefCell<bool> = const { RefCell::new(false) };
}

/// The one player, made the first time something plays (nothing opens before).
static PLAYER: OnceLock<Player> = OnceLock::new();
/// Counts the plays and stops asked for, so a sound that finishes reading after a newer play or a stop stays quiet
/// (newest wins, rule 12).
static PLAYS: AtomicU64 = AtomicU64::new(0);

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
        liked_why: liked_why(p, at).into(),
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
        reason: p.rediscover.get(row).filter(|_| p.view == REDISCOVER).map(|r| r.1.clone()).unwrap_or_default().into(),
        favourite: s.favourite,
        picked: p.picked == Some(at),
        playing: p.playing.is_some_and(|(i, _)| i == at),
    }
}

/// Why a sound is as liked as it is (empty until the scores are known).
fn liked_why(p: &Page, at: usize) -> String {
    match (p.list.liked_share(at), p.list.sound(at)) {
        (Some(_), Some(s)) => liking::explain(s, p.list.usage(at)),
        _ => String::new(),
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

/// Wires the page and starts reading the sound library at `sounds_db` in the background, then how much your songs use
/// each sound from the song library at `library_db` (both read-only). The sounds show as soon as they're read; the use
/// counts and Most liked follow when they're worked out, as in the C# app. Show in folder and Copy path report on
/// `feed`'s cards.
pub fn connect(app: &AppWindow, sounds_db: PathBuf, library_db: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    wire(app, feed, shortcuts);
    PAGE.with(|p| p.borrow_mut().galaxy.db = Some(sounds_db.clone()));
    app.global::<SoundsData>().set_busy("Reading your sounds".into());
    let here = HERE.with(|h| *h.borrow());
    let weak = app.as_weak();
    let send = move |step: Step| {
        if here {
            if let Some(app) = weak.upgrade() {
                arrive(&app, step);
            }
            return;
        }
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                arrive(&app, step);
            }
        });
    };
    if here {
        read_all(&sounds_db, &library_db, send);
        return;
    }
    if let Err(e) = background::spawn("Sounds library", move || read_all(&sounds_db, &library_db, send)) {
        log::error("Sounds", &format!("Couldn't start reading the library: {e}"));
    }
}

/// What reading brings back, in two steps: the sounds first, then the list again with how your songs use them.
enum Step {
    Read(Result<Read, String>),
    Used(Arc<SoundList>),
    /// Every sound's fingerprint, for Sounds like this.
    Index(Arc<SoundIndex>),
}

fn arrive(app: &AppWindow, step: Step) {
    match step {
        Step::Read(read) => loaded(app, read),
        Step::Used(list) => use_known(app, list),
        Step::Index(index) => index_known(app, index),
    }
}

/// What reading the library gives: the sounds ready to list, what was played, and the view and autoplay you had last.
struct Read {
    library: Library,
    view: Option<String>,
    count: Option<String>,
    export_folder: Option<String>,
    autoplay: Option<String>,
}

/// The sounds made ready to list, off the window's thread: the list itself, the types with their counts, the tags,
/// and what was played (as places in the list, newest first, with when).
pub struct Library {
    list: Arc<SoundList>,
    types: Vec<(String, usize)>,
    tags: Vec<String>,
    played: Vec<(usize, NaiveDateTime)>,
    /// Each sound's place in the list, by its id.
    places: Arc<HashMap<i64, usize>>,
}

impl Library {
    /// Gets `sounds` ready to list, with what was `played` (id, when) and how much each sign of liking counts
    /// (`weights`; until the songs' use is known, Most liked counts plays, drags, sends and favourites). Takes a
    /// while for a big library, so it's done on a background thread (the screenshots do it in place).
    pub fn new(sounds: Vec<Sound>, played: Vec<(i64, NaiveDateTime)>, weights: &LikedWeights) -> Library {
        let list = background::on_pool(|| {
            let mut list = SoundList::new(sounds);
            list.set_liked(|s| weights.score(s, None));
            list
        });
        let places: HashMap<i64, usize> = (0..list.len()).filter_map(|i| list.sound(i).map(|s| (s.id, i))).collect();
        let played = played.into_iter().filter_map(|(id, at)| places.get(&id).map(|&i| (i, at))).collect();
        Library { types: list.type_counts(), tags: list.tags(), list: Arc::new(list), played, places: Arc::new(places) }
    }

    /// The same library with how your songs use each sound (by sound id) counted in, for the screenshots.
    pub fn with_usage(mut self, usage: HashMap<i64, Usage>, weights: &LikedWeights) -> Library {
        self.list = Arc::new(with_usage(&self.list, usage, weights));
        self
    }
}

/// A copy of `list` with each sound's use in your songs, and Most liked scored with it.
fn with_usage(list: &SoundList, usage: HashMap<i64, Usage>, weights: &LikedWeights) -> SoundList {
    background::on_pool(|| {
        let mut list = list.clone();
        list.set_liked(|s| weights.score(s, usage.get(&s.id)));
        list.set_used(usage);
        list
    })
}

/// The C# app's settings the page uses, from the song library (`library.db`): the view and autoplay you had last,
/// which songs count as using a sound, and the Most liked weights.
#[derive(Default)]
struct Settings {
    view: Option<String>,
    /// How many sounds Rediscover digs out.
    count: Option<String>,
    /// Where exports (kits) go.
    export_folder: Option<String>,
    autoplay: Option<String>,
    rules: UsageRules,
    weights: LikedWeights,
}

impl Settings {
    fn read(lib: &mo_library::Library) -> Settings {
        let get = |key: &str| match lib.setting(key) {
            Ok(v) => v,
            Err(e) => {
                log::warning(
                    "Sounds",
                    &format!("Couldn't read the setting {key}, so the C# app's default is used: {e}"),
                );
                None
            }
        };
        let on = |key: &str| get(key).as_deref() == Some("1");
        Settings {
            view: get(VIEW_KEY),
            count: get(COUNT_KEY),
            export_folder: get(EXPORT_FOLDER_KEY),
            autoplay: get(AUTOPLAY_KEY),
            rules: UsageRules { needs_notes: on(USED_NEEDS_NOTES_KEY), not_muted_in_newest: on(USED_NOT_MUTED_KEY) },
            weights: LikedWeights::from_json(get(LIKED_WEIGHTS_KEY).as_deref()),
        }
    }
}

/// Reads the sounds and sends them, then works out how your songs use them and sends that. Without a song library
/// the use stays unknown (the Used chips stay hidden).
fn read_all(sounds_db: &Path, library_db: &Path, send: impl Fn(Step)) {
    if !sounds_db.exists() {
        send(Step::Read(Err(String::new())));
        return;
    }
    let lib = library_db.exists().then(|| mo_library::Library::open_read_only(library_db)).and_then(|lib| {
        lib.map_err(|e| log::warning("Sounds", &format!("Couldn't read the song library for how sounds are used: {e}")))
            .ok()
    });
    let settings = lib.as_ref().map(Settings::read).unwrap_or_default();
    let now = Local::now().naive_local();
    let db = match SoundsDb::open_read_only(sounds_db) {
        Ok(db) => db,
        Err(e) => {
            send(Step::Read(Err(e.to_string())));
            return;
        }
    };
    let read = (|| {
        let sounds = db.all(now).map_err(|e| e.to_string())?;
        let history = db.history(500).unwrap_or_default();
        Ok(Library::new(sounds, history, &settings.weights))
    })();
    let list = read.as_ref().ok().map(|l: &Library| l.list.clone());
    send(Step::Read(read.map(|library| Read {
        library,
        view: settings.view.clone(),
        count: settings.count.clone(),
        export_folder: settings.export_folder.clone(),
        autoplay: settings.autoplay.clone(),
    })));
    let Some(list) = list else { return };
    // Sounds like this: every sound's fingerprint, read once.
    match SoundIndex::read(&db, list.sounds()) {
        Ok(index) => send(Step::Index(Arc::new(index))),
        Err(e) => log::warning("Sounds", &format!("Couldn't read the sounds' fingerprints for Sounds like this: {e}")),
    }
    drop(db);
    let Some(lib) = lib else { return };
    let year_ago = now.checked_sub_months(chrono::Months::new(12)).unwrap_or(now);
    match count_usage(&lib, &list, year_ago, settings.rules) {
        Ok(usage) => send(Step::Used(Arc::new(with_usage(&list, usage, &settings.weights)))),
        Err(e) => log::warning("Sounds", &format!("Couldn't count how your songs use sounds: {e}")),
    }
}

/// How your songs use each sound in `list` (by sound id), counting songs saved since `recent_since` as recent.
fn count_usage(
    lib: &mo_library::Library,
    list: &SoundList,
    recent_since: NaiveDateTime,
    rules: UsageRules,
) -> Result<HashMap<i64, Usage>, mo_library::LibraryError> {
    let uses = lib.sample_usage(recent_since, rules)?;
    Ok(background::on_pool(|| usage::usage(list.sounds(), &uses)))
}

/// How many sounds Sounds like this shows.
const ALIKE_COUNT: usize = 8;

/// A search for sounds like the one at `at`, answered off the window's thread (comparing with 450,000 others takes
/// about half a second); only the newest pick's answer is shown.
struct AlikeAsk {
    at: usize,
    list: Arc<SoundList>,
    places: Arc<HashMap<i64, usize>>,
    index: Arc<SoundIndex>,
}

impl AlikeAsk {
    /// The sounds like it, most alike first, as places in the list with how alike (0 to 1).
    fn answer(&self) -> Vec<(usize, f64)> {
        let Some(s) = self.list.sound(self.at) else { return Vec::new() };
        let get = |id: i64| self.places.get(&id).and_then(|&i| self.list.sound(i));
        let found = background::on_pool(|| self.index.similar(s, get, ALIKE_COUNT, false));
        found.into_iter().filter_map(|(id, likeness)| self.places.get(&id).map(|&i| (i, likeness))).collect()
    }
}

/// The fingerprints arrived: the picked sound can now say what sounds like it. Public for the screenshots.
pub fn index_known(app: &AppWindow, index: Arc<SoundIndex>) {
    let picked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.index = Some(index);
        p.picked
    });
    if let Some(at) = picked {
        find_alike(app, at);
    }
    galaxy::index_known(app);
    // Rediscover was waiting on the fingerprints.
    if PAGE.with(|p| p.borrow().re_started) {
        rediscover_now(app);
    }
}

fn find_alike(app: &AppWindow, at: usize) {
    let data = app.global::<SoundsData>();
    data.set_alike(ModelRc::default());
    let ask = PAGE.with(|p| {
        let p = p.borrow();
        let index = p.index.clone()?;
        Some(AlikeAsk { at, list: p.list.clone(), places: p.places.clone(), index })
    });
    let Some(ask) = ask else {
        data.set_alike_note("Not ready yet: the sounds' fingerprints are still being read.".into());
        return;
    };
    data.set_alike_note("Listening for sounds like it…".into());
    if HERE.with(|h| *h.borrow()) {
        let found = ask.answer();
        alike_found(app, at, found);
        return;
    }
    ALIKE.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Sounds like this", move |ask: AlikeAsk| {
                let found = ask.answer();
                let at = ask.at;
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        alike_found(&app, at, found);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error("Sounds", &format!("Couldn't start the Sounds like this thread: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

/// Shows the sounds like the one at `at`, if it's still the one picked.
fn alike_found(app: &AppWindow, at: usize, found: Vec<(usize, f64)>) {
    let lines = PAGE.with(|p| {
        let p = p.borrow();
        if p.picked != Some(at) {
            return None;
        }
        Some(
            found
                .iter()
                .filter_map(|&(i, likeness)| {
                    let s = p.list.sound(i)?;
                    Some(AlikeLine {
                        place: i as i32,
                        name: s.name().into(),
                        sound_type: s.sound_type.clone().into(),
                        colour: list::type_colour(&s.sound_type),
                        likeness: format!("{:.0}%", likeness.clamp(0.0, 1.0) * 100.0).into(),
                    })
                })
                .collect::<Vec<_>>(),
        )
    });
    let Some(lines) = lines else { return };
    let data = app.global::<SoundsData>();
    data.set_alike_note(if lines.is_empty() { "No sounds like it found." } else { "" }.into());
    data.set_alike(ModelRc::new(VecModel::from(lines)));
}

/// The use counts arrived: the same sounds in the same order, so what's shown and picked stays as it is.
fn use_known(app: &AppWindow, list: Arc<SoundList>) {
    let picked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.list.len() != list.len() {
            return None;
        }
        p.list = list;
        Some(p.picked)
    });
    let Some(picked) = picked else { return };
    refresh(app);
    // What Rediscover dug out didn't know your taste yet.
    if PAGE.with(|p| p.borrow().re_started) {
        rediscover_now(app);
    }
    if let Some(at) = picked {
        let sound = PAGE.with(|p| picked_sound(&p.borrow(), at));
        app.global::<SoundsData>().set_picked(sound);
    }
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
    let count = read.count.and_then(|c| c.parse().ok()).filter(|c| COUNTS.contains(c)).unwrap_or(COUNT);
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.re.count = count;
        p.kit.export_folder = read.export_folder;
    });
    show(app, read.library, read.view.as_deref(), read.autoplay.as_deref());
}

/// Shows a `library` on the page, opening on the view and autoplay setting given.
pub fn show(app: &AppWindow, library: Library, view: Option<&str>, autoplay: Option<&str>) {
    let data = app.global::<SoundsData>();
    data.set_empty_title("".into());
    data.set_tags(strings(&library.tags));
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.history = library.played;
        p.list = library.list;
        p.places = library.places;
        p.types = library.types;
        p.view = view.and_then(|v| VIEWS.iter().position(|n| *n == v)).map_or(TILES, |i| i as i32);
        // As in the C# app: off unless it was turned on.
        p.autoplay = autoplay == Some("1");
        p.picked = None;
        p.rediscover.clear();
        p.re_started = false;
        galaxy::forget(&mut p.galaxy);
        // An answer still on its way was for the old list: it's dropped.
        p.re_asked += 1;
    });
    refresh(app);
    show_rediscover_chips(app);
    kit::show(app);
    match PAGE.with(|p| p.borrow().view) {
        REDISCOVER => rediscover_now(app),
        GALAXY => galaxy::shown(app),
        _ => {}
    }
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
        let ty = PAGE.with(|p| p.borrow().types.get(i as usize).map(|t| t.0.clone()));
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
        // Rediscover fills itself the first time it's opened, so it never greets you empty (as in the C# app).
        if v == REDISCOVER && PAGE.with(|p| !p.borrow().re_started) {
            rediscover_now(&app);
        }
        if v == KITS {
            kit::show(&app);
        }
        if v == GALAXY {
            galaxy::shown(&app);
        }
    });
    wire_rediscover(app);
    kit::wire(app);
    galaxy::wire(app);
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
    // A sound like the picked one: picked in turn (and played with autoplay on), or played on a double-click.
    let weak = app.as_weak();
    data.on_pick_alike(move |place| {
        let Some(app) = weak.upgrade() else { return };
        let Ok(at) = usize::try_from(place) else { return };
        pick(&app, at);
        if PAGE.with(|p| p.borrow().autoplay) {
            play_picked(&app, 0.0);
        }
    });
    let weak = app.as_weak();
    data.on_play_alike(move |place| {
        let Some(app) = weak.upgrade() else { return };
        let Ok(at) = usize::try_from(place) else { return };
        pick(&app, at);
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

/// A Rediscover ask: everything its thread needs, so the page can go on changing meanwhile.
struct RediscoverAsk {
    id: u64,
    list: Arc<SoundList>,
    places: Arc<HashMap<i64, usize>>,
    index: Arc<SoundIndex>,
    options: rediscover::Options,
}

/// What Rediscover dug out for one ask.
struct RediscoverAnswer {
    id: u64,
    /// Places in the list, with why, best first.
    found: Vec<(usize, String)>,
    /// Why nothing was found, when nothing was.
    why: String,
    folders: Vec<FolderUse>,
    years: Vec<i32>,
    /// How many sounds your projects use: what your taste was learned from.
    learned_from: usize,
}

impl RediscoverAsk {
    /// Scoring every unused sound takes a while on a big library: it's spread over the low-priority pool's threads.
    fn answer(self) -> RediscoverAnswer {
        let empty = HashMap::new();
        let usage = self.list.usage_map().unwrap_or(&empty);
        let sounds = self.list.sounds();
        let o = &self.options;
        background::on_pool(|| {
            let found: Vec<(usize, String)> = rediscover::suggest(sounds, usage, &self.index, o)
                .into_iter()
                .filter_map(|s| self.places.get(&s.id).map(|&i| (i, s.reason)))
                .collect();
            let why = if found.is_empty() { rediscover::why_nothing(sounds, &self.index, o) } else { String::new() };
            let mut folders = rediscover::folders(sounds, usage);
            folders.truncate(FOLDERS);
            RediscoverAnswer {
                id: self.id,
                found,
                why,
                folders,
                years: rediscover::years(sounds),
                learned_from: usage.values().filter(|u| u.projects > 0).count(),
            }
        })
    }
}

/// Digs out a new Rediscover list with the choices as they are now (each time a new set: the seed is 0). Waits for
/// the fingerprints when they aren't read yet; they call this again when they arrive.
fn rediscover_now(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.list.is_empty() {
            return None;
        }
        p.re_asked += 1;
        p.re_started = true;
        p.re_busy = true;
        let ask = p.index.clone().map(|index| RediscoverAsk {
            id: p.re_asked,
            list: p.list.clone(),
            places: p.places.clone(),
            index,
            options: p.re.clone(),
        });
        Some((ask, p.list.len()))
    });
    let Some((ask, total)) = ask else { return };
    data.set_re_busy(true);
    data.set_re_empty_title("Picking sounds…".into());
    data.set_re_empty_text("".into());
    let Some(ask) = ask else {
        data.set_re_intro(
            "Listening to your sounds first: their fingerprints are still being read, and the list fills in by itself."
                .into(),
        );
        return;
    };
    data.set_re_intro(format!("Looking through {} sounds…", thousands(total)).into());
    if HERE.with(|h| *h.borrow()) {
        let answer = ask.answer();
        rediscovered(app, answer);
        return;
    }
    REDISCOVERING.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Rediscover", move |ask: RediscoverAsk| {
                let answer = ask.answer();
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        rediscovered(&app, answer);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error("Sounds", &format!("Couldn't start the Rediscover thread: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

/// Rediscover's answer arrived: shows it, unless a newer one has been asked for since.
fn rediscovered(app: &AppWindow, a: RediscoverAnswer) {
    let fresh = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.re_asked != a.id {
            return None;
        }
        p.re_busy = false;
        p.rediscover = a.found;
        Some((p.rediscover.len(), p.list.knows_use()))
    });
    let Some((found, knows_use)) = fresh else { return };
    let data = app.global::<SoundsData>();
    data.set_re_busy(false);
    let folders: Vec<FolderLine> = a
        .folders
        .iter()
        .map(|f| FolderLine {
            folder: f.folder.clone().into(),
            name: f.folder.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(&f.folder).into(),
            used: format!("{} of {} used", thousands(f.used), thousands(f.sounds)).into(),
        })
        .collect();
    data.set_folders(ModelRc::new(VecModel::from(folders)));
    data.set_re_years(strings(a.years.iter().map(i32::to_string)));
    if found == 0 {
        data.set_re_empty_title("Nothing to suggest".into());
        data.set_re_empty_text(a.why.into());
        data.set_re_intro("".into());
    } else {
        let intro = if !knows_use {
            "Your projects haven't been counted yet, so these are a varied pick. They fit your taste once they are."
                .to_owned()
        } else if a.learned_from == 0 {
            "None of your sounds show up in scanned projects yet, so these are a varied pick. Star favourites or scan \
             your projects to teach it your taste."
                .to_owned()
        } else {
            format!(
                "Learned from the {} sounds your projects use, plus your favourites and plays.",
                thousands(a.learned_from)
            )
        };
        data.set_re_intro(intro.into());
    }
    show_lists(app);
    bring_picked_into_view(app);
}

/// Rediscover's choices as they are now.
fn show_rediscover_chips(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_re_types(strings(p.types.iter().map(|t| t.0.as_str())));
        data.set_re_type(p.re.sound_type.clone().unwrap_or_default().into());
        data.set_re_year(p.re.year.map(|y| y.to_string()).unwrap_or_default().into());
        data.set_re_adventure(p.re.adventure as f32);
        data.set_re_counts(strings(COUNTS.map(|c| c.to_string())));
        data.set_re_count(p.re.count.to_string().into());
        data.set_re_rarely(p.re.max_uses > 0);
    });
}

fn wire_rediscover(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    // Once there's a list, changing what it's picked by picks it again (as in the C# app); the newest ask wins.
    let weak = app.as_weak();
    data.on_re_pick(move |what, value| {
        let Some(app) = weak.upgrade() else { return };
        let again = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let v = value.as_str();
            match what.as_str() {
                "type" => p.re.sound_type = (!v.is_empty()).then(|| v.to_owned()),
                "year" => p.re.year = v.parse().ok(),
                "count" => {
                    if let Some(c) = v.parse().ok().filter(|c| COUNTS.contains(c)) {
                        p.re.count = c;
                    }
                }
                _ => {}
            }
            p.re_started
        });
        show_rediscover_chips(&app);
        if again {
            rediscover_now(&app);
        }
    });
    // The slider picks again once it's let go, not while it moves.
    let weak = app.as_weak();
    data.on_re_adventure_set(move |v| {
        let Some(app) = weak.upgrade() else { return };
        let again = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.re.adventure = f64::from(v.clamp(0.0, 1.0));
            p.re_started
        });
        show_rediscover_chips(&app);
        if again {
            rediscover_now(&app);
        }
    });
    let weak = app.as_weak();
    data.on_re_toggle_rarely(move || {
        let Some(app) = weak.upgrade() else { return };
        let again = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.re.max_uses = if p.re.max_uses == 0 { 1 } else { 0 };
            p.re_started
        });
        show_rediscover_chips(&app);
        if again {
            rediscover_now(&app);
        }
    });
    let weak = app.as_weak();
    data.on_re_shuffle(move || {
        if let Some(app) = weak.upgrade() {
            rediscover_now(&app);
        }
    });
    // A forgotten folder: its sounds in the List, with the other filters cleared so all of them show.
    let weak = app.as_weak();
    data.on_pick_folder(move |folder| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.filters = Filters { search: folder.to_string(), ..Filters::default() };
            p.view = LIST;
        });
        app.global::<SoundsData>().set_search(folder);
        refresh(&app);
        show_lists(&app);
    });
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
                let shown = background::on_pool(|| ask.list.view(&ask.filters, ask.order, ask.seed));
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
        p.shown = Arc::new(shown);
        true
    });
    if current {
        show_lists(app);
        // The filters dim the stars they leave out.
        galaxy::draw(app);
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
        } else if p.view != HISTORY && p.view != REDISCOVER && (p.filters.any() || !p.filters.search.trim().is_empty())
        {
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
    kit::redraw();
}

fn show_chips(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let f = &p.filters;
        let counts = &p.types;
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
    find_alike(app, at);
    galaxy::show_picked(app);
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
        liked_why: liked_why(p, at).into(),
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
    let ask = PLAYS.fetch_add(1, Ordering::SeqCst) + 1;
    let weak = app.as_weak();
    let started =
        background::spawn("Read sound", move || match mo_audio::read(Path::new(&path), mo_audio::MAX_SECONDS) {
            // A newer play or a stop came while this one was reading.
            _ if PLAYS.load(Ordering::SeqCst) != ask => {}
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
    PLAYS.fetch_add(1, Ordering::SeqCst);
    if let Some(player) = PLAYER.get() {
        player.stop();
    }
    PAGE.with(|p| p.borrow_mut().playing = None);
    playing_changed(app);
}

/// A sound played to its end by itself.
fn ended(key: &str) {
    let ours = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let ours = p.playing.and_then(|(i, _)| p.list.sound(i)).is_some_and(|s| s.path == key);
        if ours {
            p.playing = None;
        }
        ours
    });
    // An older sound, cut off by a newer one that's still reading: the newer one keeps the page.
    if !ours {
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A small song library like the C# app's: two songs, a kick in both, a pad with no notes in one, and settings.
    fn song_library(name: &str, settings: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join("mo-ui-sounds-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        for end in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{end}", path.display()));
        }
        let Ok(c) = rusqlite::Connection::open(&path) else { panic!("couldn't make {}", path.display()) };
        let made = c.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
                 daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0);
             CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER, unplayed INTEGER NOT NULL DEFAULT 0);
             INSERT INTO projects (id, path, name, daw, folder, mtime) VALUES
                 (1, 'C:\\beats\\Song.flp', 'Song', 'FL Studio', 'C:\\beats', 1590969600),
                 (2, 'C:\\beats\\Other.flp', 'Other', 'FL Studio', 'C:\\beats', 1717200000);
             INSERT INTO samples (project_id, path, found, unplayed) VALUES
                 (1, 'C:\\s\\kick.wav', 1, 0), (2, 'C:\\s\\kick.wav', 1, 0),
                 (1, 'C:\\s\\pad.wav', 1, 1), (2, 'C:\\s\\pad.wav', 1, 0);",
        );
        assert!(made.is_ok(), "{made:?}");
        for (k, v) in settings {
            assert!(c.execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", [k, v]).is_ok());
        }
        path
    }

    fn sound(id: i64, path: &str) -> Sound {
        Sound {
            id,
            path: path.into(),
            error: None,
            analysis: None,
            sound_type: "Kick".into(),
            type_source: String::new(),
            type_confidence: 0.0,
            user_type: None,
            name_type: None,
            tags: Vec::new(),
            favourite: false,
            hidden: false,
            rating: 0,
            plays: 0,
            added: NaiveDateTime::default(),
            activity: Default::default(),
        }
    }

    #[test]
    fn songs_use_sounds_by_the_c_sharp_apps_settings() {
        let list =
            SoundList::new(vec![sound(1, r"C:\s\kick.wav"), sound(2, r"C:\s\pad.wav"), sound(3, r"C:\s\hat.wav")]);
        let since =
            chrono::NaiveDate::from_ymd_opt(2023, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap_or_default();
        let read = |name: &str, settings: &[(&str, &str)]| {
            let path = song_library(name, settings);
            let Ok(lib) = mo_library::Library::open_read_only(&path) else { panic!("couldn't read {name}") };
            let s = Settings::read(&lib);
            let used = count_usage(&lib, &list, since, s.rules).unwrap_or_default();
            (s, used)
        };
        // With nothing set: every song listing a sound uses it; autoplay is off and the weights are the C# app's.
        let (s, used) = read("plain.db", &[]);
        assert_eq!(used.get(&1).map(|u| u.projects), Some(2));
        assert_eq!(used.get(&2).map(|u| (u.projects, u.recent)), Some((2, 1)));
        assert!(!used.contains_key(&3));
        assert_eq!((s.view, s.autoplay), (None, None));
        assert_eq!(s.weights, LikedWeights::default());
        // "Only where it has notes": the pad isn't played in Song. And the page's own settings come through.
        let (s, used) = read(
            "notes.db",
            &[
                (USED_NEEDS_NOTES_KEY, "1"),
                (VIEW_KEY, "List"),
                (AUTOPLAY_KEY, "1"),
                (LIKED_WEIGHTS_KEY, r#"{"Project":7}"#),
            ],
        );
        assert_eq!(used.get(&2).map(|u| u.projects), Some(1));
        assert_eq!((s.view.as_deref(), s.autoplay.as_deref()), (Some("List"), Some("1")));
        assert_eq!(s.weights, LikedWeights { project: 7.0, ..LikedWeights::default() });
    }

    #[test]
    fn use_counts_change_most_liked_but_not_the_order() {
        let list = SoundList::new(vec![sound(1, "a.wav"), sound(2, "b.wav")]);
        let w = LikedWeights::default();
        let used = with_usage(&list, HashMap::from([(2, Usage { projects: 3, recent: 3, years: vec![2026] })]), &w);
        assert_eq!(used.sounds().iter().map(|s| s.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(used.used_in(0), Some(0));
        assert_eq!(used.used_in(1), Some(3));
        assert_eq!(used.liked_share(1), Some(1.0));
    }

    #[test]
    fn rediscover_suggests_unused_sounds_by_their_place() {
        let mut sounds: Vec<Sound> = (1..=6).map(|id| sound(id, &format!("Pack/{id}.wav"))).collect();
        for s in &mut sounds {
            s.analysis = Some(mo_sounds::SoundAnalysis { seconds: 0.5, ..Default::default() });
        }
        let lib = Library::new(sounds, Vec::new(), &LikedWeights::default()).with_usage(
            HashMap::from([(2, Usage { projects: 3, recent: 1, years: vec![2026] })]),
            &LikedWeights::default(),
        );
        let features = (1..=6i64).map(|id| (id, (0..8).map(|d| ((id + d) % 3) as f32).collect())).collect();
        let index = SoundIndex::build(&features, &Default::default(), 6);
        let ask = RediscoverAsk {
            id: 7,
            list: lib.list.clone(),
            places: lib.places.clone(),
            index: Arc::new(index),
            options: rediscover::Options { seed: 1, ..rediscover::Options::default() },
        };
        let a = ask.answer();
        assert_eq!(a.id, 7);
        assert_eq!(a.learned_from, 1);
        assert!(!a.found.is_empty(), "{}", a.why);
        // Sound 2 is used, so it isn't suggested; every place found is a sound in the list.
        for (at, reason) in &a.found {
            let s = lib.list.sound(*at).map(|s| s.id);
            assert!(s.is_some() && s != Some(2));
            assert!(!reason.is_empty());
        }
        assert_eq!(a.folders.first().map(|f| (f.sounds, f.used)), Some((6, 1)));
    }

    #[test]
    fn counts_and_times_read_well() {
        let now =
            chrono::NaiveDate::from_ymd_opt(2026, 11, 1).and_then(|d| d.and_hms_opt(12, 0, 0)).unwrap_or_default();
        assert_eq!(when(now - chrono::Duration::minutes(5), now), "11:55");
        assert_eq!(when(now - chrono::Duration::days(2), now), "Fri");
        assert_eq!(when(now - chrono::Duration::days(30), now), "2 Oct");
        assert_eq!(key_choices().len(), 24);
    }
}
