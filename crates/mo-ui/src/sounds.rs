//! Fills the Sounds page from the sound library and plays the picked sound. The library (`sounds.db`, the C# app's,
//! read-only) is read once on a background thread; each change of the filters or order is worked out on its own
//! low-priority thread, newest ask first (rule 12), across every core (rule 21). Tiles and lines are only made when
//! the list draws them, so 357,000 sounds cost what's on screen. Sounds are read and played off the window's thread.
//! Rediscover (unused sounds that fit your taste) is dug out on its own low-priority thread too.

mod drag_out;
pub use drag_out::card_at_row;
mod galaxy;
mod grid;
mod kit;
mod sky;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::filters::{Filter, Shown};
use mo_app::finder::{self, Kind};
use mo_app::places::Place;
use mo_app::shortcuts::{Shortcuts, what_of};
use mo_app::sounds::{self as list, Filters, Order, SoundCounts, SoundList};
use mo_app::{log, sweep_timing, to_bitwig, year_strip};
use mo_audio::play::Player;
use mo_library::UsageRules;
use mo_library::songs::thousands;
use mo_sounds::compact::{self, PageRead};
use mo_sounds::index::{Gathered, SoundIndex};
use mo_sounds::liking::{self, LIKED_WEIGHTS_KEY, LikedWeights};
use mo_sounds::rediscover::{self, FolderUse};
use mo_sounds::usage::{self, Usage};
use mo_sounds::{Sound, SoundsDb, SoundsError};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer, TimerMode, VecModel};

use crate::filters::FilterModels;
use crate::finder::How;
use crate::{
    AlikeLine, AppWindow, CardAction, FolderLine, MenuItem, PickedSound, SoundFact, SoundLine, SoundTile, SoundsData,
    TileRow, YearBar,
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
/// The views, in the switch's order. Ctrl+K offers each one as a page of its own.
pub const VIEWS: [&str; 6] = ["Tiles", "List", "Galaxy", "History", "Rediscover", "Kits"];
/// The C# app's export folder setting in `library.db` (kits go in its Kits folder).
const EXPORT_FOLDER_KEY: &str = "export_folder";

/// Rediscover's How many choices (the C# app's), and the one it starts on.
const COUNTS: [usize; 4] = [30, 60, 120, 250];
const COUNT: usize = 60;
/// How many forgotten folders Rediscover lists, as in the C# app.
const FOLDERS: usize = 40;

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
    /// The year strip's years, newest first, with how many sounds each holds.
    years: YearCounts,
    /// Every sound played, newest first, as places in `list`, with when.
    played: Arc<Vec<(usize, NaiveDateTime)>>,
    /// What History shows: the sounds played that the filters let through (the year strip's years aside).
    history: Vec<(usize, NaiveDateTime)>,
    /// How many tiles fit side by side.
    columns: usize,
    /// The picked sound, as its place in `list`.
    picked: Option<usize>,
    /// A sound Ctrl+K jumped to that the filters hid: picked once the list without them arrives.
    to_pick: Option<usize>,
    autoplay: bool,
    /// How long each sound plays while a held arrow key steps through them, and when the pick last moved.
    step: Duration,
    stepping: list::Stepping,
    /// Counts the asks for a new list, so an older answer that arrives late is dropped.
    asked: u64,
    /// The sounds have been read and handed to the page ([`show`]): the next list shown is of them.
    read: bool,
    /// The type chips' types with their counts, in chip order, then the rest (the More types menu).
    types: Vec<(String, usize)>,
    /// Every tag on a sound, for the Tag filter.
    tags: Vec<String>,
    /// How many sounds each filter's choices would show, from the newest list worked out (none before the first).
    counts: Option<SoundCounts>,
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
            years: Vec::new(),
            played: Arc::default(),
            history: Vec::new(),
            columns: 4,
            picked: None,
            to_pick: None,
            autoplay: false,
            step: Duration::from_millis(list::STEP_MS),
            stepping: list::Stepping::default(),
            asked: 0,
            read: false,
            types: Vec::new(),
            tags: Vec::new(),
            counts: None,
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
    played: Arc<Vec<(usize, NaiveDateTime)>>,
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    static TILE_ROWS: Rc<TileRows> = PAGE.with(|p| Rc::new(TileRows { page: p.clone(), notify: ModelNotify::default() }));
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
    static WORKER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    /// The filters' chips and menu lines (type, length, key, tempo, tag), changed in place so an open menu keeps its
    /// place.
    static FILTER_MODELS: [FilterModels; 5] = Default::default();
    /// The picked sound's tags on its panel.
    static PICKED_TAGS: FilterModels = FilterModels::default();
    static ALIKE: RefCell<Option<Newest<AlikeAsk>>> = const { RefCell::new(None) };
    static REDISCOVERING: RefCell<Option<Newest<RediscoverAsk>>> = const { RefCell::new(None) };
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
    /// Moves the picked tile's waveform along while it plays.
    static TICKER: Timer = Timer::default();
    /// Works out lists in place, for the screenshots, which have no event loop to send answers back to.
    static HERE: RefCell<bool> = const { RefCell::new(false) };
}

/// The one player, made the first time something plays or Settings lists the outputs (nothing opens before a sound
/// plays).
static PLAYER: OnceLock<Player> = OnceLock::new();
/// The output Settings chose (`None`: the computer's default), for the player once it's made.
static OUTPUT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The one player, made on first use, playing through the output Settings chose.
pub fn player() -> &'static Player {
    if let Some(player) = PLAYER.get() {
        return player;
    }
    // The choice stays locked until the player is made, so one changed meanwhile waits and then reaches it.
    let output = OUTPUT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    PLAYER.get_or_init(|| {
        let player = Player::new(|key| {
            let key = key.to_owned();
            let _ = slint::invoke_from_event_loop(move || ended(&key));
        });
        player.choose(output.clone());
        player
    })
}

/// Whether a Galaxy sweep holds the pointer to the star that's playing (Settings › General).
pub fn hold_sweeps(on: bool) {
    galaxy::hold_sweeps(on);
}

/// Plays through this output from now on (`None`: the computer's default), as Settings › Sound output chose. Before
/// the player is made it's only remembered.
pub fn choose_output(name: Option<String>) {
    let mut output = OUTPUT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *output = name.clone();
    if let Some(player) = PLAYER.get() {
        player.choose(name);
    }
}
/// Counts the plays and stops asked for, so a sound that finishes reading after a newer play or a stop stays quiet
/// (newest wins, rule 12).
static PLAYS: AtomicU64 = AtomicU64::new(0);
/// The newest play's number (not a stop's), so a read cut short can tell a newer sound from a stop.
static NEWEST_PLAY: AtomicU64 = AtomicU64::new(0);

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
        sound_type: s.sound_type.as_str().into(),
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
    s.analysis.as_ref().map(|a| list::wave_path_of(a.waveform.iter())).unwrap_or_default()
}

fn points(s: &Sound) -> i32 {
    s.analysis.as_ref().map_or(0, |a| a.waveform.len() as i32)
}

pub(crate) fn new_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

pub(crate) fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(|s| SharedString::from(s.as_ref())).collect::<Vec<_>>()))
}

/// For the screenshots: work lists out on the window's thread, as there's no event loop to send them back with.
pub fn compute_here() {
    HERE.with(|h| *h.borrow_mut() = true);
}

/// A graphics card came (the window is up, or came back): the galaxy is drawn on it.
pub fn graphics_ready(app: &AppWindow) {
    galaxy::draw(app);
    grid::graphics_ready(app);
}

/// The graphics card went (the window closed, or the card was reset): the galaxy's and the tiles' painters go too, at
/// once, even as the window itself goes.
pub fn card_gone() {
    galaxy::painter_gone();
    grid::card_gone();
}

/// The graphics card went: the galaxy's picture on show goes too, and the tiles draw their own shapes.
pub fn graphics_gone(app: &AppWindow) {
    galaxy::card_gone(app);
    grid::graphics_gone(app);
}

/// Wires the page and starts reading the sound library at `sounds_db` in the background, then how much your songs use
/// each sound from the song library at `library_db` (both read-only). The sounds show as soon as they're read; the use
/// counts and Most liked follow when they're worked out, as in the C# app. Show in folder and Copy path report on
/// `feed`'s cards, and so does the app's own copy of the sound library as it's made or brought up to date.
pub fn connect(app: &AppWindow, sounds_db: PathBuf, library_db: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    let cards = feed.clone();
    wire(app, feed, shortcuts);
    crate::finder::on_open(Kind::Sound, |app, hit, how| {
        let Some(at) = hit.id.parse::<i64>().ok().and_then(|id| PAGE.with(|p| p.borrow().places.get(&id).copied()))
        else {
            return false;
        };
        match how {
            How::Play => play_sound(app, at, 0.0, mo_audio::MAX_SECONDS),
            How::Open | How::InDaw => {
                crate::go_to(app, Place::Sounds.index() as i32);
                show_sound(app, at);
            }
        }
        true
    });
    PAGE.with(|p| p.borrow_mut().galaxy.db = Some(sounds_db.clone()));
    reading(app);
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
        // The screenshots read their fixture as it is: nothing is written beside it.
        read_all(&sounds_db, &library_db, None, send);
        return;
    }
    if let Err(e) = background::spawn("Sounds library", move || read_all(&sounds_db, &library_db, Some(cards), send)) {
        // The sounds would never come: the page says why, rather than reading for ever.
        loaded(app, Err(format!("its reader couldn't start ({e})")));
    }
}

