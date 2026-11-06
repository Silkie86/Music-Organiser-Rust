//! The Presets page's logic and every word on it (spec 3.4a), tested without a window. Two sections:
//!
//! - **Sounds**: one list of every sound made inside your songs (each plugin's saved settings, by song and channel or
//!   track: the newest save of each song) and the preset files on disk (`.fxp`, `.fxb`, `.vstpreset`, `.bwpreset`),
//!   with the part each one plays guessed from its notes. Your fix to a part wins, and is kept in the app's own
//!   `presets.db` (`mo_presets::fixes`).
//! - **Curves** (Solomon, 2026-11-02): the shapes settings were moved in, from FL knob moves, Bitwig automation clips
//!   and Bitwig's curves library, kept once per shape (`mo_presets::curves`), to drop onto a Bitwig lane.
//!
//! The engine is `mo-presets`; this file only picks, sorts and words what it finds.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use mo_formats::PluginKind;
use mo_formats::bitwig::curve::CurvePoint;
use mo_library::Save;
use mo_library::songs::Song;
use mo_presets::curves::{Curve, SourceKind};
use mo_presets::export::Exported;
use mo_presets::filter::{self, Choice, Maker, Type};

use crate::filters::{self as chips, Filter};
use mo_presets::library::{PresetRow, Source};
use mo_presets::role::{Basis, Role};

/// The page's two sections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Sounds,
    Curves,
}

impl Section {
    pub const ALL: [Section; 2] = [Section::Sounds, Section::Curves];

    pub fn name(self) -> &'static str {
        match self {
            Section::Sounds => "Sounds",
            Section::Curves => "Curves",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Section> {
        Section::ALL.get(i).copied()
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Where the sounds come from

/// A save the page lists the sounds of: the newest save of one song.
#[derive(Clone, Debug, PartialEq)]
pub struct SongSave {
    pub path: String,
    /// The song's name, as the Songs page shows it.
    pub song: String,
    /// When the save was last written (seconds since 1970), for its year and for newest first.
    pub modified: i64,
}

/// Whether a save keeps its plugins' settings where the page can read them (FL Studio and Bitwig; Studio One's
/// reader doesn't take them out).
pub fn keeps_settings(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".flp") || lower.ends_with(".bwproject")
}

/// The saves the Sounds section reads: the newest save of each song that keeps its settings, newest first, leaving
/// out saves that have gone and the converter's own copies in the export folder (they'd show every sound twice).
pub fn newest_saves(songs: &[Song], export_root: Option<&Path>) -> Vec<SongSave> {
    // Each song gives its own name: no looking it up again among every song (H-14).
    let mut out: Vec<SongSave> = songs
        .iter()
        .filter(|s| !s.latest.file_missing && keeps_settings(&s.latest.path))
        .filter(|s| !mo_presets::curves::is_export(Path::new(&s.latest.path), export_root))
        .map(|s| SongSave { path: s.latest.path.clone(), song: s.name.clone(), modified: s.latest.mtime })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    out
}

/// The saves the Curves section looks in: every save of every song that keeps its settings (a knob move may be in an
/// older save only), once each, without the export folder's copies. `curves.db` reads each again only when it
/// changes, so this is quick after the first time.
pub fn curve_saves(saves: &[Save], export_root: Option<&Path>) -> Vec<String> {
    let mut seen = HashSet::new();
    saves
        .iter()
        .filter(|s| !s.file_missing && keeps_settings(&s.path))
        .filter(|s| !mo_presets::curves::is_export(Path::new(&s.path), export_root))
        .filter(|s| seen.insert(s.path.clone()))
        .map(|s| s.path.clone())
        .collect()
}

/// Bitwig's own folders ("…\Bitwig Studio", which holds Projects and Library): the one in Documents where it's there
/// (`defaults`, from `mo_presets::library::default_folders`), plus any a Bitwig save sits in, so a Bitwig folder on
/// another drive (Solomon's is on K:) is found without a setting.
pub fn bitwig_folders(defaults: &[PathBuf], bitwig_saves: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = defaults.iter().filter(|d| is_bitwig_folder(d)).cloned().collect::<Vec<_>>();
    for save in bitwig_saves {
        let save = save.as_ref();
        if !save.to_lowercase().ends_with(".bwproject") {
            continue;
        }
        let path = Path::new(save);
        if let Some(found) = path.ancestors().skip(1).find(|a| is_bitwig_folder(a))
            && !out.iter().any(|o| same_folder(o, found))
        {
            out.push(found.to_path_buf());
        }
    }
    out
}

fn is_bitwig_folder(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("Bitwig Studio"))
}

fn same_folder(a: &Path, b: &Path) -> bool {
    is_inside(a, b) && is_inside(b, a)
}

/// The folders preset files are looked for in: the plugins' and Bitwig's usual ones, and every Bitwig folder found.
pub fn preset_folders(defaults: &[PathBuf], bitwig: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for f in defaults.iter().chain(bitwig) {
        if !out.iter().any(|o| same_folder(o, f) || is_inside(f, o)) {
            out.retain(|o| !is_inside(o, f));
            out.push(f.clone());
        }
    }
    out
}

/// Whether `inner` is `outer` or a folder inside it.
fn is_inside(inner: &Path, outer: &Path) -> bool {
    // The library's own test, whichever way the slashes lean.
    mo_library::scan::is_inside(&inner.to_string_lossy(), &outer.to_string_lossy())
}

/// Where a curve goes in Bitwig's curves library: `<Bitwig Studio>\Library\Curves` in the first Bitwig folder found.
pub fn bitwig_curves_folder(bitwig: &[PathBuf]) -> Option<PathBuf> {
    bitwig.first().map(|b| b.join("Library").join("Curves"))
}

/// The curve files (`.bwcurve`) under `roots`, for the Curves section (Bitwig's curves library). Folders starting
/// with "." are skipped, as the preset finder does; asks the disk, so never on the window's thread.
pub fn curve_files(roots: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut todo: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    while let Some((dir, depth)) = todo.pop() {
        if depth > 16 || !seen.insert(std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if path.is_dir() {
                if !name.starts_with('.') {
                    todo.push((path, depth + 1));
                }
            } else if name.to_lowercase().ends_with(".bwcurve")
                && let Some(p) = path.to_str()
            {
                out.push(p.to_owned());
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// The Sounds section's list

/// One line of the list: a sound or a preset file, with its song (or "" for a file) and the year it's from.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub row: PresetRow,
    /// The song's name; "" for a preset file.
    pub song: String,
    /// The save's or file's year, for the year strip.
    pub year: Option<i32>,
    /// What a search looks in, in lower case, worked out once (the page searches every line on each key).
    words: String,
    /// A preset file's path in lower case, which the list sorts the files by, worked out once; "" for a song's sound.
    file_key: String,
    /// The plugin's format, worked out once.
    pub format: Format,
}

impl Item {
    pub fn new(row: PresetRow, song: String, year: Option<i32>) -> Item {
        let [name, plugin, place, file] = row.search_texts();
        let words = [name, plugin, place, file, &song].join("\u{1f}").to_lowercase();
        let file_key = match &row.source {
            Source::File { path } => path.to_lowercase(),
            Source::Song { .. } => String::new(),
        };
        let format = Format::of(&row);
        Item { row, song, year, words, file_key, format }
    }

    /// Your part for the line with `fixes` laid over it: a fix made since the line was read wins.
    pub fn fixed_with(&self, fixes: &Fixes) -> Option<Role> {
        fixes.get(&self.row.hash).copied().unwrap_or(self.row.fixed)
    }

    /// The part shown with `fixes` laid over the line ([`Item::role`]).
    pub fn role_with(&self, fixes: &Fixes) -> Option<Role> {
        self.fixed_with(fixes).or(self.row.guess.role)
    }

    /// The line as it shows with `fixes` laid over it: itself, unless a fix changes its part (then a copy of it).
    pub fn as_fixed<'a>(&'a self, fixes: &Fixes) -> Cow<'a, Item> {
        let fixed = self.fixed_with(fixes);
        if fixed == self.row.fixed {
            return Cow::Borrowed(self);
        }
        let mut i = self.clone();
        i.row.fixed = fixed;
        Cow::Owned(i)
    }

    /// Every word of a search (already in lower case) is in the line's texts or its part.
    /// `role`: the part shown ([`Item::role_with`]).
    fn found(&self, search: &[String], role: Option<Role>) -> bool {
        // The part's words only when a word isn't in the line's own: this runs for every line, several times a key.
        let in_part = |w: &str| role.is_some_and(|r| r.label().to_lowercase().contains(w));
        search.iter().all(|w| self.words.contains(w.as_str()) || in_part(w))
    }

    /// The part shown: yours when you set one, else the guess.
    pub fn role(&self) -> Option<Role> {
        self.row.fixed.or(self.row.guess.role)
    }

    pub fn is_file(&self) -> bool {
        matches!(self.row.source, Source::File { .. })
    }

    /// The file the line stands for: the song's save, or the preset file.
    pub fn path(&self) -> &str {
        match &self.row.source {
            Source::Song { save, .. } => save,
            Source::File { path } => path,
        }
    }
}

/// The year of a time in seconds since 1970 (UTC), or `None` for one that's not a real date.
pub fn year_of(seconds: i64) -> Option<i32> {
    use chrono::Datelike as _;
    chrono::DateTime::from_timestamp(seconds, 0).map(|d| d.year()).filter(|y| *y > 1980)
}

/// The page's lines for a song's sounds as they're read.
pub fn song_items(rows: Vec<PresetRow>, song: &SongSave) -> Vec<Item> {
    let year = year_of(song.modified);
    rows.into_iter().map(|row| Item::new(row, song.song.clone(), year)).collect()
}

/// Where the lines come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum From {
    #[default]
    All,
    /// Sounds inside your songs.
    Songs,
    /// Preset files on disk.
    Files,
}

impl From {
    pub const ALL: [From; 3] = [From::All, From::Songs, From::Files];

    pub fn label(self) -> &'static str {
        match self {
            From::All => "",
            From::Songs => "My songs",
            From::Files => "Preset files",
        }
    }

    pub fn from_label(l: &str) -> From {
        From::ALL.into_iter().find(|f| f.label() == l).unwrap_or_default()
    }
}

/// The part chip: any part, one part, or the sounds with no guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PartPick {
    #[default]
    Any,
    Role(Role),
    NoGuess,
}

/// The chip's words for "no part".
pub const NO_PART: &str = "No guess";

