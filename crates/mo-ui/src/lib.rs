//! The window shell and its screens. The screens themselves are in `ui/*.slint`; this file fills them from `mo-app`
//! (the look, the places, the shortcut list) and passes clicks and keys back.

slint::include_modules!();

use mo_app::places::Place;
use mo_app::shortcuts::Shortcuts;
use mo_app::themes::Look;
use slint::platform::Key;
use slint::{Color, ComponentHandle, ModelRc, VecModel, Weak};

/// Builds the main window in a look, opening on a place.
pub fn new_app(look: &Look, shortcuts: Shortcuts, start: Place) -> Result<AppWindow, slint::PlatformError> {
    let app = AppWindow::new()?;
    apply_look(&app, look);
    let places: Vec<PlaceInfo> =
        Place::ALL.iter().map(|p| PlaceInfo { name: p.name().into(), coming: coming(*p).into() }).collect();
    app.set_places(ModelRc::new(VecModel::from(places)));
    app.set_place(start.index() as i32);

    let weak = app.as_weak();
    app.on_place_chosen(move |i| {
        if let Some(app) = weak.upgrade() {
            app.set_place(i);
        }
    });
    let weak = app.as_weak();
    app.on_key(move |text, ctrl, shift, alt, meta| {
        let key = key_name(&text, ctrl, shift, alt, meta);
        shortcuts.action_for(&key).is_some_and(|id| run(id, &weak))
    });
    Ok(app)
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
        Place::Playground => "The small MIDI DAW, with the MIDI browser folded in. Built in phase 6.",
        Place::Plugins => "Every plugin your songs use, and what to install to open more of them. Built in phase 4.",
        Place::Space => {
            "The biggest ways to free disk space, across plugins, songs, spare copies and duplicates. Built in phase 4."
        }
        Place::Settings => "General, Folders, DAWs, Shortcuts and Developer. Filled in as each page arrives.",
    }
}

/// Runs an app-wide action. Returns whether it did anything, so an unused key can go on to the page.
fn run(id: &str, weak: &Weak<AppWindow>) -> bool {
    let Some(app) = weak.upgrade() else { return false };
    if let Some(p) = Place::ALL.into_iter().find(|p| p.shortcut_id() == id) {
        app.set_place(p.index() as i32);
        return true;
    }
    let n = Place::ALL.len() as i32;
    match id {
        "app.prevPage" => app.set_place((app.get_place() + n - 1) % n),
        "app.nextPage" => app.set_place((app.get_place() + 1) % n),
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
    ];
    let name = match named.iter().find(|(k, _)| char::from(*k) == c) {
        Some((_, n)) => (*n).to_owned(),
        None if c == ' ' => "Space".to_owned(),
        // Modifier keys on their own are not shortcuts.
        None if c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c) => return String::new(),
        None => c.to_uppercase().collect(),
    };
    // A symbol typed with Shift ("?" is Shift+/ on most keyboards) is its own key.
    let shift = shift && !(name.chars().count() == 1 && !c.is_alphanumeric());
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
    fn keys_are_named_like_the_shortcut_list() {
        assert_eq!(key_name("1", true, false, false, false), "Ctrl+1");
        assert_eq!(key_name("k", true, false, false, false), "Ctrl+K");
        assert_eq!(key_name("?", false, true, false, false), "?");
        assert_eq!(key_name("E", true, true, false, false), "Ctrl+Shift+E");
        assert_eq!(key_name(&char::from(Key::F11).to_string(), false, false, false, false), "F11");
        assert_eq!(key_name(&char::from(Key::LeftArrow).to_string(), false, false, true, false), "Alt+Left");
        assert_eq!(key_name(&char::from(Key::Control).to_string(), true, false, false, false), "");
    }
}
