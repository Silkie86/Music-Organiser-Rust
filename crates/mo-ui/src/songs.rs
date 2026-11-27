//! Fills the Songs page from mo-app's song list and answers its search box and chips. The library is read on a
//! background thread (rule 12); searching and filtering then walk the rows already made, and a line's text is only
//! made when the list draws that line. Picking a song opens its panel at once with what the list knows; the newest
//! save's plugins, samples and details follow from a background thread that only reads the last song picked.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::bitwig_convert::{self as bitwig, Converted};
use mo_app::daw_copy;
use mo_app::finder::{self, Kind};
use mo_app::live::{Live, LiveDetails, Updated};
use mo_app::log;
use mo_app::places::Place;
use mo_app::shell;
use mo_app::shortcuts::Shortcuts;
use mo_app::song_panel::{self as panel, Go, SaveLine as PanelSaveLine, Tab};
use mo_app::songs::{Entry, Filters, Listed, NextUp, Order, SongRow, Songs, View};
use mo_app::{paths, sound_library, year_strip};
use mo_library::scan::is_project_file;
use mo_library::song_fields::{self, Change};
use mo_library::songs::DEFAULT_RUN_GAP_MONTHS;
use mo_library::watch::{Changes, FolderWatch};
use mo_library::{Library, STATUSES, SaveDetail, Todo};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::filters::FilterModels;
use crate::finder::How;
use crate::{
    AppWindow, BoardCard, BoardColumn, BounceLine, CountLine, DetailPair, DustLine, HealthLine, MenuItem, NextLine,
    OlderLine, PanelSong, PluginLine, SampleLine, SaveLine, SongLine, SongTag, SongsData, TodoLine, YearBar,
};

/// The page's rows, filters and what they let through, kept on the window's thread.
struct Page {
    songs: Songs,
    filters: Filters,
    /// The column the list goes by: a click on its heading.
    listed: Listed,
    view: View,
    /// Every year a song was last saved in, newest first, with how many: the year strip.
    years: Vec<(i32, usize)>,
    /// When the view was made, for "This week" and the short dates.
    now: NaiveDateTime,
    /// The song whose panel is open: its place in `songs.rows`.
    picked: Option<usize>,
    /// Counts the songs picked, so a save's record that arrives after another song was picked is dropped.
    asked: u64,
    /// The picked song's newest save, as the library records it, once read.
    detail: Option<SaveDetail>,
    /// Where each line of the Health strip's buttons goes, as shown.
    health: Vec<Go>,
    /// The library, for Open in Bitwig's settings.
    db: PathBuf,
    /// The board shows the ideas saved more than a year ago too ("Show all").
    all_ideas: bool,
    /// How many cards each of the board's columns shows, by status ("Show more" adds [`BOARD_CARDS`]).
    board_cards: [usize; STATUSES.len()],
}

/// Whether the board is on show, for the page's thread: guesses arriving while it's away wait ([`fill_board`]).
static BOARD_ON: AtomicBool = AtomicBool::new(false);

/// The most cards a board column shows, however often "Show more" is pressed: past that, the search box and filters
/// find a song quicker than scrolling (rule 12).
const BOARD_MOST: usize = 600;

/// How many cards a board column shows at first, and how many more each "Show more" adds: a column is a plain
/// layout, not a list that makes only the lines in sight, so it's kept to a few screens (rule 12).
const BOARD_CARDS: usize = 60;

impl Default for Page {
    fn default() -> Page {
        Page {
            songs: Songs::default(),
            filters: Filters::default(),
            listed: Listed::default(),
            view: View::default(),
            years: Vec::new(),
            now: Local::now().naive_local(),
            picked: None,
            asked: 0,
            detail: None,
            health: Vec::new(),
            db: PathBuf::new(),
            all_ideas: false,
            board_cards: [BOARD_CARDS; STATUSES.len()],
        }
    }
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    /// Made-up songs on show (mo-screens): nothing is looked for in folders.
    static SAMPLE: Cell<bool> = const { Cell::new(false) };
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
    /// Reads a save's record for the panel, one at a time, newest ask first.
    static READER: RefCell<Option<Newest<(u64, i64)>>> = const { RefCell::new(None) };
    /// The activity cards, for Open, Show in folder and Copy path.
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
    /// The save last right-clicked on the Saves tab, by its path, for its menu's lines.
    static MENU_SAVE: RefCell<String> = const { RefCell::new(String::new()) };
    /// The save picked on the Saves tab, by its path ("" for none): the song's buttons and keys act on it rather than
    /// on the newest save. Only ever used when it's one of the picked song's saves ([`aimed`]).
    static PICKED_SAVE: RefCell<String> = const { RefCell::new(String::new()) };
    /// The DAW, status and tag filters' chips and menus, kept from one showing to the next.
    static FILTER_MODELS: [FilterModels; 3] = Default::default();
}

/// The list's lines, made from the page's view one at a time as the list draws them, so a new search doesn't build
/// 16,000 lines nobody sees.
struct Lines {
    page: Rc<RefCell<Page>>,
    notify: ModelNotify,
}

impl Model for Lines {
    type Data = SongLine;

    fn row_count(&self) -> usize {
        self.page.borrow().view.entries.len()
    }

    fn row_data(&self, row: usize) -> Option<SongLine> {
        let p = self.page.borrow();
        p.view.entries.get(row).map(|e| line(&p.songs, e, p.now))
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The library's read, started before the window is made ([`read_ahead`]) and handed to [`connect`].
#[must_use = "the read goes on the page only once `connect` has it"]
pub struct Ahead {
    path: PathBuf,
    details: LiveDetails,
    /// Whether the library is the app's own songs.db, as the app found at its start
    /// ([`mo_library::songs_db::AtStart::is_songs_db`]): only then is what's read, and given on cards, kept in it.
    own: bool,
    /// Tells the reading thread which window the list goes on. None when that thread couldn't be started:
    /// [`connect`] starts it then.
    window: Option<mpsc::Sender<slint::Weak<AppWindow>>>,
}

/// A library read and made into the page's list.
struct Loaded {
    live: Live,
    songs: Songs,
    /// What Ctrl+K finds.
    found: Arc<finder::Lines>,
    /// The year strip's counts ([`Songs::year_counts`]).
    years: Vec<(i32, usize)>,
}

/// Starts reading the library at `path` and making the page's list from it, in the background. Called as soon as the
/// app knows which library it reads and how many background threads it may use, before the graphics card is found
/// and the window made, so the list is ready when the window first draws (or soon after) rather than only begun then.
pub fn read_ahead(path: PathBuf, own: bool) -> Ahead {
    let details = LiveDetails::default();
    let window = match start(path.clone(), details.clone(), own) {
        Ok(window) => Some(window),
        Err(e) => {
            log::warning("Songs", &format!("Couldn't start reading the library ahead of the window: {e}"));
            None
        }
    };
    Ahead { path, details, own, window }
}

/// Starts the page's thread: it reads the library at `path` at once ([`load`]), waits to be told its window, puts
/// the list there and then keeps it up to date ([`watch`]). A read that fails is logged as it fails, and shown once
/// there's a window. If it's never told a window (the window couldn't be made), it ends.
fn start(path: PathBuf, details: LiveDetails, own: bool) -> std::io::Result<mpsc::Sender<slint::Weak<AppWindow>>> {
    let (window, told) = mpsc::channel::<slint::Weak<AppWindow>>();
    background::spawn("Songs", move || {
        let loaded = load(&path, details, own);
        if let Err(e) = &loaded {
            log::warning("Songs", e);
        }
        let Ok(weak) = told.recv() else { return };
        let Loaded { mut live, songs, found, years } = match loaded {
            Ok(loaded) => loaded,
            Err(e) => {
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let data = app.global::<SongsData>();
                    data.set_message(CANT_READ.into());
                    data.set_message_text(e.into());
                });
                return;
            }
        };
        let offered = fit_songs(&songs);
        let _ = weak.upgrade_in_event_loop(move |app| {
            show(&app, songs, years);
            crate::finder::add_source(&app, "songs", found);
            crate::midi::songs_offered(&app, offered);
        });
        watch(&mut live, &weak, &path);
    })?;
    Ok(window)
}

/// How long FL Studio has had each song open, from the details the library at `path` keeps of its newest FL save:
/// read once the list is on show (it unpacks each one's details). Whether any was found, so the list is shown again.
fn learn_minutes(live: &mut Live, path: &Path) -> bool {
    let found = Library::open_read_only(path).and_then(|library| live.learn_minutes(&library));
    found.unwrap_or_else(|e| {
        log::warning("Songs", &format!("Couldn't read how long songs were open in FL Studio: {e}"));
        false
    })
}

/// The shapes the library at `path` keeps of the songs' newest saves ([`Live::learn_shapes`]). Whether any was found.
fn learn_shapes(live: &mut Live, path: &Path) -> bool {
    let found = Library::open_read_only(path).and_then(|library| live.learn_shapes(&library));
    found.unwrap_or_else(|e| {
        log::warning("Songs", &format!("Couldn't read the songs' shapes: {e}"));
        false
    })
}

/// How often the list is shown again while shapes are coming in: often enough to see the board fill, not so often the
/// page's thread does little else.
const SHAPES_SHOWN_EVERY: Duration = Duration::from_secs(5);

/// Reads the shapes of the songs' newest saves that haven't been read ([`Live::shapes_to_read`]) on a thread of its
/// own, telling `tell` as each few are read and once it ends. Whether it started.
fn read_shapes(live: &mut Live, tell: &mpsc::Sender<Told>) -> bool {
    let list = live.shapes_to_read();
    if list.is_empty() {
        return false;
    }
    let stop = live.stopper();
    let tell = tell.clone();
    let count = list.len();
    let started = background::spawn("Songs: shapes", move || {
        let began = Instant::now();
        mo_app::board::read_shapes(list, &stop, |read| {
            let _ = tell.send(Told::Shapes(read));
        });
        if count >= 20 {
            log::info("Songs", &format!("Read the shapes of {count} saves in {:.1} s", began.elapsed().as_secs_f32()));
        }
        let _ = tell.send(Told::ShapesDone);
    });
    match started {
        Ok(_) => true,
        Err(e) => {
            log::warning("Songs", &format!("Couldn't read the songs' shapes: {e}"));
            false
        }
    }
}

/// The songs the MIDI page's "Fits my song" offers (the few saved last), from a list as it was just made: worked out
/// here, on the page's thread, and given to the MIDI page with each list, so it follows the songs you save without
/// reading the library itself.
fn fit_songs(songs: &Songs) -> Vec<mo_app::midi_fits::SongChoice> {
    mo_app::midi_fits::song_choices(songs.rows.iter().map(|r| &r.song), &songs.real)
}

/// Reads the library at `path` and makes the page's list from it, to be kept in the library if it's the app's own
/// songs.db (`own`, as the app found at its start). Why not, when it can't be read.
fn load(path: &Path, details: LiveDetails, own: bool) -> Result<Loaded, String> {
    let began = Instant::now();
    let read = Library::open_read_only(path)
        .and_then(|lib| Live::load(&lib, DEFAULT_RUN_GAP_MONTHS, details))
        // Missing samples are looked for in the Sounds library beside it last (rule 13).
        .map(|live| live.with_sounds(sound_library::database_beside(path)))
        // What's read is kept in the app's own songs.db, as the app found at its start: asked again here, a songs.db
        // held by another write just then left the whole run unable to keep anything (review SG-2). library.db is
        // never written.
        .map(|live| {
            if own && path.file_name().is_some_and(|n| n == mo_library::songs_db::FILE_NAME) {
                live.with_writes(path.to_owned())
            } else {
                live
            }
        });
    let live = read.map_err(|e| e.to_string())?;
    let read_in = began.elapsed();
    let songs = live.songs();
    let found = Arc::new(finder::songs(&songs, Local::now().naive_local()));
    log::info(
        "Songs",
        &format!(
            "{} songs from {} saves (library read in {:.1} s{}, grouped in {:.1} s)",
            songs.rows.len(),
            songs.saves,
            read_in.as_secs_f32(),
            live.source().map(|from| format!(", {from}")).unwrap_or_default(),
            (began.elapsed() - read_in).as_secs_f32()
        ),
    );
    // The year strip is counted here too, not on the window's thread (review A-4).
    let years = songs.year_counts();
    Ok(Loaded { live, songs, found, years })
}

