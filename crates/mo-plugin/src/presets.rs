//! The Presets tab: the preset files the app's Presets page lists, read-only, on a background thread.
//!
//! The plugin never writes the app's files. It looks in the folders the app's Presets page looks in (the plugins' and
//! Bitwig's usual ones, and every Bitwig folder a song of the library sits in, `mo_app::presets::preset_folders`),
//! and takes what the app's page kept of each file (`preset-files.db`) when the file hasn't changed since; the rest
//! are read now and not kept (`mo_presets::file_store::find_read_only`). The list, its filters and its orders are the
//! Presets page's own (`mo_app::presets`), so the two agree. A preset dragged out is its own file: nothing is copied
//! or written.
//!
//! The sounds in your songs' saves come too, as the app's Presets page last kept them (`preset-sounds.db`, read-only):
//! a save changed since is left out until the app's page reads it again (the plugin doesn't read saves itself). Each
//! line's plugin is told a synth or an effect by what the library knows of it (`mo_app::presets::KnownPlugins`): the
//! installed plugin's own file, then your songs.
//!
//! Like the sound library, the presets are shared by every copy of the plugin in the DAW while any of them holds them.

use crate::threads::lock;
use mo_app::presets::{self as page, ChoiceCounts, FilterChips, Filtered, Filters, Item, Order};
use mo_presets::file_store;
use mo_presets::library as plib;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

/// The orders the tab offers, with their words: as the files were found (by folder), or by a column of the app's list.
pub const ORDERS: [(Order, &str); 4] =
    [(Order::Found, "By folder"), (Order::Name, "Name"), (Order::Part, "Part"), (Order::Format, "File type")];

/// The presets, as the app's Presets page holds them.
pub struct Presets {
    /// In the list's order: your songs' sounds, newest song first, then the preset files by folder and name.
    pub items: Vec<Item>,
    /// Songs whose newest save changed since the app's Presets page last read it (or that it never read): left out.
    pub songs_missed: usize,
    /// The plugins among them, most used first: the plugin filter's order.
    pub plugins: Vec<(String, usize)>,
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
    let library = library_facts(&mo_library::songs_db::to_read(&data));
    let defaults = plib::default_folders();
    let bitwig = page::bitwig_folders(&defaults, library.saves.iter().map(|s| s.path.as_str()));
    let roots = page::preset_folders(&defaults, &bitwig);
    // On the app's low-priority threads, as the app reads them: never at the DAW's own priority.
    let read = mo_app::background::on_pool(|| {
        let (songs, missed) = song_items(&library, &data);
        read_with(&roots, &data.join(file_store::FILE_NAME), &library.known, songs, missed)
    });
    match &read {
        Read::Ready(presets) => {
            *lock(&SHARED) = Arc::downgrade(presets);
            crate::log::took("Presets", &format!("Read {} preset files", presets.items.len()), started);
        }
        Read::Not(why) => crate::log::took("Presets", &format!("Not read: {why}"), started),
    }
    read
}

/// The folders the app's Presets page looks in for preset files: the plugins' and Bitwig's usual ones, and every
/// Bitwig folder a song of the library sits in. Reads the library: off the DAW's threads.
pub fn folders() -> Vec<PathBuf> {
    let defaults = plib::default_folders();
    let data = mo_app::paths::data_dir();
    let bitwig = page::bitwig_folders(&defaults, bitwig_saves(&mo_library::songs_db::to_read(&data)));
    page::preset_folders(&defaults, &bitwig)
}

/// The library's saves' paths (opened read-only, read and let go): the Bitwig folders are found from them. None when
/// the library can't be read; the usual folders are still looked in. Quick from the app's own `songs.db`; slow from the
/// C# app's `library.db`, read only until `songs.db` is made (the plugin doesn't keep the app's list copy: that's a
/// write).
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

/// What the library says, read in one go and let go: its saves (for the Bitwig folders and the songs), how songs are
/// grouped, where Bitwig exports go, and what it knows of the plugins.
#[derive(Default)]
struct LibraryFacts {
    saves: Vec<mo_library::Save>,
    choices: mo_library::SongChoices,
    export_root: Option<PathBuf>,
    known: page::KnownPlugins,
}

/// The library at `path`, opened read-only: nothing when it can't be read (the usual preset folders are still looked
/// in).
fn library_facts(path: &Path) -> LibraryFacts {
    let lib = match mo_library::Library::open_read_only(path) {
        Ok(lib) => lib,
        Err(e) => {
            crate::log::line("Presets", &format!("Couldn't open the library: {e}"));
            return LibraryFacts::default();
        }
    };
    let saves = lib.saves().unwrap_or_else(|e| {
        crate::log::line("Presets", &format!("Couldn't read the library's saves: {e}"));
        Vec::new()
    });
    LibraryFacts {
        saves,
        choices: lib.song_choices().unwrap_or_default(),
        export_root: mo_app::bitwig_convert::export_left_out(&lib).ok().flatten(),
        known: page::KnownPlugins::read(&lib),
    }
}

