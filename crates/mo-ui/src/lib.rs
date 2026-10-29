//! The window shell and its screens. The screens themselves are in `ui/*.slint`; this file fills them from `mo-app`
//! (the look, the places, the shortcut list) and passes clicks and keys back.

slint::include_modules!();

pub mod cards;
pub mod note;
pub mod watchdog;

use std::sync::atomic::{AtomicUsize, Ordering};

use mo_app::activity::Feed;
use mo_app::log;
use mo_app::places::Place;
use mo_app::shortcuts::{APP_ACTIONS, Shortcuts};
use mo_app::themes::Look;
use note::open_note;
use slint::platform::Key;
use slint::{Color, ComponentHandle, ModelRc, VecModel, Weak};

/// The place on show, kept where the freeze watchdog can read it without asking the window.
static PAGE: AtomicUsize = AtomicUsize::new(0);

/// The name of the place on show, safe to read from any thread.
pub fn page_name() -> &'static str {
    Place::from_index(PAGE.load(Ordering::Relaxed)).map_or("?", Place::name)
}

/// Builds the main window in a look, opening on a place. Jobs report to `feed`, and its cards show bottom right.
pub fn new_app(look: &Look, shortcuts: Shortcuts, start: Place, feed: Feed) -> Result<AppWindow, slint::PlatformError> {
    let app = AppWindow::new()?;
    apply_look(&app, look);
    let places: Vec<PlaceInfo> =
        Place::ALL.iter().map(|p| PlaceInfo { name: p.name().into(), coming: coming(*p).into() }).collect();
    let settings = Place::Settings.index();
    app.set_rail(ModelRc::new(VecModel::from(places[..settings].to_vec())));
    app.set_places(ModelRc::new(VecModel::from(places)));
    app.set_settings_place(settings as i32);
    go_to(&app, start.index() as i32);
    app.set_key_rows(ModelRc::new(VecModel::from(key_rows(&shortcuts))));

    let weak = app.as_weak();
    app.on_place_chosen(move |i| {
        if let Some(app) = weak.upgrade() {
            go_to(&app, i);
        }
    });
    let weak = app.as_weak();
    app.on_key(move |text, ctrl, shift, alt, meta| {
        let key = key_name(&text, ctrl, shift, alt, meta);
        if key == "F12" {
            return weak.upgrade().is_some_and(|app| {
                open_note(&app);
                true
            });
        }
        shortcuts.action_for(&key).is_some_and(|id| run(id, &weak))
    });

    cards::connect(&app, &feed);
    note::connect(&app, &feed);
    Ok(app)
}

/// Shows a place, and notes it for the log and the watchdog.
fn go_to(app: &AppWindow, index: i32) {
    let Some(place) = usize::try_from(index).ok().and_then(Place::from_index) else { return };
    if PAGE.swap(place.index(), Ordering::Relaxed) != place.index() || app.get_place() != index {
        log::info("Page", place.name());
    }
    app.set_place(index);
}

/// The key sheet's lines: every app-wide action with the key it has now, and F12, which the app keeps for itself.
fn key_rows(shortcuts: &Shortcuts) -> Vec<KeyRow> {
    // Keys whose action isn't built yet say so, rather than looking broken.
    let mut rows: Vec<KeyRow> = APP_ACTIONS
        .iter()
        .map(|a| {
            let what = if handles(a.id) { a.what.to_owned() } else { format!("{} (coming)", a.what) };
            KeyRow { key: shortcuts.key_of(a.id).into(), what: what.into() }
        })
        .collect();
    rows.push(KeyRow { key: "F12".into(), what: "Write a note for Claude about what's on screen".into() });
    rows
}

/// Puts a look's colours and fonts into the screens' `Theme` tokens.
pub fn apply_look(app: &AppWindow, look: &Look) {
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
}

fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// What a place's page will hold, shown until the page is built.
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
        Place::Settings => "General, Folders, DAWs, Shortcuts and Developer. Filled in as each page arrives.",
    }
}

/// The app-wide actions `run` does something for so far (besides going to a place).
const HANDLED: [&str; 5] = ["app.prevPage", "app.nextPage", "app.help", "app.helpF1", "app.fullScreen"];

fn handles(id: &str) -> bool {
    HANDLED.contains(&id) || Place::ALL.iter().any(|p| p.shortcut_id() == id)
}

/// Runs an app-wide action. Returns whether it did anything, so an unused key can go on to the page.
fn run(id: &str, weak: &Weak<AppWindow>) -> bool {
    let Some(app) = weak.upgrade() else { return false };
    if let Some(p) = Place::ALL.into_iter().find(|p| p.shortcut_id() == id) {
        go_to(&app, p.index() as i32);
        return true;
    }
    let n = Place::ALL.len() as i32;
    match id {
        "app.prevPage" => go_to(&app, (app.get_place() + n - 1) % n),
        "app.nextPage" => go_to(&app, (app.get_place() + 1) % n),
        "app.help" | "app.helpF1" => app.set_sheet_open(!app.get_sheet_open()),
        "app.fullScreen" => {
            let w = app.window();
            w.set_fullscreen(!w.is_fullscreen());
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
