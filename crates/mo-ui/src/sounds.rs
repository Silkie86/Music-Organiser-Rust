//! Fills the Sounds page from the sound library and plays the picked sound. The library (`sounds.db`, the C# app's,
//! read-only) is read once on a background thread; each change of the filters or order is worked out on its own
//! low-priority thread, newest ask first (rule 12), across every core (rule 21). Tiles and lines are only made when
//! the list draws them, so 357,000 sounds cost what's on screen. Sounds are read and played off the window's thread.
//! Rediscover (unused sounds that fit your taste) is dug out on its own low-priority thread too.

mod bitwig;
mod copies;
mod did;
mod drag_out;
pub use drag_out::card_at_row;
mod galaxy;
mod grid;
mod kit;
mod like;
mod peaks;
mod sky;
mod yours;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak, mpsc};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use mo_app::activity::{Feed, Job};
use mo_app::background::{self, Newest};
use mo_app::filters::{self, Filter, Shown};
use mo_app::finder::{self, Kind};
use mo_app::places::Place;
use mo_app::shortcuts::{Shortcuts, what_of};
use mo_app::sound_notes::{Held, Holds, Noter};
use mo_app::sounds::{
    self as list, AUTOPLAY_KEY, Filters, Order, REDISCOVER_COUNT_KEY as COUNT_KEY, SoundCounts, SoundList, VIEW_KEY,
};
use mo_app::{log, sweep_timing, to_bitwig, year_strip};
use mo_audio::play::Player;
use mo_library::UsageRules;
use mo_library::songs::thousands;
use mo_sounds::compact::{self, PageRead};
use mo_sounds::index::{Gathered, SoundIndex};
use mo_sounds::liking::{LIKED_WEIGHTS_KEY, LikedWeights};
use mo_sounds::rediscover::{self, FolderUse};
use mo_sounds::usage::{self, Usage};
use mo_sounds::{Sound, SoundsDb, SoundsError};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer, TimerMode, VecModel};

use crate::filters::FilterModels;
use crate::finder::How;
use crate::{
    AlikeLine, AppKeys, AppWindow, CardAction, FolderLine, MenuItem, PickedSound, SoundFact, SoundLine, SoundTile,
    SoundsData, TileRow, UsedInLine, YearBar,
};
use mo_library::ProjectFile;

/// The C# app's Settings in `library.db` for which songs count as using a sound.
const USED_NEEDS_NOTES_KEY: &str = "used_in_needs_notes";
const USED_NOT_MUTED_KEY: &str = "used_in_not_muted_newest";

/// The views, in the order of the page's view buttons.
const TILES: i32 = 0;
const LIST: i32 = 1;
const GALAXY: i32 = 2;
const HISTORY: i32 = 3;
const REDISCOVER: i32 = 4;
const KITS: i32 = 5;
/// The views, in the switch's order. Ctrl+K offers each one as a page of its own.
pub const VIEWS: [&str; 6] = ["Tiles", "List", "Galaxy", "History", "Rediscover", "Kits"];

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
    /// The order turned round: a second click on its column's heading in the List view.
    reversed: bool,
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
    /// A sound was played while History was on show: what was played as History lists it until you come back to it
    /// (the sound you played stays under your pointer; [`did::take`]).
    history_behind: Option<Arc<Vec<(usize, NaiveDateTime)>>>,
    /// The plays since the sounds were read, in the order played, as they were noted: the sounds read again leave out
    /// those they hold already ([`did::read_again`]).
    noted: Vec<did::Noted>,
    /// Your say about sounds since they were read (a favourite, your own type, tags), in the order said: shown
    /// over the sounds at once ([`yours`]), and left out of the sounds read again once they hold it.
    said: Vec<yours::Said>,
    /// Keeps what's done with sounds, and your say about them, in the library ([`did`], [`yours`]); none for the
    /// screenshots.
    noter: Option<Arc<Noter>>,
    /// How many tiles fit side by side.
    columns: usize,
    /// Whether the list made last was the tiles (or the List): a list made again comes back where it was by
    /// itself (list.slint's `kept-y`), so only one made for the other view brings the picked sound into view.
    made_tiles: Option<bool>,
    /// While tiles made again are being laid out (from `list-made` until a turn later), how many columns they had
    /// before. They first count their columns from a page that isn't laid out yet (1 or 2 for a moment), then their
    /// own. Each count is taken at once, so the rows always fit the tiles' widths, but none of them turns the list to
    /// the picked sound: that lost where the tiles were kept. A turn later, the picked sound is brought into view only
    /// if the count is a new one (the window or the card changed width while the tiles were away).
    columns_before: Option<usize>,
    /// The picked sound, as its place in `list`.
    picked: Option<usize>,
    /// A sound Ctrl+K jumped to that the filters hid: picked once the list without them arrives.
    to_pick: Option<usize>,
    autoplay: bool,
    /// You picked a view, or turned Autoplay on or off: when the sounds arrive (or arrive again), the page keeps your
    /// choice rather than the one you had last ([`Page::open_on`]).
    chose_view: bool,
    chose_autoplay: bool,
    /// How long each sound plays while a held arrow key steps through them, and when the pick last moved.
    step: Duration,
    stepping: list::Stepping,
    /// Counts the asks for a new list, so an older answer that arrives late is dropped.
    asked: u64,
    /// The sounds have been read and handed to the page ([`show`]): the next list shown is of them.
    read: bool,
    /// There's no sound library, or it couldn't be read: no sounds are coming ([`loaded`]).
    unread: bool,
    /// Only the first few sounds are in `list`, shown at once while the rest are read: how many there are in all.
    /// Their list is the start of the whole one's ([`list::first_shuffled`]), so nothing on show moves when the rest
    /// arrive ([`Step::Rest`]).
    partial: Option<usize>,
    /// The type chips' types with their counts, in chip order, then the rest (the More types menu).
    types: Vec<(String, usize)>,
    /// How many of them the filter column shows as tick lines, the rest in More types: as many as it has room for,
    /// and never fewer than the usual [`list::TYPE_CHIPS`] ([`room`]).
    type_lines: usize,
    /// Every tag on a sound, for the Tag filter.
    tags: Vec<String>,
    /// How many sounds each filter's choices would show, from the newest list worked out (none before the first).
    counts: Option<SoundCounts>,
    /// What History's times are told by ("14:05" today, "Tue" this week): taken again each time the lists are shown
    /// ([`show_lists`]), so a page left open over midnight moves on with the days.
    now: NaiveDateTime,
    /// The sound playing (its place in `list`) and how long the part shown in its waveform lasts.
    playing: Option<(usize, f64)>,
    /// That play's number: the player knows it by that ([`ended`]), not by its file, so an older play of the same
    /// sound that ends meanwhile isn't taken for it.
    play: u64,
    /// How many seconds of it that play reads at most (a sweep reads only its start): a click on its waveform past
    /// that reads it again from there ([`seeks`]).
    play_reads: f64,
    /// That play was trimmed (Trim silence was on): its samples don't hold the whole file, so a click on its waveform
    /// plays it again from there rather than moving along what was read.
    play_trimmed: bool,
    /// Each sound's place in `list`, by id.
    places: Arc<HashMap<i64, usize>>,
    /// Every sound's fingerprint, once read, for Sounds like this and Rediscover.
    index: Option<Arc<SoundIndex>>,
    /// Why the fingerprints couldn't be read, when they couldn't: nothing waits for them then ([`index_failed`]).
    no_index: Option<String>,
    /// What Rediscover dug out, as places in `list` with why, best first.
    rediscover: Vec<(usize, String)>,
    /// What Rediscover digs out next time.
    re: rediscover::Options,
    /// How many types Rediscover's choices show as tick lines, as `type_lines` is for the Type filter (the column
    /// has other things under them there).
    re_type_lines: usize,
    /// Counts Rediscover's asks, so an older answer that arrives late is dropped.
    re_asked: u64,
    /// Rediscover has run on this library (so changing a choice picks again).
    re_started: bool,
    /// The answer to Rediscover's newest ask hasn't come yet.
    re_busy: bool,
    /// Rediscover's year strip, from its newest answer: each year, newest first, with how many sounds it holds.
    re_years: YearCounts,
    /// The sound each one Rediscover dug out sounds like, by id, when its line names one: the card lists that one's
    /// projects when none use it.
    re_likes: HashMap<i64, i64>,
    /// Hide duplicates has been set: by you, or from its setting when the sounds first showed.
    hide_copies_chosen: bool,
    /// How far the sounds like your favourites reach has been set: by you, or from its setting when the sounds first
    /// showed.
    like_chosen: bool,
    /// Trim silence is on: sounds play without the silence at their start and end ([`mo_audio::trim`]).
    trim: bool,
    /// The level Trim silence counts as silence, in dB below full scale (`mo_audio::trim::LEVELS_DB`).
    trim_db: i32,
    /// Trim silence has been set: by you, or from its setting when the sounds first showed.
    trim_chosen: bool,
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
            reversed: false,
            seed: new_seed(),
            view: TILES,
            shown: Arc::default(),
            years: Vec::new(),
            played: Arc::default(),
            history: Vec::new(),
            history_behind: None,
            noted: Vec::new(),
            said: Vec::new(),
            noter: None,
            columns: 4,
            made_tiles: None,
            columns_before: None,
            picked: None,
            to_pick: None,
            autoplay: false,
            chose_view: false,
            chose_autoplay: false,
            step: Duration::from_millis(list::STEP_MS),
            stepping: list::Stepping::default(),
            asked: 0,
            read: false,
            unread: false,
            partial: None,
            types: Vec::new(),
            type_lines: list::TYPE_CHIPS,
            tags: Vec::new(),
            counts: None,
            now: Local::now().naive_local(),
            playing: None,
            play: 0,
            play_reads: 0.0,
            play_trimmed: false,
            places: Arc::default(),
            index: None,
            no_index: None,
            rediscover: Vec::new(),
            re: rediscover::Options { count: COUNT, ..rediscover::Options::default() },
            re_type_lines: list::TYPE_CHIPS,
            re_asked: 0,
            re_started: false,
            re_busy: false,
            re_years: Vec::new(),
            re_likes: HashMap::new(),
            hide_copies_chosen: false,
            like_chosen: false,
            trim: false,
            trim_db: mo_audio::trim::DEFAULT_DB,
            trim_chosen: false,
            kit: kit::Kit::default(),
            galaxy: galaxy::Galaxy::default(),
        }
    }
}

impl Page {
    /// Opens on the view and Autoplay you had last (the settings `view` and `autoplay`), unless you chose them while
    /// the sounds were read.
    fn open_on(&mut self, view: Option<&str>, autoplay: Option<&str>) {
        if !self.chose_view {
            self.view = opening_view(view);
        }
        // As in the C# app: off unless it was turned on.
        if !self.chose_autoplay {
            self.autoplay = autoplay == Some("1");
        }
    }

    /// You picked view `v` (an index into [`VIEWS`]).
    fn pick_view(&mut self, v: i32) {
        self.view = v;
        self.chose_view = true;
        // Plays from History while it showed: listed now.
        if v == HISTORY && self.history_behind.take().is_some() {
            self.history = self.list.played(&self.played, &self.filters);
        }
    }

    /// Whether the picked sound is on none of the kit's pads: the Kits view's card is for the pad you click, so a
    /// sound picked in another view leaves it when Kits opens (the UX review of 10 Nov, E3).
    fn picked_off_the_kit(&self) -> bool {
        self.picked.is_some_and(|at| {
            !self.kit.slots.iter().filter_map(|s| s.sound).any(|id| self.places.get(&id) == Some(&at))
        })
    }

    /// You turned Autoplay on or off: on, it says so.
    fn toggle_autoplay(&mut self) -> bool {
        self.autoplay = !self.autoplay;
        self.chose_autoplay = true;
        self.autoplay
    }

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
    reversed: bool,
    seed: u64,
    played: Arc<Vec<(usize, NaiveDateTime)>>,
}

/// Each view's own choices, every one on show as chips (Solomon, 4 Nov 2026: "makes sure things that are useful arent
/// hidden away"): Tiles and List's order, the galaxy's Placed by and Light by, and Rediscover's type, year and how
/// many.
#[derive(Default)]
struct Choices {
    order: FilterModels,
    placing: FilterModels,
    light: FilterModels,
    sweep_play: FilterModels,
    re_type: FilterModels,
    re_count: FilterModels,
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    static TILE_ROWS: Rc<TileRows> = PAGE.with(|p| Rc::new(TileRows { page: p.clone(), notify: ModelNotify::default() }));
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
    static WORKER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    /// The filters' chips and menu lines (type, length, key, tag), changed in place so an open menu keeps its place.
    static FILTER_MODELS: [FilterModels; 4] = Default::default();
    /// The picked sound's tags on its panel.
    static PICKED_TAGS: FilterModels = FilterModels::default();
    /// Each view's own choices on show, changed in place as the filters' are.
    static CHOICES: Choices = Choices::default();
    static ALIKE: RefCell<Option<Newest<AlikeAsk>>> = const { RefCell::new(None) };
    static REDISCOVERING: RefCell<Option<Newest<RediscoverAsk>>> = const { RefCell::new(None) };
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
    /// Moves the picked tile's waveform along while it plays.
    static TICKER: Timer = Timer::default();
    /// Works out lists in place, for the screenshots, which have no event loop to send answers back to.
    static HERE: RefCell<bool> = const { RefCell::new(false) };
    /// Starts reading the sound library, until it has started ([`wanted`]).
    static START_READ: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// How long after the window opens the sounds are read when nothing has asked for them yet: the page on show reads
/// its own things from the disk first.
const READ_AFTER: Duration = Duration::from_secs(3);

/// Asks for the sample folders to be scanned again ([`sample_folders_changed`]), for the thread that reads the sounds:
/// it takes them once its first read and scan are done ([`scans`]).
static SCAN_AGAIN: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);

/// The sample folders changed in Settings (one was added): the page scans them again and takes the sounds it finds,
/// as at start (the whole-app review of 10 Nov, X2-4: they waited for the next start, and nothing said so). Asked
/// while the page's first read or a scan is still going, the folders are scanned once more after it, as they are
/// then. Whether they will be, for Settings' note: not when the sounds were never read (there's no sound library, or
/// these are the screenshots), nor when the page reads the old app's library rather than its own ([`scans`]).
pub fn sample_folders_changed() -> bool {
    SCAN_AGAIN.lock().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|again| again.send(()).is_ok())
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
/// The newest play handed to the output, and when (a Galaxy sweep's set length counts from then).
static STARTED: Mutex<Option<(u64, Instant)>> = Mutex::new(None);

/// When play `ask` was handed to the output, if it was the last one to be.
fn started_at(ask: u64) -> Option<Instant> {
    let started = *STARTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    started.filter(|&(a, _)| a == ask && ask > 0).map(|(_, at)| at)
}

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
    let Some(now) = p.list.now(at) else { return SoundTile::default() };
    let s = now.sound;
    SoundTile {
        row: row as i32,
        name: s.name().into(),
        facts: list::facts_line_now(&now).into(),
        colour: list::type_colour(now.sound_type),
        favourite: now.favourite,
        wave: wave(s).into(),
        points: points(s),
        picked: p.picked == Some(at),
        playing: p.playing.is_some_and(|(i, _)| i == at),
    }
}

fn line(p: &Page, row: usize, at: usize) -> SoundLine {
    let Some(now) = p.list.now(at) else { return SoundLine::default() };
    let s = now.sound;
    let a = s.analysis.as_ref();
    let used = p.list.used_in(at);
    SoundLine {
        row: row as i32,
        name: s.name().into(),
        sound_type: now.sound_type.into(),
        colour: list::type_colour(now.sound_type),
        key: list::key_text(s).into(),
        bpm: a.and_then(|a| a.bpm).map(list::bpm_text).unwrap_or_default().into(),
        length: a.map(|a| list::length_text(a.seconds)).unwrap_or_default().into(),
        used: list::used_text(used).into(),
        used_words: list::used_words(used).into(),
        folder: s.folder().into(),
        when: p.history.get(row).filter(|_| p.view == HISTORY).map(|h| when(h.1, p.now)).unwrap_or_default().into(),
        reason: p.rediscover.get(row).filter(|_| p.view == REDISCOVER).map(|r| r.1.clone()).unwrap_or_default().into(),
        favourite: now.favourite,
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

/// A sound's waveform for the screens: every stored peak, one thin line each. Empty while it's read ([`peaks`]).
fn wave(s: &Sound) -> String {
    peaks::with(s, |w| list::wave_path_of(w.iter())).unwrap_or_default()
}

/// How many peaks across a sound's waveform is: none while it's read ([`peaks`]).
fn points(s: &Sound) -> i32 {
    peaks::with(s, |w| w.len() as i32).unwrap_or(0)
}

/// The waveforms of the sounds on show with these ids are back ([`peaks`]): the tiles show them, and so does the
/// picked sound's card when it's one of them.
fn waves_arrived(app: &AppWindow, ids: &[i64]) {
    let (tiles, card) = PAGE.with(|p| {
        let p = p.borrow();
        let s = p.picked.and_then(|at| p.list.sound(at)).filter(|s| ids.contains(&s.id));
        (p.view == TILES, s.map(|s| (wave(s), points(s))))
    });
    if tiles {
        TILE_ROWS.with(|m| m.notify.reset());
        grid::changed();
    }
    if let Some((wave, points)) = card {
        let data = app.global::<SoundsData>();
        let mut picked = data.get_picked();
        picked.wave = wave.into();
        picked.points = points;
        data.set_picked(picked);
    }
}

pub(crate) fn new_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

pub(crate) fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(|s| SharedString::from(s.as_ref())).collect::<Vec<_>>()))
}

/// As the app closes: the plays, drags and sends not kept yet, and a galaxy map being kept, are kept now, waiting a
/// moment at most.
pub fn finish_up() {
    did::finish_up();
    galaxy::finish_up();
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
    peaks::connect(app);
    crate::finder::on_open(Kind::Sound, |app, hit, how| {
        let Ok(id) = hit.id.parse::<i64>() else { return false };
        match how {
            How::Play => {
                let Some(at) = PAGE.with(|p| p.borrow().places.get(&id).copied()) else { return false };
                play_sound(app, at, 0.0, mo_audio::MAX_SECONDS);
                did::played(app, at);
                true
            }
            How::Open | How::InDaw => show_sound_by_id(app, id),
        }
    });
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.galaxy.db = Some(sounds_db.clone());
        copies::connect(&sounds_db);
        // Kits read the export folder from it as they're exported.
        p.kit.library = Some(library_db.clone());
    });
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
        read_all(&sounds_db, &library_db, None, None, None, send);
        return;
    }
    // Plays, drags and sends are kept from now on.
    let noter = did::connect(&sounds_db);
    let (again, asked_again) = mpsc::channel();
    *SCAN_AGAIN.lock().unwrap_or_else(PoisonError::into_inner) = Some(again);
    // Not at once: the first thing on show reads from the disk first. The sounds are read when this page, Space or
    // Ctrl+K wants them ([`wanted`]), or a few seconds after the window opens, whichever is first.
    let weak = app.as_weak();
    START_READ.with(|s| {
        *s.borrow_mut() = Some(Box::new(move || {
            // The shuffle the page opens on: the sounds it puts first are read first. Not when you've picked another
            // order meanwhile, nor with Hide duplicates on: copies among the first would go again once the copies
            // are known, and the tiles after them move (review B-5).
            let opening = PAGE.with(|p| {
                let p = p.borrow();
                let hiding = if p.hide_copies_chosen {
                    p.filters.hide_copies
                } else {
                    crate::settings::value(list::HIDE_COPIES_KEY).as_deref() == Some("on")
                };
                // The view you picked while the page waited, or else the one you left it on (read before the Settings
                // page connects when the app opens on this page: `crate::settings::value`).
                let view =
                    if p.chose_view { p.view } else { opening_view(crate::settings::value(VIEW_KEY).as_deref()) };
                (p.order == Order::Shuffled && !hiding).then_some(Opening { seed: p.seed, view })
            });
            let read = move || {
                let on_show = read_all(&sounds_db, &library_db, Some(cards.clone()), opening, Some(&noter), &send);
                // Once the page has all it needs from the library: it's backed up as it's due, and the sample
                // folders are scanned ([`scans`]).
                match on_show {
                    Some(mut on_show) => {
                        let asked = &asked_again;
                        scans(&sounds_db, &library_db, &cards, &mut on_show, Some(&noter), &send, asked);
                    }
                    // Nothing could be read: its backup copy is made all the same, when it's due.
                    None => {
                        back_up(&sounds_db, compact::backup::When::Daily, true, &cards, None, Some(&noter), &send);
                    }
                }
            };
            if let Err(e) = background::spawn("Sounds library", read)
                && let Some(app) = weak.upgrade()
            {
                // The sounds would never come: the page says why, rather than reading for ever.
                loaded(&app, Err(format!("its reader couldn't start ({e})")));
            }
        }))
    });
    crate::finder::when_opening(|_| wanted());
    if [Place::Sounds, Place::Space].iter().any(|p| p.index() as i32 == app.get_place()) {
        wanted();
    } else {
        Timer::single_shot(READ_AFTER, wanted);
    }
}

/// Something wants the sounds (their page or Space came on show, or Ctrl+K opened): they're read now, if they
/// haven't been yet.
pub fn wanted() {
    if let Some(start) = START_READ.with(|s| s.borrow_mut().take()) {
        start();
    }
}

/// What every view says in the list's place when the sound library has no sounds in it (made, but nothing scanned
/// into it yet): the words Rediscover has for it (`rediscover::why_nothing`). The page scans the sample folders at
/// each start, and again when one is added in Settings ([`scans`]).
const NO_SOUNDS: (&str, &str) = ("No sounds yet", "Add your sample folders in Settings and let the scan finish.");