/// While the sounds are read, every view shows one word in the list's place: not "No sounds match" (nothing is
/// filtered out yet), and not the galaxy's own word on top of it (Solomon's F12 note, 2026-11-02 04:25). Said once,
/// as the Songs page says it, not again in the page's header. The first list of the sounds takes its place once they
/// arrive ([`listed`]), or why they couldn't be read does ([`loaded`]).
pub fn reading(app: &AppWindow) {
    PAGE.with(|p| p.borrow_mut().read = false);
    let data = app.global::<SoundsData>();
    data.set_empty_title("Reading your sounds…".into());
    data.set_empty_text("".into());
}

/// What reading brings back, in steps: the sounds first, then the list again with how your songs use them.
enum Step {
    Read(Result<Box<Read>, String>),
    Used(Arc<SoundList>),
    /// Every sound's fingerprint, for Sounds like this.
    Index(Arc<SoundIndex>),
    /// The sounds again, once the app's copy of the library was brought up to date with what the C# app changed.
    Refreshed(Refreshed),
    /// Where the galaxy's saved maps are read from now: `sounds.db`, as the copy damaged couldn't be made again.
    Source(PathBuf),
}

fn arrive(app: &AppWindow, step: Step) {
    let _doing = crate::watchdog::doing("Sounds: the library arrived");
    match step {
        Step::Read(read) => loaded(app, read),
        Step::Used(list) => use_known(app, list),
        Step::Index(index) => index_known(app, index),
        Step::Refreshed(refreshed) => brought_up_to_date(app, refreshed),
        Step::Source(source) => PAGE.with(|p| p.borrow_mut().galaxy.db = Some(source)),
    }
}

