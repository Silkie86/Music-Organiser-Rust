//! The Space page's plugins you haven't used: the last of its biggest wins (the page's question 3: "no project has
//! used it for 2 years, like the old app", or as many years as the Plugins page is set to). Read-only here, as the
//! question round agreed (question 2): Clean up and Uninstall are on each plugin's card on the Plugins page, which each
//! plugin's line here opens. The list, the sizes and the rule are the Plugins page's own ([`crate::plugins`]), so the
//! two pages say the same: an installed plugin the old app has measured that no song has used in those years, or
//! ever. Its room is its own: a folder it shares with another plugin (a maker's folder) isn't counted, as the Plugins
//! page doesn't count it.

use chrono::NaiveDateTime;
use mo_library::scan::Inside;
use mo_space::plugin_space::PluginSpace;
use mo_space::size::size_text;

use crate::drag_out::count_of;
use crate::plugins::folder_fixes::FolderFix;
use crate::plugins::{PluginRow, Plugins, Read, unused_since};
use crate::space::{Card, Line, OnPlugins, Part};

/// The line's id, and its card's.
pub const UNUSED_PLUGINS: &str = "unused-plugins";

/// What the line says while the first look reads your plugins.
pub const LOOKING: &str = "Reading your plugins…";

/// How many plugins each part of the card names; the rest add up in one line, which shows them all on the Plugins
/// page.
pub const LISTED: usize = 30;

const KIND: &str = "PLUGINS NOT USED LATELY";
const TITLE: &str = "Plugins you haven't used";

/// What the page says while the old app hasn't measured any plugin's size.
const NOT_MEASURED: &str =
    "Plugins' sizes aren't measured yet: the old app measures them in the background after a scan.";

/// What the card says of where a plugin's Clean up and Uninstall are, and what each takes (Solomon's rules of 25 Oct
/// 2026, as `mo_space::plugin_clean` checks them).
const ON_ITS_CARD: &str = "Each plugin's card on the Plugins page has Clean up, which takes its presets, and its sounds \
    when no song has ever used it (the plugin stays installed), and Uninstall, which takes the plugin itself.";

/// One installed plugin the old app measured, with its room and when a song last used it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measured {
    pub name: String,
    pub vendor: String,
    /// Its line's name key and own (the DAW of a DAW's own device): what the Plugins page finds it by.
    pub key: String,
    pub own: String,
    /// When a song last used it, and how many songs ever did (none: it's never been used).
    pub last_used: Option<NaiveDateTime>,
    pub songs: usize,
    /// Its files and folders, as the Plugins page has them: what it counts is its own room, without a folder it shares
    /// with another plugin (a maker's folder).
    pub room: PluginSpace,
    /// Another version a missing plugin's songs may open with (`PluginRow::stands_in`): never unused, whatever the
    /// years (review E2-6).
    pub stands_in: bool,
}

impl Measured {
    /// Its own plugin files, presets and settings, and sounds.
    pub fn program(&self) -> u64 {
        self.room.program()
    }
    pub fn presets(&self) -> u64 {
        self.room.presets()
    }
    pub fn sounds(&self) -> u64 {
        self.room.sounds()
    }

    /// Its own room.
    pub fn bytes(&self) -> u64 {
        self.room.bytes()
    }

    /// No song has ever used it.
    pub fn never_used(&self) -> bool {
        self.songs == 0
    }
}

/// What a look found: every plugin no song has used lately, biggest first, how many years make one unused, and
/// whether and when the old app measured plugin sizes (without them, none can be counted).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub plugins: Vec<Measured>,
    /// The other installed plugins the old app measured that take room of their own: a song used each in those
    /// years. Kept so the years can change without reading again ([`Found::as_of`]).
    pub used: Vec<Measured>,
    pub years: u32,
    /// When the look was: the years count back from it.
    pub now: NaiveDateTime,
    pub sizes: bool,
    /// When the sizes were measured (the oldest measure).
    pub measured: Option<NaiveDateTime>,
    /// How many of those no song has used lately the app has taken off since the old app measured them (cleaned up or
    /// uninstalled to nothing): [`Found::without`] counts them.
    pub gone: usize,
}

