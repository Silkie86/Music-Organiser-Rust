//! Fills the Songs page from mo-app's song list and answers its search box and chips. The library is read on a
//! background thread (rule 12); searching and filtering then walk the rows already made, and a line's text is only
//! made when the list draws that line.

use std::cell::RefCell;
use std::rc::Rc;

use chrono::{Local, NaiveDateTime};
use mo_app::background;
use mo_app::log;
use mo_app::songs::{Entry, Filters, Songs, View};
use mo_library::songs::DEFAULT_RUN_GAP_MONTHS;
use mo_library::{Library, STATUSES};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::{AppWindow, SongLine, SongsData, YearStart};

/// The page's rows, filters and what they let through, kept on the window's thread.
struct Page {
    songs: Songs,
    filters: Filters,
    view: View,
    /// When the view was made, for "This week" and the short dates.
    now: NaiveDateTime,
}

impl Default for Page {
    fn default() -> Page {
        Page {
            songs: Songs::default(),
            filters: Filters::default(),
            view: View::default(),
            now: Local::now().naive_local(),
        }
    }
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
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
            app.global::<SongsData>().set_selected(row);
        }
    });

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
    data.set_daws(strings(songs.daws().iter().map(String::as_str)));
    data.set_tags(strings(songs.tags.iter().map(String::as_str)));
    data.set_message(if songs.rows.is_empty() { "No songs in the library yet.".into() } else { SharedString::new() });
    PAGE.with(|p| p.borrow_mut().songs = songs);
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