/// What reading the library gives: the sounds ready to list, what was played, and the view and autoplay you had last.
struct Read {
    library: Library,
    /// Where the galaxy's saved maps are read from: the app's copy of the library, or `sounds.db` without one.
    source: PathBuf,
    view: Option<String>,
    count: Option<String>,
    export_folder: Option<String>,
    drum_machine: Option<String>,
    autoplay: Option<String>,
    step_ms: Option<String>,
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

/// `list` again, sharing its sounds rather than copying them, with each sound's use in your songs and Most liked
/// scored with it.
fn with_usage(list: &SoundList, usage: HashMap<i64, Usage>, weights: &LikedWeights) -> SoundList {
    background::on_pool(|| {
        let mut list = list.sharing();
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
    /// The Drum Machine's ID the C# app read from Bitwig's files, for kits sent to Bitwig.
    drum_machine: Option<String>,
    autoplay: Option<String>,
    /// How long each sound plays while a held arrow key steps through them.
    step_ms: Option<String>,
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
            drum_machine: get(to_bitwig::DRUM_MACHINE_KEY),
            autoplay: get(AUTOPLAY_KEY),
            step_ms: get(list::STEP_KEY),
            rules: UsageRules { needs_notes: on(USED_NEEDS_NOTES_KEY), not_muted_in_newest: on(USED_NOT_MUTED_KEY) },
            weights: LikedWeights::from_json(get(LIKED_WEIGHTS_KEY).as_deref()),
        }
    }
}

/// Reads the sounds and sends them, then their fingerprints, then works out how your songs use them and sends that.
/// Without a song library the use stays unknown (the Used chips stay hidden).
///
/// With `cards` (not for the screenshots, which read their fixture as it is), the sounds come from the app's own copy
/// of the library beside `sounds.db` ([`compact`]): a fraction of the reading. With no copy yet, it's made on the
/// same pass that reads `sounds.db` for the page. A copy that's behind (the C# app changed `sounds.db` since) is shown
/// at once, then brought up to date, and the page takes the sounds again from it ([`Step::Refreshed`]).
fn read_all(sounds_db: &Path, library_db: &Path, cards: Option<Feed>, send: impl Fn(Step)) {
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
    let began = Instant::now();
    let first = match &cards {
        Some(cards) => read_through_copy(sounds_db, now, cards),
        None => read_in_place(sounds_db, now),
    };
    let mut first = match first {
        Ok(first) => first,
        Err(e) => {
            send(Step::Read(Err(e.to_string())));
            return;
        }
    };
    let read_in = began.elapsed();
    let sounds = std::mem::take(&mut first.sounds);
    let analysed = mo_sounds::count_analysed(&sounds);
    let library = Library::new(sounds, std::mem::take(&mut first.played), &settings.weights);
    log::info(
        "Sounds",
        &format!(
            "{} sounds (read from {} in {:.1} s, listed in {:.1} s)",
            library.list.len(),
            first.source.file_name().unwrap_or_default().to_string_lossy(),
            read_in.as_secs_f32(),
            (began.elapsed() - read_in).as_secs_f32()
        ),
    );
    let list = library.list.clone();
    send(Step::Read(Ok(Box::new(Read {
        library,
        source: first.source.clone(),
        view: settings.view.clone(),
        count: settings.count.clone(),
        export_folder: settings.export_folder.clone(),
        drum_machine: settings.drum_machine.clone(),
        autoplay: settings.autoplay.clone(),
        step_ms: settings.step_ms.clone(),
    }))));
    // Sounds like this: every sound's fingerprint, read with the sounds or from the copy after them; their blocks
    // become the index.
    let mut remade = None;
    let prints = match first.prints {
        Prints::Read(features, clap) => Ok((features, clap, analysed)),
        Prints::InCopy(db) => match db.fingerprints(analysed) {
            Ok((features, clap)) => Ok((features, clap, analysed)),
            // Damaged where they're kept (a bad sector): the copy is made again from sounds.db, which gives them, and
            // the sounds as they are now, on the same pass.
            Err(e) if e.damaged() => {
                drop(db);
                log::warning(
                    "Sounds",
                    &format!("The app's copy of the sound library is damaged, so it's made again: {e}"),
                );
                match compact::remake_reading(sounds_db, &first.source, now) {
                    Ok((mut everything, made)) => {
                        say_made(&made, &first.source, sounds_db, cards.as_ref(), true);
                        if made.is_err() {
                            // Not made again: the rest is read from sounds.db, as it is without a copy.
                            first.source = sounds_db.to_owned();
                            send(Step::Source(first.source.clone()));
                        }
                        let (features, clap) =
                            (std::mem::take(&mut everything.features), std::mem::take(&mut everything.clap));
                        let analysed = everything.analysed();
                        // Taken in place of the copy's sounds shown, when those were behind: else they're the same.
                        remade = first.behind.then_some(everything.sounds);
                        Ok((features, clap, analysed))
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        },
    };
    send_index(prints, &send);
    let year_ago = now.checked_sub_months(chrono::Months::new(12)).unwrap_or(now);
    // How your songs use samples, read once: counted again for the sounds read again after an update.
    let uses = lib.and_then(|lib| {
        lib.sample_usage(year_ago, settings.rules)
            .map_err(|e| log::warning("Sounds", &format!("Couldn't count how your songs use sounds: {e}")))
            .ok()
    });
    if let Some(uses) = &uses {
        send(Step::Used(Arc::new(with_usage(&list, usage_of(&list, uses), &settings.weights))));
    }
    if !first.behind {
        return;
    }
    // The sounds shown were as the copy had them before the C# app's changes: the page takes them again, as they were
    // read just now to make the copy again, or else from the copy once it's brought up to date.
    let (sounds, db, synced) = match remade {
        Some(sounds) => (sounds, SoundsDb::open_read_only(&first.source), None),
        None => {
            let Some(synced) = bring_up_to_date(sounds_db, &first.source, cards.as_ref()) else { return };
            // Only what the C# app keeps for itself changed (its settings, the galaxy's saved places, copies found).
            if !synced.changed_the_list() {
                return;
            }
            match SoundsDb::open_read_only(&first.source).and_then(|db| Ok((db.all(now)?, db))) {
                Ok((sounds, db)) => (sounds, Ok(db), Some(synced)),
                Err(e) => {
                    log::warning(
                        "Sounds",
                        &format!("Couldn't read the sounds again once they were brought up to date: {e}"),
                    );
                    return;
                }
            }
        }
    };
    // What was played: the sounds are in hand, so a History that can't be read just now stays empty.
    let played = db
        .as_ref()
        .map_err(SoundsError::to_string)
        .and_then(|db| db.history(HISTORY_LENGTH).map_err(|e| e.to_string()));
    let played = played.unwrap_or_else(|e| {
        log::warning("Sounds", &format!("Couldn't read what was played once the sounds were read again: {e}"));
        Vec::new()
    });
    let analysed_now = mo_sounds::count_analysed(&sounds);
    let mut library = Library::new(sounds, played, &settings.weights);
    if let Some(uses) = &uses {
        library.list = Arc::new(with_usage(&library.list, usage_of(&library.list, uses), &settings.weights));
    }
    // Where each sound on show is in the new list, when they aren't the same sounds in the same order: worked out
    // here, off the window's thread, for the page to move what it holds by place (half a million sounds to look up).
    let moved = (!list.same_sounds_as(&library.list)).then(|| Arc::new(places_in(&list, &library.places)));
    // The same sounds in the same places, one given another type (in the C# app): the galaxy colours its stars, and
    // names its neighbourhoods, by type.
    let retyped =
        moved.is_none() && list.sounds().iter().zip(library.list.sounds()).any(|(a, b)| a.sound_type != b.sound_type);
    send(Step::Refreshed(Refreshed { library, was: list, moved, retyped }));
    // The fingerprints read before stay right unless a sound gained, lost or changed one (or the count that says
    // whether CLAP has heard enough changed).
    if let (Some(synced), Ok(db)) = (synced, db)
        && (synced.prints > 0 || analysed_now != analysed)
    {
        send_index(db.fingerprints(analysed_now).map(|(f, c)| (f, c, analysed_now)), &send);
    }
}

/// How many plays History lists.
const HISTORY_LENGTH: usize = 500;

/// Where each sound of `list` is in a list read again (`places`: each sound's place there, by id); `None` for one
/// that's gone.
fn places_in(list: &SoundList, places: &HashMap<i64, usize>) -> Vec<Option<usize>> {
    list.sounds().iter().map(|s| places.get(&s.id).copied()).collect()
}

/// Makes the index of the fingerprints read (with how many sounds had been analysed when they were), and sends it.
fn send_index(prints: Result<(Gathered, Gathered, usize), SoundsError>, send: &impl Fn(Step)) {
    match prints {
        Ok((features, clap, analysed)) => {
            let index = background::on_pool(move || SoundIndex::from_gathered(features, clap, analysed));
            send(Step::Index(Arc::new(index)));
        }
        Err(e) => log::warning("Sounds", &format!("Couldn't read the sounds' fingerprints: {e}")),
    }
}

/// The page's first read of the sound library, wherever it came from.
struct FirstRead {
    sounds: Vec<Sound>,
    /// What was played (sound id, when), newest first.
    played: Vec<(i64, NaiveDateTime)>,
    /// What the rest is read from: the app's copy of the library, or `sounds.db` without one.
    source: PathBuf,
    prints: Prints,
    /// The sounds came from a copy that's behind `sounds.db`: it's brought up to date once the page has them.
    behind: bool,
}

/// Where the sounds' fingerprints come from.
enum Prints {
    /// Read on the same pass as the sounds.
    Read(Gathered, Gathered),
    /// Read from the app's copy of the library after the sounds.
    InCopy(SoundsDb),
}

/// The sounds read straight from `sounds_db`, fingerprints and all, writing nothing beside it (the screenshots).
fn read_in_place(sounds_db: &Path, now: NaiveDateTime) -> Result<FirstRead, SoundsError> {
    let db = SoundsDb::open_read_only(sounds_db)?;
    // The sounds and their fingerprints in one pass over the library: reading the fingerprints apart meant reading
    // the whole file twice more.
    let everything = db.everything(now)?;
    Ok(FirstRead {
        sounds: everything.sounds,
        played: db.history(HISTORY_LENGTH).unwrap_or_default(),
        source: sounds_db.to_owned(),
        prints: Prints::Read(everything.features, everything.clap),
        behind: false,
    })
}

/// The sounds read from the app's own copy of the library beside `sounds_db`, or (with no copy it can read yet) from
/// `sounds_db` while the copy is made on the same pass ([`compact::read_for_page`]). Making it goes on `cards`.
fn read_through_copy(sounds_db: &Path, now: NaiveDateTime, cards: &Feed) -> Result<FirstRead, SoundsError> {
    let copy = compact::copy_of(sounds_db);
    match compact::read_for_page(sounds_db, now)? {
        PageRead::Copy { db, sounds, behind } => Ok(FirstRead {
            sounds,
            played: db.history(HISTORY_LENGTH).unwrap_or_default(),
            source: copy,
            prints: Prints::InCopy(db),
            behind,
        }),
        PageRead::Library { everything, made } => {
            say_made(&made, &copy, sounds_db, Some(cards), false);
            // The rest is read from the copy made at the moment the sounds were read, or else (when it couldn't be
            // made, or opened just now) from `sounds.db`. What was played: the sounds are in hand, so a History that
            // can't be read just now stays empty.
            let (source, played) = match made.as_ref().ok().and_then(|_| SoundsDb::open_read_only(&copy).ok()) {
                Some(db) => (copy, db.history(HISTORY_LENGTH)),
                None => (
                    sounds_db.to_owned(),
                    SoundsDb::open_read_only(sounds_db).and_then(|db| db.history(HISTORY_LENGTH)),
                ),
            };
            Ok(FirstRead {
                sounds: everything.sounds,
                played: played.unwrap_or_default(),
                source,
                prints: Prints::Read(everything.features, everything.clap),
                behind: false,
            })
        }
    }
}

/// Says (in the log, and on `cards`) how making the app's copy of the library beside `sounds_db` went: `again` when
/// it was made again because it was damaged.
fn say_made(
    made: &Result<compact::Synced, SoundsError>,
    copy: &Path,
    sounds_db: &Path,
    cards: Option<&Feed>,
    again: bool,
) {
    let size = |path: &Path| fs::metadata(path).map_or(0, |m| m.len());
    match made {
        Ok(synced) if synced.made => {
            let line = list::copy_made_line(size(copy), size(sounds_db));
            log::info("Sounds", &format!("Made the app's own copy of the sound library: {line}"));
            let title = if again {
                "Made the app's own copy of your sound library again (it was damaged)"
            } else {
                "Made the app's own copy of your sound library"
            };
            if let Some(cards) = cards {
                cards.start(title, "").done(&line);
            }
        }
        // One there that couldn't be read just then, brought up to date instead.
        Ok(synced) => {
            log::info("Sounds", &format!("Brought the app's copy of the sound library up to date: {synced:?}"));
        }
        Err(e) => {
            log::warning("Sounds", &format!("{e}"));
            if let Some(cards) = cards {
                cards
                    .start("Couldn't make the app's own copy of your sound library", "")
                    .failed(&list::copy_failed_line(e));
            }
        }
    }
}

/// Brings the app's copy of the library up to date with `sounds_db` after the C# app changed it, saying so on
/// `cards`. What changed, or `None` when it couldn't be done (it's tried again at the next start).
fn bring_up_to_date(sounds_db: &Path, copy: &Path, cards: Option<&Feed>) -> Option<compact::Synced> {
    let job = cards.map(|c| c.start("Bringing your sound library up to date", "With what the C# app changed in it"));
    let began = Instant::now();
    match compact::sync(sounds_db, copy) {
        Ok(synced) => {
            let line = list::copy_synced_line(&synced);
            log::info(
                "Sounds",
                &format!(
                    "Brought the app's copy of the sound library up to date in {:.1} s: {line}",
                    began.elapsed().as_secs_f32()
                ),
            );
            if let Some(job) = job {
                job.done_as("Brought your sound library up to date", &line);
            }
            Some(synced)
        }
        Err(e) => {
            log::warning("Sounds", &format!("{e}"));
            if let Some(job) = job {
                job.failed_as("Couldn't bring your sound library up to date", &list::copy_failed_line(&e));
            }
            None
        }
    }
}

/// How your songs use each sound in `list` (by sound id), from how they use samples (`uses`).
fn usage_of(list: &SoundList, uses: &[mo_library::SampleUse]) -> HashMap<i64, Usage> {
    background::on_pool(|| usage::usage(list.sounds(), uses))
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
    let (picked, first) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let first = p.index.replace(index).is_none();
        // New ones in place of those the galaxy was laid out from (sounds new since, or heard again once the library
        // was brought up to date): it's laid out again from these.
        if !first {
            galaxy::forget(&mut p.galaxy);
        }
        (p.picked, first)
    });
    if let Some(at) = picked {
        find_alike(app, at);
    }
    galaxy::index_known(app);
    // Rediscover was waiting on the fingerprints. New ones later don't dig its sounds out again: they'd all change.
    if first && PAGE.with(|p| p.borrow().re_started) {
        rediscover_now(app);
    }
}

fn find_alike(app: &AppWindow, at: usize) {
    let data = app.global::<SoundsData>();
    data.set_alike(ModelRc::default());
    if PAGE.with(|p| p.borrow().index.is_none()) {
        data.set_alike_note("Not ready yet: the sounds' fingerprints are still being read.".into());
        return;
    }
    data.set_alike_note("Listening for sounds like it…".into());
    ask_alike(app, at);
}

/// Asks what sounds like the one at `at`, leaving what the panel shows as it is until the answer comes (nothing is
/// asked before the fingerprints are in).
fn ask_alike(app: &AppWindow, at: usize) {
    let ask = PAGE.with(|p| {
        let p = p.borrow();
        let index = p.index.clone()?;
        Some(AlikeAsk { at, list: p.list.clone(), places: p.places.clone(), index })
    });
    let Some(ask) = ask else { return };
    if HERE.with(|h| *h.borrow()) {
        let found = ask.answer();
        alike_found(app, at, &ask.list, found);
        return;
    }
    ALIKE.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Sounds like this", move |ask: AlikeAsk| {
                let found = ask.answer();
                let AlikeAsk { at, list, .. } = ask;
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        alike_found(&app, at, &list, found);
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

/// Shows the sounds like the one at `at` (places in `list`), if it's still the one picked in that list: an answer for
/// a list read again since (its places may belong to other sounds now) was asked again for the new one.
fn alike_found(app: &AppWindow, at: usize, list: &SoundList, found: Vec<(usize, f64)>) {
    let _doing = crate::watchdog::doing("Sounds: show the sounds like this");
    let lines = PAGE.with(|p| {
        let p = p.borrow();
        if p.picked != Some(at) || !p.list.shares_with(list) {
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
                        sound_type: s.sound_type.as_str().into(),
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
        p.list = list.clone();
        Some(p.picked)
    });
    let Some(picked) = picked else { return };
    // Ctrl+K now knows which sounds you use most.
    crate::finder::add_source(app, "sounds", Arc::new(finder::Sounds(list)));
    refresh(app);
    // What Rediscover dug out didn't know your taste yet.
    if PAGE.with(|p| p.borrow().re_started) {
        rediscover_now(app);
    }
    if let Some(at) = picked {
        let data = app.global::<SoundsData>();
        PAGE.with(|p| {
            let p = p.borrow();
            data.set_picked(picked_sound(&p, at));
            show_picked_tags(&data, &p);
        });
    }
}

/// The sounds read again once the app's copy of the library was brought up to date: a [`Library`] to show in place of
/// the one on show (`was`, the list it was compared with), and where each sound of that one is in it (`moved`, by
/// place; `None` when they're the same sounds in the same order). `retyped`: the same sounds in the same order, one of
/// them given another type.
struct Refreshed {
    library: Library,
    was: Arc<SoundList>,
    moved: Option<Arc<Vec<Option<usize>>>>,
    retyped: bool,
}

/// The sounds were brought up to date with what the C# app changed while the page showed them as they were: they
/// take the place of the ones on show, with nothing moving that needn't. The view, filters, order and scroll stay;
/// what the page holds by place in the list (what's shown, History, picked, playing, Rediscover, Sounds like this)
/// moves with its sound to its new place. A sound gone from the library goes from all of these: the panel closes if
/// it was picked, and it stops if it was playing. Rediscover keeps the sounds it dug out (it isn't asked again unless
/// its answer was still on its way, for the old places).
fn brought_up_to_date(app: &AppWindow, r: Refreshed) {
    let _doing = crate::watchdog::doing("Sounds: the sounds were brought up to date");
    let Refreshed { library, was, moved, retyped } = r;
    // Only for the list it was compared with, or one sharing its sounds (the same list with your songs' use counted
    // in): nothing else gives the page other sounds, so this only guards against a mix-up.
    if !PAGE.with(|p| p.borrow().list.shares_with(&was)) {
        return;
    }
    drop(was);
    let (after, list) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let after = take_refreshed(&mut p, library, moved.as_deref().map(Vec::as_slice), retyped);
        (after, p.list.clone())
    });
    crate::finder::add_source(app, "sounds", Arc::new(finder::Sounds(list)));
    refresh(app);
    show_rediscover_chips(app);
    kit::show(app);
    let data = app.global::<SoundsData>();
    if after.gone.picked {
        data.set_panel_shown(false);
        data.set_alike(ModelRc::default());
    } else if let Some(at) = after.picked {
        PAGE.with(|p| {
            let p = p.borrow();
            data.set_picked(picked_sound(&p, at));
            show_picked_tags(&data, &p);
        });
        // The lines shown move with their sounds meanwhile; an answer still on its way was for the old list.
        if let Some(moved) = moved.as_deref() {
            let lines: Vec<AlikeLine> = data
                .get_alike()
                .iter()
                .filter_map(|mut line| {
                    let at = moved.get(usize::try_from(line.place).ok()?).copied().flatten()?;
                    line.place = i32::try_from(at).ok()?;
                    Some(line)
                })
                .collect();
            data.set_alike(ModelRc::new(VecModel::from(lines)));
        }
        ask_alike(app, at);
    }
    if after.rediscover_again {
        rediscover_now(app);
    }
    if after.gone.playing {
        stop(app);
    } else {
        redraw();
    }
    if after.galaxy_again {
        galaxy::shown(app);
    }
}

/// What the page does once [`take_refreshed`] has put the sounds read again in place: what needs the window.
#[derive(Debug, Default, PartialEq)]
struct After {
    /// The picked sound's place now, when it's still there: what sounds like it is asked again.
    picked: Option<usize>,
    /// Whether the picked sound, or the one playing, is gone.
    gone: Gone,
    /// Rediscover's answer was on its way, for the old places: it's asked again for the new ones.
    rediscover_again: bool,
    /// The galaxy is on show and its map was let go: it's read again.
    galaxy_again: bool,
}

/// Puts the sounds read again (`library`) in place of the ones on show, moving what the page holds by place with its
/// sound (`moved`: each old place's new one; `None` when every sound kept its place). `retyped`: a sound has another
/// type, so the galaxy, which colours its stars and names its neighbourhoods by type, is laid out again.
fn take_refreshed(p: &mut Page, library: Library, moved: Option<&[Option<usize>]>, retyped: bool) -> After {
    p.list = library.list;
    p.places = library.places;
    p.types = library.types;
    p.tags = library.tags;
    // History lists them once the filters have been through them again ([`listed`], asked for by the page next).
    p.played = Arc::new(library.played);
    let gone = moved.map(|moved| move_places(p, moved)).unwrap_or_default();
    let anew = moved.is_some() || retyped;
    if anew {
        // The stars are kept by place, each coloured by its sound's type: laid out again for the new list.
        galaxy::forget(&mut p.galaxy);
    } else {
        // What the galaxy worked out from the old list (how lit each star is) is let go, and the old list with it:
        // it's worked out again for the new one when the galaxy's next drawn.
        galaxy::list_changed(&mut p.galaxy);
    }
    After {
        picked: p.picked,
        gone,
        rediscover_again: moved.is_some() && p.re_started && p.re_busy,
        galaxy_again: anew && p.view == GALAXY,
    }
}

/// What [`move_places`] found gone from the library.
#[derive(Debug, Default, PartialEq)]
struct Gone {
    picked: bool,
    playing: bool,
}

/// Moves what the page holds by place in its list to the places the same sounds have in a list read again (`moved`:
/// each old place's new one, `None` for a sound that's gone), leaving out the gone ones.
fn move_places(p: &mut Page, moved: &[Option<usize>]) -> Gone {
    let to = |at: usize| moved.get(at).copied().flatten();
    p.shown = Arc::new(p.shown.iter().filter_map(|&at| to(at)).collect());
    p.history = std::mem::take(&mut p.history).into_iter().filter_map(|(at, when)| Some((to(at)?, when))).collect();
    p.rediscover = std::mem::take(&mut p.rediscover).into_iter().filter_map(|(at, why)| Some((to(at)?, why))).collect();
    p.to_pick = p.to_pick.and_then(to);
    let gone = Gone {
        picked: p.picked.is_some_and(|at| to(at).is_none()),
        playing: p.playing.is_some_and(|(at, _)| to(at).is_none()),
    };
    p.picked = p.picked.and_then(to);
    p.playing = p.playing.and_then(|(at, seconds)| Some((to(at)?, seconds)));
    galaxy::places_moved(&mut p.galaxy, to);
    gone
}

fn loaded(app: &AppWindow, read: Result<Box<Read>, String>) {
    let _doing = crate::watchdog::doing("Sounds: the sounds were read");
    let data = app.global::<SoundsData>();
    let read = match read {
        Ok(read) => *read,
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
        p.galaxy.db = Some(read.source);
        p.re.count = count;
        p.kit.export_folder = read.export_folder;
        p.kit.drum_machine = read.drum_machine;
        p.step = Duration::from_millis(list::step_ms(read.step_ms.as_deref()));
    });
    show(app, read.library, read.view.as_deref(), read.autoplay.as_deref());
}

/// Shows a `library` on the page, opening on the view and autoplay setting given.
pub fn show(app: &AppWindow, library: Library, view: Option<&str>, autoplay: Option<&str>) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // "Reading your sounds…" stays until the list of them is worked out, so "No sounds match" never flashes up
        // in between ([`listed`]).
        p.read = true;
        // History lists them once the filters have been through them ([`listed`]).
        p.played = Arc::new(library.played);
        p.history.clear();
        p.list = library.list;
        p.to_pick = None;
        p.places = library.places;
        p.types = library.types;
        p.tags = library.tags;
        // Counted afresh for these sounds: the choices show without numbers until then.
        p.counts = None;
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
    let list = PAGE.with(|p| p.borrow().list.clone());
    crate::finder::add_source(app, "sounds", Arc::new(finder::Sounds(list)));
    refresh(app);
    show_rediscover_chips(app);
    kit::show(app);
    match PAGE.with(|p| p.borrow().view) {
        REDISCOVER => rediscover_now(app),
        GALAXY => galaxy::shown(app),
        _ => {}
    }
}

