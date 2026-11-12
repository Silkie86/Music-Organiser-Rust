//! The Presets tab: the preset files the app's Presets page lists, read-only, on a background thread.
//!
//! The plugin never writes the app's files. It looks in the folders the app's Presets page looks in (the plugins' and
//! Bitwig's usual ones, and every Bitwig folder a song of the library sits in, `mo_app::presets::preset_folders`),
//! and takes what the app's page kept of each file (`preset-files.db`) when the file hasn't changed since; the rest
//! are read now and not kept (`mo_presets::file_store::find_read_only`). The list, its filters and its orders are the
//! Presets page's own (`mo_app::presets`), so the two agree. A preset dragged out is its own file: nothing is copied
//! or written.
//!
//! Only the preset files for now: the sounds in songs' saves are a later step (dragging one out writes a file).
//!
//! Like the sound library, the presets are shared by every copy of the plugin in the DAW while any of them holds them.

use crate::threads::lock;
use mo_app::presets::{self as page, ChoiceCounts, FilterChips, Filters, Item, Order};
use mo_presets::file_store;
use mo_presets::library::{self as plib, Source};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

/// The orders the tab offers, with their words: as the files were found (by folder), or by a column of the app's list.
pub const ORDERS: [(Order, &str); 4] =
    [(Order::Found, "By folder"), (Order::Name, "Name"), (Order::Part, "Part"), (Order::Format, "File type")];

/// The preset files, as the app's Presets page holds them.
pub struct Presets {
    /// In the list's order (by folder, then name).
    pub items: Vec<Item>,
    /// The plugins among them, most used first: the plugin filter's order.
    pub plugins: Vec<(String, usize)>,
    /// The years the files were last changed in, newest first, with how many each: the year strip.
    pub years: Vec<(i32, usize)>,
}

/// How reading the presets went.
pub enum Read {
    Ready(Arc<Presets>),
    /// Nothing found, or it couldn't be read: why, in plain words.
    Not(String),
}

static SHARED: Mutex<Weak<Presets>> = Mutex::new(Weak::new());

/// The preset files, read once for every copy of the plugin while any of them holds them. Slow the first time (the
/// folders are walked): call it off the panel's thread.
pub fn read() -> Read {
    if let Some(presets) = lock(&SHARED).upgrade() {
        return Read::Ready(presets);
    }
    static READING: Mutex<()> = Mutex::new(());
    let _reading = lock(&READING);
    if let Some(presets) = lock(&SHARED).upgrade() {
        return Read::Ready(presets);
    }
    let started = std::time::Instant::now();
    let data = mo_app::paths::data_dir();
    let defaults = plib::default_folders();
    let bitwig = page::bitwig_folders(&defaults, bitwig_saves(&data.join("library.db")));
    let roots = page::preset_folders(&defaults, &bitwig);
    let read = read_from(&roots, &data.join(file_store::FILE_NAME));
    match &read {
        Read::Ready(presets) => {
            *lock(&SHARED) = Arc::downgrade(presets);
            crate::log::took("Presets", &format!("Read {} preset files", presets.items.len()), started);
        }
        Read::Not(why) => crate::log::took("Presets", &format!("Not read: {why}"), started),
    }
    read
}

/// The library's saves' paths (opened read-only, read and let go): the Bitwig folders are found from them. None when
/// the library can't be read; the usual folders are still looked in.
fn bitwig_saves(library: &Path) -> Vec<String> {
    let saves = mo_library::Library::open_read_only(library).and_then(|lib| lib.saves());
    match saves {
        Ok(saves) => saves.into_iter().map(|s| s.path).collect(),
        Err(e) => {
            crate::log::line("Presets", &format!("Couldn't read the library for its Bitwig folders: {e}"));
            Vec::new()
        }
    }
}

/// The preset files under `roots`, with what the app's store at `store` kept of them.
pub fn read_from(roots: &[PathBuf], store: &Path) -> Read {
    let (found, why) = file_store::find_read_only(roots, store);
    if let Some(why) = why {
        crate::log::line("Presets", &format!("Every preset file read, none taken from the app's list: {why}"));
    }
    crate::log::line("Presets", &format!("{} of {} preset files read from their files", found.read, found.files.len()));
    let years: HashMap<String, Option<i32>> =
        found.files.iter().zip(&found.modified).map(|(f, at)| (f.path.clone(), at.and_then(year_of_time))).collect();
    let mut items: Vec<Item> = plib::rows(&[], &found.files)
        .into_iter()
        .map(|row| {
            let year = match &row.source {
                Source::File { path } => years.get(path).copied().flatten(),
                Source::Song { .. } => None,
            };
            Item::new(row, String::new(), year)
        })
        .collect();
    if items.is_empty() {
        return Read::Not(
            "No preset files found in your plugins' and Bitwig's preset folders (VST3 Presets, Bitwig's Library)"
                .into(),
        );
    }
    page::sort_files(&mut items);
    let plugins = page::plugins(&items);
    let years = page::year_counts(&items, &Filters::default());
    Read::Ready(Arc::new(Presets { items, plugins, years }))
}

fn year_of_time(m: std::time::SystemTime) -> Option<i32> {
    let seconds = m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    page::year_of(i64::try_from(seconds).ok()?)
}

impl Presets {
    /// The presets `filters` let through, in order, as places in [`Presets::items`], and the filter column's counts.
    /// Goes over every line: call it off the panel's thread.
    pub fn view(&self, filters: &Filters) -> (Vec<usize>, ChoiceCounts) {
        let found = page::filtered(&self.items, filters);
        (found.shown, found.counts)
    }

    pub fn item(&self, at: usize) -> Option<&Item> {
        self.items.get(at)
    }

    /// The preset file at `at`, checked to be there still: what a drag carries out. Asks the disk: call it off the
    /// panel's thread.
    pub fn file(&self, at: usize) -> Result<PathBuf, String> {
        let item = self.item(at).ok_or("That preset isn't in the list any more")?;
        let path = PathBuf::from(item.path());
        if path.is_file() { Ok(path) } else { Err(format!("{} isn't there any more", path.display())) }
    }

    /// The filter column for these counts (none before the first list is worked out) and picks.
    pub fn chips(&self, counts: Option<&ChoiceCounts>, filters: &Filters) -> FilterChips {
        page::filter_chips(counts.unwrap_or(&ChoiceCounts::default()), &self.plugins, filters)
    }
}

/// The status line over the list: "1,234 preset files", or "40 of 1,234 preset files" when filters narrow it.
pub fn count_line(total: usize, shown: Option<usize>) -> String {
    let files = format!("{} preset file{}", page::thousands(total), if total == 1 { "" } else { "s" });
    match shown {
        Some(n) => format!("{} of {files}", page::thousands(n)),
        None => files,
    }
}

/// What's said under the list when a preset is picked: where a file of its type loads.
pub fn hint(item: &Item) -> String {
    let hint = page::load_hint(&item.row.extension);
    if hint.is_empty() { "Drag it onto a track or a device to load it".into() } else { hint }
}
