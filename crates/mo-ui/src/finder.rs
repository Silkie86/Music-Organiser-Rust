//! The Ctrl+K finder's glue: keeps every page's [`Source`], asks mo-app's `finder` on the pool what matches (newest
//! search wins, low priority: rules 12 and 21), shows the lines and opens what's chosen through the page it belongs to.
//!
//! A page joins in two calls, from its own `connect`: [`add_source`] each time its list is read (again), and
//! [`on_open`] once, saying how to open one of its things. Nothing here needs to change for a new page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use mo_app::background::{self, Newest};
use mo_app::finder::{self, Group, Hit, Kind, Line, Recent, Source};
use mo_app::log;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, FinderData, FinderLine};

/// How a thing is opened: Enter, Ctrl+Enter (a song in its DAW), or Space (play a sound without leaving).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Open,
    InDaw,
    Play,
}

/// Opens one of a page's things, by its id. Returns whether it could.
type Opener = Rc<dyn Fn(&AppWindow, &Hit, How) -> bool>;

#[derive(Default)]
struct State {
    /// Every page's source, by the name it was added under, so a list read again replaces its old source.
    sources: Vec<(&'static str, Arc<dyn Source>)>,
    query: String,
    only: Option<Kind>,
    lines: Vec<Line>,
    /// The line picked with the arrows, as its place in `lines`.
    selected: Option<usize>,
    /// The arrows moved since the last letter typed: Space then plays instead of typing a space.
    moved: bool,
    recent: Recent,
    /// Counts the searches asked for, so an older answer that arrives late is dropped.
    asked: u64,
    /// Search on the window's thread (the screenshots), not the pool.
    here: bool,
}

/// A search for the pool: everything it needs, so the box can go on changing meanwhile.
struct Ask {
    id: u64,
    sources: Vec<Arc<dyn Source>>,
    query: String,
    only: Option<Kind>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static OPENERS: RefCell<HashMap<Kind, Opener>> = RefCell::default();
    static WORKER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
}

/// Hands the finder a page's things, under a name of the page's own; handing them again under the same name replaces
/// them. A search on show is done again with them.
pub fn add_source(app: &AppWindow, name: &'static str, source: Arc<dyn Source>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.sources.retain(|(n, _)| *n != name);
        s.sources.push((name, source));
    });
    if app.global::<FinderData>().get_open() {
        search(app);
    }
}

/// Says how to open a page's things of one kind.
pub fn on_open(kind: Kind, open: impl Fn(&AppWindow, &Hit, How) -> bool + 'static) {
    OPENERS.with(|o| o.borrow_mut().insert(kind, Rc::new(open)));
}

/// Searches on the window's thread from now on (the screenshots, which can't wait for the pool).
pub fn search_here() {
    STATE.with(|s| s.borrow_mut().here = true);
}

/// Wires the finder's box, keys and lines.
pub fn connect(app: &AppWindow) {
    let data = app.global::<FinderData>();
    let weak = app.as_weak();
    data.on_edited(move |text| {
        let Some(app) = weak.upgrade() else { return };
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.query = text.to_string();
            s.moved = false;
        });
        search(&app);
    });
    let weak = app.as_weak();
    data.on_key(move |text, ctrl, shift| weak.upgrade().is_some_and(|app| key(&app, &text, ctrl, shift)));
    let weak = app.as_weak();
    data.on_chosen(move |line, ctrl| {
        if let Some(app) = weak.upgrade() {
            choose(&app, usize::try_from(line).ok(), if ctrl { How::InDaw } else { How::Open });
        }
    });
    let weak = app.as_weak();
    data.on_kind_picked(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let kinds = kinds();
        let only = usize::try_from(i).ok().and_then(|i| i.checked_sub(1)).and_then(|i| kinds.get(i).copied());
        show_kind(&app, only);
    });
    let weak = app.as_weak();
    data.on_play(move |line| {
        if let Some(app) = weak.upgrade() {
            choose(&app, usize::try_from(line).ok(), How::Play);
        }
    });
    let weak = app.as_weak();
    data.on_open_asked(move || {
        if let Some(app) = weak.upgrade()
            && !app.global::<FinderData>().get_open()
        {
            toggle(&app);
        }
    });
    let weak = app.as_weak();
    data.on_close(move || {
        if let Some(app) = weak.upgrade() {
            close(&app);
        }
    });
}

/// Opens the finder, empty, on every kind; or closes it when it's open (Ctrl+K again).
pub fn toggle(app: &AppWindow) {
    let data = app.global::<FinderData>();
    if data.get_open() {
        close(app);
        return;
    }
    let _doing = crate::watchdog::doing("Finder: open");
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.query.clear();
        s.only = None;
        s.moved = false;
    });
    data.set_query(SharedString::new());
    data.set_open(true);
    search(app);
}

