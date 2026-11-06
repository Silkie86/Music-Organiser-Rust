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
pub mod drag;
pub mod files;
pub mod filters;
pub mod finder;
pub mod gpu;
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
    APP_ACTIONS, Action, MIDI_ACTIONS, PLUGINS_ACTIONS, SONGS_ACTIONS, SOUNDS_ACTIONS, SPACE_ACTIONS, Shortcuts,
    repeats, what_of,
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
        let _doing = watchdog::doing("a key");
        let key = key_name(&text, ctrl, shift, alt, meta);
        // Settings › Shortcuts listening for a new key: this press is the key, not a shortcut.
        if settings::listening()
            && let Some(app) = weak.upgrade()
        {
            return repeat || settings::key_pressed(&app, &key);
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
            && let Some(id) = shortcuts.page_action_for(&key, SONGS_ACTIONS)
            && songs::has_pick()
        {
            return repeat || songs::run_action(&app, id);
        }
        if let Some(app) = weak.upgrade()
            && app.get_place() == Place::Plugins.index() as i32
            && let Some(id) = shortcuts.page_action_for(&key, PLUGINS_ACTIONS)
            && plugins::has_pick()
        {
            return repeat || plugins::run_action(&app, id);
        }
        if let Some(app) = weak.upgrade()
            && app.get_place() == Place::Presets.index() as i32
            && let Some(id) = shortcuts.page_action_for(&key, presets::page_actions())
            && (repeat || presets::run_action(&app, id))
        {
            return true;
        }
        if let Some(app) = weak.upgrade()
            && app.get_place() == Place::Sounds.index() as i32
            && let Some(id) = shortcuts.page_action_for(&key, SOUNDS_ACTIONS)
            && (repeat || sounds::run_action(&app, id))
        {
            return true;
        }
        if let Some(app) = weak.upgrade()
            && app.get_place() == Place::Midi.index() as i32
            && let Some(id) = shortcuts.page_action_for(&key, MIDI_ACTIONS)
            && (repeat || midi::run_action(&app, id))
        {
            return true;
        }
        if let Some(app) = weak.upgrade()
            && app.get_place() == Place::Space.index() as i32
            && let Some(id) = shortcuts.page_action_for(&key, SPACE_ACTIONS)
            && (repeat || space::run_action(&app, id))
        {
            return true;
        }
        let Some(id) = shortcuts.action_for(&key) else { return false };
        // A key held down did its job once (unless it's one that steps), and the page behind doesn't get it either.
        if repeat && !repeats(id) {
            return handles(id);
        }
        run(id, &weak, &key_feed)
    });

    cards::connect(&app, &feed);
    note::connect(&app, &feed);
    Ok(app)
}

/// Ctrl+K: the pages, Settings tabs and actions to jump to, and how to open them. Songs, sounds, presets, MIDI parts
/// and plugins join from their own pages.
fn connect_finder(app: &AppWindow, shortcuts: &Shortcuts, feed: &Feed) {
    finder::connect(app);
    finder_pages(app, shortcuts);
    finder::add_source(app, "settings", std::sync::Arc::new(mo_app::finder::settings()));
    finder::on_open(mo_app::finder::Kind::Setting, |app, hit, _| {
        let Some(tab) = mo_app::settings::Tab::ALL.into_iter().find(|t| t.name() == hit.id) else { return false };
        go_to(app, Place::Settings.index() as i32);
        settings::show_tab(app, tab);
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
                Place::Presets => presets::run_action(app, id),
                Place::Midi => midi::run_action(app, id),
                Place::Space => space::run_action(app, id),
                Place::Backup | Place::Settings => false,
            };
        }
        run(id, &app.as_weak(), &feed)
    });
}

/// The places Ctrl+K offers, with their keys, and the Sounds page's views.
fn finder_pages(app: &AppWindow, shortcuts: &Shortcuts) {
    let views: [(Place, &[&str]); 1] = [(Place::Sounds, &sounds::VIEWS)];
    finder::add_source(app, "pages", std::sync::Arc::new(mo_app::finder::pages(shortcuts, &views)));
}

/// The actions Ctrl+K offers on a place: its page's own, then the app-wide ones that do something so far (not Ctrl+K
/// itself, and the help key once).
fn finder_actions(app: &AppWindow, shortcuts: &Shortcuts, place: Place) {
    let page = page_actions(place);
    let wide = APP_ACTIONS.iter().filter(|a| handles(a.id) && !matches!(a.id, "app.jump" | "app.helpF1"));
    let actions = mo_app::finder::actions(shortcuts, page.iter().chain(wide));
    finder::add_source(app, "actions", std::sync::Arc::new(actions));
}