/// Wires the page's search box and chips, and puts the library `ahead` has been reading on the page once it's read.
/// Open, Show in folder and Copy path report on `feed`'s cards.
pub fn connect(app: &AppWindow, ahead: Ahead, feed: Feed) {
    let Ahead { path, details, own, window } = ahead;
    let data = app.global::<SongsData>();
    FEED.with(|f| *f.borrow_mut() = Some(feed));
    PAGE.with(|p| p.borrow_mut().db = path.clone());
    data.on_open(|| with_picked(|p, r| open_main(p, aimed(r))));
    data.on_open_own(|| with_picked(|_, r| open_save(aimed(r))));
    data.on_open_save(|path| with_save(&path, |_, s| open_save(s)));
    let weak = app.as_weak();
    data.on_save_menu_for(move |path| {
        let Some(app) = weak.upgrade() else { return };
        let mut items = Vec::new();
        with_save(&path, |_, s| items = save_menu(&s.daw, &crate::shortcuts()));
        MENU_SAVE.with(|m| *m.borrow_mut() = path.to_string());
        app.global::<SongsData>().set_save_menu(ModelRc::new(VecModel::from(items)));
    });
    data.on_save_menu_chosen(|id| {
        let path = MENU_SAVE.with(|m| m.borrow().clone());
        do_to_save(&path, &id);
    });
    // The same, from the buttons on the save's line.
    data.on_save_chosen(|path, id| do_to_save(&path, &id));
    finder_opens();
    data.on_reveal(|| with_picked(|_, r| reveal(aimed(r))));
    data.on_copy_path(|| with_picked(|_, r| copy_path(&aimed(r).path)));
    let weak = app.as_weak();
    data.on_show_presets(move || {
        let Some(app) = weak.upgrade() else { return };
        let mut save = None;
        with_picked(|_, r| save = Some((r.song.latest.path.clone(), r.song.name.clone(), r.song.latest.mtime)));
        if let Some((save, name, modified)) = save {
            crate::go_to(&app, Place::Presets.index() as i32);
            crate::presets::show_song(&app, &save, &name, modified);
        }
    });
    wire_filters(app);
    wire_list(app);
    let weak = app.as_weak();
    data.on_select(move |row| {
        if let Some(app) = weak.upgrade() {
            pick(&app, usize::try_from(row).ok());
        }
    });
    let weak = app.as_weak();
    data.on_health_go(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let go = PAGE.with(|p| p.borrow().health.get(usize::try_from(i).ok()?).copied());
        match go {
            Some(Go::Tab(t)) => app.global::<SongsData>().set_tab(t.index() as i32),
            Some(Go::Song(row)) => show_song(&app, row),
            _ => {}
        }
    });
    data.set_statuses(strings(STATUSES.iter().copied()));
    let weak = app.as_weak();
    data.on_close_panel(move || {
        if let Some(app) = weak.upgrade() {
            pick(&app, None);
        }
    });

    let weak = app.as_weak();
    let db = path.clone();
    let mut library: Option<Library> = None;
    let live_details = details.clone();
    let reader = Newest::start("Song panel", move |(asked, id): (u64, i64)| {
        // A save read while the app was open has its details here, newer than the library's.
        if let Some(detail) = live_details.lock().ok().and_then(|d| d.get(&id).cloned()) {
            let _ = weak.upgrade_in_event_loop(move |app| arrived(&app, asked, Ok(Some(detail))));
            return;
        }
        // Opened at the first pick and kept; tried again at the next pick if it couldn't be.
        let opened = match library.take() {
            Some(lib) => Ok(lib),
            None => Library::open_read_only(&db),
        };
        let read = match opened {
            Ok(lib) => {
                // A row that trips the reader says so in the panel; the library is opened again at the next pick.
                match background::guarded(|| lib.detail(id).map_err(|e| e.to_string())) {
                    Ok(read) => {
                        library = Some(lib);
                        read
                    }
                    Err(broke) => Err(broke),
                }
            }
            Err(e) => Err(format!("the library can't be opened ({e})")),
        };
        let _ = weak.upgrade_in_event_loop(move |app| arrived(&app, asked, read));
    });
    match reader {
        Ok(r) => READER.with(|slot| *slot.borrow_mut() = Some(r)),
        Err(e) => log::error("Songs", &format!("Couldn't start the song panel's reader: {e}")),
    }

    // The thread that has been reading since the app started is told its window; started only now if it couldn't
    // be then.
    match window.map_or_else(|| start(path, details, own), Ok) {
        Ok(window) => {
            let _ = window.send(app.as_weak());
        }
        Err(e) => log::error("Songs", &format!("Couldn't start reading the library: {e}")),
    }
}

/// How long the project folders must be quiet before what changed is read: a DAW saves in several steps (the C# app's
/// wait).
const WATCH_QUIET: Duration = Duration::from_secs(3);

/// How long after a save couldn't be read it's tried again.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// How long after the list is up every project folder is walked to catch up with what changed while the app was
/// closed: the pages read what they show first.
const CATCH_UP_AFTER: Duration = Duration::from_secs(20);

/// How long after the catch-up a save it couldn't read (still being written, locked), read by a later try, is still
/// told to the other pages as the catch-up's own are. Later ones are saves made in this run: those pages take them
/// at their next reading, not at each save.
const CATCH_UP_TRIES: Duration = Duration::from_secs(120);

/// How long after what was read couldn't be kept in songs.db (another copy of the app holding it) it's tried again.
const KEEP_AGAIN_AFTER: Duration = Duration::from_secs(60);

/// What the page says when the library has no songs, and the line under it.
const NO_SONGS: &str = "No songs in the library yet";
const NO_SONGS_TEXT: &str =
    "Add the folders your projects are in on Settings › Folders: the songs in them are listed from the next start.";

/// What the page says when the library can't be read (the reason goes under it).
const CANT_READ: &str = "Your songs couldn't be read";

/// Says over the list that the library has no songs, or takes that away.
fn say_empty(data: &SongsData, empty: bool) {
    data.set_message(if empty { NO_SONGS.into() } else { SharedString::new() });
    data.set_message_text(if empty { NO_SONGS_TEXT.into() } else { SharedString::new() });
}

