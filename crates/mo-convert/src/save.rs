//! Converting an FL project to a Bitwig project on disk (the C# `Exporter.BitwigFromFl`): read the song, plan it,
//! write it into a copy of the template, and save it as a new file in the export folder, laid out as Bitwig expects
//! (`<export folder>/<song>/Bitwig/<name>/<name>.bwproject`), with what didn't come across beside it.
//!
//! Safety (rules 3 and "copy, check, then rename"): the FL project and the template are only read. The new project is
//! written beside where it goes under a temporary name, read back and decoded to check it, and only then renamed into
//! place. An earlier convert is only replaced when it's this song's own and nothing has changed it since
//! ([`can_save_over`]); one saved in Bitwig, or another song's with the same name, is kept and the caller picks the next
//! name ([`bitwig_name`]).

use std::path::{Path, PathBuf};

use mo_formats::fl::{events, song};
use mo_formats::paths;

use crate::bitwig;
use crate::plan::{Options, from_fl};
use crate::sample::read_sample;

/// The longest file name the app makes (as the C# app's).
const LONGEST_FILE_NAME: usize = 120;

/// A converted project on disk.
#[derive(Debug)]
pub struct Saved {
    pub file: PathBuf,
    /// What couldn't come across (also saved beside it).
    pub notes: Vec<String>,
    /// What to remember about it, so a later convert can tell whether it may replace it ([`can_save_over`]).
    pub written: String,
}

/// A name that's safe as a file name on every system: no characters Windows refuses, no spaces or dots at the ends, at
/// most 120 characters, and "untitled" when nothing's left (the C# `TextScan.SafeFileName`).
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c < ' ' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches([' ', '.']);
    // C# counts UTF-16 units; whole characters are kept here, so a name is never cut inside one.
    let mut out = String::new();
    for c in cleaned.chars() {
        if out.encode_utf16().count() + c.len_utf16() > LONGEST_FILE_NAME {
            break;
        }
        out.push(c);
    }
    if out.is_empty() { "untitled".to_owned() } else { out }
}

/// Where a project's Bitwig copy named `name` is saved.
pub fn bitwig_file(project: &Path, export_root: &Path, name: &str) -> PathBuf {
    let name = safe_file_name(name);
    let song = safe_file_name(paths::stem(&project.to_string_lossy()));
    export_root.join(song).join("Bitwig").join(&name).join(format!("{name}.bwproject"))
}

/// The name a project's Bitwig copy is saved under: the project's own, or "Name (2)", "Name (3)" and so on while that
/// name is `taken` (Bitwig may have a copy with that name open, which opening again only brings forward, or a copy with
/// it can't be saved over).
pub fn bitwig_name(project: &Path, taken: impl Fn(&str) -> bool) -> String {
    let name = safe_file_name(paths::stem(&project.to_string_lossy()));
    if !taken(&name) {
        return name;
    }
    (2..1000).map(|n| format!("{name} ({n})")).find(|n| !taken(n)).unwrap_or(name)
}

/// What to remember about a Bitwig copy just saved: when it was saved and which FL project it's from.
pub fn written(project: &Path, file: &Path) -> std::io::Result<String> {
    let modified = std::fs::metadata(file)?.modified()?;
    let nanos = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let full = std::path::absolute(project)?;
    Ok(format!("{nanos} {}", full.display()))
}

/// Whether a new convert of `project` can be saved as `file`: nothing is there, or it's this project's earlier convert
/// (`written`, from [`written`]) and nothing has changed it since. A copy saved in Bitwig, one made before the app
/// remembered them, or another song's with the same name may hold work, so it's kept.
pub fn can_save_over(project: &Path, file: &Path, written_before: Option<&str>) -> bool {
    if !file.exists() {
        return true;
    }
    written_before.is_some_and(|w| written(project, file).is_ok_and(|now| now == w))
}

/// Converts an FL project into the export folder as `name` (see [`bitwig_name`]), from `template` (a project Bitwig
/// saved). `written_before` is what [`Saved::written`] said about this name's earlier convert, if any.
pub fn bitwig_from_fl(
    project: &Path,
    export_root: &Path,
    template: &[u8],
    name: &str,
    written_before: Option<&str>,
    options: Options,
) -> Result<Saved, String> {
    let file = bitwig_file(project, export_root, name);
    if !can_save_over(project, &file, written_before) {
        return Err(format!("{} is already there and may hold work, so it's kept", file.display()));
    }
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read {}: {e}", project.display()))?;
    let flp = events::load(&raw).map_err(|_| format!("{} is empty or damaged", project.display()))?;
    let mut song = song::read_song(&flp);
    song.resolve_samples(project);
    let plan = from_fl(&song, options, &mut |p| read_sample(Path::new(p)));
    let converted = bitwig::write(&plan, template)?;

    let dir = file.parent().ok_or("no folder to save in")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    save_checked(&file, &converted.bytes)?;

    let name = safe_file_name(name);
    let notes_file = dir.join(format!("{name} - not carried over.txt"));
    if converted.notes.is_empty() {
        // This convert's own folder: an older list would be wrong now.
        if notes_file.exists() {
            std::fs::remove_file(&notes_file).map_err(|e| format!("Couldn't update {}: {e}", notes_file.display()))?;
        }
    } else {
        let mut text = format!("What didn't come across from FL Studio to Bitwig for {name}\r\n\r\n");
        for n in &converted.notes {
            text.push_str(&format!("  {n}\r\n"));
        }
        save_checked(&notes_file, text.as_bytes())?;
    }
    let written = written(project, &file).map_err(|e| e.to_string())?;
    Ok(Saved { file, notes: converted.notes, written })
}

/// Writes `bytes` under a temporary name beside `file`, reads it back to check it, then renames it into place.
fn save_checked(file: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = file.file_name().ok_or("no file name")?.to_string_lossy();
    let temporary = file.with_file_name(format!("{name}.{}.saving", std::process::id()));
    let result = (|| {
        std::fs::write(&temporary, bytes).map_err(|e| format!("Couldn't save {}: {e}", file.display()))?;
        let back = std::fs::read(&temporary).map_err(|e| format!("Couldn't check {}: {e}", file.display()))?;
        if back != bytes {
            return Err(format!("{} didn't save as written, so it's left as it was", file.display()));
        }
        std::fs::rename(&temporary, file).map_err(|e| format!("Couldn't save {}: {e}", file.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_and_numbered_while_taken() {
        assert_eq!(safe_file_name(" a:b/c. "), "a_b_c");
        assert_eq!(safe_file_name("..."), "untitled");
        assert_eq!(safe_file_name(&"é".repeat(200)).chars().count(), 120);
        let project = Path::new("songs/civil_18.flp");
        assert_eq!(bitwig_name(project, |_| false), "civil_18");
        assert_eq!(bitwig_name(project, |n| n == "civil_18"), "civil_18 (2)");
        assert_eq!(bitwig_name(project, |n| n == "civil_18" || n == "civil_18 (2)"), "civil_18 (3)");
        // Only names that are taken count: a later number that's free doesn't move the first.
        assert_eq!(bitwig_name(project, |n| n == "civil_18 (2)"), "civil_18");
        assert_eq!(
            bitwig_file(project, Path::new("Exports"), "civil_18 (2)"),
            Path::new("Exports/civil_18/Bitwig/civil_18 (2)/civil_18 (2).bwproject")
        );
    }
}