impl PartPick {
    pub fn label(self) -> &'static str {
        match self {
            PartPick::Any => "",
            PartPick::Role(r) => r.label(),
            PartPick::NoGuess => NO_PART,
        }
    }

    pub fn from_label(l: &str) -> PartPick {
        if l == NO_PART {
            return PartPick::NoGuess;
        }
        Role::ALL.into_iter().find(|r| r.label() == l).map_or(PartPick::Any, PartPick::Role)
    }

    pub fn choices() -> Vec<&'static str> {
        Role::ALL.iter().map(|r| r.label()).chain([NO_PART]).collect()
    }

    fn matches(self, role: Option<Role>) -> bool {
        match self {
            PartPick::Any => true,
            PartPick::Role(r) => role == Some(r),
            PartPick::NoGuess => role.is_none(),
        }
    }
}

/// Picks or unpicks a one-or-none filter's choice, kept as a value whose default (All) means none picked.
fn toggle_one<T: Copy + Default + PartialEq>(
    value: &mut T,
    choice: &str,
    words: impl Fn(T) -> &'static str,
    from_words: impl Fn(&str) -> T,
) {
    let mut picked: Vec<T> = if *value == T::default() { Vec::new() } else { vec![*value] };
    chips::toggle_as(&mut picked, choice, false, words, |w| Some(from_words(w)).filter(|v| *v != T::default()));
    *value = picked.first().copied().unwrap_or_default();
}

/// The plugin's format (Solomon's note, 5 Nov 2026: "filter by plugin type", which he said is the format). A sound's format comes from the file it exports as; one of the DAW's own plugins (FL Studio's, Bitwig's
/// devices) is Native whatever its file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Format {
    Vst2,
    Vst3,
    Clap,
    Au,
    Native,
    /// A format the file doesn't say (a Bitwig preset of a plugin, say).
    #[default]
    Other,
}

impl Format {
    /// In the order the chips show them.
    pub const ALL: [Format; 6] = [Format::Vst2, Format::Vst3, Format::Clap, Format::Au, Format::Native, Format::Other];

    pub fn of(row: &PresetRow) -> Format {
        if row.native {
            return Format::Native;
        }
        let ext = row.extension.trim_start_matches('.').to_ascii_lowercase();
        match ext.as_str() {
            "fxp" | "fxb" => Format::Vst2,
            "vstpreset" | "vst3state" => Format::Vst3,
            "aupreset" => Format::Au,
            e if e.starts_with("clap") => Format::Clap,
            _ => Format::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Vst2 => "VST2",
            Format::Vst3 => "VST3",
            Format::Clap => "CLAP",
            Format::Au => "AU",
            Format::Native => "Native",
            Format::Other => "Other",
        }
    }

    fn from_label(l: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.label().eq_ignore_ascii_case(l))
    }

    fn index(self) -> usize {
        Format::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}

pub fn type_label(t: Type) -> &'static str {
    match t {
        Type::All => "",
        Type::Synths => "Synths",
        Type::Effects => "Effects",
    }
}

pub fn type_from(l: &str) -> Type {
    [Type::Synths, Type::Effects].into_iter().find(|t| type_label(*t) == l).unwrap_or_default()
}

/// What the list is filtered by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub search: String,
    pub choice: Choice,
    /// Only these parts' sounds (any of them); every part when none is picked. Never holds [`PartPick::Any`].
    pub parts: Vec<PartPick>,
    pub from: From,
    /// Only these formats' sounds (any of them); every format when none is picked.
    pub formats: Vec<Format>,
    /// Only these plugins' sounds (any of them), as the list names them; every plugin when none is picked.
    pub plugins: Vec<String>,
    /// Only one song's sounds (a song's Presets tab), by its save's path.
    pub song: Option<String>,
    /// Only these years, first to last.
    pub years: Option<(i32, i32)>,
    /// Show synths whose channel or track plays no notes (tucked away otherwise, as the C# window did).
    pub unused: bool,
}

impl Filters {
    /// Whether any chip is on (the Clear chip shows then). The search box has its own ×, and the unused synths are
    /// an option, not a filter.
    pub fn any(&self) -> bool {
        self.choice != Choice::default()
            || !self.formats.is_empty()
            || !self.parts.is_empty()
            || self.from != From::All
            || !self.plugins.is_empty()
            || self.song.is_some()
            || self.years.is_some()
    }

    /// Picks or unpicks `choice` (as its chip or menu line says it) on the filter `chip` ([`PART`], [`PLUGIN`],
    /// [`FORMAT`], [`TYPE`], [`FROM`]). More than one part, plugin or format can be picked; the type and where the
    /// sounds are from are one or none, as each has only two choices.
    pub fn toggle(&mut self, chip: &str, choice: &str) {
        let part = |l: &str| Some(PartPick::from_label(l)).filter(|p| *p != PartPick::Any);
        match chip {
            PART => chips::toggle_as(&mut self.parts, choice, true, PartPick::label, part),
            PLUGIN => chips::toggle(&mut self.plugins, choice, true),
            FORMAT => chips::toggle_as(&mut self.formats, choice, true, Format::label, Format::from_label),
            TYPE => toggle_one(&mut self.choice.kind, choice, type_label, type_from),
            FROM => toggle_one(&mut self.from, choice, From::label, From::from_label),
            _ => {}
        }
    }

    /// Picks nothing on the filter `chip`: its menu's Any.
    pub fn clear_chip(&mut self, chip: &str) {
        match chip {
            PART => self.parts.clear(),
            PLUGIN => self.plugins.clear(),
            FORMAT => self.formats.clear(),
            TYPE => self.choice.kind = Type::All,
            FROM => self.from = From::All,
            _ => {}
        }
    }

    /// Every chip off, the search and the option kept.
    pub fn cleared(&self) -> Filters {
        Filters { search: self.search.clone(), unused: self.unused, ..Filters::default() }
    }

    /// The search's words, in lower case, once for a whole pass over the list.
    fn words(&self) -> Vec<String> {
        self.search.split_whitespace().map(str::to_lowercase).collect()
    }

    /// Everything but the five chips lets `i` through: the unused synths, the song, the years and the search. `words`
    /// is [`Filters::words`].
    fn passes_others(&self, i: &Item, words: &[String], role: Option<Role>) -> bool {
        (self.unused || !i.row.plays_nothing)
            && self.song.as_ref().is_none_or(|s| !i.is_file() && i.path() == s)
            && self.years.is_none_or(|(a, b)| i.year.is_some_and(|y| y >= a && y <= b))
            && i.found(words, role)
    }

    /// Which of the five chips let `i` through: the format, the type, the part, where it's from and the plugin.
    fn chips_pass(&self, i: &Item, role: Option<Role>) -> [bool; 5] {
        let kind = Choice { maker: Maker::All, ..self.choice };
        [
            self.formats.is_empty() || self.formats.contains(&i.format),
            kind.matches(i.row.native, i.row.kind),
            self.parts.is_empty() || self.parts.iter().any(|p| p.matches(role)),
            match self.from {
                From::All => true,
                From::Songs => !i.is_file(),
                From::Files => i.is_file(),
            },
            self.plugins.is_empty() || self.plugins.iter().any(|p| i.row.plugin.eq_ignore_ascii_case(p)),
        ]
    }

    fn passes_with(&self, i: &Item, words: &[String]) -> bool {
        self.chips_pass(i, i.role()) == [true; 5] && self.passes_others(i, words, i.role())
    }

    /// Whether the filters let `i` through, as it was read (no part fixes laid over it: the page uses
    /// [`filtered_with`]).
    pub fn passes(&self, i: &Item) -> bool {
        self.passes_with(i, &self.words())
    }
}

/// The lines shown, as places in `items`, in the list's order: songs newest first (as they came), each song's sounds
/// in the order its save has them, then the preset files by folder and name.
pub fn shown(items: &[Item], f: &Filters) -> Vec<usize> {
    filtered(items, f).shown
}

/// How many lines each choice of each filter would show, with the other filters as they are: the numbers beside the
/// choices, so each filter says what its choices hold (Solomon's note, 4 Nov 2026).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChoiceCounts {
    /// By format, in [`Format::ALL`]'s order.
    pub formats: [usize; 6],
    pub synths: usize,
    pub effects: usize,
    /// By part, in [`PartPick::choices`]' order: each part, then no guess.
    pub parts: Vec<usize>,
    pub songs: usize,
    pub files: usize,
    /// By plugin, its name in lower case (ASCII letters only, as the plugin filter matches).
    pub plugins: HashMap<String, usize>,
}

/// Counts every filter's choices in one pass. A line counts beside every filter when all five let it through, and
/// beside the one that doesn't when only one doesn't: picking another choice there could show it.
pub fn counts(items: &[Item], f: &Filters) -> ChoiceCounts {
    filtered(items, f).counts
}

/// What the filters make of the list, worked out in one pass (the page does it off the window's thread): the lines
/// shown ([`shown`]), the year strip's counts ([`year_counts`]) and every filter's choices' counts ([`counts`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filtered {
    pub shown: Vec<usize>,
    pub years: Vec<(i32, usize)>,
    pub counts: ChoiceCounts,
}

/// [`filtered_with`] the lines as they were read, no part fixes laid over them.
pub fn filtered(items: &[Item], f: &Filters) -> Filtered {
    filtered_with(items, f, &Fixes::new())
}

/// Your part fixes made since the lines were read, by the sound's fingerprint (None: the fix was taken away). They're
/// laid over the lines rather than written into them: the page shares its lines with other threads (the filtering,
/// Ctrl+K), and changing one line would mean copying them all.
pub type Fixes = HashMap<String, Option<Role>>;

/// [`filtered`] with `fixes` laid over the lines.
pub fn filtered_with(items: &[Item], f: &Filters, fixes: &Fixes) -> Filtered {
    let words = f.words();
    let without_years = Filters { years: None, ..f.clone() };
    let mut tally = Tally::new();
    let mut shown: Vec<usize> = Vec::new();
    let mut by_year: BTreeMap<i32, usize> = BTreeMap::new();
    for (n, i) in items.iter().enumerate() {
        // Worked out once: each fix laid over the line is found by its fingerprint.
        let role = i.role_with(fixes);
        if !without_years.passes_others(i, &words, role) {
            continue;
        }
        let ok = f.chips_pass(i, role);
        let all = ok == [true; 5];
        // The year strip counts the years with every other filter on.
        if all && let Some(y) = i.year {
            *by_year.entry(y).or_default() += 1;
        }
        if !f.years.is_none_or(|(a, b)| i.year.is_some_and(|y| y >= a && y <= b)) {
            continue;
        }
        tally.add(i, ok, role);
        if all {
            shown.push(n);
        }
    }
    // Files after the songs; files among themselves by folder, then name. The sort is stable, so songs keep theirs.
    // The page keeps its lines in this order already ([`sort_files`]), so the sort is only a check then.
    let order = |&a: &usize, &b: &usize| {
        let (a, b) = (&items[a], &items[b]);
        (a.is_file(), &a.file_key).cmp(&(b.is_file(), &b.file_key))
    };
    if !shown.is_sorted_by(|a, b| order(a, b).is_le()) {
        shown.sort_by(order);
    }
    Filtered { shown, years: crate::year_strip::strip(by_year), counts: tally.done() }
}

