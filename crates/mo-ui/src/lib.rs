//! The window shell and its screens. The screens themselves are in `ui/*.slint`; this file fills them from `mo-app`
//! (the look, the places, the shortcut list) and passes clicks and keys back.

// The code Slint writes for the screens unwraps freely, so it's kept apart from the rule against it (rule 17's crash
// log still catches anything).
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod screens {
    slint::include_modules!();
}
pub use screens::*;

pub mod arrangement;
pub mod backup;
mod canvas;
pub mod cards;
pub mod daws;
pub mod drag;
pub mod files;
pub mod filters;
pub mod finder;
pub mod gpu;
pub mod helper;
mod in_flight;
pub mod midi;
pub mod note;
mod piano_roll;
pub mod plugins;
pub mod presets;
pub mod settings;
pub mod songs;
pub mod sounds;
pub mod space;
pub mod startup;
mod stuck;
pub mod watchdog;
mod waves;

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use mo_app::activity::Feed;
use mo_app::log;
use mo_app::places::Place;
use mo_app::shortcuts::{
    APP_ACTIONS, Action, BACKUP_ACTIONS, BUILT, MIDI_ACTIONS, PLUGINS_ACTIONS, SONGS_ACTIONS, SOUNDS_ACTIONS,
    SPACE_ACTIONS, Shortcuts, is_built, list_keys, repeats, what_of,
};
use mo_app::themes::Look;
use note::open_note;
use slint::platform::Key;
use slint::{Color, ComponentHandle, ModelRc, VecModel, Weak};

/// The place on show, kept where the freeze watchdog can read it without asking the window.
static PAGE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// The keys in use, for the key sheet, which changes with the page.
    static KEYS: RefCell<Option<std::rc::Rc<Shortcuts>>> = const { RefCell::new(None) };
    /// The look in use, for what's drawn outside Slint (the card that follows a drag).
    static LOOK: std::cell::Cell<Option<Look>> = const { std::cell::Cell::new(None) };
}

/// The look in use (on the window's thread): the one last put on the window, else the default.
pub fn look() -> Look {
    LOOK.with(std::cell::Cell::get).unwrap_or_else(|| Look::from_settings(None, None))
}

/// The place on show, safe to read from any thread.
pub fn place() -> Option<Place> {
    Place::from_index(PAGE.load(Ordering::Relaxed))
}

/// Notes `place` as the one on show, as the window's keys go by it (Esc, Ctrl+Up and Ctrl+Down), without showing it or
/// starting what showing it starts ([`go_to`]): for mo-screens, which shows Space with made-up drives and must never
/// have it look at the real ones.
pub fn note_place(place: Place) {
    PAGE.store(place.index(), Ordering::Relaxed);
}

/// The name of the place on show, safe to read from any thread.
pub fn page_name() -> &'static str {
    Place::from_index(PAGE.load(Ordering::Relaxed)).map_or("?", Place::name)
}

/// Builds the main window in a look, opening on a place. Jobs report to `feed`, and its cards show bottom right.
pub fn new_app(look: &Look, shortcuts: Shortcuts, start: Place, feed: Feed) -> Result<AppWindow, slint::PlatformError> {
    let app = AppWindow::new()?;
    arrangement::connect(&app);
    waves::connect(&app);
    apply_look(&app, look);
    show_places(&app, &shortcuts);
    app.set_settings_place(Place::Settings.index() as i32);
    app.set_songs_place(Place::Songs.index() as i32);
    app.set_sounds_place(Place::Sounds.index() as i32);
    app.set_backup_place(Place::Backup.index() as i32);
    app.set_plugins_place(Place::Plugins.index() as i32);
    app.set_presets_place(Place::Presets.index() as i32);
    app.set_curves_place(Place::Curves.index() as i32);
    app.set_midi_place(Place::Midi.index() as i32);
    app.set_space_place(Place::Space.index() as i32);
    KEYS.with(|k| *k.borrow_mut() = Some(std::rc::Rc::new(shortcuts.clone())));
    show_button_keys(&app, &shortcuts);
    connect_finder(&app, &shortcuts, &feed);
    go_to(&app, start.index() as i32);

    let weak = app.as_weak();
    app.on_place_chosen(move |i| {
        if let Some(app) = weak.upgrade() {
            go_to(&app, i);
        }
    });
    let weak = app.as_weak();
    let key_feed = feed.clone();
    app.on_key(move |text, ctrl, shift, alt, meta, repeat| {
        key_pressed(&weak, &key_feed, &key_name(&text, ctrl, shift, alt, meta), repeat)
    });
    // The same keys from a search box, for the ones it would only use to move its cursor (Ctrl+Up and Ctrl+Down).
    let weak = app.as_weak();
    let key_feed = feed.clone();
    app.global::<AppKeys>().on_key(move |text, ctrl, shift, alt, meta, repeat| {
        key_pressed(&weak, &key_feed, &key_name(&text, ctrl, shift, alt, meta), repeat)
    });

    cards::connect(&app, &feed);
    note::connect(&app, &feed);
    Ok(app)
}

