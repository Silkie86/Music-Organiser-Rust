//! The Plugins page's logic and words: every plugin your songs use, with what to install to open more of them, which
//! plugins take room without being used, and the picked plugin's songs, installed copies and folders. Tested without
//! a window; `mo-ui`'s `plugins.rs` puts it on screen.
//!
//! Where it differs from the C# page (twin tests are a guide, CLAUDE.md): plugins are counted by **songs**, not saves
//! (a song with 300 autosaves counts once), and a missing plugin "opens" a song when the song's **newest** save misses
//! nothing else. The C# page counted every save, so an old save of a song that has since moved on could make a
//! plugin look more needed than it is. Its rows, kinds, sizes and the unused rule are the C# app's.
//!
//! The Space page reuses [`Plugins::rows`] and the sizes (spec: "Space lists unused plugins and opens their card here").

use std::collections::{HashMap, HashSet};

use crate::filters::{self, Filter};
use chrono::{Datelike, Months, NaiveDateTime};
use mo_formats::case;
use mo_formats::plugins::format_from_path;
use mo_library::plugins::{NEEDED_COUNT_KEY, PluginUse, ScanCounts, ScannedPlugin, StoredSpace, UNUSED_YEARS_KEY};
use mo_library::songs::group_songs;
use mo_library::{Library, LibraryError, PluginKind};
use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind};
use mo_space::plugins::{family, is_waves};
use mo_space::size::size_text;

/// How many years without a song using it make an installed plugin "unused", as the C# app offers.
pub const UNUSED_YEARS_CHOICES: [u32; 4] = [1, 2, 3, 5];
pub const DEFAULT_UNUSED_YEARS: u32 = 2;

/// Before this, no song using a plugin makes it unused: `years` before `now`. Space's plugins you haven't used count
/// back the same way.
pub fn unused_since(now: NaiveDateTime, years: u32) -> NaiveDateTime {
    now.checked_sub_months(Months::new(years.saturating_mul(12))).unwrap_or(now)
}
/// How many "Install these" cards to show; 0 hides them.
pub const NEEDED_COUNT_CHOICES: [usize; 5] = [0, 3, 6, 9, 12];
pub const DEFAULT_NEEDED_COUNT: usize = 6;

/// Everything the page reads from the library, in one go, away from the window.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub uses: Vec<PluginUse>,
    pub saves: Vec<SaveBrief>,
    /// Each save's song, by the song's place in `songs`.
    pub song_of: HashMap<i64, usize>,
    pub songs: Vec<SongBrief>,
    pub spaces: Vec<StoredSpace>,
    pub installed_names: Vec<String>,
    pub scans: ScanCounts,
    /// Whether the C# app has looked for installed plugins at all (with none found, every plugin counts as there).
    pub looked_for_installed: bool,
    pub unused_years: u32,
    pub needed_count: usize,
}

/// What the side card needs of a save.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SaveBrief {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub daw: String,
    pub modified: NaiveDateTime,
}

/// A song, as its saves are grouped on the Songs page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongBrief {
    pub name: String,
    /// The save the song opens.
    pub latest: i64,
}

/// Reads what the page shows. Slow on a big library (every plugin in every save, and every save to group into
/// songs), so never on the window's thread.
pub fn read(lib: &Library) -> Result<Read, LibraryError> {
    let pending = lib.pending_fixes()?;
    // As the Songs page groups them: a Fix copy waiting for Keep isn't a save yet, and a save whose file has gone
    // doesn't count (its plugins aren't read either).
    let all: Vec<_> = lib.saves()?.into_iter().filter(|s| !s.file_missing && !pending.contains(&s.path)).collect();
    let grouped = group_songs(&all, &lib.song_choices()?);
    let mut song_of = HashMap::new();
    let mut songs = Vec::with_capacity(grouped.len());
    for (i, song) in grouped.iter().enumerate() {
        for s in song.saves().chain(&song.copies) {
            song_of.insert(s.id, i);
        }
        songs.push(SongBrief { name: song.name.clone(), latest: song.latest.id });
    }
    let saves = all
        .into_iter()
        .map(|s| SaveBrief { id: s.id, name: s.name, path: s.path, daw: s.daw, modified: s.modified })
        .collect();
    let mut spaces: Vec<StoredSpace> = lib.plugin_spaces()?.into_values().collect();
    spaces.sort_by(|a, b| a.key.cmp(&b.key));
    let installed_names = lib.installed_names()?;
    let setting = |key: &str| lib.setting(key).ok().flatten().and_then(|v| v.trim().parse::<u32>().ok());
    Ok(Read {
        uses: lib.plugin_uses()?.into_iter().filter(|u| song_of.contains_key(&u.save)).collect(),
        saves,
        song_of,
        songs,
        spaces,
        looked_for_installed: !installed_names.is_empty(),
        installed_names,
        scans: lib.scan_counts()?,
        unused_years: setting(UNUSED_YEARS_KEY)
            .filter(|y| UNUSED_YEARS_CHOICES.contains(y))
            .unwrap_or(DEFAULT_UNUSED_YEARS),
        needed_count: setting(NEEDED_COUNT_KEY)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| NEEDED_COUNT_CHOICES.contains(n))
            .unwrap_or(DEFAULT_NEEDED_COUNT),
    })
}

/// A plugin's room on disk, from the C# app's last measure, in the Space engine's own shape.
fn space_of(s: &StoredSpace) -> PluginSpace {
    PluginSpace {
        key: s.key.clone(),
        name: s.name.clone(),
        vendor: s.vendor.clone(),
        folders: s
            .folders
            .iter()
            .map(|f| SpaceFolder {
                path: f.path.clone(),
                kind: match f.kind {
                    0 => SpaceKind::Program,
                    1 => SpaceKind::Presets,
                    _ => SpaceKind::Sounds,
                },
                bytes: f.bytes,
                shared_with: f.shared_with.clone(),
                added: f.added,
            })
            .collect(),
    }
}