/// While the sounds are read, every view shows one word in the list's place: not "No sounds match" (nothing is
/// filtered out yet), and not the galaxy's own word on top of it (Solomon's F12 note, 2026-11-02 04:25). Said once,
/// as the Songs page says it, not again in the page's header. The first list of the sounds takes its place once they
/// arrive ([`listed`]), or why they couldn't be read does ([`loaded`]).
pub fn reading(app: &AppWindow) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.read = false;
        p.unread = false;
    });
    let data = app.global::<SoundsData>();
    data.set_empty_title(READING.into());
    data.set_empty_text("".into());
}

/// What every view says in the list's place while the sounds are read ([`reading`]).
const READING: &str = "Reading your sounds…";

/// What reading brings back, in steps: the sounds first, then the list again with how your songs use them.
enum Step {
    /// The sounds: all of them, or (from the app's own library, opening on Tiles or List) the first few to show at
    /// once ([`Read::partial`]), the rest to follow.
    Read(Result<Box<Read>, String>),
    /// All the sounds, after the first few: the page takes them in place of those, which keep their places on show.
    Rest(Refreshed),
    Used(Arc<SoundList>),
    /// Every sound's fingerprint, for Sounds like this.
    Index(Arc<SoundIndex>),
    /// The fingerprints couldn't be read, and why: Sounds like this, the galaxy and Rediscover say so rather than wait.
    NoIndex(String),
    /// The sounds again, once the app's copy of the library was brought up to date with what the C# app changed.
    Refreshed(Refreshed),
    /// Where the galaxy's saved maps are read from now: `sounds.db`, as the copy damaged couldn't be made again.
    Source(PathBuf),
}

fn arrive(app: &AppWindow, step: Step) {
    let _doing = crate::watchdog::doing("Sounds: the library arrived");
    match step {
        Step::Read(read) => loaded(app, read),
        Step::Rest(rest) => rest_arrived(app, rest),
        Step::Used(list) => use_known(app, list),
        Step::Index(index) => index_known(app, index),
        Step::NoIndex(why) => index_failed(app, why),
        Step::Refreshed(refreshed) => brought_up_to_date(app, refreshed),
        Step::Source(source) => PAGE.with(|p| p.borrow_mut().galaxy.db = Some(source)),
    }
}

/// What reading the library gives: the sounds ready to list, what was played, and the view and autoplay you had last.
struct Read {
    library: Library,
    /// These are only the first few sounds, to show at once: how many there are in all, the rest to follow
    /// ([`Step::Rest`]).
    partial: Option<usize>,
    /// Where the galaxy's saved maps are read from: the app's copy of the library, or `sounds.db` without one.
    source: PathBuf,
    drum_machine: Option<String>,
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
    /// Which things done with sounds, and which of your say, these hold, when they were read with their writer held
    /// off ([`did::read_again`]).
    holds: Option<Holds>,
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
        Library {
            types: list.type_counts(),
            tags: list.tags(),
            list: Arc::new(list),
            played,
            places: Arc::new(places),
            holds: None,
        }
    }

    /// The same library, read holding the things done with sounds and your say that `holds` says (when it was read
    /// with their writer held off).
    fn read_holding(mut self, holds: Option<Holds>) -> Library {
        self.holds = holds;
        self
    }

    /// The same library with how your songs use each sound (by sound id) counted in, and the files of the projects
    /// that use them (by id), for the screenshots.
    pub fn with_usage(
        mut self,
        usage: HashMap<i64, Usage>,
        projects: HashMap<i64, ProjectFile>,
        weights: &LikedWeights,
    ) -> Library {
        self.list = Arc::new(with_usage(&self.list, usage, &Arc::new(projects), weights));
        self
    }
}

/// `list` again, sharing its sounds rather than copying them, with each sound's use in your songs (and the files of
/// the projects that use them, shared) and Most liked scored with it.
fn with_usage(
    list: &SoundList,
    usage: HashMap<i64, Usage>,
    projects: &Arc<HashMap<i64, ProjectFile>>,
    weights: &LikedWeights,
) -> SoundList {
    background::on_pool(|| {
        let mut list = list.sharing();
        list.set_liked(|s| weights.score(s, usage.get(&s.id)));
        list.set_used(usage);
        list.set_project_files(projects.clone());
        list
    })
}

/// The C# app's settings the page reads from the song library (`library.db`): which songs count as using a sound, and
/// the Most liked weights. The view, Autoplay and Rediscover's count are the app's own settings, which fall back to the
/// C# app's (`mo_app::app_settings::FROM_CSHARP`).
#[derive(Default)]
struct Settings {
    /// The Drum Machine's ID the C# app read from Bitwig's files, for kits sent to Bitwig.
    drum_machine: Option<String>,
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
            drum_machine: get(to_bitwig::DRUM_MACHINE_KEY),
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
///
/// With `opening` (how the page opens: shuffled), when it opens on Tiles or List, the first sounds its shuffle puts on
/// top are read from the copy on their own and sent first ([`FIRST`]), so they show in a moment; all of them follow
/// ([`Step::Rest`]). Reading half a million takes seconds.
///
/// With `noter`, what's done with sounds isn't written meanwhile ([`Noter::hold`]): the page knows which of what it
/// shows the sounds read hold already ([`did::read_again`]).
fn read_all(
    sounds_db: &Path,
    library_db: &Path,
    cards: Option<Feed>,
    opening: Option<Opening>,
    noter: Option<&Noter>,
    send: impl Fn(Step),
) -> Option<OnShow> {
    // A fresh install, with no library at all: the app's own is made empty, for the scan to fill (it scans only the
    // app's own). Not for the screenshots (no cards), which never write beside their fixture.
    let copy = compact::copy_of(sounds_db);
    if cards.is_some() {
        match compact::make_empty(sounds_db, &copy) {
            Ok(true) => log::info("Sounds", "Made the app's own sound library, empty, for the scan to fill"),
            Ok(false) => {}
            Err(e) => log::warning("Sounds", &format!("Couldn't make the app's own sound library: {e}")),
        }
    }
    // No library at all. Once the copy is the app's own, it's all there is: `sounds.db` may have been archived.
    if !sounds_db.exists() && (cards.is_none() || !copy.exists()) {
        send(Step::Read(Err(String::new())));
        return None;
    }
    let held = noter.map(Noter::hold);
    let holds = held.as_ref().map(Held::holds);
    let lib = library_db.exists().then(|| mo_library::Library::open_read_only(library_db)).and_then(|lib| {
        lib.map_err(|e| log::warning("Sounds", &format!("Couldn't read the song library for how sounds are used: {e}")))
            .ok()
    });
    let settings = lib.as_ref().map(Settings::read).unwrap_or_default();
    let now = Local::now().naive_local();
    let began = Instant::now();
    let read = |library: Library, source: PathBuf, partial: Option<usize>| Read {
        library,
        partial,
        source,
        drum_machine: settings.drum_machine.clone(),
        step_ms: settings.step_ms.clone(),
    };
    // The list of the first sounds shown, once they're sent: the rest take its place.
    let mut shown_first: Option<Arc<SoundList>> = None;
    // Only for Tiles and List, which show the shuffled list from its start: the other views need every sound.
    let seed = opening.filter(|o| matches!(o.view, TILES | LIST)).map(|o| o.seed);
    let first = match &cards {
        Some(cards) => read_through_copy(sounds_db, now, cards, |db| {
            let Some(seed) = seed else { return };
            match db.shown_ids().and_then(|ids| {
                let first = list::first_shuffled(&ids, seed, FIRST);
                Ok((db.some(&first, now)?, ids.len()))
            }) {
                Ok((sounds, all)) => {
                    let library = Library::new(sounds, Vec::new(), &settings.weights).read_holding(holds.clone());
                    log::info(
                        "Sounds",
                        &format!(
                            "The first {} of {all} sounds read in {:.2} s, the rest to follow",
                            library.list.len(),
                            began.elapsed().as_secs_f32()
                        ),
                    );
                    shown_first = Some(library.list.clone());
                    send(Step::Read(Ok(Box::new(read(library, db.path().to_owned(), Some(all))))));
                }
                // They come with the rest.
                Err(e) => log::warning("Sounds", &format!("Couldn't read the first sounds on their own: {e}")),
            }
        }),
        None => read_in_place(sounds_db, now),
    };
    let mut first = match first {
        Ok(first) => first,
        Err(e) => {
            send(Step::Read(Err(e.to_string())));
            return None;
        }
    };
    let read_in = began.elapsed();
    let sounds = std::mem::take(&mut first.sounds);
    // Sounds the app's own scan added aren't counted for CLAP: it never hears them in this app.
    let new_since = compact::grow::new_since(&first.source);
    let analysed = mo_sounds::count_clap_could_hear(&sounds, new_since);
    let library =
        Library::new(sounds, std::mem::take(&mut first.played), &settings.weights).read_holding(holds.clone());
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
    match shown_first {
        // Where each of the first sounds is in the whole list (all of them, but for a copy found damaged and made
        // again from sounds.db meanwhile): the page moves what it holds by place with its sound.
        Some(was) => {
            let moved = Some(Arc::new(places_in(&was, &library.places)));
            send(Step::Source(first.source.clone()));
            send(Step::Rest(Refreshed { library, was, moved, retyped: false }));
        }
        None => send(Step::Read(Ok(Box::new(read(library, first.source.clone(), None))))),
    }
    // Sounds like this: every sound's fingerprint, read with the sounds or from the copy after them; their blocks
    // become the index.
    let mut remade = None;
    // The copy made again from sounds.db, which gives the fingerprints, and the sounds as they are now, on the same
    // pass: for one found damaged inside (a bad sector). The sounds are taken in place of the copy's shown: those were
    // as the damaged copy had them, which (once it's the app's own library) needn't be how sounds.db has them.
    let remake = |first: &mut FirstRead, remade: &mut Option<Vec<Sound>>, from_backup: &mut bool| {
        let (mut everything, made) = compact::mend_reading(sounds_db, &first.source, now)?;
        say_made(&made, &first.source, sounds_db, cards.as_ref(), true);
        *from_backup = made.as_ref().is_ok_and(|m| m.from_backup.is_some());
        if made.is_err() {
            // Not made again: the rest is read from sounds.db, as it is without a copy.
            first.source = sounds_db.to_owned();
            send(Step::Source(first.source.clone()));
        }
        let (features, clap) = (std::mem::take(&mut everything.features), std::mem::take(&mut everything.clap));
        let analysed = mo_sounds::count_clap_could_hear(&everything.sounds, compact::grow::new_since(&first.source));
        *remade = Some(everything.sounds);
        Ok((features, clap, analysed))
    };
    let prints = match std::mem::replace(&mut first.prints, Prints::Read(Gathered::default(), Gathered::default())) {
        Prints::Read(features, clap) => Ok((features, clap, analysed)),
        Prints::InCopy(db) => match db.fingerprints(analysed) {
            Ok((features, clap)) => Ok((features, clap, analysed)),
            // Damaged where they're kept: made again.
            Err(e) if e.damaged() => {
                drop(db);
                log::warning(
                    "Sounds",
                    &format!("The app's copy of the sound library is damaged, so it's made again: {e}"),
                );
                remake(&mut first, &mut remade, &mut false)
            }
            Err(e) => Err(e),
        },
    };
    let mut index = send_index(prints, &send);
    // How your songs use samples, with the file of each project counted (for the card's Used in list), read once:
    // counted again for the sounds read again after an update.
    let uses = lib.and_then(|lib| uses_in(&lib, now, settings.rules));
    if let Some((uses, projects)) = &uses {
        send(Step::Used(Arc::new(with_usage(&list, usage_of(&list, uses), projects, &settings.weights))));
    }
    // A copy of the format before, read as it is: laid out anew now that the page has all it needs from it (one that's
    // behind is made afresh below instead). The page works as usual meanwhile: it holds nothing of it.
    if first.formerly && !first.behind && remade.is_none() {
        const MAKING: &str = "Making your sound library smaller";
        // Its card shows from when the work starts; one that couldn't start gets its card at the end.
        let mut job = None;
        let began = Instant::now();
        let laid_out = compact::lay_out(sounds_db, || {
            job = cards.as_ref().map(|c| c.start(MAKING, "Once, for this version of the app. Sounds works as usual."));
        });
        let card_for = |job: Option<Job>| job.or_else(|| cards.as_ref().map(|c| c.start(MAKING, "")));
        match laid_out {
            // Nothing to do after all (laid out meanwhile, or sounds.db changed since).
            None => {}
            Some(compact::LaidOut::Damaged(why))
                if sounds_db.exists() || compact::backup::backup_of(&first.source).exists() =>
            {
                log::warning("Sounds", &format!("The app's own sound library is damaged ({why}), so it's mended"));
                let mut remade_from_backup = false;
                let made = remake(&mut first, &mut remade, &mut remade_from_backup);
                let job = card_for(job);
                match made {
                    Ok(prints) => {
                        index = send_index(Ok(prints), &send);
                        if let Some(job) = job {
                            job.done_as(
                                "Made your sound library smaller",
                                if remade_from_backup {
                                    "It was damaged, so it was put back from its backup copy."
                                } else {
                                    "It was damaged, so it was made again from sounds.db."
                                },
                            );
                        }
                    }
                    Err(e) => {
                        log::warning("Sounds", &format!("Couldn't make the app's own sound library again: {e}"));
                        if let Some(job) = job {
                            job.failed_as("Couldn't make your sound library smaller", &list::not_remade_line(&e));
                        }
                    }
                }
            }
            Some(laid_out) => say_laid_out(&laid_out, card_for(job), began),
        }
    }
    let mut on_show = OnShow {
        list,
        uses,
        weights: settings.weights,
        source: first.source.clone(),
        new_since,
        analysed,
        rules: settings.rules,
        recount: false,
        index,
        added: 0,
    };
    if !first.behind && remade.is_none() {
        return Some(on_show);
    }
    // The sounds shown were as the copy had them before the C# app's changes (or before it was found damaged): the
    // page takes them again, as they were read just now to make the copy again, or else from the copy once it's
    // brought up to date.
    let (sounds, db, synced) = match remade {
        Some(sounds) => (sounds, SoundsDb::open_read_only(&first.source), None),
        None => {
            let Some(synced) = bring_up_to_date(sounds_db, &first.source, cards.as_ref()) else {
                return Some(on_show);
            };
            // Only what the C# app keeps for itself changed (its settings, the galaxy's saved places, copies found).
            if !synced.changed_the_list() {
                return Some(on_show);
            }
            match SoundsDb::open_read_only(&first.source).and_then(|db| Ok((db.all(now)?, db))) {
                Ok((sounds, db)) => (sounds, Ok(db), Some(synced)),
                Err(e) => {
                    log::warning(
                        "Sounds",
                        &format!("Couldn't read the sounds again once they were brought up to date: {e}"),
                    );
                    return Some(on_show);
                }
            }
        }
    };
    // The fingerprints read before stay right unless the copy was made afresh (or the count that says whether CLAP
    // has heard enough changed); with the sounds made again from sounds.db, they came with them.
    let prints = match synced {
        None => PrintsAgain::CameWith,
        Some(s) if s.made => PrintsAgain::Again,
        Some(_) => PrintsAgain::IfCountChanged,
    };
    take_again(sounds, db, prints, &mut on_show, holds, &send);
    Some(on_show)
}

/// Scans the sample folders for new, changed, moved and gone sounds and keeps them in the app's own sound library
/// (stage 2 of "the sound library grows again", review finding X1-own1), once the page has its sounds: the page takes
/// them again when anything changed ([`take_again`]). A card shows while sounds are listened to. Nothing when the
/// sounds on show aren't from the app's own library (it's made from `sounds.db` first), or there are no sample
/// folders. Whether the library changed. With `noter`, what's done with sounds isn't written while they're read again
/// ([`Noter::hold`]).
fn scan_for_sounds(
    sounds_db: &Path,
    library_db: &Path,
    cards: &Feed,
    on_show: &mut OnShow,
    noter: Option<&Noter>,
    send: &impl Fn(Step),
) -> bool {
    let copy = compact::copy_of(sounds_db);
    // No song library, no sample folders: nothing to say.
    if on_show.source != copy || !library_db.exists() {
        return false;
    }
    let folders = match mo_library::Library::open_read_only(library_db).and_then(|l| l.list_setting("sample_folders")) {
        Ok(folders) => folders,
        Err(e) => {
            log::warning("Sounds", &format!("Couldn't read your sample folders, so they weren't scanned: {e}"));
            return false;
        }
    };
    let job: RefCell<Option<Job>> = RefCell::new(None);
    let scanned = mo_app::sound_scan::scan(sounds_db, &folders, &|done, of| {
        let mut job = job.borrow_mut();
        let job =
            job.get_or_insert_with(|| cards.start("Listening to your sounds", "Sounds works as usual meanwhile."));
        job.progress(done as f32 / of.max(1) as f32, Some(&format!("{} of {}", thousands(done), thousands(of))));
    });
    let job = job.into_inner();
    let scanned = match scanned {
        Ok(Some(scanned)) => scanned,
        Ok(None) => return false,
        Err(e) => {
            log::warning("Sounds", &format!("Couldn't scan your sample folders: {e}"));
            if let Some(job) = job {
                job.failed_as("Couldn't keep your new sounds", &e);
            }
            return false;
        }
    };
    let line = mo_app::sound_scan::scanned_line(&scanned);
    // Stopped part way: what was kept before stays kept, and the page takes it.
    if scanned.stopped.is_some() {
        log::warning("Sounds", &line);
        if let Some(job) = job {
            let (title, detail) = mo_app::sound_scan::stopped_card(&scanned);
            job.failed_as(&title, &detail);
        }
    } else {
        log::info("Sounds", &line);
        if let Some(job) = job {
            let (title, detail) = mo_app::sound_scan::scanned_card(&scanned);
            job.done_as(&title, &detail);
        }
    }
    if scanned.not_forgotten > 0 {
        cards.notice("Some sounds look gone", &mo_app::sound_scan::not_forgotten_line(&scanned));
    }
    if !scanned.kept.any() {
        return false;
    }
    // The first new sound the scan numbered, now there may be one: CLAP's count leaves it out.
    on_show.new_since = compact::grow::new_since(&copy);
    let now = Local::now().naive_local();
    // How your songs use samples, let go while the page waited for this scan ([`scans`]): counted again, as they are.
    if on_show.recount && on_show.uses.is_none() {
        on_show.uses = match mo_library::Library::open_read_only(library_db) {
            Ok(lib) => uses_in(&lib, now, on_show.rules),
            Err(e) => {
                log::warning("Sounds", &format!("Couldn't read the song library for how sounds are used: {e}"));
                None
            }
        };
    }
    let held = noter.map(Noter::hold);
    let holds = held.as_ref().map(Held::holds);
    match SoundsDb::open_read_only(&copy).and_then(|db| Ok((db.all(now)?, db))) {
        Ok((sounds, db)) => {
            let prints = prints_after(&scanned, &sounds);
            take_again(sounds, Ok(db), prints, on_show, holds, send);
        }
        Err(e) => log::warning("Sounds", &format!("Couldn't read the sounds again once the scan kept them: {e}")),
    }
    true
}

/// Which fingerprints are read again once a scan kept sounds (`sounds`, read again): only those of the sounds it
/// listened to, new and changed, not every one (review finding S-5). Moved ones bring none, so none are read unless
/// the count that says whether CLAP has heard enough changed; gone ones are taken out of the index. By path, letter
/// case aside, as SQLite finds them.
fn prints_after(scanned: &mo_app::sound_scan::Scanned, sounds: &[Sound]) -> PrintsAgain {
    if scanned.listened.is_empty() && scanned.kept.forgotten == 0 {
        return PrintsAgain::IfCountChanged;
    }
    let listened: HashSet<String> = scanned.listened.iter().map(|p| p.to_ascii_lowercase()).collect();
    PrintsAgain::These(
        sounds.iter().filter(|s| listened.contains(&s.path.to_ascii_lowercase())).map(|s| s.id).collect(),
    )
}

/// Makes the library's backup copy as it's due ([`back_up`]), then scans the sample folders and the page takes what
/// changed ([`scan_for_sounds`]): once at start, then each time Settings asks ([`sample_folders_changed`]), until the
/// app closes. Asks already in when a scan starts are for the folders it reads, so one scan does them all.
///
/// The backup copy comes first, so a library found damaged is mended before the scan writes into it, and a start makes
/// one at most (review finding S-5). It's made when the library changed by more than plays, your say (favourites,
/// types, tags) and the galaxy's maps, or by those once it's a day old; a scan that kept sounds has one made straight
/// after, unless one was made just before. While the app stays open it's looked at again every hour, so what a scan
/// kept is in it within the hour, and a long session loses a day of the rest at most too (review finding S-6). A
/// library found damaged that couldn't be mended isn't tried again until the next start.
fn scans(
    sounds_db: &Path,
    library_db: &Path,
    cards: &Feed,
    on_show: &mut OnShow,
    noter: Option<&Noter>,
    send: &impl Fn(Step),
    asked: &mpsc::Receiver<()>,
) {
    use compact::backup::When;
    // Only the app's own library takes new sounds ([`scan_for_sounds`]): on the old app's, nothing waits for Settings'
    // asks, so its note says nothing of a scan.
    let own = on_show.source == compact::copy_of(sounds_db);
    let start = back_up(sounds_db, When::Daily, true, cards, Some(&mut *on_show), noter, send);
    let (mut mend, mut made) = (start.mend, start.made);
    loop {
        while asked.try_recv().is_ok() {}
        if scan_for_sounds(sounds_db, library_db, cards, on_show, noter, send) && !made {
            mend &= back_up(sounds_db, When::Now, mend, cards, Some(&mut *on_show), noter, send).mend;
        }
        made = false;
        // Not kept while it waits, which may be all the time the app is open: how your songs use samples is read again
        // by the next scan that keeps sounds (your songs may have changed by then too).
        on_show.recount |= on_show.uses.take().is_some();
        if !own {
            return;
        }
        loop {
            match asked.recv_timeout(BACKUP_LOOK_EVERY) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    mend &= back_up(sounds_db, When::Daily, mend, cards, Some(&mut *on_show), noter, send).mend;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// How often, while the app stays open, the sound library's backup copy is looked at to see whether it's due
/// ([`scans`]): it's made only once it's a day old, so this costs a look at two small notes.
const BACKUP_LOOK_EVERY: Duration = Duration::from_secs(60 * 60);

/// What the page shows once its first read is done, for sounds read again to take its place ([`take_again`]): the
/// list sent last, how your songs use samples (counted again for the sounds read again), the Most liked weights, where
/// the sounds come from, the first number the app's own scan gave a sound ([`compact::grow::new_since`]), and how many
/// analysed sounds CLAP could have heard. `rules` count how your songs use samples, read again for a later scan when
/// `recount` (the count was let go while the page waited for it: [`scans`]).
struct OnShow {
    list: Arc<SoundList>,
    uses: Option<Uses>,
    weights: LikedWeights,
    source: PathBuf,
    new_since: Option<i64>,
    analysed: usize,
    rules: UsageRules,
    recount: bool,
    /// The fingerprints' index sent last, for a scan's few new sounds to be put in it ([`SoundIndex::with_features`]).
    index: Option<Arc<SoundIndex>>,
    /// How many sounds' fingerprints have been put in it since it was built afresh ([`FEW`]).
    added: usize,
}

/// How many sounds' fingerprints may be put in the index on show, for each one in it, before it's built afresh
/// ([`take_again`]): what makes the fingerprints ready (the averages and spreads each number's taken from and divided
/// by) stays that of most of them, and a scan that brings a lot (a new sample folder) reads them all once, not one at
/// a time.
const FEW: usize = 10;

/// How your songs use samples, and the file of each project counted.
type Uses = (Vec<mo_library::SampleUse>, Arc<HashMap<i64, ProjectFile>>);

/// How your songs in `lib` use samples, by `rules`, with what was saved in the year to `now` counted as recent.
fn uses_in(lib: &mo_library::Library, now: NaiveDateTime, rules: UsageRules) -> Option<Uses> {
    let year_ago = now.checked_sub_months(chrono::Months::new(12)).unwrap_or(now);
    lib.sample_usage_with_projects(year_ago, rules)
        .map(|u| (u.uses, Arc::new(u.projects)))
        .map_err(|e| log::warning("Sounds", &format!("Couldn't count how your songs use sounds: {e}")))
        .ok()
}

/// When [`take_again`] reads the sounds' fingerprints again too.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PrintsAgain {
    /// Never: they came with the sounds (made again from `sounds.db`).
    CameWith,
    /// Only when the count that says whether CLAP has heard enough changed.
    IfCountChanged,
    /// Always: the library was made afresh.
    Again,
    /// Only those of these sounds, a scan listened to, put in the index on show ([`SoundIndex::with_features`]); all
    /// of them when the count that says whether CLAP has heard enough changed, or there's no index on show.
    These(Vec<i64>),
}

/// The sounds read again (`sounds`, from `db`) take the place of those on show: the page moves what it holds by place
/// with each sound ([`Step::Refreshed`]). Their fingerprints are read again too as `prints` says. `holds`: the things
/// done with sounds and your say that they hold, when they're read with their writer held off ([`Noter::hold`]).
fn take_again(
    sounds: Vec<Sound>,
    db: Result<SoundsDb, SoundsError>,
    prints: PrintsAgain,
    on_show: &mut OnShow,
    holds: Option<Holds>,
    send: &impl Fn(Step),
) {
    // What was played: the sounds are in hand, so a History that can't be read just now stays empty.
    let played = db
        .as_ref()
        .map_err(SoundsError::to_string)
        .and_then(|db| db.history(HISTORY_LENGTH).map_err(|e| e.to_string()));
    let played = played.unwrap_or_else(|e| {
        log::warning("Sounds", &format!("Couldn't read what was played once the sounds were read again: {e}"));
        Vec::new()
    });
    let analysed_now = mo_sounds::count_clap_could_hear(&sounds, on_show.new_since);
    // The sounds there now, in id order, for the index on show to lose those gone (hidden ones too: it has them).
    let there: Vec<i64> = match (&prints, &on_show.index) {
        (PrintsAgain::These(_), Some(_)) => {
            let mut ids: Vec<i64> = sounds.iter().map(|s| s.id).collect();
            ids.sort_unstable();
            ids
        }
        _ => Vec::new(),
    };
    let mut library = Library::new(sounds, played, &on_show.weights).read_holding(holds);
    // How your songs use each sound, as counted for this list; or when they couldn't be counted again, as the list on
    // show has them (a sound new since then has none yet), so the Used chips and Most liked don't lose them.
    let usage = match &on_show.uses {
        Some((uses, projects)) => Some((usage_of(&library.list, uses), projects.clone())),
        None => on_show.list.usage_map().cloned().zip(on_show.list.project_files().cloned()),
    };
    if let Some((usage, projects)) = usage {
        library.list = Arc::new(with_usage(&library.list, usage, &projects, &on_show.weights));
    }
    let list = &on_show.list;
    // Where each sound on show is in the new list, when they aren't the same sounds in the same order: worked out
    // here, off the window's thread, for the page to move what it holds by place (half a million sounds to look up).
    let moved = (!list.same_sounds_as(&library.list)).then(|| Arc::new(places_in(list, &library.places)));
    // The same sounds in the same places, one given another type (in the C# app): the galaxy colours its stars, and
    // names its neighbourhoods, by type.
    let retyped =
        moved.is_none() && list.sounds().iter().zip(library.list.sounds()).any(|(a, b)| a.sound_type != b.sound_type);
    let now_shown = library.list.clone();
    send(Step::Refreshed(Refreshed { library, was: list.clone(), moved, retyped }));
    let count_changed = analysed_now != on_show.analysed;
    let mut afresh = None;
    match (prints, db, on_show.index.clone()) {
        (PrintsAgain::CameWith, ..) | (_, Err(_), _) => {}
        (PrintsAgain::IfCountChanged, ..) if !count_changed => {}
        (PrintsAgain::These(ids), Ok(db), Some(index))
            if !count_changed && (on_show.added + ids.len()) * FEW <= index.features.len() =>
        {
            match with_these(&db, &ids, &there, index.clone()) {
                Ok(Some(now)) => {
                    if !Arc::ptr_eq(&now, &index) {
                        send(Step::Index(now.clone()));
                        on_show.index = Some(now);
                    }
                    on_show.added += ids.len();
                }
                // CLAP's index has a sound that's gone.
                Ok(None) => afresh = Some(db),
                Err(e) => {
                    log::warning("Sounds", &format!("Couldn't read the new sounds' fingerprints, so all of them: {e}"));
                    afresh = Some(db);
                }
            }
        }
        (_, Ok(db), _) => afresh = Some(db),
    }
    if let Some(db) = afresh {
        on_show.index = send_index(db.fingerprints(analysed_now).map(|(f, c)| (f, c, analysed_now)), send);
        on_show.added = 0;
    }
    on_show.list = now_shown;
    on_show.analysed = analysed_now;
}

/// The index on show (`index`) after a scan ([`SoundIndex::with_features`]): with the fingerprints of the sounds it
/// listened to (`ids`) read again, and without the sounds no longer `there` (in id order): the same one when none of
/// that changes it. `None` when it has to be built afresh.
fn with_these(
    db: &SoundsDb,
    ids: &[i64],
    there: &[i64],
    index: Arc<SoundIndex>,
) -> Result<Option<Arc<SoundIndex>>, SoundsError> {
    let more = db.features_of(ids)?;
    let again: HashSet<i64> = ids.iter().copied().collect();
    Ok(background::on_pool(move || {
        let gone: HashSet<i64> =
            index.features.ids().iter().copied().filter(|id| there.binary_search(id).is_err()).collect();
        if gone.is_empty() && more.is_empty() && !again.iter().any(|id| index.features.contains(*id)) {
            return Some(index);
        }
        index.with_features(&gone, &again, more).map(Arc::new)
    }))
}

/// How many plays History lists.
const HISTORY_LENGTH: usize = 500;

/// How many sounds are read on their own and shown at once, while the rest are read ([`read_all`]): screens of tiles
/// to look through and pick from meanwhile.
const FIRST: usize = 200;

/// How the page opens, as its sounds are about to be read ([`read_all`]): shuffled by `seed`, on `view` (the one you
/// picked while it waited for them, or else the one you left it on).
#[derive(Clone, Copy, Debug)]
struct Opening {
    seed: u64,
    view: i32,
}

/// The view the page opens on, with the one you left it on by its name (`name`, as kept in the settings).
fn opening_view(name: Option<&str>) -> i32 {
    name.and_then(|v| VIEWS.iter().position(|n| *n == v)).map_or(TILES, |i| i as i32)
}

/// Where each sound of `list` is in a list read again (`places`: each sound's place there, by id); `None` for one
/// that's gone.
fn places_in(list: &SoundList, places: &HashMap<i64, usize>) -> Vec<Option<usize>> {
    list.sounds().iter().map(|s| places.get(&s.id).copied()).collect()
}

/// Makes the index of the fingerprints read (with how many sounds had been analysed when they were), and sends it.
/// The index sent, if one was.
fn send_index(
    prints: Result<(Gathered, Gathered, usize), SoundsError>,
    send: &impl Fn(Step),
) -> Option<Arc<SoundIndex>> {
    match prints {
        Ok((features, clap, analysed)) => {
            let index = Arc::new(background::on_pool(move || SoundIndex::from_gathered(features, clap, analysed)));
            send(Step::Index(index.clone()));
            Some(index)
        }
        Err(e) => {
            log::warning("Sounds", &format!("Couldn't read the sounds' fingerprints: {e}"));
            send(Step::NoIndex(e.to_string()));
            None
        }
    }
}

/// The page's first read of the sound library, wherever it came from.
struct FirstRead {
    sounds: Vec<Sound>,
    /// The sounds came from a copy of the format before ([`SoundsDb::is_formerly`]): it's laid out anew once the page
    /// has all it needs from it ([`compact::lay_out`]).
    formerly: bool,
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
        formerly: false,
        played: db.history(HISTORY_LENGTH).unwrap_or_default(),
        source: sounds_db.to_owned(),
        prints: Prints::Read(everything.features, everything.clap),
        behind: false,
    })
}

/// The sounds read from the app's own copy of the library beside `sounds_db`, or (with no copy it can read yet) from
/// `sounds_db` while the copy is made on the same pass ([`compact::read_for_page`]). Making it goes on `cards`.
/// `first` is handed the copy before its sounds are read, to send a few at once.
fn read_through_copy(
    sounds_db: &Path,
    now: NaiveDateTime,
    cards: &Feed,
    first: impl FnOnce(&SoundsDb),
) -> Result<FirstRead, SoundsError> {
    let copy = compact::copy_of(sounds_db);
    match compact::read_for_page(sounds_db, now, first)? {
        PageRead::Copy { db, sounds, behind, adopted } => {
            if adopted {
                say_adopted();
            }
            Ok(FirstRead {
                sounds,
                formerly: db.is_formerly(),
                played: db.history(HISTORY_LENGTH).unwrap_or_default(),
                source: copy,
                prints: Prints::InCopy(db),
                behind,
            })
        }
        PageRead::Library { everything, made } => {
            say_made(&made, &copy, sounds_db, Some(cards), false);
            // The rest is read from the copy made at the moment the sounds were read, or else (when it couldn't be
            // made, or opened just now) from `sounds.db`. What was played: the sounds are in hand, so a History that
            // can't be read just now stays empty.
            let (source, played) = match made.as_ref().ok().and_then(|_| SoundsDb::open_read_only(&copy).ok()) {
                Some(db) => (copy, db.history(HISTORY_LENGTH)),
                None => (sounds_db.to_owned(), SoundsDb::read_once(sounds_db, |db| db.history(HISTORY_LENGTH))),
            };
            Ok(FirstRead {
                sounds: everything.sounds,
                formerly: false,
                played: played.unwrap_or_default(),
                source,
                prints: Prints::Read(everything.features, everything.clap),
                behind: false,
            })
        }
    }
}

/// Says (in the log, and on its card, `job`) how laying the app's own sound library out anew went
/// ([`compact::lay_out`]), begun at `began`.
fn say_laid_out(laid_out: &compact::LaidOut, job: Option<Job>, began: Instant) {
    let took = began.elapsed().as_secs_f32();
    let (title, line, done) = match laid_out {
        compact::LaidOut::Done { before, after } => {
            let line = list::laid_out_line(*before, *after);
            log::info("Sounds", &format!("Laid the app's own sound library out anew in {took:.1} s: {line}"));
            ("Made your sound library smaller", line, true)
        }
        // Read as it is meanwhile: nothing's different on the page.
        compact::LaidOut::NotNow(why) => {
            log::warning(
                "Sounds",
                &format!(
                    "The app's own sound library is to be laid out anew, which couldn't be done just now ({why}): it's \
                     read as it is, and laid out anew another time"
                ),
            );
            ("Couldn't make your sound library smaller just now", list::not_laid_out_line(why), false)
        }
        compact::LaidOut::Damaged(why) => {
            log::warning(
                "Sounds",
                &format!(
                    "The app's own sound library is damaged ({why}), and there's no sounds.db to make it again from: \
                     it's read as it is"
                ),
            );
            ("Couldn't make your sound library smaller", list::damaged_not_laid_out_line(), false)
        }
    };
    if let Some(job) = job {
        if done { job.done_as(title, &line) } else { job.failed_as(title, &line) }
    }
}

/// Makes the sound library's backup copy as `when` says ([`compact::backup::back_up_when`]), and says how it went. One
/// not made because the library itself is damaged, where the page's read never looks (review finding S-3), has the
/// library mended now when `mend` says so: put back from its backup copy, or made again from sounds.db, either way
/// with everything that can still be read of it. The page then takes the mended library's sounds (`on_show`, with
/// what's done with sounds held off meanwhile: `noter`), and the mended library is backed up.
fn back_up(
    sounds_db: &Path,
    when: compact::backup::When,
    mend: bool,
    cards: &Feed,
    on_show: Option<&mut OnShow>,
    noter: Option<&Noter>,
    send: &impl Fn(Step),
) -> Backup {
    let why = match compact::backup::back_up_when(sounds_db, when) {
        Some(compact::backup::BackedUp::Damaged(why)) if mend => why,
        backed_up => return Backup { made: say_backed_up(backed_up), mend },
    };
    let not_mended = Backup { made: false, mend: false };
    log::warning("Sounds", &format!("The sound library's backup copy couldn't be made, so it's mended: {why}"));
    let copy = compact::copy_of(sounds_db);
    let held = noter.map(Noter::hold);
    let holds = held.as_ref().map(Held::holds);
    match compact::mend_reading(sounds_db, &copy, Local::now().naive_local()) {
        Ok((mut everything, made)) => {
            say_made(&made, &copy, sounds_db, Some(cards), true);
            if made.is_err() {
                return not_mended;
            }
            if let Some(on_show) = on_show {
                // As the mended library has them: what it was mended from may say otherwise.
                on_show.new_since = compact::grow::new_since(&copy);
                let (features, clap) = (std::mem::take(&mut everything.features), std::mem::take(&mut everything.clap));
                let db = SoundsDb::open_read_only(&copy);
                take_again(everything.sounds, db, PrintsAgain::CameWith, on_show, holds, send);
                on_show.index = send_index(Ok((features, clap, on_show.analysed)), send);
                on_show.added = 0;
            }
            drop(held);
            Backup { made: say_backed_up(compact::backup::back_up(sounds_db)), mend: true }
        }
        Err(e) => {
            log::warning("Sounds", &format!("Couldn't mend the sound library: {e}"));
            cards.start("Couldn't mend your sound library", "").failed(&list::not_remade_line(&e));
            not_mended
        }
    }
}

/// How making the sound library's backup copy went, for what comes after it ([`back_up`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Backup {
    /// One was made.
    made: bool,
    /// A library found damaged may still be mended: not once one couldn't be (it's tried again at the next start).
    mend: bool,
}