/// A key pressed on the window, or passed on by a search box (`AppKeys.key`), written as [`key_name`] writes it:
/// Settings › Shortcuts listening for one, F12, then the page's own key, then an app-wide one. Whether it did anything,
/// so an unused key goes on.
fn key_pressed(weak: &Weak<AppWindow>, feed: &Feed, key: &str, repeat: bool) -> bool {
    let _doing = watchdog::doing("a key");
    // Settings › Shortcuts listening for a new key: this press is the key, not a shortcut.
    if settings::listening()
        && let Some(app) = weak.upgrade()
    {
        return repeat || settings::key_pressed(&app, key);
    }
    // The keys as they are now: Settings may have changed one.
    let Some(shortcuts) = KEYS.with(|k| k.borrow().clone()) else { return false };
    if key == "F12" {
        return weak.upgrade().is_some_and(|app| {
            if !repeat {
                open_note(&app);
            }
            true
        });
    }
    // A page's own key beats an app-wide one, when there's something for it to do; otherwise the key goes on.
    if let Some(app) = weak.upgrade()
        && app.get_place() == Place::Songs.index() as i32
        && let Some(id) = shortcuts.page_action_for(key, SONGS_ACTIONS)
        && songs::has_pick()
    {
        return repeat || songs::run_action(&app, id);
    }
    if let Some(app) = weak.upgrade()
        && app.get_place() == Place::Plugins.index() as i32
        && let Some(id) = shortcuts.page_action_for(key, PLUGINS_ACTIONS)
        && (plugins::has_pick() || id == plugins::MEASURE)
    {
        return repeat || plugins::run_action(&app, id);
    }
    if let Some(app) = weak.upgrade()
        && on_presets_page(app.get_place())
        && let Some(id) = shortcuts.page_action_for(key, presets::page_actions())
        && (repeat || presets::run_action(&app, id))
    {
        return true;
    }
    if let Some(app) = weak.upgrade()
        && app.get_place() == Place::Sounds.index() as i32
        && let Some(id) = shortcuts.page_action_for(key, SOUNDS_ACTIONS)
        && (repeat || sounds::run_action(&app, id))
    {
        return true;
    }
    if let Some(app) = weak.upgrade()
        && app.get_place() == Place::Midi.index() as i32
        && let Some(id) = shortcuts.page_action_for(key, MIDI_ACTIONS)
        && (repeat || midi::run_action(&app, id))
    {
        return true;
    }
    if let Some(app) = weak.upgrade()
        && app.get_place() == Place::Space.index() as i32
        && let Some(id) = shortcuts.page_action_for(key, SPACE_ACTIONS)
        && (repeat || space::run_action(&app, id))
    {
        return true;
    }
    if let Some(app) = weak.upgrade()
        && app.get_place() == Place::Backup.index() as i32
        && let Some(id) = shortcuts.page_action_for(key, BACKUP_ACTIONS)
        && (repeat || backup::run_action(&app, id))
    {
        return true;
    }
    let Some(id) = shortcuts.action_for(key) else { return false };
    // A key held down did its job once (unless it's one that steps), and the page behind doesn't get it either.
    if repeat && !repeats(id) {
        return is_built(id);
    }
    run(id, weak, feed, repeat)
}

