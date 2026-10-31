//! Fills the Songs page from mo-app's song list and answers its search box and chips. The library is read on a
//! background thread (rule 12); searching and filtering then walk the rows already made, and a line's text is only
//! made when the list draws that line. Picking a song opens its panel at once with what the list knows; the newest
//! save's plugins, samples and details follow from a background thread that only reads the last song picked.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::live::{Live, LiveDetails};
use mo_app::log;
use mo_app::shell;
use mo_app::song_panel::{self as panel, Go, SaveLine as PanelSaveLine, Tab};
use mo_app::songs::{Entry, Filters, SongRow, Songs, View};
use mo_library::scan::is_project_file;
use mo_library::songs::DEFAULT_RUN_GAP_MONTHS;
use mo_library::watch::{Changes, FolderWatch};
use mo_library::{Library, STATUSES, SaveDetail};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::{
    AppWindow, CountLine, DetailPair, HealthLine, OlderLine, PanelSong, PluginLine, SampleLine, SaveLine, SongLine,
    SongsData, YearStart,
};

/// The page's rows, filters and what they let through, kept on the window's thread.
struct Page {
    songs: Songs,
    filters: Filters,
    view: View,
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
}

impl Default for Page {
    fn default() -> Page {
        Page {
            songs: Songs::default(),
            filters: Filters::default(),
            view: View::default(),
            now: Local::now().naive_local(),
            picked: None,
            asked: 0,
            detail: None,
            health: Vec::new(),
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
    /// The clipboard, kept open: on Linux what's copied only stays while it is.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
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

/// Wires the page's search box and chips, and starts reading the library at `path` in the background. Open, Show in
/// folder and Copy path report on `feed`'s cards.
pub fn connect(app: &AppWindow, path: PathBuf, feed: Feed) {
    let data = app.global::<SongsData>();
    FEED.with(|f| *f.borrow_mut() = Some(feed));
    data.on_open(|| with_picked(|p, r| open_save(p, &r.song.latest)));
    data.on_open_save(|path| {
        with_picked(|p, r| {
            if let Some(s) = r.song.saves().find(|s| s.path == path.as_str()) {
                open_save(p, s);
            }
        });
    });
    data.on_reveal(|| with_picked(|_, r| reveal(&r.song.latest)));
    data.on_copy_path(|| with_picked(|_, r| copy_path(&r.song.latest.path)));
    data.set_statuses(strings(STATUSES.iter().copied()));
    let weak = app.as_weak();
    data.on_search_edited(move |text| {
        change(&weak, |f| f.search = text.to_string());
    });
    let weak = app.as_weak();
    data.on_pick(move |kind, value| {
        let value = (!value.is_empty()).then(|| value.to_string());
        change(&weak, |f| match kind.as_str() {
            "daw" => f.daw = value,
            "status" => f.status = value,
            "tag" => f.tag = value,
            _ => {}
        });
    });
    let weak = app.as_weak();
    data.on_toggle(move |kind| {
        change(&weak, |f| match kind.as_str() {
            "favourites" => f.favourites = !f.favourites,
            "missing" => f.missing = !f.missing,
            "hide-archived" => f.hide_archived = !f.hide_archived,
            _ => {}
        });
    });
    let weak = app.as_weak();
    data.on_clear(move || {
        change(&weak, |f| *f = Filters { hide_archived: f.hide_archived, ..Filters::default() });
    });
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
    let details = LiveDetails::default();
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
                let read = lib.detail(id).map_err(|e| e.to_string());
                library = Some(lib);
                read
            }
            Err(e) => Err(format!("the library can't be opened ({e})")),
        };
        let _ = weak.upgrade_in_event_loop(move |app| arrived(&app, asked, read));
    });
    match reader {
        Ok(r) => READER.with(|slot| *slot.borrow_mut() = Some(r)),
        Err(e) => log::error("Songs", &format!("Couldn't start the song panel's reader: {e}")),
    }

    let weak = app.as_weak();
    let started = background::spawn("Songs", move || {
        let read = Library::open_read_only(&path).and_then(|lib| Live::load(&lib, DEFAULT_RUN_GAP_MONTHS, details));
        let mut live = match read {
            Ok(live) => live,
            Err(e) => {
                log::warning("Songs", &e.to_string());
                let _ = weak.upgrade_in_event_loop(move |app| {
                    app.global::<SongsData>().set_message(format!("{e}").into());
                });
                return;
            }
        };
        let songs = live.songs();
        log::info("Songs", &format!("{} songs from {} saves", songs.rows.len(), songs.saves));
        let _ = weak.upgrade_in_event_loop(move |app| show(&app, songs));
        watch(&mut live, &weak);
    });
    if let Err(e) = started {
        log::error("Songs", &format!("Couldn't start reading the library: {e}"));
    }
}

