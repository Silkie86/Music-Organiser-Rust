//! The app's own settings: what you change on the Settings page (the look, the start page, the pace, the sound output,
//! your keys) and what the app remembers between runs (the last page, the Settings tab).
//!
//! `library.db` is shared with the C# app and stays read-only until the Rust pages are trusted (CLAUDE.md), so this
//! app keeps its changes in a small file of its own, `rust-settings.json` in the app's folder. On the first run, and
//! for anything you haven't changed here, the C# app's choices are read from `library.db`'s `settings` table under the
//! same names, so the Rust app opens in your look, on your start page, with your keys. The C# app never sees what you
//! change here.
//!
//! The file is written the safe way (rule on files on disk): to a `.mo-part` file beside it, flushed to the disk, then
//! renamed over the old one, so a crash part way leaves the old file whole. A file that can't be read is kept as
//! `rust-settings.json.unreadable` (nothing is lost) and the app opens in the C# app's choices.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use mo_library::Library;

use crate::{places, shortcuts, themes};

/// The file's name, in the app's folder.
pub const FILE: &str = "rust-settings.json";

/// Every file in the data folder written through [`write_checked`] (as `<name>.mo-part` first): the app's settings and
/// the Space page's list of what it put in the Recycle Bin. A new caller adds its file's name here, so the start-up
/// sweep clears what a crash leaves of it (`crate::tidy::is_cut_short`).
pub const WRITTEN_CHECKED: [&str; 2] = [FILE, crate::binned::FILE];

/// Which page the app opens on: a page's name, or [`places::LAST_PAGE_CHOICE`]. The C# app's name.
pub const START_PAGE_KEY: &str = "start_page";
/// The page last shown, for "Last page I used". The C# app's name.
pub const LAST_PAGE_KEY: &str = "last_page";
/// How hard background work may push the computer: Gentle, Balanced or Fast. The C# app's name.
pub const PACE_KEY: &str = "performance";
/// The output sounds play through, by name; empty for the computer's default. The C# app's name.
pub const SOUND_OUTPUT_KEY: &str = "sound_output";
/// The Settings tab last shown, by its name. The C# app's name (its tabs had other names; an unknown one opens General).
pub const TAB_KEY: &str = "settings_tab";

/// Whether a Galaxy sweep holds the pointer back to the star that's playing, so a quick sweep can't pass stars before
/// they're heard: "1" for on. Off unless set (this app's own; the C# app had no such setting).
pub const SWEEP_HOLD_KEY: &str = "galaxy_sweep_hold";

/// The C# app's settings this app reads from `library.db`, for whatever hasn't been changed here.
pub const FROM_CSHARP: &[&str] = &[
    themes::THEME_KEY,
    themes::ACCENT_KEY,
    themes::FONT_KEY,
    themes::SIZE_KEY,
    START_PAGE_KEY,
    LAST_PAGE_KEY,
    PACE_KEY,
    SOUND_OUTPUT_KEY,
    TAB_KEY,
    shortcuts::SETTING_KEY,
    crate::midi::SOUNDFONT_KEY,
];

/// The settings in use: this app's own changes, over the C# app's choices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppSettings {
    /// Changed in this app: what goes in the file. Sorted, so the file reads the same way each time.
    own: BTreeMap<String, String>,
    /// The C# app's, as read from `library.db` at the start.
    csharp: HashMap<String, String>,
}

/// What loading found that the log should hear about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    pub settings: AppSettings,
    /// Why the app's own file couldn't be used (it was put aside), or why the C# app's couldn't be read.
    pub problems: Vec<String>,
    /// Whether changes may be saved. Not when the file is there but couldn't be read and is still in its place
    /// (locked by another program, say): saving would write over your settings with only this run's changes.
    pub writable: bool,
}

impl AppSettings {
    /// From this app's file (its text, if there is one) and the C# app's choices.
    pub fn from_parts(own_json: Option<&str>, csharp: HashMap<String, String>) -> Result<AppSettings, String> {
        let own = match own_json {
            None => BTreeMap::new(),
            Some(text) if text.trim().is_empty() => BTreeMap::new(),
            Some(text) => serde_json::from_str::<BTreeMap<String, String>>(text).map_err(|e| e.to_string())?,
        };
        Ok(AppSettings { own, csharp })
    }

