//! The widths of the parts of the pages you can drag wider or narrower ([`mo_app::widths`]): given to the pages at the
//! start, and kept in the app's settings each time you let go of a seam (or double-click it for the usual width).

use std::cell::RefCell;

use mo_app::widths::{self, Widths};
use slint::ComponentHandle;

use crate::{AppWindow, MidiData, PluginsData, PresetsData, SoundsData, SpaceData};

thread_local! {
    static WIDTHS: RefCell<Widths> = RefCell::new(Widths::default());
}

/// Reads the kept widths, gives each page its parts' widths, and wires the seams. After `settings::connect`.
pub fn connect(app: &AppWindow) {
    let kept = Widths::read(&crate::settings::value(widths::KEY).unwrap_or_default());
    for part in widths::PARTS {
        give(app, part.name, kept.width(part.name));
    }
    WIDTHS.with(|w| *w.borrow_mut() = kept);
    let data = app.global::<crate::Widths>();
    data.on_least(|name| widths::find(&name).map_or(0.0, |p| p.least));
    data.on_most(|name| widths::find(&name).map_or(100_000.0, |p| p.most));
    data.on_resized(|name, width| {
        WIDTHS.with(|w| w.borrow_mut().set(&name, width));
        keep();
    });
    let weak = app.as_weak();
    data.on_reset(move |name| {
        let width = WIDTHS.with(|w| w.borrow_mut().reset(&name));
        if let Some(app) = weak.upgrade() {
            give(&app, &name, width);
        }
        keep();
    });
}

/// Keeps the widths in the app's settings (written off this thread; nothing is written when they didn't change).
fn keep() {
    let value = WIDTHS.with(|w| w.borrow().value());
    crate::settings::remember(widths::KEY, &value);
}

/// Sets the page's width for the part called `name`.
fn give(app: &AppWindow, name: &str, width: f32) {
    match name {
        "sounds.card" => app.global::<SoundsData>().set_panel_width(width),
        "presets.card" => app.global::<PresetsData>().set_panel_width(width),
        "midi.card" => app.global::<MidiData>().set_panel_width(width),
        "plugins.card" => app.global::<PluginsData>().set_panel_width(width),
        "space.card" => app.global::<SpaceData>().set_panel_width(width),
        _ => debug_assert!(false, "no page has a part called {name}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_part_has_a_page() {
        // `give` has a line for each part: a part added to the table without one would never get its width.
        let source = include_str!("widths.rs");
        for part in mo_app::widths::PARTS {
            assert!(source.contains(&format!("\"{}\" =>", part.name)), "{} has no line in give", part.name);
        }
    }
}