impl Found {
    /// What they take in all.
    pub fn bytes(&self) -> u64 {
        self.plugins.iter().fold(0u64, |sum, p| sum.saturating_add(p.bytes()))
    }

    /// As it is when `years` make a plugin unused, counted back from `now`: the years are the Plugins page's pick,
    /// which can change while the app runs, and `now` moves on when a look finds the library as it was. Nothing is read
    /// again, but every plugin is copied and sorted: call it away from the window's thread. The Plugins page counts
    /// back the same way ([`unused_since`]).
    pub fn as_of(&self, years: u32, now: NaiveDateTime) -> Found {
        let since = unused_since(now, years);
        let (mut plugins, mut used): (Vec<Measured>, Vec<Measured>) = self
            .plugins
            .iter()
            .chain(&self.used)
            .cloned()
            .partition(|p| !p.stands_in && !p.last_used.is_some_and(|t| t >= since));
        biggest_first(&mut plugins);
        biggest_first(&mut used);
        Found { plugins, used, years, now, sizes: self.sizes, measured: self.measured, gone: self.gone }
    }

    /// As it is now the app has removed what's at `gone` (sent to the Recycle Bin, or uninstalled) since the old app
    /// measured: what's in there isn't counted, and a plugin with nothing left goes. Each folder is looked up once,
    /// however much is gone ([`Inside`]), but it goes through every plugin's folders: call it away from the window's
    /// thread. The Plugins page leaves them out the same way (`Plugins::forget_paths`).
    pub fn without(mut self, gone: &[String]) -> Found {
        let gone = Inside::new(gone);
        if gone.is_empty() {
            return self;
        }
        let left = |plugins: &mut Vec<Measured>| {
            for p in plugins.iter_mut() {
                p.room.folders.retain(|f| !gone.holds(&f.path));
            }
            plugins.retain(|p| p.bytes() > 0);
            biggest_first(plugins);
        };
        let before = self.plugins.len();
        left(&mut self.plugins);
        left(&mut self.used);
        self.gone += before - self.plugins.len();
        self
    }
}

/// Works out the plugins no song has used lately from what the Plugins page reads (`read`, from the library), as of
/// `now`, biggest first, with the Plugins page's `fixes` to plugins' folders this session
/// ([`Plugins::apply_folder_fixes`]). Builds the whole plugin list: call it away from the window's thread.
pub fn look(read: Read, now: NaiveDateTime, fixes: &[FolderFix]) -> Found {
    let mut plugins = Plugins::new(read, now);
    // Your say on a plugin's folders on the Plugins page (Not its, Take off, Add a folder) counts here too, as there.
    plugins.apply_folder_fixes(fixes);
    plugins.forget_gone();
    let (mut unused, mut used) = (Vec::new(), Vec::new());
    // What "Big and unused" on the Plugins page picks from: installed, measured, with room of its own.
    for r in plugins.rows.iter().filter(|r| r.installed && r.bytes() > 0) {
        let Some(m) = measured_of(r) else { continue };
        if r.unused { unused.push(m) } else { used.push(m) }
    }
    biggest_first(&mut unused);
    biggest_first(&mut used);
    Found {
        plugins: unused,
        used,
        years: plugins.unused_years,
        now,
        sizes: plugins.has_sizes(),
        measured: plugins.measured,
        gone: 0,
    }
}

fn biggest_first(plugins: &mut [Measured]) {
    plugins.sort_by(|a, b| b.bytes().cmp(&a.bytes()).then_with(|| a.name.cmp(&b.name)));
}

fn measured_of(r: &PluginRow) -> Option<Measured> {
    Some(Measured {
        name: r.name.clone(),
        vendor: r.vendor.clone(),
        key: r.key.clone(),
        own: r.own.clone(),
        last_used: r.last_used,
        songs: r.songs,
        room: r.room.clone()?,
        stands_in: r.stands_in.is_some(),
    })
}