/// Watches the project folders while the app is open and keeps the list up to date, on this background thread: each
/// time saves change, the list is made again and put on the page with a card. A little after the start every folder is
/// walked once to catch up with what changed while the app was closed (even when the folders can't be watched), and
/// what's read is kept in the app's own songs.db.
fn watch(live: &mut Live, weak: &slint::Weak<AppWindow>, library: &Path) {
    let (tx, rx) = mpsc::channel::<Told>();
    let to_watch = tx.clone();
    let watching = FolderWatch::start(live.roots(), is_project_file, WATCH_QUIET, move |c| {
        let _ = to_watch.send(Told::Changed(c));
    });
    // The card's status, stars, heart and tags come here too ([`give`]): this thread has the list.
    *TELL.lock().unwrap_or_else(PoisonError::into_inner) = Some((tx.clone(), live.stopper()));
    let to_tell = tx.clone();
    // Held so the loop below waits rather than ends if the watch can't start: the catch-up and the retries still run.
    let _still_here = tx;
    // How long FL Studio had each song open, once the list is on show and what you give songs is taken (so a star
    // given meanwhile waits for this, rather than being refused): the first pass shows the list again with it.
    let mut minutes_found = learn_minutes(live, library);
    // The board's stage guesses: the shapes kept of the songs' newest saves, then the rest read from their files on a
    // thread of their own, a few at a time, each few kept as it comes.
    minutes_found |= learn_shapes(live, library);
    let shapes_tell = to_tell.clone();
    let mut reading_shapes = read_shapes(live, &shapes_tell);
    // New saves were read while the shapes were being read: read theirs once that ends.
    let mut shapes_again = false;
    // Shapes came in since the list was last shown with them, and when it was: shown every few seconds as they come.
    let mut shapes_waiting = false;
    let mut shapes_shown = Instant::now();
    // And each song's bounces, from the folders beside its saves: listed on a thread of their own, so a slow drive
    // doesn't hold up this one.
    // Only what the look needs of each song: not a copy of the whole list (every song with every save).
    let wanted = mo_app::bounces::songs(&live.songs().rows);
    if let Err(e) = background::spawn("Songs: all bounces", move || {
        let _ = to_tell.send(Told::AllBounces(mo_app::bounces::find_for(&wanted)));
    }) {
        log::warning("Songs", &format!("Couldn't look for the songs' bounces: {e}"));
    }
    let _watch = match watching {
        Ok(w) => {
            log::info("Songs", &format!("Watching {} project folders", w.count()));
            Some(w)
        }
        Err(e) => {
            log::warning("Songs", &format!("Can't watch the project folders: {e}; they're walked once a little later"));
            None
        }
    };
    let mut catch_up_at = Some(Instant::now() + CATCH_UP_AFTER);
    let mut keep_again_at: Option<Instant> = None;
    // Whether a card said that what you give songs isn't kept this time: once a run is enough.
    let mut said_held = false;
    // What came while the thread was busy, still to do ([`next`]).
    let mut waiting: VecDeque<Told> = VecDeque::new();
    // The start-up catch-up found saves songs.db didn't have, and the pages that took their saves from it before
    // (Presets, Curves, MIDI) haven't been told yet: they are once it's kept.
    let mut tell_pages = false;
    // Until when a save the catch-up couldn't read first time (still being written, locked), read by a later try,
    // still counts as the catch-up's: the pages are told of it too.
    let mut catch_up_tries_until: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let due = |at: Option<Instant>| at.map_or(RETRY_AFTER, |at| at.saturating_duration_since(now));
        // A save that couldn't be read (still being written, or locked) is tried again after a quiet moment.
        let wait =
            if minutes_found { Duration::ZERO } else { due(catch_up_at).min(due(keep_again_at)).min(RETRY_AFTER) };
        // What you gave songs on their cards: kept, and the list made again with it. If it can't be kept the list is
        // made again all the same, so the card goes back to what the library has, and a card says why.
        let mut gave = Gave::default();
        // A try again at saves that couldn't be read, not a change the folder watch saw.
        let mut tried_again = false;
        let changes = match next(&rx, &mut waiting, wait) {
            Ok(Told::Changed(changes)) => Some(changes),
            Ok(Told::Give(given)) => {
                gave.keep(live, given);
                None
            }
            Ok(Told::PutOff(key)) => {
                gave.put_off(live, &key);
                None
            }
            Ok(Told::Bounces(key, found)) => {
                live.set_bounces(&key, found);
                // A bounce newer than the newest save can make the song look finished: guessed again.
                minutes_found = true;
                None
            }
            Ok(Told::AllBounces(found)) => {
                minutes_found |= live.set_all_bounces(found);
                None
            }
            Ok(Told::PutAway(id, mtime)) => {
                gave.put_away(live, id, mtime);
                None
            }
            Ok(Told::Shapes(read)) => {
                match live.set_shapes(read) {
                    Ok(changed) => shapes_waiting |= changed,
                    Err(e) => {
                        shapes_waiting = true;
                        log::warning("Songs", &format!("Couldn't keep the songs' shapes in songs.db: {e}"));
                    }
                }
                // Only the board shows them: while it's away they're shown once the reading ends.
                if shapes_waiting && BOARD_ON.load(Ordering::Relaxed) && shapes_shown.elapsed() >= SHAPES_SHOWN_EVERY {
                    minutes_found = true;
                }
                None
            }
            Ok(Told::ShapesDone) => {
                reading_shapes = false;
                minutes_found |= shapes_waiting;
                if std::mem::take(&mut shapes_again) {
                    reading_shapes = read_shapes(live, &shapes_tell);
                }
                None
            }
            // The app is closing, and what you gave songs before it is kept: the rest is let go.
            Ok(Told::Finish(done)) => {
                let _ = done.send(());
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                tried_again = true;
                live.retries()
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut updated = Updated::default();
        if let Some(changes) = &changes {
            updated = live.update(changes);
            if !updated.is_empty() {
                log::info(
                    "Watch",
                    &format!(
                        "Project folders: {} read, {} moved, {} gone{}{} ({} changes seen)",
                        updated.read.len(),
                        updated.moved.len(),
                        updated.gone,
                        mended_line(updated.mended),
                        quiet_line(updated.quiet),
                        changes.files.len()
                    ),
                );
            }
        }
        // What you gave songs meanwhile is kept before the catch-up or a keep can make it wait (review SG-11). Once
        // the app is closing nothing more is read.
        let closing = gave.take_waiting(live, &rx, &mut waiting);
        // Looked at after every batch too, so a folder that keeps changing (a DAW saving every few seconds) can't put
        // it off.
        if !closing && catch_up_at.is_some_and(|at| Instant::now() >= at) {
            catch_up_at = None;
            let began = Instant::now();
            let caught = live.catch_up();
            let how = if caught.stopped {
                "Stopped catching up with the project folders as the app closed, after"
            } else {
                "Caught up with the project folders in"
            };
            log::info(
                "Songs",
                &format!(
                    "{how} {:.1} s: {} read, {} moved, {} gone{}{}",
                    began.elapsed().as_secs_f32(),
                    caught.read.len(),
                    caught.moved.len(),
                    caught.gone,
                    mended_line(caught.mended),
                    quiet_line(caught.quiet)
                ),
            );
            tell_pages = caught.for_card();
            catch_up_tries_until = Some(Instant::now() + CATCH_UP_TRIES);
            updated = updated.and(caught);
        } else if tried_again && updated.for_card() && catch_up_tries_until.is_some_and(|until| Instant::now() < until)
        {
            // A save the catch-up couldn't read (still being written, locked) is read now: the pages hear of it too.
            tell_pages = true;
        }
        let closing = gave.take_waiting(live, &rx, &mut waiting);
        // Kept in songs.db with each change, or a while after it couldn't be kept. As the app closes, only when you gave
        // a song something just now: a save it went to that the library hadn't taken yet (just read) has it here only
        // till it's kept.
        let mut renumbered = false;
        let mut kept_now = false;
        let keep_now = if closing {
            gave.any
        } else {
            !updated.is_empty() || live.note_to_keep() || keep_again_at.is_some_and(|at| Instant::now() >= at)
        };
        if keep_now {
            keep_again_at = match live.keep() {
                Some(Ok(kept)) => {
                    let w = &kept.written;
                    let noted = if w.mended { ", and that the muted samples are mended" } else { "" };
                    // Nothing kept is no line (review SG-12).
                    if w.put > 0 || w.missing > 0 || w.mended {
                        log::info(
                            "Songs",
                            &format!("Kept in songs.db: {} saves read, {} marked missing{noted}", w.put, w.missing),
                        );
                    }
                    for (path, why) in &kept.not_kept {
                        log::warning("Songs", &format!("Couldn't keep {path} in songs.db: {why}"));
                    }
                    renumbered = !w.ids.is_empty();
                    kept_now = true;
                    None
                }
                Some(Err(e)) if closing => {
                    log::warning(
                        "Songs",
                        &format!("{e}, as the app closed: saves not kept yet are read again next time"),
                    );
                    None
                }
                Some(Err(e)) => {
                    log::warning("Songs", &format!("{e}; it's kept with the next change, or in a minute"));
                    Some(Instant::now() + KEEP_AGAIN_AFTER)
                }
                None => None,
            };
        }
        // Presets, Curves and MIDI read their saves from songs.db, and since they're read just after the start that's
        // before this catch-up: saves made as new files while the app was closed were missing from them for the whole
        // run (the 19 November 2026 review's RA-1 and RA-2). Told once, and only when songs.db has them.
        if tell_pages && kept_now && !closing {
            tell_pages = false;
            let _ = weak.upgrade_in_event_loop(|app| {
                crate::midi::songs_caught_up(&app);
                crate::presets::songs_caught_up(&app);
            });
        }
        if closing || (updated.is_empty() && !renumbered && !gave.any && !minutes_found) {
            continue;
        }
        minutes_found = false;
        if shapes_waiting {
            shapes_waiting = false;
            shapes_shown = Instant::now();
        }
        // Saves read just now: their shapes too.
        if !updated.read.is_empty() {
            if reading_shapes {
                shapes_again = true;
            } else {
                reading_shapes = read_shapes(live, &shapes_tell);
            }
        }
        let say_held = gave.held && !std::mem::replace(&mut said_held, true);
        if say_held {
            log::warning("Songs", &format!("{HELD_ONLY}. {HELD_ONLY_WHY}"));
        }
        // Made again when a save's id changed too, so the page picks it by the id the library has.
        let songs = live.songs();
        let found = Arc::new(finder::songs(&songs, Local::now().naive_local()));
        let years = songs.year_counts();
        let offered = fit_songs(&songs);
        let _ = weak.upgrade_in_event_loop(move |app| {
            show_again(&app, songs, years);
            crate::finder::add_source(&app, "songs", found);
            crate::midi::songs_offered(&app, offered);
            for why in &gave.not_kept {
                failed_card(NOT_KEPT, why);
            }
            if say_held {
                failed_card(HELD_ONLY, HELD_ONLY_WHY);
            }
            if !updated.for_card() {
                return;
            }
            let (title, line) = updated.card();
            FEED.with(|f| {
                if let Some(feed) = f.borrow().as_ref() {
                    feed.start(&title, "").done(&line);
                }
            });
        });
    }
}

/// What the page's thread is told: the project folders changed, you gave a song something on its card, or the app is
/// closing (it says back once what you gave songs is kept).
enum Told {
    Changed(Changes),
    Give(Given),
    // "Not now" on the Dust off card, for the song with this key.
    PutOff(String),
    // The bounces of the song with this key, as its card found them just now.
    Bounces(String, Vec<mo_app::bounces::Bounce>),
    // Every song's bounces, looked for once the list is on show.
    AllBounces(HashMap<String, Vec<mo_app::bounces::Bounce>>),
    // The board's x on a card ("not for this save"): the save its guess is from, by id, at its time.
    PutAway(i64, i64),
    // Some songs' newest saves' shapes, just read from their files; and that the reading has ended.
    Shapes(Vec<mo_library::song_writes::ShapeRead>),
    ShapesDone,
    Finish(mpsc::Sender<()>),
}

/// A song given something on its card ([`Live::give`]): the song as the list showed it (its key, and the ids of its
/// saves and copies), and what.
struct Given {
    song: String,
    save_ids: Vec<i64>,
    change: Change,
}

/// What the page's thread does next ([`watch`]): what you gave songs on their cards first, then the app closing, then
/// the rest in the order it came. It waits up to `wait` for something when nothing's waiting.
fn next(rx: &Receiver<Told>, waiting: &mut VecDeque<Told>, wait: Duration) -> Result<Told, RecvTimeoutError> {
    waiting.extend(rx.try_iter());
    let first = waiting
        .iter()
        .position(|t| matches!(t, Told::Give(_) | Told::PutOff(_) | Told::PutAway(..)))
        .or_else(|| waiting.iter().position(|t| matches!(t, Told::Finish(_))))
        .unwrap_or(0);
    match waiting.remove(first) {
        Some(told) => Ok(told),
        None => rx.recv_timeout(wait),
    }
}

/// What you gave songs on their cards in one pass of [`watch`]: whether you gave any, and why each that couldn't be
/// kept wasn't.
#[derive(Default)]
struct Gave {
    any: bool,
    not_kept: Vec<String>,
    /// Shown, but held here only until the app closes ([`mo_app::live::ChangeMade::held_only`], review SG-2).
    held: bool,
}

impl Gave {
    /// Keeps `given` ([`Live::give`]). Why it can't be kept goes in the log now and on a card with the list.
    fn keep(&mut self, live: &mut Live, given: Given) {
        self.any = true;
        match live.give(&given.song, &given.save_ids, &given.change) {
            Ok(made) => self.held |= made.held_only,
            Err(e) => {
                log::warning("Songs", &format!("Couldn't keep what the song was given: {e}"));
                self.not_kept.push(e.to_string());
            }
        }
        let _ = GIVES_WAITING.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }

    /// Keeps the board's guess from save `id` at `mtime` put away ("not for this save", [`Live::put_away`]), as
    /// [`Gave::keep`] keeps a change.
    fn put_away(&mut self, live: &mut Live, id: i64, mtime: i64) {
        self.any = true;
        match live.put_away(id, mtime) {
            Ok(held_only) => self.held |= held_only,
            Err(e) => {
                log::warning("Songs", &format!("Couldn't keep the stage guess put away: {e}"));
                self.not_kept.push(e.to_string());
            }
        }
        let _ = GIVES_WAITING.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }

    /// Keeps the song `key` off the Dust off card ("Not now", [`Live::put_off`]), as [`Gave::keep`] keeps a change.
    fn put_off(&mut self, live: &mut Live, key: &str) {
        self.any = true;
        match live.put_off(key) {
            Ok(held_only) => self.held |= held_only,
            Err(e) => {
                log::warning("Songs", &format!("Couldn't keep the song off the Dust off card: {e}"));
                self.not_kept.push(e.to_string());
            }
        }
        let _ = GIVES_WAITING.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }

    /// Keeps what you've given songs since the pass began, ahead of the rest that came meanwhile ([`next`] takes that
    /// in turn). Whether the app is closing.
    fn take_waiting(&mut self, live: &mut Live, rx: &Receiver<Told>, waiting: &mut VecDeque<Told>) -> bool {
        waiting.extend(rx.try_iter());
        while let Some(at) =
            waiting.iter().position(|t| matches!(t, Told::Give(_) | Told::PutOff(_) | Told::PutAway(..)))
        {
            match waiting.remove(at) {
                Some(Told::Give(given)) => self.keep(live, given),
                Some(Told::PutOff(key)) => self.put_off(live, &key),
                Some(Told::PutAway(id, mtime)) => self.put_away(live, id, mtime),
                _ => {}
            }
        }
        waiting.iter().any(|t| matches!(t, Told::Finish(_)))
    }
}

/// How the window's thread tells the page's thread ([`watch`]), and the switch that stops the page's thread reading
/// your project folders part way as the app closes ([`Live::stopper`]); None until the list is up, and once the app
/// closes.
static TELL: Mutex<Option<(mpsc::Sender<Told>, Arc<AtomicBool>)>> = Mutex::new(None);

/// Counts the looks for the picked song's bounces, so only the newest is shown ([`look_for_bounces`]).
static BOUNCE_ASKS: AtomicU64 = AtomicU64::new(0);

/// How many songs given something on their cards the page's thread hasn't kept yet, waited for as the app closes.
static GIVES_WAITING: AtomicUsize = AtomicUsize::new(0);

/// How long the app waits as it closes for what you gave songs on their cards to be kept: the page's thread may be part
/// way through reading a folder or a save, which it finishes before it stops ([`Live::stopper`]), or waiting for
/// songs.db.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

/// As the app closes: the page's thread is told to stop once what you gave songs on their cards and it hasn't kept yet
/// is kept, waited for [`CLOSE_WAIT`] at most (review SG-11). Its catch-up with your project folders, or a batch of
/// saves it's reading, stops part way so what you gave can't wait behind it (review SS-3): what's left is read at the
/// next start. What still isn't kept is said in the log.
pub fn finish_up() {
    let tell = TELL.lock().unwrap_or_else(PoisonError::into_inner).take();
    let (done, finished) = mpsc::channel();
    let told = tell.as_ref().is_some_and(|(t, _)| t.send(Told::Finish(done)).is_ok());
    if let Some((_, stop)) = &tell {
        stop.store(true, Ordering::SeqCst);
    }
    if told && GIVES_WAITING.load(Ordering::SeqCst) > 0 {
        let _ = finished.recv_timeout(CLOSE_WAIT);
    }
    let left = GIVES_WAITING.load(Ordering::SeqCst);
    if left > 0 {
        log::warning("Songs", &not_kept_line(left));
    }
}

/// The log's words for `left` changes given on song cards that the app closed before it kept.
fn not_kept_line(left: usize) -> String {
    let (changes, were) = if left == 1 { ("change", "was") } else { ("changes", "were") };
    format!(
        "The app closed before {left} {changes} given on song cards {were} kept in songs.db (the Songs page was busy \
         reading your project folders): those songs show as they were at the next start"
    )
}

/// The card's title when what you gave a song couldn't be kept.
const NOT_KEPT: &str = "Couldn't keep that for the song";

/// The card's title and words when what you give songs is shown but not kept this time ([`Gave::held`]).
const HELD_ONLY: &str = "Shown until the app closes";
const HELD_ONLY_WHY: &str = "The app isn't keeping what you give songs on their cards (status, stars, hearts, tags, to-dos) \
                             in its song library this time, so it lasts only until the app closes.";

/// A card that says what couldn't be done (`title`) and why, on the window's thread.
fn failed_card(title: &str, why: &str) {
    FEED.with(|f| {
        if let Some(feed) = f.borrow().as_ref() {
            feed.start(title, "").failed(why);
        }
    });
}

/// The change a click on the card asks for (`what` and `value` as songs.slint's `give` says them), for a song that
/// has `favourite` and `todos` now; `now` is when, in Unix seconds, for a to-do added. A to-do is ticked or taken off
/// by its place in the list (`value`), and the change names it by its words and when it was added, so it finds the
/// same one however the list has changed meanwhile. None for words the card never sends, a to-do with no words, or
/// one that isn't there.
fn asked_change(what: &str, value: &str, favourite: bool, todos: &[Todo], now: i64) -> Option<Change> {
    let at = || value.parse::<usize>().ok().and_then(|n| todos.get(n));
    Some(match what {
        "status" => Change::Status(value.to_owned()),
        "stars" => Change::Rating(value.parse().ok()?),
        "favourite" => Change::Favourite(!favourite),
        "add-tag" => Change::AddTag(value.to_owned()),
        "remove-tag" => Change::RemoveTag(value.to_owned()),
        // Spaces alone are nothing typed: no card says so.
        "add-todo" if value.trim().is_empty() => return None,
        "add-todo" => Change::AddTodo(Todo { text: value.to_owned(), done: false, added: now }),
        "tick-todo" => {
            let t = at()?;
            Change::TickTodo { text: t.text.clone(), added: t.added, done: !t.done }
        }
        "remove-todo" => {
            let t = at()?;
            Change::RemoveTodo { text: t.text.clone(), added: t.added }
        }
        _ => return None,
    })
}

/// Makes `change` (already checked) to a row of the list as it shows, ahead of the library: the page's thread may be
/// busy reading folders for some seconds, and a click shows at once. The list made again after it has what's kept.
fn show_given(row: &mut SongRow, change: &Change) {
    match change {
        Change::Status(status) => row.status.clone_from(status),
        Change::Rating(stars) => row.rating = *stars,
        Change::Favourite(on) => row.favourite = *on,
        Change::AddTag(tag) => {
            if !row.tags.iter().any(|t| song_fields::same_tag(t) == *tag) {
                row.tags.push(tag.clone());
            }
        }
        Change::RemoveTag(tag) => row.tags.retain(|t| song_fields::same_tag(t) != *tag),
        Change::Todos(todos) => row.todos.clone_from(todos),
        Change::AddTodo(todo) => {
            if !row.todos.iter().any(|t| song_fields::same_todo(t, &todo.text, todo.added)) {
                row.todos.push(todo.clone());
            }
        }
        Change::TickTodo { text, added, done } => {
            for t in row.todos.iter_mut().filter(|t| song_fields::same_todo(t, text, *added)) {
                t.done = *done;
            }
        }
        Change::RemoveTodo { text, added } => row.todos.retain(|t| !song_fields::same_todo(t, text, *added)),
        Change::Key(_) | Change::Notes(_) => {}
    }
}

/// You gave a song a status, stars, a heart, a tag or a to-do on its card, or ticked one of its to-dos on Next up
/// (`at`, its place in the page's rows; the picked song when none). Rule 14: the song's, so every save of it gets it,
/// its copies in other folders too. Shown at once, then kept by the page's thread ([`watch`]), which makes the list
/// again; a card says so if it can't be kept.
fn give(app: &AppWindow, at: Option<usize>, what: &str, value: &str) {
    let failed = |why: &str| failed_card(NOT_KEPT, why);
    let now = Local::now().timestamp();
    let asked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let at = at.or(p.picked)?;
        let row = p.songs.rows.get_mut(at)?;
        let change = match asked_change(what, value, row.favourite, &row.todos, now)?.checked() {
            Ok(change) => change,
            Err(why) => return Some(Err(why)),
        };
        let save_ids: Vec<i64> = row.song.saves().chain(&row.song.copies).map(|s| s.id).collect();
        show_given(row, &change);
        Some(Ok(Given { song: row.song.key.clone(), save_ids, change }))
    });
    let given = match asked {
        Some(Ok(given)) => given,
        Some(Err(why)) => return failed(&why),
        None => return,
    };
    // The line in the list, Next up and the card show it now; the filters' counts follow when the list is made again.
    LINES.with(|l| l.notify.reset());
    fill_next_up(app);
    fill_board(app);
    // A song given Finished leaves the Dust off card.
    fill_dust_off(app);
    fill_panel(app);
    GIVES_WAITING.fetch_add(1, Ordering::SeqCst);
    let told = TELL.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|(t, _)| t.send(Told::Give(given)));
    if !matches!(told, Some(Ok(()))) {
        let _ = GIVES_WAITING.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        failed("the song list isn't being kept up to date just now; it shows again as it was when the app next starts");
    }
}

