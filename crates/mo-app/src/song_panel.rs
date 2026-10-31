//! The song panel beside the Songs list: what it says about the picked song. The facts come from the list's row;
//! the plugins, samples, tracks and details come from the library's record of the song's newest save, read on a
//! background thread when the song is picked (rule 12). As the C# app's panel, less what needs writing (song fields
//! stay read-only until the Rust app's database is trusted).

use mo_formats::paths::file_name;
use mo_library::{PluginKind, ProjectPlugin, ProjectSample, Save, SaveDetail, SaveRun};

use crate::shell::folder_of;
use crate::songs::{Other, SongRow, Songs, plural, tempo_text, thousands};

/// The line above the name: "124 BPM · First made 3 Mar 2019 · saved 27 Oct 2026", or "Saved 27 Oct 2026" when it
/// was all one day.
pub fn facts(row: &SongRow) -> String {
    let mut parts = Vec::new();
    if let Some(t) = row.song.latest.tempo {
        parts.push(format!("{} BPM", tempo_text(t)));
    }
    if row.made.date() == row.saved.date() {
        parts.push(format!("Saved {}", row.saved.format("%-d %b %Y")));
    } else {
        parts.push(format!("First made {} · saved {}", row.made.format("%-d %b %Y"), row.saved.format("%-d %b %Y")));
    }
    parts.join(" · ")
}

/// The line under the name: "Newest of 77 saves: Night Drive_77.flp · runs: 2019 original, 2022 rework", or "" for a
/// song saved once.
pub fn saves_text(row: &SongRow) -> String {
    let n = row.song.save_count();
    if n < 2 {
        return String::new();
    }
    let mut text = format!("Newest of {} saves: {}", thousands(n), file_name(&row.song.latest.path));
    if row.runs.len() > 1 {
        let runs: Vec<String> = row.runs.iter().map(|r| r.label()).collect();
        text.push_str(&format!(" · runs: {}", runs.join(", ")));
    }
    text
}

/// "No plugins", "1 plugin", "1,234 plugins".
pub fn count_of(n: usize, thing: &str) -> String {
    if n == 0 { format!("No {thing}s") } else { plural(n, thing) }
}

/// Overview's line of counts, each one opening its own tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Count {
    pub text: String,
    /// "1 missing" beside it, or "".
    pub missing: String,
    pub tab: Tab,
}

/// The panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Saves,
    Plugins,
    Samples,
    Details,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Overview, Tab::Saves, Tab::Plugins, Tab::Samples, Tab::Details];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Saves => "Saves",
            Tab::Plugins => "Plugins",
            Tab::Samples => "Samples",
            Tab::Details => "Details",
        }
    }

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// "9 plugins (1 missing) · 31 samples (3 missing) · 77 saves · 14 channels". Until the save's record is read, the
/// counts the list already has stand in; then all of them come from the record, read in one go.
pub fn counts(row: &SongRow, detail: Option<&SaveDetail>) -> Vec<Count> {
    let l = detail.map_or(&row.song.latest, |d| &d.save);
    let plugins = detail.map_or(l.plugin_count as usize, |d| d.plugins.len());
    let samples = detail.map_or(l.sample_count as usize, |d| d.samples.len());
    let missing = |n: u32| if n > 0 { format!("{} missing", thousands(n as usize)) } else { String::new() };
    let mut out = vec![
        Count { text: count_of(plugins, "plugin"), missing: missing(l.missing_plugins), tab: Tab::Plugins },
        Count { text: count_of(samples, "sample"), missing: missing(l.missing_samples), tab: Tab::Samples },
        Count { text: count_of(row.song.save_count(), "save"), missing: String::new(), tab: Tab::Saves },
    ];
    // A record without its track names (an older scan) keeps the count the list has.
    let tracks = detail.map(|d| d.tracks.len()).filter(|n| *n > 0).or_else(|| usize::try_from(l.track_count).ok());
    if let Some(n) = tracks {
        out.push(Count { text: count_of(n, track_word(&row.song.latest)), missing: String::new(), tab: Tab::Details });
    }
    out
}

/// FL Studio has channels; the other DAWs have tracks.
fn track_word(save: &Save) -> &'static str {
    if save.daw == "FL Studio" { "channel" } else { "track" }
}

/// One plugin on the Plugins tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginLine {
    pub name: String,
    /// "Instrument · VST3 · Xfer Records", leaving out what isn't known.
    pub about: String,
    pub installed: bool,
}