/// One line of the list: a plugin across your songs, or an installed plugin no song uses.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginRow {
    pub name: String,
    pub vendor: String,
    pub format: String,
    pub kind: PluginKind,
    /// The name key it's grouped by.
    pub key: String,
    /// For a DAW's own device, that DAW (Bitwig's Arpeggiator isn't Studio One's); "" for a plugin.
    pub own: String,
    /// The DAWs whose songs use it, A to Z.
    pub daws: Vec<String>,
    /// On this computer, as the library last found it (every save that uses it says so).
    pub installed: bool,
    /// The saves that use it, newest first.
    pub saves: Vec<i64>,
    /// How many saves use it in each format ("VST3", "VST2" …), as the projects name it.
    pub formats: Vec<(String, u32)>,
    /// The songs with a save that uses it.
    pub songs: usize,
    /// The songs whose newest save uses it.
    pub songs_now: usize,
    /// The songs whose newest save uses it and is missing it here (what "Install these" counts).
    pub missing_now: usize,
    /// The songs whose newest save misses only this plugin: installing it opens them with nothing missing.
    pub opens: usize,
    pub last_used: Option<NaiveDateTime>,
    /// The years its saves were made in, newest first.
    pub years: Vec<i32>,
    pub room: Option<PluginSpace>,
    /// Installed, measured, and no song has used it in the years asked about (or ever).
    pub unused: bool,
}

impl PluginRow {
    pub fn bytes(&self) -> u64 {
        self.room.as_ref().map_or(0, PluginSpace::bytes)
    }

    /// Installed, and no song uses it.
    pub fn never_used(&self) -> bool {
        self.installed && self.songs == 0
    }

    pub fn is_effect(&self) -> bool {
        matches!(self.kind, PluginKind::Effect | PluginKind::NoteEffect)
    }

    /// "FL Studio native" for FL's own plugins, else the format as stored.
    pub fn format_text(&self) -> String {
        if self.format == "Native" && !self.own.is_empty() {
            format!("{} native", self.own)
        } else {
            self.format.clone()
        }
    }
}

/// A choice in one of the filters over the list (the DAWs are the fourth filter, by name). Within a filter more than
/// one can be picked, and a plugin shows when it's any of them; across filters it must be one of each's picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Chip {
    Missing,
    Installed,
    BigUnused,
    NeverUsed,
    Instruments,
    Effects,
    Vst2,
    Vst3,
    Clap,
    Native,
}

/// The filters made of chips, as the page names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// On this computer or not, and the room it takes.
    Show,
    Kind,
    Format,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Show, Group::Kind, Group::Format];

    pub fn name(self) -> &'static str {
        match self {
            Group::Show => "Show",
            Group::Kind => "Kind",
            Group::Format => "Format",
        }
    }

    /// Its chips, in the order they show.
    pub fn chips(self) -> &'static [Chip] {
        match self {
            Group::Show => &[Chip::Missing, Chip::Installed, Chip::NeverUsed, Chip::BigUnused],
            Group::Kind => &[Chip::Instruments, Chip::Effects],
            Group::Format => &[Chip::Vst2, Chip::Vst3, Chip::Clap, Chip::Native],
        }
    }
}

impl Chip {
    pub const ALL: [Chip; 10] = [
        Chip::Missing,
        Chip::Installed,
        Chip::BigUnused,
        Chip::NeverUsed,
        Chip::Instruments,
        Chip::Effects,
        Chip::Vst2,
        Chip::Vst3,
        Chip::Clap,
        Chip::Native,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Chip::Missing => "Missing",
            Chip::Installed => "Installed",
            Chip::BigUnused => "Big and unused",
            Chip::NeverUsed => "Never used",
            Chip::Instruments => "Instruments",
            Chip::Effects => "Effects",
            Chip::Vst2 => "VST2",
            Chip::Vst3 => "VST3",
            Chip::Clap => "CLAP",
            Chip::Native => "Native",
        }
    }

    pub fn from_name(name: &str) -> Option<Chip> {
        Chip::ALL.into_iter().find(|c| c.name().eq_ignore_ascii_case(name))
    }

    pub fn group(self) -> Group {
        match self {
            Chip::Missing | Chip::Installed | Chip::BigUnused | Chip::NeverUsed => Group::Show,
            Chip::Instruments | Chip::Effects => Group::Kind,
            Chip::Vst2 | Chip::Vst3 | Chip::Clap | Chip::Native => Group::Format,
        }
    }

    fn allows(self, p: &PluginRow) -> bool {
        match self {
            Chip::Missing => !p.installed,
            Chip::Installed => p.installed,
            Chip::BigUnused => p.unused && p.bytes() > 0,
            Chip::NeverUsed => p.never_used(),
            Chip::Instruments => p.kind == PluginKind::Instrument,
            Chip::Effects => p.is_effect(),
            Chip::Vst2 => p.format.eq_ignore_ascii_case("VST2"),
            Chip::Vst3 => p.format.eq_ignore_ascii_case("VST3"),
            Chip::Clap => p.format.eq_ignore_ascii_case("CLAP"),
            Chip::Native => p.format.eq_ignore_ascii_case("Native"),
        }
    }
}

/// How the list is sorted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// The most songs first (the C# page's order).
    #[default]
    MostUsed,
    Name,
    Biggest,
    LastUsed,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::MostUsed, Sort::Name, Sort::Biggest, Sort::LastUsed];

    pub fn name(self) -> &'static str {
        match self {
            Sort::MostUsed => "Most used",
            Sort::Name => "Name",
            Sort::Biggest => "Biggest",
            Sort::LastUsed => "Last used",
        }
    }

    pub fn from_name(name: &str) -> Option<Sort> {
        Sort::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// What the list is showing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub search: String,
    /// The chips picked, in the order picked.
    pub chips: Vec<Chip>,
    /// Only plugins used in songs from these DAWs.
    pub daws: Vec<String>,
    pub sort: Sort,
    /// Only plugins used in these years, first to last (the year strip).
    pub years: Option<(i32, i32)>,
}