/// Says in the log how making the sound library's backup copy went ([`compact::backup::back_up`]): nothing when there
/// was nothing to do. Whether one was made.
fn say_backed_up(backed_up: Option<compact::backup::BackedUp>) -> bool {
    let made = matches!(backed_up, Some(compact::backup::BackedUp::Made { .. }));
    match backed_up {
        None => {}
        Some(compact::backup::BackedUp::Made { bytes, took }) => log::info(
            "Sounds",
            &format!(
                "Made the sound library's backup copy ({}) and checked it, in {:.1} s",
                mo_space::size::size_text(bytes),
                took.as_secs_f32()
            ),
        ),
        Some(compact::backup::BackedUp::NotNow(why)) => {
            log::warning("Sounds", &format!("Couldn't make the sound library's backup copy this time: {why}"));
        }
        // Mended once at a time ([`back_up`]): found damaged again straight after it was mended.
        Some(compact::backup::BackedUp::Damaged(why)) => {
            log::warning("Sounds", &format!("Couldn't make the sound library's backup copy: {why}"));
        }
    }
    made
}

/// Says in the log, once, that the app's copy of the sound library is its own library from now on (the C# app is
/// retired, Solomon 2026-11-05): `sounds.db` isn't read again.
fn say_adopted() {
    log::info(
        "Sounds",
        &format!(
            "{} is the app's own sound library from now on: sounds.db is no longer read (the C# app is retired)",
            compact::FILE_NAME
        ),
    );
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
    if made.as_ref().is_ok_and(|synced| synced.adopted) {
        say_adopted();
    }
    match made {
        Ok(synced) if synced.made && synced.from_backup.is_some() => {
            let made = synced.from_backup.map(chrono::DateTime::<Local>::from).unwrap_or_else(Local::now);
            let line = list::put_back_line(made, synced.salvaged.as_ref());
            log::warning("Sounds", &format!("The app's own sound library was damaged: {line}"));
            if let Some(cards) = cards {
                cards.start("Mended your sound library from its backup copy", "").done(&line);
            }
        }
        // Made again from sounds.db with what could still be read of it (review finding S-1).
        Ok(synced) if synced.made && synced.salvaged.is_some() => {
            let unused = synced.backup_unused.as_deref();
            let line = synced.salvaged.as_ref().map(|s| list::salvaged_line(s, unused)).unwrap_or_default();
            log::warning("Sounds", &format!("The app's own sound library was damaged: {line} ({synced:?})"));
            if let Some(cards) = cards {
                cards.start("Mended your sound library", "").done(&line);
            }
        }
        Ok(synced) if synced.made => {
            if let Some(why) = &synced.backup_unused {
                log::warning("Sounds", &format!("The sound library was made again from sounds.db, as {why}"));
            }
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

/// Brings the app's copy of the library up to date with `sounds_db` one last time, after the C# app changed it, and
/// makes it the app's own library ([`compact::last_sync`]), saying so on `cards`. What changed, or `None` when it
/// couldn't be done (it's tried again at the next start).
fn bring_up_to_date(sounds_db: &Path, copy: &Path, cards: Option<&Feed>) -> Option<compact::Synced> {
    // The app's own library already (another read made it so meanwhile): nothing to bring up to date, nor to say.
    if compact::own_since(copy).is_some() {
        return None;
    }
    let job = cards.map(|c| c.start("Bringing your sound library up to date", "With what changed in sounds.db"));
    let began = Instant::now();
    match compact::last_sync(sounds_db, copy) {
        Ok(synced) => {
            if synced.adopted {
                say_adopted();
            }
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
        p.no_index = None;
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
    // Sounds like your favourites, for these fingerprints.
    like::wanted(app);
    // Rediscover was waiting on the fingerprints. New ones later don't dig its sounds out again: they'd all change.
    if first && PAGE.with(|p| p.borrow().re_started) {
        rediscover_now(app);
    }
}

/// The fingerprints couldn't be read (`why`): what was waiting for them says so (review finding B-1).
fn index_failed(app: &AppWindow, why: String) {
    let picked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // Fingerprints the page has already (read before, on another pass) are kept.
        if p.index.is_some() {
            return None;
        }
        p.no_index = Some(why);
        Some(p.picked)
    });
    let Some(picked) = picked else { return };
    if let Some(at) = picked {
        find_alike(app, at);
    }
    galaxy::index_known(app);
    if PAGE.with(|p| p.borrow().re_started) {
        rediscover_now(app);
    }
}

/// What waits for the fingerprints says when they couldn't be read (`why`): that `what` can't be done, and why.
pub(super) fn no_index_note(what: &str, why: &str) -> String {
    format!("The sounds' fingerprints couldn't be read, so {what} ({why}).")
}

fn find_alike(app: &AppWindow, at: usize) {
    let data = app.global::<SoundsData>();
    data.set_alike(ModelRc::default());
    let (none, no_index) = PAGE.with(|p| {
        let p = p.borrow();
        (p.index.is_none(), p.no_index.clone())
    });
    if none {
        let note = match no_index {
            Some(why) => no_index_note("nothing can listen for sounds like it", &why),
            None => "Not ready yet: the sounds' fingerprints are still being read.".to_owned(),
        };
        data.set_alike_note(note.into());
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
                    let now = p.list.now(i)?;
                    Some(AlikeLine {
                        place: i as i32,
                        name: now.sound.name().into(),
                        sound_type: now.sound_type.into(),
                        colour: list::type_colour(now.sound_type),
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
        // The copies Hide duplicates found meanwhile stay.
        let mut with_use = (*list).clone();
        with_use.keep_copies_of(&p.list);
        with_use.keep_like_favourites_of(&p.list);
        p.list = Arc::new(with_use);
        // Your say since the sounds were read stays over them.
        yours::keep_showing(&mut p);
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
    // The Space page keeps the copies it counts to the one to keep that your songs use.
    crate::space::sounds_changed(app);
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
    crate::space::sounds_changed(app);
    // Samples saved from Bitwig may be among the sounds now; Mark as favourites may work now.
    bitwig::sounds_read(app);
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
    // Read again, they hold what was done with them so far: what wasn't kept yet as they were read goes on showing.
    did::read_again(p, library.holds.as_ref());
    yours::read_again(p, library.holds.as_ref());
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
            // The first few were shown, and the rest couldn't be read: none are, so the page says why. Shown empty,
            // their chips, years and Ctrl+K's list go with them.
            if PAGE.with(|p| p.borrow_mut().partial.take().is_some()) {
                data.set_panel_shown(false);
                stop(app);
                PAGE.with(|p| {
                    let mut p = p.borrow_mut();
                    p.list = Arc::default();
                    p.places = Arc::default();
                    p.types.clear();
                    p.tags.clear();
                    p.played = Arc::default();
                    p.history.clear();
                    p.picked = None;
                    p.counts = None;
                    // Not read after all: the list's word ([`listed`]) doesn't take the place of why.
                    p.read = false;
                });
                crate::finder::add_source(app, "sounds", Arc::new(finder::Sounds(Arc::default())));
                refresh(app);
            }
            PAGE.with(|p| p.borrow_mut().unread = true);
            crate::space::sounds_changed(app);
            // One there that couldn't be read is said to be so, not missing (review finding B-1).
            data.set_empty_title(
                if why.is_empty() { "No sound library yet" } else { "The sound library couldn't be read" }.into(),
            );
            data.set_empty_text(
                if why.is_empty() {
                    // Not the old app's Sound Lab, which is retired (review B-6).
                    "This version can't make one yet. The page shows the sound library the old app made (sounds.db in \
                     the app's data folder), and there isn't one there."
                        .to_string()
                } else {
                    why
                }
                .into(),
            );
            return;
        }
    };
    let count =
        crate::settings::value(COUNT_KEY).and_then(|c| c.parse().ok()).filter(|c| COUNTS.contains(c)).unwrap_or(COUNT);
    if read.partial.is_some() {
        // The first sounds come with their waveforms, the rest without them, these among them: kept, so the tiles on
        // show keep theirs whatever list they're in.
        peaks::keep_own(&read.source, read.library.list.sounds());
    }
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.galaxy.db = Some(read.source);
        p.re.count = count;
        p.kit.drum_machine = read.drum_machine;
        p.step = Duration::from_millis(list::step_ms(read.step_ms.as_deref()));
        p.partial = read.partial;
    });
    // As you left them: the app's own settings, else the C# app's (review G-3).
    let (view, autoplay) = (crate::settings::value(VIEW_KEY), crate::settings::value(AUTOPLAY_KEY));
    show(app, read.library, view.as_deref(), autoplay.as_deref());
}

/// All the sounds have arrived after the first few: they take those's place, which keep theirs on show.
fn rest_arrived(app: &AppWindow, rest: Refreshed) {
    // Only in place of the first few ([`brought_up_to_date`] checks the same): nothing else gives the page other
    // sounds meanwhile.
    let first = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let first = p.list.shares_with(&rest.was);
        if first {
            p.partial = None;
        }
        first
    });
    if first {
        brought_up_to_date(app, rest);
    }
}

