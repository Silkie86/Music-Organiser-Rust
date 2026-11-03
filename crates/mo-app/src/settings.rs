//! The Settings page (spec 3.9, folded from the C# app's nine tabs to five, 3A): General (the look, the size of
//! everything, the start page, the pace, the sound output, and About with the "Made with Slint" credit), Folders (where
//! your projects, samples, MIDI and moved plugin data are), DAWs, Shortcuts and Developer. A page's own numbers live in
//! an Options popover on that page, not here.
//!
//! Every word the page shows is decided here, so it's tested without a window. The choices are kept by
//! [`crate::app_settings`]: this app's own file over the C# app's, which stays read-only.

use std::path::Path;

use mo_space::folder_move::MovedFolder;

use crate::background::Pace;
use crate::places::{LAST_PAGE_CHOICE, Place};
use crate::themes::{ACCENTS, FONTS, SIZES, THEMES, Theme, font_named, parse_hex, size_from_setting};

/// The page's tabs, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    General,
    Folders,
    Daws,
    Shortcuts,
    Developer,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::General, Tab::Folders, Tab::Daws, Tab::Shortcuts, Tab::Developer];

    pub fn name(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Folders => "Folders",
            Tab::Daws => "DAWs",
            Tab::Shortcuts => "Shortcuts",
            Tab::Developer => "Developer",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Tab> {
        Tab::ALL.get(i).copied()
    }

    /// The tab Settings opens on, from the `settings_tab` setting (the last one used, as in the C# app). The C# app's
    /// tabs fold into these: Appearance and Performance were General's, Projects, Sounds and Plugins held their
    /// folders, DAW links is DAWs, and anything else opens General.
    pub fn from_setting(value: Option<&str>) -> Tab {
        match value.unwrap_or_default() {
            "Folders" | "Projects" | "Sounds" | "Plugins" | "MIDI" => Tab::Folders,
            "DAWs" | "DAW links" => Tab::Daws,
            "Shortcuts" => Tab::Shortcuts,
            "Developer" => Tab::Developer,
            _ => Tab::General,
        }
    }

    /// What the tab is for, under the page's title.
    pub fn summary(self) -> &'static str {
        match self {
            Tab::General => "How the app looks and behaves everywhere",
            Tab::Folders => "Where your projects, samples, MIDI and plugin data are",
            Tab::Daws => "The links that let the app talk to Bitwig and FL Studio",
            Tab::Shortcuts => "Every key, in one list",
            Tab::Developer => "The app's version, its logs and its own files, for testing at home",
        }
    }
}

/// One theme to pick, drawn as a little picture of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeCard {
    pub name: &'static str,
    pub description: &'static str,
    /// Its colours, 0xRRGGBB, for the picture.
    pub background: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub square: bool,
    pub picked: bool,
}

/// Every theme, the picked one marked (Graphite when the setting names none we know).
pub fn theme_cards(theme_setting: Option<&str>) -> Vec<ThemeCard> {
    let picked = theme_setting.filter(|n| THEMES.iter().any(|t| t.name == *n)).unwrap_or(THEMES[0].name);
    THEMES
        .iter()
        .map(|t: &Theme| ThemeCard {
            name: t.name,
            description: t.description,
            background: t.background,
            sidebar: t.sidebar,
            surface: t.surface,
            text: t.text,
            muted: t.muted,
            accent: t.accent,
            square: t.square,
            picked: t.name == picked,
        })
        .collect()
}

/// One accent to pick: its name and colour (the theme's own for the first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccentChoice {
    pub name: &'static str,
    pub colour: u32,
    pub picked: bool,
}

/// The accents, with the theme's own first, and which is picked. The setting is "#RRGGBB", or empty for the theme's
/// own, as the C# app saves it; a colour that isn't one of these (picked some other way) leaves none marked but the
/// theme's own.
pub fn accent_choices(theme: &Theme, accent_setting: Option<&str>) -> Vec<AccentChoice> {
    let picked = accent_setting.and_then(parse_hex);
    let any = ACCENTS.iter().any(|(_, c)| c.is_some() && *c == picked);
    ACCENTS
        .iter()
        .map(|(name, colour)| AccentChoice {
            name,
            colour: colour.unwrap_or(theme.accent),
            picked: match colour {
                None => !any,
                Some(c) => Some(*c) == picked,
            },
        })
        .collect()
}