impl Filters {
    /// Picks or unpicks a chip (more than one can be on in a filter). Turning on a space chip sorts the biggest first,
    /// as the C# page does.
    pub fn pick(&mut self, chip: Chip) {
        filters::toggle_as(&mut self.chips, chip.name(), true, Chip::name, Chip::from_name);
        if self.chips.contains(&chip) && matches!(chip, Chip::BigUnused | Chip::NeverUsed) {
            self.sort = Sort::Biggest;
        }
    }

    /// Shows only the big unused plugins (the "Show them" link): the Show filter's other picks go, and it stays on
    /// when it's on already.
    pub fn show(&mut self, chip: Chip) {
        self.chips.retain(|c| c.group() != chip.group());
        self.chips.push(chip);
        if matches!(chip, Chip::BigUnused | Chip::NeverUsed) {
            self.sort = Sort::Biggest;
        }
    }

    /// Picks or unpicks a DAW.
    pub fn pick_daw(&mut self, daw: &str) {
        filters::toggle(&mut self.daws, daw, true);
    }

    /// Unpicks every chip in `group`.
    pub fn clear_group(&mut self, group: Group) {
        self.chips.retain(|c| c.group() != group);
    }

    pub fn any(&self) -> bool {
        !self.search.trim().is_empty() || !self.chips.is_empty() || !self.daws.is_empty() || self.years.is_some()
    }

    fn allows(&self, p: &PluginRow, search: &str) -> bool {
        self.allows_but(p, search, None)
    }

    /// Whether `p` gets through every filter but `skip` (a group, or the DAWs for `Some(None)`), for the counts.
    fn allows_but(&self, p: &PluginRow, search: &str, skip: Option<Option<Group>>) -> bool {
        for g in Group::ALL {
            if skip == Some(Some(g)) {
                continue;
            }
            let mut picked = self.chips.iter().filter(|c| c.group() == g).peekable();
            if picked.peek().is_some() && !picked.any(|c| c.allows(p)) {
                return false;
            }
        }
        if skip != Some(None) && !self.daws.is_empty() && !p.daws.iter().any(|d| self.daws.iter().any(|w| w == d)) {
            return false;
        }
        if let Some((a, b)) = self.years
            && !p.years.iter().any(|y| (a..=b).contains(y))
        {
            return false;
        }
        search.is_empty() || case::to_lower(&p.name).contains(search) || case::to_lower(&p.vendor).contains(search)
    }
}

/// An "Install these" card: a missing plugin and how many songs it would help open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Needed {
    /// The row in [`Plugins::rows`].
    pub row: usize,
    /// The big number.
    pub count: usize,
    /// "songs it opens" or "songs need it".
    pub label: &'static str,
    /// "also in 14 older · VST2 · Xfer Records · last used 2019".
    pub details: String,
    /// "You have Ozone 11, which may open them", when another version is installed.
    pub other_version: String,
}

/// The page: every row, and what the filters show.
#[derive(Clone, Debug, Default)]
pub struct Plugins {
    pub rows: Vec<PluginRow>,
    /// Every measured plugin's room, as the old app last measured it (a row's `room` is one of these), less what Clean
    /// up has sent to the Recycle Bin since.
    spaces: Vec<PluginSpace>,
    /// When each plugin's room was measured, by name key.
    measured_at: HashMap<String, NaiveDateTime>,
    /// Places in `rows` the filters let through, in the sort's order.
    pub shown: Vec<usize>,
    pub filters: Filters,
    saves: HashMap<i64, SaveBrief>,
    song_of: HashMap<i64, usize>,
    songs: Vec<SongBrief>,
    /// Each song's newest save's missing plugins (name keys), for songs missing any.
    missing_now: Vec<HashSet<String>>,
    installed_names: Vec<String>,
    pub scans: ScanCounts,
    looked_for_installed: bool,
    pub unused_years: u32,
    pub needed_count: usize,
    /// When the sizes were measured (the oldest measure), if ever.
    pub measured: Option<NaiveDateTime>,
    now: NaiveDateTime,
}

impl Plugins {
    /// Builds the rows from what was read, as of `now` (for "unused for 2 years").
    pub fn new(read: Read, now: NaiveDateTime) -> Plugins {
        let Read {
            uses,
            saves,
            song_of,
            songs,
            spaces,
            installed_names,
            scans,
            looked_for_installed,
            unused_years,
            needed_count,
        } = read;
        let saves: HashMap<i64, SaveBrief> = saves.into_iter().map(|s| (s.id, s)).collect();
        let newest: HashSet<i64> = songs.iter().map(|s| s.latest).collect();

        // Each song's newest save's missing plugins.
        let mut missing_by_save: HashMap<i64, HashSet<String>> = HashMap::new();
        for u in uses.iter().filter(|u| !u.installed && newest.contains(&u.save)) {
            missing_by_save.entry(u.save).or_default().insert(u.key.clone());
        }

        // Grouped by name key and, for a DAW's own device, the DAW, in the order first met.
        let mut order: Vec<(String, String)> = Vec::new();
        let mut groups: HashMap<(String, String), Vec<&PluginUse>> = HashMap::new();
        for u in &uses {
            let own = if u.format == "Native" { u.daw.clone() } else { String::new() };
            groups
                .entry((u.key.clone(), own))
                .or_insert_with_key(|k| {
                    order.push(k.clone());
                    Vec::new()
                })
                .push(u);
        }
        let mut rows: Vec<PluginRow> = order
            .into_iter()
            .filter_map(|k| groups.remove(&k).map(|g| row(k, &g, &song_of, &saves, &newest, &missing_by_save)))
            .collect();
        // The most used first: by songs (the C# page counted saves, so a song saved 40 times came first), then saves,
        // then by name.
        rows.sort_by(|a, b| (b.songs, b.saves.len()).cmp(&(a.songs, a.saves.len())).then_with(|| a.name.cmp(&b.name)));

        let measured = spaces.iter().filter_map(|s| s.measured).min();
        let cutoff = unused_since(now, unused_years);
        with_room(&mut rows, &spaces, cutoff);
        let measured_at = spaces.iter().filter_map(|s| Some((s.key.clone(), s.measured?))).collect();
        let spaces: Vec<PluginSpace> = spaces.iter().map(space_of).collect();

        let mut p = Plugins {
            rows,
            spaces,
            measured_at,
            shown: Vec::new(),
            filters: Filters::default(),
            saves,
            song_of,
            songs,
            missing_now: missing_by_save.into_values().collect(),
            installed_names,
            scans,
            looked_for_installed,
            unused_years,
            needed_count,
            measured,
            now,
        };
        p.refilter();
        p
    }