/// Shows a `library` on the page, opening on the view and autoplay setting given.
pub fn show(app: &AppWindow, library: Library, view: Option<&str>, autoplay: Option<&str>) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // "Reading your sounds…" stays until the list of them is worked out, so "No sounds match" never flashes up
        // in between ([`listed`]).
        p.read = true;
        // Read now, whatever came before (the screenshots show a library without reading one): the Space page counts
        // its copies ([`known`]).
        p.unread = false;
        // History lists them once the filters have been through them ([`listed`]).
        p.played = Arc::new(library.played);
        p.history.clear();
        p.list = library.list;
        p.to_pick = None;
        p.places = library.places;
        p.types = library.types;
        p.tags = library.tags;
        did::read_again(&mut p, library.holds.as_ref());
        yours::read_again(&mut p, library.holds.as_ref());
        // Counted afresh for these sounds: the choices show without numbers until then.
        p.counts = None;
        p.open_on(view, autoplay);
        // Hide duplicates as you left it, unless you've set it since the app started.
        if !p.hide_copies_chosen {
            p.hide_copies_chosen = true;
            p.filters.hide_copies = crate::settings::value(list::HIDE_COPIES_KEY).as_deref() == Some("on");
        }
        // How far the sounds like your favourites reach too.
        if !p.like_chosen {
            p.like_chosen = true;
            p.filters.like_favourites = crate::settings::value(list::LIKE_FAVOURITES_KEY)
                .and_then(|v| v.parse::<u8>().ok())
                .map_or(0, |v| v.min(list::LIKE_STEPS));
        }
        // Trim silence too.
        if !p.trim_chosen {
            p.trim_chosen = true;
            p.trim = crate::settings::value(list::TRIM_SILENCE_KEY).as_deref() == Some("on");
            p.trim_db = mo_audio::trim::level_db(crate::settings::value(list::TRIM_LEVEL_KEY).as_deref());
        }
        p.picked = None;
        p.rediscover.clear();
        p.re_started = false;
        // The years come with Rediscover's first answer for these sounds.
        p.re_years.clear();
        // And the years picked on it go with them: the new sounds may not have those years, nor a strip to clear.
        p.re.years = None;
        p.re_likes.clear();
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
    crate::space::sounds_changed(app);
    bitwig::sounds_read(app);
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
    // Favourite has its heart on the card, and a line here (as in the C# app's menu); Add a tag, its box on the card,
    // which takes the keys (the review of 10 Nov, X3-6: its key shows here and in the box's tooltip, not on the card).
    let favourite = MenuItem {
        what: "Favourite, or not".into(),
        key: shortcuts.key_of(FAVOURITE).into(),
        id: FAVOURITE.into(),
        enabled: true,
    };
    let tag =
        MenuItem { what: "Add a tag…".into(), key: shortcuts.key_of(TAG).into(), id: TAG.into(), enabled: true };
    let mut menu = vec![play, favourite, tag, MenuItem { enabled: true, ..MenuItem::default() }];
    let mut actions = Vec::new();
    for &(text, id) in CARD_ACTIONS {
        let key = SharedString::from(shortcuts.key_of(id));
        menu.push(MenuItem { what: text.into(), key: key.clone(), id: id.into(), enabled: true });
        actions.push(CardAction {
            text: text.into(),
            tip: what_of(id).into(),
            key,
            id: id.into(),
            enabled: true,
            main: false,
            danger: false,
        });
    }
    data.set_menu(ModelRc::new(VecModel::from(menu)));
    data.set_actions(ModelRc::new(VecModel::from(actions)));
    data.set_play_tip(
        "Play or stop the picked sound. A click on the sound playing stops it too, and so does a right click on its \
         waveform."
            .into(),
    );
    data.set_play_key(shortcuts.key_of("app.play").into());
    data.set_favourite_key(shortcuts.key_of(FAVOURITE).into());
    data.set_tag_key(shortcuts.key_of(TAG).into());
    data.set_tag_tip(what_of(TAG).into());
}

/// Favourite's and Add a tag's ids in the shortcut list (the C# app's).
const FAVOURITE: &str = "sounds.favourite";
const TAG: &str = "sounds.tag";

fn wire(app: &AppWindow, feed: Feed, shortcuts: &Shortcuts) {
    FEED.with(|f| *f.borrow_mut() = Some(feed));
    let data = app.global::<SoundsData>();
    data.set_tile_rows(ModelRc::from(TILE_ROWS.with(|m| m.clone()) as Rc<dyn Model<Data = TileRow>>));
    data.set_lines(ModelRc::from(LINES.with(|m| m.clone()) as Rc<dyn Model<Data = SoundLine>>));
    data.set_views(strings(VIEWS));
    show_actions(&data, shortcuts);
    // The filters whose choices don't come from the sounds (length, key, tempo) show from the start, without numbers
    // until the sounds are read, so the rows don't jump when they are.
    show_chips(app);

    let weak = app.as_weak();
    data.on_search_edited(move |text| change(&weak, |f| f.search = text.to_string()));
    let weak = app.as_weak();
    data.on_toggle_choice(move |chip, choice| change(&weak, |f| f.pick(&chip, &choice)));
    let weak = app.as_weak();
    data.on_tempo_picked(move |low, high| change(&weak, |f| f.set_tempo(low, high)));
    data.set_tempo_low_end(list::TEMPO_LOW);
    data.set_tempo_high_end(list::TEMPO_HIGH);
    data.set_tempo_step(list::TEMPO_STEP);
    let weak = app.as_weak();
    data.on_clear_chip(move |chip| {
        if chip == "more-types" {
            // The More types menu's Any: the types on tick lines of their own stay picked.
            let (types, lines) = PAGE.with(|p| {
                let p = p.borrow();
                (p.types.clone(), p.type_lines)
            });
            change(&weak, |f| list::keep_first_types(&mut f.sound_types, &types, lines));
        } else {
            change(&weak, |f| f.clear_chip(&chip));
        }
    });
    // The filter column has room for more or fewer types (the window or what's under them changed): its type list
    // shows as many again. Only the type list: the window's edge being dragged changes it at every line's height.
    let weak = app.as_weak();
    data.on_type_room(move |lines, re_lines| {
        let Some(app) = weak.upgrade() else { return };
        let (types, re_types) = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            (room(&mut p.type_lines, lines), room(&mut p.re_type_lines, re_lines))
        });
        let data = app.global::<SoundsData>();
        PAGE.with(|p| {
            let p = p.borrow();
            if types {
                show_types(&data, &p);
            }
            if re_types {
                show_re_types(&data, &p);
            }
        });
    });
    let weak = app.as_weak();
    let weak_again = app.as_weak();
    data.on_toggle(move |kind| {
        change(&weak, |f| f.toggle(&kind));
        if kind == list::HIDE_COPIES {
            // Kept for next time (rule: the app's own settings file, never library.db).
            let on = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.hide_copies_chosen = true;
                p.filters.hide_copies
            });
            crate::settings::remember(list::HIDE_COPIES_KEY, if on { "on" } else { "off" });
            // Turned on after a look that failed: it's tried again.
            if on && let Some(app) = weak_again.upgrade() {
                copies::look_again(&app, true);
            }
        }
    });
    // The Favourites filter's "how adventurous" slider: how many sounds like each favourite show beside them. Kept
    // for next time (the app's own settings file).
    let weak = app.as_weak();
    data.on_like_favourites_set(move |share| {
        let step = list::like_favourites_step(share);
        PAGE.with(|p| p.borrow_mut().like_chosen = true);
        crate::settings::remember(list::LIKE_FAVOURITES_KEY, &step.to_string());
        change(&weak, |f| f.like_favourites = step);
    });
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
    data.on_open_project(move |id| {
        let Some(app) = weak.upgrade() else { return };
        if id < 0 {
            return;
        }
        crate::go_to(&app, Place::Songs.index() as i32);
        if !crate::songs::show_save(&app, i64::from(id)) {
            log::info("Sounds", "The song isn't on the Songs page yet (its list is still being read)");
        }
    });
    let weak = app.as_weak();
    data.on_clear(move || change(&weak, |f| *f = f.cleared()));
    let weak = app.as_weak();
    data.on_pick_order(move |name| {
        let Some(app) = weak.upgrade() else { return };
        // The order already picked stays picked, and the list stays as it is (unless it was turned round: a chip
        // puts it the usual way round).
        let order = Order::from_name(&name);
        let changed = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let was = (p.order, p.reversed);
            (p.order, p.reversed) = (order, false);
            was != (order, false)
        });
        if changed {
            refresh(&app);
        }
    });
    let weak = app.as_weak();
    data.on_order_by_column(move |column| {
        // A click on a List heading: the list goes by its column, or turns round when it does already.
        let (Some(app), Some(order)) = (weak.upgrade(), Order::from_column(&column)) else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.reversed = p.order == order && !p.reversed;
            p.order = order;
        });
        refresh(&app);
    });
    let weak = app.as_weak();
    data.on_shuffle(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.seed = new_seed();
            p.order = Order::Shuffled;
            p.reversed = false;
        });
        refresh(&app);
    });
    let weak = app.as_weak();
    data.on_toggle_autoplay(move || {
        let Some(app) = weak.upgrade() else { return };
        let on = PAGE.with(|p| p.borrow_mut().toggle_autoplay());
        app.global::<SoundsData>().set_autoplay(on);
        // Kept for the next start (review G-3).
        crate::settings::remember(AUTOPLAY_KEY, if on { "1" } else { "0" });
    });
    let weak = app.as_weak();
    data.on_pick_trim(move |picked| {
        let Some(app) = weak.upgrade() else { return };
        // "" turns it off; a level ("-50 dB") turns it on at that level. Both kept for next time, in the app's own
        // settings; they count from the next play (the one playing goes on).
        let picked = mo_audio::trim::parse_pick(&picked);
        let on = picked.is_some();
        let db = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.trim = on;
            if let Some(db) = picked {
                p.trim_db = db;
            }
            p.trim_chosen = true;
            p.trim_db
        });
        let data = app.global::<SoundsData>();
        data.set_trim_silence(on);
        data.set_trim_level_db(db);
        crate::settings::remember(list::TRIM_SILENCE_KEY, if on { "on" } else { "off" });
        crate::settings::remember(list::TRIM_LEVEL_KEY, &db.to_string());
    });
    let weak = app.as_weak();
    data.on_pick_view(move |v| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().pick_view(v));
        keep_view(v);
        show_lists(&app);
        bring_picked_into_view(&app);
        // Rediscover fills itself the first time it's opened, so it never greets you empty (as in the C# app).
        if v == REDISCOVER && PAGE.with(|p| !p.borrow().re_started) {
            rediscover_now(&app);
        }
        if v == KITS {
            if PAGE.with(|p| p.borrow().picked_off_the_kit()) {
                app.global::<SoundsData>().invoke_close_panel();
            }
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
    data.on_list_made(move || {
        // The tiles and the List each come back where they were when they're made again: back on the page, or
        // back from "No sounds match". Going from one to the other brings the picked sound into view, as the
        // other may never have shown it.
        let other = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let tiles = p.view == TILES;
            let other = p.made_tiles.replace(tiles) != Some(tiles);
            p.columns_before = (tiles && !other).then_some(p.columns);
            other
        });
        // A turn later, once the new list is laid out: an ask made while it's still being made is the one its
        // `changed show-ask` starts from, so it wouldn't scroll.
        let weak = weak.clone();
        Timer::single_shot(Duration::ZERO, move || {
            let Some(app) = weak.upgrade() else { return };
            let moved = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.columns_before.take().is_some_and(|before| before != p.columns)
            });
            if other || moved {
                bring_picked_into_view(&app);
            }
        });
    });
    let weak = app.as_weak();
    data.on_columns_changed(move |n| {
        let Some(app) = weak.upgrade() else { return };
        let n = n.max(1) as usize;
        let (changed, laying_out) = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            (std::mem::replace(&mut p.columns, n) != n, p.columns_before.is_some())
        });
        if changed {
            TILE_ROWS.with(|m| m.notify.reset());
            grid::changed();
            if !laying_out {
                bring_picked_into_view(&app);
            }
        }
    });
    let weak = app.as_weak();
    data.on_select(move |row| {
        let Some(app) = weak.upgrade() else { return };
        if let Some(at) = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().entry(r))) {
            clicked(&app, at);
        }
    });
    // A right click over the waveform of the sound playing (on its tile or its card).
    let weak = app.as_weak();
    data.on_stop_sound(move || {
        if let Some(app) = weak.upgrade() {
            stop(&app);
        }
    });
    let weak = app.as_weak();
    // A double-click: a play you meant, kept for History and Most liked (a single click, autoplay, the arrow keys and
    // the galaxy's sweep only try sounds out, as in the C# app).
    data.on_play_row(move |row| {
        let Some(app) = weak.upgrade() else { return };
        // Not one the list has just lost (it changed between the clicks): nothing was played.
        let Some(at) = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().entry(r))) else { return };
        select_row(&app, row, false);
        play_picked(&app, 0.0);
        did::played(&app, at);
    });
    // A sound like the picked one, clicked as its tile would be ([`clicked`]), or played on a double-click.
    let weak = app.as_weak();
    data.on_pick_alike(move |place| {
        let Some(app) = weak.upgrade() else { return };
        let Ok(at) = usize::try_from(place) else { return };
        clicked(&app, at);
    });
    let weak = app.as_weak();
    data.on_play_alike(move |place| {
        let Some(app) = weak.upgrade() else { return };
        let Ok(at) = usize::try_from(place) else { return };
        pick(&app, at);
        play_picked(&app, 0.0);
        did::played(&app, at);
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
                let to = f64::from(at) * seconds;
                match PLAYER.get() {
                    Some(player) if PAGE.with(|p| seeks(&p.borrow(), player.playing().as_deref(), to)) => {
                        player.seek(to);
                    }
                    // Still being read, or read only as far as its start: read again from there (newest wins).
                    _ => play_picked(&app, f64::from(at)),
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
        // The galaxy's ring leaves the star with it.
        galaxy::show_picked(&app);
    });
    wire_yours(app, &data);
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

/// The card's heart, Your type menu and tags ([`yours`]).
fn wire_yours(app: &AppWindow, data: &SoundsData) {
    let choices: Vec<SharedString> = yours::choices().into_iter().map(SharedString::from).collect();
    data.set_type_choices(ModelRc::new(VecModel::from(choices)));
    let weak = app.as_weak();
    data.on_toggle_favourite(move || {
        if let Some(app) = weak.upgrade() {
            yours::favourite(&app);
        }
    });
    let weak = app.as_weak();
    data.on_set_type(move |t| {
        if let Some(app) = weak.upgrade() {
            yours::own_type(&app, &t);
        }
    });
    let weak = app.as_weak();
    data.on_add_tag(move |t| {
        if let Some(app) = weak.upgrade() {
            yours::tag(&app, &t);
        }
    });
    let weak = app.as_weak();
    data.on_remove_tag(move |t| {
        if let Some(app) = weak.upgrade() {
            yours::untag(&app, &t);
        }
    });
    // Favourites from Bitwig ([`bitwig`]): its card, a multisample's tick (-1 all, -2 none), and Mark as favourites
    // (or Stop).
    let weak = app.as_weak();
    data.on_bitwig_toggle(move || {
        if let Some(app) = weak.upgrade() {
            bitwig::toggle(&app);
        }
    });
    let weak = app.as_weak();
    data.on_bitwig_tick(move |at| {
        if let Some(app) = weak.upgrade() {
            bitwig::tick(&app, at);
        }
    });
    let weak = app.as_weak();
    data.on_bitwig_mark(move || {
        if let Some(app) = weak.upgrade() {
            bitwig::mark(&app);
        }
    });
    let weak = app.as_weak();
    data.on_bitwig_save(move || {
        if let Some(app) = weak.upgrade() {
            bitwig::save(&app);
        }
    });
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
    /// The sound each one sounds like, by id (keyed by the found one's id), when its line names one.
    likes: HashMap<i64, i64>,
    /// Why nothing was found, when nothing was.
    why: String,
    folders: Vec<FolderUse>,
    years: YearCounts,
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
            let suggested = rediscover::suggest(sounds, usage, &self.index, o);
            let likes = suggested.iter().filter_map(|s| Some((s.id, s.like?))).collect();
            let found: Vec<(usize, String)> =
                suggested.into_iter().filter_map(|s| self.places.get(&s.id).map(|&i| (i, s.reason))).collect();
            let why = if found.is_empty() { rediscover::why_nothing(sounds, &self.index, o) } else { String::new() };
            let mut folders = rediscover::folders(sounds, usage);
            folders.truncate(FOLDERS);
            RediscoverAnswer {
                id: self.id,
                found,
                likes,
                why,
                folders,
                years: year_strip::strip(rediscover::years(sounds)),
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
        let intro = match PAGE.with(|p| p.borrow().no_index.clone()) {
            Some(why) => no_index_note("there's nothing to dig sounds out by", &why),
            None => "Listening to your sounds first: their fingerprints are still being read, and the list fills in \
                     by itself."
                .to_owned(),
        };
        data.set_re_intro(intro.into());
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
        p.re_likes = a.likes;
        p.re_busy = false;
        p.rediscover = a.found;
        p.re_years = a.years;
        Some((p.rediscover.len(), p.list.knows_use()))
    });
    let Some((found, knows_use)) = fresh else { return };
    let data = app.global::<SoundsData>();
    data.set_re_busy(false);
    // The picked sound's Used in list names the projects of the sound its line now says it sounds like.
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some(at) = p.picked {
            data.set_picked(picked_sound(&p, at));
        }
    });
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
    show_rediscover_chips(app);
    if found == 0 {
        data.set_re_empty_title("Nothing to suggest".into());
        data.set_re_empty_text(a.why.into());
        data.set_re_intro("".into());
    } else {
        let intro = if !knows_use {
            "Your projects haven't been counted yet, so these are a varied pick. They fit your taste once they are."
                .to_owned()
        } else if a.learned_from == 0 {
            "None of your sounds show up in your projects yet, so these are a varied pick. They fit your taste once \
             your songs use some of them."
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

/// Picks `value` on Rediscover's `what`, saying whether that changed anything (its years are picked on the year
/// strip). "type": as on the Type filter, a click picks one more type or unpicks it, and "" is any type (the
/// whole-app review of 6 Nov, X3-23: it picked one at most, so Kick then Snare lost Kick). "more-types": the More
/// types menu's Any, which unpicks the types picked in that menu but keeps the ones picked on tick lines of their
/// own, by the Type filter's own rule (`types` is the type counts they're shown from, the first `lines` of them on
/// tick lines). "count": one of the counts is always picked.
fn re_pick(re: &mut rediscover::Options, what: &str, value: &str, types: &[(String, usize)], lines: usize) -> bool {
    let before = (re.sound_types.clone(), re.count);
    match what {
        "type" if value.is_empty() => re.sound_types.clear(),
        "type" => filters::toggle(&mut re.sound_types, value, true),
        "more-types" => list::keep_first_types(&mut re.sound_types, types, lines),
        "count" => {
            if let Some(c) = value.parse().ok().filter(|c| COUNTS.contains(c)) {
                re.count = c;
            }
        }
        _ => {}
    }
    before != (re.sound_types.clone(), re.count)
}

/// Rediscover's choices as they are now, each on show: the types as the Type filter shows them (the most used as
/// chips, the rest in a menu), the years, and how many. Nothing is counted: Rediscover digs out how many it's asked
/// for.
fn show_rediscover_chips(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        show_re_types(&data, &p);
        data.set_re_any_type(!p.re.sound_types.is_empty());
        let counts = filters::one_of(&COUNTS.map(|c| c.to_string()), &p.re.count.to_string());
        CHOICES.with(|c| data.set_re_count_filter(c.re_count.view(&counts)));
        data.set_re_adventure(p.re.adventure as f32);
        data.set_re_rarely(p.re.max_uses > 0);
    });
    show_re_years(app);
}