/// What the picked sound offers besides Play: (its words, its id in the shortcut list). The card shows them as
/// buttons after Play (Solomon's F12 note, 2026-11-02 03:28) and the right-click menu lists them under Play, from this
/// one list so the two can't drift apart.
const CARD_ACTIONS: &[(&str, &str)] =
    &[("Send to Bitwig", TO_BITWIG), ("Show in folder", "sounds.reveal"), ("Copy path", "sounds.copyPath")];

/// Send to Bitwig's id in the shortcut list (the C# app's).
const TO_BITWIG: &str = "sounds.toBitwig";

/// A sound is on its way to Bitwig: Send to Bitwig waits meanwhile, as it did in the C# app, so a second press doesn't
/// put a second track into the project.
static SENDING: AtomicBool = AtomicBool::new(false);

/// The card's buttons and menu again, with keys changed in Settings.
pub fn show_keys(app: &AppWindow, shortcuts: &Shortcuts) {
    show_actions(&app.global::<SoundsData>(), shortcuts);
}

/// The card's buttons and the right-click menu, with each one's key and tooltip from the shortcut list (rule 15).
fn show_actions(data: &SoundsData, shortcuts: &Shortcuts) {
    // Play has the app-wide key, but the page plays it itself (`on_menu_chosen`). A line with no words is the rule
    // under it.
    let play = MenuItem {
        what: "Play or stop".into(),
        key: shortcuts.key_of("app.play").into(),
        id: "play".into(),
        enabled: true,
    };
    let mut menu = vec![play, MenuItem { enabled: true, ..MenuItem::default() }];
    let mut actions = Vec::new();
    for &(text, id) in CARD_ACTIONS {
        let key = SharedString::from(shortcuts.key_of(id));
        menu.push(MenuItem { what: text.into(), key: key.clone(), id: id.into(), enabled: true });
        actions.push(CardAction { text: text.into(), tip: what_of(id).into(), key, id: id.into(), enabled: true });
    }
    data.set_menu(ModelRc::new(VecModel::from(menu)));
    data.set_actions(ModelRc::new(VecModel::from(actions)));
    data.set_play_tip("Play or stop the picked sound".into());
    data.set_play_key(shortcuts.key_of("app.play").into());
}

