//! Converting an FL project to a Bitwig project on disk (the C# `Exporter.BitwigFromFl`): read the song, plan it,
//! write it into a copy of the template, and save it as a new file in the export folder, laid out as Bitwig expects
//! (`<export folder>/<song>/Bitwig/<name>/<name>.bwproject`), with what didn't come across beside it.
//!
//! Safety (rules 3 and "copy, check, then rename"): the FL project and the template are only read. The new project is
//! written beside where it goes under a temporary name, flushed to the disk, read back and decoded to check it, and
//! only then renamed into place. An earlier convert is only replaced when it's this song's own and nothing has changed
//! it since ([`can_save_over`], asked again just before the rename); one saved in Bitwig, or another song's with the
//! same name, is kept and the caller picks the next name ([`bitwig_name`]).
//!
//! A sample that can't be read where FL saved it is looked for (rule 13) by the finder the caller passes in (for the
//! app, mo-library's `SampleLocator`: the project's folders, then the DAW's), and the Bitwig project points at where
//! it was found. The convert page, when it's built, passes the locator.
//!
//! Not here yet (a later piece): plugin sounds a save lost, taken from older saves (the C# `LostSounds`).

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mo_formats::fl::{events, song};
use mo_formats::paths;
use rayon::prelude::*;

use crate::bitwig;
use crate::plan::{Options, SampleFile, from_fl};
use crate::sample::read_sample;

/// The longest file name the app makes (as the C# app's).
const LONGEST_FILE_NAME: usize = 120;

/// .NET's ticks (100 ns) at 1970-01-01, so [`written`] reads the same as the C# app's in the shared settings.
const TICKS_AT_1970: u128 = 621_355_968_000_000_000;

/// A converted project on disk.
#[derive(Debug)]
pub struct Saved {
    pub file: PathBuf,
    /// What couldn't come across (also saved beside it).
    pub notes: Vec<String>,
    /// Samples that weren't where FL saved them, and where they were found and used (also saved beside it).
    pub found: Vec<String>,
    /// What to remember about it, so a later convert can tell whether it may replace it ([`can_save_over`]).
    pub written: String,
}

/// A name that's safe as a file name on every system: no characters Windows refuses, no spaces or dots at the ends, at
/// most 120 characters, not one of Windows' device names, and "untitled" when nothing's left (the C#
/// `TextScan.SafeFileName`).
pub fn safe_file_name(name: &str) -> String {
    cut(name, LONGEST_FILE_NAME)
}

/// [`safe_file_name`], at most `longest` UTF-16 units long (as C# counts), cut between whole characters.
fn cut(name: &str, longest: usize) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c < ' ' { '_' } else { c })
        .collect();
    let mut out = String::new();
    for c in cleaned.trim_matches([' ', '.']).chars() {
        if out.encode_utf16().count() + c.len_utf16() > longest {
            break;
        }
        out.push(c);
    }
    // Cutting can leave a space or dot at the end.
    let mut out = out.trim_end_matches([' ', '.']).to_owned();
    if out.is_empty() {
        return "untitled".to_owned();
    }
    let stem = out.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit() && c != '0'));
    if device {
        out.insert(0, '_');
    }
    out
}

/// Where a project's Bitwig copy named `name` is saved.
pub fn bitwig_file(project: &Path, export_root: &Path, name: &str) -> PathBuf {
    let name = safe_file_name(name);
    let song = safe_file_name(paths::stem(&project.to_string_lossy()));
    export_root.join(song).join("Bitwig").join(&name).join(format!("{name}.bwproject"))
}