/// The log's words for saves read again only to mend their muted samples, if there were any.
fn mended_line(mended: usize) -> String {
    match mended {
        0 => String::new(),
        n => format!(", {n} read again for their muted samples"),
    }
}

/// The log's words for the saves made for trying the app out that were read, moved or gone ([`Updated::quiet`]).
fn quiet_line(quiet: usize) -> String {
    match quiet {
        0 => String::new(),
        n => format!(", {n} made for trying the app out (kept, not shown)"),
    }
}

/// Puts a list made again on the page, keeping the filters, the song picked and its tab, with its year strip (`years`,
/// [`Songs::year_counts`], counted where the list was made: not on the window's thread).
fn show_again(app: &AppWindow, mut songs: Songs, years: Vec<(i32, usize)>) {
    let _doing = crate::watchdog::doing("Songs: show the list again");
    let data = app.global::<SongsData>();
    let tab = data.get_tab();
    let key = PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|i| p.songs.rows.get(i)).map(|r| r.song.key.clone())
    });
    say_empty(&data, songs.rows.is_empty());
    let row = key.and_then(|k| songs.rows.iter().position(|r| r.song.key == k));
    // Read again only when the picked song's newest save changed: a save of another song leaves its panel as it is.
    let same = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let old = p.picked.and_then(|i| p.songs.rows.get(i)).map(|r| &r.song.latest);
        let new = row.and_then(|i| songs.rows.get(i)).map(|r| &r.song.latest);
        let same = matches!((old, new), (Some(a), Some(b)) if a.id == b.id && a.mtime == b.mtime && a.size == b.size
            && a.file_missing == b.file_missing);
        // The year strip counts the new list, as a fresh read does: a save made this year moves its song (review A-4).
        p.years = years;
        // A song just put off the Dust off card stays off while the page's thread hasn't heard yet (a list made before
        // it did).
        for (key, until) in &p.songs.later {
            songs.later.entry(key.clone()).or_insert(*until);
        }
        // The same for a stage guess just put away.
        songs.not_now.extend(p.songs.not_now.iter().copied());
        // The old list (thousands of songs) is let go on the freeing thread, not the window's.
        background::free_later(std::mem::replace(&mut p.songs, songs));
        p.picked = if same { row } else { None };
        same
    });
    refresh(app);
    if same {
        data.set_selected(row.map_or(-1, |r| i32::try_from(r).unwrap_or(-1)));
        fill_panel(app);
    } else if let Some(row) = row {
        pick(app, Some(row));
        data.set_tab(tab);
    } else {
        pick(app, None);
    }
}

/// Wires the search box, the filters, their chips and the year strip.
fn wire_filters(app: &AppWindow) {
    let data = app.global::<SongsData>();
    let weak = app.as_weak();
    data.on_search_edited(move |text| {
        change(&weak, |f| f.search = text.to_string());
    });
    let weak = app.as_weak();
    data.on_toggle_choice(move |chip, choice| {
        change(&weak, |f| f.toggle(&chip, &choice));
    });
    let weak = app.as_weak();
    data.on_clear_chip(move |chip| {
        change(&weak, |f| f.clear_chip(&chip));
    });
    let weak = app.as_weak();
    data.on_toggle(move |kind| {
        change(&weak, |f| match kind.as_str() {
            "favourites" => f.favourites = !f.favourites,
            "missing" => f.missing = !f.missing,
            "todos" => f.todos = !f.todos,
            "has-sound" => f.has_sound = !f.has_sound,
            "no-sound" => f.no_sound = !f.no_sound,
            "hide-archived" => f.hide_archived = !f.hide_archived,
            "only" => f.only = None,
            _ => {}
        });
    });
    let weak = app.as_weak();
    data.on_order_picked(move |column| {
        let (Some(app), Some(by)) = (weak.upgrade(), Order::from_column(&column)) else { return };
        let _doing = crate::watchdog::doing("Songs: order");
        PAGE.with(|p| p.borrow_mut().listed.order_by(by));
        refresh(&app);
    });
    let weak = app.as_weak();
    data.on_clear(move || {
        change(&weak, |f| *f = Filters { hide_archived: f.hide_archived, ..Filters::default() });
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
}

/// The list's keys and its lines' right-click menu: Up and Down, Esc, and the menu (review X3-8, X3-24), and the
/// card's status, stars, heart and tags. For the screenshots too: there, with nothing to keep it in, a change only
/// shows.
fn wire_list(app: &AppWindow) {
    let data = app.global::<SongsData>();
    let weak = app.as_weak();
    data.on_give(move |what, value| {
        if let Some(app) = weak.upgrade() {
            give(&app, None, &what, &value);
        }
    });
    let weak = app.as_weak();
    data.on_play_sound(move |path| {
        let Some(app) = weak.upgrade() else { return };
        let mut title = String::new();
        with_picked(|_, r| title.clone_from(&r.song.name));
        crate::song_player::play(&app, &path, &title);
    });
    let weak = app.as_weak();
    data.on_not_now(move |row| {
        if let (Some(app), Ok(row)) = (weak.upgrade(), usize::try_from(row)) {
            not_now(&app, row);
        }
    });
    // The board: a card dragged to another column, or its guess's arrow, gives its song that status; its x puts the
    // guess away for this save; "Show all" and "Show more" show more of a column.
    let weak = app.as_weak();
    data.on_board_move(move |row, status| {
        if let (Some(app), Ok(row)) = (weak.upgrade(), usize::try_from(row)) {
            give(&app, Some(row), "status", &status);
        }
    });
    let weak = app.as_weak();
    data.on_board_put_away(move |row| {
        if let (Some(app), Ok(row)) = (weak.upgrade(), usize::try_from(row)) {
            put_away(&app, row);
        }
    });
    let weak = app.as_weak();
    data.on_board_show(move |what| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            match usize::try_from(what) {
                // A column's "Show more", by its place.
                Ok(at) => {
                    if let Some(n) = p.board_cards.get_mut(at) {
                        *n = (*n + BOARD_CARDS).min(BOARD_MOST);
                    }
                }
                // Its Idea column's "Show all" (or "Last year only").
                Err(_) => p.all_ideas = !p.all_ideas,
            }
        });
        fill_board(&app);
    });
    // The board switched on: filled now (it isn't while it's away).
    let weak = app.as_weak();
    data.on_board_shown(move || {
        if let Some(app) = weak.upgrade() {
            fill_board(&app);
        }
    });
    // A to-do ticked on Next up: given to its song, which needn't be the one picked.
    let weak = app.as_weak();
    data.on_give_to(move |row, what, value| {
        if let (Some(app), Ok(row)) = (weak.upgrade(), usize::try_from(row)) {
            give(&app, Some(row), &what, &value);
        }
    });
    let weak = app.as_weak();
    data.on_pick_save(move |path, name| {
        let Some(app) = weak.upgrade() else { return };
        // The newest save is what the buttons act on anyway: picking it is picking none.
        let mut older = false;
        with_picked(|_, r| {
            older = path.as_str() != r.song.latest.path && r.song.saves().any(|s| s.path == path.as_str());
        });
        show_picked_save(&app, if older { Some((path.as_str(), name.as_str())) } else { None });
    });
    let weak = app.as_weak();
    data.on_step(move |by| {
        if let Some(app) = weak.upgrade() {
            step(&app, by);
        }
    });
    // Made here, for the song right-clicked (picked first), so each line names its DAW and shows its key as the
    // shortcut list has it now.
    let weak = app.as_weak();
    data.on_song_menu_for(move |row| {
        let Some(app) = weak.upgrade() else { return };
        let daw = PAGE.with(|p| {
            let p = p.borrow();
            usize::try_from(row).ok().and_then(|i| p.songs.rows.get(i)).map(|r| r.song.latest.daw.clone())
        });
        let items = daw.map(|daw| song_menu(&daw, &crate::shortcuts())).unwrap_or_default();
        app.global::<SongsData>().set_song_menu(ModelRc::new(VecModel::from(items)));
    });
    let weak = app.as_weak();
    data.on_song_menu_chosen(move |id| {
        if let Some(app) = weak.upgrade() {
            run_action(&app, &id);
        }
    });
}

/// Says how Ctrl+K opens a song: on this page, picked; or its latest save in its DAW (Ctrl+Enter).
fn finder_opens() {
    crate::finder::on_open(Kind::Song, |app, hit, how| {
        let Some(row) = PAGE.with(|p| p.borrow().songs.rows.iter().position(|r| r.song.key == hit.id)) else {
            return false;
        };
        match how {
            How::InDaw => PAGE.with(|p| {
                let p = p.borrow();
                if let Some(r) = p.songs.rows.get(row) {
                    open_main(&p, &r.song.latest);
                }
            }),
            How::Open | How::Play => {
                crate::go_to(app, Place::Songs.index() as i32);
                show_song(app, row);
            }
        }
        true
    });
}

/// Shows made-up songs with the filters wired, and Ctrl+K opening them, for mo-screens (no library, nothing read).
pub fn show_sample(app: &AppWindow, songs: Songs) {
    SAMPLE.with(|s| s.set(true));
    wire_filters(app);
    wire_list(app);
    finder_opens();
    // A handful of songs: counted here.
    let years = songs.year_counts();
    show(app, songs, years);
}

/// Puts a freshly read list on the page, keeping the filters as they are, with its year strip (`years`, as
/// [`show_again`] has it).
fn show(app: &AppWindow, songs: Songs, years: Vec<(i32, usize)>) {
    let _doing = crate::watchdog::doing("Songs: show the list");
    let data = app.global::<SongsData>();
    data.set_tabs(strings(Tab::ALL.iter().map(|t| t.name())));
    data.set_saves_tab(Tab::Saves.index() as i32);
    say_empty(&data, songs.rows.is_empty());
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.years = years;
        // The old list (thousands of songs) is let go on the freeing thread, not the window's.
        background::free_later(std::mem::replace(&mut p.songs, songs));
    });
    pick(app, None);
    data.set_lines(LINES.with(|l| ModelRc::from(l.clone() as Rc<dyn Model<Data = SongLine>>)));
    refresh(app);
}

/// Changes the filters and shows the list again.
fn change(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut Filters)) {
    let _doing = crate::watchdog::doing("Songs: search or filter");
    let Some(app) = weak.upgrade() else { return };
    PAGE.with(|p| f(&mut p.borrow_mut().filters));
    refresh(&app);
}

/// Shows only the songs with one of `saves` (the Plugins page's Show in Songs), under a chip called `label`. The other
/// filters go, archived songs included, so every song counted on the plugin's card is there.
pub fn show_only(app: &AppWindow, label: &str, saves: HashSet<i64>) {
    // Every other filter goes, but "hide archived" is a choice the user keeps.
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let hide_archived = p.filters.hide_archived;
        p.filters = Filters { only: Some((label.to_owned(), saves)), hide_archived, ..Filters::default() };
    });
    pick(app, None);
    refresh(app);
}

/// Shows the song a save belongs to, by the save's path (Presets and Curves know their saves by path). Whether it was
/// found.
pub fn show_save_path(app: &AppWindow, save: &str) -> bool {
    let row = PAGE.with(|p| {
        p.borrow().songs.rows.iter().position(|r| r.song.saves().chain(&r.song.copies).any(|s| s.path == save))
    });
    match row {
        Some(row) => {
            show_song(app, row);
            true
        }
        None => false,
    }
}

/// Picks the song with this save and brings it into view (the Plugins page's song lines). Returns whether the list has
/// it (not while the library is still being read).
pub fn show_save(app: &AppWindow, save: i64) -> bool {
    let row = PAGE.with(|p| {
        p.borrow().songs.rows.iter().position(|r| r.song.saves().chain(&r.song.copies).any(|s| s.id == save))
    });
    match row {
        Some(row) => {
            show_song(app, row);
            true
        }
        None => false,
    }
}

/// The newest of your Bitwig saves with the name Bitwig shows for the project open in it (the file's name without its
/// ending, in any case, as the C# app matched them), for the rail's "In your DAWs" line: by the list's real dates, so
/// one dated still to come, or with no date, isn't taken for the newest (mo_formats::dates). None while the list is
/// still being read.
pub fn bitwig_save_named(name: &str) -> Option<i64> {
    let wanted = name.trim().to_lowercase();
    PAGE.with(|p| {
        let p = p.borrow();
        let real = &p.songs.real;
        p.songs
            .rows
            .iter()
            .flat_map(|r| r.song.saves())
            .filter(|s| s.daw == BITWIG_STUDIO && s.name.trim().to_lowercase() == wanted)
            .max_by_key(|s| (real.seconds(s.mtime), s.mtime))
            .map(|s| s.id)
    })
}

/// Picks the song at `row` and brings it into view in the list, clearing the filters first when they hide it (but
/// Hide archived, unless the song is archived itself).
fn show_song(app: &AppWindow, row: usize) {
    let _doing = crate::watchdog::doing("Songs: show a song");
    let hidden = PAGE.with(|p| !p.borrow().view.entries.contains(&Entry::Song(row)));
    if hidden {
        PAGE.with(|p| {
            let mut page = p.borrow_mut();
            let page = &mut *page;
            if let Some(song) = page.songs.rows.get(row) {
                page.filters = page.filters.to_show(song);
            }
        });
        refresh(app);
    }
    let data = app.global::<SongsData>();
    // Every line is one height, so the song's place is its line number times that (as the list itself works it out).
    let line = data.get_line_height();
    let y = PAGE.with(|p| {
        let at = p.borrow().view.entries.iter().position(|e| *e == Entry::Song(row))?;
        Some(at as f32 * line)
    });
    if let Some(y) = y {
        data.set_show_y(y);
        data.set_show_ask(data.get_show_ask().wrapping_add(1));
    }
    pick(app, Some(row));
}