/// How long the project folders must be quiet before what changed is read: a DAW saves in several steps (the C# app's
/// wait).
const WATCH_QUIET: Duration = Duration::from_secs(3);

/// How long after a save couldn't be read it's tried again.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// What the page says when the library has no songs.
const NO_SONGS: &str = "No songs in the library yet.";

/// Watches the project folders while the app is open and keeps the list up to date, on this background thread: each
/// time saves change, the list is made again and put on the page with a card.
fn watch(live: &mut Live, weak: &slint::Weak<AppWindow>) {
    let (tx, rx) = mpsc::channel::<Changes>();
    let watching = FolderWatch::start(live.roots(), is_project_file, WATCH_QUIET, move |c| {
        let _ = tx.send(c);
    });
    let _watch = match watching {
        Ok(w) => {
            log::info("Songs", &format!("Watching {} project folders", w.count()));
            w
        }
        Err(e) => {
            log::warning("Songs", &format!("Can't watch the project folders: {e}"));
            return;
        }
    };
    loop {
        // A save that couldn't be read (still being written, or locked) is tried again after a quiet moment.
        let changes = match rx.recv_timeout(RETRY_AFTER) {
            Ok(changes) => changes,
            Err(mpsc::RecvTimeoutError::Timeout) => match live.retries() {
                Some(changes) => changes,
                None => continue,
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let updated = live.update(&changes);
        if updated.is_empty() {
            continue;
        }
        log::info(
            "Watch",
            &format!(
                "Project folders: {} read, {} moved, {} gone ({} changes seen)",
                updated.read.len(),
                updated.moved.len(),
                updated.gone,
                changes.files.len()
            ),
        );
        let songs = live.songs();
        let _ = weak.upgrade_in_event_loop(move |app| {
            show_again(&app, songs);
            let (title, line) = updated.card();
            FEED.with(|f| {
                if let Some(feed) = f.borrow().as_ref() {
                    feed.start(&title, "").done(&line);
                }
            });
        });
    }
}

/// Puts a list made again on the page, keeping the filters, the song picked and its tab.
fn show_again(app: &AppWindow, songs: Songs) {
    let data = app.global::<SongsData>();
    let tab = data.get_tab();
    let key = PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|i| p.songs.rows.get(i)).map(|r| r.song.key.clone())
    });
    data.set_daws(strings(songs.daws().iter().map(String::as_str)));
    data.set_tags(strings(songs.tags.iter().map(String::as_str)));
    data.set_message(if songs.rows.is_empty() { NO_SONGS.into() } else { SharedString::new() });
    let row = key.and_then(|k| songs.rows.iter().position(|r| r.song.key == k));
    // Read again only when the picked song's newest save changed: a save of another song leaves its panel as it is.
    let same = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let old = p.picked.and_then(|i| p.songs.rows.get(i)).map(|r| &r.song.latest);
        let new = row.and_then(|i| songs.rows.get(i)).map(|r| &r.song.latest);
        let same = matches!((old, new), (Some(a), Some(b)) if a.id == b.id && a.mtime == b.mtime && a.size == b.size
            && a.file_missing == b.file_missing);
        p.songs = songs;
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

/// Puts a freshly read list on the page, keeping the filters as they are.
pub fn show(app: &AppWindow, songs: Songs) {
    let data = app.global::<SongsData>();
    data.set_tabs(strings(Tab::ALL.iter().map(|t| t.name())));
    data.set_saves_tab(Tab::Saves.index() as i32);
    data.set_daws(strings(songs.daws().iter().map(String::as_str)));
    data.set_tags(strings(songs.tags.iter().map(String::as_str)));
    data.set_message(if songs.rows.is_empty() { NO_SONGS.into() } else { SharedString::new() });
    PAGE.with(|p| p.borrow_mut().songs = songs);
    pick(app, None);
    data.set_lines(LINES.with(|l| ModelRc::from(l.clone() as Rc<dyn Model<Data = SongLine>>)));
    refresh(app);
}

/// Changes the filters and shows the list again.
fn change(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut Filters)) {
    let Some(app) = weak.upgrade() else { return };
    PAGE.with(|p| f(&mut p.borrow_mut().filters));
    refresh(&app);
}