/// Rediscover's types: as many on tick lines as the filter column has room for, the rest in its More types.
fn show_re_types(data: &SoundsData, p: &Page) {
    let types: Vec<(String, String)> = p.types.iter().map(|(t, _)| (t.clone(), String::new())).collect();
    let types = filters::show_split(&types, &p.re.sound_types, p.re_type_lines);
    CHOICES.with(|c| data.set_re_type_filter(c.re_type.view(&types)));
}

/// Rediscover's year strip, as [`show_years`] shows the filters' one: each year, how full it is, and the span picked.
fn show_re_years(app: &AppWindow) {
    let (bars, (from, to)) = PAGE.with(|p| {
        let p = p.borrow();
        (year_bars(&p.re_years), year_strip::picked_places(&p.re_years, p.re.years).unwrap_or((-1, -1)))
    });
    let data = app.global::<SoundsData>();
    data.set_re_years(ModelRc::new(VecModel::from(bars)));
    data.set_re_years_from(from);
    data.set_re_years_to(to);
}

/// The bars of a year strip: each year, and how full it is next to the fullest.
fn year_bars(years: &[(i32, usize)]) -> Vec<YearBar> {
    let most = years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
    years.iter().map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most }).collect()
}

/// Changes Rediscover's choices with `f` and, once it has a list, picks again (the newest ask wins).
fn re_change(app: &AppWindow, f: impl FnOnce(&mut Page)) {
    let again = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        f(&mut p);
        p.re_started
    });
    show_rediscover_chips(app);
    if again {
        rediscover_now(app);
    }
}

fn wire_rediscover(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    // Once there's a list, changing what it's picked by picks it again (as in the C# app); the newest ask wins.
    let weak = app.as_weak();
    data.on_re_pick(move |what, value| {
        let Some(app) = weak.upgrade() else { return };
        let (again, count) = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let p = &mut *p;
            // Picking again only when something changed: a click on the count picked already keeps the list.
            let changed = re_pick(&mut p.re, &what, &value, &p.types, p.re_type_lines);
            (changed && p.re_started, (changed && what == "count").then_some(p.re.count))
        });
        // How many is kept for the next start (review G-3).
        if let Some(count) = count {
            crate::settings::remember(COUNT_KEY, &count.to_string());
        }
        show_rediscover_chips(&app);
        if again {
            rediscover_now(&app);
        }
    });
    // The year strip, as on Tiles, List and Galaxy: a drag over it picks a span of years, and its Clear any year.
    let weak = app.as_weak();
    data.on_re_years_picked(move |a, b| {
        let Some(app) = weak.upgrade() else { return };
        let span = PAGE.with(|p| year_strip::year_span(&p.borrow().re_years, a, b));
        if span.is_some_and(|span| PAGE.with(|p| p.borrow().re.years != Some(span))) {
            re_change(&app, |p| p.re.years = span);
        }
    });
    let weak = app.as_weak();
    data.on_re_years_cleared(move || {
        let Some(app) = weak.upgrade() else { return };
        if PAGE.with(|p| p.borrow().re.years.is_some()) {
            re_change(&app, |p| p.re.years = None);
        }
    });
    // The slider picks again once it's let go, not while it moves.
    let weak = app.as_weak();
    data.on_re_adventure_set(move |v| {
        if let Some(app) = weak.upgrade() {
            re_change(&app, |p| p.re.adventure = f64::from(v.clamp(0.0, 1.0)));
        }
    });
    let weak = app.as_weak();
    data.on_re_toggle_rarely(move || {
        if let Some(app) = weak.upgrade() {
            re_change(&app, |p| p.re.max_uses = if p.re.max_uses == 0 { 1 } else { 0 });
        }
    });
    let weak = app.as_weak();
    data.on_re_shuffle(move || {
        if let Some(app) = weak.upgrade() {
            rediscover_now(&app);
        }
    });
    // A forgotten folder: its own sounds in the List (the ones its card counts, not its subfolders' nor a folder whose
    // name starts the same way), with the other filters and the search cleared so all of them show.
    let weak = app.as_weak();
    data.on_pick_folder(move |folder| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let hide_copies = p.filters.hide_copies;
            p.filters = Filters { folder: Some(folder.to_string()), hide_copies, ..Filters::default() };
            p.pick_view(LIST);
        });
        keep_view(LIST);
        app.global::<SoundsData>().set_search(SharedString::new());
        refresh(&app);
        show_lists(&app);
    });
}

/// Each year on the year strip, newest first, with how many sounds it holds.
type YearCounts = Vec<(i32, usize)>;

/// Keeps the view `v` you moved to for the next start: the page opens on the view you left it on (review G-3), however
/// you got there.
fn keep_view(v: i32) {
    if let Some(name) = usize::try_from(v).ok().and_then(|i| VIEWS.get(i)) {
        crate::settings::remember(VIEW_KEY, name);
    }
}

/// Keeps in `kept` how many types the filter column has room for as tick lines, `lines` as the page counted them
/// (0 while the view they're for is away: `kept` stays as it is), and never fewer than the usual
/// [`list::TYPE_CHIPS`], as in a window too short for them. Says whether that's a new count, which shows them again.
fn room(kept: &mut usize, lines: i32) -> bool {
    let Some(lines) = usize::try_from(lines).ok().filter(|&n| n > 0) else { return false };
    let lines = lines.max(list::TYPE_CHIPS);
    std::mem::replace(kept, lines) != lines
}

fn change(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut Filters)) {
    let Some(app) = weak.upgrade() else { return };
    PAGE.with(|p| f(&mut p.borrow_mut().filters));
    refresh(&app);
}

/// Asks for the list again with the filters and order as they are now, and shows the chips meanwhile. With Hide
/// duplicates on, the copies are looked for first if they aren't known for these sounds ([`copies::wanted`]).
fn refresh(app: &AppWindow) {
    copies::wanted(app);
    like::wanted(app);
    show_chips(app);
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.asked += 1;
        Ask {
            id: p.asked,
            list: p.list.clone(),
            filters: p.filters.clone(),
            order: p.order,
            reversed: p.reversed,
            seed: p.seed,
            played: p.played.clone(),
        }
    });
    if HERE.with(|h| *h.borrow()) {
        let (mut shown, counts) = ask.list.view_counted(&ask.filters, ask.order, ask.seed);
        if ask.reversed {
            ask.list.turn_round(&mut shown, ask.order);
        }
        let years = ask.list.year_counts(ask.filters.used);
        let history = ask.list.played(&ask.played, &ask.filters);
        listed(app, ask.id, Listed { shown, counts, years, history, played: ask.played });
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
                    let (mut shown, counts) = ask.list.view_counted(&ask.filters, ask.order, ask.seed);
                    if ask.reversed {
                        ask.list.turn_round(&mut shown, ask.order);
                    }
                    Listed { shown, counts, years, history, played: ask.played.clone() }
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
    /// What was played, as History was worked out from.
    played: Arc<Vec<(usize, NaiveDateTime)>>,
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
        // Not while only the first few sounds are in: their counts would be a few of all.
        p.counts = p.partial.is_none().then_some(new.counts);
        p.years = new.years;
        // What History lists: what was played as it shows it while you play from it ([`did::take`]); a sound played
        // while the list was worked out is listed too.
        let from = match &p.history_behind {
            Some(was) if p.view == HISTORY => was.clone(),
            _ => {
                p.history_behind = None;
                p.played.clone()
            }
        };
        p.history = if Arc::ptr_eq(&new.played, &from) { new.history } else { p.list.played(&from, &p.filters) };
        true
    });
    if current {
        // The counts beside the filters' choices, for the filters as they are now.
        show_chips(app);
        show_years(app);
        show_lists(app);
        // Once the sounds are read, the list (or "No sounds match") takes the place of the word saying so. A library
        // with no sounds in it says so on every view instead, as the word did.
        let (read, none, waiting) = PAGE.with(|p| {
            let p = p.borrow();
            // Only the first few are in, and none of them on this view: more may be, once the rest arrive. (Kits
            // lists kits, not sounds.)
            (p.read, p.list.is_empty(), p.partial.is_some() && p.view != KITS && p.entries() == 0)
        });
        if read {
            let (title, text) = if none {
                NO_SOUNDS
            } else if waiting {
                (READING, "")
            } else {
                ("", "")
            };
            let data = app.global::<SoundsData>();
            data.set_empty_title(title.into());
            data.set_empty_text(text.into());
        }
        // The filters dim the stars they leave out.
        galaxy::draw(app);
        if let Some(at) = PAGE.with(|p| p.borrow_mut().to_pick.take()) {
            pick(app, at);
        }
    }
}

/// The Space page counts the copies of the same sound: they're looked for (as Hide duplicates does) from now on.
pub fn copies_for_space(app: &AppWindow) {
    copies::for_space(app);
}

/// Space's Measure again: the copies are looked for again in the files as they are now (only files changed since the
/// last look are read).
pub fn copies_look_again(app: &AppWindow) {
    copies::look_again(app, false);
}

/// Whether Hide duplicates is on.
pub fn hides_copies() -> bool {
    PAGE.with(|p| p.borrow().filters.hide_copies)
}

/// How far looking for the copies has got (Hide duplicates', which the Space page counts too).
pub fn copies_found() -> list::CopiesFound {
    copies::found()
}

/// For the screenshots: the copies Hide duplicates hides, as if they'd been found (`None`: none, not looked for).
pub fn show_copies_for_screens(app: &AppWindow, same: Option<&mo_sounds::copies::SameAudio>) {
    copies::show_for_screens(app, same);
}

/// What the screenshots show on Favourites from Bitwig's card ([`show_bitwig_for_screens`]).
pub struct BitwigShot<'a> {
    pub names: &'a [&'a str],
    pub ticked: &'a [&'a str],
    pub said: &'a str,
    pub missing: &'a [&'a str],
    /// How many of them Save as sounds would save (into a made-up I:\Samples).
    pub saveable: usize,
}

/// For the screenshots: Favourites from Bitwig's card open on made-up multisamples (`ticked` by name), with what a
/// look said and the samples it didn't find (none for a card not used yet); `None` closes it. Never reads a folder.
pub fn show_bitwig_for_screens(app: &AppWindow, shown: Option<BitwigShot<'_>>) {
    match shown {
        Some(BitwigShot { names, ticked, said, missing, saveable }) => {
            let found = names
                .iter()
                .map(|n| mo_app::bitwig_favourites::Multisample {
                    path: std::path::PathBuf::from(format!(r"K:\Bitwig Studio\Library\Multisamples\{n}.multisample")),
                    name: (*n).to_owned(),
                })
                .collect();
            bitwig::show_for_screens(app, found, ticked, said, missing, saveable);
        }
        None => bitwig::close(app),
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
        // Hide duplicates stays on unless the sound has copies (one of them might show in its place).
        let hide_copies = p.filters.hide_copies && !p.list.has_copies(at);
        p.filters = Filters { hide_copies, ..Filters::default() };
        p.to_pick = Some(at);
    });
    app.global::<SoundsData>().set_search(SharedString::new());
    refresh(app);
}

/// Opens the page on the sound with id `id` and picks it, as Ctrl+K does: the Space page's links to it. False when the
/// list doesn't have it (it's hidden, or gone since).
pub fn show_sound_by_id(app: &AppWindow, id: i64) -> bool {
    let Some(at) = PAGE.with(|p| p.borrow().places.get(&id).copied()) else { return false };
    crate::go_to(app, Place::Sounds.index() as i32);
    show_sound(app, at);
    true
}

/// The sounds as the page has them, for the Space page, which counts their copies ([`known`]).
pub enum Known {
    /// Still being read.
    Reading,
    /// There's no sound library, or it couldn't be read.
    Unread,
    /// The sounds the page lists (hidden copies left out), with how your songs use them once that's known, and the
    /// sound library they were read from.
    Read(Arc<SoundList>, PathBuf),
}

/// The sounds as the page has them now. The Space page is told each time that changes (`space::sounds_changed`).
pub fn known() -> Known {
    PAGE.with(|p| {
        let p = p.borrow();
        match (p.read, p.unread, &p.galaxy.db) {
            (_, true, _) | (true, false, None) => Known::Unread,
            // Only the first few so far.
            _ if p.partial.is_some() => Known::Reading,
            (true, false, Some(db)) => Known::Read(p.list.clone(), db.clone()),
            (false, false, _) => Known::Reading,
        }
    })
}

/// The year strip: each year, how full it is, and whether the filter has it.
fn show_years(app: &AppWindow) {
    let (bars, (from, to)) = PAGE.with(|p| {
        let p = p.borrow();
        (year_bars(&p.years), year_strip::picked_places(&p.years, p.filters.years).unwrap_or((-1, -1)))
    });
    let data = app.global::<SoundsData>();
    data.set_years(ModelRc::new(VecModel::from(bars)));
    data.set_years_from(from);
    data.set_years_to(to);
}

fn show_lists(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| p.borrow_mut().now = Local::now().naive_local());
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_view(p.view);
        data.set_shown(p.entries() as i32);
        let total = p.list.len();
        let summary = if let Some(all) = p.partial {
            format!("Reading the rest of your {} sounds…", thousands(all))
        } else if p.list.is_empty() {
            String::new()
        } else if p.view == HISTORY
            && (Filters { years: None, ..p.filters.clone() }.any() || !p.filters.search.trim().is_empty())
        {
            // The years picked on the year strip don't narrow History, so they don't count here either.
            format!("{} of {} played", thousands(p.history.len()), thousands(p.played.len()))
        } else if p.view != HISTORY
            && p.view != REDISCOVER
            && (p.filters.any()
                || !p.filters.search.trim().is_empty()
                || (p.filters.hide_copies && p.list.copies().is_some_and(|n| n > 0)))
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

/// The Type filter's chips alone: as many types on tick lines as the filter column has room for, the rest in More
/// types (the other filters' chips are made too, and left as they are).
fn show_types(data: &SoundsData, p: &Page) {
    let chips = list::chips_showing(&p.filters, p.counts.as_ref(), &p.types, &p.tags, p.type_lines);
    FILTER_MODELS.with(|m| data.set_type_filter(m[0].view(&chips.types)));
}

/// The filters' chips and menus as the filters are now, each choice with the counts from the newest list worked out
/// (a pick shows at once; its counts come with the list it asked for).
fn show_chips(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let f = &p.filters;
        let chips = list::chips_showing(f, p.counts.as_ref(), &p.types, &p.tags, p.type_lines);
        FILTER_MODELS.with(|m| {
            data.set_type_filter(m[0].view(&chips.types));
            data.set_length_filter(m[1].view(&chips.length));
            data.set_key_filter(m[2].view(&chips.key));
            data.set_tag_filter(m[3].view(&chips.tag));
        });
        data.set_tempo_low(chips.tempo.low);
        data.set_tempo_high(chips.tempo.high);
        data.set_tempo_words(chips.tempo.words.into());
        data.set_tempo_bars(ModelRc::new(VecModel::from(chips.tempo.bars)));
        data.set_any_type(!f.sound_types.is_empty());
        data.set_mix_well(f.mix_well);
        data.set_unused(f.unused);
        data.set_used(f.used);
        data.set_knows_use(p.list.knows_use());
        data.set_favourites(f.favourites);
        data.set_like_favourites(list::like_favourites_share(f.like_favourites));
        // Without fingerprints nothing can be found like them: it says so rather than "Finding…" for good.
        let known = match (p.list.knows_like_favourites() && !like::busy(), &p.no_index) {
            (true, _) => list::LikeKnown::Yes,
            (false, Some(_)) => list::LikeKnown::Missing,
            (false, None) => list::LikeKnown::Finding,
        };
        data.set_like_favourites_words(list::like_favourites_words(f.like_favourites, known).into());
        data.set_hide_copies(f.hide_copies);
        data.set_hide_copies_text(list::hide_copies_text(copies::found()).into());
        data.set_folder(f.folder.as_deref().unwrap_or_default().into());
        data.set_any_filter(f.any());
        let order = filters::one_of(&Order::ALL.map(Order::name), p.order.name());
        data.set_order_filter(CHOICES.with(|c| c.order.view(&order)));
        // In the List view the heading of the column the list goes by is lit too, with which way (review B5).
        data.set_order_column(p.order.column().into());
        data.set_order_reversed(p.reversed);
        data.set_autoplay(p.autoplay);
        data.set_trim_silence(p.trim);
        data.set_trim_level_db(p.trim_db);
        data.set_trim_choices(slint::ModelRc::new(slint::VecModel::from(
            mo_audio::trim::choices().into_iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
        show_picked_tags(&data, &p);
    });
}

/// The picked sound's tags on its panel, each lit while the Tag filter has it.
fn show_picked_tags(data: &SoundsData, p: &Page) {
    let tags = p.picked.and_then(|at| p.list.now(at)).map(|now| now.tags).unwrap_or_default();
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

/// A click on a sound, on its tile, list line, kit pad or star, or on a line of Sounds like this or of the Galaxy's
/// Swept card ([`click_on`]): the sound playing stops when it's the one clicked, and stays picked (Solomon, 11 Nov
/// 2026: "clicking it again stops the sound so we can still have a right click menu"); any other is picked, and played
/// with Autoplay on. A double-click still plays: its second click stops the sound, then the double-click plays it
/// again. A list line's pick for its right-click menu (`AppKeys.picking-for-menu`) only picks: the sound playing goes
/// on and none starts, so the menu has the sound it was opened on, playing or not.
pub(super) fn clicked(app: &AppWindow, at: usize) {
    let autoplay = PAGE.with(|p| p.borrow().autoplay);
    click_on(app, at, autoplay);
}

/// [`clicked`], with `play` in place of Autoplay: whether a sound the click picks plays. A line of the Galaxy's Swept
/// card plays the sound it goes to, Autoplay or not.
pub(super) fn click_on(app: &AppWindow, at: usize, play: bool) {
    let for_menu = app.global::<AppKeys>().get_picking_for_menu();
    let does = PAGE.with(|p| {
        let p = p.borrow();
        click_does(at, p.playing.map(|(i, _)| i), p.picked, play, for_menu)
    });
    match does {
        Click::Stop { pick: also } => {
            stop(app);
            if also {
                pick(app, at);
            }
        }
        Click::Pick { play } => {
            pick(app, at);
            if play {
                play_picked(app, 0.0);
            }
        }
    }
}

/// What a click on the sound at `at` does ([`clicked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Click {
    /// It's the one playing: it stops, and is picked if it wasn't.
    Stop { pick: bool },
    /// It's picked, and played when `play`.
    Pick { play: bool },
}

/// What a click on the sound at `at` does, with the sound at `playing` playing and the one at `picked` picked: `play`
/// is whether a sound it picks plays (the page's Autoplay, mostly), `for_menu` a list line's pick for its right-click
/// menu.
fn click_does(at: usize, playing: Option<usize>, picked: Option<usize>, play: bool, for_menu: bool) -> Click {
    if for_menu {
        Click::Pick { play: false }
    } else if playing == Some(at) {
        Click::Stop { pick: picked != Some(at) }
    } else {
        Click::Pick { play }
    }
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
    let Some(now) = p.list.now(at) else { return PickedSound::default() };
    let s = now.sound;
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
    let used = used_in(p, at);
    let projects: Vec<UsedInLine> = used
        .lines
        .iter()
        .map(|l| UsedInLine {
            id: i32::try_from(l.id).unwrap_or(-1),
            name: l.name.as_str().into(),
            daw: l.daw.as_str().into(),
            saved: l.saved.as_str().into(),
        })
        .collect();
    let type_from = match now.type_source {
        "you" => "· you set this",
        "name" => "· from its name",
        "learned" => "· learned from your library",
        "model" => "· guessed from how it sounds",
        _ => "",
    };
    PickedSound {
        name: s.name().into(),
        sound_type: now.sound_type.into(),
        colour: list::type_colour(now.sound_type),
        type_from: type_from.into(),
        path: mo_app::shell::wrappable_path(&s.path).into(),
        wave: wave(s).into(),
        points: points(s),
        seconds: a.map_or(0.0, |a| a.seconds.min(mo_audio::MAX_SECONDS)) as f32,
        facts: ModelRc::new(VecModel::from(facts)),
        favourite: now.favourite,
        own_type: now.own_type.unwrap_or_default().into(),
        character: a.map(|a| a.character.join(", ")).unwrap_or_default().into(),
        projects: ModelRc::new(VecModel::from(projects)),
        projects_title: used.title.into(),
        projects_note: used.note.into(),
    }
}

/// The picked sound's Used in list: the projects that use it; or, for one Rediscover dug out that none use, those of
/// the sound its line says it sounds like.
fn used_in(p: &Page, at: usize) -> list::ProjectsCard {
    let own = p.list.projects_of(at);
    let like = p
        .list
        .sound(at)
        .and_then(|s| p.re_likes.get(&s.id))
        .and_then(|id| p.places.get(id))
        .and_then(|&i| Some((p.list.sound(i)?.name(), p.list.projects_of(i)?)));
    list::projects_card(p.list.dates(), own.as_deref(), like.as_ref().map(|(name, of)| (*name, of.as_slice())))
}

/// The arrow keys: across a row of tiles (-1, 1) or down and up the list (-2, 2), a row of tiles at a time; on the
/// galaxy, by the map ([`galaxy::arrow`]).
fn step(app: &AppWindow, d: i32, held: bool) {
    let next = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // A held key's repeats wait for the sound they're on to have played (with Autoplay on).
        let (autoplay, step) = (p.autoplay, p.step);
        if !p.stepping.may_step(autoplay, held, step, Instant::now()) {
            return None;
        }
        if p.view == GALAXY {
            return Some(None);
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
        Some(Some(row))
    });
    match next {
        Some(Some(row)) => select_row(app, row as i32, true),
        // The galaxy's arrows go by the map, not the list's order.
        Some(None) => galaxy::arrow(app, d),
        None => {}
    }
}