/// Picks the song above (`by` -1) or below (1) the one picked, the first one shown when none is, and brings it into
/// sight: Up and Down on the list, and Down from the search box (review X3-8).
fn step(app: &AppWindow, by: i32) {
    let next = PAGE.with(|p| {
        let p = p.borrow();
        p.view.step(p.picked, by)
    });
    if let Some(row) = next {
        show_song(app, row);
    }
}

/// Works out what the rows and filters let through and shows it.
fn refresh(app: &AppWindow) {
    let data = app.global::<SongsData>();
    PAGE.with(|p| {
        {
            let mut p = p.borrow_mut();
            p.now = Local::now().naive_local();
            p.view = p.songs.view(&p.filters, p.now);
            let (listed, now) = (p.listed, p.now);
            let p = &mut *p;
            p.view.put_in_order(&p.songs, listed, now);
        }
        LINES.with(|l| l.notify.reset());
        let p = p.borrow();
        let view = &p.view;
        let f = &p.filters;
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
        data.set_count(view.count.as_str().into());
        // The heading of the column the list goes by is lit, with which way (the UX review of 10 Nov, B5).
        data.set_order_column(p.listed.by.column().into());
        data.set_order_reversed(p.listed.reversed);
        data.set_shown(i32::try_from(view.shown).unwrap_or(i32::MAX));
        let chips = p.songs.chips(f, &view.counts);
        FILTER_MODELS.with(|m| {
            data.set_daw_filter(m[0].view(&chips.daw));
            data.set_status_filter(m[1].view(&chips.status));
            data.set_tag_filter(m[2].view(&chips.tag));
        });
        data.set_favourites(f.favourites);
        data.set_missing(f.missing);
        data.set_has_todos(f.todos);
        data.set_has_sound(f.has_sound);
        data.set_no_sound(f.no_sound);
        data.set_hide_archived(f.hide_archived);
        data.set_any_filter(f.any());
        data.set_only(f.only.as_ref().map(|o| o.0.as_str()).unwrap_or_default().into());
        if data.get_search().as_str() != f.search {
            data.set_search(f.search.as_str().into());
        }
        // The card's tags light with the Tag filter.
        if let Some(r) = p.picked.and_then(|i| p.songs.rows.get(i)) {
            fill_tags(&data, f, r);
        }
    });
    fill_next_up(app);
    fill_board(app);
    fill_dust_off(app);
}

/// The Dust off card: this week's few songs you spent real time on and left, each with its status, time and year.
fn fill_dust_off(app: &AppWindow) {
    let data = app.global::<SongsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        data.set_dust_lines(model(p.songs.dust_off_card(p.now).into_iter().filter_map(|row| {
            let r = p.songs.rows.get(row)?;
            let year = r.saved.map(|d| chrono::Datelike::year(&d).to_string()).unwrap_or_default();
            let about = [r.status.as_str(), &r.time_text(), &year]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            Some(DustLine { row: i32::try_from(row).ok()?, name: r.song.name.as_str().into(), about: about.into() })
        })));
    });
}

/// Looks again for the picked song's bounces, on a thread of its own (it lists its folders), so one exported a minute
/// ago shows on its card and its line; the page's thread keeps them for the lists it makes from here.
fn look_for_bounces(app: &AppWindow) {
    // Made-up songs (mo-screens) have no folders to look in.
    if SAMPLE.with(Cell::get) {
        return;
    }
    let Some((row, rivals)) = PAGE.with(|p| {
        let p = p.borrow();
        let row = p.picked.and_then(|i| p.songs.rows.get(i))?;
        Some((row.clone(), mo_app::bounces::rivals(&p.songs.rows, row)))
    }) else {
        return;
    };
    // Newest wins: a look that ends after a newer one began (another song picked) is let go.
    let ask = BOUNCE_ASKS.fetch_add(1, Ordering::SeqCst) + 1;
    let weak = app.as_weak();
    let looked = background::spawn("Songs: bounces", move || {
        // None of its folders there (a drive away): what was found before stands.
        if !row.song.saves().any(|s| Path::new(&s.folder).is_dir()) {
            return;
        }
        let found = mo_app::bounces::Folders::default().of(&row, &rivals);
        if found == row.bounces || BOUNCE_ASKS.load(Ordering::SeqCst) != ask {
            return;
        }
        let key = row.song.key.clone();
        let _ = weak.upgrade_in_event_loop(move |app| {
            if BOUNCE_ASKS.load(Ordering::SeqCst) != ask {
                return;
            }
            let (shown, picked, filtered) = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let filtered = p.filters.has_sound != p.filters.no_sound;
                let picked = p.picked.and_then(|i| p.songs.rows.get(i)).is_some_and(|r| r.song.key == key);
                let Some(r) = p.songs.rows.iter_mut().find(|r| r.song.key == key) else { return (false, false, false) };
                r.bounces.clone_from(&found);
                (true, picked, filtered)
            });
            if !shown {
                return;
            }
            // A sound filter on: the song may come or go from the list.
            if filtered {
                refresh(&app);
            } else {
                LINES.with(|l| l.notify.reset());
                fill_board(&app);
            }
            if picked {
                fill_panel(&app);
            }
            if let Some((tell, _)) = TELL.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                let _ = tell.send(Told::Bounces(key, found));
            }
        });
    });
    if let Err(e) = looked {
        log::warning("Songs", &format!("Couldn't look for the song's bounces: {e}"));
    }
}

/// "Not now" on the Dust off card: the song at `row` leaves the card at once, and is kept off it for about three months
/// ([`Live::put_off`], on the page's thread).
fn not_now(app: &AppWindow, row: usize) {
    let key = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let key = p.songs.rows.get(row)?.song.key.clone();
        let until = Local::now().timestamp() + mo_app::songs::NOT_NOW_DAYS * 86_400;
        p.songs.later.insert(key.clone(), until);
        Some(key)
    });
    let Some(key) = key else { return };
    // The list too: ordered by Dust off, the song leaves the top.
    refresh(app);
    GIVES_WAITING.fetch_add(1, Ordering::SeqCst);
    let told = TELL.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|(t, _)| t.send(Told::PutOff(key)));
    if !matches!(told, Some(Ok(()))) {
        let _ = GIVES_WAITING.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        failed_card(
            NOT_KEPT,
            "the song list isn't being kept up to date just now, so it's back on the card when the app next starts",
        );
    }
}

/// The board: a column a status, each with the songs the filters show in the list's order (a few screens of them),
/// its stage guess where it looks further along than its status says. Only while the board is on show.
fn fill_board(app: &AppWindow) {
    let data = app.global::<SongsData>();
    BOARD_ON.store(data.get_board(), Ordering::Relaxed);
    // Not while a card is carried: its column would be made again under it. Filled once it's let go.
    if !data.get_board() || data.get_board_dragging() {
        return;
    }
    PAGE.with(|p| {
        let p = p.borrow();
        let columns = mo_app::board::columns(&p.view, &p.songs, p.all_ideas, p.now);
        data.set_board_all_ideas(p.all_ideas);
        BOARD.with_borrow_mut(|board| {
            for (at, column) in columns.iter().enumerate() {
                let most = p.board_cards.get(at).copied().unwrap_or(BOARD_CARDS);
                let cards: Vec<BoardCard> = column
                    .cards
                    .iter()
                    .take(most)
                    .filter_map(|&row| {
                        let r = p.songs.rows.get(row)?;
                        let guess = p.songs.suggestion(row);
                        Some(BoardCard {
                            row: i32::try_from(row).ok()?,
                            name: r.song.name.as_str().into(),
                            daw: r.song.latest.daw.as_str().into(),
                            time: r.time_text().into(),
                            sound: !r.bounces.is_empty(),
                            todos: i32::try_from(r.todos_left()).unwrap_or(i32::MAX),
                            guess: guess.map(|g| g.stage).unwrap_or_default().into(),
                            guess_why: guess.map(|g| guess_words(g, r, p.now)).unwrap_or_default().into(),
                        })
                    })
                    .collect();
                let shown = column.cards.len().min(most);
                let line = BoardColumn {
                    status: column.status.into(),
                    count: mo_app::songs::thousands(column.cards.len() + column.older).into(),
                    older: i32::try_from(column.older).unwrap_or(i32::MAX),
                    // Past the most a column shows, there's no more to ask for.
                    more: if most >= BOARD_MOST {
                        0
                    } else {
                        i32::try_from(column.cards.len() - shown).unwrap_or(i32::MAX)
                    },
                    cards: ModelRc::from(board.cards(at)),
                };
                board.set_cards(at, cards);
                board.set_column(at, line);
            }
            if data.get_board_columns().row_count() != columns.len() {
                data.set_board_columns(ModelRc::from(board.columns.clone()));
            }
        });
    });
}

/// The board's columns and each one's cards, changed in place so a column keeps where it's scrolled to, and only the
/// cards that changed are made again.
#[derive(Default)]
struct Board {
    columns: Rc<VecModel<BoardColumn>>,
    cards: Vec<Rc<VecModel<BoardCard>>>,
}

impl Board {
    fn cards(&mut self, at: usize) -> Rc<VecModel<BoardCard>> {
        while self.cards.len() <= at {
            self.cards.push(Rc::default());
        }
        self.cards[at].clone()
    }

    fn set_cards(&mut self, at: usize, cards: Vec<BoardCard>) {
        let model = self.cards(at);
        if model.row_count() != cards.len() {
            model.set_vec(cards);
            return;
        }
        for (n, card) in cards.into_iter().enumerate() {
            if model.row_data(n).as_ref() != Some(&card) {
                model.set_row_data(n, card);
            }
        }
    }

    fn set_column(&mut self, at: usize, column: BoardColumn) {
        if at < self.columns.row_count() {
            let same = self.columns.row_data(at).is_some_and(|c| {
                c.status == column.status && c.count == column.count && c.older == column.older && c.more == column.more
            });
            if !same {
                self.columns.set_row_data(at, column);
            }
        } else {
            self.columns.push(column);
        }
    }
}

thread_local! {
    static BOARD: RefCell<Board> = RefCell::new(Board::default());
}

/// A card's guess said in full, for its tooltip: "Looks like Mixing now", the save it's from, and why.
fn guess_words(guess: &mo_library::stage::Guess, r: &SongRow, now: NaiveDateTime) -> String {
    let save = mo_app::board::newest(&r.song);
    let day =
        if chrono::Datelike::year(&save.modified) == chrono::Datelike::year(&now) { "%-d %b" } else { "%-d %b %Y" };
    let mut words = format!("Looks like {} now\nFrom the newest save ({}):", guess.stage, save.modified.format(day));
    for why in &guess.why {
        words.push_str("\n\u{b7} ");
        words.push_str(why);
    }
    words.push_str("\nYour status stays until you press \u{2192}.");
    words
}

/// The board's x on a card: its guess goes, for this save; a newer save is guessed afresh. Kept by the page's thread
/// ([`Live::put_away`]).
fn put_away(app: &AppWindow, row: usize) {
    let from = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let from = p.songs.rows.get(row)?.guessed_from?;
        p.songs.not_now.insert(from);
        Some(from)
    });
    let Some((id, mtime)) = from else { return };
    fill_board(app);
    GIVES_WAITING.fetch_add(1, Ordering::SeqCst);
    let told =
        TELL.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|(t, _)| t.send(Told::PutAway(id, mtime)));
    if !matches!(told, Some(Ok(()))) {
        let _ = GIVES_WAITING.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        failed_card(
            NOT_KEPT,
            "the song list isn't being kept up to date just now, so the guess is back when the app next starts",
        );
    }
}

/// Next up: each song the filters show with something still to do, and those to-dos under it, and how many there are.
/// A few hundred lines at most, so made whole.
fn fill_next_up(app: &AppWindow) {
    let data = app.global::<SongsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let lines = p.view.next_up(&p.songs);
        let left = lines.iter().filter(|l| matches!(l, NextUp::Todo(..))).count();
        data.set_next_count(i32::try_from(left).unwrap_or(i32::MAX));
        data.set_next_lines(model(lines.into_iter().filter_map(|l| {
            let (NextUp::Song(row) | NextUp::Todo(row, _)) = l;
            let r = p.songs.rows.get(row)?;
            let row = i32::try_from(row).ok()?;
            Some(match l {
                NextUp::Song(_) => NextLine {
                    song: true,
                    row,
                    n: -1,
                    text: r.song.name.as_str().into(),
                    about: [r.status.as_str(), &r.short_date(p.now)]
                        .into_iter()
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                        .into(),
                },
                NextUp::Todo(_, n) => NextLine {
                    song: false,
                    row,
                    n: i32::try_from(n).ok()?,
                    text: r.todos.get(n)?.text.as_str().into(),
                    about: SharedString::new(),
                },
            })
        })));
    });
}

/// The picked song's to-dos for its card, in their order (a ticked one stays where it is, so a second click can't land
/// on another), each with its place in the song's list and the day it was added; and how many are left ("3 left").
fn fill_todos(data: &SongsData, r: &SongRow, now: NaiveDateTime) {
    let line = |(n, t): (usize, &Todo)| TodoLine {
        n: i32::try_from(n).unwrap_or(-1),
        text: t.text.as_str().into(),
        done: t.done,
        date: added_day(t.added, now).into(),
    };
    data.set_song_todos(model(r.todos.iter().enumerate().map(line)));
    let left = r.todos_left();
    data.set_todos_left(if r.todos.is_empty() { SharedString::new() } else { format!("{left} left").into() });
}

/// The day a to-do was added: "16 Nov" this year, "16 Nov 2025" before.
fn added_day(added: i64, now: NaiveDateTime) -> String {
    use chrono::Datelike as _;
    let Some(day) = chrono::DateTime::from_timestamp(added, 0).map(|t| t.with_timezone(&Local).naive_local()) else {
        return String::new();
    };
    if day.year() == now.year() { day.format("%-d %b").to_string() } else { day.format("%-d %b %Y").to_string() }
}