/// Puts preset files in the list's order (by folder, then name), so the filtering finds them in order and doesn't have
/// to sort them each time.
pub fn sort_files(files: &mut [Item]) {
    files.sort_by(|a, b| a.file_key.cmp(&b.file_key));
}

/// Counts every filter's choices. A line counts beside every filter when all five let it through, and beside the one
/// that doesn't when only one doesn't: picking another choice there could show it.
struct Tally<'a> {
    c: ChoiceCounts,
    by_plugin: HashMap<&'a str, usize>,
}

impl<'a> Tally<'a> {
    fn new() -> Tally<'a> {
        Tally {
            c: ChoiceCounts { parts: vec![0; Role::ALL.len() + 1], ..ChoiceCounts::default() },
            by_plugin: HashMap::new(),
        }
    }

    /// A line the other filters let through, with which of the five chips let it through.
    fn add(&mut self, i: &'a Item, ok: [bool; 5], role: Option<Role>) {
        let synths = Choice { kind: Type::Synths, ..Choice::default() };
        let effects = Choice { kind: Type::Effects, ..Choice::default() };
        let c = &mut self.c;
        let fails = ok.iter().filter(|&&o| !o).count();
        let counts_on = |chip: usize| fails == 0 || (fails == 1 && !ok[chip]);
        if counts_on(0) {
            c.formats[i.format.index()] += 1;
        }
        if counts_on(1) {
            c.synths += usize::from(synths.matches(i.row.native, i.row.kind));
            c.effects += usize::from(effects.matches(i.row.native, i.row.kind));
        }
        if counts_on(2) {
            let no_guess = Role::ALL.len();
            let at = role.and_then(|r| Role::ALL.iter().position(|&x| x == r)).unwrap_or(no_guess);
            c.parts[at] += 1;
        }
        if counts_on(3) {
            *if i.is_file() { &mut c.files } else { &mut c.songs } += 1;
        }
        if counts_on(4) && !i.row.plugin.is_empty() {
            *self.by_plugin.entry(i.row.plugin.as_str()).or_default() += 1;
        }
    }

    fn done(mut self) -> ChoiceCounts {
        for (name, n) in self.by_plugin {
            *self.c.plugins.entry(name.to_ascii_lowercase()).or_default() += n;
        }
        self.c
    }
}

/// The filters' names, as the page calls them when one is picked.
pub const PART: &str = "part";
pub const PLUGIN: &str = "plugin";
pub const FORMAT: &str = "format";
pub const TYPE: &str = "type";
pub const FROM: &str = "from";
/// The Curves section's Found in filter.
pub const FOUND_IN: &str = "found-in";

/// The format filter's choices, each with how many lines it would show: ("VST3", "120"). VST2, VST3, CLAP and Native
/// are always there; AU and Other only when some line has them.
pub fn format_choices(c: &ChoiceCounts) -> Vec<(String, String)> {
    Format::ALL
        .into_iter()
        .zip(c.formats)
        .filter(|(f, n)| *n > 0 || !matches!(f, Format::Au | Format::Other))
        .map(|(f, n)| (f.label().to_owned(), thousands(n)))
        .collect()
}

/// The type filter's choices, each with how many lines it would show.
pub fn type_choices(c: &ChoiceCounts) -> Vec<(String, String)> {
    [(Type::Synths, c.synths), (Type::Effects, c.effects)]
        .into_iter()
        .map(|(t, n)| (type_label(t).to_owned(), thousands(n)))
        .collect()
}

/// The page's filters as their chips and menus show them, with what's picked and how many lines each choice would
/// show: the format, the type and where the sounds are from (a few choices each, so chips of their own), the part and
/// the plugin (menus that scroll). More than one part, plugin or format can be picked. `plugins` is [`plugins`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterChips {
    pub format: Filter,
    pub kind: Filter,
    pub from: Filter,
    pub part: Filter,
    pub plugin: Filter,
}

pub fn filter_chips(c: &ChoiceCounts, plugins: &[(String, usize)], f: &Filters) -> FilterChips {
    let one = |label: &str| if label.is_empty() { Vec::new() } else { vec![label.to_owned()] };
    let from = [(From::Songs, c.songs), (From::Files, c.files)].map(|(w, n)| (w.label().to_owned(), thousands(n)));
    let parts: Vec<(String, String)> =
        PartPick::choices().into_iter().zip(&c.parts).map(|(l, n)| (l.to_owned(), thousands(*n))).collect();
    // The most used first, as `plugins` has them: an order the other filters don't change, so a line doesn't move
    // under the mouse when they do.
    let by_plugin: Vec<(String, usize)> = plugins
        .iter()
        .map(|(name, _)| (name.clone(), c.plugins.get(&name.to_ascii_lowercase()).copied().unwrap_or(0)))
        .collect();
    FilterChips {
        format: chips::show(&format_choices(c), &f.formats.iter().map(|x| x.label().to_owned()).collect::<Vec<_>>()),
        kind: chips::show(&type_choices(c), &one(type_label(f.choice.kind))),
        from: chips::show(&from, &one(f.from.label())),
        part: chips::show(&parts, &f.parts.iter().map(|p| p.label().to_owned()).collect::<Vec<_>>()),
        plugin: chips::show(
            &by_plugin.into_iter().map(|(name, n)| (name, thousands(n))).collect::<Vec<_>>(),
            &f.plugins,
        ),
    }
}

/// The plugins in the list, most used first (ties by name), each with how many lines it has; for the plugin chip.
/// Names that differ only in ASCII capitals are one plugin, as the plugin filter matches them.
pub fn plugins(items: &[Item]) -> Vec<(String, usize)> {
    let mut by: HashMap<String, (String, usize)> = HashMap::new();
    for i in items.iter().filter(|i| !i.row.plugin.is_empty()) {
        by.entry(i.row.plugin.to_ascii_lowercase()).or_insert_with(|| (i.row.plugin.clone(), 0)).1 += 1;
    }
    let mut out: Vec<(String, usize)> = by.into_values().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    out
}

/// How many lines each year has with the other filters as they are (not the years themselves), newest first.
pub fn year_counts(items: &[Item], f: &Filters) -> Vec<(i32, usize)> {
    filtered(items, f).years
}

pub use crate::songs::thousands;

/// The line under the page's title.
pub fn summary(total: usize, shown: usize, songs: usize, files: usize, reading: bool) -> String {
    if total == 0 {
        return if reading { "Reading the sounds in your songs…".into() } else { String::new() };
    }
    let what = format!(
        "{} sound{} from {} song{}, {} preset file{}",
        thousands(total - files),
        if total - files == 1 { "" } else { "s" },
        thousands(songs),
        if songs == 1 { "" } else { "s" },
        thousands(files),
        if files == 1 { "" } else { "s" }
    );
    // Shown counts preset files too, so it's set against everything listed, not the sounds alone: "Showing 62,318 of
    // 61,725 sounds..." read as more shown than there are.
    if shown == total { what } else { format!("Showing {} of {}: {what}", thousands(shown), thousands(total)) }
}

/// The list's words when it shows nothing: a title and a line, or ("", "") when there are lines. `filtering`: what's
/// shown is still being worked out. "No sounds match" only shows once it's sure: not while songs are still being read
/// or a filtering is on its way (H-9).
pub fn empty_words(
    total: usize,
    shown: usize,
    reading: bool,
    filtering: bool,
    any_filter: bool,
    failed: Option<&str>,
) -> (String, String) {
    if let Some(why) = failed {
        return ("Couldn't read your songs".into(), why.to_owned());
    }
    if shown > 0 {
        return (String::new(), String::new());
    }
    if total == 0 {
        return if reading {
            ("Reading your songs".into(), "The sounds show here as each song is read.".into())
        } else {
            (
                "No sounds yet".into(),
                "Only FL Studio and Bitwig songs keep their plugins' settings where they can be read. Add your project \
                 folders on the Songs page, and preset files in your VST3 and Bitwig folders show here too."
                    .into(),
            )
        };
    }
    if reading {
        return ("Reading your songs".into(), "The sounds that match show here as each song is read.".into());
    }
    if filtering {
        return (String::new(), String::new());
    }
    (
        "No sounds match".into(),
        if any_filter { "Try fewer filters or other words.".into() } else { "Try other words.".into() },
    )
}

/// The list's words when a song's sounds are asked for and none were read from it: why, in a few words.
pub fn song_not_read(save: &str) -> (String, String) {
    let text = if keeps_settings(save) {
        "Its newest save has no plugin sounds the page can read, or it's a copy in the Bitwig export folder."
    } else {
        "Only FL Studio and Bitwig songs keep their plugins' settings where they can be read."
    };
    ("No sounds from this song".into(), text.into())
}

/// One line of the list, in words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub name: String,
    /// The plugin, and where it sits: "Serum · Bass".
    pub under: String,
    /// The part shown, "Bass", with "?" for a weak guess and nothing for no guess.
    pub part: String,
    /// The part's colour (a place in the Sounds page's type colours), or -1.
    pub colour: i32,
    /// Why: the guess's reason, or that you set it.
    pub part_tip: String,
    pub fixed: bool,
    /// The song's name, or the preset file's folder.
    pub from: String,
    /// "Synth", "Effect", or "".
    pub kind: String,
    /// The file type: "fxp", "bwpreset".
    pub format: String,
}

/// A part's colour among the Sounds page's type colours (studio.slint `sound-types`), so a bass is the same blue
/// everywhere.
pub fn role_colour(role: Option<Role>) -> i32 {
    match role {
        Some(Role::Drums) => 0,
        Some(Role::Bass) => 8,
        Some(Role::Lead) => 9,
        Some(Role::Pad) => 10,
        Some(Role::Arp) => 11,
        Some(Role::Stabs) => 12,
        Some(Role::Chords) => 13,
        Some(Role::Fx) => 15,
        None => -1,
    }
}

pub fn kind_word(kind: PluginKind) -> &'static str {
    match kind {
        PluginKind::Instrument => "Synth",
        PluginKind::Effect | PluginKind::NoteEffect => "Effect",
        PluginKind::Unknown => "",
    }
}