/// The accent setting for a choice: "#RRGGBB", or "" for the theme's own.
pub fn accent_setting(index: usize) -> Option<String> {
    let (_, colour) = ACCENTS.get(index)?;
    Some(colour.map_or_else(String::new, |c| format!("#{c:06X}")))
}

/// Which of [`FONTS`] is picked: the theme's own (0) unless the setting names another we have.
pub fn font_index(setting: Option<&str>) -> usize {
    setting.filter(|f| font_named(f).is_some()).and_then(|f| FONTS.iter().position(|n| *n == f)).unwrap_or(0)
}

/// The size choices as shown ("100%").
pub fn size_labels() -> Vec<String> {
    SIZES.iter().map(|s| format!("{s}%")).collect()
}

/// Which of [`SIZES`] is picked.
pub fn size_index(setting: Option<&str>) -> usize {
    let size = size_from_setting(setting);
    SIZES.iter().position(|s| *s == size).unwrap_or(3)
}

/// What the size of everything does, under its choices.
pub fn size_note(size: u32) -> String {
    match size {
        100 => "Everything at its normal size.".to_owned(),
        s if s < 100 => format!("Everything at {s}%: more fits on the screen."),
        s => format!("Everything at {s}%: easier to read."),
    }
}

/// The start page choices: every place, then "Last page I used".
pub fn start_choices() -> Vec<&'static str> {
    Place::ALL.iter().map(|p| p.name()).chain([LAST_PAGE_CHOICE]).collect()
}

/// Which start page choice is picked. The C# app's names count as the places they became (Projects is Songs).
pub fn start_index(setting: Option<&str>) -> usize {
    match setting {
        None => 0,
        Some(LAST_PAGE_CHOICE) => Place::ALL.len(),
        Some(name) => Place::from_page_name(name).index(),
    }
}

/// The setting for a start page choice.
pub fn start_setting(index: usize) -> Option<&'static str> {
    start_choices().get(index).copied()
}

/// What a pace does on this computer, under its choices, and that it takes over at the next start.
pub fn pace_note(pace: Pace, chosen: Pace, cores: usize) -> String {
    let threads = pace.threads(cores);
    let what = match pace {
        Pace::Gentle => format!(
            "Background work uses up to {threads} of your {cores} processor threads, so the app stays smooth while it works."
        ),
        Pace::Balanced => format!("Background work uses up to {threads} of your {cores} processor threads."),
        Pace::Fast => format!(
            "Background work uses all {cores} of your processor threads, always at low priority, so it finishes soonest."
        ),
    };
    if pace == chosen {
        what
    } else {
        format!("{what} From the next time the app starts: it's using {} now.", chosen.name())
    }
}

/// The sound output list: the computer's default first, then each output there is now, and the one saved when it
/// isn't there now (unplugged). Each line's label and the name it saves ("" for the default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outputs {
    pub labels: Vec<String>,
    pub names: Vec<String>,
    pub picked: usize,
}

/// The list from the outputs there are (the computer's default first, as the player lists them) and the saved name.
/// `windows`: Windows says it's "Windows' default output"; elsewhere "the computer's".
pub fn outputs(there: &[String], saved: Option<&str>, windows: bool) -> Outputs {
    let default = if windows { "Windows' default output" } else { "The computer's default output" };
    let first = match there.first() {
        Some(now) if windows => format!("{default} (now {now})"),
        _ => default.to_owned(),
    };
    let mut labels = vec![first];
    let mut names = vec![String::new()];
    for name in there {
        labels.push(name.clone());
        names.push(name.clone());
    }
    let saved = saved.filter(|s| !s.is_empty());
    if let Some(s) = saved
        && !there.iter().any(|n| n == s)
    {
        labels.push(format!("{s} (not there now)"));
        names.push(s.to_owned());
    }
    let picked = saved.and_then(|s| names.iter().position(|n| n == s)).unwrap_or(0);
    Outputs { labels, names, picked }
}

/// Where sounds go, under the list: the output in use (once a sound has opened one) or why sound can't go where chosen.
pub fn output_note(open: Option<&str>, problem: Option<&str>) -> String {
    match (problem, open) {
        (Some(p), _) => p.to_owned(),
        (None, Some(name)) => format!("Sounds play through {name}."),
        (None, None) => "Nothing has played yet: the output opens with the first sound.".to_owned(),
    }
}

/// One group of folders on the Folders tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderGroup {
    pub title: &'static str,
    pub note: &'static str,
    pub folders: Vec<String>,
    /// Shown when there are none.
    pub empty: &'static str,
}