/// Ctrl+K: the pages, Settings tabs and actions to jump to, and how to open them. Songs, sounds, presets, MIDI parts
/// and plugins join from their own pages.
fn connect_finder(app: &AppWindow, shortcuts: &Shortcuts, feed: &Feed) {
    finder::connect(app);
    finder_pages(app, shortcuts);
    finder::add_source(app, "settings", std::sync::Arc::new(mo_app::finder::settings()));
    finder::on_open(mo_app::finder::Kind::Setting, |app, hit, _| {
        let Some(tab) = mo_app::settings::Tab::ALL.into_iter().find(|t| t.name() == hit.id) else { return false };
        settings::open_on(app, tab);
        true
    });
    // The page on show may have changed its own list since (the Presets page's Sounds or Curves).
    finder::when_opening(|app| {
        if let Some(place) = place() {
            finder_actions(app, &crate::shortcuts(), place);
        }
    });
    finder::on_open(mo_app::finder::Kind::Page, |app, hit, _| {
        let Some((place, view)) = mo_app::finder::page_of(&hit.id) else { return false };
        go_to(app, place.index() as i32);
        if let Some(view) = view
            && place == Place::Sounds
        {
            sounds::show_view(app, view);
        }
        true
    });
    let feed = feed.clone();
    finder::on_open(mo_app::finder::Kind::Action, move |app, hit, _| {
        let id = hit.id.as_str();
        // A page's own action runs on that page, which the finder was opened on.
        if let Some(place) = place()
            && page_actions(place).iter().any(|a| a.id == id)
        {
            return match place {
                Place::Songs => songs::run_action(app, id),
                Place::Sounds => sounds::run_action(app, id),
                Place::Plugins => plugins::run_action(app, id),
                Place::Presets | Place::Curves => presets::run_action(app, id),
                Place::Midi => midi::run_action(app, id),
                Place::Space => space::run_action(app, id),
                Place::Backup => backup::run_action(app, id),
                Place::Settings => false,
            };
        }
        run(id, &app.as_weak(), &feed, false)
    });
}

/// The places Ctrl+K offers, with their keys, and the Sounds page's views.
fn finder_pages(app: &AppWindow, shortcuts: &Shortcuts) {
    let views: [(Place, &[&str]); 1] = [(Place::Sounds, &sounds::VIEWS)];
    finder::add_source(app, "pages", std::sync::Arc::new(mo_app::finder::pages(shortcuts, &views)));
}

/// The actions Ctrl+K offers on a place: its page's own, then the app-wide ones that do something so far (not Ctrl+K
/// itself, the help key once, and not the keys for moving about the page: Ctrl+F, Esc, Ctrl+Up, Ctrl+Down and Home,
/// which act on where the keys were before Ctrl+K took them).
fn finder_actions(app: &AppWindow, shortcuts: &Shortcuts, place: Place) {
    let page = page_actions(place);
    let wide = APP_ACTIONS.iter().filter(|a| {
        is_built(a.id)
            && !matches!(
                a.id,
                "app.jump"
                    | "app.helpF1"
                    | "app.search"
                    | "app.escape"
                    | "app.prevItem"
                    | "app.nextItem"
                    | "app.listTop"
            )
    });
    let actions = mo_app::finder::actions(shortcuts, page.iter().chain(wide));
    finder::add_source(app, "actions", std::sync::Arc::new(actions));
}

/// The keys in use now.
pub fn shortcuts() -> Shortcuts {
    KEYS.with(|k| k.borrow().as_deref().cloned()).unwrap_or_default()
}