/// Why the part is what it is, for the hover text and the card.
pub fn part_reason(i: &Item) -> String {
    match (i.row.fixed, i.row.guess.role) {
        (Some(f), Some(g)) if f != g => {
            format!("You set this to {}. The guess was {}: {}", f.label(), g.label().to_lowercase(), i.row.guess.reason)
        }
        (Some(f), Some(_)) => format!("You set this to {}. The guess said the same: {}", f.label(), i.row.guess.reason),
        (Some(f), None) => format!("You set this to {}. There was no guess: {}", f.label(), i.row.guess.reason),
        (None, _) => i.row.guess.reason.clone(),
    }
}

pub fn line(i: &Item) -> Line {
    let role = i.role();
    let weak = i.row.fixed.is_none() && i.row.guess.weak && role.is_some();
    let under = [i.row.plugin.as_str(), i.row.place.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(&i.row.name))
        .fold(Vec::<&str>::new(), |mut out, s| {
            // A preset file's folder is often named after its plugin: "Vital · Vital" says it once.
            if !out.iter().any(|o| o.eq_ignore_ascii_case(s)) {
                out.push(s);
            }
            out
        })
        .join(" · ");
    Line {
        name: i.row.name.clone(),
        under,
        part: role.map(|r| if weak { format!("{}?", r.label()) } else { r.label().to_owned() }).unwrap_or_default(),
        colour: role_colour(role),
        part_tip: part_reason(i),
        fixed: i.row.fixed.is_some(),
        from: if i.is_file() { i.row.place.clone() } else { i.song.clone() },
        kind: kind_word(i.row.kind).to_owned(),
        format: i.row.extension.trim_start_matches('.').to_lowercase(),
    }
}

/// The picked line's card, in words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// "SYNTH · FL STUDIO PLUGIN", over the name.
    pub over: String,
    pub name: String,
    pub plugin: String,
    /// The part shown and its colour, or "No part guessed".
    pub part: String,
    pub colour: i32,
    pub reason: String,
    /// The part you set, as its label, or "" when the guess stands.
    pub fixed: String,
    /// What it is, in pairs: ("SONG", "Didums 77"), ("ON", "Insert 3 · Vocals")…
    pub facts: Vec<(String, String)>,
    /// Where a file of its type loads.
    pub hint: String,
    /// The save or file it's in.
    pub path: String,
    /// The part can be fixed (a preset file has no fingerprint to keep a fix by yet).
    pub fixable: bool,
}

pub fn card(i: &Item) -> Card {
    let role = i.role();
    let ext = i.row.extension.to_lowercase();
    let mut over: Vec<&str> = Vec::new();
    if !kind_word(i.row.kind).is_empty() {
        over.push(kind_word(i.row.kind));
    }
    if i.row.native {
        over.push(if ext == ".bwpreset" || i.path().to_lowercase().ends_with(".bwproject") {
            "Bitwig device"
        } else {
            "FL Studio plugin"
        });
    }
    over.push(if i.is_file() { "Preset file" } else { "In a song" });
    let mut facts: Vec<(String, String)> = Vec::new();
    if i.is_file() {
        facts.push(("FOLDER".into(), i.row.place.clone()));
    } else {
        facts.push(("SONG".into(), i.song.clone()));
        if !i.row.place.is_empty() {
            facts.push(("ON".into(), i.row.place.clone()));
        }
    }
    facts.push(("TYPE".into(), ext.trim_start_matches('.').to_owned()));
    if let Some(y) = i.year {
        facts.push(("YEAR".into(), y.to_string()));
    }
    let basis = match (i.row.fixed, i.row.guess.basis) {
        (Some(_), _) => "",
        (None, _) if role.is_none() => "",
        (None, Basis::Notes) => " (from its notes)",
        (None, Basis::Name) => " (from a name)",
        (None, Basis::Track) => " (from its track)",
    };
    Card {
        over: over.join(" · ").to_uppercase(),
        name: i.row.name.clone(),
        plugin: if i.row.plugin.eq_ignore_ascii_case(&i.row.name) { String::new() } else { i.row.plugin.clone() },
        part: role.map_or_else(|| "No part guessed".to_owned(), |r| format!("{}{basis}", r.label())),
        colour: role_colour(role),
        reason: part_reason(i),
        fixed: i.row.fixed.map(|r| r.label().to_owned()).unwrap_or_default(),
        facts,
        hint: load_hint(&ext),
        path: i.path().to_owned(),
        fixable: !i.row.hash.is_empty(),
    }
}

/// Where a file of a type can be loaded, as a sentence.
pub fn load_hint(extension: &str) -> String {
    let hint = filter::load_hint(extension);
    if hint.is_empty() {
        return String::new();
    }
    let mut c = hint.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>() + ".").unwrap_or_default()
}

/// A line of the page's lines, for the list under the page: where each type in it loads (the C# window's FormatNote).
pub fn format_note(items: &[Item], shown: &[usize]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for &i in shown {
        let ext = items[i].row.extension.to_lowercase();
        if !seen.contains(&ext) {
            seen.push(ext);
        }
    }
    seen.sort();
    seen.into_iter()
        .filter_map(|e| {
            let hint = filter::load_hint(&e);
            (!hint.is_empty()).then(|| format!("{}: {hint}", e.trim_start_matches('.')))
        })
        .collect::<Vec<_>>()
        .join("  ·  ")
}

// ---------------------------------------------------------------------------------------------------------------------
// What can be done with a line

/// The Sounds section's actions, by their shortcut ids (see `shortcuts::PRESETS_ACTIONS`).
pub mod act {
    pub const EXPORT: &str = "presets.export";
    pub const TO_BITWIG: &str = "presets.toBitwig";
    /// Audition in Bitwig (a preset on a track of its own, playing a part), and Stop playing in Bitwig while it plays.
    pub const AUDITION: &str = "presets.audition";
    /// Keep an audition in Bitwig: its track stays, as Send to Bitwig would have left it.
    pub const KEEP: &str = "presets.keepAudition";
    pub const EXPORT_SHOWN: &str = "presets.exportShown";
    pub const REVEAL: &str = "presets.reveal";
    /// The sound's song on the Songs page (Show in Songs, as Plugins has it).
    pub const SHOW_IN_SONGS: &str = "presets.showInSongs";
    pub const COPY_PATH: &str = "presets.copyPath";
    pub const USE_GUESS: &str = "presets.useGuess";
    /// A part fix: this, then the part's key ("presets.part.bass").
    pub const PART: &str = "presets.part.";
}

/// One thing the card or the right-click menu offers: its words, its id, and whether it can be used now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub text: String,
    pub id: &'static str,
    pub enabled: bool,
}

fn offer(text: &str, id: &'static str, enabled: bool) -> Offer {
    Offer { text: text.into(), id, enabled }
}

/// Whether the picked line is being auditioned in Bitwig: not, starting (its track going in), or playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hearing {
    #[default]
    Off,
    Starting,
    Playing,
}

/// What the picked line offers, in the card's order. `busy` while an export runs; `hearing` says whether this line is
/// being auditioned (then Audition is Stop playing in Bitwig, and Keep in Bitwig shows beside it once it plays).
pub fn offers(i: &Item, busy: bool, hearing: Hearing) -> Vec<Offer> {
    let ext = i.row.extension.to_lowercase();
    let file = i.is_file();
    // A Bitwig preset file, or a sound a Bitwig preset can be made from (which waits for an export to finish).
    let bitwig = if file { ext == ".bwpreset" } else { can_be_bitwig(&ext) };
    let mut out = if file {
        // A preset file is already a file: show it, copy where it is, or send a Bitwig one into Bitwig.
        vec![offer("Send to Bitwig", act::TO_BITWIG, bitwig && !busy)]
    } else {
        vec![offer("Export", act::EXPORT, !busy), offer("Send to Bitwig", act::TO_BITWIG, !busy && bitwig)]
    };
    match hearing {
        // Stop always works, so a part never plays on with no way to stop it from here.
        Hearing::Starting => out.push(offer("Stop playing in Bitwig", act::AUDITION, true)),
        Hearing::Playing => {
            out.push(offer("Stop playing in Bitwig", act::AUDITION, true));
            out.push(offer("Keep in Bitwig", act::KEEP, true));
        }
        Hearing::Off => out.push(offer("Audition in Bitwig", act::AUDITION, bitwig && (file || !busy))),
    }
    if file {
        out.push(offer("Show in folder", act::REVEAL, true));
        out.push(offer("Copy path", act::COPY_PATH, true));
    } else {
        out.push(offer("Show in Songs", act::SHOW_IN_SONGS, true));
        out.push(offer("Show the song's file", act::REVEAL, true));
        out.push(offer("Copy the song's path", act::COPY_PATH, true));
    }
    out
}

/// Which line an audition is of, however the list is filtered or sorted meanwhile: its song's save and the sound's key
/// there, or the preset file.
pub fn audition_id(i: &Item) -> String {
    match &i.row.source {
        Source::Song { save, key } => format!("{save}\0{key}"),
        Source::File { .. } => i.path().to_owned(),
    }
}

/// Whether a sound's settings can go into a Bitwig preset: a VST2 or VST3 plugin's (not an FL plugin's own state).
pub fn can_be_bitwig(extension: &str) -> bool {
    !mo_presets::bitwig::state_format(extension).is_empty()
}

/// The part fixes on the right-click menu and the card: each part (the one set marked), then Use the guess.
pub fn part_offers(i: &Item) -> Vec<(String, String, bool)> {
    let mut out: Vec<(String, String, bool)> = Role::ALL
        .iter()
        .map(|r| (r.label().to_owned(), format!("{}{}", act::PART, r.key()), i.row.fixed == Some(*r)))
        .collect();
    if i.row.fixed.is_some() {
        out.push(("Use the guess".into(), act::USE_GUESS.into(), false));
    }
    out
}

/// The part a fix id names ("presets.part.bass" is Bass).
pub fn part_of(id: &str) -> Option<Role> {
    Role::from_key(id.strip_prefix(act::PART)?)
}

/// What to export for the lines shown (Export these): each save with the keys of its sounds shown, songs in list
/// order. Preset files aren't exported (they're files already), and are counted.
pub fn export_plan(items: &[Item], shown: &[usize]) -> (Vec<(String, Vec<String>)>, usize) {
    let mut order: Vec<String> = Vec::new();
    let mut by: HashMap<String, Vec<String>> = HashMap::new();
    let mut files = 0;
    for &i in shown {
        match &items[i].row.source {
            Source::Song { save, key } => by
                .entry(save.clone())
                .or_insert_with(|| {
                    order.push(save.clone());
                    Vec::new()
                })
                .push(key.clone()),
            Source::File { .. } => files += 1,
        }
    }
    (order.into_iter().map(|s| (by.remove(&s).unwrap_or_default(), s)).map(|(k, s)| (s, k)).collect(), files)
}