/// What the Folders tab shows, as the C# app keeps it in library.db: the project folders (`roots`), sample folders,
/// MIDI folders, plugin folders (the standard ones when none were chosen) and plugin data moved to another drive.
pub fn folder_groups(
    projects: Vec<String>,
    samples: Vec<String>,
    midi: Vec<String>,
    plugins: Vec<String>,
    plugin_defaults: Vec<String>,
    moved: &[MovedFolder],
) -> Vec<FolderGroup> {
    let plugins_are_defaults = plugins.is_empty();
    vec![
        FolderGroup {
            title: "PROJECTS",
            note: "Searched for Bitwig, FL Studio and Studio One projects, subfolders too. Autosaves, backups and the \
                   demo songs that came with your DAWs are skipped.",
            folders: projects,
            empty: "No project folders yet.",
        },
        FolderGroup {
            title: "SAMPLES",
            note: "Your sample libraries: what Sounds shows, and where missing samples are looked for.",
            folders: samples,
            empty: "No sample folders yet.",
        },
        FolderGroup {
            title: "MIDI",
            note: "Your MIDI packs, read for the MIDI page beside the MIDI inside your songs.",
            folders: midi,
            empty: "No MIDI folders yet.",
        },
        FolderGroup {
            title: "PLUGINS",
            note: if plugins_are_defaults {
                "Where your VST3, CLAP and VST2 plugins are installed: the standard folders, as none were chosen."
            } else {
                "Where your VST3, CLAP and VST2 plugins are installed."
            },
            folders: if plugins_are_defaults { plugin_defaults } else { plugins },
            empty: "No plugin folders.",
        },
        FolderGroup {
            title: "PLUGIN DATA ON OTHER DRIVES",
            note: "Plugin folders moved off their drive, and where they are now. Move one from its plugin's card.",
            folders: moved.iter().map(|m| format!("{}  →  {}", m.from, m.to)).collect(),
            empty: "Nothing moved yet.",
        },
    ]
}

/// The line under the Folders tab's title: they're shown, and changing them waits.
pub const FOLDERS_NOTE: &str = "These are the folders the C# app keeps. Adding and removing them here comes once the \
                                app may write to your library (library.db); until then, change them in the C# app.";

/// One line of the About part or the Developer tab: what it is and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub what: &'static str,
    pub value: String,
    /// What its button does to it, if it has one.
    pub button: Option<FactButton>,
}

/// A Developer fact's button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactButton {
    /// Open the folder.
    Open,
    /// Show the file in its folder.
    Show,
}

impl FactButton {
    pub fn name(self) -> &'static str {
        match self {
            FactButton::Open => "Open",
            FactButton::Show => "Show",
        }
    }
}

/// The Developer tab's facts: the build, and where the app keeps its files and logs.
pub fn developer_facts(version: &str, debug: bool, data: &Path, logs: &Path, log_file: Option<&Path>) -> Vec<Fact> {
    vec![
        Fact { what: "Version", value: version.to_owned(), button: None },
        Fact {
            what: "Build",
            value: if debug { "Debug (the launcher's: slower, with checks)" } else { "Release" }.to_owned(),
            button: None,
        },
        Fact { what: "Data folder", value: data.display().to_string(), button: Some(FactButton::Open) },
        Fact { what: "Logs folder", value: logs.display().to_string(), button: Some(FactButton::Open) },
        match log_file {
            Some(p) => Fact { what: "This run's log", value: p.display().to_string(), button: Some(FactButton::Show) },
            None => {
                Fact { what: "This run's log", value: "Not written (the log couldn't start)".to_owned(), button: None }
            }
        },
    ]
}

/// One line of the Shortcuts tab: a page's heading (only `page` set) or an action with its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyLine {
    /// The page's name on a heading line; empty on an action's line.
    pub page: &'static str,
    pub id: &'static str,
    pub what: &'static str,
    /// Its key now, or "None".
    pub key: String,
    /// Not the key it started with, so Default can put that back.
    pub changed: bool,
    /// The key it starts with, for the Default button's tooltip.
    pub starts: String,
}