/// The name a project's Bitwig copy is saved under: the project's own, or "Name (2)", "Name (3)" and so on while that
/// name is `taken` (Bitwig may have a copy with that name open, which opening again only brings forward, or a copy with
/// it can't be saved over). A long name is shortened to leave room for the number, so each number is its own file.
pub fn bitwig_name(project: &Path, taken: impl Fn(&str) -> bool) -> String {
    let stem = paths::stem(&project.to_string_lossy()).to_owned();
    let name = safe_file_name(&stem);
    if !taken(&name) {
        return name;
    }
    (2..1000)
        .map(|n| {
            let number = format!(" ({n})");
            format!("{}{number}", cut(&stem, LONGEST_FILE_NAME - number.len()))
        })
        .find(|n| !taken(n))
        .unwrap_or(name)
}

/// What to remember about a Bitwig copy just saved: when it was saved and which FL project it's from, written as the
/// C# app writes it (.NET ticks), since both apps keep it in the same settings.
pub fn written(project: &Path, file: &Path) -> std::io::Result<String> {
    let modified = std::fs::metadata(file)?.modified()?;
    let ticks = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) + TICKS_AT_1970;
    let full = std::path::absolute(project)?;
    Ok(format!("{ticks} {}", full.display()))
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
/// saved). `written_before` is what [`Saved::written`] said about this name's earlier convert, if any. `find_sample`
/// says where a sample that isn't where FL saved it is now, if anywhere. The samples are looked for and read on every
/// core, so the app calls this on its low-priority pool, never on the window's thread.
pub fn bitwig_from_fl(
    project: &Path,
    export_root: &Path,
    template: &[u8],
    name: &str,
    written_before: Option<&str>,
    options: Options,
    find_sample: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<Saved, String> {
    let file = bitwig_file(project, export_root, name);
    let kept = || format!("{} is already there and may hold work, so it's kept", file.display());
    if !can_save_over(project, &file, written_before) {
        return Err(kept());
    }
    // A template that can't be used is said before any sample is read.
    if let Some(problem) = bitwig::template_problem(template) {
        return Err(problem);
    }
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read {}: {e}", project.display()))?;
    let flp = events::load(&raw).map_err(|_| format!("{} is empty or damaged", project.display()))?;
    let mut song = song::read_song(&flp);
    song.resolve_samples(project);
    // Each sample read once, all at the same time (rule 21). One that can't be read where FL saved it is looked for
    // (rule 13), and used where it's found if it reads there. FL's own content ("%FLStudioFactoryData%…") is
    // FL's to find, never another file with its name.
    let wanted: BTreeSet<&str> = song.channels.iter().map(|c| c.sample.as_str()).filter(|p| !p.is_empty()).collect();
    let read: HashMap<String, (Option<String>, Option<SampleFile>)> = wanted
        .into_par_iter()
        .map(|p| {
            let here = read_sample(Path::new(p));
            let moved = || {
                let f = find_sample(p).filter(|f| f != p)?;
                read_sample(Path::new(&f)).map(|s| (Some(f), Some(s)))
            };
            let got = match here {
                Some(s) => (None, Some(s)),
                None if p.starts_with('%') => (None, None),
                None => moved().unwrap_or((None, None)),
            };
            (p.to_owned(), got)
        })
        .collect();
    let mut found: Vec<String> = Vec::new();
    for c in &mut song.channels {
        if let Some((Some(f), _)) = read.get(&c.sample) {
            found.push(format!("{}: {} wasn't there, so {f} is used", c.name, c.sample));
            c.sample = f.clone();
        }
    }
    let samples: HashMap<String, Option<SampleFile>> =
        read.into_iter().map(|(p, (f, s))| (f.unwrap_or(p), s)).collect();
    let plan = from_fl(&song, options, &mut |p| match samples.get(p) {
        Some(s) => s.clone(),
        None => read_sample(Path::new(p)),
    });
    let mut converted = bitwig::write(&plan, template)?;
    // A damaged save, or one too big to read whole, is converted as far as it goes, and that's said first (rule 9).
    for warning in [song.warning, flp.warning].into_iter().flatten() {
        converted.notes.insert(0, format!("The FL project: {warning}"));
    }

    let dir = file.parent().ok_or("no folder to save in")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    let name = safe_file_name(name);
    let notes_file = dir.join(format!("{name} - not carried over.txt"));
    // The notes go first, so a failure there leaves the earlier convert as it was.
    let has_notes = !converted.notes.is_empty() || !found.is_empty();
    if has_notes {
        let mut text = String::new();
        if !converted.notes.is_empty() {
            text.push_str(&format!("What didn't come across from FL Studio to Bitwig for {name}\r\n\r\n"));
            for n in &converted.notes {
                text.push_str(&format!("  {n}\r\n"));
            }
        }
        if !found.is_empty() {
            if !text.is_empty() {
                text.push_str("\r\n");
            }
            text.push_str("Samples found somewhere else, and used from there\r\n\r\n");
            for n in &found {
                text.push_str(&format!("  {n}\r\n"));
            }
        }
        save_checked(&notes_file, text.as_bytes(), |_| None)?;
    }
    // Reading the samples can take minutes: the earlier convert may have been saved in Bitwig meanwhile.
    if !can_save_over(project, &file, written_before) {
        return Err(kept());
    }
    save_checked(&file, &converted.bytes, |back| match mo_formats::bitwig::decode(back) {
        Ok(doc) => doc.body_error,
        Err(_) => Some("not a Bitwig project".to_owned()),
    })?;
    let written = written(project, &file).map_err(|e| e.to_string())?;
    // This convert's own folder: an older list would be wrong now. The project is saved, so a list that can't be
    // removed is only left behind.
    if !has_notes && notes_file.exists() {
        let _ = std::fs::remove_file(&notes_file);
    }
    Ok(Saved { file, notes: converted.notes, found, written })
}

/// Writes `bytes` under a temporary name beside `file`, flushes it to the disk, reads it back and checks it (the same
/// bytes, and `problem` finds nothing wrong), then renames it into place.
fn save_checked(file: &Path, bytes: &[u8], problem: impl Fn(&[u8]) -> Option<String>) -> Result<(), String> {
    // Unique to this save, even with two converts running at once.
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let name = file.file_name().ok_or("no file name")?.to_string_lossy();
    let count = SAVES.fetch_add(1, Ordering::Relaxed);
    let temporary = file.with_file_name(format!("{name}.{}-{count}.saving", std::process::id()));
    let result = (|| {
        let saving = |e: std::io::Error| format!("Couldn't save {}: {e}", file.display());
        let mut out = std::fs::File::create(&temporary).map_err(saving)?;
        out.write_all(bytes).map_err(saving)?;
        out.sync_all().map_err(saving)?;
        drop(out);
        let back = std::fs::read(&temporary).map_err(|e| format!("Couldn't check {}: {e}", file.display()))?;
        if back != bytes {
            return Err(format!("{} didn't save as written, so it's left as it was", file.display()));
        }
        if let Some(p) = problem(&back) {
            return Err(format!("{} came out wrong ({p}), so it's left as it was", file.display()));
        }
        std::fs::rename(&temporary, file).map_err(saving)
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
        // A cut never leaves a space at the end, and Windows' device names get a mark.
        assert_eq!(safe_file_name(&format!("{} b", "z".repeat(119))), "z".repeat(119));
        assert_eq!(safe_file_name("con"), "_con");
        assert_eq!(safe_file_name("COM1.final"), "_COM1.final");
        assert_eq!(safe_file_name("Console"), "Console");
        // A long name makes room for its number, so (2) isn't the first one's file.
        let long = Path::new("x.flp").with_file_name(format!("{}.flp", "y".repeat(130)));
        let second = bitwig_name(&long, |n| n.len() == 120 && !n.ends_with(')'));
        assert_eq!(second, format!("{} (2)", "y".repeat(116)));
        assert_ne!(bitwig_file(&long, Path::new("E"), &second), bitwig_file(&long, Path::new("E"), &"y".repeat(130)));
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
