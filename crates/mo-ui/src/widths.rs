//! The widths of the parts of the pages you can drag wider or narrower ([`mo_app::widths`]): given to the pages at the
//! start, and kept in the app's settings each time you let go of a seam (or double-click it for the usual width).

use std::cell::RefCell;

use mo_app::log;
use mo_app::widths::{self, Widths};
use slint::ComponentHandle;

use crate::{AppWindow, MidiData, PluginsData, PresetsData, SoundsData, SpaceData};

/// The limits a seam has when its part isn't in the table: PanelDivider's own, for a seam with no part.
const UNKNOWN_LEAST: f32 = 320.0;
const UNKNOWN_MOST: f32 = 100_000.0;

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
    // A name not in the table (a slip in a page) gets the seam's own limits, as a seam with no part has.
    data.on_least(|name| widths::find(&name).map_or(UNKNOWN_LEAST, |p| p.least));
    data.on_most(|name| widths::find(&name).map_or(UNKNOWN_MOST, |p| p.most));
    data.on_usual(|name| widths::find(&name).map_or(0.0, |p| p.usual));
    data.on_resized(|name, width| {
        let Some(part) = widths::find(&name) else {
            log::warning("Widths", &format!("A seam for {name}, which isn't in the table, was dragged"));
            return;
        };
        let got = WIDTHS.with(|w| w.borrow_mut().set(part.name, width));
        log::info("Widths", &format!("{} dragged to {width:.0}, kept as {got:.0}", part.name));
        keep();
    });
    let weak = app.as_weak();
    data.on_reset(move |name| {
        let Some(part) = widths::find(&name) else {
            log::warning("Widths", &format!("A seam for {name}, which isn't in the table, was double-clicked"));
            return;
        };
        let width = WIDTHS.with(|w| w.borrow_mut().reset(part.name));
        if part.usual > 0.0 {
            log::info("Widths", &format!("{} back to its usual {width:.0}", part.name));
        } else {
            log::info("Widths", &format!("{} back to its usual width, worked out by the page", part.name));
        }
        if let Some(app) = weak.upgrade() {
            give(&app, part.name, width);
        }
        keep();
    });
}

/// Keeps the widths in the app's settings (written off this thread; nothing is written when they didn't change).
fn keep() {
    let value = WIDTHS.with(|w| w.borrow().value());
    // Nothing kept and nothing to keep: no empty setting written into the file.
    if value.is_empty() && crate::settings::value(widths::KEY).is_none_or(|v| v.is_empty()) {
        return;
    }
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
        "sounds.filters" => app.global::<crate::Widths>().set_sounds_filters(width),
        "presets.filters" => app.global::<crate::Widths>().set_presets_filters(width),
        "midi.filters" => app.global::<crate::Widths>().set_midi_filters(width),
        "plugins.filters" => app.global::<crate::Widths>().set_plugins_filters(width),
        "songs.filters" => app.global::<crate::Widths>().set_songs_filters(width),
        "sounds.folders" => app.global::<crate::Widths>().set_sounds_folders(width),
        "songs.list" => app.global::<crate::Widths>().set_songs_list(width),
        "curves.list" => app.global::<crate::Widths>().set_curves_list(width),
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

    #[test]
    fn every_seam_names_a_part_in_the_table() {
        // A seam's `part: "…"` in a page must be in the table, or its width is never kept.
        let pages = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/pages");
        let mut seen = 0;
        for entry in std::fs::read_dir(&pages).into_iter().flatten().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            for line in text.lines().map(str::trim) {
                let Some(name) = line.strip_prefix("part: \"").and_then(|r| r.strip_suffix("\";")) else { continue };
                seen += 1;
                assert!(
                    mo_app::widths::find(name).is_some(),
                    "{} names {name}, which isn't in the table",
                    entry.path().display()
                );
            }
        }
        assert!(seen >= mo_app::widths::PARTS.len(), "found only {seen} seams in {}", pages.display());
    }
}