/// The Shortcuts tab's lines: the keys that work everywhere, then each page's own, each under its page's name.
pub fn key_lines(keys: &crate::shortcuts::Shortcuts) -> Vec<KeyLine> {
    let mut out = Vec::new();
    let mut last = "";
    for (page, a) in crate::shortcuts::all_actions() {
        if page != last {
            out.push(KeyLine { page, id: "", what: "", key: String::new(), changed: false, starts: String::new() });
            last = page;
        }
        let key = keys.key_of(a.id);
        out.push(KeyLine {
            page: "",
            id: a.id,
            what: a.what,
            key: if key.is_empty() { "None".to_owned() } else { key.to_owned() },
            changed: keys.is_changed(a.id),
            starts: if a.default_key.is_empty() {
                "It starts with no key".to_owned()
            } else {
                format!("It starts as {}", a.default_key)
            },
        });
    }
    out
}

/// The Shortcuts tab's words above the list.
pub const KEYS_NOTE: &str = "Keys that work on every page come first, then each page's own. Click a key and press the \
                             one you want (Esc stops listening); the same idea has the same key on every page. F12 \
                             stays the note for Claude, and F1 or ? on a page shows its keys.";

/// The credit Slint's royalty-free licence asks for (decided 2026-10-30), with what it means.
/// How a DAW's link stands, for a row on the DAWs tab: a short state, whether it's working, and the facts under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DawStatus {
    pub daw: &'static str,
    pub state: String,
    /// Whether the link answers and the app could send to it now.
    pub ok: bool,
    pub facts: Vec<Fact>,
}

/// Bitwig's row before it's been checked.
pub fn bitwig_checking() -> DawStatus {
    DawStatus { daw: "Bitwig Studio", state: "Checking\u{2026}".to_owned(), ok: false, facts: Vec::new() }
}

/// Bitwig's row from what the link said when the app connected (`mo_dawlink::bitwig::Bitwig::check`). Only reads:
/// nothing is sent to the project.
pub fn bitwig_status(checked: &Result<mo_dawlink::bitwig::Hello, mo_dawlink::bitwig::Error>) -> DawStatus {
    use mo_dawlink::bitwig::Error;
    let fact = |what, value: String| Fact { what, value, button: None };
    let hello = match checked {
        Ok(hello) => hello,
        Err(Error::NotRunning) => {
            return DawStatus {
                daw: "Bitwig Studio",
                state: "Not answering".to_owned(),
                ok: false,
                facts: vec![fact("What to check", Error::NotRunning.to_string())],
            };
        }
        Err(e) => {
            return DawStatus {
                daw: "Bitwig Studio",
                state: "Not linked".to_owned(),
                ok: false,
                facts: vec![fact("What happened", e.to_string())],
            };
        }
    };
    let mut facts = Vec::new();
    let product = if hello.product.is_empty() { "Bitwig Studio" } else { hello.product.as_str() };
    facts.push(fact("Bitwig", format!("{product} {}", hello.version).trim_end().to_owned()));
    let mut answering = hello.answering();
    if let Some(first) = answering.get(..1) {
        answering = first.to_uppercase() + &answering[1..];
    }
    facts.push(fact("Answering", answering));
    if hello.error.is_none() {
        let project = if hello.project.is_empty() { "A new project, not saved yet" } else { hello.project.as_str() };
        let unsaved = if hello.modified { " \u{b7} unsaved changes" } else { "" };
        facts.push(fact("Open project", format!("{project}{unsaved}")));
        facts.push(fact(
            "Changes from the app",
            if hello.changes {
                "Allowed".to_owned()
            } else {
                "Off: turn on \u{201c}Let the app make changes\u{201d} in Bitwig (Settings > Controllers > Music \
                 Organiser) to send sounds and kits"
                    .to_owned()
            },
        ));
        if let Some(back) = hello.loopback {
            facts.push(fact(
                "Loopback MIDI",
                if back {
                    "Working".to_owned()
                } else {
                    format!(
                        "Not coming back: set the controller's MIDI in and out to \u{201c}{}\u{201d}",
                        mo_dawlink::bitwig::LOOPBACK_PORT
                    )
                },
            ));
        }
    }
    let (state, ok) = match hello.can_send() {
        Ok(()) => ("Linked".to_owned(), true),
        Err(Error::NotStarted(_)) => ("Linked, but not working fully".to_owned(), false),
        Err(Error::OldLink) => ("Linked, but too old to send to".to_owned(), false),
        Err(_) => ("Linked, reading only".to_owned(), false),
    };
    if let Err(why) = hello.can_send()
        && !matches!(why, Error::ChangesOff)
    {
        facts.push(fact("Sending", why.to_string()));
    }
    DawStatus { daw: "Bitwig Studio", state, ok, facts }
}