/// The card's line about an export: what was written, how many also as Bitwig presets, what failed, and which
/// plugins could get a Bitwig preset after one save in Bitwig (the C# `ExportSummary`). `without` is those plugins.
pub fn export_summary(done: &[Exported], without: &[String], failed_saves: &[String]) -> String {
    let files: Vec<&String> = done.iter().flat_map(|e| &e.files).collect();
    // A .bwpreset made beside a sound's own file (same name, another ending) is the extra; a Bitwig device's sound is
    // a .bwpreset on its own, and counts as a preset.
    let is_bw = |f: &str| f.to_lowercase().ends_with(".bwpreset");
    let stem = |f: &str| f.rsplit_once('.').map_or(f, |(s, _)| s).to_lowercase();
    let own: HashSet<String> = done
        .iter()
        .flat_map(|e| e.files.iter().filter(|f| !is_bw(f)).map(move |f| (e.folder.clone(), stem(f))))
        .map(|(folder, s)| format!("{}|{s}", folder.display()))
        .collect();
    let bitwig = done
        .iter()
        .flat_map(|e| e.files.iter().filter(|f| is_bw(f)).map(move |f| format!("{}|{}", e.folder.display(), stem(f))))
        .filter(|k| own.contains(k))
        .count();
    let plain = files.len() - bitwig;
    let mut text = if files.is_empty() {
        "Nothing was exported.".to_owned()
    } else if files.len() == 1 {
        format!("Exported {}.", files[0])
    } else {
        format!("Exported {} preset{}", thousands(plain), if plain == 1 { "" } else { "s" })
    };
    if bitwig > 0 {
        let how_many = if bitwig == plain { "all".to_owned() } else { thousands(bitwig) };
        text += &format!(", {how_many} also as Bitwig presets (.bwpreset) that load in Bitwig with their settings.");
    } else if !text.ends_with('.') {
        text.push('.');
    }
    let failed: Vec<&String> = done.iter().flat_map(|e| &e.failed).chain(failed_saves).collect();
    if !failed.is_empty() {
        text += &format!(
            " {} couldn't be: {}{}",
            thousands(failed.len()),
            failed.iter().take(2).map(|s| s.as_str()).collect::<Vec<_>>().join("; "),
            if failed.len() > 2 { format!(" and {} more.", failed.len() - 2) } else { ".".into() }
        );
    }
    if let Some(w) = done.iter().find_map(|e| e.warning.as_deref()) {
        text += &format!(" {w}");
    }
    if !without.is_empty() {
        let more = if without.len() > 3 { format!(" and {} more", without.len() - 3) } else { String::new() };
        text += &format!(
            " For a Bitwig preset of {}{more}, save a preset of any plugin of the same kind (VST2 or VST3) once in \
             Bitwig, then export again.",
            without.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
    }
    text
}

/// The card's line once a preset is in Bitwig.
pub fn sent_words(name: &str) -> String {
    format!("{name} is in Bitwig, on a new track.")
}

/// The card's line when a preset couldn't go into Bitwig: why, and that it's shown to drag in instead.
pub fn send_failed(why: &str) -> String {
    let why = why.trim_end_matches('.');
    let mut c = why.chars();
    let why: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
    format!("Couldn't put it into Bitwig: {why}. The Bitwig preset is shown in its folder: drag it onto a track.")
}

// ---------------------------------------------------------------------------------------------------------------------
// The Curves section

/// The Curves section's actions, by their shortcut ids (see `shortcuts::CURVES_ACTIONS`).
pub mod curve_act {
    pub const TO_BITWIG: &str = "curves.toBitwig";
    pub const REVEAL: &str = "curves.reveal";
    pub const RENAME: &str = "curves.rename";
    pub const HIDE: &str = "curves.hide";
    pub const OPEN_SONG: &str = "curves.revealSong";
    pub const SHOW_IN_SONGS: &str = "curves.showInSongs";
}

/// What the curves list is filtered by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CurveFilters {
    pub search: String,
    /// Only curves found in one of these ways; none for every way.
    pub kinds: Vec<SourceKind>,
    /// Show the curves you hid.
    pub hidden: bool,
}

pub fn kind_label(k: SourceKind) -> &'static str {
    match k {
        SourceKind::FlKnobMoves => "FL knob moves",
        SourceKind::BitwigClip => "Bitwig clips",
        SourceKind::BitwigCurve => "Bitwig's library",
    }
}

pub const KINDS: [SourceKind; 3] = [SourceKind::FlKnobMoves, SourceKind::BitwigClip, SourceKind::BitwigCurve];

pub fn kind_from(l: &str) -> Option<SourceKind> {
    KINDS.into_iter().find(|k| kind_label(*k) == l)
}

impl CurveFilters {
    pub fn passes(&self, c: &Curve) -> bool {
        self.passes_rest(c) && self.found_in(c)
    }

    /// Whether the search and Show hidden let it through, whichever way it was found.
    fn passes_rest(&self, c: &Curve) -> bool {
        (self.hidden || !c.hidden)
            && filter::search_matches(&self.search, &curve_texts(c).iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// Whether it was found in one of the ways picked (or none is picked).
    fn found_in(&self, c: &Curve) -> bool {
        self.kinds.is_empty() || c.sources.iter().any(|s| self.kinds.contains(&s.kind))
    }

    pub fn any(&self) -> bool {
        !self.kinds.is_empty()
    }

    /// Picks or unpicks a way curves are found, by its words: more than one can be picked.
    pub fn toggle_kind(&mut self, label: &str) {
        chips::toggle_as(&mut self.kinds, label, true, kind_label, kind_from);
    }
}

fn curve_texts(c: &Curve) -> Vec<String> {
    let mut out = vec![c.name.clone()];
    for s in &c.sources {
        out.extend([s.song.clone(), s.device.clone(), s.setting.clone(), s.name.clone()]);
    }
    out
}

/// What the curves list searches and sorts a curve by, worked out once when the curves are read or one changes, so a
/// key typed in the search makes no copies (H-5).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurveKey {
    /// Every text a search looks in, in lower case, joined.
    words: String,
    /// Its newest place's time.
    newest: i64,
    /// Its name in lower case.
    name: String,
}

impl CurveKey {
    pub fn of(c: &Curve) -> CurveKey {
        CurveKey {
            words: curve_texts(c).iter().map(|t| t.to_lowercase()).collect::<Vec<_>>().join("\u{1f}"),
            newest: c.sources.iter().map(|s| s.modified).max().unwrap_or(0),
            name: c.name.to_lowercase(),
        }
    }

    pub fn all(curves: &[Curve]) -> Vec<CurveKey> {
        curves.iter().map(CurveKey::of).collect()
    }
}

/// The curves shown, as places in `curves`: newest found first (by their newest place), then by name. With them, the
/// Found in filter as its chips show it: each way curves are found, with how many of them the search and Show hidden
/// let through. Both come from one look at each curve, as this runs with each key typed in the search. `keys` are
/// [`CurveKey::all`] of `curves`.
pub fn shown_curves(curves: &[Curve], keys: &[CurveKey], f: &CurveFilters) -> (Vec<usize>, Filter) {
    let words: Vec<String> = f.search.split_whitespace().map(str::to_lowercase).collect();
    let passes =
        |c: &Curve, k: &CurveKey| (f.hidden || !c.hidden) && words.iter().all(|w| k.words.contains(w.as_str()));
    let mut found = [0usize; KINDS.len()];
    let mut out = Vec::new();
    for (i, (c, _)) in curves.iter().zip(keys).enumerate().filter(|(_, (c, k))| passes(c, k)) {
        for (n, kind) in found.iter_mut().zip(KINDS) {
            if c.sources.iter().any(|s| s.kind == kind) {
                *n += 1;
            }
        }
        if f.found_in(c) {
            out.push(i);
        }
    }
    out.sort_by(|&a, &b| keys[b].newest.cmp(&keys[a].newest).then_with(|| keys[a].name.cmp(&keys[b].name)));
    let choices: Vec<(String, String)> =
        KINDS.iter().zip(found).map(|(&k, n)| (kind_label(k).to_owned(), thousands(n))).collect();
    let picked: Vec<String> = f.kinds.iter().map(|&k| kind_label(k).to_owned()).collect();
    (out, chips::show(&choices, &picked))
}

/// One line of the curves list, in words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurveLine {
    pub name: String,
    /// What it moved: "Serum · Cutoff", or where it came from for a library curve.
    pub under: String,
    /// "In 3 songs", "In Bitwig's library".
    pub found: String,
    /// How long it was: "4 bars", "3 beats".
    pub length: String,
    pub hidden: bool,
    /// Its shape as path commands in a box three times as wide as high, 300 by 100 (see [`curve_path`], [`SHAPE_BOX`]).
    pub shape: String,
}

/// How long, in bars of four beats where it's whole bars, else in beats.
pub fn beats_words(beats: f64) -> String {
    if beats <= 0.0 {
        return String::new();
    }
    let bars = beats / 4.0;
    let one = |n: f64, what: &str| {
        let text = if (n - n.round()).abs() < 0.01 { format!("{}", n.round() as i64) } else { format!("{n:.1}") };
        format!("{text} {what}{}", if text == "1" { "" } else { "s" })
    };
    if beats >= 4.0 && (bars - bars.round()).abs() < 0.01 { one(bars, "bar") } else { one(beats, "beat") }
}

/// Where a curve was found, in a few words.
pub fn found_words(c: &Curve) -> String {
    let songs: HashSet<&str> =
        c.sources.iter().filter(|s| s.kind != SourceKind::BitwigCurve).map(|s| s.song.as_str()).collect();
    let library = c.sources.iter().any(|s| s.kind == SourceKind::BitwigCurve);
    match (songs.len(), library) {
        (0, _) => "In Bitwig's library".into(),
        (n, lib) => format!(
            "In {} song{}{}",
            thousands(n),
            if n == 1 { "" } else { "s" },
            if lib { " and Bitwig's library" } else { "" }
        ),
    }
}

fn under_words(c: &Curve) -> String {
    let Some(s) = c.sources.first() else { return String::new() };
    let what = [s.device.as_str(), s.setting.as_str()].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>();
    if what.is_empty() { kind_label(s.kind).to_owned() } else { what.join(" · ") }
}