    /// Works out the shown rows again after a filter changed.
    pub fn refilter(&mut self) {
        let search = case::to_lower(self.filters.search.trim());
        let mut shown: Vec<usize> =
            (0..self.rows.len()).filter(|&i| self.filters.allows(&self.rows[i], &search)).collect();
        let rows = &self.rows;
        match self.filters.sort {
            // `rows` is already in this order.
            Sort::MostUsed => {}
            Sort::Name => shown.sort_by_cached_key(|&i| case::to_lower(&rows[i].name)),
            Sort::Biggest => shown.sort_by(|&a, &b| rows[b].bytes().cmp(&rows[a].bytes())),
            Sort::LastUsed => shown.sort_by(|&a, &b| rows[b].last_used.cmp(&rows[a].last_used)),
        }
        self.shown = shown;
    }

    /// A chip filter as the page shows it: each chip with how many plugins it would show with the other filters as
    /// they are.
    pub fn filter(&self, group: Group) -> Filter {
        let search = case::to_lower(self.filters.search.trim());
        let others: Vec<&PluginRow> =
            self.rows.iter().filter(|r| self.filters.allows_but(r, &search, Some(Some(group)))).collect();
        let choices: Vec<(String, String)> = group
            .chips()
            .iter()
            .map(|c| (c.name().to_owned(), others.iter().filter(|r| c.allows(r)).count().to_string()))
            .collect();
        let picked: Vec<String> =
            self.filters.chips.iter().filter(|c| c.group() == group).map(|c| c.name().to_owned()).collect();
        filters::show(&choices, &picked)
    }

    /// The DAW filter: every DAW a song using a plugin comes from, with its count. A picked one no longer in the list
    /// stays, so it can be unpicked.
    pub fn daw_filter(&self) -> Filter {
        let search = case::to_lower(self.filters.search.trim());
        let others: Vec<&PluginRow> =
            self.rows.iter().filter(|r| self.filters.allows_but(r, &search, Some(None))).collect();
        let mut daws: Vec<String> = self.rows.iter().flat_map(|r| r.daws.iter().cloned()).collect();
        daws.extend(self.filters.daws.iter().cloned());
        daws.sort();
        daws.dedup();
        let choices: Vec<(String, String)> = daws
            .into_iter()
            .map(|d| {
                let n = others.iter().filter(|r| r.daws.contains(&d)).count();
                (d, n.to_string())
            })
            .collect();
        filters::show(&choices, &self.filters.daws)
    }

    /// Before this, no song using a plugin makes it unused.
    fn cutoff(&self) -> NaiveDateTime {
        unused_since(self.now, self.unused_years)
    }

    /// Changes how many years make a plugin unused, from what was read (nothing is read again).
    pub fn set_unused_years(&mut self, years: u32) {
        self.unused_years = years;
        let cutoff = self.cutoff();
        for r in &mut self.rows {
            r.unused = unused(r, cutoff);
        }
        self.refilter();
    }

    /// "412 plugins used, 23 not installed on this computer, 57 installed that no song uses".
    pub fn summary(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        let missing = self.rows.iter().filter(|r| !r.installed).count();
        let never = self.rows.iter().filter(|r| r.never_used()).count();
        let mut s = plural(self.rows.len() - never, "plugin used", "plugins used");
        if missing > 0 {
            s.push_str(&format!(", {missing} not installed on this computer"));
        }
        if never > 0 {
            s.push_str(&format!(", {never} installed that no song uses"));
        }
        s
    }

    /// "Showing 23 of 412", when the filters hide some.
    pub fn count_text(&self) -> String {
        if self.shown.len() == self.rows.len() {
            String::new()
        } else {
            format!("Showing {} of {}", self.shown.len(), self.rows.len())
        }
    }

    /// The "Install these" cards: missing plugins, the ones that are the last thing missing from the most songs first
    /// (they open those songs completely), then the ones the most songs' newest saves use.
    pub fn needed(&self) -> Vec<Needed> {
        let mut missing: Vec<usize> =
            (0..self.rows.len()).filter(|&i| !self.rows[i].installed && self.rows[i].missing_now > 0).collect();
        let rows = &self.rows;
        missing.sort_by(|&a, &b| {
            (rows[b].opens, rows[b].missing_now)
                .cmp(&(rows[a].opens, rows[a].missing_now))
                .then_with(|| case::to_lower(&rows[a].name).cmp(&case::to_lower(&rows[b].name)))
        });
        missing
            .into_iter()
            .take(self.needed_count)
            .map(|i| {
                let r = &rows[i];
                let (count, label) = if r.opens > 0 {
                    (r.opens, if r.opens == 1 { "song it opens" } else { "songs it opens" })
                } else {
                    (r.missing_now, if r.missing_now == 1 { "song needs it" } else { "songs need it" })
                };
                let mut parts = Vec::new();
                if r.opens > 0 && r.missing_now > r.opens {
                    parts.push(format!("needed by {}", r.missing_now));
                }
                parts.extend([r.format_text(), r.vendor.clone()].into_iter().filter(|s| !s.is_empty()));
                if let Some(t) = r.last_used {
                    parts.push(format!("last used {}", t.year()));
                }
                let other = other_version(&r.name, &self.installed_names)
                    .map(|o| format!("You have {o}, which may open them"))
                    .unwrap_or_default();
                Needed { row: i, count, label, details: parts.join(" · "), other_version: other }
            })
            .collect()
    }