/// Your songs' sounds, as the app's Presets page kept them (each song's newest save), with your part fixes laid over
/// and what's known of their plugins put in; and how many songs were left out (changed since, or never read).
fn song_items(library: &LibraryFacts, data: &Path) -> (Vec<Item>, usize) {
    let songs = mo_library::songs::group_songs(&library.saves, &library.choices, &mo_formats::dates::RealDates::now());
    let newest = page::newest_saves(&songs, library.export_root.as_deref());
    if newest.is_empty() {
        return (Vec::new(), 0);
    }
    let paths: Vec<String> = newest.iter().map(|s| s.path.clone()).collect();
    let limits = mo_presets::role::Thresholds::default();
    let store = data.join(mo_presets::sound_store::FILE_NAME);
    let (kept, missed) = match mo_presets::sound_store::SoundStore::kept_read_only(&store, &limits, &paths) {
        Ok(found) => found,
        Err(e) => {
            crate::log::line("Presets", &format!("Your songs' sounds aren't here: {}", e.0));
            return (Vec::new(), paths.len());
        }
    };
    let fixes = mo_presets::fixes::RoleFixes::open_read_only(&data.join(mo_presets::fixes::FILE_NAME))
        .and_then(|f| f.all())
        .unwrap_or_default();
    let mut items = Vec::new();
    for mut save in kept {
        mo_presets::list::apply_fixes(&mut save.sounds, &fixes);
        let Some(song) = newest.iter().find(|n| n.path == save.path) else { continue };
        let mut rows = plib::rows(std::slice::from_ref(&save), &[]);
        for row in &mut rows {
            library.known.fill(row);
        }
        items.extend(page::song_items(rows, song));
    }
    crate::log::line("Presets", &format!("{} sounds from your songs; {missed} songs left out", items.len()));
    (items, missed)
}

/// The preset files under `roots`, with what the app's store at `store` kept of them. Reads over every core of the
/// current rayon pool.
pub fn read_from(roots: &[PathBuf], store: &Path) -> Read {
    read_with(roots, store, &page::KnownPlugins::default(), Vec::new(), 0)
}

/// [`read_from`], with what's `known` of the plugins put in, after your songs' sounds (`songs`; `missed` left out).
fn read_with(roots: &[PathBuf], store: &Path, known: &page::KnownPlugins, songs: Vec<Item>, missed: usize) -> Read {
    let (found, why) = file_store::find_read_only(roots, store);
    if let Some(why) = why {
        crate::log::line("Presets", &format!("Every preset file read, none taken from the app's list: {why}"));
    }
    crate::log::line("Presets", &format!("{} of {} preset files read from their files", found.read, found.files.len()));
    let mut items = songs;
    items.extend(page::file_items_with(&found.files, &found.modified, known));
    if items.is_empty() {
        return Read::Not(
            "No preset files found in your plugins' and Bitwig's preset folders (VST3 Presets, Bitwig's Library)"
                .into(),
        );
    }
    let plugins = page::plugins(&items);
    Read::Ready(Arc::new(Presets { items, songs_missed: missed, plugins }))
}

impl Presets {
    /// The presets `filters` let through, in order, as places in [`Presets::items`], with the year strip's and the
    /// filter column's counts for them. Goes over every line: call it off the panel's thread.
    pub fn view(&self, filters: &Filters) -> Filtered {
        page::filtered(&self.items, filters)
    }

    pub fn item(&self, at: usize) -> Option<&Item> {
        self.items.get(at)
    }

    /// The preset file at `at`, checked to be there still: what a drag carries out. Asks the disk: call it off the
    /// panel's thread.
    pub fn file(&self, at: usize) -> Result<PathBuf, String> {
        let item = self.item(at).ok_or("That preset isn't in the list any more")?;
        if !item.is_file() {
            return Err(format!(
                "{} is a sound inside a song: dragging one out isn't built yet (a preset file drags)",
                item.row.name
            ));
        }
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
    let files = format!("{} preset{}", page::thousands(total), if total == 1 { "" } else { "s" });
    match shown {
        Some(n) => format!("{} of {files}", page::thousands(n)),
        None => files,
    }
}

/// The status line with the songs left out said: "54,892 presets · 12 songs changed since the app's Presets page
/// last read them: open it there to see them here".
pub fn status_line(total: usize, shown: Option<usize>, songs_missed: usize) -> String {
    let line = count_line(total, shown);
    match songs_missed {
        0 => line,
        1 => format!(
            "{line}  \u{b7}  1 song changed since the app's Presets page last read it: open that page to see it here"
        ),
        n => format!(
            "{line}  \u{b7}  {} songs changed since the app's Presets page last read them: open that page to see them here",
            page::thousands(n)
        ),
    }
}

/// What's said under the list when a preset is picked: where a file of its type loads.
pub fn hint(item: &Item) -> String {
    let hint = page::load_hint(&item.row.extension);
    if hint.is_empty() { "Drag it onto a track or a device to load it".into() } else { hint }
}