    /// Reads this app's file in `folder` and the C# app's choices from `library` (if it opened). Never fails: a file
    /// that can't be read is put aside and its problem told, so a bad file can't stop the app opening.
    pub fn load(folder: &Path, library: Option<&Library>) -> Loaded {
        let mut problems = Vec::new();
        let mut csharp = HashMap::new();
        if let Some(library) = library {
            for key in FROM_CSHARP {
                match library.setting(key) {
                    Ok(Some(value)) => {
                        csharp.insert((*key).to_owned(), value);
                    }
                    Ok(None) => {}
                    Err(e) => problems.push(format!("Couldn't read the C# app's {key} setting: {e}")),
                }
            }
        }
        let path = folder.join(FILE);
        // Bytes, so a file with a broken character counts as unreadable (and is put aside) rather than as missing.
        let text = match fs::read(&path) {
            Ok(bytes) => Ok(Some(String::from_utf8(bytes).map_err(|e| e.to_string()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        };
        let parsed = match text {
            Ok(None) => Ok(AppSettings { own: BTreeMap::new(), csharp: csharp.clone() }),
            Ok(Some(Ok(text))) => AppSettings::from_parts(Some(&text), csharp.clone()),
            Ok(Some(Err(why))) => Err(why),
            // There, but couldn't be opened (another program has it): left alone, and nothing is saved this run.
            Err(e) => {
                problems.push(format!(
                    "Couldn't read {} ({e}): the app opened in the C# app's settings, and won't save changes until it \
                     starts again",
                    path.display()
                ));
                return Loaded { settings: AppSettings { own: BTreeMap::new(), csharp }, problems, writable: false };
            }
        };
        match parsed {
            Ok(settings) => Loaded { settings, problems, writable: true },
            Err(why) => {
                let aside = free_name(folder);
                let kept = fs::rename(&path, &aside);
                let writable = kept.is_ok();
                problems.push(match kept {
                    Ok(()) => format!(
                        "{FILE} couldn't be read ({why}), so it was kept as {} and the app opened in the C# app's settings",
                        aside.display()
                    ),
                    Err(e) => format!(
                        "{FILE} couldn't be read ({why}) or put aside ({e}): it's left as it is, and the app won't save \
                         changes until it starts again"
                    ),
                });
                Loaded { settings: AppSettings { own: BTreeMap::new(), csharp }, problems, writable }
            }
        }
    }

    /// A setting: this app's own, else the C# app's. A value of "" counts as set (the C# app writes "" for "none").
    pub fn get(&self, key: &str) -> Option<&str> {
        self.own.get(key).or_else(|| self.csharp.get(key)).map(String::as_str)
    }

    /// Whether this app changed `key` (rather than reading the C# app's).
    pub fn changed_here(&self, key: &str) -> bool {
        self.own.contains_key(key)
    }

    /// Changes a setting. Returns whether it changed, so nothing is written for a click on what's already chosen.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        // Already so, here or in the C# app's choices (then it's read from there anyway).
        if self.get(key) == Some(value) {
            return false;
        }
        self.own.insert(key.to_owned(), value.to_owned());
        true
    }

    /// The file's text.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.own).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// Where an unreadable file is kept: `rust-settings.json.unreadable`, or `.unreadable-2`, `-3`… so one kept before
/// is never written over.
fn free_name(folder: &Path) -> PathBuf {
    let first = folder.join(format!("{FILE}.unreadable"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| folder.join(format!("{FILE}.unreadable-{n}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Writes the file's text into `folder` the safe way ([`write_checked`]). Slow (it waits for the disk): call it off the
/// window's thread.
pub fn save(folder: &Path, json: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(folder)?;
    let path = folder.join(FILE);
    write_checked(&path, json)?;
    Ok(path)
}

/// Writes `text` to `path` the safe way (copy, check, then rename): a `.mo-part` file beside it
/// (`<name>.mo-part`), flushed to the disk and read back to check it, then renamed over the old file, so a crash part
/// way leaves the old one whole. The part file never stays behind, but for a crash part way: the start-up sweep
/// (rule 28) clears that one, by its whole name, so each file written this way is on its list
/// (`crate::tidy::is_cut_short`). The app's settings and the Space page's list of what it put in the Recycle Bin
/// ([`crate::binned`]) are written this way. Waits for the disk: call it off the window's thread.
///
/// Its file's name must be on [`WRITTEN_CHECKED`] (rule 28).
pub fn write_checked(path: &Path, text: &str) -> io::Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(crate::backup::PART_ENDING);
    let part = PathBuf::from(part);
    let written = (|| {
        let mut f = fs::File::create(&part)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        if fs::read(&part)? != text.as_bytes() {
            return Err(io::Error::other("the copy written didn't read back the same"));
        }
        fs::rename(&part, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&part);
    }
    written
}

/// The place the app opens on, from the settings.
pub fn start_place(settings: &AppSettings) -> places::Place {
    places::Place::start_place(settings.get(START_PAGE_KEY), settings.get(LAST_PAGE_KEY))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csharp(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn this_apps_changes_win_over_the_csharp_apps() {
        let s = AppSettings::from_parts(
            Some(r#"{"appearance_theme":"Paper"}"#),
            csharp(&[("appearance_theme", "Retro"), ("start_page", "Sound Lab")]),
        )
        .unwrap();
        assert_eq!(s.get("appearance_theme"), Some("Paper"));
        assert_eq!(s.get("start_page"), Some("Sound Lab"));
        assert_eq!(s.get("performance"), None);
        assert!(s.changed_here("appearance_theme"));
        assert!(!s.changed_here("start_page"));
    }

    #[test]
    fn a_change_is_kept_only_when_it_changes_something() {
        let mut s = AppSettings::from_parts(None, csharp(&[("performance", "Gentle")])).unwrap();
        assert!(!s.set("performance", "Gentle"), "the same as the C# app's: nothing to write");
        assert!(s.set("performance", "Fast"));
        assert!(!s.set("performance", "Fast"));
        // Back to the C# app's value is still a change of this app's, so it's kept (the C# value may change later).
        assert!(s.set("performance", "Gentle"));
        assert_eq!(s.get("performance"), Some("Gentle"));
        assert_eq!(s.to_json(), "{\n  \"performance\": \"Gentle\"\n}");
    }

    #[test]
    fn an_empty_or_missing_file_means_no_changes() {
        assert_eq!(AppSettings::from_parts(None, HashMap::new()).unwrap(), AppSettings::default());
        assert_eq!(AppSettings::from_parts(Some("  \n"), HashMap::new()).unwrap(), AppSettings::default());
        assert!(AppSettings::from_parts(Some("{not json"), HashMap::new()).is_err());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-app-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saving_and_loading_round_trips_and_leaves_no_part_file() {
        let dir = scratch("round");
        let mut s = AppSettings::default();
        s.set("start_page", "Sounds");
        s.set("sound_output", "");
        save(&dir, &s.to_json()).unwrap();
        assert!(!dir.join(format!("{FILE}.mo-part")).exists());
        let loaded = AppSettings::load(&dir, None);
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert!(loaded.writable);
        assert_eq!(loaded.settings.get("start_page"), Some("Sounds"));
        assert_eq!(loaded.settings.get("sound_output"), Some(""));
        assert_eq!(start_place(&loaded.settings), places::Place::Sounds);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_file_is_put_aside_not_lost() {
        let dir = scratch("bad");
        fs::write(dir.join(FILE), "{broken").unwrap();
        let loaded = AppSettings::load(&dir, None);
        assert_eq!(loaded.settings, AppSettings::default());
        assert_eq!(loaded.problems.len(), 1);
        assert!(!dir.join(FILE).exists());
        assert_eq!(fs::read_to_string(dir.join(format!("{FILE}.unreadable"))).unwrap(), "{broken");
        assert!(loaded.writable, "put aside, so saving can't lose it");
        // A second bad file is kept beside the first, never over it.
        fs::write(dir.join(FILE), [0xFF, 0xFE]).unwrap();
        let again = AppSettings::load(&dir, None);
        assert_eq!(again.problems.len(), 1);
        assert_eq!(fs::read_to_string(dir.join(format!("{FILE}.unreadable"))).unwrap(), "{broken");
        assert_eq!(fs::read(dir.join(format!("{FILE}.unreadable-2"))).unwrap(), vec![0xFF, 0xFE]);
        let _ = fs::remove_dir_all(&dir);
    }
}