    /// "31 of your songs are missing a plugin. Install these 6 and 19 of them open with nothing missing."
    pub fn needed_summary(&self, cards: &[Needed]) -> String {
        let missing = self.missing_now.len();
        if cards.is_empty() || missing == 0 {
            return String::new();
        }
        let keys: HashSet<&str> = cards.iter().map(|c| self.rows[c.row].key.as_str()).collect();
        let opened = self.missing_now.iter().filter(|m| m.iter().all(|k| keys.contains(k.as_str()))).count();
        let first = if missing == 1 {
            "1 of your songs is missing a plugin.".to_owned()
        } else {
            format!("{missing} of your songs are missing a plugin.")
        };
        let these = if cards.len() == 1 { "this one".to_owned() } else { format!("these {}", cards.len()) };
        if opened > 0 {
            format!("{first} Install {these} and {} open with nothing missing.", plural(opened, "song", "songs"))
        } else {
            format!("{first} Each also misses other plugins, so these are the ones the most songs need.")
        }
    }

    /// Every song is missing nothing: said where the cards would be.
    pub fn all_installed(&self) -> bool {
        !self.rows.is_empty() && self.missing_now.is_empty() && self.looked_for_installed
    }

    /// Some song's newest save misses a plugin here.
    pub fn any_missing(&self) -> bool {
        !self.missing_now.is_empty()
    }

    /// The old app measured some plugin's room.
    pub fn has_sizes(&self) -> bool {
        self.rows.iter().any(|r| r.room.is_some())
    }

    /// "You could free 71.4 GB from 12 plugins none of your songs have used in over 2 years." Empty when there's
    /// nothing to free.
    pub fn freeable(&self) -> String {
        let unused: Vec<&PluginRow> = self.rows.iter().filter(|r| r.unused && r.bytes() > 0).collect();
        if unused.is_empty() {
            return String::new();
        }
        let total: u64 = unused.iter().map(|r| r.bytes()).sum();
        let years = if self.unused_years == 1 { "a year".to_owned() } else { format!("{} years", self.unused_years) };
        format!(
            "{} in {} none of your songs have used in over {years}.",
            size_text(total),
            plural(unused.len(), "plugin", "plugins")
        )
    }

    /// When the sizes were measured, or that they haven't been.
    pub fn sizes_note(&self) -> String {
        match self.measured {
            Some(t) => format!("Sizes measured {}.", t.format("%-d %b %Y")),
            None => "Sizes aren't measured yet: the old app measures them in the background after a scan.".to_owned(),
        }
    }

    /// What the plugin scanner has read so far, for the installed copies.
    pub fn scanner_note(&self) -> String {
        let ScanCounts { files, plugins, failed } = self.scans;
        if files == 0 {
            return "The old app's plugin scanner hasn't read your plugin files yet, so IDs and versions aren't known."
                .to_owned();
        }
        let mut s = format!(
            "The plugin scanner has read {} holding {}",
            plural(files as usize, "plugin file", "plugin files"),
            plural(plugins as usize, "plugin", "plugins")
        );
        if failed > 0 {
            s.push_str(&format!("; {failed} couldn't be read."));
        } else {
            s.push('.');
        }
        s
    }