/// Picks the song at `row` and brings it into view in the list, clearing the filters first when they hide it.
fn show_song(app: &AppWindow, row: usize) {
    let hidden = PAGE.with(|p| !p.borrow().view.entries.contains(&Entry::Song(row)));
    if hidden {
        PAGE.with(|p| p.borrow_mut().filters = Filters::default());
        refresh(app);
    }
    let data = app.global::<SongsData>();
    let (heading, song) = (data.get_heading_height(), data.get_song_height());
    let y = PAGE.with(|p| {
        let p = p.borrow();
        let at = p.view.entries.iter().position(|e| *e == Entry::Song(row))?;
        let before = p.view.entries.get(..at).unwrap_or_default();
        Some(before.iter().map(|e| if matches!(e, Entry::Heading(_)) { heading } else { song }).sum::<f32>())
    });
    if let Some(y) = y {
        data.set_show_y(y);
        data.set_show_ask(data.get_show_ask().wrapping_add(1));
    }
    pick(app, Some(row));
}

/// Works out what the rows and filters let through and shows it.
fn refresh(app: &AppWindow) {
    let data = app.global::<SongsData>();
    let heading = data.get_heading_height();
    let song = data.get_song_height();
    PAGE.with(|p| {
        {
            let mut p = p.borrow_mut();
            p.now = Local::now().naive_local();
            p.view = p.songs.view(&p.filters, p.now);
        }
        LINES.with(|l| l.notify.reset());
        let p = p.borrow();
        let view = &p.view;
        // Where each year's heading starts, for the year scrubber: every line is one of two heights.
        let mut y = 0.0;
        let mut at = 0;
        let mut years = Vec::with_capacity(view.years.len());
        for (year, start) in &view.years {
            for e in view.entries.get(at..*start).unwrap_or_default() {
                y += if matches!(e, Entry::Heading(_)) { heading } else { song };
            }
            at = *start;
            years.push(YearStart { year: year.to_string().into(), y });
        }
        let f = &p.filters;
        data.set_years(ModelRc::new(VecModel::from(years)));
        data.set_count(view.count.as_str().into());
        data.set_daw(f.daw.clone().unwrap_or_default().into());
        data.set_status(f.status.clone().unwrap_or_default().into());
        data.set_tag(f.tag.clone().unwrap_or_default().into());
        data.set_favourites(f.favourites);
        data.set_missing(f.missing);
        data.set_hide_archived(f.hide_archived);
        data.set_any_filter(f.any());
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
            PanelSaveLine::Save { name, date, path, missing, newest } => SaveLine {
                run: SharedString::new(),
                details: SharedString::new(),
                name: name.into(),
                date: date.into(),
                path: path.into(),
                missing,
                newest,
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
        "projects.reveal" => data.invoke_reveal(),
        "projects.copyPath" => data.invoke_copy_path(),
        _ => return false,
    }
    true
}

/// Does `f` with the page and the picked song's row, when a song is picked.
fn with_picked(f: impl FnOnce(&Page, &SongRow)) {
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some(r) = p.picked.and_then(|i| p.songs.rows.get(i)) {
            f(&p, r);
        }
    });
}

/// Whether a song is picked, for the page's keys.
pub fn has_pick() -> bool {
    PAGE.with(|p| p.borrow().picked.is_some())
}

/// Opens a save in its DAW: the one picked in Settings (shared with the C# app), else whatever opens its kind of
/// file. Off the window's thread: handing a file to a DAW that's already running can keep the system busy for
/// seconds. The library is read-only for now, so the save's "last opened" isn't noted yet.
fn open_save(page: &Page, save: &mo_library::Save) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let exe = page.songs.daw_exes.get(&save.daw).cloned();
    let (path, daw, name) = (save.path.clone(), save.daw.clone(), save.name.clone());
    let started = background::spawn("Open", move || {
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "")
                .failed("It has moved or been deleted since the library last looked. Rescan to update the library.");
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

/// Shows the file picked in its folder, or the folder when the file has gone.
fn reveal(save: &mo_library::Save) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let (path, folder) = (save.path.clone(), save.folder.clone());
    let started = background::spawn("Show in folder", move || {
        let launch = shell::reveal(&path, &folder, shell::exists(&path), shell::Os::THIS);
        if let Err(e) = shell::run(&launch) {
            log::warning("Show in folder", &format!("{path}: {e}"));
            feed.start("Couldn't show the file", "").failed(&e.to_string());
        }
    });
    if let Err(e) = started {
        log::error("Show in folder", &format!("Couldn't start: {e}"));
    }
}

/// Puts the file's path on the clipboard.
fn copy_path(path: &str) {
    let copied = CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(arboard::Clipboard::new()?);
        }
        match c.as_mut() {
            Some(clip) => clip.set_text(path.to_owned()),
            None => Ok(()),
        }
    });
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    match copied {
        Ok(()) => feed.start("Copied the path", "").done(path),
        Err(e) => {
            log::warning("Copy path", &e.to_string());
            feed.start("Couldn't copy the path", "").failed(&e.to_string());
        }
    }
}