/// "in over 2 years", "in over a year".
fn in_years(years: u32) -> String {
    if years == 1 { "in over a year".to_owned() } else { format!("in over {years} years") }
}

/// The line under Biggest wins first, with the room the plugins take (what the wins are sorted by). While the first
/// look is on (`looking`, nothing found yet) it says so; none when a look found none, or none has finished and none is
/// on.
pub fn line(found: Option<&Found>, looking: bool) -> Option<(u64, Line)> {
    let line = |why: String, size: String| Line { id: UNUSED_PLUGINS, title: TITLE, why, size };
    match found {
        Some(f) if !f.plugins.is_empty() => {
            let never = f.plugins.iter().filter(|p| p.never_used()).count();
            let mut why = format!(
                "{} your songs haven't used {}",
                count_of(f.plugins.len(), "plugin", "plugins"),
                in_years(f.years)
            );
            match never {
                0 => {}
                n if n == f.plugins.len() => {
                    why = format!("{} your songs have never used", count_of(n, "plugin", "plugins"))
                }
                n => why.push_str(&format!(", {n} of them never")),
            }
            Some((f.bytes(), line(why, size_text(f.bytes()))))
        }
        None if looking => Some((0, line(LOOKING.to_owned(), "Measuring…".to_owned()))),
        _ => None,
    }
}

/// What the page says under Biggest wins first about plugins you haven't used when there's no line: that the old app
/// hasn't measured their sizes yet, that the ones a look found have gone since, or that it found none; or nothing.
pub fn none_note(found: Option<&Found>) -> String {
    match found {
        Some(f) if !f.sizes => format!("{NOT_MEASURED} "),
        Some(f) if f.plugins.is_empty() && f.gone > 0 => {
            "The plugins your songs haven't used have gone since the old app measured them. ".to_owned()
        }
        Some(f) if f.plugins.is_empty() => {
            let years = if f.years == 1 { "year".to_owned() } else { format!("{} years", f.years) };
            format!("Every plugin the old app measured was used in the last {years}. ")
        }
        _ => String::new(),
    }
}

/// The card: what the plugins take, then those no song has ever used and those not used lately (the kind that takes
/// most first), and under each its plugins, biggest first, the first [`LISTED`] by name, each with what it holds and a
/// link that shows it on the Plugins page; the rest add up in one line, whose link shows them all there.
pub fn card(found: Option<&Found>, looking: bool) -> Card {
    let years = found.map_or(crate::plugins::DEFAULT_UNUSED_YEARS, |f| f.years);
    let mut kinds: Vec<(bool, Vec<&Measured>)> = [true, false]
        .into_iter()
        .map(|never| {
            (never, found.map(|f| f.plugins.iter().filter(|p| p.never_used() == never).collect()).unwrap_or_default())
        })
        .filter(|(_, plugins): &(bool, Vec<&Measured>)| !plugins.is_empty())
        .collect();
    let total = |plugins: &[&Measured]| plugins.iter().fold(0u64, |sum, p| sum.saturating_add(p.bytes()));
    kinds.sort_by_key(|(_, plugins)| std::cmp::Reverse(total(plugins)));
    let mut parts = Vec::new();
    for (never, plugins) in &kinds {
        let (name, what) = if *never {
            (
                format!("{} no song has used", count_of(plugins.len(), "plugin", "plugins")),
                "Installed, but in none of your songs".to_owned(),
            )
        } else {
            (
                format!("{} not used lately", count_of(plugins.len(), "plugin", "plugins")),
                format!("No song has used them {}", in_years(years)),
            )
        };
        parts.push(Part { name, what, size: size_text(total(plugins)), ..Part::default() });
        parts.extend(plugins.iter().take(LISTED).map(|p| plugin_part(p)));
        let rest = plugins.get(LISTED..).unwrap_or_default();
        if !rest.is_empty() {
            parts.push(Part {
                name: format!("And {}", count_of(rest.len(), "more plugin", "more plugins")),
                size: size_text(total(rest)),
                under: true,
                plugins: Some(OnPlugins::Unused),
                ..Part::default()
            });
        }
    }
    let bytes = found.map(Found::bytes);
    Card {
        kind: KIND,
        title: TITLE,
        big: match (found, bytes) {
            (None, _) if looking => "Measuring…".to_owned(),
            (None, _) => String::new(),
            (Some(_), Some(0) | None) => "Nothing to free".to_owned(),
            (Some(_), Some(b)) => size_text(b),
        },
        big_note: match found {
            Some(f) if !f.plugins.is_empty() => format!("in {}", count_of(f.plugins.len(), "plugin", "plugins")),
            _ => String::new(),
        },
        lead: lead(found, years),
        ticks: false,
        parts,
        note: found.map(note).unwrap_or_default(),
        room: String::new(),
        warning: String::new(),
    }
}

