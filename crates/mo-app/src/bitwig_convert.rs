//! "Open in Bitwig" for an FL Studio song: converts it into the export folder and says what came of it (the C# app's
//! `ProjectDetailViewModel.OpenInBitwig`). The screen puts up the button and the card, calls [`convert`] on the
//! low-priority pool (it reads the template, the song and its samples, and writes the new project), then opens
//! [`Converted::file`] with [`crate::shell`] in Bitwig Studio.
//!
//! Safety: the FL project and the template are only read; the new project goes in the export folder, never over a copy
//! that may hold work (see `mo_convert::save`). Settings are only read here: while the Rust app opens `library.db`
//! read-only, the stamp that lets a later convert replace this one ([`Converted::written`]) isn't kept, so an earlier
//! convert is always kept and the new one gets the next name ("Name (2)").

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mo_convert::Options;
use mo_convert::save::{Saved, bitwig_file, bitwig_from_fl, bitwig_name, can_save_over};

/// The setting naming the export folder (the C# app's key), set on Settings › Folders
/// ([`mo_library::folders::set_export_folder`]).
pub const EXPORT_FOLDER_KEY: &str = mo_library::folders::EXPORT_FOLDER;
/// The setting naming the Bitwig project picked as the template (the C# app's key).
pub const BITWIG_TEMPLATE_KEY: &str = "bitwig_template";
/// The setting that's "1" when FL's steps on a Sampler play until the next note or the clip's end (the C# app's key).
pub const STEPS_UNTIL_NEXT_NOTE_KEY: &str = "bitwig_steps_until_next_note";
/// The template's file name in the app's own folder.
pub const TEMPLATE_FILE: &str = "Music Organiser template.bwproject";

/// The setting that remembers a converted copy (the C# app's key), so a later convert may replace it.
pub fn convert_key(file: &Path) -> String {
    format!("bitwig_convert:{}", file.display())
}

/// Where every export goes (Bitwig converts, kits, MIDI parts): the export folder setting, or "Music Organiser
/// Exports" in `documents` (the Documents folder [`documents`] finds). None when there's neither, rather than writing
/// wherever the app started. Only works the path out: [`export_folder`] checks it before anything is written.
pub fn export_root(setting: Option<&str>, documents: Option<&Path>) -> Option<PathBuf> {
    match setting.map(str::trim).filter(|s| !s.is_empty()) {
        Some(folder) => Some(PathBuf::from(folder)),
        None => documents.map(|d| d.join("Music Organiser Exports")),
    }
}

