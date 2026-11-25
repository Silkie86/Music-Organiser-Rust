//! The widths of the plugin's parts you can drag wider or narrower ([`mo_app::widths::PLUGIN_PARTS`]): the filter
//! column, the folder tree and Chord check's takes. Kept in a small file of the plugin's own in the app's data folder
//! (the plugin never writes the app's settings), shared by every copy of the plugin: a window opened later, in any
//! project, comes up as you left the last one.

use std::path::PathBuf;
use std::sync::Mutex;

use mo_app::widths::{self, Widths};
use slint::ComponentHandle;

use crate::threads::lock;
use crate::ui::Panel;

/// What every copy of the plugin chose, as the file has it (read once, the first time a window opens).
static CHOSEN: Mutex<Option<Widths>> = Mutex::new(None);
/// Held while the file is written (every copy of the plugin writes the same file).
static WRITING: Mutex<()> = Mutex::new(());

/// The file, or none in the tests (they never write the data folder, rule 11).
fn file() -> Option<PathBuf> {
    (!cfg!(test)).then(|| mo_app::paths::data_dir().join(widths::PLUGIN_FILE))
}

/// The widths kept, read from the file the first time.
fn chosen() -> Widths {
    lock(&CHOSEN)
        .get_or_insert_with(|| {
            let text = file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
            Widths::read(text.trim())
        })
        .clone()
}

/// Gives the panel its parts' widths and wires its seams.
pub fn connect(panel: &Panel) {
    let kept = chosen();
    for part in widths::PLUGIN_PARTS {
        give(panel, part.name, kept.width(part.name));
    }
    let data = panel.global::<crate::ui::Widths>();
    // A name not in the table (a slip in the panel; a test catches it) gets wide limits, as a seam with no part has.
    data.on_least(|name| widths::find(&name).map_or(160.0, |p| p.least));
    data.on_most(|name| widths::find(&name).map_or(100_000.0, |p| p.most));
    data.on_usual(|name| widths::find(&name).map_or(0.0, |p| p.usual));
    data.on_resized(|name, width| {
        let Some(part) = widths::find(&name) else { return };
        let got = change(|w| w.set(part.name, width));
        crate::log::line("Widths", &format!("{} dragged to {width:.0}, kept as {got:.0}", part.name));
    });
    let weak = panel.as_weak();
    data.on_reset(move |name| {
        let Some(part) = widths::find(&name) else { return };
        let width = change(|w| w.reset(part.name));
        crate::log::line("Widths", &format!("{} back to its usual {width:.0}", part.name));
        if let Some(panel) = weak.upgrade() {
            give(&panel, part.name, width);
        }
    });
}

/// Changes the kept widths and writes the file, off the window's thread (a slow disk never holds a frame).
fn change(how: impl FnOnce(&mut Widths) -> f32) -> f32 {
    let (got, changed) = {
        let mut chosen = lock(&CHOSEN);
        let widths = chosen.get_or_insert_with(Widths::default);
        let before = widths.value();
        let got = how(widths);
        (got, widths.value() != before)
    };
    if let (true, Some(file)) = (changed, file()) {
        std::thread::spawn(move || {
            // One write at a time, each of the widths as they are by then: the newest always lands last.
            let _writing = lock(&WRITING);
            let value = lock(&CHOSEN).as_ref().map(Widths::value).unwrap_or_default();
            write(&file, &value);
        });
    }
    got
}

/// Writes the file by a copy beside it and a rename, so a cut-off write never leaves half a file; an empty value
/// takes the file away (nothing chosen: nothing kept).
fn write(file: &std::path::Path, value: &str) {
    let done = if value.is_empty() {
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        // Named as the app's start-up sweep knows a cut-off write of it (`mo_app::tidy`).
        let mut next = file.as_os_str().to_owned();
        next.push(mo_app::backup::PART_ENDING);
        let next = PathBuf::from(next);
        std::fs::write(&next, value).and_then(|()| std::fs::rename(&next, file))
    };
    if let Err(e) = done {
        crate::log::line("Widths", &format!("Couldn't keep the widths in {}: {e}", file.display()));
    }
}

/// Sets the panel's width for the part called `name`.
fn give(panel: &Panel, name: &str, width: f32) {
    let data = panel.global::<crate::ui::Widths>();
    match name {
        "plugin.filters" => data.set_plugin_filters(width),
        "plugin.folders" => data.set_plugin_folders(width),
        "plugin.takes" => data.set_plugin_takes(width),
        _ => debug_assert!(false, "the plugin has no part called {name}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_plugin_part_has_a_line_in_give() {
        let source = include_str!("widths.rs");
        for part in mo_app::widths::PLUGIN_PARTS {
            assert!(source.contains(&format!("\"{}\" =>", part.name)), "{} has no line in give", part.name);
        }
    }

    #[test]
    fn every_seam_in_the_panel_names_a_plugin_part() {
        let panel = include_str!("../ui/panel.slint");
        let mut seen = 0;
        for line in panel.lines().map(str::trim) {
            let Some(rest) = line.strip_prefix("part:") else { continue };
            for name in rest.split('"').skip(1).step_by(2).filter(|n| !n.is_empty()) {
                seen += 1;
                assert!(name.starts_with("plugin.") && mo_app::widths::find(name).is_some(), "{name}");
            }
        }
        assert!(seen >= mo_app::widths::PLUGIN_PARTS.len(), "found only {seen} seams in panel.slint");
    }
}
