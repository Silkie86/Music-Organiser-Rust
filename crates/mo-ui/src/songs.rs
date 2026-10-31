//! Fills the Songs page from mo-app's song list and answers its search box and chips. The library is read on a
//! background thread (rule 12); searching and filtering then walk the rows already made, and a line's text is only
//! made when the list draws that line. Picking a song opens its panel at once with what the list knows; the newest
//! save's plugins, samples and details follow from a background thread that only reads the last song picked.

use std::cell::RefCell;
use std::rc::Rc;

use chrono::{Local, NaiveDateTime};
use mo_app::background::{self, Newest};
use mo_app::log;
use mo_app::song_panel::{self as panel, SaveLine as PanelSaveLine, Tab};
use mo_app::songs::{Entry, Filters, SongRow, Songs, View};
use mo_library::songs::DEFAULT_RUN_GAP_MONTHS;
use mo_library::{Library, STATUSES, SaveDetail};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::{
    AppWindow, CountLine, DetailPair, PanelSong, PluginLine, SampleLine, SaveLine, SongLine, SongsData, YearStart,
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
        }
    }
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
    /// Reads a save's record for the panel, one at a time, newest ask first.
    static READER: RefCell<Option<Newest<(u64, i64)>>> = const { RefCell::new(None) };
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

/// Wires the page's search box and chips, and starts reading the library at `path` in the background.
pub fn connect(app: &AppWindow, path: std::path::PathBuf) {
    let data = app.global::<SongsData>();
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
    data.on_close_panel(move || {
        if let Some(app) = weak.upgrade() {
            pick(&app, None);
        }
    });

    let weak = app.as_weak();
    let db = path.clone();
    let mut library: Option<Library> = None;
    let reader = Newest::start("Song panel", move |(asked, id): (u64, i64)| {
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
        let read = Library::open_read_only(&path).and_then(|lib| Songs::load(&lib, DEFAULT_RUN_GAP_MONTHS));
        let _ = weak.upgrade_in_event_loop(move |app| match read {
            Ok(songs) => {
                log::info("Songs", &format!("{} songs from {} saves", songs.rows.len(), songs.saves));
                show(&app, songs);
            }
            Err(e) => {
                log::warning("Songs", &e.to_string());
                app.global::<SongsData>().set_message(format!("{e}").into());
            }
        });
    });
    if let Err(e) = started {
        log::error("Songs", &format!("Couldn't start reading the library: {e}"));
    }
}

/// Puts a freshly read list on the page, keeping the filters as they are.
pub fn show(app: &AppWindow, songs: Songs) {
    let data = app.global::<SongsData>();
    data.set_tabs(strings(Tab::ALL.iter().map(|t| t.name())));
    data.set_daws(strings(songs.daws().iter().map(String::as_str)));
    data.set_tags(strings(songs.tags.iter().map(String::as_str)));
    data.set_message(if songs.rows.is_empty() { "No songs in the library yet.".into() } else { SharedString::new() });
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
    let Some((asked, id)) = id else { return };
    data.set_tab(Tab::Overview.index() as i32);
    data.set_detail_message("Reading…".into());
    fill_panel(app);
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
        data.set_runs(model(
            r.runs.iter().rev().map(|run| SharedString::from(format!("{} · {}", run.label(), run.details()))),
        ));
        data.set_saves_heading(panel::saves_heading(r).into());
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

/// The parts of the panel that come from the newest save's record: empty until it has been read.
fn fill_detail(app: &AppWindow) {
    let data = app.global::<SongsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let Some(r) = p.picked.and_then(|i| p.songs.rows.get(i)) else { return };
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