impl PluginLine {
    pub fn new(p: &ProjectPlugin) -> PluginLine {
        let kind = match p.kind {
            PluginKind::Instrument => "Instrument",
            PluginKind::Effect => "Effect",
            PluginKind::NoteEffect => "Note FX",
            PluginKind::Unknown => "",
        };
        let about: Vec<&str> =
            [kind, p.format.as_str(), p.vendor.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
        PluginLine { name: p.name.clone(), about: about.join(" · "), installed: p.installed }
    }
}

/// "Plugins (12, 1 missing)".
pub fn plugins_heading(detail: &SaveDetail) -> String {
    heading("Plugins", detail.plugins.len(), detail.save.missing_plugins)
}

/// "Samples (31, 3 missing)".
pub fn samples_heading(detail: &SaveDetail) -> String {
    heading("Samples", detail.samples.len(), detail.save.missing_samples)
}

fn heading(what: &str, n: usize, missing: u32) -> String {
    if missing > 0 {
        format!("{what} ({}, {} missing)", thousands(n), thousands(missing as usize))
    } else {
        format!("{what} ({})", thousands(n))
    }
}

/// One sound file on the Samples tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleLine {
    pub name: String,
    pub folder: String,
    pub path: String,
    /// Not where the project says it is.
    pub missing: bool,
    /// Loaded, but no note plays it.
    pub unplayed: bool,
}

impl SampleLine {
    pub fn new(s: &ProjectSample) -> SampleLine {
        SampleLine {
            name: file_name(&s.path).to_owned(),
            folder: folder_of(&s.path).to_owned(),
            path: s.path.clone(),
            missing: !s.found,
            unplayed: s.unplayed,
        }
    }
}

/// One line of the Saves tab: a run's heading ("2024 rework", "Didums_77 · 3 Aug 2024 · 12 saves") or a save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveLine {
    Run { label: String, details: String },
    Save { name: String, date: String, path: String, missing: bool, newest: bool },
}

/// The Saves tab: every save of the song, newest first, under the run it belongs to.
pub fn save_lines(row: &SongRow) -> Vec<SaveLine> {
    let mut saves: Vec<&Save> = row.song.saves().collect();
    saves.sort_by_key(|s| std::cmp::Reverse(s.modified));
    let mut out = Vec::with_capacity(saves.len() + row.runs.len());
    let mut run_at = None;
    for s in saves {
        // A save belongs to the run its date falls in. A moved file isn't in any run (runs only count files still
        // there), so it goes with the run nearest its date.
        let run = row
            .runs
            .iter()
            .enumerate()
            .min_by_key(|(_, r)| {
                if s.modified < r.start {
                    r.start - s.modified
                } else if s.modified > r.end {
                    s.modified - r.end
                } else {
                    chrono::TimeDelta::zero()
                }
            })
            .map(|(i, _)| i);
        if row.runs.len() > 1
            && run.is_some()
            && run != run_at
            && let Some(r) = run.and_then(|i| row.runs.get(i))
        {
            out.push(SaveLine::Run { label: r.label(), details: r.details() });
        }
        run_at = run;
        out.push(SaveLine::Save {
            name: s.name.clone(),
            date: s.modified.format("%-d %b %Y").to_string(),
            path: s.path.clone(),
            missing: s.file_missing,
            newest: s.path == row.song.latest.path,
        });
    }
    out
}

/// A version's line, in the Overview's list and the Older saves menu: "2024 rework · Didums_77 · 3 Aug 2024 · 12 saves".
pub fn run_line(run: &SaveRun) -> String {
    format!("{} · {}", run.label(), run.details())
}

/// The song panel's "Older saves" menu, as the C# app's (`OpenMenu.FillOlderSaves`): the song's versions when it has
/// several (saves months apart), otherwise its newest older saves, then a line that shows every save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlderSaves {
    /// "VERSIONS" or "RECENT SAVES".
    pub heading: &'static str,
    pub lines: Vec<OlderSave>,
    /// "See all 12 saves".
    pub see_all: String,
}

/// One line of the Older saves menu: clicking it opens `path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlderSave {
    pub text: String,
    pub path: String,
}

/// How many older saves the menu lists when the song has no versions (the C# app's `OlderSavesShown`).
pub const OLDER_SAVES_SHOWN: usize = 8;

