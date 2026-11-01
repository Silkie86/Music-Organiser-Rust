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

/// The setting naming the export folder (the C# app's key).
pub const EXPORT_FOLDER_KEY: &str = "export_folder";
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

/// Where converts go: the folder picked in Settings, or "Music Organiser Exports" in Documents in the home folder
/// (the C# app asks Windows where Documents is, so a Documents moved to OneDrive differs: pick the folder in Settings
/// there). None when there's neither a folder picked nor a home folder, rather than writing wherever the app started.
pub fn export_root(setting: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    match setting.map(str::trim).filter(|s| !s.is_empty()) {
        Some(folder) => Some(PathBuf::from(folder)),
        None => home.map(|h| h.join("Documents").join("Music Organiser Exports")),
    }
}

/// The user's home folder, where Documents is.
pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
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
        let home = Path::new("home");
        let documents = home.join("Documents").join("Music Organiser Exports");
        assert_eq!(export_root(Some("  "), Some(home)), Some(documents.clone()));
        assert_eq!(export_root(None, Some(home)), Some(documents));
        assert_eq!(export_root(None, None), None);
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