/// Space and the Play button: stops the picked sound if it's playing, else plays it (a play you meant, kept for
/// History and Most liked).
pub fn toggle_play(app: &AppWindow) -> bool {
    let (picked, playing) = PAGE.with(|p| {
        let p = p.borrow();
        (p.picked, p.playing)
    });
    match (picked, playing) {
        (Some(i), Some((j, _))) if i == j => stop(app),
        (Some(i), _) => {
            play_picked(app, 0.0);
            did::played(app, i);
        }
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
/// galaxy's sweep plays the stars it passes without picking them), reading at most `most` seconds of it: a sweep only
/// needs its start, which reads far quicker than a whole loop.
fn play_sound(app: &AppWindow, at: usize, from: f64, most: f64) {
    let Some((at, path, seconds, trim)) = PAGE.with(|p| {
        let p = p.borrow();
        let s = p.list.sound(at)?;
        let seconds = s.analysis.as_ref().map_or(0.0, |a| a.seconds).min(mo_audio::MAX_SECONDS);
        Some((at, s.path.clone(), seconds, p.trim.then(|| mo_audio::trim::level_of_db(p.trim_db))))
    }) else {
        return;
    };
    // A MIDI loop playing, or one still being made, would play over it (or start on top of it later).
    crate::midi::stop_for_another(app);
    // The screenshots show a sound playing without a sound card.
    let here = HERE.with(|h| *h.borrow());
    let ask = if here { 0 } else { PLAYS.fetch_add(1, Ordering::SeqCst) + 1 };
    // Straight after, so a read this one overtakes never takes it for a stop.
    NEWEST_PLAY.fetch_max(ask, Ordering::SeqCst);
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.playing = Some((at, seconds.max(0.001)));
        p.play = ask;
        p.play_reads = most;
        p.play_trimmed = trim.is_some();
    });
    playing_changed(app);
    if here {
        return;
    }
    let job = PlayJob { ask, path, from, seconds, most, trim, window: app.as_weak(), held: Arc::default() };
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
    /// Trim silence was on when it was asked for, with the level it counts as silence: it starts past the silence at
    /// its start and stops at the end of the sound, not the end of the file.
    trim: Option<f32>,
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
    let PlayJob { ask, ref path, from, seconds, most, trim, ref window, .. } = job;
    let (path, window) = (path.clone(), window.clone());
    // Still the newest play, with no stop since.
    let wanted = || {
        if NEWEST_PLAY.load(Ordering::SeqCst) != ask {
            Err(Unwanted::Overtaken)
        } else if PLAYS.load(Ordering::SeqCst) != ask {
            Err(Unwanted::Stopped)
        } else {
            Ok(())
        }
    };
    // Passed over since it was asked for (it waited behind another read): never opened, so the sweep's timings
    // forget it by themselves.
    if wanted().is_err() {
        return;
    }
    sweep_timing::opening(&path, ask);
    // Trimmed, a sweep's short read is read on past the silence at the start, so it still hears `most` of the sound.
    let reads = if trim.is_some() && most < mo_audio::MAX_SECONDS { most + TRIM_LEAD_ROOM } else { most };
    let went = match mo_audio::read(Path::new(&path), reads) {
        // Looked at again as it starts, with no stop or other sound able to come in between: one asked for since
        // wins, rather than this sound starting after it and playing on while the page shows another or none.
        Ok(mut audio) => {
            let start = match trim {
                Some(level) => trimmed(&mut audio, from * seconds, most, level),
                None => from * seconds,
            };
            // Read as far as it was asked to (a sweep reads only a sound's start; a long one is read so far): it
            // stops there, so it fades out rather than clicking.
            if audio.sample_rate > 0 && audio.seconds() + 1.0 / f64::from(audio.sample_rate) >= reads.min(start + most)
            {
                audio.fade_out_end();
            }
            player().play_if(Arc::new(audio), ask.to_string(), start, wanted)
        }
        Err(e) => match wanted() {
            Err(unwanted) => Err(unwanted),
            Ok(()) => {
                couldnt_play(&path, ask, &e.to_string(), window);
                return;
            }
        },
    };
    match went {
        Ok(()) => {
            *STARTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((ask, Instant::now()));
            let output = player().info();
            sweep_timing::output(output.mix_every_ms.map(|m| m / 2.0), output.delay_ms);
            sweep_timing::started(&path, ask);
        }
        // A newer play came while this one was reading: overtaken. A stop (the mouse let go, say): dropped.
        Err(Unwanted::Overtaken) => sweep_timing::overtaken(&path, ask),
        Err(Unwanted::Stopped) => sweep_timing::dropped(&path, ask),
    }
}

/// How much further a sweep's read goes with Trim silence on, for the silence at the start it skips.
const TRIM_LEAD_ROOM: f64 = 2.0;

/// Trim silence for a sound about to play from `from` seconds, reading at most `most` seconds of it, with what's
/// quieter than `level` as silence: cuts the silence
/// at its end off (and what's past `most` of the sound, for a sweep's longer read) and gives where it starts, past the
/// silence at its start. The start stays in the samples, so the waveform's place and the play's position still mean
/// the same second of the file. A sound silent all through, or a play from inside the silence at the end, plays as
/// asked.
fn trimmed(audio: &mut mo_audio::AudioData, from: f64, most: f64, level: f32) -> f64 {
    let rate = f64::from(audio.sample_rate.max(1));
    let asked = (from * rate).round() as usize;
    let Some((first, end)) = mo_audio::trim::play_span(audio, asked, level) else {
        // Played as asked, as far as asked: a sweep's longer read is cut back to `most`.
        let keep = ((from + most) * rate).round() as usize;
        for channel in &mut audio.channels {
            channel.truncate(keep);
        }
        return from;
    };
    let start = if first > asked { first as f64 / rate } else { from };
    // The start stays in the samples (`keep` from 0); a cut end fades out.
    audio.keep(0, end.min(((start + most) * rate).round() as usize));
    start
}

/// Why a sound that was read isn't played.
enum Unwanted {
    /// A newer sound was asked for.
    Overtaken,
    /// A stop was asked for.
    Stopped,
}