pub fn close(app: &AppWindow) {
    app.global::<FinderData>().set_open(false);
}

fn kinds() -> Vec<Kind> {
    STATE.with(|s| finder::kinds(&s.borrow().sources.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>()))
}

/// Shows only one kind (or every kind, `None`), keeping what's typed.
fn show_kind(app: &AppWindow, only: Option<Kind>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.only = only;
        s.moved = false;
    });
    search(app);
}

/// Asks for what matches the box as it is now.
fn search(app: &AppWindow) {
    let ask = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.asked += 1;
        Ask {
            id: s.asked,
            sources: s.sources.iter().map(|(_, s)| s.clone()).collect(),
            query: s.query.clone(),
            only: s.only,
        }
    });
    show_chips(app, &ask.sources, ask.only);
    let each = finder::each(&ask.query, ask.only);
    if STATE.with(|s| s.borrow().here) {
        let groups = finder::find(&ask.sources, &ask.query, ask.only, each);
        found(app, ask.id, groups);
        return;
    }
    WORKER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Finder", move |ask: Ask| {
                let each = finder::each(&ask.query, ask.only);
                let groups = background::on_pool(|| finder::find(&ask.sources, &ask.query, ask.only, each));
                let _ = weak.upgrade_in_event_loop(move |app| found(&app, ask.id, groups));
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error("Finder", &format!("Couldn't start the finder's thread: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

/// The kind chips: All, then each kind there's something to find of.
fn show_chips(app: &AppWindow, sources: &[Arc<dyn Source>], only: Option<Kind>) {
    let kinds = finder::kinds(sources);
    let data = app.global::<FinderData>();
    let names: Vec<SharedString> =
        std::iter::once("All").chain(kinds.iter().map(|k| k.name())).map(SharedString::from).collect();
    data.set_kinds(ModelRc::new(VecModel::from(names)));
    let at = only.and_then(|k| kinds.iter().position(|x| *x == k)).map_or(0, |i| i + 1);
    data.set_kind(at as i32);
}

/// The answer to a search: shown unless a newer one has been asked for since.
fn found(app: &AppWindow, id: u64, groups: Vec<Group>) {
    let _doing = crate::watchdog::doing("Finder: show what was found");
    let shown = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.asked != id {
            return None;
        }
        let typed = !s.query.trim().is_empty();
        s.lines = finder::lines(&groups, &s.recent, typed, s.only);
        s.selected = s.lines.iter().position(|l| matches!(l, Line::Hit(_)));
        let note = if typed && groups.is_empty() { finder::nothing_found(&s.query, s.only) } else { String::new() };
        Some(note)
    });
    let Some(note) = shown else { return };
    let data = app.global::<FinderData>();
    let lines: Vec<FinderLine> = STATE.with(|s| s.borrow().lines.iter().map(|l| finder_line(app, l)).collect());
    data.set_lines(ModelRc::new(VecModel::from(lines)));
    data.set_note(note.into());
    show_selected(app);
}

fn finder_line(app: &AppWindow, line: &Line) -> FinderLine {
    match line {
        Line::Heading(t) => {
            FinderLine { kind: "heading".into(), title: t.to_uppercase().into(), ..FinderLine::default() }
        }
        Line::Note(t) => FinderLine { kind: "note".into(), title: t.as_str().into(), ..FinderLine::default() },
        Line::More(_, t) => FinderLine { kind: "more".into(), title: t.as_str().into(), ..FinderLine::default() },
        Line::Hit(h) => FinderLine {
            kind: "hit".into(),
            title: h.title.as_str().into(),
            detail: h.detail.as_str().into(),
            key: h.key.as_str().into(),
            icon: icon(app, h),
            place: place_of(h).map_or(-1, |p| p.index() as i32),
            plays: h.plays,
            daw: h.opens_in_daw,
        },
    }
}

/// A thing's icon: its page's, from the shared style's Icons; none for an action.
fn icon(app: &AppWindow, hit: &Hit) -> SharedString {
    place_of(hit).map(|p| crate::icon(app, p)).unwrap_or_default()
}

/// The place a thing lives on: its icon and colour are that place's.
fn place_of(hit: &Hit) -> Option<mo_app::places::Place> {
    match hit.kind {
        Kind::Song => Some(mo_app::places::Place::Songs),
        Kind::Sound => Some(mo_app::places::Place::Sounds),
        Kind::Preset => Some(mo_app::places::Place::Presets),
        Kind::MidiPart => Some(mo_app::places::Place::Midi),
        Kind::Plugin => Some(mo_app::places::Place::Plugins),
        Kind::Setting => Some(mo_app::places::Place::Settings),
        Kind::Page => finder::page_of(&hit.id).map(|(p, _)| p),
        Kind::Action => None,
    }
}