fn line(songs: &Songs, entry: &Entry, now: NaiveDateTime) -> SongLine {
    match entry {
        Entry::Heading(h) => SongLine { heading: h.to_string().into(), row: -1, ..SongLine::default() },
        Entry::Song(i) => {
            let Some(r) = songs.rows.get(*i) else { return SongLine::default() };
            SongLine {
                heading: SharedString::new(),
                row: i32::try_from(*i).unwrap_or(-1),
                name: r.song.name.as_str().into(),
                details: r.details_line().into(),
                date: r.short_date(now).into(),
                date_tip: r.date_tip().into(),
                missing: r.missing_text().into(),
                missing_words: r.missing_words().into(),
                rating: r.rating,
                favourite: r.favourite,
                todos: i32::try_from(r.todos_left()).unwrap_or(i32::MAX),
                time: r.time_text().into(),
                sound: !r.bounces.is_empty(),
                // The first status ("Idea") is every song's to begin with, so it isn't shown.
                status: if r.status == STATUSES[0] { SharedString::new() } else { r.status.as_str().into() },
                daw: r.song.latest.daw.as_str().into(),
            }
        }
    }
}

fn strings<'a>(items: impl Iterator<Item = &'a str>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.map(SharedString::from).collect::<Vec<_>>()))
}

/// Opens the panel on the song at `row` of the page's rows, or closes it.
fn pick(app: &AppWindow, row: Option<usize>) {
    let _doing = crate::watchdog::doing("Songs: pick a song");
    let data = app.global::<SongsData>();
    let (row, id) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let row = row.filter(|r| *r < p.songs.rows.len());
        if row.is_some() && row == p.picked {
            // Already open: keep its tab and what has been read.
            return (row, None);
        }
        p.picked = row;
        p.asked += 1;
        p.detail = None;
        (row, row.and_then(|r| p.songs.rows.get(r)).map(|r| (p.asked, r.song.latest.id)))
    });
    data.set_selected(row.map_or(-1, |r| i32::try_from(r).unwrap_or(-1)));
    if row.is_none() {
        crate::arrangement::show_song(app, None, false);
    }
    if row.is_none() || id.is_some() {
        // Another song, or none: the save picked on the last one's Saves tab is no longer what the buttons act on.
        show_picked_save(app, None);
    }
    let Some((asked, id)) = id else { return };
    data.set_tab(Tab::Overview.index() as i32);
    data.set_health_open(false);
    data.set_detail_message("Reading…".into());
    fill_panel(app);
    look_for_bounces(app);
    with_picked(|_, r| crate::arrangement::show_song(app, Some(&r.song.latest.path), r.song.latest.file_missing));
    READER.with(|slot| {
        if let Some(reader) = slot.borrow().as_ref() {
            reader.ask((asked, id));
        }
    });
}

/// A save's record has been read: shown if it's still the picked song's.
fn arrived(app: &AppWindow, asked: u64, read: Result<Option<SaveDetail>, String>) {
    let _doing = crate::watchdog::doing("Songs: show the picked song's details");
    let data = app.global::<SongsData>();
    if PAGE.with(|p| p.borrow().asked != asked) {
        return;
    }
    match read {
        Ok(Some(detail)) => {
            PAGE.with(|p| p.borrow_mut().detail = Some(detail));
            data.set_detail_message(SharedString::new());
        }
        Ok(None) => data.set_detail_message("This save isn't in the library any more.".into()),
        Err(e) => {
            log::warning("Songs", &e);
            data.set_detail_message(format!("Couldn't read this save's details: {e}").into());
        }
    }
    fill_detail(app);
}

/// The picked song's tags on its card, each lit while the Tag filter has it.
fn fill_tags(data: &SongsData, filters: &Filters, r: &SongRow) {
    data.set_song_tags(model(
        r.tags
            .iter()
            .map(|t| SongTag { text: t.as_str().into(), on: filters.tags.iter().any(|f| f.eq_ignore_ascii_case(t)) }),
    ));
}

/// Puts the picked song in the panel: what the list knows, then [`fill_detail`].
fn fill_panel(app: &AppWindow) {
    let data = app.global::<SongsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let Some(r) = p.picked.and_then(|i| p.songs.rows.get(i)) else { return };
        data.set_song(panel_song(r));
        fill_tags(&data, &p.filters, r);
        fill_todos(&data, r, p.now);
        data.set_song_sounds(model(r.bounces.iter().map(|b| BounceLine {
            path: b.path.as_str().into(),
            name: b.name.as_str().into(),
            date: added_day(b.mtime, p.now).into(),
        })));
        data.set_runs(model(r.runs.iter().rev().map(|run| SharedString::from(panel::run_line(run)))));
        data.set_saves_heading(panel::saves_heading(r).into());
        let older = panel::older_saves(r, &p.songs.real);
        data.set_older_heading(older.as_ref().map_or("", |o| o.heading).into());
        data.set_older_see_all(older.as_ref().map(|o| o.see_all.as_str()).unwrap_or_default().into());
        data.set_older(model(
            older.into_iter().flat_map(|o| o.lines).map(|l| OlderLine { text: l.text.into(), path: l.path.into() }),
        ));
        data.set_saves(model(panel::save_lines(r, &p.songs.real).into_iter().map(|l| match l {
            PanelSaveLine::Heading { label, details } => {
                SaveLine { run: label.into(), details: details.into(), ..SaveLine::default() }
            }
            PanelSaveLine::Save { name, date, path, missing, newest, daw } => SaveLine {
                run: SharedString::new(),
                details: SharedString::new(),
                name: name.into(),
                date: date.into(),
                path: path.into(),
                missing,
                newest,
                daw: daw.into(),
            },
        })));
    });
    fill_detail(app);
}

/// The parts of the panel that come from the newest save's record: what the list knows until it has been read.
fn fill_detail(app: &AppWindow) {
    let data = app.global::<SongsData>();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(picked) = p.picked else { return };
        let health = panel::health(&p.songs, picked, p.detail.as_ref());
        p.health = health.iter().map(|h| h.go).collect();
        data.set_health_summary(panel::health_summary(&health).into());
        data.set_health(model(health.into_iter().map(|h| HealthLine {
            text: h.text.into(),
            why: h.why.into(),
            action: h.go.label().into(),
        })));
        let p = &*p;
        let Some(r) = p.songs.rows.get(picked) else { return };
        let detail = p.detail.as_ref();
        data.set_counts(model(panel::counts(r, detail).into_iter().map(|c| CountLine {
            text: c.text.into(),
            missing: c.missing.into(),
            tab: c.tab.index() as i32,
        })));
        let empty = SaveDetail::default();
        let d = detail.unwrap_or(&empty);
        data.set_plugins_heading(panel::plugins_heading(d).into());
        data.set_plugins(model(d.plugins.iter().map(|pl| {
            let l = panel::PluginLine::new(pl);
            PluginLine { name: l.name.into(), about: l.about.into(), installed: l.installed }
        })));
        data.set_samples_heading(panel::samples_heading(d).into());
        data.set_samples(model(d.samples.iter().map(|sm| {
            let l = panel::SampleLine::new(sm);
            SampleLine {
                name: l.name.into(),
                folder: l.folder.into(),
                path: l.path.into(),
                missing: l.missing,
                unplayed: l.unplayed,
            }
        })));
        data.set_tracks(d.tracks.join(", ").into());
        data.set_details(model(
            d.details.iter().map(|(n, v)| DetailPair { name: n.as_str().into(), value: v.as_str().into() }),
        ));
        data.set_warnings(model(d.warnings.iter().map(|w| SharedString::from(w.as_str()))));
    });
}

fn panel_song(r: &SongRow) -> PanelSong {
    PanelSong {
        daw: r.song.latest.daw.as_str().into(),
        facts: panel::facts(r).into(),
        name: r.song.name.as_str().into(),
        saves: panel::saves_text(r).into(),
        backed: crate::backup::song_line(&r.song, r.newest_time()).into(),
        status: r.status.as_str().into(),
        rating: r.rating,
        favourite: r.favourite,
        key: r.key().into(),
        key_heard: r.musical_key.trim().is_empty() && !r.key().is_empty(),
        tags: r.tags.join(", ").into(),
        notes: r.notes.as_str().into(),
        time: r.time_text().into(),
        time_tip: r.time_words().into(),
    }
}

fn model<T: Clone + 'static>(items: impl Iterator<Item = T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items.collect::<Vec<_>>()))
}

/// The place in the page's rows of the song called `name`: for the screenshots.
pub fn sample_row(name: &str) -> Option<usize> {
    PAGE.with(|p| p.borrow().songs.rows.iter().position(|r| r.song.name == name))
}

/// Opens the panel on the song at `row` with `detail` as its newest save's record, without reading the library: for
/// the screenshots.
pub fn show_picked(app: &AppWindow, row: usize, detail: SaveDetail, tab: Tab) {
    pick(app, Some(row));
    let asked = PAGE.with(|p| p.borrow().asked);
    arrived(app, asked, Ok(Some(detail)));
    app.global::<SongsData>().set_tab(tab.index() as i32);
}

/// Runs one of the Songs page's own shortcuts ([`mo_app::shortcuts::SONGS_ACTIONS`]). Returns whether it did
/// anything, so an unused key goes on.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let data = app.global::<SongsData>();
    if !has_pick() {
        return false;
    }
    match id {
        "projects.open" => data.invoke_open(),
        "projects.openOwn" => data.invoke_open_own(),
        "projects.openInBitwig" => {
            let mut done = false;
            with_picked(|p, r| {
                let save = aimed(r);
                if save.daw == FL {
                    open_in_bitwig(p, save);
                    done = true;
                } else if save.daw.starts_with(BITWIG) {
                    open_save(save);
                    done = true;
                }
            });
            return done;
        }
        "projects.copyFl" | "projects.copyStudioOne" => {
            let daw = if id == "projects.copyFl" { daw_copy::Daw::FlStudio } else { daw_copy::Daw::StudioOne };
            let mut done = false;
            with_picked(|p, r| {
                let save = aimed(r);
                // A song made in that DAW opens as it is: there's nothing to copy it into.
                if !makes_copy_for(&save.daw, daw) {
                    return;
                }
                open_as_copy(p, save, daw);
                done = true;
            });
            return done;
        }
        "projects.reveal" => data.invoke_reveal(),
        "projects.copyPath" => data.invoke_copy_path(),
        "projects.favourite" => give(app, None, "favourite", ""),
        // The card's tag box takes the keys.
        "projects.tag" => data.set_tag_asked(data.get_tag_asked().wrapping_add(1)),
        _ => return false,
    }
    true
}

/// Does `f` with the page and the picked song's row, when a song is picked.
/// Runs `f` on one save of the picked song, found by its path.
fn with_save(path: &str, f: impl FnOnce(&Page, &mo_library::Save)) {
    with_picked(|p, r| {
        if let Some(s) = r.song.saves().find(|s| s.path == path) {
            f(p, s);
        }
    });
}

/// Does `what` to the save at `path`: "open" (in its own DAW), "bitwig" (an FL Studio save in Bitwig), "reveal" or
/// "copy" (its path), from its right-click menu or the buttons on its line.
fn do_to_save(path: &str, what: &str) {
    with_save(path, |p, s| match what {
        "open" => open_save(s),
        "bitwig" => open_in_bitwig(p, s),
        "fl" => open_as_copy(p, s, daw_copy::Daw::FlStudio),
        "studio-one" => open_as_copy(p, s, daw_copy::Daw::StudioOne),
        "reveal" => reveal(s),
        "copy" => copy_path(&s.path),
        _ => {}
    });
}

/// Whether a save made in `daw` gets an "Open a copy in…" line for `copy` (not for its own DAW; an FL Studio save
/// goes to Bitwig as it is, through Open in Bitwig).
fn makes_copy_for(daw: &str, copy: daw_copy::Daw) -> bool {
    daw != copy.name()
}

/// The lines of a save's right-click menu: open it in its own DAW (what a double-click does), FL Studio saves also
/// in Bitwig, a copy for FL Studio and for Studio One (but not for its own DAW), then Show in folder and Copy path.
/// Right-clicking a save picks it, so each line shows the key that does the same to it (rule 15).
fn save_menu(daw: &str, keys: &Shortcuts) -> Vec<MenuItem> {
    let item = |what: String, key: &str, id: &str| MenuItem {
        what: what.into(),
        key: key.into(),
        id: id.into(),
        enabled: true,
    };
    let mut items = vec![item(format!("Open in {daw}"), "Double-click", "open")];
    if daw == FL {
        items.push(item("Open in Bitwig".into(), keys.key_of("projects.openInBitwig"), "bitwig"));
    }
    // A copy of the song made for that DAW, in the export folder: the song itself is only read.
    if makes_copy_for(daw, daw_copy::Daw::FlStudio) {
        items.push(item("Open a copy in FL Studio".into(), keys.key_of("projects.copyFl"), "fl"));
    }
    if makes_copy_for(daw, daw_copy::Daw::StudioOne) {
        items.push(item("Open a copy in Studio One".into(), keys.key_of("projects.copyStudioOne"), "studio-one"));
    }
    items.push(item(String::new(), "", ""));
    items.push(item("Show in folder".into(), keys.key_of("projects.reveal"), "reveal"));
    items.push(item("Copy path".into(), keys.key_of("projects.copyPath"), "copy"));
    items
}