/// Where an export writes ([`export_root`]), checked first. An export folder that's set but isn't there stops the
/// export, saying so, rather than being made again or the export going somewhere else (Solomon, 2026-11-05: it's most
/// likely on a drive that isn't plugged in, and C: is nearly full). The one in Documents is made when it's needed.
/// Looks at the disk, so never on the window's thread.
pub fn export_folder(setting: Option<&str>, documents: Option<&Path>) -> Result<PathBuf, String> {
    // Without Documents, export_root gives only a folder that's set.
    let Some(set) = export_root(setting, None) else {
        return export_root(None, documents).ok_or_else(|| NO_EXPORT_FOLDER.to_owned());
    };
    if !set.is_absolute() {
        // It would be looked for from wherever the app started.
        return Err(format!(
            "The export folder {} isn't a full path, so the app can't tell where it is.",
            set.display()
        ));
    }
    match std::fs::metadata(&set) {
        Ok(m) if m.is_dir() => Ok(set),
        Ok(_) => Err(format!("The export folder {} is a file, not a folder.", set.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound || drive_not_ready(&e) => Err(not_there(&set)),
        Err(e) => Err(format!("The export folder {} can't be opened: {e}.", set.display())),
    }
}

/// Windows' word for a drive with nothing in it (a card reader with no card in, say), or one that isn't ready yet.
fn drive_not_ready(e: &std::io::Error) -> bool {
    const ERROR_NOT_READY: i32 = 21;
    cfg!(windows) && e.raw_os_error() == Some(ERROR_NOT_READY)
}

/// What an export says when the export folder that's set isn't there. On Windows its drive tells which: when that
/// isn't there either, it's unplugged (or a card reader has no card in); else the folder has gone from it. Elsewhere a
/// drive shows as a folder too, so both are said. Looks at the disk.
fn not_there(set: &Path) -> String {
    let folder = set.display();
    if !cfg!(windows) {
        return format!(
            "The export folder {folder} isn't there. Plug its drive in, or make the folder again, then try again."
        );
    }
    if set.ancestors().last().is_some_and(|drive| std::fs::metadata(drive).is_ok()) {
        format!(
            "The export folder {folder} isn't there any more, though its drive is. Make the folder again, then try \
             again."
        )
    } else {
        format!("The export folder {folder} isn't there. Plug its drive in, then try again.")
    }
}

/// [`export_folder`], with the setting read from the song library at `library` (none is set without one). A setting
/// that can't be read stops the export too, rather than it going to Documents in place of the folder set. Opens the
/// library and looks at the disk, so never on the window's thread.
pub fn export_folder_in(library: &Path) -> Result<PathBuf, String> {
    let setting = if library.exists() {
        mo_library::Library::open_read_only(library)
            .and_then(|lib| lib.setting(EXPORT_FOLDER_KEY))
            .map_err(|e| format!("The export folder setting couldn't be read. {e}."))?
    } else {
        None
    };
    export_folder(setting.as_deref(), documents().as_deref())
}

/// Why nothing can be exported with no export folder set and no Documents folder to make one in.
pub const NO_EXPORT_FOLDER: &str = "There's no export folder, and no Documents folder to make one in.";

/// The Documents folder: on Windows where Windows really keeps it (it may have moved, to OneDrive say), elsewhere the
/// one in the home folder. None without a home folder. Asking Windows can wait on the disk, so never on the window's
/// thread.
pub fn documents() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(d) = known_folders::get_known_folder_path(known_folders::KnownFolder::Documents) {
        return Some(d);
    }
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| PathBuf::from(h).join("Documents"))
}

/// The template to convert with: the app's own copy (`<data>/Bitwig template/Music Organiser template.bwproject`),
/// unless the project picked in Settings (`picked`) was saved after it, as the C# app's `BitwigTemplateStore.Find`
/// picks; None when there's neither. Reads the disk; copies nothing.
pub fn template_path(data_dir: &Path, picked: Option<&str>) -> Option<PathBuf> {
    let mine = data_dir.join("Bitwig template").join(TEMPLATE_FILE);
    let picked = picked.map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from).filter(|p| p.is_file());
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (mine.is_file(), picked) {
        (true, Some(p)) if modified(&p) > modified(&mine) => Some(p),
        (true, _) => Some(mine),
        (false, p) => p,
    }
}

/// What the screen says when there's no template.
pub const NO_TEMPLATE: &str = "Converting needs a Bitwig template: a project saved in Bitwig with a Sampler track, a \
     VST3 synth with a Note Transpose before it and a clip of notes, and a VST2 synth (and an audio track with an audio \
     clip on it, for FL Studio's audio clips). Pick it in Settings or try again.";

/// A song converted for Bitwig.
#[derive(Debug)]
pub struct Converted {
    /// The new Bitwig project, to open.
    pub file: PathBuf,
    /// The name it was saved under.
    pub name: String,
    /// What didn't come across (also saved beside it).
    pub notes: Vec<String>,
    /// What to remember under [`convert_key`] once the library can be written, so a later convert may replace it.
    pub written: String,
    /// What the screen says, in the C# app's words.
    pub message: String,
}