/// A key was changed in Settings: everything that shows or runs a key takes the new list at once (the rail's tooltips,
/// the key sheet, the Songs buttons, the Sounds, Presets, MIDI and Plugins cards and menus, the Space buttons). Every
/// page's keys run from this one list.
pub fn keys_changed(app: &AppWindow, shortcuts: &Shortcuts) {
    KEYS.with(|k| *k.borrow_mut() = Some(std::rc::Rc::new(shortcuts.clone())));
    show_places(app, shortcuts);
    show_button_keys(app, shortcuts);
    sounds::show_keys(app, shortcuts);
    presets::show_keys(app, shortcuts);
    midi::show_keys(app, shortcuts);
    plugins::show_keys(app, shortcuts);
    space::show(app);
    if let Some(place) = place() {
        show_key_rows(app, shortcuts, place);
        finder_actions(app, shortcuts, place);
    }
    finder_pages(app, shortcuts);
}

/// The places on the rail, with each one's key for its tooltip.
fn show_places(app: &AppWindow, shortcuts: &Shortcuts) {
    let places: Vec<PlaceInfo> = Place::ALL
        .iter()
        .map(|p| PlaceInfo {
            name: p.name().into(),
            coming: coming(*p).into(),
            key: shortcuts.key_of(p.shortcut_id()).into(),
            icon: icon(app, *p),
        })
        .collect();
    let settings = Place::Settings.index();
    app.set_rail(ModelRc::new(VecModel::from(places[..settings].to_vec())));
    app.set_places(ModelRc::new(VecModel::from(places)));
    app.global::<FinderData>().set_jump_key(shortcuts.key_of("app.jump").into());
}

/// The Songs and Backup pages' buttons' tooltips, and the MIDI page's Read MIDI again's key: what each does and its
/// key, from the one shortcut list (rule 15).
fn show_button_keys(app: &AppWindow, shortcuts: &Shortcuts) {
    let action = |id: &str| ActionKey { what: what_of(id).into(), key: shortcuts.key_of(id).into() };
    let data = app.global::<SongsData>();
    data.set_open_key(action("projects.open"));
    data.set_open_own_key(action("projects.openOwn"));
    data.set_reveal_key(action("projects.reveal"));
    data.set_copy_key(action("projects.copyPath"));
    let backup = app.global::<BackupData>();
    backup.set_run_key(action("backup.now"));
    backup.set_stop_key(action("backup.stop"));
    app.global::<MidiData>().set_read_again_key(shortcuts.key_of("midi.readAgain").into());
    app.global::<AppKeys>().set_list_top_key(shortcuts.key_of("app.listTop").into());
}

/// Whether the place at `index` is one of the Presets page's two (Presets and Curves: one page, a section each).
pub(crate) fn on_presets_page(index: i32) -> bool {
    index == Place::Presets.index() as i32 || index == Place::Curves.index() as i32
}

/// Shows a place, and notes it for the log and the watchdog.
pub(crate) fn go_to(app: &AppWindow, index: i32) {
    let Some(place) = usize::try_from(index).ok().and_then(Place::from_index) else { return };
    let _doing = watchdog::doing("go to a page");
    let before = PAGE.swap(place.index(), Ordering::Relaxed);
    if before != place.index() || app.get_place() != index {
        log::info("Page", place.name());
    }
    // An audition in Bitwig stops as its page goes (the 6 Nov review's X3-1).
    // (Presets and Curves are one page's two sections: only leaving both leaves it.)
    let presets_page = |p: Place| matches!(p, Place::Presets | Place::Curves);
    if Place::from_index(before).is_some_and(presets_page) && !presets_page(place) {
        presets::left(app);
    }
    // The section first, so the page never shows the other place's list for a frame.
    match place {
        Place::Presets => presets::show_section(app, mo_app::presets::Section::Sounds),
        Place::Curves => presets::show_section(app, mo_app::presets::Section::Curves),
        _ => {}
    }
    // The place's colour first, so the new page's mark never shows the old place's colour for a frame (the window's
    // own `changed place` comes a frame late).
    app.global::<Theme>().set_here(index);
    app.set_place(index);
    finder::close(app);
    settings::page_shown(place);
    if matches!(place, Place::Sounds | Place::Space) {
        sounds::wanted();
    }
    if place == Place::Backup {
        backup::shown();
    }
    if place == Place::Plugins {
        plugins::shown();
    }
    if place == Place::Midi {
        midi::shown(app);
    }
    if place == Place::Settings {
        settings::shown(app);
    }
    if place == Place::Space {
        space::shown(app);
    }
    if presets_page(place) {
        presets::shown(app);
    }
    refresh_keys(app);
}