/// The lines of a song's right-click menu: the panel's buttons, each with its key from `keys` (rule 15; review X3-24).
/// Open, as Ctrl+O does (an FL Studio song goes to Bitwig), an FL Studio song also in FL Studio itself, then the
/// card's heart and tag box (Favourite, or not; Add a tag…), then Show in folder and Copy path. Each line's id is its
/// action's, so choosing it runs what its key does ([`run_action`]).
fn song_menu(daw: &str, keys: &Shortcuts) -> Vec<MenuItem> {
    let item = |what: String, id: &str| MenuItem {
        what: what.into(),
        key: keys.key_of(id).into(),
        id: id.into(),
        enabled: true,
    };
    let mut items = if daw == FL {
        vec![item(format!("Open in {BITWIG}"), "projects.open"), item(format!("Open in {FL}"), "projects.openOwn")]
    } else {
        vec![item(format!("Open in {daw}"), "projects.open")]
    };
    if makes_copy_for(daw, daw_copy::Daw::FlStudio) {
        items.push(item("Open a copy in FL Studio".into(), "projects.copyFl"));
    }
    if makes_copy_for(daw, daw_copy::Daw::StudioOne) {
        items.push(item("Open a copy in Studio One".into(), "projects.copyStudioOne"));
    }
    // The card's heart and tag box, as the Sounds menu has them: Add a tag gives the box the keys.
    items.push(item("Favourite, or not".into(), "projects.favourite"));
    items.push(item("Add a tag…".into(), "projects.tag"));
    items.push(MenuItem { enabled: true, ..MenuItem::default() });
    items.push(item("Show in folder".into(), "projects.reveal"));
    items.push(item("Copy path".into(), "projects.copyPath"));
    items
}

/// Esc on the page (`app.escape`, in the search box or anywhere else): the same order as on every page, so with nothing
/// playing here it clears the search first, then puts the panel away (the whole-app review of 6 Nov, X3-12). Says
/// whether it did either (when not, the key goes on).
pub fn escape(app: &AppWindow) -> bool {
    let data = app.global::<SongsData>();
    if !data.get_search().is_empty() {
        data.set_search(SharedString::new());
        data.invoke_search_edited(SharedString::new());
        return true;
    }
    if has_pick() {
        pick(app, None);
        return true;
    }
    false
}

/// Says which save of the picked song the song's buttons act on: one picked on the Saves tab (its path and name), or
/// none, the newest.
fn show_picked_save(app: &AppWindow, save: Option<(&str, &str)>) {
    let (path, name) = save.unwrap_or_default();
    PICKED_SAVE.with(|s| *s.borrow_mut() = path.to_owned());
    let data = app.global::<SongsData>();
    data.set_picked_save(path.into());
    data.set_picked_save_name(name.into());
}

/// The save the song's buttons and keys act on: the one picked on the Saves tab while it's still one of this song's
/// saves, else the newest.
fn aimed(r: &SongRow) -> &mo_library::Save {
    PICKED_SAVE
        .with(|s| {
            let path = s.borrow();
            if path.is_empty() { None } else { r.song.saves().find(|s| s.path == *path) }
        })
        .unwrap_or(&r.song.latest)
}

fn with_picked(f: impl FnOnce(&Page, &SongRow)) {
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some(r) = p.picked.and_then(|i| p.songs.rows.get(i)) {
            f(&p, r);
        }
    });
}

/// The backup changed (a run ended, or another drive was picked): the picked song's "Backed up" line is put right.
pub fn backup_changed(app: &AppWindow) {
    let data = app.global::<SongsData>();
    with_picked(|_, r| {
        // Only when the line changed: the rest of the panel is left as it is.
        let song = panel_song(r);
        if song.backed != data.get_song().backed {
            data.set_song(song);
        }
    });
}

/// Whether a song is picked, for the page's keys.
pub fn has_pick() -> bool {
    PAGE.with(|p| p.borrow().picked.is_some())
}

/// What Open says when the save isn't where the library last saw it: the list catches up by itself (the folder watch,
/// or the walk a little after the start, marks it missing or finds where it went), as there's no Rescan (review A-6).
const MOVED_OR_DELETED: &str =
    "It has moved or been deleted. The list catches up by itself when it next looks in that folder.";

/// Opens a save in its DAW: the program chosen for it on Settings › DAWs (as it stands at the click), else whatever
/// opens its kind of file. Off the window's thread: handing a file to a DAW that's already running can keep the system
/// busy for seconds. The save's "last opened" isn't noted yet.
fn open_save(save: &mo_library::Save) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let chosen = crate::settings::value(&shell::daw_exe_key(&save.daw));
    let (path, daw, name) = (save.path.clone(), save.daw.clone(), save.name.clone());
    let started = background::spawn("Open", move || {
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "").failed(MOVED_OR_DELETED);
            return;
        }
        let exe = chosen.as_deref().filter(|e| shell::usable_exe(e));
        let job = feed.start(&format!("Opening {name}"), "");
        match shell::run(&shell::open(&path, exe, shell::Os::THIS)) {
            Ok(()) => job.done(&shell::sent_to(&daw, chosen.as_deref(), exe.is_some(), shell::Os::THIS)),
            Err(e) => {
                log::warning("Open", &format!("{path}: {e}"));
                job.failed(&format!("Couldn't open it in {daw}: {e}"));
            }
        }
    });
    if let Err(e) = started {
        log::error("Open", &format!("Couldn't start opening a save: {e}"));
    }
}

/// The library's names for the two DAWs Open in Bitwig is about. Bitwig's saves are named "Bitwig Studio" (as the
/// program chosen for it on Settings › DAWs is kept, `daw_exe:Bitwig Studio`); `BITWIG` is what every one of them
/// starts with.
const FL: &str = "FL Studio";
const BITWIG: &str = "Bitwig";
const BITWIG_STUDIO: &str = mo_formats::bitwig::reader::DAW;

/// The names converted and opened in Bitwig while the app runs: Bitwig only brings an open project forward, so a new
/// convert of the same song takes the next name.
static OPENED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// The saves being converted for Bitwig just now.
static CONVERTING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// A save's convert while it runs: it goes from [`CONVERTING`] when this is dropped, however the convert ends.
struct Converting(String);

impl Converting {
    /// None when `path` is already being converted.
    fn start(path: &str) -> Option<Converting> {
        let mut converting = CONVERTING.lock().ok()?;
        converting.insert(path.to_owned()).then(|| Converting(path.to_owned()))
    }
}

impl Drop for Converting {
    fn drop(&mut self) {
        if let Ok(mut converting) = CONVERTING.lock() {
            converting.remove(&self.0);
        }
    }
}

/// The main Open button: FL Studio songs go to Bitwig (as in the C# app), the rest open in their own DAW.
fn open_main(page: &Page, save: &mo_library::Save) {
    if save.daw == FL { open_in_bitwig(page, save) } else { open_save(save) }
}

/// Writes a copy of a save for `daw` into the export folder and opens it there, on a background thread; the card says
/// how it went and what didn't come across. The save is only read, and the copy never goes over a file
/// ([`daw_copy::copy_for`]).
fn open_as_copy(page: &Page, save: &mo_library::Save, daw: daw_copy::Daw) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let chosen = crate::settings::value(&shell::daw_exe_key(daw.name()));
    let (db, path, name) = (page.db.clone(), save.path.clone(), save.name.clone());
    // A second press while the first is still writing would make a second copy: the first is the one that counts. A
    // copy for another DAW is its own.
    let Some(converting) = Converting::start(&format!("{path}\n{}", daw.name())) else { return };
    let started = background::spawn("Copy for DAW", move || {
        let _converting = converting;
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "").failed(MOVED_OR_DELETED);
            return;
        }
        let job = feed.start(&format!("Making a copy of {name} for {}", daw.name()), "");
        let root = Library::open_read_only(&db)
            .map_err(|e| format!("Couldn't read the library: {e}"))
            .and_then(|lib| {
                lib.setting(bitwig::EXPORT_FOLDER_KEY).map_err(|e| format!("Couldn't read the settings: {e}"))
            })
            .and_then(|setting| bitwig::export_folder(setting.as_deref(), bitwig::documents().as_deref()));
        let locator = sound_library::locator(Some(&sound_library::database_beside(&db)));
        let made = root.and_then(|root| {
            background::on_pool(move || {
                let find = |sample: &str| locator.locate_to_use(sample, &path);
                // A damaged song can make a reader give up: that's one warning, and the card says so.
                mo_catch::catch(std::panic::AssertUnwindSafe(|| {
                    daw_copy::copy_for(Path::new(&path), &root, daw, &find)
                }))
                .unwrap_or_else(|_| Err("The song couldn't be read well enough to copy.".into()))
            })
        });
        match made {
            Ok(copy) => {
                log::info("Copy for DAW", &format!("{name} became {}", copy.file.display()));
                let exe = chosen.as_deref().filter(|e| shell::usable_exe(e));
                let file = copy.file.to_string_lossy().into_owned();
                match shell::run(&shell::open(&file, exe, shell::Os::THIS)) {
                    Ok(()) => job.done(&copy.message),
                    Err(e) => {
                        log::warning("Copy for DAW", &format!("{file}: {e}"));
                        job.failed(&format!("{} It couldn't be opened in {}: {e}", copy.message, daw.name()));
                    }
                }
            }
            Err(e) => {
                log::warning("Copy for DAW", &format!("{name}: {e}"));
                job.failed(&format!("Couldn't make a copy for {}: {e}", daw.name()));
            }
        }
    });
    if let Err(e) = started {
        log::error("Copy for DAW", &format!("Couldn't start making a copy: {e}"));
    }
}

/// Converts an FL Studio save for Bitwig into the export folder and opens the new project in Bitwig, on a background
/// thread; the card says how it went. The FL project and the template are only read.
fn open_in_bitwig(page: &Page, save: &mo_library::Save) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    // The Bitwig program chosen on Settings › DAWs, kept under the library's own name for Bitwig.
    let chosen = crate::settings::value(&shell::daw_exe_key(BITWIG_STUDIO));
    let (db, path, name) = (page.db.clone(), save.path.clone(), save.name.clone());
    // A second press while the first still converts would pick the same name: the first is the one that counts.
    let Some(converting) = Converting::start(&path) else { return };
    let started = background::spawn("Convert", move || {
        let _converting = converting;
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "").failed(MOVED_OR_DELETED);
            return;
        }
        let job = feed.start(&format!("Converting {name} for Bitwig"), "");
        let started = Instant::now();
        match convert_save(&db, Path::new(&path)) {
            Ok(c) => {
                log::info("Convert", &format!("{path} became {} in {:.1?}", c.file.display(), started.elapsed()));
                let exe = chosen.as_deref().filter(|e| shell::usable_exe(e));
                let file = c.file.to_string_lossy().into_owned();
                match shell::run(&shell::open(&file, exe, shell::Os::THIS)) {
                    Ok(()) => {
                        if let Ok(mut opened) = OPENED.lock() {
                            opened.insert(c.name.clone());
                        }
                        match shell::sent_with(chosen.as_deref(), exe.is_some(), shell::Os::THIS) {
                            Some(with) => job.done(&format!("{} {with}", c.message)),
                            None => job.done(&c.message),
                        }
                        // After the card says so: a busy library never holds the result up.
                        remember_opened(&db, &c.file);
                    }
                    Err(e) => {
                        log::warning("Convert", &format!("{file}: {e}"));
                        job.failed(&format!("Converted to {file}, but couldn't open it in Bitwig: {e}"));
                    }
                }
                // Remembered once Bitwig has been asked to open it, so a library busy with another change never holds
                // the opening up; the copy is written, so it's remembered either way.
                remember_convert(&db, &c);
            }
            Err(e) => {
                log::warning("Convert", &format!("{path}: {e}"));
                // No screen picks a template yet, so the words say where the app looks for it.
                let more = if e.starts_with("The Bitwig template") {
                    " Save a working template over \"Music Organiser template.bwproject\" in the \"Bitwig \
                     template\" folder of the app's data folder."
                } else {
                    ""
                };
                job.failed(&format!("Converting for Bitwig failed: {e}{more}"));
            }
        }
    });
    if let Err(e) = started {
        log::error("Convert", &format!("Couldn't start converting a save: {e}"));
    }
}

/// Remembers a convert in the song library while it's the app's own songs.db (library.db is never written), so the next
/// convert of the same FL project, in this run or a later one, may save over this copy while nothing has changed it
/// rather than keeping it and taking the next name (review X1-5). Not being able to only costs that: the log says why.
/// Reads and writes the disk: never on the window's thread.
fn remember_convert(db: &Path, c: &Converted) {
    if let Err(e) = mo_library::converts::remember(db, &c.file, &c.written) {
        log::warning("Convert", &format!("Couldn't remember {} in the library: {e}", c.file.display()));
    }
}

/// Remembers in the app's own songs.db that Bitwig was asked to open a convert, so for a day a new convert of the same
/// song takes the next name even after the app restarts: Bitwig may still show this one, and only brings an open
/// project forward (review CN-11). Not being able to only costs that: the log says why. Writes the disk: never on the
/// window's thread.
fn remember_opened(db: &Path, file: &Path) {
    if let Err(e) = mo_library::converts::remember_opened(db, file, seconds_now()) {
        log::warning("Convert", &format!("Couldn't remember that {} was opened: {e}", file.display()));
    }
}

/// Seconds since 1970, as the opened records count them.
fn seconds_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Reads the settings (export folder, template, how steps play) and converts `project` on the low-priority pool.
/// What was written is remembered after, in songs.db ([`remember_convert`]).
fn convert_save(db: &Path, project: &Path) -> Result<Converted, String> {
    let lib = Library::open_read_only(db).map_err(|e| format!("Couldn't read the library: {e}"))?;
    let setting = |key: &str| lib.setting(key).map_err(|e| format!("Couldn't read the settings: {e}"));
    let root = bitwig::export_folder(setting(bitwig::EXPORT_FOLDER_KEY)?.as_deref(), bitwig::documents().as_deref())?;
    let template = bitwig::template_path(&paths::data_dir(), setting(bitwig::BITWIG_TEMPLATE_KEY)?.as_deref())
        .ok_or(bitwig::NO_TEMPLATE)?;
    let steps = setting(bitwig::STEPS_UNTIL_NEXT_NOTE_KEY)?.as_deref() == Some("1");
    let mut opened = OPENED.lock().map(|o| o.clone()).unwrap_or_default();
    // And this song's copies Bitwig was asked to open in the last day, by this run of the app or an earlier one:
    // Bitwig may still show them (review CN-11). Not being able to read them only means the names this run opened
    // count, and the log says so.
    match mo_library::converts::opened_lately(&lib, seconds_now()) {
        Ok(lately) => opened.extend(bitwig::opened_names(project, &root, &lately)),
        Err(e) => log::warning("Convert", &format!("Couldn't read which converts were opened lately: {e}")),
    }
    let locator = sound_library::locator(Some(&sound_library::database_beside(db)));
    let at = project.to_string_lossy().into_owned();
    background::on_pool(move || {
        let written_of = |file: &Path| lib.setting(&bitwig::convert_key(file)).ok().flatten();
        // FL Studio's own sounds only from FL Studio's folder, never another file with the name.
        let find = |sample: &str| locator.locate_to_use(sample, &at);
        bitwig::convert(project, &root, &template, steps, &opened, &written_of, &find)
    })
}

