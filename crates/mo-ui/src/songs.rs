//! Fills the Songs page from mo-app's song list and answers its search box and chips. The library is read on a
//! background thread (rule 12); searching and filtering then walk the rows already made, and a line's text is only
//! made when the list draws that line. Picking a song opens its panel at once with what the list knows; the newest
//! save's plugins, samples and details follow from a background thread that only reads the last song picked.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::bitwig_convert::{self as bitwig, Converted};
use mo_app::finder::{self, Kind};
use mo_app::live::{Live, LiveDetails, Updated};
use mo_app::log;
use mo_app::places::Place;
use mo_app::shell;
use mo_app::shortcuts::Shortcuts;
use mo_app::song_panel::{self as panel, Go, SaveLine as PanelSaveLine, Tab};
use mo_app::songs::{Entry, Filters, Listed, Order, SongRow, Songs, View};
use mo_app::{paths, sound_library, year_strip};
use mo_library::scan::is_project_file;
use mo_library::songs::DEFAULT_RUN_GAP_MONTHS;
use mo_library::watch::{Changes, FolderWatch};
use mo_library::{Library, STATUSES, SaveDetail};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::filters::FilterModels;
use crate::finder::How;
use crate::{
    AppWindow, CountLine, DetailPair, HealthLine, MenuItem, OlderLine, PanelSong, PluginLine, SampleLine, SaveLine,
    SongLine, SongsData, YearBar,
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
}

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
        }
    }
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
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
pub fn read_ahead(path: PathBuf) -> Ahead {
    let details = LiveDetails::default();
    let window = match start(path.clone(), details.clone()) {
        Ok(window) => Some(window),
        Err(e) => {
            log::warning("Songs", &format!("Couldn't start reading the library ahead of the window: {e}"));
            None
        }
    };
    Ahead { path, details, window }
}

/// Starts the page's thread: it reads the library at `path` at once ([`load`]), waits to be told its window, puts
/// the list there and then keeps it up to date ([`watch`]). A read that fails is logged as it fails, and shown once
/// there's a window. If it's never told a window (the window couldn't be made), it ends.
fn start(path: PathBuf, details: LiveDetails) -> std::io::Result<mpsc::Sender<slint::Weak<AppWindow>>> {
    let (window, told) = mpsc::channel::<slint::Weak<AppWindow>>();
    background::spawn("Songs", move || {
        let loaded = load(&path, details);
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
        let _ = weak.upgrade_in_event_loop(move |app| {
            show(&app, songs, years);
            crate::finder::add_source(&app, "songs", found);
        });
        watch(&mut live, &weak);
    })?;
    Ok(window)
}

