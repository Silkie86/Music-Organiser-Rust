//! Exporting a song's sounds as preset files a plugin (or another DAW) can load, as the C# `Exporter.Presets` does:
//! into `<export folder>\<song's save name>\Presets`, each named "Plugin - Name (Where).ext". Exporting again writes
//! the same names over the last export, so exporting one plugin after another builds one folder without copies.
//!
//! Each sound's name is worked out among all its save's sounds, so it's the same whichever of them are exported: the
//! second of two "Serum" channels is "Serum 2.fxp" even when it's exported on its own, and never takes the first one's
//! file (the C# app named only what was exported, so it did).
//!
//! Each file is written beside its place under a name of its own, made sure of on the disk, read back and checked, then
//! renamed into place (copy, check, rename), so a half-written preset never sits under the real name. A preset that
//! can't be written is listed with why, and the others still go. Nothing here touches a project or a preset you made:
//! only the export folder is written to.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mo_formats::paths;
use mo_formats::presets::PresetBlob;

use crate::names;

use crate::files;
use crate::filter::Choice;
use crate::role::Thresholds;

/// A preset's file name without its extension: "Plugin - Name", or the name alone when it's the plugin's, then where
/// it sits in brackets ("Insert 3 Vocals"), made safe.
pub fn file_label(p: &PresetBlob) -> String {
    let mut label =
        if !p.plugin.is_empty() && p.plugin != p.name { format!("{} - {}", p.plugin, p.name) } else { p.name.clone() };
    if !p.place.is_empty() {
        label += &format!(" ({})", p.place.replace(" · ", " "));
    }
    names::safe_file_name(&label)
}

/// A preset's file ending, made safe as a name is: a Bitwig project names its plugin states' files itself.
fn safe_extension(ext: &str) -> String {
    let cleaned: String = ext.chars().map(|c| if names::unsafe_in_file_name(c) { '_' } else { c }).collect();
    cleaned.trim_end_matches([' ', '.']).to_owned()
}

/// Each preset's file name, in order: "label.ext", then "label 2.ext", "label 3.ext"... for later ones whose name is
/// taken. Letter case aside, as Windows and .NET's `OrdinalIgnoreCase` see names (both by capitals, so "ı" is "i").
pub fn file_names(presets: &[PresetBlob]) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    presets
        .iter()
        .map(|p| {
            let label = file_label(p);
            let ext = safe_extension(&p.extension);
            let mut name = format!("{label}{ext}");
            let mut n = 2;
            while !used.insert(name.to_uppercase()) {
                name = format!("{label} {n}{ext}");
                n += 1;
            }
            name
        })
        .collect()
}

/// What an export wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exported {
    pub folder: PathBuf,
    /// The files' names in the folder, in the order written.
    pub files: Vec<String>,
    /// What couldn't be exported, each with why ("Serum - Lead.fxp: ..."); everything else still was.
    pub failed: Vec<String>,
    /// Something off about the save the sounds came from, which was still read (an FL save cut short: rule 9).
    pub warning: Option<String>,
}

/// The folder a save's presets go to: `<root>\<save's name>\Presets`.
pub fn folder_for(export_root: &Path, save: &str) -> PathBuf {
    export_root.join(names::safe_file_name(paths::stem(paths::file_name(save)))).join("Presets")
}

/// Writes presets for the save at `save` (its path, for the folder's name) into the export folder. A preset that
/// can't be written is listed in [`Exported::failed`] and the rest still go; it's an error when none could be.
pub fn export(presets: &[PresetBlob], save: &str, export_root: &Path) -> io::Result<Exported> {
    write_all(presets.iter().zip(file_names(presets)).collect(), Vec::new(), save, export_root)
}