/// FL Studio's row: its link comes later in phase 5.
pub fn fl_status() -> DawStatus {
    DawStatus {
        daw: "FL Studio",
        state: "Comes later".to_owned(),
        ok: false,
        facts: vec![Fact {
            what: "For now",
            value: "The Rust app's FL Studio link comes later in phase 5. The old app's Settings > DAW links still \
                    installs and checks FL's script."
                .to_owned(),
            button: None,
        }],
    }
}

/// Under the DAW rows: where the scripts are installed from for now.
pub const DAWS_NOTE: &str = "The app talks to Bitwig through the Music Organiser extension. Installing it, and the \
    live tests, stay in the old app's Settings > DAW links for now; this tab checks that the link answers, and only \
    reads.";

pub const SLINT_CREDIT: &str = "Music Organiser's screens are made with Slint, under its royalty-free licence.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_open_where_you_left_them_and_csharp_tabs_fold_in() {
        assert_eq!(Tab::from_setting(None), Tab::General);
        assert_eq!(Tab::from_setting(Some("Shortcuts")), Tab::Shortcuts);
        assert_eq!(Tab::from_setting(Some("DAW links")), Tab::Daws);
        assert_eq!(Tab::from_setting(Some("Sounds")), Tab::Folders);
        assert_eq!(Tab::from_setting(Some("Playground")), Tab::General);
        for t in Tab::ALL {
            assert_eq!(Tab::from_setting(Some(t.name())), t);
            assert_eq!(Tab::from_index(t.index()), Some(t));
        }
    }

    #[test]
    fn one_theme_is_picked_and_graphite_by_default() {
        let cards = theme_cards(None);
        assert_eq!(cards.len(), THEMES.len());
        assert!(cards[0].picked && cards.iter().filter(|c| c.picked).count() == 1);
        let paper = theme_cards(Some("Paper"));
        assert!(paper.iter().find(|c| c.name == "Paper").unwrap().picked);
        assert_eq!(paper.iter().filter(|c| c.picked).count(), 1);
        assert!(theme_cards(Some("Gone")).iter().find(|c| c.name == "Graphite").unwrap().picked);
    }

    #[test]
    fn accents_round_trip_as_the_csharp_app_saves_them() {
        let graphite = THEMES[0];
        let own = accent_choices(&graphite, Some(""));
        assert!(own[0].picked);
        assert_eq!(own[0].colour, graphite.accent);
        let blue = accent_choices(&graphite, Some("#4FB3FF"));
        assert!(blue[2].picked && !blue[0].picked);
        assert_eq!(accent_setting(2).as_deref(), Some("#4FB3FF"));
        assert_eq!(accent_setting(0).as_deref(), Some(""));
        assert_eq!(accent_setting(99), None);
        // A colour picked some other way: only the theme's own is marked, never nothing.
        assert!(accent_choices(&graphite, Some("#123456"))[0].picked);
    }

    #[test]
    fn fonts_sizes_and_start_pages_read_the_csharp_values() {
        assert_eq!(font_index(Some("Space (retro)")), 2);
        assert_eq!(font_index(Some("Inter")), 0);
        assert_eq!(font_index(None), 0);
        assert_eq!(size_index(Some("125")), 5);
        assert_eq!(size_index(Some("77")), 3);
        assert_eq!(size_labels()[0], "70%");
        assert_eq!(start_index(Some("Sound Lab")), Place::Sounds.index());
        assert_eq!(start_index(Some("Projects")), Place::Songs.index());
        assert_eq!(start_index(Some(LAST_PAGE_CHOICE)), Place::ALL.len());
        assert_eq!(start_setting(start_index(Some(LAST_PAGE_CHOICE))), Some(LAST_PAGE_CHOICE));
        assert_eq!(start_setting(Place::Midi.index()), Some("MIDI"));
    }

    #[test]
    fn the_pace_note_says_when_a_change_takes_over() {
        assert_eq!(
            pace_note(Pace::Gentle, Pace::Gentle, 16),
            "Background work uses up to 8 of your 16 processor threads, so the app stays smooth while it works."
        );
        assert!(
            pace_note(Pace::Balanced, Pace::Fast, 16)
                .ends_with("From the next time the app starts: it's using Fast now.")
        );
    }

    #[test]
    fn an_unplugged_output_stays_picked_and_says_so() {
        let there = vec!["Speakers".to_owned(), "Focusrite USB".to_owned()];
        let o = outputs(&there, Some("Focusrite USB"), true);
        assert_eq!(o.labels[0], "Windows' default output (now Speakers)");
        assert_eq!(o.picked, 2);
        let gone = outputs(&there, Some("MOTU M2"), true);
        assert_eq!(gone.labels.last().unwrap(), "MOTU M2 (not there now)");
        assert_eq!(gone.names[gone.picked], "MOTU M2");
        let default = outputs(&[], Some(""), false);
        assert_eq!(default.labels, vec!["The computer's default output".to_owned()]);
        assert_eq!(default.picked, 0);
        assert_eq!(output_note(Some("Speakers"), None), "Sounds play through Speakers.");
        assert_eq!(output_note(None, Some("MOTU M2 isn't connected")), "MOTU M2 isn't connected");
    }

    #[test]
    fn only_folders_and_a_written_log_have_buttons() {
        let facts = developer_facts("0.1.0 (abc)", true, Path::new("D"), Path::new("D/logs"), None);
        assert_eq!(facts.iter().filter(|f| f.button == Some(FactButton::Open)).count(), 2);
        assert_eq!(facts[4].button, None);
        let written = developer_facts("v", false, Path::new("D"), Path::new("L"), Some(Path::new("L/r.log")));
        assert_eq!(written[4].button, Some(FactButton::Show));
        assert_eq!(written[1].value, "Release");
    }

    #[test]
    fn key_lines_come_under_their_pages() {
        let mut keys = crate::shortcuts::Shortcuts::default();
        keys.set("app.fullRescan", "F6");
        let lines = key_lines(&keys);
        assert_eq!(lines[0].page, crate::shortcuts::EVERYWHERE);
        let heads: Vec<&str> = lines.iter().filter(|l| !l.page.is_empty()).map(|l| l.page).collect();
        assert_eq!(heads, vec!["Everywhere", "Songs", "Sounds", "Space"]);
        let rescan = lines.iter().find(|l| l.id == "app.fullRescan").unwrap();
        assert_eq!((rescan.key.as_str(), rescan.changed), ("F6", true));
        assert_eq!(rescan.starts, "It starts with no key");
        let search = lines.iter().find(|l| l.id == "app.search").unwrap();
        assert_eq!((search.key.as_str(), search.changed), ("Ctrl+F", false));
    }

    #[test]
    fn plugin_folders_fall_back_to_the_standard_ones() {
        let groups = folder_groups(vec![], vec![], vec![], vec![], vec!["C:\\VST3".to_owned()], &[]);
        assert_eq!(groups[3].folders, vec!["C:\\VST3".to_owned()]);
        assert!(groups[3].note.contains("standard"));
        let chosen =
            folder_groups(vec![], vec![], vec![], vec!["D:\\VST".to_owned()], vec!["C:\\VST3".to_owned()], &[]);
        assert_eq!(chosen[3].folders, vec!["D:\\VST".to_owned()]);
    }

    #[test]
    fn bitwig_rows_say_how_the_link_stands() {
        use mo_dawlink::bitwig::{Error, Hello};
        let off = bitwig_status(&Err(Error::NotRunning));
        assert_eq!((off.state.as_str(), off.ok), ("Not answering", false));
        let hello = Hello {
            link: 3,
            product: "Bitwig Studio".into(),
            version: "6.1.5".into(),
            project: "Night drive".into(),
            modified: true,
            changes: true,
            runtime: "java".into(),
            extension: "3.18".into(),
            loopback: Some(true),
            ..Hello::default()
        };
        let row = bitwig_status(&Ok(hello.clone()));
        assert_eq!((row.state.as_str(), row.ok), ("Linked", true));
        let values: Vec<&str> = row.facts.iter().map(|f| f.value.as_str()).collect();
        assert_eq!(
            values,
            [
                "Bitwig Studio 6.1.5",
                "The Music Organiser extension 3.18",
                "Night drive \u{b7} unsaved changes",
                "Allowed",
                "Working"
            ]
        );
        let reading = bitwig_status(&Ok(Hello { changes: false, ..hello.clone() }));
        assert_eq!((reading.state.as_str(), reading.ok), ("Linked, reading only", false));
        let broken = bitwig_status(&Ok(Hello { error: Some("no MIDI".into()), ..hello }));
        assert_eq!(broken.state, "Linked, but not working fully");
        assert!(broken.facts.iter().all(|f| f.what != "Open project"));
        assert!(broken.facts.last().is_some_and(|f| f.value.ends_with("Bitwig said: no MIDI")));
    }
}