/// The menu, or none for a song with only one save.
pub fn older_saves(row: &SongRow) -> Option<OlderSaves> {
    let count = row.song.save_count();
    if count < 2 {
        return None;
    }
    let see_all = format!("See all {} saves", thousands(count));
    if row.runs.len() > 1 {
        // Newest version first; each opens the last save of its run.
        let lines = row.runs.iter().rev().map(|r| OlderSave { text: run_line(r), path: r.last.path.clone() }).collect();
        return Some(OlderSaves { heading: "VERSIONS", lines, see_all });
    }
    let mut older: Vec<&Save> = row.song.saves().filter(|s| s.path != row.song.latest.path).collect();
    older.sort_by_key(|s| std::cmp::Reverse(s.modified));
    let lines = older
        .into_iter()
        .take(OLDER_SAVES_SHOWN)
        .map(|s| OlderSave { text: format!("{} · {}", s.name, s.modified.format("%-d %b %Y")), path: s.path.clone() })
        .collect();
    Some(OlderSaves { heading: "RECENT SAVES", lines, see_all })
}

/// "77 saves, newest first", or "1 save".
pub fn saves_heading(row: &SongRow) -> String {
    match row.song.save_count() {
        1 => "1 save".to_owned(),
        n => format!("{} saves, newest first", thousands(n)),
    }
}

/// One line of the song's Health strip: something to look at, and where to look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthItem {
    pub text: String,
    /// Why, in a few words, or "".
    pub why: String,
    pub go: Go,
}

/// Where a Health line's button takes you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Go {
    /// Nothing to show yet: the fix comes with a later step.
    Nowhere,
    Tab(Tab),
    /// The other song, by its place in [`Songs::rows`].
    Song(usize),
}

impl Go {
    /// The button's words, or "" for none.
    pub fn label(self) -> &'static str {
        match self {
            Go::Nowhere => "",
            Go::Tab(Tab::Plugins) => "See plugins",
            Go::Tab(Tab::Samples) => "See samples",
            Go::Tab(Tab::Saves) => "See saves",
            Go::Tab(Tab::Overview) => "See overview",
            Go::Tab(Tab::Details) => "See details",
            Go::Song(_) => "Show it",
        }
    }
}

/// The song's Health strip, worst first: a file that can't be read or has gone, what's missing, then the questions
/// about songs that may be saves of one. Answering those (Join, Keep apart) and Fix need the app to keep its own
/// library, so for now the strip shows where to look. The counts come from the save as just read when there is one
/// (`detail`), so they agree with the tabs.
pub fn health(songs: &Songs, row: usize, detail: Option<&SaveDetail>) -> Vec<HealthItem> {
    let Some(r) = songs.rows.get(row) else { return Vec::new() };
    let l = detail.map_or(&r.song.latest, |d| &d.save);
    let mut out = Vec::new();
    if let Some(e) = l.error.as_deref().filter(|e| !e.trim().is_empty()) {
        out.push(HealthItem { text: "The newest save couldn't be read".into(), why: e.to_owned(), go: Go::Nowhere });
    }
    if l.file_missing {
        out.push(HealthItem {
            text: "The newest save's file has moved or gone".into(),
            why: "since the library last looked".into(),
            go: Go::Nowhere,
        });
    }
    if l.missing_plugins > 0 {
        let n = l.missing_plugins as usize;
        out.push(HealthItem {
            text: format!(
                "{} not installed",
                if n == 1 { "1 plugin is".to_owned() } else { format!("{} plugins are", thousands(n)) }
            ),
            why: String::new(),
            go: Go::Tab(Tab::Plugins),
        });
    }
    if l.missing_samples > 0 {
        let n = l.missing_samples as usize;
        out.push(HealthItem {
            text: format!(
                "{} can't be found",
                if n == 1 { "1 sample".to_owned() } else { format!("{} samples", thousands(n)) }
            ),
            why: String::new(),
            go: Go::Tab(Tab::Samples),
        });
    }
    for d in songs.doubts_of(row) {
        let item = match &d.other {
            Other::Save(name) => HealthItem {
                text: format!("{name} might be a song of its own"),
                why: d.why.clone(),
                go: Go::Tab(Tab::Saves),
            },
            Other::Song(b) => {
                let other = if *b == row { d.a } else { *b };
                let name = songs.rows.get(other).map_or("", |o| o.song.name.as_str());
                HealthItem {
                    text: format!("Might be the same song as {name}"),
                    why: d.why.clone(),
                    go: Go::Song(other),
                }
            }
        };
        out.push(item);
    }
    out
}