/// Reads the library at `path` and makes the page's list from it. Why not, when it can't be read.
fn load(path: &Path, details: LiveDetails) -> Result<Loaded, String> {
    let began = Instant::now();
    let read = Library::open_read_only(path)
        .and_then(|lib| Live::load(&lib, DEFAULT_RUN_GAP_MONTHS, details))
        // Missing samples are looked for in the Sounds library beside it last (rule 13).
        .map(|live| live.with_sounds(sound_library::database_beside(path)))
        // What's read is kept in the app's own songs.db; library.db is never written.
        .map(|live| match mo_library::songs_db::own_since(path) {
            Some(_) => live.with_writes(path.to_owned()),
            None => live,
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
    let Ahead { path, details, window } = ahead;
    let data = app.global::<SongsData>();
    FEED.with(|f| *f.borrow_mut() = Some(feed));
    PAGE.with(|p| p.borrow_mut().db = path.clone());
    data.on_open(|| with_picked(|p, r| open_main(p, aimed(r))));
    data.on_open_own(|| with_picked(|p, r| open_save(p, aimed(r))));
    data.on_open_save(|path| with_save(&path, open_save));
    let weak = app.as_weak();
    data.on_save_menu_for(move |path| {
        let Some(app) = weak.upgrade() else { return };
        let mut items = Vec::new();
        with_save(&path, |_, s| items = save_menu(&s.daw));
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
    match window.map_or_else(|| start(path, details), Ok) {
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
fn watch(live: &mut Live, weak: &slint::Weak<AppWindow>) {
    let (tx, rx) = mpsc::channel::<Changes>();
    let to_watch = tx.clone();
    let watching = FolderWatch::start(live.roots(), is_project_file, WATCH_QUIET, move |c| {
        let _ = to_watch.send(c);
    });
    // Held so the loop below waits rather than ends if the watch can't start: the catch-up and the retries still run.
    let _still_here = tx;
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
    loop {
        let now = Instant::now();
        let due = |at: Option<Instant>| at.map_or(RETRY_AFTER, |at| at.saturating_duration_since(now));
        // A save that couldn't be read (still being written, or locked) is tried again after a quiet moment.
        let wait = due(catch_up_at).min(due(keep_again_at)).min(RETRY_AFTER);
        let changes = match rx.recv_timeout(wait) {
            Ok(changes) => Some(changes),
            Err(mpsc::RecvTimeoutError::Timeout) => live.retries(),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut updated = Updated::default();
        if let Some(changes) = &changes {
            updated = live.update(changes);
            if !updated.is_empty() {
                log::info(
                    "Watch",
                    &format!(
                        "Project folders: {} read, {} moved, {} gone{} ({} changes seen)",
                        updated.read.len(),
                        updated.moved.len(),
                        updated.gone,
                        mended_line(updated.mended),
                        changes.files.len()
                    ),
                );
            }
        }
        // Looked at after every batch too, so a folder that keeps changing (a DAW saving every few seconds) can't put
        // it off.
        if catch_up_at.is_some_and(|at| Instant::now() >= at) {
            catch_up_at = None;
            let began = Instant::now();
            let caught = live.catch_up();
            log::info(
                "Songs",
                &format!(
                    "Caught up with the project folders in {:.1} s: {} read, {} moved, {} gone{}",
                    began.elapsed().as_secs_f32(),
                    caught.read.len(),
                    caught.moved.len(),
                    caught.gone,
                    mended_line(caught.mended)
                ),
            );
            updated = updated.and(caught);
        }
        // Kept in songs.db with each change, or a while after it couldn't be kept.
        let mut renumbered = false;
        if !updated.is_empty() || live.note_to_keep() || keep_again_at.is_some_and(|at| Instant::now() >= at) {
            keep_again_at = match live.keep() {
                Some(Ok(kept)) => {
                    let w = &kept.written;
                    let noted = if w.mended { ", and that the muted samples are mended" } else { "" };
                    log::info(
                        "Songs",
                        &format!("Kept in songs.db: {} saves read, {} marked missing{noted}", w.put, w.missing),
                    );
                    for (path, why) in &kept.not_kept {
                        log::warning("Songs", &format!("Couldn't keep {path} in songs.db: {why}"));
                    }
                    renumbered = !w.ids.is_empty();
                    None
                }
                Some(Err(e)) => {
                    log::warning("Songs", &format!("{e}; it's kept with the next change, or in a minute"));
                    Some(Instant::now() + KEEP_AGAIN_AFTER)
                }
                None => None,
            };
        }
        if updated.is_empty() && !renumbered {
            continue;
        }
        // Made again when a save's id changed too, so the page picks it by the id the library has.
        let songs = live.songs();
        let found = Arc::new(finder::songs(&songs, Local::now().naive_local()));
        let years = songs.year_counts();
        let _ = weak.upgrade_in_event_loop(move |app| {
            show_again(&app, songs, years);
            crate::finder::add_source(&app, "songs", found);
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

/// The log's words for saves read again only to mend their muted samples, if there were any.
fn mended_line(mended: usize) -> String {
    match mended {
        0 => String::new(),
        n => format!(", {n} read again for their muted samples"),
    }
}

/// Puts a list made again on the page, keeping the filters, the song picked and its tab, with its year strip (`years`,
/// [`Songs::year_counts`], counted where the list was made: not on the window's thread).
fn show_again(app: &AppWindow, songs: Songs, years: Vec<(i32, usize)>) {
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

/// The list's keys and its lines' right-click menu: Up and Down, Esc, and the menu (review X3-8, X3-24). For the
/// screenshots too.
fn wire_list(app: &AppWindow) {
    let data = app.global::<SongsData>();
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
/// ending, in any case, as the C# app matched them), for the rail's "In your DAWs" line: by real dates, so one dated
/// still to come, or with no date, isn't taken for the newest (mo_formats::dates). None while the list is still being
/// read.
pub fn bitwig_save_named(name: &str) -> Option<i64> {
    let wanted = name.trim().to_lowercase();
    let real = mo_formats::dates::RealDates::now();
    PAGE.with(|p| {
        p.borrow()
            .songs
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
        data.set_hide_archived(f.hide_archived);
        data.set_any_filter(f.any());
        data.set_only(f.only.as_ref().map(|o| o.0.as_str()).unwrap_or_default().into());
        if data.get_search().as_str() != f.search {
            data.set_search(f.search.as_str().into());
        }
    });
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
                rating: r.rating,
                favourite: r.favourite,
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

/// Puts the picked song in the panel: what the list knows, then [`fill_detail`].
fn fill_panel(app: &AppWindow) {
    let data = app.global::<SongsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let Some(r) = p.picked.and_then(|i| p.songs.rows.get(i)) else { return };
        data.set_song(panel_song(r));
        data.set_runs(model(r.runs.iter().rev().map(|run| SharedString::from(panel::run_line(run)))));
        data.set_saves_heading(panel::saves_heading(r).into());
        let older = panel::older_saves(r);
        data.set_older_heading(older.as_ref().map_or("", |o| o.heading).into());
        data.set_older_see_all(older.as_ref().map(|o| o.see_all.as_str()).unwrap_or_default().into());
        data.set_older(model(
            older.into_iter().flat_map(|o| o.lines).map(|l| OlderLine { text: l.text.into(), path: l.path.into() }),
        ));
        data.set_saves(model(panel::save_lines(r).into_iter().map(|l| match l {
            PanelSaveLine::Run { label, details } => {
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
    }
}

fn model<T: Clone + 'static>(items: impl Iterator<Item = T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items.collect::<Vec<_>>()))
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
                    open_save(p, save);
                    done = true;
                }
            });
            return done;
        }
        "projects.reveal" => data.invoke_reveal(),
        "projects.copyPath" => data.invoke_copy_path(),
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
        "open" => open_save(p, s),
        "bitwig" => open_in_bitwig(p, s),
        "reveal" => reveal(s),
        "copy" => copy_path(&s.path),
        _ => {}
    });
}

/// The lines of a save's right-click menu: open it in its own DAW (what a double-click does), FL Studio saves also
/// in Bitwig, then Show in folder and Copy path.
fn save_menu(daw: &str) -> Vec<MenuItem> {
    let item = |what: String, key: &str, id: &str| MenuItem {
        what: what.into(),
        key: key.into(),
        id: id.into(),
        enabled: true,
    };
    let mut items = vec![item(format!("Open in {daw}"), "Double-click", "open")];
    if daw == FL {
        items.push(item("Open in Bitwig".into(), "", "bitwig"));
    }
    items.push(item(String::new(), "", ""));
    items.push(item("Show in folder".into(), "", "reveal"));
    items.push(item("Copy path".into(), "", "copy"));
    items
}

/// The lines of a song's right-click menu: the panel's buttons, each with its key from `keys` (rule 15; review X3-24).
/// Open, as Ctrl+O does (an FL Studio song goes to Bitwig), an FL Studio song also in FL Studio itself, then Show in
/// folder and Copy path. Each line's id is its action's, so choosing it runs what its key does ([`run_action`]).
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

/// Opens a save in its DAW: the one picked in Settings, else whatever opens its kind of file. Off the window's
/// thread: handing a file to a DAW that's already running can keep the system busy for seconds. The save's "last
/// opened" isn't noted yet.
fn open_save(page: &Page, save: &mo_library::Save) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let exe = page.songs.daw_exes.get(&save.daw).cloned();
    let (path, daw, name) = (save.path.clone(), save.daw.clone(), save.name.clone());
    let started = background::spawn("Open", move || {
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "").failed(MOVED_OR_DELETED);
            return;
        }
        let exe = exe.filter(|e| shell::usable_exe(e));
        let job = feed.start(&format!("Opening {name}"), "");
        match shell::run(&shell::open(&path, exe.as_deref(), shell::Os::THIS)) {
            Ok(()) => job.done(&format!("Sent to {daw}. It opens there in a moment.")),
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
/// Settings' program for it is kept, `daw_exe:Bitwig Studio`); `BITWIG` is what every one of them starts with.
const FL: &str = "FL Studio";
const BITWIG: &str = "Bitwig";
const BITWIG_STUDIO: &str = mo_formats::bitwig::reader::DAW;

/// The names converted and opened in Bitwig while the app runs: Bitwig only brings an open project forward, so a new
/// convert of the same song takes the next name.
static OPENED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// The main Open button: FL Studio songs go to Bitwig (as in the C# app), the rest open in their own DAW.
fn open_main(page: &Page, save: &mo_library::Save) {
    if save.daw == FL { open_in_bitwig(page, save) } else { open_save(page, save) }
}

/// Converts an FL Studio save for Bitwig into the export folder and opens the new project in Bitwig, on a background
/// thread; the card says how it went. The FL project and the template are only read.
fn open_in_bitwig(page: &Page, save: &mo_library::Save) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    // The Bitwig program picked in Settings, kept under the library's own name for Bitwig.
    let exe = page.songs.daw_exes.get(BITWIG_STUDIO).cloned();
    let (db, path, name) = (page.db.clone(), save.path.clone(), save.name.clone());
    let started = background::spawn("Convert", move || {
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "").failed(MOVED_OR_DELETED);
            return;
        }
        let job = feed.start(&format!("Converting {name} for Bitwig"), "");
        let started = Instant::now();
        match convert_save(&db, Path::new(&path)) {
            Ok(c) => {
                log::info("Convert", &format!("{path} became {} in {:.1?}", c.file.display(), started.elapsed()));
                let exe = exe.filter(|e| shell::usable_exe(e));
                let file = c.file.to_string_lossy().into_owned();
                match shell::run(&shell::open(&file, exe.as_deref(), shell::Os::THIS)) {
                    Ok(()) => {
                        if let Ok(mut opened) = OPENED.lock() {
                            opened.insert(c.name.clone());
                        }
                        job.done(&c.message);
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
                let more =
                    if e.starts_with("The Bitwig template") { " Pick another template in Settings." } else { "" };
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

/// Reads the settings (export folder, template, how steps play) and converts `project` on the low-priority pool.
/// What was written is remembered after, in songs.db ([`remember_convert`]).
fn convert_save(db: &Path, project: &Path) -> Result<Converted, String> {
    let lib = Library::open_read_only(db).map_err(|e| format!("Couldn't read the library: {e}"))?;
    let setting = |key: &str| lib.setting(key).map_err(|e| format!("Couldn't read the settings: {e}"));
    let root = bitwig::export_folder(setting(bitwig::EXPORT_FOLDER_KEY)?.as_deref(), bitwig::documents().as_deref())?;
    let template = bitwig::template_path(&paths::data_dir(), setting(bitwig::BITWIG_TEMPLATE_KEY)?.as_deref())
        .ok_or(bitwig::NO_TEMPLATE)?;
    let steps = setting(bitwig::STEPS_UNTIL_NEXT_NOTE_KEY)?.as_deref() == Some("1");
    let opened = OPENED.lock().map(|o| o.clone()).unwrap_or_default();
    let locator = sound_library::locator(Some(&sound_library::database_beside(db)));
    let at = project.to_string_lossy().into_owned();
    background::on_pool(move || {
        let written_of = |file: &Path| lib.setting(&bitwig::convert_key(file)).ok().flatten();
        let find = |sample: &str| locator.locate(sample, &at);
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

    #[test]
    fn an_fl_save_s_menu_also_opens_it_in_bitwig() {
        let ids = |daw: &str| save_menu(daw).iter().map(|i| i.id.to_string()).collect::<Vec<_>>();
        assert_eq!(ids(FL), ["open", "bitwig", "", "reveal", "copy"]);
        assert_eq!(ids("Bitwig Studio"), ["open", "", "reveal", "copy"]);
        assert_eq!(save_menu(FL)[0].what, "Open in FL Studio");
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
                line("", "", ""),
                line("Show in folder", "Ctrl+Shift+E", "projects.reveal"),
                line("Copy path", "Ctrl+Shift+C", "projects.copyPath"),
            ]
        );
        let bitwig: Vec<_> = lines(BITWIG_STUDIO, &keys);
        assert_eq!(bitwig[0], line("Open in Bitwig Studio", "Ctrl+O", "projects.open"));
        assert_eq!(bitwig.len(), 4);
        // Every line runs a Songs action, so its key and its line do the same thing.
        for (_, _, id) in fl.iter().filter(|(_, _, id)| !id.is_empty()) {
            assert!(mo_app::shortcuts::SONGS_ACTIONS.iter().any(|a| a.id == id), "{id}");
        }
        // A key changed in Settings shows on its line.
        let changed = Shortcuts::from_setting(Some(r#"{"projects.reveal":"Ctrl+R"}"#));
        assert_eq!(lines(BITWIG_STUDIO, &changed)[2], line("Show in folder", "Ctrl+R", "projects.reveal"));
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
        let loaded = load(&library, LiveDetails::default()).expect("read");
        assert_eq!(loaded.songs.saves, 3);
        let names: Vec<&str> = loaded.songs.rows.iter().map(|r| r.song.name.as_str()).collect();
        assert_eq!(names.len(), 2, "the two Night saves are one song: {names:?}");
        assert_eq!(loaded.years.iter().map(|y| y.1).sum::<usize>(), 2, "each song counted in its year");
        assert!(!loaded.live.writes(), "the C# app's library.db is never written");
        // A library that can't be read says why, for the page to show; nothing crashes.
        assert!(load(&dir.join("gone").join("library.db"), LiveDetails::default()).is_err());
        // Started with no window and no event loop: the read needs neither. Never told a window (it couldn't be
        // made), its thread ends once the read is done.
        let ahead = read_ahead(library.clone());
        assert_eq!(ahead.path, library);
        assert!(ahead.window.is_some(), "started");
        drop(ahead);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_in_bitwig_looks_up_the_program_by_the_library_s_name_for_bitwig() {
        // Settings keep the program as `daw_exe:Bitwig Studio`, the name Bitwig's saves carry in the library.
        assert_eq!(BITWIG_STUDIO, "Bitwig Studio");
        assert!(BITWIG_STUDIO.starts_with(BITWIG));
    }
}