/// Shows the file picked in its folder, or the folder when the file has gone.
fn reveal(save: &mo_library::Save) {
    if let Some(feed) = FEED.with(|f| f.borrow().clone()) {
        crate::files::reveal(&feed, &save.path, &save.folder);
    }
}

/// Puts the file's path on the clipboard.
fn copy_path(path: &str) {
    if let Some(feed) = FEED.with(|f| f.borrow().clone()) {
        crate::files::copy_path(&feed, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UX review A2: each click on the card is one change the library takes, and a word the card never sends is none.
    #[test]
    fn a_click_on_the_card_is_one_change_to_the_song() {
        assert_eq!(asked_change("status", "Mixing", false, &[], 0), Some(Change::Status("Mixing".to_owned())));
        assert_eq!(asked_change("stars", "4", false, &[], 0), Some(Change::Rating(4)));
        assert_eq!(asked_change("stars", "lots", false, &[], 0), None);
        // The heart turns over what the song has now.
        assert_eq!(asked_change("favourite", "", false, &[], 0), Some(Change::Favourite(true)));
        assert_eq!(asked_change("favourite", "", true, &[], 0), Some(Change::Favourite(false)));
        assert_eq!(
            asked_change("add-tag", " Dark ", false, &[], 0).and_then(|c| c.checked().ok()),
            Some(Change::AddTag("dark".to_owned()))
        );
        assert_eq!(asked_change("remove-tag", "dark", false, &[], 0), Some(Change::RemoveTag("dark".to_owned())));
        assert_eq!(asked_change("key", "F# minor", false, &[], 0), None);
        // Every status the card's menu offers is one the library keeps.
        for status in STATUSES {
            assert!(asked_change("status", status, false, &[], 0).is_some_and(|c| c.checked().is_ok()), "{status}");
        }
        // A to-do is added, or ticked over or taken off by its place (named by its words and when it was added); one
        // that isn't there, or spaces alone, is no change.
        let one = Todo { text: "Redo the bass".into(), done: false, added: 5 };
        let todos = [one.clone()];
        let ask = |what: &str, value: &str| asked_change(what, value, false, &todos, 9);
        let added = Todo { text: "Vocal chop".into(), done: false, added: 9 };
        assert_eq!(ask("add-todo", "Vocal chop"), Some(Change::AddTodo(added)));
        assert_eq!(ask("add-todo", "   "), None);
        let named = (one.text.clone(), one.added);
        assert_eq!(ask("tick-todo", "0"), Some(Change::TickTodo { text: named.0.clone(), added: 5, done: true }));
        assert_eq!(ask("remove-todo", "0"), Some(Change::RemoveTodo { text: named.0, added: named.1 }));
        assert_eq!(ask("tick-todo", "1"), None);
        assert_eq!(ask("remove-todo", "x"), None);
        // Six stars, or a tag with no name, is refused before anything shows.
        assert!(Change::Rating(6).checked().is_err());
        assert!(Change::AddTag("  ".to_owned()).checked().is_err());
    }

    /// Review SG-11: what you gave songs on their cards goes before the app closing, and that before folders to read,
    /// whatever order they came in; a give that came while the thread was busy never waits behind a read.
    #[test]
    fn what_you_give_songs_goes_first_then_the_app_closing_then_folders_read() {
        let (tx, rx) = mpsc::channel();
        let card =
            |song: &str| Told::Give(Given { song: song.to_owned(), save_ids: vec![1], change: Change::Rating(1) });
        let (done, _finished) = mpsc::channel();
        tx.send(Told::Changed(Changes::default())).unwrap();
        tx.send(card("a")).unwrap();
        tx.send(Told::Finish(done)).unwrap();
        tx.send(card("b")).unwrap();
        let mut waiting = VecDeque::new();
        let mut order = Vec::new();
        while let Ok(told) = next(&rx, &mut waiting, Duration::ZERO) {
            order.push(match told {
                Told::Give(given) => given.song,
                Told::PutOff(key) | Told::Bounces(key, _) => key,
                Told::Finish(_) => "closing".to_owned(),
                Told::PutAway(..) => "put away".to_owned(),
                Told::Changed(_) | Told::AllBounces(_) | Told::Shapes(_) | Told::ShapesDone => "folders".to_owned(),
            });
        }
        assert_eq!(order, ["a", "b", "closing", "folders"]);
        assert!(waiting.is_empty());
        // What the log says of changes the app closed before it kept.
        assert!(not_kept_line(1).starts_with("The app closed before 1 change given on song cards was kept"));
        assert!(not_kept_line(2).contains("2 changes given on song cards were kept"));
    }

    /// Review SS-3: as the app closes, the page's thread is told to finish, and its reading of your project folders is
    /// stopped part way, so what you gave songs on their cards can't wait behind it.
    #[test]
    fn closing_stops_the_page_reading_your_project_folders() {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        *TELL.lock().unwrap_or_else(PoisonError::into_inner) = Some((tx, stop.clone()));
        finish_up();
        assert!(stop.load(Ordering::SeqCst));
        assert!(matches!(rx.try_recv(), Ok(Told::Finish(_))));
        assert!(TELL.lock().unwrap_or_else(PoisonError::into_inner).is_none());
    }

    #[test]
    fn an_fl_save_s_menu_also_opens_it_in_bitwig_and_every_line_shows_its_key() {
        let keys = Shortcuts::default();
        let ids = |daw: &str| save_menu(daw, &keys).iter().map(|i| i.id.to_string()).collect::<Vec<_>>();
        assert_eq!(ids(FL), ["open", "bitwig", "studio-one", "", "reveal", "copy"]);
        assert_eq!(ids("Bitwig Studio"), ["open", "fl", "studio-one", "", "reveal", "copy"]);
        assert_eq!(ids("Studio One"), ["open", "fl", "", "reveal", "copy"]);
        assert_eq!(save_menu("Ableton Live", &keys)[1].what, "Open a copy in FL Studio");
        assert_eq!(save_menu(FL, &keys)[0].what, "Open in FL Studio");
        // Rule 15: every line but the gap shows its key.
        let shown: Vec<_> = save_menu(FL, &keys).iter().map(|i| i.key.to_string()).collect();
        assert_eq!(shown, ["Double-click", "Ctrl+B", "Alt+S", "", "Ctrl+Shift+E", "Ctrl+Shift+C"]);
        assert_eq!(save_menu("Bitwig Studio", &keys)[1].key, "Alt+F");
    }

    /// Review X3-24: a song line's menu has the panel's buttons, each line with its key (rule 15), and each id an
    /// action the page's keys run.
    #[test]
    fn a_song_s_menu_has_the_panel_s_buttons_with_their_keys() {
        let lines = |daw: &str, keys: &Shortcuts| {
            song_menu(daw, keys).iter().map(|i| (i.what.to_string(), i.key.to_string(), i.id.to_string())).collect()
        };
        let line = |what: &str, key: &str, id: &str| (what.to_owned(), key.to_owned(), id.to_owned());
        let keys = Shortcuts::default();
        let fl: Vec<_> = lines(FL, &keys);
        assert_eq!(
            fl,
            [
                line("Open in Bitwig", "Ctrl+O", "projects.open"),
                line("Open in FL Studio", "Ctrl+Shift+O", "projects.openOwn"),
                line("Open a copy in Studio One", "Alt+S", "projects.copyStudioOne"),
                line("Favourite, or not", "Ctrl+D", "projects.favourite"),
                line("Add a tag…", "Ctrl+T", "projects.tag"),
                line("", "", ""),
                line("Show in folder", "Ctrl+Shift+E", "projects.reveal"),
                line("Copy path", "Ctrl+Shift+C", "projects.copyPath"),
            ]
        );
        let bitwig: Vec<_> = lines(BITWIG_STUDIO, &keys);
        assert_eq!(bitwig[0], line("Open in Bitwig Studio", "Ctrl+O", "projects.open"));
        assert_eq!(bitwig[1], line("Open a copy in FL Studio", "Alt+F", "projects.copyFl"));
        assert_eq!(bitwig[2], line("Open a copy in Studio One", "Alt+S", "projects.copyStudioOne"));
        assert_eq!(bitwig.len(), 8);
        // Every line runs a Songs action, so its key and its line do the same thing.
        for (_, _, id) in fl.iter().filter(|(_, _, id)| !id.is_empty()) {
            assert!(mo_app::shortcuts::SONGS_ACTIONS.iter().any(|a| a.id == id), "{id}");
        }
        // A key changed in Settings shows on its line.
        let changed = Shortcuts::from_setting(Some(r#"{"projects.reveal":"Ctrl+R"}"#));
        assert_eq!(lines(BITWIG_STUDIO, &changed)[6], line("Show in folder", "Ctrl+R", "projects.reveal"));
    }

    #[test]
    fn the_library_is_read_ahead_of_the_window_into_the_page_s_list() {
        let dir = std::env::temp_dir().join(format!("mo-ui-songs-ahead-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("made");
        let library = dir.join("library.db");
        rusqlite::Connection::open(&library)
            .expect("opened")
            .execute_batch(
                "CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
                 INSERT INTO projects VALUES (1, 'C:\\Music\\Night\\Night_1.flp', 'Night_1', 'FL Studio',
                     'C:\\Music\\Night', 10, 1700000000, '');
                 INSERT INTO projects VALUES (2, 'C:\\Music\\Night\\Night_2.flp', 'Night_2', 'FL Studio',
                     'C:\\Music\\Night', 10, 1700003600, '');
                 INSERT INTO projects VALUES (3, 'C:\\Music\\Day\\Day.flp', 'Day', 'FL Studio',
                     'C:\\Music\\Day', 10, 1600000000, '');",
            )
            .expect("filled");
        // No window, no event loop: the read needs neither, so it can start before them.
        // Even when told it's the app's own, library.db is never written.
        let loaded = load(&library, LiveDetails::default(), true).expect("read");
        assert_eq!(loaded.songs.saves, 3);
        let names: Vec<&str> = loaded.songs.rows.iter().map(|r| r.song.name.as_str()).collect();
        assert_eq!(names.len(), 2, "the two Night saves are one song: {names:?}");
        assert_eq!(loaded.years.iter().map(|y| y.1).sum::<usize>(), 2, "each song counted in its year");
        assert!(!loaded.live.writes(), "the C# app's library.db is never written");
        // So a change given on a card is shown, but held only until the app closes, and a card says so (review
        // SG-2).
        let (mut live, mut gave) = (loaded.live, Gave::default());
        gave.keep(&mut live, Given { song: "a song".to_owned(), save_ids: vec![1], change: Change::Rating(3) });
        assert!(gave.any && gave.held && gave.not_kept.is_empty());
        // A library that can't be read says why, for the page to show; nothing crashes.
        assert!(load(&dir.join("gone").join("library.db"), LiveDetails::default(), false).is_err());
        // Started with no window and no event loop: the read needs neither. Never told a window (it couldn't be
        // made), its thread ends once the read is done.
        let ahead = read_ahead(library.clone(), false);
        assert_eq!(ahead.path, library);
        assert!(ahead.window.is_some(), "started");
        drop(ahead);
        // A songs.db is kept in only when the app found it its own as it started; the page doesn't ask again (review
        // SG-2).
        let songs = dir.join(mo_library::songs_db::FILE_NAME);
        mo_library::songs_db::make(&library, &songs, &mut |_| {}).expect("made");
        assert!(load(&songs, LiveDetails::default(), true).expect("read").live.writes());
        assert!(!load(&songs, LiveDetails::default(), false).expect("read").live.writes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_program_the_c_sharp_app_chose_is_the_one_open_reads() {
        // The C# app kept each DAW's program in its settings, which songs.db took over. Open reads it as it stands at
        // the click (`settings::value`), so Settings › DAWs shows and changes the one Open uses (review SS-4).
        let fl = r"C:\Program Files\Image-Line\FL Studio 2025\FL64.exe";
        let dir = std::env::temp_dir().join(format!("mo-ui-songs-program-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("made");
        let library = dir.join(mo_library::songs_db::FILE_NAME);
        let c = rusqlite::Connection::open(&library).expect("opened");
        c.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
                 daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0);",
        )
        .expect("made");
        c.execute("INSERT INTO settings (key, value) VALUES ('daw_exe:FL Studio', ?1)", [fl]).expect("kept");
        drop(c);
        let (settings, _) = crate::settings::load(&dir.join("no-settings-folder"), &library);
        let program = |daw: &str| crate::settings::value(&shell::daw_exe_key(daw));
        assert_eq!(settings.get("daw_exe:FL Studio"), Some(fl));
        assert_eq!(program(mo_formats::fl::DAW).as_deref(), Some(fl));
        assert_eq!(program(BITWIG_STUDIO), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_in_bitwig_looks_up_the_program_by_the_library_s_name_for_bitwig() {
        // Settings keep the program as `daw_exe:Bitwig Studio`, the name Bitwig's saves carry in the library.
        assert_eq!(BITWIG_STUDIO, "Bitwig Studio");
        assert!(BITWIG_STUDIO.starts_with(BITWIG));
    }
}