/// The keys in use now.
pub fn shortcuts() -> Shortcuts {
    KEYS.with(|k| k.borrow().as_deref().cloned()).unwrap_or_default()
}

/// A key was changed in Settings: everything that shows or runs a key takes the new list at once (the rail's tooltips,
/// the key sheet, the Songs buttons, the Sounds and Presets cards and menus, the Space buttons). Every page's keys run
/// from this one list.
pub fn keys_changed(app: &AppWindow, shortcuts: &Shortcuts) {
    KEYS.with(|k| *k.borrow_mut() = Some(std::rc::Rc::new(shortcuts.clone())));
    show_places(app, shortcuts);
    show_button_keys(app, shortcuts);
    sounds::show_keys(app, shortcuts);
    presets::show_keys(app, shortcuts);
    space::show(app);
    if let Some(place) = place() {
        app.set_key_rows(ModelRc::new(VecModel::from(key_rows(shortcuts, place))));
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

/// The Songs page's buttons' tooltips: what each does and its key, from the one shortcut list (rule 15).
fn show_button_keys(app: &AppWindow, shortcuts: &Shortcuts) {
    let action = |id: &str| ActionKey { what: what_of(id).into(), key: shortcuts.key_of(id).into() };
    let data = app.global::<SongsData>();
    data.set_open_key(action("projects.open"));
    data.set_open_own_key(action("projects.openOwn"));
    data.set_reveal_key(action("projects.reveal"));
    data.set_copy_key(action("projects.copyPath"));
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
    if before == Place::Presets.index() && place != Place::Presets {
        presets::left(app);
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
    if place == Place::Presets {
        presets::shown(app);
    }
    refresh_keys(app);
}

/// Fills the key sheet for the place on show (and, on Presets, the section on show).
pub(crate) fn refresh_keys(app: &AppWindow) {
    let Some(place) = usize::try_from(app.get_place()).ok().and_then(Place::from_index) else { return };
    KEYS.with(|k| {
        if let Some(shortcuts) = k.borrow().as_ref() {
            app.set_key_rows(ModelRc::new(VecModel::from(key_rows(shortcuts, place))));
            finder_actions(app, shortcuts, place);
        }
    });
}

/// A page's own actions: what its keys do, the key sheet lists and Ctrl+K offers on it.
fn page_actions(place: Place) -> &'static [Action] {
    match place {
        Place::Songs => SONGS_ACTIONS,
        Place::Sounds => SOUNDS_ACTIONS,
        Place::Plugins => PLUGINS_ACTIONS,
        Place::Presets => presets::page_actions(),
        Place::Midi => MIDI_ACTIONS,
        Place::Space => SPACE_ACTIONS,
        Place::Backup | Place::Settings => &[],
    }
}

/// The key sheet's lines: the page's own actions, then every app-wide action, with the key each has now, and F12,
/// which the app keeps for itself.
fn key_rows(shortcuts: &Shortcuts, place: Place) -> Vec<KeyRow> {
    let page = page_actions(place);
    let mut rows: Vec<KeyRow> =
        page.iter().map(|a| KeyRow { key: shortcuts.key_of(a.id).into(), what: a.what.into() }).collect();
    // Keys whose action isn't built yet say so, rather than looking broken.
    rows.extend(APP_ACTIONS.iter().map(|a| {
        let what = if handles(a.id) { a.what.to_owned() } else { format!("{} (coming)", a.what) };
        KeyRow { key: shortcuts.key_of(a.id).into(), what: what.into() }
    }));
    rows.push(KeyRow { key: "F12".into(), what: "Write a note for Claude about what's on screen".into() });
    rows
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

/// The app-wide actions `run` does something for so far (besides going to a place).
const HANDLED: [&str; 8] = [
    "app.jump",
    "app.prevPage",
    "app.nextPage",
    "app.help",
    "app.helpF1",
    "app.fullScreen",
    "app.play",
    "app.recycleBin",
];

fn handles(id: &str) -> bool {
    HANDLED.contains(&id) || Place::ALL.iter().any(|p| p.shortcut_id() == id)
}

/// Runs an app-wide action. Returns whether it did anything, so an unused key can go on to the page.
fn run(id: &str, weak: &Weak<AppWindow>, feed: &Feed) -> bool {
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
                || (place == Place::Presets.index() as i32 && presets::toggle_play(&app));
        }
        _ => {
            debug_assert!(!HANDLED.contains(&id), "{id} is in HANDLED but run() doesn't do it");
            return false;
        }
    }
    debug_assert!(handles(id), "run() does {id}: add it to HANDLED so the key sheet doesn't say (coming)");
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
    fn every_handled_action_is_on_the_list() {
        for id in HANDLED {
            assert!(APP_ACTIONS.iter().any(|a| a.id == id), "{id} isn't in the shortcut list");
        }
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