/// Converts `project` (an FL Studio save) for Bitwig into `export_root` with the template at `template`.
/// `opened` are the names converted and opened in Bitwig this session (Bitwig only brings an open one forward, so a
/// new convert takes the next name), and `written_of` what was remembered about a copy (see [`convert_key`]).
/// `find_sample` finds a sample that isn't where FL saved it (rule 13). Reads and writes the disk: never on the
/// window's thread.
pub fn convert(
    project: &Path,
    export_root: &Path,
    template: &Path,
    steps_until_next_note: bool,
    opened: &HashSet<String>,
    written_of: &dyn Fn(&Path) -> Option<String>,
    find_sample: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<Converted, String> {
    let template_bytes = std::fs::read(template).map_err(|e| format!("Couldn't read the Bitwig template: {e}"))?;
    let kept = |n: &str| {
        let file = bitwig_file(project, export_root, n);
        !can_save_over(project, &file, written_of(&file).as_deref())
    };
    let plain = bitwig_name(project, |_| false);
    let name = bitwig_name(project, |n| opened.iter().any(|o| o.eq_ignore_ascii_case(n)) || kept(n));
    let renamed_kept = name != plain && kept(&plain);
    let file = bitwig_file(project, export_root, &name);
    let written_before = written_of(&file);
    let options = Options { steps_until_next_note, ..Options::default() };
    let Saved { file, notes, taken, found, written } =
        bitwig_from_fl(project, export_root, &template_bytes, &name, written_before.as_deref(), options, find_sample)?;
    let mut message = message(&name, &plain, renamed_kept, &notes);
    if !found.is_empty() {
        message +=
            &format!(" {} not where FL Studio saved them were found somewhere else and used.", count(found.len()));
    }
    if !taken.is_empty() {
        message += &format!(" {} this save lost were taken from an older save.", sounds(taken.len()));
    }
    Ok(Converted { file, name, notes, written, message })
}

fn count(n: usize) -> String {
    if n == 1 { "1 sample".to_owned() } else { format!("{n} samples") }
}

fn sounds(n: usize) -> String {
    if n == 1 { "1 plugin sound".to_owned() } else { format!("{n} plugin sounds") }
}

/// What the screen says once a song is converted: its name, why it has a new one, and what didn't come across (the
/// first four; the rest are in the notes saved beside it).
pub fn message(name: &str, plain: &str, renamed_kept: bool, notes: &[String]) -> String {
    let renamed = if name == plain {
        ""
    } else if renamed_kept {
        " It has a new name so the earlier convert isn't saved over: it may have been changed in Bitwig, or be another \
         song's with the same name."
    } else {
        " It has a new name because Bitwig may still have an earlier convert open, and would show that one instead. \
         Close the old one without saving."
    };
    let left = match notes.len() {
        0 => String::new(),
        n => {
            let first: Vec<&str> = notes.iter().take(4).map(String::as_str).collect();
            let more = if n > 4 {
                format!("; and {} more, listed in the notes saved next to it", n - 4)
            } else {
                String::new()
            };
            format!(" Not carried over: {}{more}.", first.join("; "))
        }
    };
    format!("Opening {name} in Bitwig.{renamed}{left}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_go_where_settings_say_or_in_documents() {
        assert_eq!(export_root(Some(r"D:\Exports"), None), Some(PathBuf::from(r"D:\Exports")));
        let documents = Path::new("Documents");
        let exports = documents.join("Music Organiser Exports");
        assert_eq!(export_root(Some("  "), Some(documents)), Some(exports.clone()));
        assert_eq!(export_root(None, Some(documents)), Some(exports));
        assert_eq!(export_root(None, None), None);
    }

    #[test]
    fn an_export_folder_that_isnt_there_stops_the_export() {
        let dir = std::env::temp_dir().join(format!("mo-app-export-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(export_folder(dir.to_str(), None), Ok(dir.clone()));
        // Gone from a drive that's there: the export stops, and the folder isn't made again.
        let gone = dir.join("removed");
        let why = export_folder(gone.to_str(), Some(&dir)).unwrap_err();
        let says = if cfg!(windows) {
            "isn't there any more, though its drive is. Make the folder again, then try again."
        } else {
            "isn't there. Plug its drive in, or make the folder again, then try again."
        };
        assert_eq!(why, format!("The export folder {} {says}", gone.display()));
        assert!(!gone.exists());
        // Windows: on a drive that isn't there (unplugged, say), it says to plug it in.
        #[cfg(windows)]
        if let Some(drive) = ('D'..='Z').rev().map(|l| PathBuf::from(format!("{l}:\\"))).find(|d| !d.exists()) {
            let unplugged = drive.join("Exports");
            assert_eq!(
                export_folder(unplugged.to_str(), Some(&dir)),
                Err(format!(
                    "The export folder {} isn't there. Plug its drive in, then try again.",
                    unplugged.display()
                ))
            );
        }
        // None set: the one in Documents, made when it's needed.
        assert_eq!(export_folder(Some(" "), Some(&dir)), Ok(dir.join("Music Organiser Exports")));
        assert_eq!(export_folder(None, None), Err(NO_EXPORT_FOLDER.to_owned()));
        // A file where the folder should be, and a path that isn't a full one: each says what's wrong.
        let file = dir.join("a file");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(
            export_folder(file.to_str(), Some(&dir)),
            Err(format!("The export folder {} is a file, not a folder.", file.display()))
        );
        assert_eq!(
            export_folder(Some("Exports"), Some(&dir)),
            Err("The export folder Exports isn't a full path, so the app can't tell where it is.".to_owned())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_export_folder_setting_is_read_from_the_song_library() {
        let dir = std::env::temp_dir().join(format!("mo-app-export-setting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A song library with the export folder setting, or none.
        let library = |name: &str, folder: Option<&Path>| {
            let path = dir.join(name);
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE projects (id INTEGER PRIMARY KEY); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
            )
            .unwrap();
            if let Some(folder) = folder {
                let value = folder.to_string_lossy().into_owned();
                c.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                    rusqlite::params![EXPORT_FOLDER_KEY, value],
                )
                .unwrap();
            }
            path
        };
        // None set, or no song library: the one in Documents.
        let in_documents = export_folder(None, documents().as_deref());
        assert_eq!(export_folder_in(&library("none set.db", None)), in_documents);
        assert_eq!(export_folder_in(&dir.join("no library.db")), in_documents);
        // The one set; one set that isn't there stops the export.
        assert_eq!(export_folder_in(&library("set.db", Some(&dir))), Ok(dir.clone()));
        let gone = dir.join("unplugged");
        assert!(export_folder_in(&library("gone.db", Some(&gone))).is_err_and(|why| why.contains("isn't there")));
        // A library that can't be read stops it too, rather than the export going to Documents.
        let damaged = dir.join("damaged.db");
        std::fs::write(&damaged, b"not a database").unwrap();
        let why = export_folder_in(&damaged).unwrap_err();
        assert!(why.starts_with("The export folder setting couldn't be read."), "{why}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_message_says_why_a_convert_has_a_new_name_and_what_was_left_out() {
        assert_eq!(message("sly", "sly", false, &[]), "Opening sly in Bitwig.");
        let notes: Vec<String> = (1..=6).map(|n| format!("note {n}")).collect();
        let m = message("sly (2)", "sly", false, &notes);
        assert!(m.starts_with("Opening sly (2) in Bitwig. It has a new name because Bitwig may still have"));
        assert!(m.ends_with(
            "Not carried over: note 1; note 2; note 3; note 4; and 2 more, listed in the notes saved next to it."
        ));
        assert!(message("sly (2)", "sly", true, &[]).contains("isn't saved over"));
    }

    #[test]
    fn the_template_is_the_apps_copy_unless_the_picked_one_is_newer() {
        let dir = std::env::temp_dir().join(format!("mo-app-template-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("Bitwig template");
        std::fs::create_dir_all(&folder).unwrap();
        let picked = dir.join("picked.bwproject");
        assert_eq!(template_path(&dir, None), None);
        std::fs::write(&picked, b"x").unwrap();
        assert_eq!(template_path(&dir, Some(picked.to_str().unwrap())), Some(picked.clone()));
        let mine = folder.join(TEMPLATE_FILE);
        std::fs::write(&mine, b"x").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&picked).unwrap().set_modified(old).unwrap();
        assert_eq!(template_path(&dir, Some(picked.to_str().unwrap())), Some(mine.clone()));
        assert_eq!(template_path(&dir, Some("missing.bwproject")), Some(mine.clone()));
        // Saved after the app's copy: the picked one.
        let new = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&picked).unwrap().set_modified(new).unwrap();
        assert_eq!(template_path(&dir, Some(picked.to_str().unwrap())), Some(picked));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_template_that_cant_be_used_says_so_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("mo-app-convert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let template = dir.join("template.bwproject");
        std::fs::write(&template, b"not a project").unwrap();
        let exports = dir.join("exports");
        let r = convert(&dir.join("sly.flp"), &exports, &template, false, &HashSet::new(), &|_| None, &|_| None);
        assert!(r.is_err_and(|e| e.contains("template")));
        assert!(!exports.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