fn wire(app: &AppWindow, feed: Feed, shortcuts: &Shortcuts) {
    FEED.with(|f| *f.borrow_mut() = Some(feed));
    let data = app.global::<SoundsData>();
    data.set_tile_rows(ModelRc::from(TILE_ROWS.with(|m| m.clone()) as Rc<dyn Model<Data = TileRow>>));
    data.set_lines(ModelRc::from(LINES.with(|m| m.clone()) as Rc<dyn Model<Data = SoundLine>>));
    data.set_views(strings(VIEWS));
    data.set_orders(strings(Order::ALL.map(Order::name)));
    show_actions(&data, shortcuts);
    // The filters whose choices don't come from the sounds (length, key, tempo) show from the start, without numbers
    // until the sounds are read, so the rows don't jump when they are.
    show_chips(app);

    let weak = app.as_weak();
    data.on_search_edited(move |text| change(&weak, |f| f.search = text.to_string()));
    let weak = app.as_weak();
    data.on_toggle_choice(move |chip, choice| change(&weak, |f| f.pick(&chip, &choice)));
    let weak = app.as_weak();
    data.on_clear_chip(move |chip| {
        if chip == "more-types" {
            // The More types menu's Any: the types on chips of their own stay picked.
            let types = PAGE.with(|p| p.borrow().types.clone());
            change(&weak, |f| f.clear_more_types(&types));
        } else {
            change(&weak, |f| f.clear_chip(&chip));
        }
    });
    let weak = app.as_weak();
    data.on_toggle(move |kind| change(&weak, |f| f.toggle(&kind)));
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
    grid::connect(app);
    kit::wire(app);
    galaxy::wire(app);
    drag_out::wire(app);
    let weak = app.as_weak();
    data.on_columns_changed(move |n| {
        let Some(app) = weak.upgrade() else { return };
        let n = n.max(1) as usize;
        let changed = PAGE.with(|p| std::mem::replace(&mut p.borrow_mut().columns, n) != n);
        if changed {
            TILE_ROWS.with(|m| m.notify.reset());
            grid::changed();
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
    data.on_step(move |d, held| {
        if let Some(app) = weak.upgrade() {
            step(&app, d, held);
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
    let _doing = crate::watchdog::doing("Sounds: show Rediscover");
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

/// Each year on the year strip, newest first, with how many sounds it holds.
type YearCounts = Vec<(i32, usize)>;

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
        Ask {
            id: p.asked,
            list: p.list.clone(),
            filters: p.filters.clone(),
            order: p.order,
            seed: p.seed,
            played: p.played.clone(),
        }
    });
    if HERE.with(|h| *h.borrow()) {
        let (shown, counts) = ask.list.view_counted(&ask.filters, ask.order, ask.seed);
        let years = ask.list.year_counts(ask.filters.used);
        let history = ask.list.played(&ask.played, &ask.filters);
        listed(app, ask.id, Listed { shown, counts, years, history });
        return;
    }
    WORKER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            // The year strip's counts change only with the sounds and the Used chip, not with each search or other
            // filter: kept here (only this worker uses them) and counted again only when either changes. The list
            // they were counted from is known by a weak hold: it never keeps an old list alive (a big one takes a
            // while to free), and while it's held no new list can be given the same place in memory, so a new list is
            // never taken for the old one.
            let mut counted: Option<(Weak<SoundList>, bool, YearCounts)> = None;
            match Newest::start("Sounds list", move |ask: Ask| {
                let kept = counted
                    .as_ref()
                    .filter(|(list, used, _)| {
                        std::ptr::eq(list.as_ptr(), Arc::as_ptr(&ask.list)) && *used == ask.filters.used
                    })
                    .map(|(_, _, years)| years.clone());
                let new = background::on_pool(|| {
                    let years = kept.unwrap_or_else(|| ask.list.year_counts(ask.filters.used));
                    let history = ask.list.played(&ask.played, &ask.filters);
                    let (shown, counts) = ask.list.view_counted(&ask.filters, ask.order, ask.seed);
                    Listed { shown, counts, years, history }
                });
                counted = Some((Arc::downgrade(&ask.list), ask.filters.used, new.years.clone()));
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        listed(&app, ask.id, new);
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

/// What the filters let through, worked out off the window's thread.
struct Listed {
    /// For Tiles, List and the galaxy, as places in the page's list, in order.
    shown: Vec<usize>,
    /// How many sounds each filter's choices would show.
    counts: SoundCounts,
    /// The year strip's years.
    years: YearCounts,
    /// For History: the sounds played, newest first, with when.
    history: Vec<(usize, NaiveDateTime)>,
}

/// A new list has been worked out: shows it, unless a newer one has been asked for since.
fn listed(app: &AppWindow, id: u64, new: Listed) {
    let _doing = crate::watchdog::doing("Sounds: show the filtered list");
    let current = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.asked != id {
            return false;
        }
        p.shown = Arc::new(new.shown);
        p.counts = Some(new.counts);
        p.years = new.years;
        p.history = new.history;
        true
    });
    if current {
        // The counts beside the filters' choices, for the filters as they are now.
        show_chips(app);
        show_years(app);
        show_lists(app);
        // Once the sounds are read, the list (or "No sounds match") takes the place of the word saying so.
        if PAGE.with(|p| p.borrow().read) {
            app.global::<SoundsData>().set_empty_title("".into());
        }
        // The filters dim the stars they leave out.
        galaxy::draw(app);
        if let Some(at) = PAGE.with(|p| p.borrow_mut().to_pick.take()) {
            pick(app, at);
        }
    }
}

/// Shows one of the views by its name (Ctrl+K's "Sounds › Galaxy"), as the view switch does.
pub fn show_view(app: &AppWindow, name: &str) {
    if let Some(v) = VIEWS.iter().position(|n| *n == name) {
        app.global::<SoundsData>().invoke_pick_view(v as i32);
    }
}

/// Picks the sound at `at` in the list and brings it into view (Ctrl+K). A view that only shows some sounds (History,
/// Rediscover, Kits) gives way to Tiles, and filters that hide it are cleared; the search box with them.
fn show_sound(app: &AppWindow, at: usize) {
    let view = PAGE.with(|p| p.borrow().view);
    if matches!(view, HISTORY | REDISCOVER | KITS) {
        show_view(app, VIEWS[TILES as usize]);
    }
    let hidden = PAGE.with(|p| !p.borrow().shown.contains(&at));
    if !hidden {
        pick(app, at);
        return;
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.filters = Filters::default();
        p.to_pick = Some(at);
    });
    app.global::<SoundsData>().set_search(SharedString::new());
    refresh(app);
}

/// The year strip: each year, how full it is, and whether the filter has it.
fn show_years(app: &AppWindow) {
    let (bars, (from, to)) = PAGE.with(|p| {
        let p = p.borrow();
        let most = p.years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        let bars = p
            .years
            .iter()
            .map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most })
            .collect::<Vec<_>>();
        (bars, year_strip::picked_places(&p.years, p.filters.years).unwrap_or((-1, -1)))
    });
    let data = app.global::<SoundsData>();
    data.set_years(ModelRc::new(VecModel::from(bars)));
    data.set_years_from(from);
    data.set_years_to(to);
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
        } else if p.view == HISTORY
            && (Filters { years: None, ..p.filters.clone() }.any() || !p.filters.search.trim().is_empty())
        {
            // The years picked on the year strip don't narrow History, so they don't count here either.
            format!("{} of {} played", thousands(p.history.len()), thousands(p.played.len()))
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
    grid::changed();
    LINES.with(|m| m.notify.reset());
    kit::redraw();
}

/// The filters' chips and menus as the filters are now, each choice with the counts from the newest list worked out
/// (a pick shows at once; its counts come with the list it asked for).
fn show_chips(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let f = &p.filters;
        let chips = list::chips(f, p.counts.as_ref(), &p.types, &p.tags);
        FILTER_MODELS.with(|m| {
            data.set_type_filter(m[0].view(&chips.types));
            data.set_length_filter(m[1].view(&chips.length));
            data.set_key_filter(m[2].view(&chips.key));
            data.set_tempo_filter(m[3].view(&chips.tempo));
            data.set_tag_filter(m[4].view(&chips.tag));
        });
        data.set_any_type(!f.sound_types.is_empty());
        data.set_mix_well(f.mix_well);
        data.set_unused(f.unused);
        data.set_used(f.used);
        data.set_knows_use(p.list.knows_use());
        data.set_favourites(f.favourites);
        data.set_any_filter(f.any());
        data.set_order(p.order.name().into());
        data.set_autoplay(p.autoplay);
        show_picked_tags(&data, &p);
    });
}

