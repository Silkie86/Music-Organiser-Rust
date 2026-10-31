//! Fills the Songs page from mo-app's song list and answers its search box and chips. The library is read on a
//! background thread (rule 12); searching and filtering then walk the rows already made.

use std::cell::RefCell;
use std::rc::Rc;

use chrono::{Local, NaiveDateTime};
use mo_app::background;
use mo_app::log;
use mo_app::songs::{Entry, Filters, Songs};
use mo_library::songs::DEFAULT_RUN_GAP_MONTHS;
use mo_library::{Library, STATUSES};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, SongLine, SongsData, YearStart};

/// The page's rows and filters, kept on the window's thread.
#[derive(Default)]
struct Page {
    songs: Songs,
    filters: Filters,
}

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
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
    refresh(app);
}

/// Changes the filters and shows the list again.
fn change(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut Filters)) {
    let Some(app) = weak.upgrade() else { return };
    PAGE.with(|p| f(&mut p.borrow_mut().filters));
    refresh(&app);
}

/// Builds the lines the list shows from the rows and filters.
fn refresh(app: &AppWindow) {
    let data = app.global::<SongsData>();
    let now = Local::now().naive_local();
    let heading = data.get_heading_height();
    let song = data.get_song_height();
    PAGE.with(|p| {
        let p = p.borrow();
        let view = p.songs.view(&p.filters, now);
        let lines: Vec<SongLine> = view.entries.iter().map(|e| line(&p.songs, e, now)).collect();
        // Where each line starts, for the year scrubber.
        let mut y = 0.0;
        let mut starts = Vec::with_capacity(view.entries.len());
        for e in &view.entries {
            starts.push(y);
            y += if matches!(e, Entry::Heading(_)) { heading } else { song };
        }
        let years: Vec<YearStart> = view
            .years
            .iter()
            .map(|(year, at)| YearStart { year: year.to_string().into(), y: starts.get(*at).copied().unwrap_or(0.0) })
            .collect();
        let f = &p.filters;
        data.set_lines(ModelRc::new(VecModel::from(lines)));
        data.set_years(ModelRc::new(VecModel::from(years)));
        data.set_count(view.count.into());
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
        Entry::Heading(h) => SongLine { heading: h.as_str().into(), row: -1, ..SongLine::default() },
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
                status: r.status.as_str().into(),
                daw: r.song.latest.daw.as_str().into(),
            }
        }
    }
}

fn strings<'a>(items: impl Iterator<Item = &'a str>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.map(SharedString::from).collect::<Vec<_>>()))
}