/// Writes each preset under its name, after `failed` (what was missed before writing). An export that wrote nothing
/// and missed something is an error.
fn write_all(
    chosen: Vec<(&PresetBlob, String)>,
    mut failed: Vec<String>,
    save: &str,
    export_root: &Path,
) -> io::Result<Exported> {
    if chosen.is_empty() && !failed.is_empty() {
        return Err(io::Error::other(failed.join("; ")));
    }
    let folder = folder_for(export_root, save);
    std::fs::create_dir_all(&folder)?;
    let mut files = Vec::new();
    for (p, name) in chosen {
        // Settings let go once listed (`SongPreset::forget_data`) are never written as an empty file over the last
        // export: a reader never gives a sound with none.
        let written = if p.data.is_empty() {
            Err(io::Error::other("its settings were let go after it was listed, so it's exported from its save"))
        } else {
            write_checked(&folder.join(&name), &p.data)
        };
        match written {
            Ok(()) => files.push(name),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    if files.is_empty() && !failed.is_empty() {
        return Err(io::Error::other(failed.join("; ")));
    }
    Ok(Exported { folder, files, failed, warning: None })
}

/// Writes `data` to `path` under a name of its own first, makes sure it's on the disk, reads it back, checks it's the
/// same (and, for a plugin preset, that it still reads as one), then renames it into place over any older export.
pub fn write_checked(path: &Path, data: &[u8]) -> io::Result<()> {
    // Its own name for each write (this copy of the app, then which write in it), so two exports at once never write
    // into one file and put the other's half-written bytes in place.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".part-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let temp = PathBuf::from(temp);
    let result = (|| {
        {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
            file.write_all(data)?;
            // On the disk before it takes the real name, so a power cut can't leave an empty file there.
            file.sync_all()?;
        }
        let back = std::fs::read(&temp)?;
        if back != data {
            return Err(io::Error::other("the file didn't read back as it was written"));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if files::FileFormat::of(&name).is_some()
            && let Err(e) = files::read_bytes(&back, &name)
        {
            return Err(io::Error::other(format!("the preset doesn't read back as one: {e}")));
        }
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Reads a save again and exports its sounds that `choice` lets through, or only those with these
/// keys ([`crate::list::place_keys`]) when `only` is given (one sound, or the rows picked on the page). Picked sounds that
/// aren't in the save any more (it was saved again since it was listed) are told in [`Exported::failed`].
pub fn export_from_save(
    save: &str,
    choice: Choice,
    only: Option<&[String]>,
    export_root: &Path,
) -> io::Result<Exported> {
    let raw = std::fs::read(save)?;
    // Read as the page lists it, so the keys match; a damaged save gives nothing to write.
    let (sounds, warning) = crate::library::sounds_in(save, raw, &Thresholds::default()).map_err(io::Error::other)?;
    let keys = crate::list::place_keys(&sounds);
    let blobs: Vec<PresetBlob> = sounds.into_iter().map(|s| s.blob).collect();
    // Named among all the save's sounds, so each keeps its name whichever are exported.
    let names = file_names(&blobs);
    let wanted: Option<HashSet<&str>> = only.map(|keys| keys.iter().map(String::as_str).collect());
    let mut failed = Vec::new();
    if let Some(wanted) = &wanted {
        let found: HashSet<&str> = keys.iter().map(String::as_str).collect();
        let gone = wanted.iter().filter(|k| !found.contains(*k)).count();
        if gone > 0 {
            failed.push(format!(
                "{gone} of the sounds picked {} in the save any more (was it saved again since?)",
                if gone == 1 { "isn't" } else { "aren't" }
            ));
        }
    }
    let chosen: Vec<(&PresetBlob, String)> = blobs
        .iter()
        .zip(names)
        .zip(&keys)
        .filter(|((blob, _), key)| match &wanted {
            Some(wanted) => wanted.contains(key.as_str()),
            None => choice.matches_blob(blob),
        })
        .map(|(chosen, _)| chosen)
        .collect();
    let mut exported = write_all(chosen, failed, save, export_root)?;
    exported.warning = warning.map(str::to_owned);
    Ok(exported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_plugin_name_and_place() {
        let mut p = PresetBlob::new("Wobble", "Serum", ".fxp", vec![]);
        assert_eq!(file_label(&p), "Serum - Wobble");
        p.place = "Insert 3 · Vocals".into();
        assert_eq!(file_label(&p), "Serum - Wobble (Insert 3 Vocals)");
        let q = PresetBlob::new("Pro-Q 3", "Pro-Q 3", ".fxp", vec![]);
        assert_eq!(file_label(&q), "Pro-Q 3");
    }

    #[test]
    fn names_are_numbered_letter_case_aside_and_endings_made_safe() {
        let blob = |name: &str, ext: &'static str| PresetBlob::new(name, "", ext, vec![]);
        let names = file_names(&[blob("Pad", ".fxp"), blob("PAD", ".FXP"), blob("Odd", ".fx:p"), blob("kır", ".fxp")]);
        assert_eq!(names, ["Pad.fxp", "PAD 2.FXP", "Odd.fx_p", "kır.fxp"]);
        // Windows and .NET take "ı" and "i" for one letter once in capitals.
        assert_eq!(file_names(&[blob("kır", ".fxp"), blob("kir", ".fxp")]), ["kır.fxp", "kir 2.fxp"]);
    }
}