/// The picked sound's tags on its panel, each lit while the Tag filter has it.
fn show_picked_tags(data: &SoundsData, p: &Page) {
    let tags = p.picked.and_then(|at| p.list.sound(at)).map(|s| s.tags.as_slice()).unwrap_or_default();
    let shown: Vec<Shown> = tags
        .iter()
        .map(|t| Shown {
            text: t.clone(),
            count: String::new(),
            on: p.filters.tags.iter().any(|picked| picked.eq_ignore_ascii_case(t)),
        })
        .collect();
    let view = PICKED_TAGS.with(|m| m.view(&Filter { chips: shown, ..Filter::default() }));
    data.set_picked_tags(view.chips);
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
    PAGE.with(|p| {
        p.borrow_mut().picked = Some(at);
        let p = p.borrow();
        data.set_picked(picked_sound(&p, at));
        show_picked_tags(&data, &p);
    });
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
        sound_type: s.sound_type.as_str().into(),
        colour: list::type_colour(&s.sound_type),
        type_from: type_from.into(),
        path: mo_app::shell::wrappable_path(&s.path).into(),
        wave: wave(s).into(),
        points: points(s),
        seconds: a.map_or(0.0, |a| a.seconds.min(mo_audio::MAX_SECONDS)) as f32,
        facts: ModelRc::new(VecModel::from(facts)),
        favourite: s.favourite,
        character: a.map(|a| a.character.join(", ")).unwrap_or_default().into(),
        liked_why: liked_why(p, at).into(),
    }
}

/// The arrow keys: across a row of tiles (-1, 1) or down and up the list (-2, 2), a row of tiles at a time.
fn step(app: &AppWindow, d: i32, held: bool) {
    let next = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // A held key's repeats wait for the sound they're on to have played (with Autoplay on).
        let (autoplay, step) = (p.autoplay, p.step);
        if !p.stepping.may_step(autoplay, held, step, Instant::now()) {
            return None;
        }
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
    if let Some(at) = PAGE.with(|p| p.borrow().picked) {
        play_sound(app, at, from, mo_audio::MAX_SECONDS);
    }
}

