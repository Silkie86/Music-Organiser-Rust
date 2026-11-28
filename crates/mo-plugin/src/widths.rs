//! The widths of the plugin's parts you can drag wider or narrower ([`mo_app::widths::PLUGIN_PARTS`]): the filter
//! column, the folder tree, Chord check's takes, and how high the Edit strip is. Kept in a small file of the plugin's own in the app's data folder
//! (the plugin never writes the app's settings), shared by every copy of the plugin: a window opened later, in any
//! project, comes up as you left the last one.
//!
//! Two DAWs running side by side (or Bitwig's plugin hosts) each have a copy of the plugin's code of their own, with
//! the widths as that copy last saw them. So the file is read again as each window opens, and a part dragged is written
//! into what the file has then: never over a part another program changed since (the 26 Nov review's WI-3).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use mo_app::widths::{self, Widths};
use slint::ComponentHandle;

use crate::threads::lock;
use crate::ui::Panel;

/// The widths as this program has them: the file's as last read or written, with the parts dragged here since.
#[derive(Default)]
struct Chosen {
    widths: Widths,
    /// The parts dragged (or put back) here and not written to the file yet: the next write takes them there.
    unwritten: Vec<&'static str>,
}

impl Chosen {
    /// Takes in the file's widths `on_file`, but for the parts dragged here and not written yet; whether anything
    /// changed.
    fn take_in(&mut self, on_file: &Widths) -> bool {
        let before = self.widths.clone();
        for part in widths::PLUGIN_PARTS {
            if !self.unwritten.contains(&part.name) {
                self.widths.set(part.name, on_file.width(part.name));
            }
        }
        self.widths != before
    }
}

static CHOSEN: Mutex<Option<Chosen>> = Mutex::new(None);
/// Goes up each time a read of the file changes [`CHOSEN`]: each open window then gives its panel the widths again
/// ([`tick`]).
static READ: AtomicU64 = AtomicU64::new(0);
/// Held while the file is read and written again (every copy of the plugin in this program writes the same file).
static WRITING: Mutex<()> = Mutex::new(());

/// Works on the widths this program has.
fn with_chosen<R>(work: impl FnOnce(&mut Chosen) -> R) -> R {
    work(lock(&CHOSEN).get_or_insert_with(Chosen::default))
}

/// The file, or none in the tests (they never write the data folder, rule 11).
fn file() -> Option<PathBuf> {
    (!cfg!(test)).then(|| mo_app::paths::data_dir().join(widths::PLUGIN_FILE))
}

/// Gives the panel its parts' widths and wires its seams. The widths it has at once are this program's; the file is
/// read again off the window's thread (the data drive may be asleep), and what it says comes to the panel through
/// [`tick`]. The number to hand [`tick`] is returned.
pub fn connect(panel: &Panel) -> u64 {
    let seen = READ.load(Ordering::Acquire);
    give_all(panel);
    if let Some(file) = file()
        && !crate::threads::background("mo-plugin-widths", move || read_again(&file))
    {
        crate::log::line("Widths", "Couldn't start reading the widths again: the ones read before are shown");
    }
    let data = panel.global::<crate::ui::Widths>();
    // A name not in the table (a slip in the panel; a test catches it) gets wide limits, as a seam with no part has.
    data.on_least(|name| widths::find(&name).map_or(160.0, |p| p.least));
    data.on_most(|name| widths::find(&name).map_or(100_000.0, |p| p.most));
    data.on_usual(|name| widths::find(&name).map_or(0.0, |p| p.usual));
    data.on_resized(|name, width| {
        let Some(part) = widths::find(&name) else { return };
        let got = change(part.name, |w| w.set(part.name, width));
        crate::log::line("Widths", &format!("{} dragged to {width:.0}, kept as {got:.0}", part.name));
    });
    let weak = panel.as_weak();
    data.on_reset(move |name| {
        let Some(part) = widths::find(&name) else { return };
        let width = change(part.name, |w| w.reset(part.name));
        crate::log::line("Widths", &format!("{} back to its usual {width:.0}", part.name));
        if let Some(panel) = weak.upgrade() {
            give(&panel, part.name, width);
        }
    });
    seen
}

/// Gives the panel the widths again when a read of the file changed them since it last had them (`seen`, which
/// [`connect`] started).
pub fn tick(panel: &Panel, seen: &Cell<u64>) {
    let read = READ.load(Ordering::Acquire);
    if read != seen.get() {
        seen.set(read);
        give_all(panel);
    }
}

/// Changes the widths this program has (the part called `name`) and writes the change to the file, off the window's
/// thread at low priority (a slow disk never holds a frame, and the DAW's own work comes first).
fn change(name: &'static str, how: impl FnOnce(&mut Widths) -> f32) -> f32 {
    let (got, changed) = with_chosen(|chosen| {
        let before = chosen.widths.value();
        let got = how(&mut chosen.widths);
        let changed = chosen.widths.value() != before;
        if changed && !chosen.unwritten.contains(&name) {
            chosen.unwritten.push(name);
        }
        (got, changed)
    });
    if let (true, Some(file)) = (changed, file())
        && !crate::threads::background("mo-plugin-widths", move || write_now(&file))
    {
        crate::log::line("Widths", "Couldn't start writing the widths: the next change writes this one too");
    }
    got
}