/// The card's lead: what the plugins are, and where Clean up and Uninstall are.
fn lead(found: Option<&Found>, years: u32) -> String {
    let some_never = found.is_some_and(|f| f.plugins.iter().any(Measured::never_used));
    let never = if some_never { ", and some no song has ever used" } else { "" };
    format!("No song has used these {}{never}. {ON_ITS_CARD}", in_years(years))
}

/// What the card says about the sizes, above the plugins: whose measure they are, and that nothing goes from here.
fn note(found: &Found) -> String {
    let when = found.measured.map(|t| format!(", on {}", t.format("%-d %b %Y"))).unwrap_or_default();
    match (found.sizes, found.plugins.is_empty()) {
        (false, _) => NOT_MEASURED.to_owned(),
        (true, true) => String::new(),
        (true, false) => format!(
            "Sizes are the old app's last measure{when}, without any folder a plugin shares with another. Nothing goes \
             from here."
        ),
    }
}

/// A plugin's line under its kind: its name, its maker, when a song last used it, what it holds by kind, its room,
/// and a link that shows it on the Plugins page.
fn plugin_part(p: &Measured) -> Part {
    let mut said: Vec<String> = Vec::new();
    if !p.vendor.trim().is_empty() {
        said.push(p.vendor.trim().to_owned());
    }
    if let Some(t) = p.last_used.filter(|_| !p.never_used()) {
        said.push(format!("last used {} in {}", t.format("%b %Y"), count_of(p.songs, "song", "songs")));
    }
    let holds: Vec<String> = [("sounds", p.sounds()), ("presets", p.presets()), ("the plugin", p.program())]
        .into_iter()
        .filter(|(_, bytes)| *bytes > 0)
        .map(|(what, bytes)| format!("{what} {}", size_text(bytes)))
        .collect();
    if !holds.is_empty() {
        said.push(holds.join(", "));
    }
    Part {
        name: p.name.clone(),
        what: capitalised(&said.join(" · ")),
        size: size_text(p.bytes()),
        under: true,
        plugins: Some(OnPlugins::Plugin { key: p.key.clone(), own: p.own.clone(), name: p.name.clone() }),
        ..Part::default()
    }
}