    /// The years on the year strip, newest first, with how many plugins were used in each.
    pub fn year_counts(&self) -> Vec<(i32, usize)> {
        let mut counts: HashMap<i32, usize> = HashMap::new();
        for r in &self.rows {
            for y in &r.years {
                *counts.entry(*y).or_default() += 1;
            }
        }
        let mut out: Vec<(i32, usize)> = counts.into_iter().collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.0));
        out
    }

    /// Each plugin your songs use (a DAW's own devices left out), with how many saves use each format. (The Space
    /// page's spare copies count saves by id instead, with `mo_app::space_spare::formats_used`, so each counts once.)
    pub fn formats_used(&self) -> HashMap<String, HashMap<String, u32>> {
        let mut out: HashMap<String, HashMap<String, u32>> = HashMap::new();
        for r in self.rows.iter().filter(|r| r.format != "Native" && !r.formats.is_empty()) {
            let formats = out.entry(r.name.clone()).or_default();
            for (f, n) in &r.formats {
                *formats.entry(f.clone()).or_default() += n;
            }
        }
        out
    }

    /// The row whose plugin has this name and DAW (for keeping the pick after a reload).
    pub fn find(&self, key: &str, own: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.key == key && r.own == own)
    }

    /// Shows one plugin's row for another page (Space's Show on Plugins page), by its line's name key and `own`, and
    /// gives the row to pick: when the filters hide it they go, and the sort stays. When it isn't in the list (it went,
    /// or the old app has read it differently since that page looked), the list shows the plugins found by its `name`,
    /// so the link always shows something.
    pub fn show_one(&mut self, key: &str, own: &str, name: &str) -> Option<usize> {
        let row = self.find(key, own);
        if row.is_none_or(|r| !self.shown.contains(&r)) {
            let search = if row.is_some() { String::new() } else { name.to_owned() };
            self.filters = Filters { search, sort: self.filters.sort, ..Filters::default() };
            self.refilter();
        }
        row
    }

    /// Shows only the plugins that take room and no song has used lately, for another page (Space's Show them on
    /// Plugins page): "Big and unused" and no other filter, so the list has each one that page counted.
    pub fn show_only_unused(&mut self) {
        self.filters = Filters::default();
        self.filters.show(Chip::BigUnused);
        self.refilter();
    }

    /// The picked plugin's card.
    pub fn card(&self, row: usize) -> Option<Card> {
        let r = self.rows.get(row)?;
        let mut facts = vec![r.format_text()];
        facts.push(
            match r.kind {
                PluginKind::Instrument => "Instrument",
                PluginKind::Effect => "Effect",
                PluginKind::NoteEffect => "Note effect",
                PluginKind::Unknown => "",
            }
            .to_owned(),
        );
        facts.push(r.vendor.clone());
        if let Some(t) = r.last_used {
            facts.push(format!("last used {}", t.format("%B %Y")));
        }
        let facts = facts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let state = if !r.installed {
            State::Missing
        } else if r.unused {
            State::Unused
        } else {
            State::Installed
        };
        let other = if r.installed { None } else { other_version(&r.name, &self.installed_names) };
        Some(Card {
            name: r.name.clone(),
            facts,
            state,
            other_version: other.map(|o| format!("You have {o}, which may open these songs.")).unwrap_or_default(),
            room: self.room_lines(r),
        })
    }

    /// The picked plugin's songs, newest first: each song once, with its newest save that uses it, or every save
    /// with `every`. Only `daw`'s, unless that's empty.
    pub fn songs_using(&self, row: usize, every: bool, daw: &str) -> SongsUsing {
        let Some(r) = self.rows.get(row) else { return SongsUsing::default() };
        let saves: Vec<&SaveBrief> =
            r.saves.iter().filter_map(|id| self.saves.get(id)).filter(|s| daw.is_empty() || s.daw == daw).collect();
        let mut daws: Vec<String> = r.saves.iter().filter_map(|id| self.saves.get(id)).map(|s| s.daw.clone()).collect();
        daws.sort();
        daws.dedup();
        // `saves` is newest first, so a song's first is its newest that uses it.
        let mut at: HashMap<usize, usize> = HashMap::new();
        let mut songs: Vec<UseLine> = Vec::new();
        for s in &saves {
            let song = self.song_of.get(&s.id).copied();
            match song.and_then(|g| at.get(&g)) {
                Some(&i) => songs[i].saves += 1,
                None => {
                    if let Some(g) = song {
                        at.insert(g, songs.len());
                    }
                    let name = song.and_then(|g| self.songs.get(g)).map_or_else(|| s.name.clone(), |g| g.name.clone());
                    songs.push(UseLine::of(s, name));
                }
            }
        }
        let song_count = songs.len();
        let lines = if every { saves.iter().map(|s| UseLine::of(s, s.name.clone())).collect() } else { songs };
        let place = if daw.is_empty() { String::new() } else { format!(" in {daw}") };
        let title = format!("Used in {}{place}", plural(song_count, "song", "songs"));
        let note = if saves.len() == song_count {
            String::new()
        } else if every {
            format!("Every one of its {} saves, newest first.", saves.len())
        } else {
            format!("{} saves in all. Each song shows its newest save that uses it.", saves.len())
        };
        SongsUsing { title, note, lines, daws: if daws.len() > 1 { daws } else { Vec::new() } }
    }

    /// The saves using the picked plugin (only `daw`'s, unless that's empty), for Show in Songs.
    pub fn saves_using(&self, row: usize, daw: &str) -> HashSet<i64> {
        let Some(r) = self.rows.get(row) else { return HashSet::new() };
        r.saves
            .iter()
            .filter(|id| daw.is_empty() || self.saves.get(id).is_some_and(|s| s.daw == daw))
            .copied()
            .collect()
    }

    fn room_lines(&self, r: &PluginRow) -> RoomLines {
        let Some(room) = &r.room else {
            return RoomLines {
                summary: if !r.installed {
                    "Not installed, so it takes no room.".to_owned()
                } else if self.measured.is_none() {
                    "Not measured yet.".to_owned()
                } else {
                    "Not measured: the old app found no files of its own for it.".to_owned()
                },
                ..RoomLines::default()
            };
        };
        let last = match (r.last_used, r.saves.first().and_then(|id| self.saves.get(id))) {
            (Some(t), Some(s)) => format!("last used {} in {}", t.format("%b %Y"), s.name),
            (Some(t), None) => format!("last used {}", t.format("%b %Y")),
            _ => "no song uses it".to_owned(),
        };
        let total = room.bytes().max(1) as f32;
        let parts = [
            ("Plugin files", room.program(), 0),
            ("Presets and settings", room.presets(), 1),
            ("Sounds", room.sounds(), 2),
        ]
        .into_iter()
        .filter(|p| p.1 > 0)
        .map(|(name, bytes, kind)| RoomPart { name, size: size_text(bytes), share: bytes as f32 / total, kind })
        .collect();
        let mut folders: Vec<&SpaceFolder> = room.folders.iter().collect();
        folders.sort_by_key(|a| std::cmp::Reverse(a.bytes));
        let names = |keys: &[String]| -> String {
            keys.iter()
                .map(|k| {
                    self.rows
                        .iter()
                        .find(|r| r.room.as_ref().is_some_and(|m| m.key == *k))
                        .map_or(k.as_str(), |r| r.name.as_str())
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        RoomLines {
            summary: format!("{} on disk · {last}", size_text(room.bytes())),
            shared: if room.shared_bytes() > 0 {
                format!("{} more in folders it shares with other plugins", size_text(room.shared_bytes()))
            } else {
                String::new()
            },
            parts,
            folders: folders
                .into_iter()
                .map(|f| FolderLine {
                    kind: match f.kind {
                        SpaceKind::Program => "Plugin file",
                        SpaceKind::Presets => "Presets and settings",
                        SpaceKind::Sounds => "Sounds",
                    },
                    path: f.path.clone(),
                    size: size_text(f.bytes),
                    shared: if f.shared_with.is_empty() {
                        String::new()
                    } else {
                        format!("Shared with {}", names(&f.shared_with))
                    },
                    added: f.added,
                    can_say: f.kind != SpaceKind::Program,
                })
                .collect(),
        }
    }
}

/// Whether a plugin is on this computer, missing, or installed but unused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Installed,
    Missing,
    Unused,
}

impl State {
    pub fn text(self) -> &'static str {
        match self {
            State::Installed => "Installed",
            State::Missing => "Missing",
            State::Unused => "Unused",
        }
    }
}