pub fn curve_line(c: &Curve) -> CurveLine {
    CurveLine {
        name: c.name.clone(),
        under: under_words(c),
        found: found_words(c),
        length: beats_words(c.length_beats),
        hidden: c.hidden,
        shape: curve_path(&c.points, SHAPE_BOX.0, SHAPE_BOX.1),
    }
}

/// The box the page draws curves in: a Slint path keeps its shape when it's fitted to its place, so the page's
/// boxes for curves are this shape too (three wide to one high).
pub const SHAPE_BOX: (f64, f64) = (300.0, 100.0);

/// A curve's shape as path commands (for a Slint `Path` with a `w` by `h` viewbox, 0,0 at the top left), value 1 at
/// the top. A bent line between two points is drawn as a quadratic curve leaning toward one corner of the box the
/// two points make: toward the low-then-high corner for a bend above 0, the other way below. Bitwig's own bend maths
/// isn't known, so a bend looks right in direction and strength, not to the pixel.
pub fn curve_path(points: &[CurvePoint], w: f64, h: f64) -> String {
    let at = |p: &CurvePoint| (p.x.clamp(0.0, 1.0) * w, (1.0 - p.y.clamp(0.0, 1.0)) * h);
    let mut out = String::new();
    let Some(first) = points.first() else { return out };
    let (x0, y0) = at(first);
    // A curve whose first point is past the start holds that value from the start.
    out += &format!("M 0 {} L {} {}", num(y0), num(x0), num(y0));
    for pair in points.windows(2) {
        let [a, b] = pair else { continue };
        let ((ax, ay), (bx, by)) = (at(a), at(b));
        let bend = a.bend.clamp(-1.0, 1.0);
        if bend.abs() < 0.01 || (ay - by).abs() < 0.01 {
            out += &format!(" L {} {}", num(bx), num(by));
        } else {
            let (mx, my) = ((ax + bx) / 2.0, (ay + by) / 2.0);
            // Above 0: late (stays near the first value, then moves); below: early.
            let (cx, cy) = if bend > 0.0 { (bx, ay) } else { (ax, by) };
            let t = bend.abs();
            out += &format!(" Q {} {} {} {}", num(mx + (cx - mx) * t), num(my + (cy - my) * t), num(bx), num(by));
        }
    }
    if let Some(last) = points.last() {
        let (_, y) = at(last);
        out += &format!(" L {} {}", num(w), num(y));
    }
    out
}

fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

/// The picked curve's card, in words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurveCard {
    pub name: String,
    pub shape: String,
    pub facts: Vec<(String, String)>,
    /// Each place it was found: (song or file, what it moved and where).
    pub places: Vec<(String, String)>,
    pub hidden: bool,
}

/// The beat a curve starts at in its song, as "bar 17".
fn bar_words(at: f64) -> String {
    format!("bar {}", (at / 4.0).floor() as i64 + 1)
}

/// How many places a curve's card lists, newest first.
pub const PLACES_UP_TO: usize = 20;

pub fn curve_card(c: &Curve) -> CurveCard {
    let mut facts =
        vec![("LENGTH".to_owned(), beats_words(c.length_beats)), ("POINTS".into(), c.points.len().to_string())];
    facts.push(("FOUND".into(), found_words(c)));
    // The newest places only: a common shape (any ramp) can be found in hundreds, and the card isn't a list (H-7).
    let mut sources: Vec<&mo_presets::curves::CurveSource> = c.sources.iter().collect();
    sources.sort_by_key(|s| std::cmp::Reverse(s.modified));
    let more = sources.len().saturating_sub(PLACES_UP_TO);
    sources.truncate(PLACES_UP_TO);
    let mut places: Vec<(String, String)> = sources
        .into_iter()
        .map(|s| {
            let what =
                [s.device.as_str(), s.setting.as_str()].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>();
            let mut about = vec![kind_label(s.kind).to_owned()];
            if !what.is_empty() {
                about.push(what.join(" · "));
            }
            if s.kind != SourceKind::BitwigCurve {
                about.push(bar_words(s.at));
            }
            let title = if s.name.is_empty() || s.name == s.song {
                s.song.clone()
            } else {
                format!("{} · {}", s.song, s.name)
            };
            (title, about.join(" · "))
        })
        .collect();
    if more > 0 {
        places.push((
            format!("And {} more {}", thousands(more), if more == 1 { "place" } else { "places" }),
            String::new(),
        ));
    }
    CurveCard {
        name: c.name.clone(),
        shape: curve_path(&c.points, SHAPE_BOX.0, SHAPE_BOX.1),
        facts,
        places,
        hidden: c.hidden,
    }
}

/// What the picked curve offers, in the card's order.
pub fn curve_offers(c: &Curve, has_bitwig: bool) -> Vec<Offer> {
    let in_song = c.sources.iter().any(|s| s.kind != SourceKind::BitwigCurve);
    vec![
        offer("Save to Bitwig's curves", curve_act::TO_BITWIG, has_bitwig),
        offer("Show curve file", curve_act::REVEAL, true),
        offer("Rename", curve_act::RENAME, true),
        offer(if c.hidden { "Show again" } else { "Hide" }, curve_act::HIDE, true),
        offer("Show in Songs", curve_act::SHOW_IN_SONGS, in_song),
        offer("Show the song's file", curve_act::OPEN_SONG, in_song),
    ]
}

/// The Curves section's line under the title.
pub fn curves_summary(total: usize, shown: usize, hidden: usize) -> String {
    if total == 0 {
        return String::new();
    }
    let what = format!("{} curve{}", thousands(total - hidden), if total - hidden == 1 { "" } else { "s" });
    let hid = if hidden > 0 { format!(", {} hidden", thousands(hidden)) } else { String::new() };
    if shown == total - hidden || shown == total {
        format!("{what}{hid}")
    } else {
        format!("Showing {} of {what}{hid}", thousands(shown))
    }
}

/// The curves list's words when it shows nothing.
pub fn curves_empty(total: usize, shown: usize, reading: bool, failed: Option<&str>) -> (String, String) {
    if let Some(why) = failed {
        return ("Couldn't read the curves".into(), why.to_owned());
    }
    if shown > 0 {
        return (String::new(), String::new());
    }
    if total == 0 {
        return if reading {
            ("Looking for curves".into(), "Each song is read once; after that the curves show at once.".into())
        } else {
            (
                "No curves yet".into(),
                "Curves are the shapes your settings moved in: FL Studio knob moves recorded into patterns, Bitwig \
                 automation clips, and the curves in Bitwig's library."
                    .into(),
            )
        };
    }
    ("No curves match".into(), "Try other words, or show the hidden ones.".into())
}

/// The scan's progress line: "Looking for curves: 120 of 800 songs".
pub fn curves_busy(done: usize, total: usize) -> String {
    if total == 0 {
        "Looking for curves…".into()
    } else {
        format!("Looking for curves: {} of {} saves", thousands(done), thousands(total))
    }
}