/// Reads the file again and takes in what another program changed.
fn read_again(file: &Path) {
    let _writing = lock(&WRITING);
    let Some(on_file) = read(file) else { return };
    if with_chosen(|chosen| chosen.take_in(&on_file)) {
        READ.fetch_add(1, Ordering::AcqRel);
    }
}

/// Writes the parts dragged here into what the file has now, then takes in the rest of it. One write at a time, and
/// each takes every part dragged by then: the newest always lands last.
fn write_now(file: &Path) {
    let _writing = lock(&WRITING);
    let on_file = read(file);
    let (names, written) = {
        let mut chosen = lock(&CHOSEN);
        let chosen = chosen.get_or_insert_with(Chosen::default);
        let names = std::mem::take(&mut chosen.unwritten);
        if names.is_empty() {
            // A write after the change that asked for this one took it already.
            return;
        }
        // A file that couldn't be read just now: what this program has, whole, as before.
        let written = match on_file {
            Some(on_file) => merged(on_file, &chosen.widths, &names),
            None => chosen.widths.clone(),
        };
        (names, written)
    };
    match write(file, &written.value()) {
        Ok(()) => {
            if with_chosen(|chosen| chosen.take_in(&written)) {
                READ.fetch_add(1, Ordering::AcqRel);
            }
        }
        Err(e) => {
            crate::log::line("Widths", &format!("Couldn't keep the widths in {}: {e}", file.display()));
            // Tried again with the next change.
            with_chosen(|chosen| {
                for name in names {
                    if !chosen.unwritten.contains(&name) {
                        chosen.unwritten.push(name);
                    }
                }
            });
        }
    }
}

/// What's written: the file's widths `on_file`, with the parts called `names` as this program has them (`mine`).
fn merged(mut on_file: Widths, mine: &Widths, names: &[&str]) -> Widths {
    for name in names {
        // At its usual width, a part isn't kept: `set` takes it out.
        on_file.set(name, mine.width(name));
    }
    on_file
}

/// The widths in the file (none chosen when there's no file); none when it can't be read just now.
fn read(file: &Path) -> Option<Widths> {
    match std::fs::read_to_string(file) {
        Ok(text) => Some(Widths::read(text.trim())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Widths::default()),
        Err(e) => {
            crate::log::line("Widths", &format!("Couldn't read the widths in {}: {e}", file.display()));
            None
        }
    }
}

/// Writes the file whole or not at all, through a temporary name of this program's own beside it that the app's
/// start-up sweep knows (`mo_app::backup::write_whole`): two DAWs writing at once never share one. An empty value
/// takes the file away (nothing chosen: nothing kept).
fn write(file: &Path, value: &str) -> std::io::Result<()> {
    if value.is_empty() {
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        mo_app::backup::write_whole(file, value)
    }
}

/// Gives the panel every part's width as this program has it.
fn give_all(panel: &Panel) {
    let chosen = with_chosen(|chosen| chosen.widths.clone());
    for part in widths::PLUGIN_PARTS {
        give(panel, part.name, chosen.width(part.name));
    }
}

/// Sets the panel's width for the part called `name`.
fn give(panel: &Panel, name: &str, width: f32) {
    let data = panel.global::<crate::ui::Widths>();
    match name {
        "plugin.filters" => data.set_plugin_filters(width),
        "plugin.folders" => data.set_plugin_folders(width),
        "plugin.takes" => data.set_plugin_takes(width),
        // The strip is the app's own, and reads its height where the app's page keeps it.
        "plugin.edit" => data.set_sounds_edit(width),
        _ => debug_assert!(false, "the plugin has no part called {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_part_dragged_here_is_written_into_what_the_file_has_now() {
        // FL dragged the filter column to 260 and wrote it; Bitwig, which read the file before, drags the takes to 300.
        let on_file = Widths::read("plugin.filters=260");
        let mut mine = Widths::default();
        mine.set("plugin.takes", 300.0);
        let written = merged(on_file, &mine, &["plugin.takes"]);
        assert_eq!(written.value(), "plugin.filters=260;plugin.takes=300");
        // Put back to its usual width here: it goes from the file, FL's stays.
        mine.reset("plugin.takes");
        assert_eq!(merged(written, &mine, &["plugin.takes"]).value(), "plugin.filters=260");
        // Read again, FL's width comes in, but not over a part dragged here and not written yet.
        let mut chosen = Chosen { widths: Widths::read("plugin.folders=400"), unwritten: vec!["plugin.folders"] };
        assert!(chosen.take_in(&Widths::read("plugin.filters=260;plugin.folders=200")));
        assert_eq!(chosen.widths.value(), "plugin.filters=260;plugin.folders=400");
        assert!(!chosen.take_in(&Widths::read("plugin.filters=260")));
        // Once written, the file's word goes for that part too.
        chosen.unwritten.clear();
        assert!(chosen.take_in(&Widths::read("plugin.filters=260")));
        assert_eq!(chosen.widths.value(), "plugin.filters=260");
    }
}