/// Fills the key sheet for the place on show (and, on Presets, the section on show).
pub fn refresh_keys(app: &AppWindow) {
    let Some(place) = usize::try_from(app.get_place()).ok().and_then(Place::from_index) else { return };
    KEYS.with(|k| {
        if let Some(shortcuts) = k.borrow().as_ref() {
            show_key_rows(app, shortcuts, place);
            finder_actions(app, shortcuts, place);
        }
    });
}

/// The key sheet's two lists for `place`: its own keys, and the ones that work everywhere.
fn show_key_rows(app: &AppWindow, shortcuts: &Shortcuts, place: Place) {
    let (page, everywhere) = key_rows(shortcuts, place);
    app.set_page_key_rows(ModelRc::new(VecModel::from(page)));
    app.set_key_rows(ModelRc::new(VecModel::from(everywhere)));
}

/// A page's own actions: what its keys do, the key sheet lists and Ctrl+K offers on it.
fn page_actions(place: Place) -> &'static [Action] {
    match place {
        Place::Songs => SONGS_ACTIONS,
        Place::Sounds => SOUNDS_ACTIONS,
        Place::Plugins => PLUGINS_ACTIONS,
        Place::Presets => mo_app::shortcuts::PRESETS_ACTIONS,
        Place::Curves => mo_app::shortcuts::CURVES_ACTIONS,
        Place::Midi => MIDI_ACTIONS,
        Place::Space => SPACE_ACTIONS,
        Place::Backup => BACKUP_ACTIONS,
        Place::Settings => &[],
    }
}

/// The key sheet's lines, with the key each has now, in two lists (the whole-app review of 6 Nov, X3-25): the page's
/// own actions and the keys its list keeps for itself (the arrows), then the app-wide actions and F12, which the app
/// keeps for itself. An app-wide action that isn't built yet isn't listed, so every key on the sheet works (X3-22).
fn key_rows(shortcuts: &Shortcuts, place: Place) -> (Vec<KeyRow>, Vec<KeyRow>) {
    let row = |key: &str, what: &str| KeyRow { key: key.into(), what: what.into() };
    let mut page: Vec<KeyRow> = page_actions(place).iter().map(|a| row(shortcuts.key_of(a.id), a.what)).collect();
    page.extend(list_keys(place).iter().map(|(key, what)| row(key, what)));
    let mut everywhere: Vec<KeyRow> =
        APP_ACTIONS.iter().filter(|a| is_built(a.id)).map(|a| row(shortcuts.key_of(a.id), a.what)).collect();
    everywhere.push(row("F12", "Write a note for Claude about what's on screen"));
    (page, everywhere)
}

/// Puts a look's colours and fonts into the screens' `Theme` tokens.
pub fn apply_look(app: &AppWindow, look: &Look) {
    LOOK.with(|l| l.set(Some(*look)));
    let t = app.global::<Theme>();
    let th = &look.theme;
    t.set_background(rgb(th.background));
    t.set_sidebar(rgb(th.sidebar));
    t.set_surface(rgb(th.surface));
    t.set_surface_raised(rgb(th.surface_raised));
    t.set_stroke(rgb(th.stroke));
    t.set_hover(rgb(th.hover));
    t.set_text(rgb(th.text));
    t.set_muted(rgb(th.muted));
    t.set_faint(rgb(th.faint));
    t.set_accent(rgb(look.accent));
    t.set_accent_text(rgb(look.accent_text));
    t.set_light(th.light);
    t.set_square(th.square);
    t.set_text_font(th.font.text_family().into());
    t.set_number_font(th.font.number_family().into());
    t.set_title_font(th.font.title_family().into());
    let places: Vec<Color> = mo_app::themes::place_colours(look).iter().map(|c| rgb(*c)).collect();
    t.set_places(ModelRc::new(VecModel::from(places)));
    app.global::<Palette>().set_color_scheme(if th.light {
        slint::language::ColorScheme::Light
    } else {
        slint::language::ColorScheme::Dark
    });
    arrangement::set_look(app, look);
    piano_roll::look_changed(app);
}