/// Says that the sound of play `ask` couldn't be read (`why`), once for many that fail the same way, and stops it on
/// the page.
fn couldnt_play(path: &str, ask: u64, why: &str, window: slint::Weak<AppWindow>) {
    sweep_timing::dropped(path, ask);
    log::warning("Sounds", &format!("Couldn't play {path}: {why}"));
    let why = why.to_owned();
    let _ = slint::invoke_from_event_loop(move || {
        // Only while this is still the sound asked for: a newer one asked meanwhile keeps playing.
        if PLAYS.load(Ordering::SeqCst) != ask {
            return;
        }
        // Stepping or sweeping over many sounds that won't play for the same reason (a drive unplugged) says so once;
        // the log has each of them.
        let now = Instant::now();
        let new_card = FAILED_CARD.with(|c| {
            let mut c = c.borrow_mut();
            let fresh =
                c.as_ref().is_none_or(|(at, before)| *before != why || now.duration_since(*at) >= FAILED_CARD_GAP);
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

/// The least time between two "Couldn't play the sound" cards for the same reason: as long as one stays.
const FAILED_CARD_GAP: Duration = mo_app::activity::FAILED_STAYS;

thread_local! {
    /// When the last "Couldn't play the sound" card went up, and why the sound couldn't play.
    static FAILED_CARD: RefCell<Option<(Instant, String)>> = const { RefCell::new(None) };
}

fn stop(app: &AppWindow) {
    // Counted first: a read that looks after this ([`read_and_play`]) doesn't start its sound, and one that looked
    // before has started it by the time the player takes the stop.
    PLAYS.fetch_add(1, Ordering::SeqCst);
    if let Some(player) = PLAYER.get() {
        player.stop();
    }
    PAGE.with(|p| p.borrow_mut().playing = None);
    playing_changed(app);
}

/// Esc on the page (`app.escape`, in the search box or anywhere else), the same order on every page, once nothing plays
/// ([`stop_on_escape`]): the search is cleared; else the picked sound's card closes (the whole-app review of 6 Nov,
/// X3-12). Says whether it did either (when not, the key goes on).
pub fn escape(app: &AppWindow) -> bool {
    let data = app.global::<SoundsData>();
    if !data.get_search().is_empty() {
        data.set_search(SharedString::new());
        data.invoke_search_edited(SharedString::new());
        return true;
    }
    // Favourites from Bitwig's card lies over the sound card: it goes first.
    if data.get_bitwig_shown() {
        bitwig::close(app);
        return true;
    }
    if PAGE.with(|p| p.borrow().picked.is_some()) {
        data.invoke_close_panel();
        return true;
    }
    false
}

/// Esc's first step, on any page (`app.escape`): the sound playing stops, as it plays on for a while after its page
/// goes. Whether one was playing.
pub fn stop_on_escape(app: &AppWindow) -> bool {
    let playing = plays();
    if playing {
        stop(app);
    }
    playing
}

/// Whether a sound shown playing here plays now: what [`stop_on_escape`] stops.
pub fn plays() -> bool {
    PAGE.with(|p| p.borrow().playing.is_some())
}

/// Another page is about to play (the MIDI page's loop): the sound shown playing here stops, so this page doesn't go
/// on saying it plays.
pub fn stop_for_another(app: &AppWindow) {
    if PAGE.with(|p| p.borrow().playing.is_some()) {
        stop(app);
    }
}

/// Whether a click `to` seconds into the waveform of the sound shown playing can move the play the player has
/// (`player_has`: its key), rather than reading the sound again from there: only when the player has that very play
/// (not an older one, nor none while it's still being read) and the play read that far (a sweep reads only its start).
fn seeks(p: &Page, player_has: Option<&str>, to: f64) -> bool {
    p.playing.is_some()
        && player_has.is_some_and(|key| key.parse() == Ok(p.play))
        && to < p.play_reads
        && !p.play_trimmed
}

/// A sound played to its end by itself: `key` is its play's number.
fn ended(key: &str) {
    let ours = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // By the play's number, not its file: played again near its end, the same sound's older play ends while the
        // new one is still being read, and the page goes on showing the new one.
        let ours = p.playing.is_some() && key.parse() == Ok(p.play);
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
        p.picked.and_then(|at| p.list.sound(at)).map(|s| (s.id, s.path.clone(), s.name().to_owned()))
    });
    let feed = FEED.with(|f| f.borrow().clone());
    match (id, picked, feed) {
        (FAVOURITE, Some(_), _) => yours::favourite(app),
        // The card's tag box takes the keys.
        (TAG, Some(_), _) => {
            let data = app.global::<SoundsData>();
            data.set_tag_asked(data.get_tag_asked().wrapping_add(1));
        }
        (TO_BITWIG, Some((id, path, name)), Some(feed)) => send_to_bitwig(app, &feed, id, path, name),
        ("sounds.reveal", Some((_, path, _)), Some(feed)) => {
            crate::files::reveal(&feed, &path, mo_app::shell::folder_of(&path));
        }
        ("sounds.copyPath", Some((_, path, _)), Some(feed)) => crate::files::copy_path(&feed, &path),
        _ => return false,
    }
    true
}

/// Puts a sound (its id, file and name) on a Sampler on a new track in the project open in Bitwig, on a thread of its
/// own, as its file name (without the extension) names it. Its card shows at once and says how it went. While it goes,
/// Send to Bitwig waits. Once it's in, it counts towards how liked the sound is ([`did::sent`]).
fn send_to_bitwig(app: &AppWindow, feed: &Feed, id: i64, path: String, name: String) {
    if SENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    show_sending(app, true);
    let job = to_bitwig::sound_card(feed, &name);
    let weak = app.as_weak();
    let sent = background::spawn("Send to Bitwig", move || {
        // A panic in it is logged as a crash and fails the card as it goes; Send to Bitwig still comes back.
        let to = to_bitwig::Bitwig::default();
        let went_in =
            mo_catch::catch_crash(AssertUnwindSafe(|| to_bitwig::send_sound(job, &to, &path, &name))).unwrap_or(false);
        SENDING.store(false, Ordering::Release);
        let _ = weak.upgrade_in_event_loop(move |app| {
            show_sending(&app, false);
            if went_in {
                did::sent(&app, &[id]);
            }
        });
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

    #[test]
    fn a_click_on_the_sound_playing_stops_it_and_any_other_is_picked() {
        // The sound at 3 plays and is picked: a click on it stops it, and it stays picked.
        assert_eq!(click_does(3, Some(3), Some(3), true, false), Click::Stop { pick: false });
        // Playing, but not picked (its card was closed, or a sweep played it): stopped and picked, so its card shows.
        assert_eq!(click_does(3, Some(3), None, true, false), Click::Stop { pick: true });
        assert_eq!(click_does(3, Some(3), Some(5), false, false), Click::Stop { pick: true });
        // Any other sound: picked, and played with Autoplay on, whatever plays.
        assert_eq!(click_does(4, Some(3), Some(3), true, false), Click::Pick { play: true });
        assert_eq!(click_does(4, None, None, true, false), Click::Pick { play: true });
        assert_eq!(click_does(4, None, Some(4), false, false), Click::Pick { play: false });
    }

    #[test]
    fn a_pick_for_a_menu_never_stops_or_plays_a_sound() {
        for (playing, picked) in [(Some(3), Some(3)), (Some(3), None), (None, None), (Some(5), Some(5))] {
            for autoplay in [false, true] {
                assert_eq!(click_does(3, playing, picked, autoplay, true), Click::Pick { play: false });
            }
        }
    }

    /// 100 frames a second: silent for 0.2 s, loud for 0.3 s, silent to 1 s.
    fn quiet_loud_quiet() -> mo_audio::AudioData {
        let mut s = vec![0.0; 100];
        s[20..50].fill(0.5);
        mo_audio::AudioData { sample_rate: 100, channels: vec![s.clone(), s], ..Default::default() }
    }

    #[test]
    fn trim_silence_starts_past_the_quiet_start_and_stops_at_the_sounds_end() {
        let mut a = quiet_loud_quiet();
        let start = trimmed(&mut a, 0.0, mo_audio::MAX_SECONDS, 0.001);
        assert!((start - 0.2).abs() < 1e-9);
        // The start stays in the samples (the waveform's places still mean the same second); a 10 ms tail stays.
        assert_eq!((a.channels[0].len(), a.channels[1].len()), (51, 51));
        // A click past the start plays from there.
        let mut a = quiet_loud_quiet();
        assert!((trimmed(&mut a, 0.3, mo_audio::MAX_SECONDS, 0.001) - 0.3).abs() < 1e-9);
        // A sweep hears `most` of the sound, not of the silence before it.
        let mut a = quiet_loud_quiet();
        assert!((trimmed(&mut a, 0.0, 0.1, 0.001) - 0.2).abs() < 1e-9);
        assert_eq!(a.len(), 30);
    }

    #[test]
    fn trim_silence_leaves_silence_all_through_and_a_click_in_the_quiet_end_as_asked() {
        let mut a = quiet_loud_quiet();
        assert_eq!(trimmed(&mut a, 0.8, mo_audio::MAX_SECONDS, 0.001), 0.8);
        assert_eq!(a.len(), 100);
        let mut quiet = mo_audio::AudioData { sample_rate: 100, channels: vec![vec![0.0; 50]], ..Default::default() };
        assert_eq!(trimmed(&mut quiet, 0.1, mo_audio::MAX_SECONDS, 0.001), 0.1);
        assert_eq!(quiet.len(), 50);
    }

    #[test]
    fn rediscover_picks_types_as_the_type_filter_does_and_always_a_count() {
        let mut re = rediscover::Options { count: COUNT, ..rediscover::Options::default() };
        let types = [("Kick".to_owned(), 9), ("Snare".to_owned(), 5)];
        let mut pick = |what: &str, value: &str| re_pick(&mut re, what, value, &types, list::TYPE_CHIPS);
        // As on the Type filter: each click picks one more type, and a click on one picked unpicks it (X3-23).
        assert!(pick("type", "Kick"));
        assert!(pick("type", "Snare"));
        assert!(pick("type", "snare"));
        assert!(pick("type", "Snare"));
        // Any type unpicks them all, and changes nothing when none is picked.
        assert!(pick("type", ""));
        assert!(!pick("type", ""));
        // The More types menu's Any keeps the types picked on their chips, and unpicks the ones picked in the menu.
        assert!(pick("type", "Snare"));
        assert!(!pick("more-types", ""));
        assert!(pick("type", "Riser"));
        assert!(pick("type", "Kick"));
        assert!(pick("more-types", ""));
        // The years are the year strip's, not a pick here.
        assert!(!pick("year", "2024"));
        // A count on offer is picked; the one picked already, or one not on offer, changes nothing.
        assert!(pick("count", "120"));
        assert!(!pick("count", "120"));
        assert!(!pick("count", "7"));
        assert_eq!((re.sound_types, re.years, re.count), (vec!["Snare".to_owned(), "Kick".to_owned()], None, 120));
    }

    #[test]
    fn the_type_lists_show_as_many_types_as_the_column_has_room_for() {
        // The page's count, never under the usual nine; 0 (or less) while the view is away keeps the count.
        let mut kept = list::TYPE_CHIPS;
        assert!(room(&mut kept, 14));
        assert!(!room(&mut kept, 14));
        assert!(!room(&mut kept, 0) && !room(&mut kept, -3));
        assert_eq!(kept, 14);
        assert!(room(&mut kept, 2));
        assert_eq!(kept, list::TYPE_CHIPS);
        assert!(!room(&mut kept, 5));
        // Rediscover's More types menu's Any keeps the types on tick lines, as many as there are.
        let types: Vec<(String, usize)> = (0..12).map(|i| (format!("Type {i:02}"), 100 - i)).collect();
        let mut re =
            rediscover::Options { sound_types: vec!["Type 10".into(), "Type 11".into()], ..Default::default() };
        assert!(re_pick(&mut re, "more-types", "", &types, 11));
        assert_eq!(re.sound_types, ["Type 10"]);
        assert!(re_pick(&mut re, "more-types", "", &types, list::TYPE_CHIPS));
        assert!(re.sound_types.is_empty());
    }

    /// A small song library like the C# app's: two songs, a kick in both, a pad with no notes in one, and settings.
    pub(super) fn song_library(name: &str, settings: &[(&str, &str)]) -> PathBuf {
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
        one_start_shuffled(sounds_db, library_db, cards, None)
    }

    /// The same for a page that opens shuffled by `seed` (the first sounds come first).
    fn one_start_shuffled(sounds_db: &Path, library_db: &Path, cards: &Feed, seed: Option<u64>) -> Vec<String> {
        let got = std::sync::Mutex::new(Vec::new());
        let opening = seed.map(|seed| Opening { seed, view: TILES });
        read_all(sounds_db, library_db, Some(cards.clone()), opening, None, |step| {
            if let Ok(mut got) = got.lock() {
                got.push(said(step));
            }
        });
        got.into_inner().unwrap_or_default()
    }

    /// What a step the page is sent says, in words.
    fn said(step: Step) -> String {
        match step {
            Step::Read(Ok(read)) => {
                let plays: i64 = read.library.list.sounds().iter().map(|s| i64::from(s.plays)).sum();
                let from = read.source.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let of = read.partial.map(|all| format!(" (the first of {all})")).unwrap_or_default();
                format!("{} sounds{of} from {from}, {plays} plays", read.library.list.len())
            }
            Step::Rest(r) => {
                let kept = r.moved.as_ref().map_or(0, |m| m.iter().flatten().count());
                format!("the rest: {} sounds, the first {kept} kept", r.library.list.len())
            }
            Step::Read(Err(why)) => format!("couldn't read: {why}"),
            Step::Index(index) => format!("{} fingerprints", index.features.len()),
            Step::NoIndex(why) => format!("no fingerprints: {why}"),
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
        }
    }

    /// The cards' titles and lines, as the feed shows them now.
    fn cards_said(cards: &Feed) -> Vec<String> {
        cards_said_at(cards, Instant::now())
    }

    /// The cards that showed at `at` or after: a done card goes by itself after a few seconds
    /// ([`mo_app::activity::DONE_STAYS`]).
    fn cards_said_at(cards: &Feed, at: Instant) -> Vec<String> {
        cards.visible(at).iter().map(|c| format!("{}: {}", c.title, c.detail)).collect()
    }

    #[test]
    fn the_page_scans_the_sample_folders_and_takes_the_new_sounds() {
        let sounds_db = sound_library("scan");
        // The samples in the build's own fixtures folder, under a fixed name: a scan reads types from folder names.
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/mo-ui-sound-scan/Samples");
        let _ = std::fs::remove_dir_all(&samples);
        let folders = format!("[\"{}\"]", samples.to_string_lossy().replace('\\', "\\\\"));
        let library_db = song_library("scan-songs.db", &[("sample_folders", &folders)]);
        // Two new samples (one a hit, one a longer pad), and the five sounds the C# app found elsewhere.
        for (name, seconds, hz) in [("Kick 02.wav", 0.3, 60.0), ("Pad Long.wav", 1.5, 220.0)] {
            let wave: Vec<f32> = (0..(44_100.0 * seconds) as usize)
                .map(|i| (i as f32 * hz * std::f32::consts::TAU / 44_100.0).sin())
                .collect();
            let path = samples.join("Pack").join(name);
            assert!(std::fs::create_dir_all(samples.join("Pack")).is_ok());
            assert!(mo_audio::write_wav(&path, &[wave], 44_100, 16).is_ok());
        }
        // The first start makes the app's own library from sounds.db.
        one_start(&sounds_db, &library_db, &Feed::default());
        assert!(compact::own_since(&compact::copy_of(&sounds_db)).is_some());
        // The next: the new samples are listened to and kept, the page takes them with their fingerprints, a card
        // says what was found, and the C# app's sounds elsewhere are left as they are.
        let cards = Feed::default();
        let Some(mut on_show) = read_all(&sounds_db, &library_db, Some(cards.clone()), None, None, |_| {}) else {
            panic!("no sounds read")
        };
        assert_eq!(on_show.list.len(), 5);
        let steps = std::sync::Mutex::new(Vec::new());
        let send = |step: Step| {
            if let Ok(mut steps) = steps.lock() {
                steps.push(said(step));
            }
        };
        assert!(scan_for_sounds(&sounds_db, &library_db, &cards, &mut on_show, None, &send));
        let got = steps.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default();
        assert_eq!(got, ["again: other sounds, 7 sounds, 0 plays, used: true", "7 fingerprints"]);
        assert_eq!(on_show.list.len(), 7);
        // The two new sounds (analysed, unlike the five) are left out of CLAP's count from this start on.
        assert_eq!((on_show.new_since, on_show.analysed), (Some(6), 0));
        assert_eq!(cards_said(&cards), ["Found 2 new sounds: They're in your sound library now."]);
        // Nothing new: nothing kept, nothing sent.
        assert!(!scan_for_sounds(&sounds_db, &library_db, &cards, &mut on_show, None, &send));
        assert!(steps.lock().map(|s| s.is_empty()).unwrap_or_default());
    }

    /// A fresh install, with no sound library at all: the page makes the app's own, empty, and the scan fills it (the
    /// fresh-install gap: before, none was made, so nothing was ever scanned). Its one backup copy of the start comes
    /// after the scan (review finding S-5), and only the new sound's fingerprint is read.
    #[test]
    fn a_fresh_install_makes_its_own_sound_library_for_the_scan_to_fill() {
        let dir = std::env::temp_dir().join("mo-ui-sounds-test").join("fresh");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(std::fs::create_dir_all(&dir).is_ok());
        let sounds_db = dir.join("sounds.db");
        let copy = compact::copy_of(&sounds_db);
        // The build's own fixtures folder, under a fixed name, as above.
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/mo-ui-sound-scan-fresh/Samples");
        let _ = std::fs::remove_dir_all(&samples);
        let folders = format!("[\"{}\"]", samples.to_string_lossy().replace('\\', "\\\\"));
        let library_db = song_library("fresh-songs.db", &[("sample_folders", &folders)]);
        let wave: Vec<f32> = (0..13_230).map(|i| (i as f32 * 60.0 * std::f32::consts::TAU / 44_100.0).sin()).collect();
        assert!(std::fs::create_dir_all(samples.join("Pack")).is_ok());
        assert!(mo_audio::write_wav(&samples.join("Pack").join("Kick 02.wav"), &[wave], 44_100, 16).is_ok());
        // The screenshots read what's there and write nothing: no library.
        assert!(read_all(&sounds_db, &library_db, None, None, None, |_| {}).is_none());
        assert!(!copy.exists());
        let cards = Feed::default();
        let steps = std::sync::Mutex::new(Vec::new());
        let send = |step: Step| {
            if let Ok(mut steps) = steps.lock() {
                steps.push(said(step));
            }
        };
        let Some(mut on_show) = read_all(&sounds_db, &library_db, Some(cards.clone()), None, None, send) else {
            panic!("no sounds read")
        };
        let got = steps.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default();
        assert_eq!(got.first().map(String::as_str), Some("0 sounds from sounds-rust.db, 0 plays"), "{got:?}");
        assert!(compact::own_since(&copy).is_some());
        assert!(!sounds_db.exists());
        // The page's thread at start: the backup copy (of the library as it is: empty), then the scan; then it waits
        // for Settings (the app closes).
        let (asks, asked) = mpsc::channel();
        drop(asks);
        scans(&sounds_db, &library_db, &cards, &mut on_show, None, &send, &asked);
        let got = steps.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default();
        assert_eq!(got.get(1).map(String::as_str), Some("1 fingerprints"), "{got:?}");
        assert_eq!(on_show.list.len(), 1);
        let backup = compact::backup::backup_of(&copy);
        let sounds_in = |db: &Path| {
            let c = rusqlite::Connection::open(db).ok()?;
            c.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get::<_, i64>(0)).ok()
        };
        // A start makes one at most (review finding S-5): what the scan kept goes in the next, as the hourly look
        // finds the library changed by more than plays and your say (S-6).
        assert_eq!(sounds_in(&backup), Some(0));
        let looked = back_up(&sounds_db, compact::backup::When::Daily, true, &cards, Some(&mut on_show), None, &send);
        assert_eq!(looked, Backup { made: true, mend: true });
        assert_eq!(sounds_in(&backup), Some(1));
        let looked = back_up(&sounds_db, compact::backup::When::Daily, true, &cards, Some(&mut on_show), None, &send);
        assert_eq!(looked, Backup { made: false, mend: true });
    }

    #[test]
    fn a_sample_folder_added_while_the_app_is_open_is_scanned_and_its_sounds_taken() {
        let sounds_db = sound_library("scan-again");
        // The build's own fixtures folder, under a fixed name, as above.
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/mo-ui-sound-scan-again/Samples");
        let _ = std::fs::remove_dir_all(&samples);
        let folders = format!("[\"{}\"]", samples.to_string_lossy().replace('\\', "\\\\"));
        let library_db = song_library("scan-again-songs.db", &[("sample_folders", &folders)]);
        let write = |name: &str, seconds: f32, hz: f32| {
            let wave: Vec<f32> = (0..(44_100.0 * seconds) as usize)
                .map(|i| (i as f32 * hz * std::f32::consts::TAU / 44_100.0).sin())
                .collect();
            assert!(std::fs::create_dir_all(samples.join("Pack")).is_ok());
            assert!(mo_audio::write_wav(&samples.join("Pack").join(name), &[wave], 44_100, 16).is_ok());
        };
        write("Kick 02.wav", 0.3, 60.0);
        one_start(&sounds_db, &library_db, &Feed::default());
        let cards = Feed::default();
        let Some(mut on_show) = read_all(&sounds_db, &library_db, Some(cards.clone()), None, None, |_| {}) else {
            panic!("no sounds read")
        };
        // The page's thread: its scan at start, then one for each ask from Settings, until the app closes. Its cards
        // are read as they stood from now: two scans and two backups can take longer than a done card stays.
        let began = Instant::now();
        let (asks, asked) = mpsc::channel();
        let (steps, got) = mpsc::channel();
        let scanning = std::thread::spawn({
            let (sounds_db, library_db, cards) = (sounds_db.clone(), library_db.clone(), cards.clone());
            move || {
                let send = |step: Step| {
                    let _ = steps.send(said(step));
                };
                scans(&sounds_db, &library_db, &cards, &mut on_show, None, &send, &asked);
                on_show
            }
        });
        let next = || got.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|e| format!("nothing: {e}"));
        assert_eq!([next(), next()], ["again: other sounds, 6 sounds, 0 plays, used: true", "6 fingerprints"]);
        // A pad in the folder (as if it were a new folder), then Settings asks: it's scanned, and the page takes it,
        // with how your songs use samples counted again (it isn't kept between scans).
        write("Pad Long.wav", 1.5, 220.0);
        assert!(asks.send(()).is_ok());
        assert_eq!([next(), next()], ["again: other sounds, 7 sounds, 0 plays, used: true", "7 fingerprints"]);
        // The app closes: the thread ends.
        drop(asks);
        let Ok(on_show) = scanning.join() else { panic!("the scans stopped part way") };
        assert_eq!(on_show.list.len(), 7);
        assert!(on_show.uses.is_none() && on_show.recount);
        assert_eq!(
            cards_said_at(&cards, began),
            [
                "Found a new sound: They're in your sound library now.",
                "Found a new sound: They're in your sound library now."
            ]
        );
    }

    /// A scan's few sounds go in the index on show, made ready as the ones there were, rather than every fingerprint
    /// being read again (review finding S-5): a new one put in, one listened to again taking its own place, one gone
    /// taken out.
    #[test]
    fn a_scans_few_sounds_go_in_the_index_on_show() {
        let sounds_db = sound_library("few");
        for id in 10..40 {
            add_sound(&sounds_db, id, &format!("hit {id}"));
        }
        // The build's own fixtures folder, under a fixed name, as above.
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/mo-ui-sound-scan-few/Samples");
        let _ = std::fs::remove_dir_all(&samples);
        let folders = format!("[\"{}\"]", samples.to_string_lossy().replace('\\', "\\\\"));
        let library_db = song_library("few-songs.db", &[("sample_folders", &folders)]);
        let kick = samples.join("Pack").join("Kick 02.wav");
        let write = |seconds: f32| {
            let wave: Vec<f32> = (0..(44_100.0 * seconds) as usize)
                .map(|i| (i as f32 * 60.0 * std::f32::consts::TAU / 44_100.0).sin() * (-(i as f32) / 9_000.0).exp())
                .collect();
            assert!(std::fs::create_dir_all(samples.join("Pack")).is_ok());
            assert!(mo_audio::write_wav(&kick, &[wave], 44_100, 16).is_ok());
        };
        one_start(&sounds_db, &library_db, &Feed::default());
        let cards = Feed::default();
        let Some(mut on_show) = read_all(&sounds_db, &library_db, Some(cards.clone()), None, None, |_| {}) else {
            panic!("no sounds read")
        };
        let index = |on_show: &OnShow| on_show.index.clone().unwrap_or_default();
        let first = index(&on_show);
        assert_eq!(first.features.len(), 35);
        let steps = std::sync::Mutex::new(Vec::new());
        let send = |step: Step| {
            if let (Step::Index(_), Ok(mut steps)) = (&step, steps.lock()) {
                steps.push(said(step));
            }
        };
        let scan = |on_show: &mut OnShow| {
            let (asks, asked) = mpsc::channel();
            drop(asks);
            scans(&sounds_db, &library_db, &cards, on_show, None, &send, &asked);
            steps.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default()
        };
        write(0.3);
        assert_eq!(scan(&mut on_show), ["36 fingerprints"]);
        let Some(id) = on_show.list.sounds().iter().find(|s| s.path.ends_with("Kick 02.wav")).map(|s| s.id) else {
            panic!("no kick")
        };
        let before = index(&on_show);
        assert_eq!((on_show.added, before.features.get(id).is_some()), (1, true));
        // The others as they were, to the last bit: put in, not built afresh.
        let bits =
            |i: &SoundIndex, id: i64| i.features.get(id).map(|v| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
        assert!(first.features.ids().iter().all(|i| bits(&before, *i) == bits(&first, *i)));
        // Listened to again: its own fingerprint takes its place.
        write(0.6);
        assert_eq!(scan(&mut on_show), ["36 fingerprints"]);
        let again = index(&on_show);
        assert_eq!(on_show.added, 2);
        assert!(bits(&again, id).is_some() && bits(&again, id) != bits(&before, id));
        assert!(before.features.ids().iter().filter(|i| **i != id).all(|i| bits(&again, *i) == bits(&before, *i)));
        // Gone: taken out, and nothing else read again.
        assert!(std::fs::remove_file(&kick).is_ok());
        assert_eq!(scan(&mut on_show), ["35 fingerprints"]);
        let gone = index(&on_show);
        assert!(!gone.features.contains(id) && on_show.added == 2);
        assert!(gone.features.ids().iter().all(|i| bits(&gone, *i) == bits(&before, *i)));
    }

    #[test]
    fn a_damaged_library_is_mended_from_its_backup_copy_and_named_when_it_cant_be() {
        let sounds_db = sound_library("mend");
        for id in 10..40 {
            add_sound(&sounds_db, id, &format!("hit {id}"));
        }
        let library_db = song_library("mend-songs.db", &[]);
        // The first start makes the app's own library from sounds.db; then the page makes its backup copy.
        one_start(&sounds_db, &library_db, &Feed::default());
        let copy = compact::copy_of(&sounds_db);
        assert!(compact::own_since(&copy).is_some());
        assert!(matches!(compact::backup::back_up(&sounds_db), Some(compact::backup::BackedUp::Made { .. })));
        // sounds.db gone, and the library's list damaged: put back from its backup copy, and a card says so.
        for end in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{end}", sounds_db.display()));
        }
        damage_first_page(&copy, "sounds");
        let cards = Feed::default();
        let said = one_start(&sounds_db, &library_db, &cards);
        assert_eq!(said.first().map(String::as_str), Some("35 sounds from sounds-rust.db, 0 plays"), "{said:?}");
        let shown = cards_said(&cards);
        assert!(
            shown.iter().any(|c| c.starts_with("Mended your sound library from its backup copy: It was damaged")),
            "{shown:?}"
        );
        // No backup copy any more, and the fingerprints damaged: what waits for them is told why (review finding B-1).
        let _ = std::fs::remove_file(compact::backup::backup_of(&copy));
        damage_first_page(&copy, "sound_features");
        let said = one_start(&sounds_db, &library_db, &Feed::default());
        assert!(said.iter().any(|s| s.starts_with("no fingerprints: The sound library")), "{said:?}");
        // Its list damaged too: the page says the library itself couldn't be read.
        damage_first_page(&copy, "sounds");
        let said = one_start(&sounds_db, &library_db, &Feed::default());
        let first = said.first().cloned().unwrap_or_default();
        assert!(first.starts_with("couldn't read: The sound library") && first.contains("is damaged"), "{said:?}");
    }

    #[test]
    fn the_sounds_the_shuffle_puts_first_show_before_the_rest_and_keep_their_places() {
        let sounds_db = sound_library("first");
        for id in 10..260 {
            add_sound(&sounds_db, id, &format!("hit {id}"));
        }
        // One in ten analysed, each with a waveform of its own.
        for id in (10..260).step_by(10) {
            let analysis = format!(r#"{{"Seconds": 0.5, "Waveform": [0.25, {}, 1.0]}}"#, f64::from(id) / 1000.0);
            let set = c_sharp(&sounds_db).execute("UPDATE sounds SET analysis = ?1 WHERE id = ?2", (analysis, id));
            assert!(set.is_ok(), "{set:?}");
        }
        let library_db = song_library("first-songs.db", &[]);
        // The first start reads sounds.db, making the app's own library on the same pass: all at once.
        let made = one_start_shuffled(&sounds_db, &library_db, &Feed::default(), Some(7));
        assert_eq!(made[0], "255 sounds from sounds-rust.db, 0 plays");
        // From then on the 200 the page's shuffle puts on top come first, then all of them, those 200 among them.
        let copy = sounds_db.with_file_name("sounds-rust.db");
        let said = one_start_shuffled(&sounds_db, &library_db, &Feed::default(), Some(7));
        assert_eq!(
            said,
            [
                "200 sounds (the first of 255) from sounds-rust.db, 0 plays".to_owned(),
                format!("the rest from {}", copy.display()),
                "the rest: 255 sounds, the first 200 kept".to_owned(),
                "255 fingerprints".to_owned(),
                "used: true".to_owned(),
            ]
        );
        // They're the start of the whole shuffled list, so nothing on show moves when the rest arrive.
        let (first, rest) = (std::sync::Mutex::new(None), std::sync::Mutex::new(None));
        read_all(
            &sounds_db,
            &library_db,
            Some(Feed::default()),
            Some(Opening { seed: 7, view: TILES }),
            None,
            |step| match step {
                Step::Read(Ok(read)) => {
                    *first.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(read.library.list)
                }
                Step::Rest(r) => *rest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(r.library.list),
                _ => {}
            },
        );
        let (Some(first), Some(rest)) = (first.into_inner().ok().flatten(), rest.into_inner().ok().flatten()) else {
            panic!("the first sounds or the rest didn't come")
        };
        let ids = |list: &SoundList| -> Vec<i64> {
            list.view(&Filters::default(), Order::Shuffled, 7)
                .iter()
                .filter_map(|&i| list.sound(i))
                .map(|s| s.id)
                .collect()
        };
        assert_eq!(ids(&first), ids(&rest)[..200]);
        // The first come with their waveforms, the rest without: each is read as its sound shows, as sounds.db has it.
        let waves = |list: &SoundList| -> Vec<(i64, mo_sounds::Peaks)> {
            let mut waves: Vec<_> = list
                .sounds()
                .iter()
                .filter_map(|s| Some((s.id, s.analysis.as_ref()?.waveform.clone())))
                .filter(|(_, w)| !w.is_empty())
                .collect();
            waves.sort_unstable_by_key(|w| w.0);
            waves
        };
        let wanted = waves(&SoundList::new(
            SoundsDb::open_read_only(&sounds_db).unwrap().all(Local::now().naive_local()).unwrap(),
        ));
        assert_eq!(wanted.len(), 25);
        let first_wanted: Vec<_> =
            wanted.iter().filter(|w| first.sounds().iter().any(|s| s.id == w.0)).cloned().collect();
        assert!(!first_wanted.is_empty());
        assert_eq!(waves(&first), first_wanted);
        assert!(waves(&rest).is_empty());
        let mut read = peaks::read(&copy, &ids(&rest)).unwrap();
        read.sort_unstable_by_key(|w| w.0);
        assert_eq!(read, wanted);
        // Opening on the galaxy (it needs every star), or with no shuffle to go by (the screenshots): all at once.
        let first_of = |view: i32| -> Vec<String> {
            let said = std::sync::Mutex::new(Vec::new());
            let opening = Some(Opening { seed: 7, view });
            read_all(&sounds_db, &library_db, Some(Feed::default()), opening, None, |step| {
                let line = match step {
                    Step::Read(Ok(read)) => format!("{} sounds", read.library.list.len()),
                    Step::Rest(_) => "the rest".to_owned(),
                    _ => return,
                };
                said.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(line);
            });
            said.into_inner().unwrap_or_default()
        };
        assert_eq!(first_of(GALAXY), ["255 sounds"]);
        assert_eq!(one_start(&sounds_db, &library_db, &Feed::default())[0], "255 sounds from sounds-rust.db, 0 plays");
        // List shows the shuffle from its start too.
        assert_eq!(first_of(LIST), ["200 sounds", "the rest"]);
    }

    #[test]
    fn the_page_makes_the_apps_own_sound_library_and_reads_only_that_after() {
        let sounds_db = sound_library("copy");
        let library_db = song_library("copy-songs.db", &[]);
        let copy = sounds_db.with_file_name("sounds-rust.db");
        // The first start: sounds.db is read once, and the copy made on the same pass: the app's own library now.
        let cards = Feed::default();
        let first = one_start(&sounds_db, &library_db, &cards);
        assert_eq!(first, ["5 sounds from sounds-rust.db, 0 plays", "5 fingerprints", "used: true"]);
        assert!(copy.exists());
        assert!(compact::own_since(&copy).is_some());
        let said = cards_said(&cards);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("Made the app's own copy of your sound library: "), "{said:?}");
        // The next start reads the copy, and nothing needs bringing up to date.
        let cards = Feed::default();
        assert_eq!(one_start(&sounds_db, &library_db, &cards), first);
        assert_eq!(cards_said(&cards), Vec::<String>::new());
        // The retired C# app plays a sound and finds a new one: never read into the app's own library.
        let played = c_sharp(&sounds_db)
            .execute_batch("UPDATE sounds SET plays = 2, last_played = 1780000000 WHERE id = 3; INSERT INTO sound_events VALUES (3, 'play', 1780000000);");
        assert!(played.is_ok(), "{played:?}");
        add_sound(&sounds_db, 9, "rim");
        let cards = Feed::default();
        assert_eq!(one_start(&sounds_db, &library_db, &cards), first);
        assert_eq!(cards_said(&cards), Vec::<String>::new());
        // Its fingerprints damaged (a bad sector): it's made again from sounds.db while that's still there (all it can
        // be made from), with all of it that can still be read (review finding S-1), and stays the app's own. So the
        // retired app's play and sound stay out of it, as before. The page takes the sounds it was made with.
        damage_first_page(&copy, "sound_features");
        let cards = Feed::default();
        assert_eq!(
            one_start(&sounds_db, &library_db, &cards),
            [
                "5 sounds from sounds-rust.db, 0 plays",
                "5 fingerprints",
                "used: true",
                "again: the same sounds, 5 sounds, 0 plays, used: true",
            ]
        );
        let said = cards_said(&cards);
        assert!(said.iter().any(|c| c.starts_with("Mended your sound library: It was damaged")), "{said:?}");
        assert!(compact::own_since(&copy).is_some());
        assert!(compact::backup::damaged_of(&copy).exists(), "the damaged one is kept beside it");
        assert_eq!(one_start(&sounds_db, &library_db, &Feed::default()), first);
        // Damaged, and it can't be made again (a folder in the way of the new copy): the rest comes from sounds.db,
        // as it does without a copy, and the sounds as sounds.db has them, for this start. The damaged one is put back
        // as it was, so nothing of it is lost: the next start mends it, still keeping its own word.
        let part = PathBuf::from(format!("{}.part-{}", copy.display(), std::process::id()));
        assert!(std::fs::create_dir_all(&part).is_ok());
        damage_first_page(&copy, "sound_features");
        let cards = Feed::default();
        assert_eq!(
            one_start(&sounds_db, &library_db, &cards),
            [
                "5 sounds from sounds-rust.db, 0 plays".to_owned(),
                format!("the rest from {}", sounds_db.display()),
                "6 fingerprints".to_owned(),
                "used: true".to_owned(),
                "again: other sounds, 6 sounds, 2 plays, used: true".to_owned(),
            ]
        );
        let said = cards_said(&cards);
        assert!(
            said.iter().any(|c| c.starts_with("Couldn't make the app's own copy of your sound library")),
            "{said:?}"
        );
        assert!(std::fs::remove_dir(&part).is_ok());
        let cards = Feed::default();
        assert_eq!(one_start(&sounds_db, &library_db, &cards)[1], "5 fingerprints", "made whole again");
        let said = cards_said(&cards);
        assert!(said.iter().any(|c| c.starts_with("Mended your sound library: It was damaged")), "{said:?}");
        // sounds.db archived: the app's own library is all the page needs.
        for end in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{end}", sounds_db.display()));
        }
        assert_eq!(one_start(&sounds_db, &library_db, &Feed::default()), first);
    }

    /// Makes the app's own sound library at `copy` one of the format before, as the version before this one kept it:
    /// the waveform in each sound's row, CLAP's fingerprints as `sounds_db` holds them, and a hash of each row.
    fn of_the_format_before(copy: &Path, sounds_db: &Path) {
        let c = c_sharp(copy);
        let Some(lib) = sounds_db.to_str() else { panic!("{}", sounds_db.display()) };
        let made = c.execute("ATTACH DATABASE ?1 AS lib", [lib]).and_then(|_| {
            c.execute_batch(
                "PRAGMA foreign_keys = OFF;
                 ALTER TABLE sounds ADD COLUMN waveform;
                 UPDATE sounds SET waveform = (SELECT v FROM sound_waves WHERE sound_id = sounds.id);
                 DROP TABLE sound_waves;
                 DROP INDEX sounds_clap;
                 ALTER TABLE sounds DROP COLUMN clap_numbers;
                 DELETE FROM sound_clap;
                 INSERT INTO sound_clap SELECT id, clap FROM lib.sounds WHERE clap IS NOT NULL;
                 CREATE TABLE sound_hashes (sound_id INTEGER PRIMARY KEY, row INTEGER NOT NULL,
                     features INTEGER NOT NULL, clap INTEGER NOT NULL, clap_len INTEGER);
                 INSERT INTO sound_hashes SELECT id, id, id, id, length(clap) FROM lib.sounds;
                 UPDATE copy_info SET value = 1 WHERE key = 'format';
                 DETACH DATABASE lib;",
            )
        });
        assert!(made.is_ok(), "{made:?}");
    }

    /// The format the copy at `copy` notes it's laid out in.
    fn format_of(copy: &Path) -> Option<i64> {
        c_sharp(copy).query_row("SELECT value FROM copy_info WHERE key = 'format'", [], |r| r.get(0)).ok()
    }

    #[test]
    fn the_apps_own_sound_library_of_the_format_before_is_laid_out_anew_after_the_page_has_its_sounds() {
        let sounds_db = sound_library("lay-out");
        let library_db = song_library("lay-out-songs.db", &[]);
        let copy = sounds_db.with_file_name("sounds-rust.db");
        let first = one_start(&sounds_db, &library_db, &Feed::default());
        of_the_format_before(&copy, &sounds_db);
        assert_eq!(format_of(&copy), Some(1));
        // Something else has it open: the page reads it as it is, and says why it isn't laid out anew yet.
        let other = c_sharp(&copy);
        assert!(other.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get::<_, i64>(0)).is_ok());
        let cards = Feed::default();
        assert_eq!(one_start(&sounds_db, &library_db, &cards), first);
        let said = cards_said(&cards);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].starts_with("Couldn't make your sound library smaller just now: Something else has it open"),
            "{said:?}"
        );
        assert_eq!(format_of(&copy), Some(1));
        drop(other);
        // Then it is, once the page has its sounds and their fingerprints: nothing on the page is different.
        let cards = Feed::default();
        assert_eq!(one_start(&sounds_db, &library_db, &cards), first);
        let said = cards_said(&cards);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("Made your sound library smaller: "), "{said:?}");
        assert_eq!(format_of(&copy), Some(2));
        assert!(compact::own_since(&copy).is_some());
        // Once.
        let cards = Feed::default();
        assert_eq!(one_start(&sounds_db, &library_db, &cards), first);
        assert_eq!(cards_said(&cards), Vec::<String>::new());
    }

    #[test]
    fn a_copy_made_before_the_c_sharp_app_retired_is_brought_up_to_date_one_last_time() {
        let sounds_db = sound_library("last-sync");
        let library_db = song_library("last-sync-songs.db", &[]);
        let copy = sounds_db.with_file_name("sounds-rust.db");
        // Made by an earlier version, which kept it following sounds.db.
        assert!(compact::sync(&sounds_db, &copy).is_ok_and(|s| !s.adopted));
        // The C# app played a sound since, and sounds.db can't be read into the copy just now (it isn't a library
        // any more): the page keeps what it showed, and tries again at the next start.
        let played = c_sharp(&sounds_db)
            .execute_batch("UPDATE sounds SET plays = 2, last_played = 1780000000 WHERE id = 3; INSERT INTO sound_events VALUES (3, 'play', 1780000000);");
        assert!(played.is_ok(), "{played:?}");
        assert!(c_sharp(&sounds_db).execute_batch("ALTER TABLE sounds RENAME TO gone;").is_ok());
        let cards = Feed::default();
        let kept = one_start(&sounds_db, &library_db, &cards);
        assert_eq!(kept.first().map(String::as_str), Some("5 sounds from sounds-rust.db, 0 plays"));
        assert!(!kept.iter().any(|k| k.starts_with("again")), "{kept:?}");
        let said = cards_said(&cards);
        assert!(said.iter().any(|c| c.starts_with("Couldn't bring your sound library up to date: ")), "{said:?}");
        assert_eq!(compact::own_since(&copy), None);
        // Readable again: shown as it was at once, then brought up to date one last time, and the app's own after.
        assert!(c_sharp(&sounds_db).execute_batch("ALTER TABLE gone RENAME TO sounds;").is_ok());
        let cards = Feed::default();
        assert_eq!(
            one_start(&sounds_db, &library_db, &cards),
            [
                "5 sounds from sounds-rust.db, 0 plays",
                "5 fingerprints",
                "used: true",
                "again: the same sounds, 5 sounds, 2 plays, used: true",
                // Made afresh: its fingerprints are read again.
                "5 fingerprints",
            ]
        );
        assert_eq!(cards_said(&cards), ["Brought your sound library up to date: Made afresh from 5 sounds"]);
        assert!(compact::own_since(&copy).is_some());
        // A scan in the C# app after that: not read in.
        add_sound(&sounds_db, 9, "rim");
        assert_eq!(
            one_start(&sounds_db, &library_db, &Feed::default()),
            ["5 sounds from sounds-rust.db, 2 plays", "5 fingerprints", "used: true"]
        );
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
    fn an_older_play_of_the_same_sound_ending_leaves_the_new_one_on_show() {
        // The sound at 4 played (play 5), then played again near its end (play 6): play 5 ends while play 6 is read.
        PAGE.with(|p| *p.borrow_mut() = Page { playing: Some((4, 2.5)), play: 6, ..Page::default() });
        ended("5");
        assert_eq!(PAGE.with(|p| p.borrow().playing), Some((4, 2.5)));
        // A key that isn't a play's number is never the page's.
        ended(r"C:\s\kick.wav");
        assert_eq!(PAGE.with(|p| p.borrow().playing), Some((4, 2.5)));
        // Play 6 ending by itself does stop it on the page.
        ended("6");
        assert_eq!(PAGE.with(|p| p.borrow().playing), None);
    }

    #[test]
    fn a_click_on_the_waveform_moves_only_the_play_the_player_has_as_far_as_it_was_read() {
        // Play 6 of the sound at 4 is a sweep's: only its first 4 s were read.
        let p = Page { playing: Some((4, 30.0)), play: 6, play_reads: 4.0, ..Page::default() };
        assert!(seeks(&p, Some("6"), 2.0));
        // Past what was read, the sound is read again from there; so it is while play 6 is still being read (the
        // player has nothing, or an older play of the same sound).
        assert!(!seeks(&p, Some("6"), 15.0));
        assert!(!seeks(&p, None, 2.0));
        assert!(!seeks(&p, Some("5"), 2.0));
    }

    #[test]
    fn a_view_and_autoplay_chosen_while_the_sounds_are_read_stay_when_they_arrive() {
        // Nothing chosen: the page opens on the settings, and on Tiles with Autoplay off without them.
        let mut p = Page::default();
        p.open_on(Some("List"), Some("1"));
        assert_eq!((p.view, p.autoplay), (LIST, true));
        let mut p = Page::default();
        p.open_on(None, None);
        assert_eq!((p.view, p.autoplay), (TILES, false));
        // Galaxy picked and Autoplay turned on while the sounds were read: they stay, over the settings.
        let mut p = Page::default();
        p.pick_view(GALAXY);
        assert!(p.toggle_autoplay());
        p.open_on(Some("List"), Some("0"));
        assert_eq!((p.view, p.autoplay), (GALAXY, true));
        // Only what was chosen stays: a view picked leaves Autoplay to its setting.
        let mut p = Page::default();
        p.pick_view(KITS);
        p.open_on(Some("List"), Some("1"));
        assert_eq!((p.view, p.autoplay), (KITS, true));
    }

    #[test]
    fn a_sound_picked_elsewhere_is_off_the_kit_until_a_pad_holds_it() {
        let mut p = Page { places: Arc::new(HashMap::from([(10, 0), (11, 1)])), ..Page::default() };
        // Nothing picked: nothing to close.
        assert!(!p.picked_off_the_kit());
        p.picked = Some(1);
        assert!(p.picked_off_the_kit());
        p.kit.slots[0].sound = Some(10);
        assert!(p.picked_off_the_kit());
        p.kit.slots[1].sound = Some(11);
        assert!(!p.picked_off_the_kit());
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
        assert_eq!(s.weights, LikedWeights::default());
        // "Only where it has notes": the pad isn't played in Song. And the page's own settings come through.
        let (s, used) = read("notes.db", &[(USED_NEEDS_NOTES_KEY, "1"), (LIKED_WEIGHTS_KEY, r#"{"Project":7}"#)]);
        assert_eq!(used.get(&2).map(|u| u.projects), Some(1));
        assert_eq!(s.weights, LikedWeights { project: 7.0, ..LikedWeights::default() });
    }

    #[test]
    fn the_view_autoplay_and_count_you_left_are_there_before_settings_connect() {
        // The app opening on this page reads its sounds before the Settings page is connected (review of 7cc595f).
        // Here only the C# app's choices, from the song library: the folder for the app's own settings file isn't there.
        let kept = [(VIEW_KEY, "List"), (AUTOPLAY_KEY, "1"), (COUNT_KEY, "50")];
        let library = song_library("kept-choices.db", &kept);
        let (settings, writable) = crate::settings::load(&library.with_file_name("no-settings-folder"), &library);
        assert!(writable);
        for (key, value) in kept {
            assert_eq!(settings.get(key), Some(value), "{key}");
            assert_eq!(crate::settings::value(key).as_deref(), Some(value), "{key}");
        }
    }

    #[test]
    fn use_counts_change_most_liked_but_not_the_order() {
        let list = SoundList::new(vec![sound(1, "a.wav"), sound(2, "b.wav")]);
        let w = LikedWeights::default();
        let usage = HashMap::from([(2, Usage { projects: 3, recent: 3, years: vec![2026], ..Usage::default() })]);
        let used = with_usage(&list, usage, &Arc::default(), &w);
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
            HashMap::from([(2, Usage { projects: 3, recent: 1, years: vec![2026], ..Usage::default() })]),
            HashMap::new(),
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
        // The one a line says it sounds like is the sound your projects use.
        assert!(a.likes.values().all(|&like| like == 2), "{:?}", a.likes);
    }

    #[test]
    fn plays_show_at_once_in_history_and_drags_and_sends_wait_for_the_next_read() {
        use mo_sounds::event::{DRAG, PLAY, SEND};
        let sounds = vec![sound(1, "Pack/a.wav"), sound(2, "Pack/b.wav"), sound(3, "Pack/c.wav")];
        let day = |d: u32, h: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, d).and_then(|d| d.and_hms_opt(h, 0, 0)).unwrap_or_default()
        };
        let lib = Library::new(sounds.clone(), vec![(1, day(1, 9))], &LikedWeights::default());
        let mut p = Page {
            list: lib.list.clone(),
            places: lib.places.clone(),
            played: Arc::new(lib.played.clone()),
            ..Page::default()
        };
        // A play you meant: first in History.
        did::take(&mut p, &[2], PLAY, 1, day(6, 10));
        assert_eq!(p.history, [(1, day(6, 10)), (0, day(1, 9))]);
        // Played four times more: History lists it once, at its last play.
        for n in 2..6 {
            did::take(&mut p, &[2], PLAY, n, day(6, 11));
        }
        assert_eq!(p.history, [(1, day(6, 11)), (0, day(1, 9))]);
        assert_eq!(p.played.len(), 2);
        // With History on show, the sound played stays where it is until you come back to it, whatever the list
        // worked out meanwhile.
        p.view = HISTORY;
        did::take(&mut p, &[1], PLAY, 6, day(6, 12));
        assert_eq!(p.history.first(), Some(&(1, day(6, 11))));
        assert_eq!(p.history_behind.as_ref().and_then(|was| was.first()), Some(&(1, day(6, 11))));
        p.pick_view(TILES);
        p.pick_view(HISTORY);
        assert_eq!(p.history.first(), Some(&(0, day(6, 12))));
        // A drag and a send leave the page as it was: they count the next time the sounds are read.
        let (history, played) = (p.history.clone(), p.played.clone());
        did::take(&mut p, &[3, 3, 99], DRAG, 7, day(6, 13));
        did::take(&mut p, &[3], SEND, 8, day(6, 13));
        assert_eq!((&p.history, &p.played), (&history, &played));
        assert_eq!(p.noted.len(), 6);
        // Read again holding the first five plays: the sixth goes on showing first in History, the rest is in what
        // was read (here, as the sounds were).
        let again = Library::new(sounds, vec![(1, day(1, 9))], &LikedWeights::default())
            .read_holding(Some(Holds { upto: 5, ..Holds::default() }));
        take_refreshed(&mut p, again, None, false);
        assert_eq!(p.noted.len(), 1);
        assert_eq!(p.played.first(), Some(&(0, day(6, 12))));
        // One read without holding them off: the library's word is taken.
        let again = Library::new(vec![sound(1, "Pack/a.wav")], Vec::new(), &LikedWeights::default());
        take_refreshed(&mut p, again, None, false);
        assert!(p.noted.is_empty());
    }

    #[test]
    fn plays_not_yet_kept_as_the_sounds_were_read_again_stay_first_in_history() {
        use mo_sounds::event::PLAY;
        let day = |h: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, 6).and_then(|d| d.and_hms_opt(h, 0, 0)).unwrap_or_default()
        };
        let sounds = vec![sound(1, "Pack/a.wav"), sound(2, "Pack/b.wav"), sound(3, "Pack/c.wav")];
        let lib = Library::new(sounds.clone(), Vec::new(), &LikedWeights::default());
        let mut p = Page { list: lib.list.clone(), places: lib.places.clone(), ..Page::default() };
        did::take(&mut p, &[1], PLAY, 1, day(10));
        did::take(&mut p, &[2], PLAY, 2, day(11));
        did::take(&mut p, &[3], PLAY, 3, day(12));
        // Read again holding the first play only, in another order: the two after it are first in History, newest
        // first, at their new places; the first is as the library has it.
        let mut moved = sounds;
        moved.reverse();
        let again = Library::new(moved, vec![(1, day(10))], &LikedWeights::default())
            .read_holding(Some(Holds { upto: 1, ..Holds::default() }));
        take_refreshed(&mut p, again, None, false);
        let ids: Vec<i64> = p.played.iter().filter_map(|&(at, _)| p.list.sound(at).map(|s| s.id)).collect();
        assert_eq!(ids, [3, 2, 1]);
        assert_eq!(p.noted.len(), 2);
    }

    #[test]
    fn your_say_shows_at_once_and_stays_until_the_sounds_read_hold_it() {
        use mo_sounds::compact::yours::Change;
        let mut sounds = vec![sound(1, "Pack/a.wav"), sound(2, "Pack/b.wav"), sound(3, "Pack/c.wav")];
        sounds[2].name_type = Some("Snare".into());
        sounds[2].tags = vec!["dusty".into()];
        let lib = Library::new(sounds.clone(), Vec::new(), &LikedWeights::default());
        let mut p = Page {
            list: lib.list.clone(),
            places: lib.places.clone(),
            types: lib.types.clone(),
            tags: lib.tags.clone(),
            ..Page::default()
        };
        let card = |p: &Page, at: usize| picked_sound(p, at);
        // Made a favourite: its heart on its card, tile and line, and the Favourites filter.
        yours::take(&mut p, 2, Change::Favourite(true), 1);
        assert!(card(&p, 1).favourite && tile(&p, 0, 1).favourite && line(&p, 0, 1).favourite);
        let favourites = Filters { favourites: true, ..Filters::default() };
        assert_eq!(p.list.view(&favourites, Order::Name, 0), [1]);
        // Tags (a new one joins the Tag filter) and your own type (a new one joins the type chips).
        yours::take(&mut p, 2, Change::Tag("Bright".into()), 2);
        yours::take(&mut p, 2, Change::Type(Some("Perc".into())), 3);
        yours::take(&mut p, 3, Change::Tag("Warm".into()), 4);
        yours::take(&mut p, 3, Change::Untag("DUSTY".into()), 5);
        let b = card(&p, 1);
        assert_eq!(
            (b.own_type.as_str(), b.sound_type.as_str(), b.type_from.as_str()),
            ("Perc", "Perc", "· you set this")
        );
        assert!(p.types.iter().any(|(t, _)| t == "Perc"), "{:?}", p.types);
        assert_eq!(p.tags, ["Bright", "dusty", "Warm"]);
        assert_eq!(p.list.now(2).map(|n| n.tags.to_vec()), Some(vec!["Warm".to_owned()]));
        // Your type taken away: the name's type again.
        yours::take(&mut p, 3, Change::Type(Some("Kick".into())), 6);
        yours::take(&mut p, 3, Change::Type(None), 7);
        let c = card(&p, 2);
        assert_eq!(
            (c.own_type.as_str(), c.sound_type.as_str(), c.type_from.as_str()),
            ("", "Snare", "· from its name")
        );
        // A sound not in the list isn't taken.
        yours::take(&mut p, 99, Change::Favourite(true), 8);
        assert_eq!(p.said.len(), 7);
        // Read again holding everything up to the first tag, in another order: the rest goes on showing, at the
        // sounds' new places; what was read is taken as it is (here, as the sounds were: the tag is gone with it).
        // The favourite couldn't be kept yet, so it goes on showing too.
        let mut moved = sounds.clone();
        moved.reverse();
        let holds = Holds { upto: 2, waiting: [1].into() };
        let again = Library::new(moved, Vec::new(), &LikedWeights::default()).read_holding(Some(holds));
        take_refreshed(&mut p, again, None, false);
        assert_eq!(p.said.len(), 6);
        let at = |p: &Page, id: i64| p.places[&id];
        assert!(card(&p, at(&p, 2)).favourite);
        assert_eq!(p.list.now(at(&p, 2)).map(|n| n.tags.to_vec()), Some(Vec::new()));
        assert_eq!(card(&p, at(&p, 2)).sound_type, "Perc");
        assert_eq!(p.list.now(at(&p, 3)).map(|n| n.tags.to_vec()), Some(vec!["Warm".to_owned()]));
        assert!(p.tags.iter().any(|t| t == "Warm") && p.types.iter().any(|(t, _)| t == "Perc"));
        // The use counts arriving keep it over the sounds.
        let used = with_usage(&lib.list, HashMap::new(), &Arc::default(), &LikedWeights::default());
        p.list = Arc::new(used);
        yours::keep_showing(&mut p);
        assert_eq!(card(&p, at(&p, 2)).sound_type, "Perc");
        // One read without holding them off: the library's word is taken.
        let again = Library::new(sounds, Vec::new(), &LikedWeights::default());
        take_refreshed(&mut p, again, None, false);
        assert!(p.said.is_empty() && !p.list.has_say());
    }

    #[test]
    fn a_card_lists_its_projects_or_those_of_the_sound_rediscover_says_it_sounds_like() {
        let sounds = vec![sound(1, "Pack/Boom kick.wav"), sound(2, "Pack/New kick.wav")];
        let file = ProjectFile { id: 5, path: r"K:\Projects\Night drive.flp".into(), mtime: 1_700_000_000 };
        let lib = Library::new(sounds, Vec::new(), &LikedWeights::default()).with_usage(
            HashMap::from([(1, Usage { projects: 1, project_ids: vec![5], ..Usage::default() })]),
            HashMap::from([(5, file)]),
            &LikedWeights::default(),
        );
        let mut p = Page { list: lib.list.clone(), places: lib.places.clone(), ..Page::default() };
        // The first sound's own project, which opens on the Songs page by its save's id.
        let card = picked_sound(&p, 0);
        assert_eq!(card.projects_title, "Used in 1");
        let line = card.projects.row_data(0).unwrap_or_default();
        assert_eq!((line.id, line.name.as_str(), line.daw.as_str()), (5, "Night drive", "FL Studio"));
        // The second, which no project uses: none, until Rediscover says it sounds like the first.
        let card = picked_sound(&p, 1);
        assert_eq!((card.projects_title.as_str(), card.projects.row_count()), ("Used in 0", 0));
        p.re_likes = HashMap::from([(2, 1)]);
        let card = picked_sound(&p, 1);
        assert_eq!(card.projects_title, "Used in 0");
        assert!(
            card.projects_note.contains("Boom kick, which it sounds like, is in 1 project"),
            "{}",
            card.projects_note
        );
        assert_eq!(card.projects.row_count(), 1);
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