/// `text` with its first letter a capital ("sounds 2 GB" becomes "Sounds 2 GB").
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use mo_space::plugin_space::{SpaceFolder, SpaceKind};

    use super::*;

    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    fn at(year: i32, month: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, 1).and_then(|d| d.and_hms_opt(12, 0, 0)).unwrap_or_default()
    }

    fn unused(
        name: &str,
        songs: usize,
        last: Option<NaiveDateTime>,
        (program, presets, sounds): (u64, u64, u64),
    ) -> Measured {
        let folders = [
            (SpaceKind::Program, "Plugin", program),
            (SpaceKind::Presets, "Presets", presets),
            (SpaceKind::Sounds, "Sounds", sounds),
        ]
        .into_iter()
        .filter(|(_, _, bytes)| *bytes > 0)
        .map(|(kind, what, bytes)| SpaceFolder {
            path: format!(r"D:\{name}\{what}"),
            kind,
            bytes,
            shared_with: Vec::new(),
            added: false,
        })
        .collect();
        Measured {
            name: name.to_owned(),
            vendor: "Maker".to_owned(),
            key: name.to_lowercase(),
            own: String::new(),
            last_used: last,
            songs,
            room: PluginSpace { key: name.to_lowercase(), name: name.to_owned(), vendor: "Maker".to_owned(), folders },
            stands_in: false,
        }
    }

    fn found(plugins: Vec<Measured>) -> Found {
        Found {
            plugins,
            used: Vec::new(),
            years: 2,
            now: at(2026, 11),
            sizes: true,
            measured: Some(at(2026, 11)),
            gone: 0,
        }
    }

    #[test]
    fn the_line_says_how_many_and_how_many_never_and_frees_their_own_room() {
        let f = found(vec![
            unused("Omnisphere", 3, Some(at(2022, 3)), (85 * MB, 120 * MB, 38 * GB)),
            unused("Kontakt Factory", 0, None, (0, 0, 21 * GB)),
        ]);
        let (bytes, line) = line(Some(&f), false)
            .unwrap_or_else(|| (0, Line { id: "", title: "", why: String::new(), size: String::new() }));
        assert_eq!(bytes, f.bytes());
        assert_eq!((line.id, line.title), (UNUSED_PLUGINS, TITLE));
        assert_eq!(line.why, "2 plugins your songs haven't used in over 2 years, 1 of them never");
        assert_eq!(line.size, size_text(f.bytes()));
        let never = found(vec![unused("Kontakt Factory", 0, None, (0, 0, 21 * GB))]);
        assert_eq!(line_of(&never), "1 plugin your songs have never used");
        // While the first look reads: it says so. A look that found none: no line, and the note says why.
        assert_eq!(super::line(None, true).map(|(_, l)| l.why), Some(LOOKING.to_owned()));
        assert_eq!(super::line(None, false), None);
        assert_eq!(super::line(Some(&found(Vec::new())), false), None);
        assert_eq!(
            none_note(Some(&found(Vec::new()))),
            "Every plugin the old app measured was used in the last 2 years. "
        );
        let unmeasured = Found { sizes: false, ..found(Vec::new()) };
        assert!(none_note(Some(&unmeasured)).starts_with("Plugins' sizes aren't measured yet"));
        assert_eq!(none_note(None), "");
    }

    #[test]
    fn a_look_finds_what_the_plugins_page_calls_unused_less_what_has_gone() {
        use std::collections::HashMap;

        use mo_library::PluginKind;
        use mo_library::plugins::{PluginUse, ScanCounts, StoredFolder, StoredSpace};

        use crate::plugins::{SaveBrief, SongBrief};

        let save = |id: i64, name: &str, when: NaiveDateTime| SaveBrief {
            id,
            name: name.into(),
            path: format!(r"C:\beats\{name}.flp"),
            daw: "FL Studio".into(),
            modified: when,
        };
        let uses = |save: i64, name: &str| PluginUse {
            save,
            name: name.into(),
            key: mo_library::read::name_key(name),
            vendor: String::new(),
            kind: PluginKind::Instrument,
            format: "VST3".into(),
            installed: true,
            daw: "FL Studio".into(),
        };
        let space = |name: &str, folders: &[(&str, u8, u64)]| StoredSpace {
            key: mo_library::read::name_key(name),
            name: name.into(),
            vendor: "Maker".into(),
            folders: folders
                .iter()
                .map(|(p, kind, bytes)| StoredFolder {
                    path: (*p).into(),
                    kind: *kind,
                    bytes: *bytes,
                    shared_with: Vec::new(),
                    added: false,
                })
                .collect(),
            measured: Some(at(2026, 10)),
        };
        // Serum is used this year; Diva a year ago to the day; Massive X last June; Pro-Q 3 last in 2019; Kontakt 7
        // never.
        let read = Read {
            uses: vec![uses(1, "Serum"), uses(2, "Pro-Q 3"), uses(3, "Massive X"), uses(4, "Diva")],
            saves: vec![
                save(1, "New", at(2026, 9)),
                save(2, "Old", at(2019, 5)),
                save(3, "Mid", at(2025, 6)),
                save(4, "Year", at(2025, 11)),
            ],
            song_of: HashMap::from([(1, 0), (2, 1), (3, 2), (4, 3)]),
            songs: vec![
                SongBrief { name: "New".into(), latest: 1 },
                SongBrief { name: "Old".into(), latest: 2 },
                SongBrief { name: "Mid".into(), latest: 3 },
                SongBrief { name: "Year".into(), latest: 4 },
            ],
            spaces: vec![
                space("Serum", &[(r"C:\VST3\Serum.vst3", 0, 30 * MB)]),
                space("Massive X", &[(r"C:\VST3\Massive X.vst3", 0, 60 * MB)]),
                space("Diva", &[(r"C:\VST3\Diva.vst3", 0, 20 * MB)]),
                space("Pro-Q 3", &[(r"C:\VST3\Pro-Q 3.vst3", 0, 40 * MB), (r"C:\Users\me\Pro-Q", 1, 2 * MB)]),
                space("Kontakt 7", &[(r"C:\VST3\Kontakt 7.vst3", 0, 100 * MB), (r"D:\Libraries", 2, 50 * GB)]),
            ],
            installed_names: vec![
                "Serum".into(),
                "Massive X".into(),
                "Diva".into(),
                "Pro-Q 3".into(),
                "Kontakt 7".into(),
            ],
            scans: ScanCounts::default(),
            looked_for_installed: true,
            unused_years: 2,
            needed_count: 6,
            saved_fixes: Vec::new(),
            gone: Vec::new(),
        };
        let f = look(read.clone(), at(2026, 11), &[]);
        let names: Vec<(&str, usize, u64)> = f.plugins.iter().map(|p| (p.name.as_str(), p.songs, p.bytes())).collect();
        assert_eq!(names, [("Kontakt 7", 0, 100 * MB + 50 * GB), ("Pro-Q 3", 1, 42 * MB)]);
        let kontakt = f.plugins.first().cloned();
        assert_eq!(kontakt.map(|k| (k.program(), k.presets(), k.sounds())), Some((100 * MB, 0, 50 * GB)));
        assert_eq!((f.years, f.sizes, f.measured), (2, true, Some(at(2026, 10))));
        // The ones used in those years are kept, so the years can change on the Plugins page without reading again,
        // and each count agrees with a look made with those years (as the Plugins page's would).
        let used: Vec<&str> = f.used.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(used, ["Massive X", "Serum", "Diva"]);
        // With 1 year, Massive X is unused; Diva, used a year ago to the day, still counts as used.
        let names_in = |years: u32| f.as_of(years, f.now).plugins.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names_in(1), ["Kontakt 7", "Massive X", "Pro-Q 3"]);
        assert_eq!(names_in(10), ["Kontakt 7"]);
        for years in [1, 2, 5, 10] {
            assert_eq!(
                f.as_of(years, f.now),
                look(Read { unused_years: years, ..read.clone() }, at(2026, 11), &[]),
                "{years}"
            );
        }
        // A look that finds the library as it was moves the time on: ten years later the rest are unused too.
        let ten_on = f.as_of(2, at(2036, 11));
        assert_eq!(ten_on, look(read.clone(), at(2036, 11), &[]));
        assert_eq!(ten_on.plugins.len(), 5);
        // Kontakt's library went to the bin since the old app measured it, and Pro-Q 3 was uninstalled: Kontakt's
        // library isn't counted any more, and Pro-Q 3 goes with nothing left (its presets folder was inside).
        let gone = f.clone().without(&[
            r"D:\Libraries".to_owned(),
            r"C:\VST3\Pro-Q 3.vst3".to_owned(),
            r"C:\Users\me".to_owned(),
        ]);
        let names: Vec<(&str, u64)> = gone.plugins.iter().map(|p| (p.name.as_str(), p.bytes())).collect();
        assert_eq!(names, [("Kontakt 7", 100 * MB)]);
        assert_eq!(gone.gone, 1);
        assert_eq!(f.clone().without(&[]), f);
        // Once every one has gone, the note says so, not that they were used.
        let all = gone.without(&[r"C:\VST3\Kontakt 7.vst3".to_owned()]);
        assert!(all.plugins.is_empty() && all.gone == 2);
        assert_eq!(
            none_note(Some(&all)),
            "The plugins your songs haven't used have gone since the old app measured them. "
        );
    }

    fn line_of(f: &Found) -> String {
        line(Some(f), false).map(|(_, l)| l.why).unwrap_or_default()
    }

    #[test]
    fn the_card_lists_each_kind_biggest_first_and_each_plugin_links_to_the_plugins_page() {
        let mut plugins = vec![
            unused("Omnisphere", 3, Some(at(2022, 3)), (85 * MB, 120 * MB, 38 * GB)),
            unused("Kontakt Factory", 0, None, (0, 0, 21 * GB)),
        ];
        // Never used: 31 more, so one adds up in the last line.
        for n in 0..LISTED {
            plugins.push(unused(&format!("Synth {n:02}"), 0, None, (0, 10 * MB, 0)));
        }
        let f = found(plugins);
        let card = card(Some(&f), false);
        assert_eq!((card.kind, card.title, card.ticks), (KIND, TITLE, false));
        assert_eq!(card.big, size_text(f.bytes()));
        assert_eq!(card.big_note, "in 32 plugins");
        // Not used lately takes more, so it comes first.
        let names: Vec<&str> = card.parts.iter().filter(|p| !p.under).map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["1 plugin not used lately", "31 plugins no song has used"]);
        let omni = card.parts.get(1).cloned().unwrap_or_default();
        assert_eq!(omni.name, "Omnisphere");
        assert_eq!(
            omni.what,
            format!(
                "Maker · last used Mar 2022 in 3 songs · sounds {}, presets 120 MB, the plugin 85 MB",
                size_text(38 * GB)
            )
        );
        let on = OnPlugins::Plugin { key: "omnisphere".to_owned(), own: String::new(), name: "Omnisphere".to_owned() };
        assert_eq!(omni.plugins, Some(on));
        assert!(omni.under && omni.open.is_none() && omni.tick.is_none());
        // Never used: the first named (biggest first), the rest added up in one line that shows them all.
        let never = card.parts.get(3).cloned().unwrap_or_default();
        assert_eq!(
            (never.name.as_str(), never.what.as_str()),
            ("Kontakt Factory", &*format!("Maker · sounds {}", size_text(21 * GB)))
        );
        let last = card.parts.last().cloned().unwrap_or_default();
        assert_eq!(last.name, "And 1 more plugin");
        assert_eq!(last.size, size_text(10 * MB));
        assert_eq!(last.plugins, Some(OnPlugins::Unused));
        assert_eq!(card.parts.len(), 1 + 1 + 1 + LISTED + 1);
        assert!(card.lead.starts_with("No song has used these in over 2 years, and some no song has ever used. "));
        let lately = found(vec![unused("Omnisphere", 3, Some(at(2022, 3)), (85 * MB, 120 * MB, 38 * GB))]);
        assert!(super::card(Some(&lately), false).lead.starts_with("No song has used these in over 2 years. Each"));
        assert!(card.note.contains("on 1 Nov 2026") && card.note.ends_with("Nothing goes from here."));
        // Before the first look ends it's measuring; a look that found none frees nothing.
        let reading = super::card(None, true);
        assert_eq!(reading.big, "Measuring…");
        assert!(reading.parts.is_empty());
        assert_eq!(super::card(Some(&found(Vec::new())), false).big, "Nothing to free");
    }
}