/// The side card's top and its room on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub name: String,
    /// "VST3 · Instrument · Xfer Records · last used March 2024".
    pub facts: String,
    pub state: State,
    pub other_version: String,
    pub room: RoomLines,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomLines {
    /// "38.2 GB on disk · last used Mar 2022 in garage sept", or why there's no size.
    pub summary: String,
    pub shared: String,
    /// The bar's parts.
    pub parts: Vec<RoomPart>,
    /// Biggest first.
    pub folders: Vec<FolderLine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoomPart {
    pub name: &'static str,
    pub size: String,
    /// Of its own room, 0 to 1.
    pub share: f32,
    /// 0 plugin files, 1 presets, 2 sounds (for the colour).
    pub kind: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderLine {
    pub kind: &'static str,
    pub path: String,
    pub size: String,
    /// "Shared with Kontakt 7", when it's another plugin's folder too.
    pub shared: String,
    pub added: bool,
    /// Not its plugin file: "Not its" (or "Take off", when you added it) can leave it out.
    pub can_say: bool,
}

/// The side card's songs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongsUsing {
    /// "Used in 12 songs".
    pub title: String,
    pub note: String,
    pub lines: Vec<UseLine>,
    /// The DAWs to pick from, when it's used in more than one.
    pub daws: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseLine {
    pub name: String,
    pub daw: String,
    pub date: String,
    /// "21 saves", or "" for one.
    pub saves: usize,
    /// The save, for opening it on the Songs page.
    pub save: i64,
    pub path: String,
}

impl UseLine {
    fn of(s: &SaveBrief, name: String) -> UseLine {
        UseLine {
            name,
            daw: s.daw.clone(),
            date: s.modified.format("%-d %b %Y").to_string(),
            saves: 1,
            save: s.id,
            path: s.path.clone(),
        }
    }

    pub fn saves_text(&self) -> String {
        if self.saves > 1 { format!("{} saves", self.saves) } else { String::new() }
    }
}

/// One installed copy, as the side card shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyLine {
    pub format: String,
    pub name: String,
    /// "Instrument · FabFilter · version 3.24 · in WaveShell-VST3 14.0".
    pub details: String,
    /// "ID 5653544650513370726F2D7120330000" (what projects save it under).
    pub id: String,
    pub path: String,
    /// "312 parameters", or why the scanner couldn't load it.
    pub note: String,
}

/// The installed copies the scanner read, as card lines.
pub fn copy_lines(copies: &[ScannedPlugin]) -> Vec<CopyLine> {
    copies
        .iter()
        .map(|c| {
            let mut kind = c.kind.replace('-', " ");
            if let Some(first) = kind.get(..1) {
                kind = first.to_uppercase() + kind.get(1..).unwrap_or("");
            }
            let details = [
                kind,
                c.vendor.clone(),
                if c.version.is_empty() { String::new() } else { format!("version {}", c.version) },
                if c.shell.is_empty() {
                    String::new()
                } else {
                    format!("in {}", mo_formats::paths::file_name(&c.shell))
                },
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
            let note = match c.parameters {
                Some(0) => "It has no parameters.".to_owned(),
                Some(n) => plural(n, "parameter", "parameters"),
                None if !c.error.is_empty() => format!("The scanner couldn't load it: {}", c.error),
                None => String::new(),
            };
            CopyLine {
                format: c.format.clone(),
                name: c.name.clone(),
                details,
                id: if c.id.is_empty() { String::new() } else { format!("ID {}", c.id) },
                path: c.path.clone(),
                note,
            }
        })
        .collect()
}

/// What the card says when the scanner read no copies of the plugin.
pub fn no_copies_note(p: &Plugins, row: usize) -> String {
    let installed = p.rows.get(row).is_some_and(|r| r.installed);
    if p.scans.files == 0 {
        "The plugin scanner hasn't read your plugins yet.".to_owned()
    } else if installed {
        "Installed, but the plugin scanner couldn't read it, or it's in a format it doesn't read yet (AU, AAX)."
            .to_owned()
    } else {
        "Not found in your plugin folders.".to_owned()
    }
}

/// The web search for a plugin (Search online): its maker, name and "plugin download".
pub fn search_url(name: &str, vendor: &str) -> String {
    let q = format!("{vendor} {name} plugin download");
    let mut out = String::from("https://www.google.com/search?q=");
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn row(
    (key, own): (String, String),
    g: &[&PluginUse],
    song_of: &HashMap<i64, usize>,
    saves: &HashMap<i64, SaveBrief>,
    newest: &HashSet<i64>,
    missing_by_save: &HashMap<i64, HashSet<String>>,
) -> PluginRow {
    // As the C# query: the smallest name and largest vendor and format (by their bytes), installed only when every
    // save says so, and the kind most of its saves have it as (the lowest of a tie).
    let name = g.iter().map(|u| u.name.as_str()).min().unwrap_or("").to_owned();
    let vendor = g.iter().map(|u| u.vendor.as_str()).max().unwrap_or("").to_owned();
    let format = g.iter().map(|u| u.format.as_str()).max().unwrap_or("").to_owned();
    let installed = g.iter().all(|u| u.installed);
    let mut kinds: HashMap<u8, usize> = HashMap::new();
    for u in g {
        *kinds.entry(kind_number(u.kind)).or_default() += 1;
    }
    let kind = kinds
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map_or(PluginKind::Unknown, |(k, _)| kind_of(k));
    let mut daws: Vec<String> = g.iter().map(|u| u.daw.clone()).collect();
    daws.sort();
    daws.dedup();

    let mut ids: Vec<i64> = g.iter().map(|u| u.save).collect();
    ids.sort_unstable();
    ids.dedup();
    let when = |id: &i64| saves.get(id).map(|s| s.modified);
    ids.sort_by(|a, b| when(b).cmp(&when(a)).then_with(|| b.cmp(a)));
    let last_used = ids.first().and_then(when);
    let mut years: Vec<i32> = ids.iter().filter_map(when).map(|t| t.year()).collect();
    years.dedup();
    years.sort_by(|a, b| b.cmp(a));
    years.dedup();

    let mut by_format: HashMap<&str, HashSet<i64>> = HashMap::new();
    for u in g {
        by_format.entry(u.format.as_str()).or_default().insert(u.save);
    }
    let mut formats: Vec<(String, u32)> =
        by_format.into_iter().map(|(f, s)| (f.to_owned(), u32::try_from(s.len()).unwrap_or(u32::MAX))).collect();
    formats.sort();

    let songs: HashSet<usize> = ids.iter().filter_map(|id| song_of.get(id).copied()).collect();
    let now: Vec<i64> = ids.iter().filter(|id| newest.contains(id)).copied().collect();
    let missing_now = now.iter().filter(|id| missing_by_save.get(id).is_some_and(|m| m.contains(&key))).count();
    let opens = if installed {
        0
    } else {
        now.iter().filter(|id| missing_by_save.get(id).is_some_and(|m| m.len() == 1 && m.contains(&key))).count()
    };
    PluginRow {
        name,
        vendor,
        format,
        kind,
        key,
        own,
        daws,
        installed,
        songs: songs.len(),
        songs_now: now.len(),
        missing_now,
        opens,
        saves: ids,
        formats,
        last_used,
        years,
        room: None,
        unused: false,
    }
}

fn kind_number(k: PluginKind) -> u8 {
    match k {
        PluginKind::Unknown => 0,
        PluginKind::Instrument => 1,
        PluginKind::Effect => 2,
        PluginKind::NoteEffect => 3,
    }
}

fn kind_of(n: u8) -> PluginKind {
    match n {
        1 => PluginKind::Instrument,
        2 => PluginKind::Effect,
        3 => PluginKind::NoteEffect,
        _ => PluginKind::Unknown,
    }
}

fn unused(r: &PluginRow, cutoff: NaiveDateTime) -> bool {
    r.room.is_some() && r.installed && !r.last_used.is_some_and(|t| t >= cutoff)
}

/// Gives each row its measured room, and adds the installed plugins no song uses (they aren't in the songs' list at
/// all), as the C# page's `WithSpace`. A project may name a plugin differently from its file ("Pro-Q 3" and
/// "FabFilter Pro-Q 3.vst3"), so an installed plugin counts as used when its name holds a used one's or the other way
/// round: a plugin in doubt is never offered for removal.
fn with_room(rows: &mut Vec<PluginRow>, spaces: &[StoredSpace], cutoff: NaiveDateTime) {
    let families: Vec<(usize, String)> =
        rows.iter().enumerate().filter(|(_, r)| r.format != "Native").map(|(i, r)| (i, family(&r.name))).collect();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for (i, _) in &families {
        by_key.entry(rows[*i].key.clone()).or_insert(*i);
    }
    let mut claimed: HashSet<String> = HashSet::new();
    let give = |rows: &mut Vec<PluginRow>, claimed: &mut HashSet<String>, at: usize, s: &StoredSpace| {
        rows[at].room = Some(space_of(s));
        rows[at].unused = unused(&rows[at], cutoff);
        claimed.insert(rows[at].key.clone());
    };
    // Same names first ("Serum 2" is Serum2's), then close ones for what's left. Only an installed row gets a size: a
    // plugin the library says is missing takes no room here, even when the old measure found a folder named like it
    // (that measure is still "used", so it's never offered for removal either).
    let mut rest: Vec<&StoredSpace> = Vec::new();
    for s in spaces {
        match by_key.get(&s.key) {
            Some(&at) if !rows[at].installed => {}
            Some(&at) if !claimed.contains(&s.key) => give(rows, &mut claimed, at, s),
            _ => rest.push(s),
        }
    }
    // Every Waves plugin lives in one WaveShell file, and projects name them one by one ("CLA-76 Stereo"), so the
    // shell is used whenever a song uses a Waves plugin.
    let waves_used = rows.iter().any(|r| is_waves(&r.name, "") || case::to_lower(&r.vendor).starts_with("waves"));
    for s in rest {
        let fam = family(&s.name);
        let close: Vec<usize> = families.iter().filter(|(_, f)| matched(f, &fam)).map(|(i, _)| *i).collect();
        if close.is_empty() && waves_used && is_shell(s) {
            continue;
        }
        if close.is_empty() {
            let format = s
                .folders
                .iter()
                .filter(|f| f.kind == 0)
                .map(|f| format_from_path(&f.path))
                .find(|f| !f.is_empty())
                .unwrap_or("");
            let mut r = PluginRow {
                name: s.name.clone(),
                vendor: s.vendor.clone(),
                format: format.to_owned(),
                kind: PluginKind::Unknown,
                key: s.key.clone(),
                own: String::new(),
                daws: Vec::new(),
                installed: true,
                saves: Vec::new(),
                formats: Vec::new(),
                songs: 0,
                songs_now: 0,
                missing_now: 0,
                opens: 0,
                last_used: None,
                years: Vec::new(),
                room: Some(space_of(s)),
                unused: false,
            };
            r.unused = unused(&r, cutoff);
            rows.push(r);
        } else if let Some(&at) = close.iter().find(|&&i| rows[i].installed && !claimed.contains(&rows[i].key)) {
            // A row that already has its own size keeps it; this one counts as used either way, so it's never offered
            // (as the C# page: a second Kontakt measured beside a sized Kontakt row shows no size of its own).
            give(rows, &mut claimed, at, s);
        }
    }
}

fn matched(a: &str, b: &str) -> bool {
    a.chars().count() >= 4 && b.chars().count() >= 4 && (a == b || a.contains(b) || b.contains(a))
}

/// A Waves shell: the one file every Waves plugin lives in.
fn is_shell(s: &StoredSpace) -> bool {
    is_waves(&s.name, "") || s.folders.iter().any(|f| f.kind == 0 && is_waves(&s.name, &f.path))
}

/// Another installed version of the plugin ("Ozone 11" for "Ozone 9"), as the C# `OtherVersionInstalled`.
fn other_version(name: &str, installed: &[String]) -> Option<String> {
    let fam = family(name);
    if fam.chars().count() < 3 {
        return None;
    }
    installed
        .iter()
        .filter(|i| !case::eq_ignore_case(i, name) && family(i) == fam)
        .max_by_key(|i| case::to_lower(i))
        .cloned()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

pub mod clean;
pub mod folder_fixes;
pub mod moving;
pub mod uninstall;

#[cfg(test)]
mod tests;