pub(crate) fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// What a place's page will hold, shown until the page is built.
/// The place's icon on the rail, drawn in the shared style's Icons.
fn icon(app: &AppWindow, place: Place) -> slint::SharedString {
    let icons = app.global::<Icons>();
    match place {
        Place::Songs => icons.get_songs(),
        Place::Sounds => icons.get_sounds(),
        Place::Presets => icons.get_presets(),
        Place::Curves => icons.get_curves(),
        Place::Midi => icons.get_midi(),
        Place::Plugins => icons.get_plugins(),
        Place::Space => icons.get_space(),
        Place::Backup => icons.get_backup(),
        Place::Settings => icons.get_settings(),
    }
}

fn coming(place: Place) -> &'static str {
    match place {
        Place::Songs => {
            "Your songs, grouped from their saves, with the Health strip and the song tabs. Built in phase 2."
        }
        Place::Sounds => {
            "Every sound as Tiles, a List or the Galaxy, with the kit builder beside them. Built in phase 3."
        }
        Place::Presets => {
            "Every sound you made inside your songs, and your preset files, with a guess at each one's part (bass, lead, \
             chords, drums) from the notes it played. Built in phase 5."
        }
        Place::Curves => "The shapes your settings moved in, in your songs and your plugins, to reuse and reshape.",
        Place::Midi => {
            "Your MIDI packs and the parts in your songs: search, play and drag into a DAW. Built in phase 6."
        }
        Place::Plugins => "Every plugin your songs use, and what to install to open more of them. Built in phase 4.",
        Place::Space => {
            "The biggest ways to free disk space, across plugins, songs, spare copies and duplicates. Built in phase 4."
        }
        Place::Backup => "A lean copy of your music on another drive.",
        Place::Settings => "General, Folders, DAWs, Shortcuts and Developer. Filled in as each page arrives.",
    }
}

/// The places with a list, which Home takes back to its top.
const LISTED: [Place; 6] = [Place::Songs, Place::Sounds, Place::Presets, Place::Curves, Place::Midi, Place::Plugins];

/// The places with a search box, which Ctrl+F puts the keys in.
const SEARCHED: [Place; 6] = [Place::Songs, Place::Sounds, Place::Presets, Place::Curves, Place::Midi, Place::Plugins];