/// The strip's folded line: "Health · 3 things to look at", or "" when there's nothing.
pub fn health_summary(items: &[HealthItem]) -> String {
    match items.len() {
        0 => String::new(),
        1 => "Health · 1 thing to look at".to_owned(),
        n => format!("Health · {n} things to look at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::songs::Songs;
    use chrono::{NaiveDate, NaiveDateTime};
    use mo_library::SongChoices;

    fn day(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap()
    }

    fn save(name: &str, saved: NaiveDateTime) -> Save {
        Save {
            name: name.into(),
            path: format!(r"C:\beats\{name}.flp"),
            daw: "FL Studio".into(),
            folder: r"C:\beats".into(),
            modified: saved,
            ..Save::default()
        }
    }

    fn row(saves: &[Save]) -> SongRow {
        Songs::from_saves(saves, &SongChoices::default(), Vec::new(), 3, Some(false)).rows.remove(0)
    }

    #[test]
    fn header_lines() {
        let newest = Save { tempo: Some(124.0), ..save("Night Drive_3", day(2026, 10, 27)) };
        let r = row(&[newest, save("Night Drive_2", day(2024, 5, 1)), save("Night Drive", day(2019, 3, 3))]);
        assert_eq!(facts(&r), "124 BPM · First made 3 Mar 2019 · saved 27 Oct 2026");
        assert_eq!(
            saves_text(&r),
            "Newest of 3 saves: Night Drive_3.flp · runs: 2019 original, 2024 re-save, 2026 re-save"
        );
        let once = row(&[save("Solo", day(2025, 1, 1))]);
        assert_eq!((facts(&once).as_str(), saves_text(&once).as_str()), ("Saved 1 Jan 2025", ""));
    }

    #[test]
    fn counts_use_the_list_until_the_record_is_read() {
        let newest = Save {
            plugin_count: 9,
            sample_count: 31,
            missing_samples: 3,
            track_count: 14,
            ..save("A_2", day(2025, 2, 1))
        };
        let r = row(&[newest.clone(), save("A", day(2025, 1, 1))]);
        let texts = |c: Vec<Count>| {
            c.into_iter().map(|c| format!("{} {}", c.text, c.missing).trim().to_owned()).collect::<Vec<_>>()
        };
        assert_eq!(texts(counts(&r, None)), ["9 plugins", "31 samples 3 missing", "2 saves", "14 channels"]);
        // Once read, the record's own counts are used, the missing ones too.
        let reread = Save { missing_samples: 0, ..newest };
        let detail = SaveDetail { tracks: vec!["Kick".into()], save: reread.clone(), ..SaveDetail::default() };
        assert_eq!(texts(counts(&r, Some(&detail))), ["No plugins", "No samples", "2 saves", "1 channel"]);
        // A record without track names keeps the list's count.
        let no_tracks = SaveDetail { save: reread, ..SaveDetail::default() };
        assert_eq!(texts(counts(&r, Some(&no_tracks)))[3], "14 channels");
        let unknown = row(&[save("B", day(2025, 1, 1))]);
        assert_eq!(counts(&unknown, None).len(), 3);
    }

    #[test]
    fn the_older_saves_menu_lists_versions_or_the_newest_older_saves() {
        assert_eq!(older_saves(&row(&[save("Solo", day(2025, 1, 1))])), None);

        let versions = older_saves(&row(&[
            save("Didums_4", day(2024, 8, 3)),
            save("Didums_3", day(2024, 8, 1)),
            save("Didums_2", day(2019, 1, 20)),
            save("Didums", day(2019, 1, 10)),
        ]))
        .unwrap();
        assert_eq!(versions.heading, "VERSIONS");
        assert_eq!(versions.see_all, "See all 4 saves");
        let texts: Vec<&str> = versions.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            ["2024 re-save · Didums_4 · 3 Aug 2024 · 2 saves", "2019 original · Didums_2 · 20 Jan 2019 · 2 saves"]
        );
        assert_eq!(versions.lines[0].path, r"C:\beats\Didums_4.flp");

        // One run: the newest older saves, at most eight, never the newest save itself.
        let saves: Vec<Save> = (1..=12).map(|n| save(&format!("Loop_{n}"), day(2025, 3, n))).collect();
        let recent = older_saves(&row(&saves)).unwrap();
        assert_eq!(recent.heading, "RECENT SAVES");
        assert_eq!(recent.lines.len(), OLDER_SAVES_SHOWN);
        assert_eq!(recent.lines[0].text, "Loop_11 · 11 Mar 2025");
        assert_eq!(recent.see_all, "See all 12 saves");
    }

    #[test]
    fn saves_sit_under_their_runs_newest_first() {
        let r = row(&[
            save("Didums_4", day(2024, 8, 3)),
            save("Didums_3", day(2024, 8, 1)),
            save("Didums_2", day(2019, 1, 20)),
            Save { file_missing: true, ..save("Didums", day(2019, 1, 10)) },
            Save { file_missing: true, ..save("Didums_1", day(2020, 1, 1)) },
        ]);
        let lines: Vec<String> = save_lines(&r)
            .into_iter()
            .map(|l| match l {
                SaveLine::Run { label, details } => format!("# {label} ({details})"),
                SaveLine::Save { name, date, missing, newest, .. } => {
                    format!(
                        "{name} {date}{}{}",
                        if missing { " missing" } else { "" },
                        if newest { " newest" } else { "" }
                    )
                }
            })
            .collect();
        assert_eq!(
            lines,
            [
                "# 2024 re-save (Didums_4 · 3 Aug 2024 · 2 saves)",
                "Didums_4 3 Aug 2024 newest",
                "Didums_3 1 Aug 2024",
                "# 2019 original (Didums_2 · 20 Jan 2019)",
                // Moved, and nearer the 2019 run than the 2024 one.
                "Didums_1 1 Jan 2020 missing",
                "Didums_2 20 Jan 2019",
                "Didums 10 Jan 2019 missing",
            ]
        );
        assert_eq!(saves_heading(&r), "5 saves, newest first");
        // One run needs no heading.
        let one = row(&[save("X_2", day(2024, 8, 3)), save("X", day(2024, 8, 1))]);
        assert!(save_lines(&one).iter().all(|l| matches!(l, SaveLine::Save { .. })));
    }

    #[test]
    fn health_lists_problems_then_questions() {
        let broken =
            Save { missing_plugins: 1, missing_samples: 3, file_missing: true, ..save("Sunrise", day(2025, 2, 1)) };
        let s = Songs::from_saves(
            &[broken, save("Sunrize", day(2025, 1, 1))],
            &SongChoices::default(),
            Vec::new(),
            3,
            Some(false),
        );
        let sunrise = s.rows.iter().position(|r| r.song.name == "Sunrise").unwrap();
        let sunrize = s.rows.iter().position(|r| r.song.name == "Sunrize").unwrap();
        let items = health(&s, sunrise, None);
        let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "The newest save's file has moved or gone",
                "1 plugin is not installed",
                "3 samples can't be found",
                "Might be the same song as Sunrize",
            ]
        );
        assert_eq!(items[3].go, Go::Song(sunrize));
        assert_eq!(
            (items[1].go.label(), items[2].go.label(), items[3].go.label()),
            ("See plugins", "See samples", "Show it")
        );
        // The other song asks the same question the other way round.
        assert_eq!(health(&s, sunrize, None)[0].text, "Might be the same song as Sunrise");
        assert_eq!(health_summary(&items), "Health · 4 things to look at");
        assert_eq!(health_summary(&[]), "");
        // Not asked when Settings turned the questions off.
        let quiet = Songs::from_saves(
            &[save("Sunrise", day(2025, 2, 1)), save("Sunrize", day(2025, 1, 1))],
            &SongChoices::default(),
            Vec::new(),
            3,
            None,
        );
        assert!(quiet.doubts.is_empty());
    }

    #[test]
    fn plugin_and_sample_lines() {
        let p = ProjectPlugin {
            name: "Serum".into(),
            vendor: "Xfer Records".into(),
            kind: PluginKind::Instrument,
            format: "VST3".into(),
            path: String::new(),
            installed: false,
        };
        assert_eq!(PluginLine::new(&p).about, "Instrument · VST3 · Xfer Records");
        let bare = ProjectPlugin { kind: PluginKind::Unknown, vendor: String::new(), ..p };
        assert_eq!(PluginLine::new(&bare).about, "VST3");
        let s = SampleLine::new(&ProjectSample {
            path: r"D:\Samples\Kicks\Kick 01.wav".into(),
            found: false,
            unplayed: true,
        });
        assert_eq!((s.name.as_str(), s.folder.as_str(), s.missing), ("Kick 01.wav", r"D:\Samples\Kicks", true));
        assert_eq!(folder_of("/home/me/a.wav"), "/home/me");
        assert_eq!(folder_of("a.wav"), "");
    }
}