/// Plays the sound at `at` in the list from `from` (0 to 1 of the way through), whether it's picked or not (the
/// galaxy's sweep plays the stars it passes without picking them).
/// Plays the sound at `at` from `from` (0 to 1 of the way through), reading at most `most` seconds of it: a sweep
/// only needs its start, which reads far quicker than a whole loop.
fn play_sound(app: &AppWindow, at: usize, from: f64, most: f64) {
    let Some((at, path, seconds)) = PAGE.with(|p| {
        let p = p.borrow();
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
    NEWEST_PLAY.fetch_max(ask, Ordering::SeqCst);
    let job = PlayJob { ask, path, from, seconds, most, window: app.as_weak(), held: Arc::default() };
    ask_reader(job);
}

/// How many sounds may be read at once. Plays asked while every reader is busy replace each other (rule 12), so
/// after a held arrow key or a fast sweep only the newest is read next and the ones passed over are never opened;
/// a second reader means one slow read (a sleeping drive, a long file) never holds up the next sound.
const READERS: usize = 2;

thread_local! {
    /// The threads that read each sound before it plays, each with how many of its jobs are still held (reading or
    /// waiting); started as they're needed (and tried again at the next play if the system wouldn't start one).
    static SOUND_READERS: RefCell<Vec<(Newest<PlayJob>, Arc<AtomicUsize>)>> = const { RefCell::new(Vec::new()) };
}

/// Gives `job` to a reader that's free, or else to the last one, in place of the read it has waiting.
fn ask_reader(mut job: PlayJob) {
    SOUND_READERS.with(|r| {
        let mut readers = r.borrow_mut();
        let free = readers.iter().position(|(_, held)| held.load(Ordering::SeqCst) == 0);
        let at = match free {
            Some(at) => at,
            None if readers.len() < READERS => match Newest::start("Read sound", read_and_play) {
                Ok(reader) => {
                    readers.push((reader, Arc::default()));
                    readers.len() - 1
                }
                Err(e) if readers.is_empty() => {
                    // Never opened: the sweep's timings forget it by themselves.
                    log::error("Sounds", &format!("Couldn't start reading the sound: {e}"));
                    return;
                }
                Err(_) => readers.len() - 1,
            },
            None => readers.len() - 1,
        };
        let (reader, held) = &readers[at];
        held.fetch_add(1, Ordering::SeqCst);
        job.held = held.clone();
        reader.ask(job);
    });
}

/// A sound to read and play: play number `ask`, `from` this far through its `seconds`, reading at most `most`.
struct PlayJob {
    ask: u64,
    path: String,
    from: f64,
    seconds: f64,
    most: f64,
    window: slint::Weak<AppWindow>,
    /// How many of its reader's jobs are held: one more while this one is, so the reader is free once it's none.
    held: Arc<AtomicUsize>,
}

impl Drop for PlayJob {
    // Done, passed over (replaced while waiting) or broken: no longer held by its reader.
    fn drop(&mut self) {
        self.held.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Reads one sound and plays it, if it's still the newest play when the read ends. The sweep's timings
/// (`mo_app::sweep_timing`) hear when it's being opened, and when it's handed to the output, overtaken by a newer sound
/// or dropped, by this play's number; for a sound the sweep didn't ask for they count nothing.
fn read_and_play(job: PlayJob) {
    let PlayJob { ask, ref path, from, seconds, most, ref window, .. } = job;
    let (path, window) = (path.clone(), window.clone());
    let overtaken = || NEWEST_PLAY.load(Ordering::SeqCst) != ask;
    let stopped = || PLAYS.load(Ordering::SeqCst) != ask;
    // Passed over since it was asked for (it waited behind another read): never opened, so the sweep's timings
    // forget it by themselves.
    if overtaken() || stopped() {
        return;
    }
    sweep_timing::opening(&path, ask);
    let read = mo_audio::read(Path::new(&path), most);
    match read {
        // A newer play came while this one was reading: overtaken. A stop (the mouse let go, say): dropped.
        _ if overtaken() => sweep_timing::overtaken(&path, ask),
        _ if stopped() => sweep_timing::dropped(&path, ask),
        Ok(audio) => {
            let player = player();
            player.play(Arc::new(audio), path.clone(), from * seconds);
            let output = player.info();
            sweep_timing::output(output.mix_every_ms.map(|m| m / 2.0), output.delay_ms);
            sweep_timing::started(&path, ask);
        }
        Err(e) => {
            sweep_timing::dropped(&path, ask);
            log::warning("Sounds", &format!("Couldn't play {path}: {e}"));
            let why = e.to_string();
            let _ = slint::invoke_from_event_loop(move || {
                // Only while this is still the sound asked for: a newer one asked meanwhile keeps playing.
                if PLAYS.load(Ordering::SeqCst) != ask {
                    return;
                }
                // Stepping or sweeping over many sounds that won't play for the same reason (a drive unplugged)
                // says so once; the log has each of them.
                let now = Instant::now();
                let new_card = FAILED_CARD.with(|c| {
                    let mut c = c.borrow_mut();
                    let fresh = c
                        .as_ref()
                        .is_none_or(|(at, before)| *before != why || now.duration_since(*at) >= FAILED_CARD_GAP);
                    if fresh {
                        *c = Some((now, why.clone()));
                    }
                    fresh
                });
                if new_card && let Some(feed) = FEED.with(|f| f.borrow().clone()) {
                    feed.start("Couldn't play the sound", "").failed(&why);
                }
                if let Some(app) = window.upgrade() {
                    stop(&app);
                }
            });
        }
    }
}

/// The least time between two "Couldn't play the sound" cards for the same reason: as long as one stays.
const FAILED_CARD_GAP: Duration = mo_app::activity::FAILED_STAYS;

thread_local! {
    /// When the last "Couldn't play the sound" card went up, and why the sound couldn't play.
    static FAILED_CARD: RefCell<Option<(Instant, String)>> = const { RefCell::new(None) };
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
        p.picked.and_then(|at| p.list.sound(at)).map(|s| (s.path.clone(), s.name().to_owned()))
    });
    let feed = FEED.with(|f| f.borrow().clone());
    match (id, picked, feed) {
        ("sounds.stop", _, _) => {
            if PAGE.with(|p| p.borrow().playing.is_none()) {
                return false;
            }
            stop(app);
        }
        (TO_BITWIG, Some((path, name)), Some(feed)) => send_to_bitwig(app, &feed, path, name),
        ("sounds.reveal", Some((path, _)), Some(feed)) => {
            crate::files::reveal(&feed, &path, mo_app::shell::folder_of(&path));
        }
        ("sounds.copyPath", Some((path, _)), Some(feed)) => crate::files::copy_path(&feed, &path),
        _ => return false,
    }
    true
}

/// Puts a sound on a Sampler on a new track in the project open in Bitwig, on a thread of its own, as its file name
/// (without the extension) names it. Its card shows at once and says how it went. While it goes, Send to Bitwig waits.
fn send_to_bitwig(app: &AppWindow, feed: &Feed, path: String, name: String) {
    if SENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    show_sending(app, true);
    let job = to_bitwig::sound_card(feed, &name);
    let weak = app.as_weak();
    let sent = background::spawn("Send to Bitwig", move || {
        // A panic in it is logged as a crash and fails the card as it goes; Send to Bitwig still comes back.
        let to = to_bitwig::Bitwig::default();
        let _ = mo_catch::catch_crash(AssertUnwindSafe(|| to_bitwig::send_sound(job, &to, &path, &name)));
        SENDING.store(false, Ordering::Release);
        let _ = weak.upgrade_in_event_loop(|app| show_sending(&app, false));
    });
    // Its card was taken into the thread, so it fails by itself ("It stopped before it finished").
    if let Err(e) = sent {
        log::error("Sounds", &format!("Couldn't start sending to Bitwig: {e}"));
        SENDING.store(false, Ordering::Release);
        show_sending(app, false);
    }
}

/// Send to Bitwig on the card and on the right-click menu: greyed, saying "Sending…", while a sound is on its way.
fn show_sending(app: &AppWindow, sending: bool) {
    let data = app.global::<SoundsData>();
    let text = CARD_ACTIONS.iter().find(|a| a.1 == TO_BITWIG).map_or("", |a| a.0);
    let actions = data.get_actions();
    for i in 0..actions.row_count() {
        if let Some(mut a) = actions.row_data(i).filter(|a| a.id == TO_BITWIG) {
            a.enabled = !sending;
            a.text = if sending { "Sending…".into() } else { text.into() };
            actions.set_row_data(i, a);
        }
    }
    let menu = data.get_menu();
    for i in 0..menu.row_count() {
        if let Some(mut m) = menu.row_data(i).filter(|m| m.id == TO_BITWIG) {
            m.enabled = !sending;
            menu.set_row_data(i, m);
        }
    }
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
            type_source: Default::default(),
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

    /// A sound library made as the C# app makes it, with five sounds (each with the analyser's fingerprint) in a
    /// folder of its own, made afresh each run.
    fn sound_library(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("mo-ui-sounds-test").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(std::fs::create_dir_all(&dir).is_ok());
        let path = dir.join("sounds.db");
        let made = c_sharp(&path).execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER,
                 mtime INTEGER, error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT,
                 type TEXT, type_source TEXT, type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0,
                 plays INTEGER DEFAULT 0, last_played INTEGER);
             CREATE TABLE sound_tags (sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE, tag TEXT NOT NULL,
                 source TEXT NOT NULL DEFAULT 'you', UNIQUE(sound_id, tag));
             CREATE TABLE sound_events (sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE,
                 kind TEXT NOT NULL, at INTEGER NOT NULL);
             CREATE TABLE sound_hidden (sound_id INTEGER PRIMARY KEY REFERENCES sounds(id) ON DELETE CASCADE,
                 batch TEXT NOT NULL, at INTEGER NOT NULL);",
        );
        assert!(made.is_ok(), "{made:?}");
        for (id, name) in [(1, "kick"), (2, "pad"), (3, "hat"), (4, "snare"), (5, "clap")] {
            add_sound(&path, id, name);
        }
        path
    }

    /// The C# app's connection to its library: foreign keys on, as it has them.
    fn c_sharp(db: &Path) -> rusqlite::Connection {
        let Ok(c) = rusqlite::Connection::open(db) else { panic!("couldn't open {}", db.display()) };
        assert!(c.execute_batch("PRAGMA foreign_keys = ON;").is_ok());
        c
    }

    /// The C# app's scan finding `C:\s\{name}.wav`, with a fingerprint of its own.
    fn add_sound(db: &Path, id: i64, name: &str) {
        let features: Vec<u8> = (0..73).flat_map(|i| ((id * 31 + i) as f32 / 97.0).sin().to_le_bytes()).collect();
        let added = c_sharp(db).execute(
            "INSERT INTO sounds (id, path, size, mtime, features, name_type, type, type_source, type_conf)
             VALUES (?1, ?2, 1000, 1780000000, ?3, 'Kick', 'Kick', 'name', 1)",
            rusqlite::params![id, format!("C:\\s\\{name}.wav"), features],
        );
        assert!(added.is_ok(), "{added:?}");
    }

    /// Spoils the first page of `table` in the database at `db`, as a bad sector would.
    fn damage_first_page(db: &Path, table: &str) {
        use std::io::{Seek, SeekFrom, Write};
        let c = c_sharp(db);
        let root: Result<i64, _> =
            c.query_row("SELECT rootpage FROM sqlite_master WHERE name = ?1", [table], |r| r.get(0));
        let size: Result<i64, _> = c.query_row("PRAGMA page_size", [], |r| r.get(0));
        drop(c);
        let (Ok(root), Ok(size)) = (root, size) else { panic!("no {table} in {}", db.display()) };
        let Ok(mut f) = std::fs::File::options().write(true).open(db) else { panic!("couldn't open {}", db.display()) };
        let at = u64::try_from((root - 1) * size).unwrap_or_default();
        assert!(f.seek(SeekFrom::Start(at)).is_ok() && f.write_all(&[0xEE; 64]).is_ok());
    }

    /// What one start of the page brings back from `sounds_db`, in order, in words.
    fn one_start(sounds_db: &Path, library_db: &Path, cards: &Feed) -> Vec<String> {
        let got = std::sync::Mutex::new(Vec::new());
        read_all(sounds_db, library_db, Some(cards.clone()), |step| {
            let said = match step {
                Step::Read(Ok(read)) => {
                    let plays: i64 = read.library.list.sounds().iter().map(|s| i64::from(s.plays)).sum();
                    let from = read.source.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    format!("{} sounds from {from}, {plays} plays", read.library.list.len())
                }
                Step::Read(Err(why)) => format!("couldn't read: {why}"),
                Step::Index(index) => format!("{} fingerprints", index.features.len()),
                Step::Used(list) => format!("used: {}", list.used_in(0).is_some()),
                Step::Refreshed(r) => {
                    let plays: i64 = r.library.list.sounds().iter().map(|s| i64::from(s.plays)).sum();
                    let same = match (&r.moved, r.retyped) {
                        (Some(_), _) => "other sounds",
                        (None, true) => "the same sounds, retyped",
                        (None, false) => "the same sounds",
                    };
                    format!(
                        "again: {same}, {} sounds, {plays} plays, used: {}",
                        r.library.list.len(),
                        r.library.list.knows_use()
                    )
                }
                Step::Source(source) => format!("the rest from {}", source.display()),
            };
            if let Ok(mut got) = got.lock() {
                got.push(said);
            }
        });
        got.into_inner().unwrap_or_default()
    }

    /// The cards' titles and lines, as the feed shows them now.
    fn cards_said(cards: &Feed) -> Vec<String> {
        cards.visible(Instant::now()).iter().map(|c| format!("{}: {}", c.title, c.detail)).collect()
    }

    #[test]
    fn the_page_reads_the_apps_copy_and_brings_it_up_to_date() {
        let sounds_db = sound_library("copy");
        let library_db = song_library("copy-songs.db", &[]);
        let copy = sounds_db.with_file_name("sounds-rust.db");
        // The first start: sounds.db is read once, and the copy made on the same pass.
        let cards = Feed::default();
        let first = one_start(&sounds_db, &library_db, &cards);
        assert_eq!(first, ["5 sounds from sounds-rust.db, 0 plays", "5 fingerprints", "used: true"]);
        assert!(copy.exists());
        let said = cards_said(&cards);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("Made the app's own copy of your sound library: "), "{said:?}");
        // The next start reads the copy, and nothing needs bringing up to date.
        let cards = Feed::default();
        assert_eq!(one_start(&sounds_db, &library_db, &cards), first);
        assert_eq!(cards_said(&cards), Vec::<String>::new());
        // The C# app plays a sound: the page shows the copy as it was at once, then the sounds again in place.
        let played = c_sharp(&sounds_db)
            .execute_batch("UPDATE sounds SET plays = 2, last_played = 1780000000 WHERE id = 3; INSERT INTO sound_events VALUES (3, 'play', 1780000000);");
        assert!(played.is_ok(), "{played:?}");
        let cards = Feed::default();
        assert_eq!(
            one_start(&sounds_db, &library_db, &cards),
            [
                "5 sounds from sounds-rust.db, 0 plays",
                "5 fingerprints",
                "used: true",
                // No fingerprint changed, so they aren't read again.
                "again: the same sounds, 5 sounds, 2 plays, used: true",
            ]
        );
        assert_eq!(cards_said(&cards), ["Brought your sound library up to date: 1 changed"]);
        assert_eq!(one_start(&sounds_db, &library_db, &Feed::default()).len(), 3, "up to date since");
        // A scan finds a new sound: the library is shown again, with its fingerprint.
        add_sound(&sounds_db, 9, "rim");
        assert_eq!(
            one_start(&sounds_db, &library_db, &Feed::default()),
            [
                "5 sounds from sounds-rust.db, 2 plays",
                "5 fingerprints",
                "used: true",
                "again: other sounds, 6 sounds, 2 plays, used: true",
                "6 fingerprints",
            ]
        );
        // The copy's fingerprints damaged (a bad sector): it's made again from sounds.db, which gives them.
        damage_first_page(&copy, "sound_features");
        let cards = Feed::default();
        assert_eq!(
            one_start(&sounds_db, &library_db, &cards),
            ["6 sounds from sounds-rust.db, 2 plays", "6 fingerprints", "used: true"]
        );
        let said = cards_said(&cards);
        assert!(said.iter().any(|c| c.starts_with("Made the app's own copy of your sound library again")), "{said:?}");
        assert_eq!(one_start(&sounds_db, &library_db, &Feed::default())[1], "6 fingerprints", "made whole again");
        // A sound given another type in the C# app: the same sounds in place, the galaxy coloured again.
        assert!(c_sharp(&sounds_db).execute_batch("UPDATE sounds SET type = 'Snare' WHERE id = 1;").is_ok());
        assert_eq!(
            one_start(&sounds_db, &library_db, &Feed::default()),
            [
                "6 sounds from sounds-rust.db, 2 plays",
                "6 fingerprints",
                "used: true",
                "again: the same sounds, retyped, 6 sounds, 2 plays, used: true",
            ]
        );
        // Damaged while it's behind (the C# app played a sound since): the sounds come from making it again, with
        // nothing to bring up to date after.
        let played = c_sharp(&sounds_db).execute_batch(
            "UPDATE sounds SET plays = 3 WHERE id = 3; INSERT INTO sound_events VALUES (3, 'play', 1780000100);",
        );
        assert!(played.is_ok(), "{played:?}");
        damage_first_page(&copy, "sound_features");
        let cards = Feed::default();
        assert_eq!(
            one_start(&sounds_db, &library_db, &cards),
            [
                "6 sounds from sounds-rust.db, 2 plays",
                "6 fingerprints",
                "used: true",
                "again: the same sounds, 6 sounds, 3 plays, used: true",
            ]
        );
        let said = cards_said(&cards);
        assert!(said.iter().any(|c| c.starts_with("Made the app's own copy of your sound library again")), "{said:?}");
        assert!(!said.iter().any(|c| c.starts_with("Brought")), "{said:?}");
        assert_eq!(one_start(&sounds_db, &library_db, &Feed::default()).len(), 3, "up to date since");
        // Damaged, and it can't be made again (a folder in the way of the new copy): the rest comes from sounds.db,
        // as it does without a copy.
        let part = PathBuf::from(format!("{}.part-{}", copy.display(), std::process::id()));
        assert!(std::fs::create_dir_all(&part).is_ok());
        damage_first_page(&copy, "sound_features");
        let cards = Feed::default();
        assert_eq!(
            one_start(&sounds_db, &library_db, &cards),
            [
                "6 sounds from sounds-rust.db, 3 plays".to_owned(),
                format!("the rest from {}", sounds_db.display()),
                "6 fingerprints".to_owned(),
                "used: true".to_owned(),
            ]
        );
        let said = cards_said(&cards);
        assert!(
            said.iter().any(|c| c.starts_with("Couldn't make the app's own copy of your sound library")),
            "{said:?}"
        );
        assert!(std::fs::remove_dir(&part).is_ok());
        assert_eq!(one_start(&sounds_db, &library_db, &Feed::default())[0], "6 sounds from sounds-rust.db, 3 plays");
        assert!(copy.exists(), "made at the next start");
        // The copy can't be brought up to date (sounds.db isn't a library any more): the page keeps what it showed.
        let cards = Feed::default();
        assert!(c_sharp(&sounds_db).execute_batch("UPDATE sounds SET rating = 4 WHERE id = 1;").is_ok());
        assert!(c_sharp(&sounds_db).execute_batch("ALTER TABLE sounds RENAME TO gone;").is_ok());
        let kept = one_start(&sounds_db, &library_db, &cards);
        assert_eq!(kept.first().map(String::as_str), Some("6 sounds from sounds-rust.db, 3 plays"));
        assert!(!kept.iter().any(|k| k.starts_with("again")), "{kept:?}");
        let said = cards_said(&cards);
        assert!(said.iter().any(|c| c.starts_with("Couldn't bring your sound library up to date: ")), "{said:?}");
    }

    #[test]
    fn a_refresh_asks_again_only_what_was_on_its_way_for_the_old_places() {
        let library = |ids: &[i64]| {
            let sounds = ids.iter().map(|&id| sound(id, &format!("C:\\s\\{id}.wav"))).collect();
            Library::new(sounds, Vec::new(), &LikedWeights::default())
        };
        // Rediscover waiting on its answer, the galaxy on show, a sound picked.
        let waiting = || Page { re_started: true, re_busy: true, view: GALAXY, picked: Some(1), ..Page::default() };
        // The same sounds in the same places: the answer on its way is still right, and the galaxy keeps its stars.
        let mut p = waiting();
        assert_eq!(
            take_refreshed(&mut p, library(&[1, 2, 3]), None, false),
            After { picked: Some(1), ..After::default() }
        );
        assert_eq!(p.list.len(), 3);
        // A sound new at the front: what's held by place moves, Rediscover is asked again, the galaxy laid out again.
        let new_first = [Some(1), Some(2), Some(3)];
        let mut p = waiting();
        assert_eq!(
            take_refreshed(&mut p, library(&[9, 1, 2, 3]), Some(&new_first), false),
            After { picked: Some(2), rediscover_again: true, galaxy_again: true, ..After::default() }
        );
        // A sound given another type: the galaxy, coloured by type, is laid out again; nothing else is asked again.
        let mut p = waiting();
        assert_eq!(
            take_refreshed(&mut p, library(&[1, 2, 3]), None, true),
            After { picked: Some(1), galaxy_again: true, ..After::default() }
        );
        // Neither waiting on Rediscover nor showing the galaxy: only Sounds like this is asked again.
        let mut p = Page { picked: Some(1), ..Page::default() };
        assert_eq!(
            take_refreshed(&mut p, library(&[9, 1, 2, 3]), Some(&new_first), false),
            After { picked: Some(2), ..After::default() }
        );
    }

    #[test]
    fn what_the_page_holds_moves_with_its_sound() {
        // Sounds at places 0..5, read again with the sound at 2 gone and one new at the front.
        let moved = [Some(1), Some(2), None, Some(3), Some(4)];
        let at = NaiveDateTime::default();
        let mut p = Page {
            shown: Arc::new(vec![4, 2, 0]),
            history: vec![(2, at), (0, at)],
            rediscover: vec![(3, "forgotten".into()), (2, "gone".into())],
            picked: Some(3),
            to_pick: Some(1),
            playing: Some((4, 2.5)),
            ..Page::default()
        };
        assert_eq!(move_places(&mut p, &moved), Gone::default());
        assert_eq!(*p.shown, [4, 1]);
        assert_eq!(p.history, [(1, at)]);
        assert_eq!(p.rediscover, [(3, "forgotten".to_owned())]);
        assert_eq!((p.picked, p.to_pick, p.playing), (Some(3), Some(2), Some((4, 2.5))));
        // The picked and playing sounds gone: nothing is left pointing at the places they had.
        let mut p = Page { picked: Some(2), playing: Some((2, 1.0)), ..Page::default() };
        assert_eq!(move_places(&mut p, &moved), Gone { picked: true, playing: true });
        assert_eq!((p.picked, p.playing), (None, None));
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
            let used = lib.sample_usage(since, s.rules).map(|uses| usage_of(&list, &uses)).unwrap_or_default();
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
    }

    /// Each button on the card is a page action in the shortcut list, so it has a tooltip and a key, and its key and
    /// its line on the right-click menu run the same thing.
    #[test]
    fn every_card_button_is_in_the_shortcut_list() {
        for (text, id) in CARD_ACTIONS {
            let action = mo_app::shortcuts::SOUNDS_ACTIONS.iter().find(|a| a.id == *id);
            assert!(action.is_some_and(|a| !a.what.is_empty() && !a.default_key.is_empty()), "{text}");
        }
    }
}