/// Runs an app-wide action (`held`: its key is repeating). Returns whether it did anything, so an unused key can go on
/// to the page.
fn run(id: &str, weak: &Weak<AppWindow>, feed: &Feed, held: bool) -> bool {
    let Some(app) = weak.upgrade() else { return false };
    if let Some(p) = Place::ALL.into_iter().find(|p| p.shortcut_id() == id) {
        go_to(&app, p.index() as i32);
        return true;
    }
    let n = Place::ALL.len() as i32;
    match id {
        "app.prevPage" => go_to(&app, (app.get_place() + n - 1) % n),
        "app.nextPage" => go_to(&app, (app.get_place() + 1) % n),
        "app.jump" => finder::toggle(&app),
        "app.help" | "app.helpF1" => app.set_sheet_open(!app.get_sheet_open()),
        "app.fullScreen" => {
            let w = app.window();
            w.set_fullscreen(!w.is_fullscreen());
        }
        // Where everything the app removes goes first, from any page.
        "app.recycleBin" => files::open_recycle_bin(feed),
        // Space plays the picked sound on the Sounds page, loops the picked part on the MIDI page and auditions the
        // picked sound in Bitwig on Presets (X3-13); elsewhere it's left for the page (a button, a text box).
        "app.play" => {
            let place = app.get_place();
            return (place == Place::Sounds.index() as i32 && sounds::toggle_play(&app))
                || (place == Place::Midi.index() as i32 && midi::toggle_play(&app))
                || (on_presets_page(place) && presets::toggle_play(&app));
        }
        // Ctrl+F: the keys go to the page's search box, its words picked (review X3-22). The page on show sees the
        // count change; a page without a search box lets the key go on. Not while Ctrl+K or the F12 note is open, as
        // the keys would go to a box under it; the key sheet closes first, so the search is in sight.
        "app.search" => {
            if app.global::<FinderData>().get_open()
                || app.get_note_open()
                || !place().is_some_and(|p| SEARCHED.contains(&p))
            {
                return false;
            }
            app.set_sheet_open(false);
            let keys = app.global::<AppKeys>();
            keys.set_search(keys.get_search().wrapping_add(1));
        }
        // Esc: one step back, in the same order on every page (review X3-12), and first what plays stops, whichever
        // page started it.
        "app.escape" => {
            if stop_what_plays(&app) {
                return true;
            }
            return match place() {
                Some(Place::Songs) => songs::escape(&app),
                Some(Place::Sounds) => sounds::escape(&app),
                Some(Place::Presets | Place::Curves) => presets::escape(&app),
                Some(Place::Midi) => midi::escape(&app),
                Some(Place::Plugins) => plugins::escape(&app),
                Some(Place::Space) => space::escape(&app),
                _ => false,
            };
        }
        // Home: the list on show back to its top, as the button over its corner does (Solomon's F12 note of 11
        // Nov). The list on show sees the count change. Off the places with lists the key goes on, and so it does
        // while Ctrl+K or the F12 note is open over the page.
        "app.listTop" => {
            if app.global::<FinderData>().get_open()
                || app.get_note_open()
                || !place().is_some_and(|p| LISTED.contains(&p))
            {
                return false;
            }
            let keys = app.global::<AppKeys>();
            keys.set_list_top(keys.get_list_top().wrapping_add(1));
        }
        // Ctrl+Up and Ctrl+Down: the line above or below in the page's list, even while you type a search.
        "app.prevItem" | "app.nextItem" => {
            return step_list(&app, if id == "app.prevItem" { -1 } else { 1 }, held);
        }
        _ => {
            debug_assert!(!BUILT.contains(&id), "{id} is in BUILT but run() doesn't do it");
            return false;
        }
    }
    debug_assert!(is_built(id), "run() does {id}: add it to BUILT so the key sheet and Settings list it");
    true
}

/// Esc's first step on every page: whatever plays stops, whichever page started it (a sound on Sounds, the MIDI page's
/// loop or a part playing in a DAW go on after their page goes; an audition in Bitwig stops as its page goes). Each
/// one, as two can play at once. Whether anything was playing.
fn stop_what_plays(app: &AppWindow) -> bool {
    let sound = sounds::stop_on_escape(app);
    let midi = midi::stop_on_escape(app);
    let audition = presets::stop_on_escape(app);
    sound || midi || audition
}

/// Moves the pick of the list on show one line up (`by` -1) or down (1), as its Up and Down keys do (on Sounds' Tiles, a
/// row up or down). False on a page without a list to step through.
fn step_list(app: &AppWindow, by: i32, held: bool) -> bool {
    match place() {
        Some(Place::Songs) => app.global::<SongsData>().invoke_step(by),
        Some(Place::Sounds) => app.global::<SoundsData>().invoke_step(by * 2, held),
        Some(Place::Midi) => app.global::<MidiData>().invoke_step(by),
        Some(Place::Plugins) => app.global::<PluginsData>().invoke_step(by),
        Some(Place::Presets | Place::Curves) => {
            let data = app.global::<PresetsData>();
            if data.get_section() == 0 {
                data.invoke_step(by);
            } else {
                data.invoke_curve_step(by);
            }
        }
        _ => return false,
    }
    true
}