/// Marks the picked line, scrolls it into view and says which keys work on it.
fn show_selected(app: &AppWindow) {
    let data = app.global::<FinderData>();
    let (heading, hit, more) = (data.get_heading_height(), data.get_hit_height(), data.get_more_height());
    let (selected, y, picked, moved) = STATE.with(|s| {
        let s = s.borrow();
        let y: f32 = s.selected.map_or(0.0, |at| {
            s.lines
                .iter()
                .take(at)
                .map(|l| match l {
                    Line::Heading(_) => heading,
                    Line::Hit(_) => hit,
                    Line::More(..) | Line::Note(_) => more,
                })
                .sum()
        });
        let picked = s.selected.and_then(|at| match s.lines.get(at) {
            Some(Line::Hit(h)) => Some(h.clone()),
            _ => None,
        });
        (s.selected, y, picked, s.moved)
    });
    data.set_selected(selected.map_or(-1, |i| i32::try_from(i).unwrap_or(-1)));
    data.set_hints(finder::hints(picked.as_ref(), moved).into());
    data.set_show_y(y);
    data.set_show_ask(data.get_show_ask().wrapping_add(1));
}

/// A key the box doesn't type. Returns whether it was used.
fn key(app: &AppWindow, text: &str, ctrl: bool, shift: bool) -> bool {
    let named = |k: slint::platform::Key| text == char::from(k).to_string();
    use slint::platform::Key as K;
    if named(K::DownArrow) {
        step(app, 1);
    } else if named(K::UpArrow) {
        step(app, -1);
    } else if named(K::PageDown) {
        step(app, 8);
    } else if named(K::PageUp) {
        step(app, -8);
    } else if named(K::Return) {
        let at = STATE.with(|s| s.borrow().selected);
        choose(app, at, if ctrl { How::InDaw } else { How::Open });
    } else if named(K::Tab) || named(K::Backtab) {
        let back = shift || named(K::Backtab);
        let now = STATE.with(|s| s.borrow().only);
        show_kind(app, finder::next_kind(&kinds(), now, back));
    } else if text == " " && !ctrl {
        // Space plays the picked sound once the arrows have moved to it; otherwise it's a space in the box.
        let (moved, at) = STATE.with(|s| (s.borrow().moved, s.borrow().selected));
        let plays =
            STATE.with(|s| matches!(at.and_then(|a| s.borrow().lines.get(a).cloned()), Some(Line::Hit(h)) if h.plays));
        if !(moved && plays) {
            return false;
        }
        choose(app, at, How::Play);
    } else {
        return false;
    }
    true
}

/// Moves the picked line `by` lines among the ones that can be picked, stopping at either end.
fn step(app: &AppWindow, by: i32) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let pickable: Vec<usize> = s
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, Line::Hit(_) | Line::More(..)))
            .map(|(i, _)| i)
            .collect();
        if pickable.is_empty() {
            return;
        }
        let now = s.selected.and_then(|at| pickable.iter().position(|p| *p == at)).unwrap_or(0) as i32;
        let next = (now + by).clamp(0, pickable.len() as i32 - 1) as usize;
        s.selected = pickable.get(next).copied();
        s.moved = true;
    });
    show_selected(app);
}

/// Opens the thing on line `at`, or shows the kind of a "12 more" line.
fn choose(app: &AppWindow, at: Option<usize>, how: How) {
    let Some(line) = STATE.with(|s| at.and_then(|a| s.borrow().lines.get(a).cloned())) else { return };
    let hit = match line {
        Line::Hit(h) => h,
        Line::More(kind, _) => {
            show_kind(app, Some(kind));
            return;
        }
        Line::Heading(_) | Line::Note(_) => return,
    };
    let _doing = crate::watchdog::doing("Finder: open what was chosen");
    let Some(open) = OPENERS.with(|o| o.borrow().get(&hit.kind).cloned()) else {
        log::warning("Finder", &format!("Nothing opens {}", hit.kind.name()));
        return;
    };
    if how == How::Play {
        open(app, &hit, How::Play);
        return;
    }
    // The finder closes first, so a page that takes the keys gets them.
    close(app);
    STATE.with(|s| s.borrow_mut().recent.add(hit.clone()));
    if !open(app, &hit, how) && how == How::InDaw {
        open(app, &hit, How::Open);
    }
}