/// The Sounds section's reading line.
pub fn sounds_busy(done: usize, total: usize) -> String {
    if total == 0 {
        "Reading your songs…".into()
    } else {
        format!("Reading the sounds in your songs: {} of {}", thousands(done), thousands(total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_presets::curves::CurveSource;
    use mo_presets::role::Guess;

    fn row(name: &str, plugin: &str, role: Option<Role>, save: &str) -> PresetRow {
        PresetRow {
            source: Source::Song {
                save: save.into(),
                key: format!("{}|{}", name.to_lowercase(), plugin.to_lowercase()),
            },
            name: name.into(),
            plugin: plugin.into(),
            kind: PluginKind::Instrument,
            native: false,
            place: name.into(),
            extension: ".fxp".into(),
            guess: Guess { role, basis: Basis::Notes, reason: "low notes".into(), octave_checked: true, weak: false },
            plays_nothing: false,
            hash: format!("h-{name}-{plugin}"),
            fixed: None,
        }
    }

    fn file(name: &str, path: &str) -> PresetRow {
        PresetRow {
            source: Source::File { path: path.into() },
            hash: String::new(),
            extension: ".bwpreset".into(),
            place: "Presets".into(),
            ..row(name, "Polysynth", None, "")
        }
    }

    fn items() -> Vec<Item> {
        vec![
            Item::new(row("Sub", "Serum", Some(Role::Bass), "/s/Didums_77.flp"), "Didums".into(), Some(2026)),
            Item::new(row("Hook", "Serum", Some(Role::Lead), "/s/Didums_77.flp"), "Didums".into(), Some(2026)),
            Item::new(
                PresetRow {
                    native: true,
                    kind: PluginKind::Effect,
                    ..row("Limiter", "Fruity Limiter", None, "/s/Old.flp")
                },
                "Old".into(),
                Some(2019),
            ),
            Item::new(file("Warm Pad", "/b/Presets/Warm Pad.bwpreset"), String::new(), Some(2024)),
        ]
    }

    #[test]
    fn a_sounds_format_comes_from_its_file_and_the_daws_own_are_native() {
        let with = |ext: &str, native: bool| {
            Format::of(&PresetRow { extension: ext.into(), native, ..row("a", "b", None, "") })
        };
        assert_eq!(with(".fxp", false), Format::Vst2);
        assert_eq!(with(".FXB", false), Format::Vst2);
        assert_eq!(with(".vstpreset", false), Format::Vst3);
        assert_eq!(with(".vst3state", false), Format::Vst3);
        assert_eq!(with(".clap-preset", false), Format::Clap);
        assert_eq!(with(".aupreset", false), Format::Au);
        assert_eq!(with(".flstate", true), Format::Native);
        assert_eq!(with(".bwpreset", true), Format::Native);
        assert_eq!(with(".bwpreset", false), Format::Other);
    }

    #[test]
    fn no_sounds_match_only_once_reading_and_filtering_are_done() {
        let words = |reading, filtering| empty_words(100, 0, reading, filtering, true, None).0;
        assert_eq!(words(true, false), "Reading your songs");
        assert_eq!(words(true, true), "Reading your songs");
        assert_eq!(words(false, true), "");
        assert_eq!(words(false, false), "No sounds match");
        assert_eq!(empty_words(100, 3, true, true, true, None).0, "");
    }

    #[test]
    fn the_format_filter_picks_any_of_its_formats() {
        let all = items();
        let mut f = Filters::default();
        f.toggle(FORMAT, "VST2");
        assert_eq!(shown(&all, &f), [0, 1]);
        f.toggle(FORMAT, "Native");
        assert_eq!(f.formats, [Format::Vst2, Format::Native]);
        assert_eq!(shown(&all, &f), [0, 1, 2]);
        assert!(f.any());
        // The chips: VST2, VST3, CLAP and Native always; Other because a line has it; AU not, as none has it.
        let chips = filter_chips(&counts(&all, &f), &plugins(&all), &f);
        let words: Vec<(&str, &str, bool)> =
            chips.format.chips.iter().map(|c| (c.text.as_str(), c.count.as_str(), c.on)).collect();
        assert_eq!(
            words,
            [
                ("VST2", "2", true),
                ("VST3", "0", false),
                ("CLAP", "0", false),
                ("Native", "1", true),
                ("Other", "1", false)
            ]
        );
        f.clear_chip(FORMAT);
        assert_eq!(shown(&all, &f), [0, 1, 2, 3]);
    }

    #[test]
    fn filters_and_counts_go_together() {
        let all = items();
        let f = Filters::default();
        assert_eq!(shown(&all, &f), [0, 1, 2, 3]);
        let c = counts(&all, &f);
        assert_eq!((c.formats, c.synths, c.effects), ([2, 0, 0, 0, 1, 1], 3, 1));
        let synths = Filters { choice: Choice { kind: Type::Synths, ..Choice::default() }, ..f.clone() };
        assert_eq!(shown(&all, &synths), [0, 1, 3]);
        // The type counts beside the chip keep the maker as it is, and ignore the type itself.
        assert_eq!(counts(&all, &synths).effects, 1);
        // Every filter's counts keep the others as they are: with Bass picked, the part counts ignore it, and the
        // rest count only the bass line.
        let bass = Filters { parts: vec![PartPick::Role(Role::Bass)], ..f.clone() };
        let c = counts(&all, &bass);
        let at = |r: Role| Role::ALL.iter().position(|&x| x == r).unwrap();
        assert_eq!((c.parts[at(Role::Bass)], c.parts[at(Role::Lead)], c.parts[Role::ALL.len()]), (1, 1, 2));
        assert_eq!((c.formats[0], c.formats[4], c.songs, c.files), (1, 0, 1, 0));
        assert_eq!(c.plugins.get("serum"), Some(&1));
        assert_eq!(c.plugins.get("fruity limiter"), None);
        // A line two filters leave out counts beside neither.
        let two = Filters { from: From::Files, ..bass.clone() };
        assert_eq!(counts(&all, &two).parts[Role::ALL.len()], 1);
        assert_eq!(counts(&all, &two).formats.iter().sum::<usize>(), 0);
        assert_eq!(shown(&all, &Filters { parts: vec![PartPick::Role(Role::Bass)], ..f.clone() }), [0]);
        assert_eq!(shown(&all, &Filters { parts: vec![PartPick::NoGuess], ..f.clone() }), [2, 3]);
        // More than one part: any of them.
        let bass_or_none = vec![PartPick::Role(Role::Bass), PartPick::NoGuess];
        assert_eq!(shown(&all, &Filters { parts: bass_or_none, ..f.clone() }), [0, 2, 3]);
        assert_eq!(shown(&all, &Filters { from: From::Files, ..f.clone() }), [3]);
        assert_eq!(shown(&all, &Filters { plugins: vec!["serum".into()], ..f.clone() }), [0, 1]);
        let serum_or_limiter = vec!["Serum".into(), "fruity limiter".into()];
        assert_eq!(shown(&all, &Filters { plugins: serum_or_limiter, ..f.clone() }), [0, 1, 2]);
        assert_eq!(shown(&all, &Filters { song: Some("/s/Old.flp".into()), ..f.clone() }), [2]);
        assert_eq!(shown(&all, &Filters { years: Some((2024, 2026)), ..f.clone() }), [0, 1, 3]);
        assert_eq!(shown(&all, &Filters { search: "didums hook".into(), ..f.clone() }), [1]);
        // The part is searched too.
        assert_eq!(shown(&all, &Filters { search: "bass".into(), ..f.clone() }), [0]);
        assert!(!f.any() && Filters { from: From::Songs, ..f.clone() }.any());
    }

    #[test]
    fn the_chips_pick_one_and_the_menus_more() {
        let all = items();
        let mut f = Filters::default();
        // Two choices each: chips of their own, picking one or none.
        f.toggle(TYPE, "Synths");
        assert_eq!(f.choice.kind, Type::Synths);
        f.toggle(TYPE, "Effects");
        assert_eq!(f.choice.kind, Type::Effects);
        f.toggle(TYPE, "Effects");
        assert_eq!(f.choice.kind, Type::All);
        f.toggle(FROM, From::Files.label());
        assert_eq!(f.from, From::Files);
        // The part and the plugin: more than one.
        f.toggle(PART, Role::Bass.label());
        f.toggle(PART, NO_PART);
        assert_eq!(f.parts, [PartPick::Role(Role::Bass), PartPick::NoGuess]);
        f.toggle(PLUGIN, "Serum");
        f.toggle(PLUGIN, "Vital");
        f.toggle(PLUGIN, "serum");
        assert_eq!(f.plugins, ["Vital"]);
        let shown = filter_chips(&counts(&all, &f), &plugins(&all), &f);
        assert_eq!(shown.kind.chips.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Synths", "Effects"]);
        assert_eq!(shown.from.chips.iter().map(|c| c.on).collect::<Vec<_>>(), [false, true]);
        // Each choice says how many lines it would show with the other filters as they are. No line is Vital's, so
        // with it picked no other filter's choice shows any; the plugin's own count without it: of the preset files
        // with Bass or no guess, Polysynth's Warm Pad.
        let counted = |c: &Filter| -> Vec<(String, String, bool)> {
            c.chips.iter().chain(&c.more).map(|s| (s.text.clone(), s.count.clone(), s.on)).collect()
        };
        assert!(counted(&shown.from).iter().all(|(_, n, _)| n == "0"));
        // The most used first (Serum), whatever the counts say now.
        let plugin = counted(&shown.plugin);
        assert_eq!(plugin[0], ("Serum".to_owned(), "0".to_owned(), false));
        assert!(plugin.contains(&("Polysynth".to_owned(), "1".to_owned(), false)));
        // A picked plugin that no line has (after a new look at the songs) stays on show, picked, to be unpicked.
        assert!(counted(&shown.plugin).contains(&("Vital".to_owned(), "0".to_owned(), true)));
        // The parts fit as chips of their own (there's room for them all), the two picked ones lit.
        assert!(shown.part.more.is_empty() && shown.part.chips.len() == Role::ALL.len() + 1);
        let lit: Vec<&str> = shown.part.chips.iter().filter(|c| c.on).map(|c| c.text.as_str()).collect();
        assert_eq!(lit, [Role::Bass.label(), NO_PART]);
        f.clear_chip(PART);
        assert!(f.parts.is_empty() && f.any());
        f.clear_chip(PLUGIN);
        f.clear_chip(FROM);
        assert!(!f.any());
    }

    #[test]
    fn unused_synths_are_tucked_away_until_asked_for() {
        let mut all = items();
        all[0].row.plays_nothing = true;
        assert_eq!(shown(&all, &Filters::default()), [1, 2, 3]);
        assert_eq!(shown(&all, &Filters { unused: true, ..Filters::default() }), [0, 1, 2, 3]);
    }

    #[test]
    fn files_go_after_the_songs() {
        let mut all = items();
        all.rotate_right(1);
        assert_eq!(shown(&all, &Filters::default()), [1, 2, 3, 0]);
    }

    #[test]
    fn years_count_without_the_year_filter() {
        let all = items();
        let f = Filters { years: Some((2019, 2019)), ..Filters::default() };
        assert_eq!(year_counts(&all, &f), [(2026, 2), (2024, 1), (2019, 1)]);
    }

    #[test]
    fn a_fix_is_laid_over_every_copy_of_the_sound() {
        let mut all = items();
        all.push(Item::new(row("Sub", "Serum", Some(Role::Bass), "/s/Other.flp"), "Other".into(), None));
        let mut fixes = Fixes::new();
        fixes.insert("h-Sub-Serum".into(), Some(Role::Lead));
        assert_eq!(all[0].role_with(&fixes), Some(Role::Lead));
        assert_eq!(all[4].role_with(&fixes), Some(Role::Lead));
        // The shared line itself is left as it was.
        assert_eq!(all[0].role(), Some(Role::Bass));
        assert!(part_reason(&all[0].as_fixed(&fixes)).starts_with("You set this to Lead. The guess was bass"));
        fixes.insert("h-Sub-Serum".into(), None);
        assert_eq!(all[0].role_with(&fixes), Some(Role::Bass));
        assert!(matches!(all[0].as_fixed(&fixes), Cow::Borrowed(_)));
    }

    #[test]
    fn lines_and_cards_say_what_each_sound_is() {
        let all = items();
        let l = line(&all[0]);
        assert_eq!(
            (l.name.as_str(), l.under.as_str(), l.part.as_str(), l.from.as_str()),
            ("Sub", "Serum", "Bass", "Didums")
        );
        assert_eq!(l.colour, 8);
        let mut weak = all[1].clone();
        weak.row.guess.weak = true;
        assert_eq!(line(&weak).part, "Lead?");
        let c = card(&all[2]);
        assert_eq!(c.over, "EFFECT · FL STUDIO PLUGIN · IN A SONG");
        assert_eq!(c.part, "No part guessed");
        assert_eq!(c.hint, "Load into the VST2 version of the plugin.");
        let f = card(&all[3]);
        assert!(!f.fixable && f.facts[0] == ("FOLDER".into(), "Presets".into()));
        assert_eq!(card(&all[0]).part, "Bass (from its notes)");
    }

    #[test]
    fn offers_fit_the_line() {
        let all = items();
        let ids = |i: &Item| {
            offers(i, false, Hearing::Off).into_iter().filter(|o| o.enabled).map(|o| o.id).collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&all[0]),
            [act::EXPORT, act::TO_BITWIG, act::AUDITION, act::SHOW_IN_SONGS, act::REVEAL, act::COPY_PATH]
        );
        let mut fl_own = all[2].clone();
        fl_own.row.extension = ".flstate".into();
        assert!(!ids(&fl_own).contains(&act::TO_BITWIG), "an FL plugin's own state can't go into Bitwig");
        assert!(!ids(&fl_own).contains(&act::AUDITION), "nor be auditioned there");
        assert_eq!(ids(&all[3]), [act::TO_BITWIG, act::AUDITION, act::REVEAL, act::COPY_PATH]);
        // While a preset file is auditioned: Stop playing in Bitwig, then Keep in Bitwig once it plays, even mid-export.
        let texts = |h| offers(&all[3], true, h).into_iter().filter(|o| o.enabled).map(|o| o.text).collect::<Vec<_>>();
        assert_eq!(texts(Hearing::Starting), ["Stop playing in Bitwig", "Show in folder", "Copy path"]);
        assert_eq!(
            texts(Hearing::Playing),
            ["Stop playing in Bitwig", "Keep in Bitwig", "Show in folder", "Copy path"]
        );
        // A sound in a song waits for an export to finish, as Send to Bitwig does.
        assert!(!offers(&all[0], true, Hearing::Off).iter().any(|o| o.id == act::AUDITION && o.enabled));
        assert_ne!(audition_id(&all[0]), audition_id(&all[3]));
        assert_eq!(part_of("presets.part.bass"), Some(Role::Bass));
        assert_eq!(part_of("presets.export"), None);
        let mut fixed = all[0].clone();
        fixed.row.fixed = Some(Role::Pad);
        let parts = part_offers(&fixed);
        assert!(parts.iter().any(|p| p.0 == "Pad" && p.2));
        assert_eq!(parts.last().map(|p| p.1.as_str()), Some(act::USE_GUESS));
    }

    #[test]
    fn export_plan_groups_by_save() {
        let all = items();
        let (plan, files) = export_plan(&all, &[0, 2, 1, 3]);
        assert_eq!(files, 1);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].0, "/s/Didums_77.flp");
        assert_eq!(plan[0].1.len(), 2);
    }

    #[test]
    fn export_summary_reads_like_the_old_one() {
        let e = |files: &[&str]| Exported {
            folder: PathBuf::from("/x"),
            files: files.iter().map(|f| f.to_string()).collect(),
            failed: Vec::new(),
            warning: None,
        };
        assert_eq!(export_summary(&[e(&["Sub.fxp"])], &[], &[]), "Exported Sub.fxp.");
        assert_eq!(
            export_summary(&[e(&["Sub.fxp", "Sub.bwpreset"])], &[], &[]),
            "Exported 1 preset, all also as Bitwig presets (.bwpreset) that load in Bitwig with their settings."
        );
        // A Bitwig device's own sound is a .bwpreset, not an extra.
        assert_eq!(export_summary(&[e(&["Polysynth.bwpreset"])], &[], &[]), "Exported Polysynth.bwpreset.");
        assert_eq!(
            export_summary(&[e(&["Polysynth.bwpreset", "Sub.fxp", "Sub.bwpreset"])], &[], &[]),
            "Exported 2 presets, 1 also as Bitwig presets (.bwpreset) that load in Bitwig with their settings."
        );
        let s = export_summary(&[e(&["A.fxp", "B.fxp"])], &["Serum".into()], &[]);
        assert_eq!(
            s,
            "Exported 2 presets. For a Bitwig preset of Serum, save a preset of any plugin of the same kind (VST2 or \
             VST3) once in Bitwig, then export again."
        );
    }

    #[test]
    fn every_part_fix_is_on_the_shortcut_list() {
        // Rule 15: each menu line's action has a key that can be changed.
        for r in Role::ALL {
            let id = format!("{}{}", act::PART, r.key());
            assert!(crate::shortcuts::PRESETS_ACTIONS.iter().any(|a| a.id == id), "{id}");
        }
        for id in [
            act::EXPORT,
            act::TO_BITWIG,
            act::EXPORT_SHOWN,
            act::REVEAL,
            act::SHOW_IN_SONGS,
            act::COPY_PATH,
            act::USE_GUESS,
        ] {
            assert!(crate::shortcuts::PRESETS_ACTIONS.iter().any(|a| a.id == id), "{id}");
        }
        for id in [
            curve_act::TO_BITWIG,
            curve_act::REVEAL,
            curve_act::RENAME,
            curve_act::HIDE,
            curve_act::OPEN_SONG,
            curve_act::SHOW_IN_SONGS,
        ] {
            assert!(crate::shortcuts::CURVES_ACTIONS.iter().any(|a| a.id == id), "{id}");
        }
    }

    #[test]
    fn send_words() {
        assert_eq!(sent_words("Sub"), "Sub is in Bitwig, on a new track.");
        assert_eq!(
            send_failed("bitwig isn't open."),
            "Couldn't put it into Bitwig: Bitwig isn't open. The Bitwig preset is shown in its folder: drag it onto a \
             track."
        );
    }

    #[test]
    fn bitwig_folders_are_found_from_its_saves() {
        let found = bitwig_folders(&[], [r"K:\Bitwig Studio\Projects\Song\Song.bwproject", "/x/a.flp"]);
        let found: Vec<String> = found.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        // On Linux the Windows path is one name, so check with forward slashes.
        let unix = bitwig_folders(
            &[],
            ["/k/Bitwig Studio/Projects/Song/Song.bwproject", "/k/Bitwig Studio/Projects/B.bwproject"],
        );
        assert_eq!(unix, [PathBuf::from("/k/Bitwig Studio")]);
        assert!(found.len() <= 1);
        assert_eq!(bitwig_curves_folder(&unix), Some(PathBuf::from("/k/Bitwig Studio/Library/Curves")));
        let folders = preset_folders(&[PathBuf::from("/h/VST3 Presets")], &[PathBuf::from("/k/Bitwig Studio")]);
        assert_eq!(folders.len(), 2);
        // A folder inside one already there isn't walked twice.
        let folders =
            preset_folders(&[PathBuf::from("/k/Bitwig Studio/Library")], &[PathBuf::from("/k/Bitwig Studio")]);
        assert_eq!(folders, [PathBuf::from("/k/Bitwig Studio")]);
    }

    fn curve(name: &str, kind: SourceKind, song: &str, modified: i64) -> Curve {
        Curve {
            id: format!("id-{name}"),
            name: name.into(),
            points: vec![
                CurvePoint { x: 0.0, y: 0.0, bend: 0.5 },
                CurvePoint { x: 0.5, y: 1.0, bend: 0.0 },
                CurvePoint { x: 1.0, y: 0.25, bend: 0.0 },
            ],
            length_beats: 16.0,
            hidden: false,
            sources: vec![CurveSource {
                kind,
                path: format!("/s/{song}.flp"),
                song: song.into(),
                name: String::new(),
                device: "Serum".into(),
                setting: "Cutoff".into(),
                at: 64.0,
                modified,
            }],
        }
    }

    #[test]
    fn curves_filter_and_sort_newest_first() {
        let mut list = vec![
            curve("Rise", SourceKind::FlKnobMoves, "A", 10),
            curve("Fall", SourceKind::BitwigClip, "B", 20),
            curve("Wobble", SourceKind::BitwigCurve, "Wobble", 5),
        ];
        list[2].hidden = true;
        let f = CurveFilters::default();
        let keys = CurveKey::all(&list);
        let shown = |f: &CurveFilters| shown_curves(&list, &keys, f).0;
        assert_eq!(shown(&f), [1, 0]);
        assert_eq!(shown(&CurveFilters { hidden: true, ..f.clone() }), [1, 0, 2]);
        assert_eq!(shown(&CurveFilters { kinds: vec![SourceKind::FlKnobMoves], ..f.clone() }), [0]);
        assert_eq!(shown(&CurveFilters { search: "cutoff b".into(), ..f.clone() }), [1]);
        // Found in picks more than one way, and its chips count what the other filters let through.
        let mut both = CurveFilters { hidden: true, ..f.clone() };
        both.toggle_kind("FL knob moves");
        both.toggle_kind("Bitwig's library");
        let (two, chips) = shown_curves(&list, &keys, &both);
        assert_eq!(two, [0, 2]);
        let words: Vec<(&str, &str, bool)> =
            chips.chips.iter().map(|c| (c.text.as_str(), c.count.as_str(), c.on)).collect();
        assert_eq!(
            words,
            [("FL knob moves", "1", true), ("Bitwig clips", "1", false), ("Bitwig's library", "1", true)]
        );
        both.toggle_kind("FL knob moves");
        assert_eq!(both.kinds, [SourceKind::BitwigCurve]);
        assert!(shown_curves(&list, &keys, &f).1.chips.iter().all(|c| !c.on));
        assert_eq!(curves_summary(3, 2, 1), "2 curves, 1 hidden");
        // What's shown is set against every line, preset files too (5 sounds from 2 songs and 3 preset files).
        assert_eq!(summary(8, 8, 2, 3, false), "5 sounds from 2 songs, 3 preset files");
        assert_eq!(summary(8, 6, 2, 3, false), "Showing 6 of 8: 5 sounds from 2 songs, 3 preset files");
    }

    #[test]
    fn curves_are_worded() {
        let c = curve("Rise", SourceKind::FlKnobMoves, "Didums", 10);
        let l = curve_line(&c);
        assert_eq!((l.under.as_str(), l.found.as_str(), l.length.as_str()), ("Serum · Cutoff", "In 1 song", "4 bars"));
        assert_eq!(beats_words(3.0), "3 beats");
        assert_eq!(beats_words(4.0), "1 bar");
        assert_eq!(beats_words(6.0), "6 beats");
        let card = curve_card(&c);
        assert_eq!(card.places, [("Didums".to_owned(), "FL knob moves · Serum · Cutoff · bar 17".to_owned())]);
        let mut many = c.clone();
        many.sources = (0..25)
            .map(|n| CurveSource {
                song: format!("Song {n}"),
                name: String::new(),
                modified: n,
                ..c.sources[0].clone()
            })
            .collect();
        let card = curve_card(&many);
        assert_eq!(card.places.len(), PLACES_UP_TO + 1);
        assert_eq!(card.places[0].0, "Song 24");
        assert_eq!(card.places[PLACES_UP_TO].0, "And 5 more places");
        let lib = curve("Wobble", SourceKind::BitwigCurve, "Wobble", 5);
        assert_eq!(found_words(&lib), "In Bitwig's library");
        assert!(!curve_offers(&lib, true).iter().any(|o| o.id == curve_act::OPEN_SONG && o.enabled));
        assert!(!curve_offers(&c, false)[0].enabled, "no Bitwig folder, nowhere to save it");
    }

    #[test]
    fn a_curve_draws_from_edge_to_edge() {
        let c = curve("Rise", SourceKind::FlKnobMoves, "A", 1);
        let p = curve_path(&c.points, 100.0, 100.0);
        // Bent up to the top (late, so its middle leans to the low corner), straight down, then held to the end.
        assert_eq!(p, "M 0 100 L 0 100 Q 37.5 75 50 0 L 100 75 L 100 75");
        assert_eq!(curve_path(&[], 100.0, 100.0), "");
    }

    #[test]
    fn newest_save_of_each_song_that_keeps_settings() {
        assert!(keeps_settings("/a/B.FLP") && keeps_settings("/a/b.bwproject") && !keeps_settings("/a/b.song"));
        assert_eq!(year_of(1_767_225_600), Some(2026));
        assert_eq!(year_of(0), None);
    }
}