/// A key press written the way the shortcut list writes keys ("Ctrl+Shift+E", "F11", "Alt+Left").
pub fn key_name(text: &str, ctrl: bool, shift: bool, alt: bool, meta: bool) -> String {
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return String::new() };
    let named = [
        (Key::F1, "F1"),
        (Key::F2, "F2"),
        (Key::F3, "F3"),
        (Key::F4, "F4"),
        (Key::F5, "F5"),
        (Key::F6, "F6"),
        (Key::F7, "F7"),
        (Key::F8, "F8"),
        (Key::F9, "F9"),
        (Key::F10, "F10"),
        (Key::F11, "F11"),
        (Key::F12, "F12"),
        (Key::PageUp, "PageUp"),
        (Key::PageDown, "PageDown"),
        (Key::LeftArrow, "Left"),
        (Key::RightArrow, "Right"),
        (Key::UpArrow, "Up"),
        (Key::DownArrow, "Down"),
        (Key::Delete, "Delete"),
        (Key::Escape, "Esc"),
        (Key::Tab, "Tab"),
        (Key::Return, "Enter"),
        (Key::Home, "Home"),
        (Key::End, "End"),
        (Key::Insert, "Insert"),
        (Key::Backspace, "Backspace"),
    ];
    let name = match named.iter().find(|(k, _)| char::from(*k) == c) {
        Some((_, n)) => (*n).to_owned(),
        None if c == ' ' => "Space".to_owned(),
        // Modifier keys on their own are not shortcuts.
        None if c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c) => return String::new(),
        None => c.to_uppercase().collect(),
    };
    let mut out = String::new();
    for (on, m) in [(ctrl, "Ctrl+"), (shift, "Shift+"), (alt, "Alt+"), (meta, "Meta+")] {
        if on {
            out.push_str(m);
        }
    }
    out.push_str(&name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_sheet_puts_the_page_s_keys_first_and_lists_only_keys_that_work() {
        let keys = Shortcuts::default();
        let (page, everywhere) = key_rows(&keys, Place::Songs);
        // The page's own actions, then what its list's arrows do (X3-25).
        assert_eq!(page[0].what.as_str(), what_of("projects.open"));
        assert!(page.iter().any(|r| r.key == "Up, Down"), "{page:?}");
        // Every key that works everywhere, and none that isn't built yet (X3-22), then F12.
        assert!(everywhere.iter().any(|r| r.key == "Ctrl+F" && r.what == what_of("app.search")));
        assert!(everywhere.iter().any(|r| r.key == "Esc" && r.what == what_of("app.escape")));
        assert!(!everywhere.iter().any(|r| r.key == "F5" || r.what.contains("(coming)")), "{everywhere:?}");
        assert_eq!(everywhere.last().map(|r| r.key.as_str()), Some("F12"));
        // Backup has keys of its own now (X3-21); Settings has none.
        let (backup, _) = key_rows(&keys, Place::Backup);
        assert_eq!(backup.first().map(|r| r.key.as_str()), Some("Ctrl+Enter"));
        assert!(key_rows(&keys, Place::Settings).0.is_empty());
    }

    #[test]
    fn keys_are_named_like_the_shortcut_list() {
        assert_eq!(key_name("1", true, false, false, false), "Ctrl+1");
        assert_eq!(key_name("k", true, false, false, false), "Ctrl+K");
        assert_eq!(key_name("?", false, true, false, false), "Shift+?");
        assert_eq!(key_name("E", true, true, false, false), "Ctrl+Shift+E");
        assert_eq!(key_name(&char::from(Key::F11).to_string(), false, false, false, false), "F11");
        assert_eq!(key_name(&char::from(Key::LeftArrow).to_string(), false, false, true, false), "Alt+Left");
        assert_eq!(key_name(&char::from(Key::Control).to_string(), true, false, false, false), "");
    }
}
